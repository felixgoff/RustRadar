//! The settings file on disk (plan 1.2, 2.3; doc 06 §23): `settings.json` in the config folder,
//! beside `keys.json`, holding the one JSON object today's app kept in the web view's
//! localStorage under `rustradar.v1` (`filters.ts:200-230`).
//!
//! - [`load`] reads it at start. A missing file, one that cannot be read, or one that is not the
//!   JSON object of doc 06 §23 gives the defaults; a readable one is merged part by part with the
//!   rules of doc 06 §23 ([`rustradar_core::settings::load_persisted`]).
//! - [`write()`] replaces it atomically: the text goes to a temporary file in the same folder, is
//!   flushed to the disk, and is renamed over the old file, so a crash or a full disk mid-write
//!   leaves the old settings whole (`persist`'s `localStorage.setItem` could not half-write either).
//! - [`Writer`] does the writing on a thread of its own, so the frame loop never waits on the disk
//!   (plan 2.1); a burst of saves writes only the newest text.
//!
//! The text is what `JSON.stringify` writes ([`rustradar_core::settings::persisted_json`]). When
//! the settings are saved, and what is written while `--basemap` or a canned session is on, is
//! the settings store's business ([`crate::state::settings`]).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use rustradar_backend::Dirs;
use rustradar_core::settings::{Persisted, load_persisted};

/// The file's name in the config folder.
pub const FILE: &str = "settings.json";

/// How long after the last change the settings are written (plan 2.3).
pub const SAVE_DELAY_MS: f64 = 500.0;

/// Where the settings file lives: `<config>/settings.json`.
pub fn path(dirs: &Dirs) -> PathBuf {
    dirs.config_file(FILE)
}

/// What [`load`] found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Loaded {
    /// The settings and filters, the defaults wherever the file had nothing usable.
    pub persisted: Persisted,
    /// The file's text as it was read; `None` when there is no file or it could not be read.
    pub text: Option<String>,
}

/// Reads the settings file at `path` (`loadPersisted`). No file gives the defaults; so does a file
/// that cannot be read (logged) or whose text is not a JSON object.
pub fn load(path: &Path) -> Loaded {
    match fs::read_to_string(path) {
        Ok(text) => Loaded { persisted: load_persisted(Some(&text)), text: Some(text) },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Loaded::default(),
        Err(e) => {
            // a folder in its place, no permission, bytes that are not UTF-8: as unreadable JSON
            log::warn!("couldn't read {}, using the default settings: {e}", path.display());
            Loaded::default()
        }
    }
}

/// Writes `text` as the settings file at `path`, atomically: into a temporary file beside it
/// (`.settings.json.<process>.<n>.tmp`), synced, then renamed over it. The folder is made if it
/// is missing. On an error the temporary file is removed and the old file is left as it was.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    static WRITES: AtomicUsize = AtomicUsize::new(0);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path.file_name().map_or_else(|| FILE.into(), |n| n.to_string_lossy());
    // one name per process and write: two windows, or two writers in one process, never share one
    let n = WRITES.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
    let written = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // replaces the old file in one step, on Windows too (MoveFileEx with REPLACE_EXISTING)
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// What the writer's thread is asked to do.
enum Job {
    /// Write this text (the newest of a burst wins).
    Save(String),
    /// Answer once everything sent before has been written.
    Flush(Sender<()>),
}

/// Writes the settings file on a thread of its own ([`write()`] for each save, the newest of a
/// burst only). Dropping it waits for the writes already sent.
pub struct Writer {
    path: PathBuf,
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Writer {
    /// A writer for the file at `path`. Without a thread (the system refused one) each save is
    /// written at once on the caller's.
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let target = path.clone();
        match thread::Builder::new().name("settings-writer".into()).spawn(move || run(&target, rx)) {
            Ok(thread) => Writer { path, jobs: Some(tx), thread: Some(thread) },
            Err(e) => {
                log::warn!("no thread for the settings file, writing it in the frame loop: {e}");
                Writer { path, jobs: None, thread: None }
            }
        }
    }

    /// The file it writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `text` as the file, soon.
    pub fn save(&self, text: String) {
        match &self.jobs {
            Some(jobs) => {
                if let Err(mpsc::SendError(Job::Save(text))) = jobs.send(Job::Save(text)) {
                    // the thread has gone (it never panics on its own): write here
                    save_now(&self.path, &text);
                }
            }
            None => save_now(&self.path, &text),
        }
    }

