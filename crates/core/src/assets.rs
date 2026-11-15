//! The files the app reads at run time (plan 1.6, risk K14), named in one place: the realistic
//! map's textures, the aircraft and train models and atlases, the street trees, the rail
//! overview, the fonts and the backend's rail data, 29.6 MB in all.
//!
//! Every crate reads them through the [`File`]s here, each known by its `key`: its path under
//! its folder of the repository (`root`: `static`, `assets` or `crates/backend`), which is also
//! its path in the Android APK's `assets/` folder.
//!
//! - **Desktop and iOS**: each [`File`] is the file's bytes, embedded with `include_bytes!` (plan
//!   1.6, section 7's "Assets" line), as before.
//! - **Android** (this crate's feature `apk-assets`, which `rustradar-app` turns on for its Android
//!   build only): nothing is embedded. The bytes were compiled into each of the APK's four
//!   libraries (the release APK was 220 MB, over K14's 150 MB trigger), so the APK now carries the
//!   files once, in `assets/`, stored uncompressed (`cargo xtask android-assets` copies exactly
//!   [`FILES`] there; `platform/android/build-rust.sh` runs it for both the debug and the release
//!   APK, and `cargo xtask android-release` checks the APK against [`FILES`]). The app reads every
//!   one from the APK through `AAssetManager` in `android_main`, before anything draws, and hands
//!   them over with [`install`] (a file the APK lacks stops the app there, with every missing name
//!   in the log). After that a [`File`] reads as on the other platforms.
//!
//! A feature, not `target_os = "android"`: the Tauri shim (`src-tauri`) links this crate through
//! the backend and builds for Android too; without `rustradar-app` it keeps the files embedded.
//!
//! What stays embedded everywhere, being code or a few kilobytes the UI draws at once: the WGSL
//! shaders (`crates/map/src/shaders`, 0.16 MB), the Lucide icons and the two licence texts
//! (`crates/app/src/assets.rs`, 0.03 MB). `cargo xtask`'s tests refuse any other
//! `include_bytes!`/`include_str!` in the libraries' sources.

#[cfg(feature = "apk-assets")]
use std::sync::OnceLock;

/// One file the app reads: its key, its folder in the repository and its bytes.
pub struct File {
    /// Its folder in the repository: `static`, `assets` or `crates/backend`.
    pub root: &'static str,
    /// Its path under [`Self::root`], and in the APK's `assets/`.
    pub key: &'static str,
    #[cfg(not(feature = "apk-assets"))]
    bytes: &'static [u8],
    #[cfg(feature = "apk-assets")]
    bytes: OnceLock<&'static [u8]>,
}

impl File {
    /// The file's bytes: embedded, or (Android) as [`install`] read them from the APK.
    ///
    /// # Panics
    /// On Android, before [`install`] (the app installs the files first thing in `android_main`).
    pub fn bytes(&self) -> &'static [u8] {
        #[cfg(not(feature = "apk-assets"))]
        {
            self.bytes
        }
        #[cfg(feature = "apk-assets")]
        {
            match self.bytes.get() {
                Some(bytes) => bytes,
                None => panic!("{} was read before rustradar_core::assets::install (android_main installs the APK's files)", self.key),
            }
        }
    }

    /// The file as text (the backend's JSON).
    ///
    /// # Panics
    /// If it is not UTF-8 (a test reads every text file), and as [`Self::bytes`].
    pub fn text(&self) -> &'static str {
        match std::str::from_utf8(self.bytes()) {
            Ok(text) => text,
            Err(e) => panic!("{} is not UTF-8: {e}", self.key),
        }
    }

    /// Its path in the repository, from the root (`static/realistic/ground.color.ktx2`).
    pub fn path(&self) -> String {
        format!("{}/{}", self.root, self.key)
    }
}

impl std::ops::Deref for File {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.bytes()
    }
}

impl std::fmt::Debug for File {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("File").field("root", &self.root).field("key", &self.key).finish_non_exhaustive()
    }
}

/// A file of [`FILES`]: where it is in the repository and in the APK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Its folder in the repository.
    pub root: &'static str,
    /// Its path under `root`, and in the APK's `assets/`.
    pub key: &'static str,
}

impl Entry {
    /// Its path in the repository, from the root.
    pub fn path(&self) -> String {
        format!("{}/{}", self.root, self.key)
    }
}

#[cfg(not(feature = "apk-assets"))]
macro_rules! asset_file {
    ($root:literal, $key:literal) => {
        File { root: $root, key: $key, bytes: include_bytes!(concat!("../../../", $root, "/", $key)) }
    };
}

#[cfg(feature = "apk-assets")]
macro_rules! asset_file {
    ($root:literal, $key:literal) => {
        File { root: $root, key: $key, bytes: OnceLock::new() }
    };
}