    /// Waits until every save sent so far is on the disk (or has failed).
    pub fn flush(&self) {
        let Some(jobs) = &self.jobs else { return };
        let (done, wait) = mpsc::channel();
        if jobs.send(Job::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // the channel closed: the thread writes what it was sent and ends
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The writer's thread: each round takes everything queued, writes the newest text, then answers
/// the flushes of that round.
fn run(path: &Path, jobs: Receiver<Job>) {
    while let Ok(first) = jobs.recv() {
        let mut text = None;
        let mut flushes = Vec::new();
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            match job {
                Job::Save(t) => text = Some(t),
                Job::Flush(done) => flushes.push(done),
            }
        }
        if let Some(text) = text {
            save_now(path, &text);
        }
        for done in flushes {
            let _ = done.send(());
        }
    }
}

/// [`write()`], a failure logged: as today, settings that cannot be stored just won't survive a
/// restart.
fn save_now(path: &Path, text: &str) {
    if let Err(e) = write(path, text) {
        log::warn!("couldn't save the settings to {}: {e}", path.display());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_core::settings::{Mode, persisted_json};

    use super::*;

    /// A folder of the test's own, removed when it ends.
    pub(crate) struct TempDir(pub PathBuf);

    impl TempDir {
        pub(crate) fn new(name: &str) -> Self {
            static DIRS: AtomicUsize = AtomicUsize::new(0);
            let n = DIRS.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("rustradar-persist-{name}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp folder");
            TempDir(dir)
        }

        /// The files in the folder, sorted.
        pub(crate) fn files(&self) -> Vec<String> {
            let mut names: Vec<String> =
                fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn changed() -> Persisted {
        let mut p = Persisted::default();
        p.settings.mode = Mode::Ships;
        p.settings.basemap = Basemap::Satellite;
        p.settings.label_size = 1.25;
        p.filters.text = "SAS".into();
        p
    }

    #[test]
    fn the_file_sits_in_the_config_folder_beside_the_keys() {
        let dirs = Dirs { cache: "/c".into(), config: "/cfg/com.felix.rustradar".into(), downloads: None };
        assert_eq!(path(&dirs), PathBuf::from("/cfg/com.felix.rustradar/settings.json"));
        assert_eq!(path(&dirs).parent(), dirs.config_file("keys.json").parent());
        assert_eq!(SAVE_DELAY_MS, 500.0);
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = TempDir::new("missing");
        assert_eq!(load(&dir.0.join(FILE)), Loaded { persisted: Persisted::default(), text: None });
        // nor does reading make one
        assert!(dir.files().is_empty());
        // a missing folder too
        assert_eq!(load(&dir.0.join("gone").join(FILE)).persisted, Persisted::default());
    }

    #[test]
    fn a_corrupt_file_gives_the_defaults() {
        let dir = TempDir::new("corrupt");
        let file = dir.0.join(FILE);
        let truncated = &persisted_json(&changed())[..40];
        for text in ["", "{", "not json", "null", "[1, 2]", "\"settings\"", truncated] {
            fs::write(&file, text).unwrap();
            let loaded = load(&file);
            assert_eq!(loaded.persisted, Persisted::default(), "{text:?}");
            assert_eq!(loaded.text.as_deref(), Some(text), "the text is kept, so the next save replaces it");
        }
        // bytes that are not UTF-8 cannot be read as text at all
        fs::write(&file, [0xff, 0xfe, b'{', 0x80]).unwrap();
        assert_eq!(load(&file), Loaded::default());
        // a folder where the file should be
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        assert_eq!(load(&file), Loaded::default());
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let dir = TempDir::new("round-trip");
        let file = dir.0.join(FILE);
        let text = persisted_json(&changed());
        write(&file, &text).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), text);
        assert_eq!(load(&file), Loaded { persisted: changed(), text: Some(text) });
        assert_eq!(dir.files(), [FILE], "no temporary file is left behind");
    }

    #[test]
    fn a_write_replaces_the_old_file_whole_and_makes_its_folder() {
        let dir = TempDir::new("replace");
        let file = dir.0.join("config").join(FILE);
        write(&file, &persisted_json(&changed())).unwrap();
        let shorter = persisted_json(&Persisted::default());
        write(&file, &shorter).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), shorter);
        // a temporary file a crash left behind changes nothing
        fs::write(dir.0.join("config").join(".settings.json.1.0.tmp"), "{").unwrap();
        write(&file, &persisted_json(&changed())).unwrap();
        assert_eq!(load(&file).persisted, changed());
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary_one() {
        let dir = TempDir::new("failed");
        // the target is a folder that is not empty: the rename fails
        let file = dir.0.join(FILE);
        fs::create_dir_all(file.join("inside")).unwrap();
        assert!(write(&file, "{}").is_err());
        assert_eq!(dir.files(), [FILE]);
        assert!(file.join("inside").is_dir());
    }

    #[test]
    fn the_writer_writes_the_newest_text_off_the_caller_s_thread() {
        let dir = TempDir::new("writer");
        let file = dir.0.join(FILE);
        let writer = Writer::new(file.clone());
        assert_eq!(writer.path(), file);
        let mut p = Persisted::default();
        for size in [0.85, 1.25, 1.5] {
            p.settings.label_size = size;
            writer.save(persisted_json(&p));
        }
        writer.flush();
        assert_eq!(load(&file).persisted.settings.label_size, 1.5);
        // dropping it waits for what was sent
        writer.save(persisted_json(&changed()));
        drop(writer);
        assert_eq!(load(&file).persisted, changed());
        assert_eq!(dir.files(), [FILE]);
    }

    #[test]
    fn a_writer_that_cannot_write_carries_on() {
        let dir = TempDir::new("cannot");
        let file = dir.0.join(FILE);
        fs::create_dir_all(file.join("inside")).unwrap();
        let writer = Writer::new(file.clone());
        writer.save("{}".into());
        writer.flush();
        // the failure is logged; the next save still runs
        fs::remove_dir_all(&file).unwrap();
        writer.save(persisted_json(&changed()));
        writer.flush();
        assert_eq!(load(&file).persisted, changed());
    }
}