/// The one list: a `pub static` [`File`] for each, [`FILES`] (the keys alone, for the APK's
/// build: no bytes), and `ALL` (the statics, for [`install`] and the tests).
macro_rules! files {
    ($($root:literal { $($(#[$doc:meta])* $name:ident = $key:literal;)* })*) => {
        $($(
            $(#[$doc])*
            pub static $name: File = asset_file!($root, $key);
        )*)*

        /// Every file, in the list's order: what the APK's `assets/` holds, exactly.
        pub const FILES: &[Entry] = &[$($(Entry { root: $root, key: $key }),*),*];

        #[cfg(any(test, feature = "apk-assets"))]
        static ALL: &[&File] = &[$($(&$name),*),*];
    };
}

files! {
    "static" {
        /// The realistic map's textures (doc 04 §3; Basis UASTC KTX2, the textures' own
        /// supercompression, so deflate gains nothing on them).
        BUILDING_COLOR = "realistic/building.color.ktx2";
        BUILDING_NORMAL = "realistic/building.normal.ktx2";
        BUILDING_MASK = "realistic/building.mask.ktx2";
        BUILDING_GLOW = "realistic/building.glow.ktx2";
        GROUND_COLOR = "realistic/ground.color.ktx2";
        GROUND_NORMAL = "realistic/ground.normal.ktx2";
        TREE_COLOR = "realistic/tree.color.ktx2";
        TREE_NORMAL = "realistic/tree.normal.ktx2";
        TREE_MASK = "realistic/tree.mask.ktx2";
        WATER_NORMAL = "realistic/water.normal.ktx2";
        EARTH_COLOR = "realistic/earth.color.ktx2";
        TREE_VOLUME_NORMAL = "realistic/tree-volume.normal.ktx2";
        EARTH_NIGHT = "realistic/earth.night.ktx2";
        /// The published aircraft: models, lookups and atlas layout; meshes; the silhouettes'
        /// and borders' atlases (doc 03 §5).
        AIRCRAFT_INDEX = "aircraft/index.json";
        AIRCRAFT_MESHES = "aircraft/meshes.bin";
        AIRCRAFT_ICONS = "aircraft/icons.ktx2";
        AIRCRAFT_OUTLINE = "aircraft/icons-outline.ktx2";
        /// The train cars' models (doc 02 §5).
        TRAIN_INDEX = "trains/index.json";
        TRAIN_MESHES = "trains/meshes.bin";
        /// The towns' registered street trees (doc 04 §8).
        STREET_TREES_MALMO = "street-trees/malmo.bin";
        STREET_TREES_UMEA = "street-trees/umea.bin";
        /// The rail overview at low zoom (doc 02 §8).
        RAILROADS = "rail/railroads.bin";
    }
    "assets" {
        /// Overpass, the variable `wght` font, by subset (plan 1.5; `cargo xtask vendor-fonts`).
        OVERPASS_LATIN = "fonts/overpass-latin.ttf";
        OVERPASS_LATIN_EXT = "fonts/overpass-latin-ext.ttf";
        OVERPASS_CYRILLIC = "fonts/overpass-cyrillic.ttf";
        OVERPASS_CYRILLIC_EXT = "fonts/overpass-cyrillic-ext.ttf";
        OVERPASS_VIETNAMESE = "fonts/overpass-vietnamese.ttf";
        /// Overpass Mono, the same subsets.
        OVERPASS_MONO_LATIN = "fonts/overpass-mono-latin.ttf";
        OVERPASS_MONO_LATIN_EXT = "fonts/overpass-mono-latin-ext.ttf";
        OVERPASS_MONO_CYRILLIC = "fonts/overpass-mono-cyrillic.ttf";
        OVERPASS_MONO_CYRILLIC_EXT = "fonts/overpass-mono-cyrillic-ext.ttf";
        OVERPASS_MONO_VIETNAMESE = "fonts/overpass-mono-vietnamese.ttf";
        /// Overpass's latin subset with tabular figures (item F1).
        OVERPASS_TNUM_LATIN = "fonts/overpass-tnum-latin.ttf";
    }
    "crates/backend" {
        /// Trafikverket's track speed limits (`backend::sth`).
        STH = "data/sth.json";
        /// Trafikverket's traffic flow directions (`backend::flow`).
        FLOW = "data/flow.json";
    }
}

/// What [`install`] (or [`load`]) read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loaded {
    /// Files read (all of [`FILES`]).
    pub files: usize,
    /// Their bytes.
    pub bytes: usize,
}

/// Reads every file of `entries` with `read` (its key in, its bytes or why not out), all of
/// them even after a failure, so the error names every file that could not be read.
pub fn load(
    entries: &[Entry],
    mut read: impl FnMut(&Entry) -> Result<&'static [u8], String>,
) -> Result<Vec<&'static [u8]>, String> {
    let mut found = Vec::with_capacity(entries.len());
    let mut missing = Vec::new();
    for entry in entries {
        match read(entry) {
            Ok(bytes) => found.push(bytes),
            Err(why) => missing.push(format!("{} ({why})", entry.key)),
        }
    }
    if missing.is_empty() {
        Ok(found)
    } else {
        Err(format!(
            "the app's files are missing: {} of {} could not be read: {}",
            missing.len(),
            entries.len(),
            missing.join(", ")
        ))
    }
}

/// Android: hands the app every file of [`FILES`], read with `read` (from the APK's `assets/`
/// through `AAssetManager`; the bytes must stay valid for the process, as embedded ones are).
/// Once per process: a second call (a new activity in the same process) reads nothing.
///
/// # Errors
/// Every file `read` could not give, by key; nothing is installed then.
#[cfg(feature = "apk-assets")]
pub fn install(read: impl FnMut(&Entry) -> Result<&'static [u8], String>) -> Result<Loaded, String> {
    static DONE: OnceLock<Loaded> = OnceLock::new();
    if let Some(done) = DONE.get() {
        return Ok(*done);
    }
    let found = load(FILES, read)?;
    let mut loaded = Loaded { files: 0, bytes: 0 };
    for (file, bytes) in ALL.iter().zip(found) {
        // a file set already (by a call racing this one) keeps its bytes
        let _ = file.bytes.set(bytes);
        loaded.files += 1;
        loaded.bytes += bytes.len();
    }
    Ok(*DONE.get_or_init(|| loaded))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use super::*;

    fn repo(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
    }

    /// The test process's files: embedded, or (feature `apk-assets`) installed from the
    /// repository's files, as the app installs them from the APK.
    fn ready() {
        #[cfg(feature = "apk-assets")]
        install(|e| std::fs::read(repo(&e.path())).map(|b| &*Vec::leak(b)).map_err(|e| e.to_string())).expect("installed");
    }

    #[test]
    fn the_list_names_each_file_once_by_a_plain_relative_key() {
        assert_eq!(FILES.len(), ALL.len());
        assert_eq!(FILES.len(), 35);
        let mut keys = HashSet::new();
        for (entry, file) in FILES.iter().zip(ALL) {
            assert_eq!((entry.root, entry.key), (file.root, file.key));
            assert!(keys.insert(entry.key), "{} is listed twice", entry.key);
            assert!(["static", "assets", "crates/backend"].contains(&entry.root), "{}", entry.root);
            let parts: Vec<&str> = entry.key.split('/').collect();
            assert!(parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".."), "{}", entry.key);
            assert!(entry.key.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b)), "{}", entry.key);
            assert_eq!(entry.path(), file.path());
        }
    }

    #[test]
    fn every_file_reads_as_the_repositorys() {
        ready();
        let mut total = 0;
        for &file in ALL {
            let on_disk = std::fs::read(repo(&file.path())).unwrap_or_else(|e| panic!("{}: {e}", file.path()));
            assert!(file.bytes() == on_disk.as_slice(), "{} differs from the repository's", file.key);
            assert_eq!(&**file, file.bytes());
            total += on_disk.len();
        }
        // plan 1.6's 27 MB of static/, the fonts and the backend's data
        assert!((29_000_000..31_000_000).contains(&total), "{total}");
        assert!(STH.text().starts_with('{') && FLOW.text().starts_with('{'));
        assert_eq!(&GROUND_COLOR[..4], &[0xab, 0x4b, 0x54, 0x58]);
        assert_eq!(&OVERPASS_LATIN[..4], &[0, 1, 0, 0]);
    }

    #[test]
    fn a_load_names_every_file_it_cannot_read_and_reads_them_all() {
        let entries = &FILES[..4];
        let mut asked = Vec::new();
        let err = load(entries, |e| {
            asked.push(e.key);
            if e.key.contains("normal") || e.key.contains("glow") { Err("not in the APK".into()) } else { Ok(b"x") }
        })
        .unwrap_err();
        assert_eq!(asked, entries.iter().map(|e| e.key).collect::<Vec<_>>());
        assert_eq!(
            err,
            "the app's files are missing: 2 of 4 could not be read: realistic/building.normal.ktx2 (not in the APK), \
             realistic/building.glow.ktx2 (not in the APK)"
        );
        let found = load(entries, |e| Ok(e.key.as_bytes())).unwrap();
        assert_eq!(found, entries.iter().map(|e| e.key.as_bytes()).collect::<Vec<_>>());
    }

    #[cfg(feature = "apk-assets")]
    #[test]
    fn installed_files_read_as_embedded_ones_did_and_a_second_install_reads_nothing() {
        ready();
        let again = install(|_| panic!("read again")).unwrap();
        assert_eq!(again.files, FILES.len());
        assert_eq!(again.bytes, ALL.iter().map(|f| f.bytes().len()).sum::<usize>());
    }
}
