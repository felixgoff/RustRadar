//! `cargo xtask android-assets`: the files the Android library reads from its APK instead of
//! carrying them (plan risk K14; [`rustradar_core::assets`]).
//!
//! ```text
//! cargo xtask android-assets DIR           copy them into DIR, the Gradle project's assets
//! cargo xtask android-assets --check APK   check an APK's assets/ against them
//! ```
//!
//! Until this, every file of `rustradar_core::assets` was compiled into each of the APK's four
//! libraries: 29.6 MB four times, and the release APK came to 220 MB (CI run 326), over K14's
//! 150 MB trigger. Now the Android build (`rustradar-core`'s feature `apk-assets`, on for
//! `rustradar` on Android) embeds none of them, and the APK carries each once, in `assets/`:
//!
//! - **Copy** ([`copy`]): exactly [`FILES`], each from `<root>/<key>` in the repository to
//!   `DIR/<key>`, and nothing else (a file in DIR the list lacks is removed, so a file taken off
//!   the list leaves the APK too). `platform/android/build-rust.sh` runs it into
//!   `platform/android/app/src/main/assets` (Gradle's own assets folder, ignored by git, like the
//!   libraries in `jniLibs`) after building the library, for the debug APK and the release one.
//! - **Stored, not deflated**: `app/build.gradle.kts` keeps them uncompressed (`noCompress`), so
//!   `AAssetManager` maps them from the APK as the library's own data was mapped: no inflating at
//!   start, no copy in memory. Deflate would save 4.4 MB of the 29.6 (the KTX2 textures, 21.6 MB,
//!   are supercompressed already; the meshes, JSON and fonts shrink) and cost both.
//! - **Check** ([`check_apk`]): the APK's `assets/` holds exactly [`FILES`] (and Gradle's own
//!   baseline profiles under `dexopt/`), each stored and the repository's file byte for byte (size
//!   and CRC-32). `cargo xtask android-release` runs it on
//!   the release APK, CI's phone shell job on the debug one: an asset the app would miss at start
//!   fails the build instead.
//!
//! The list itself is the one in `rustradar_core::assets`, the same macro that embeds the files on
//! the other platforms, so the two cannot drift; the tests here also refuse an `include_bytes!` or
//! `include_str!` of a file anywhere else in the libraries' sources but the few kept embedded on
//! purpose (their `KEPT_EMBEDDED`: the WGSL shaders, the Lucide icons and the two licences, under
//! 0.2 MB).

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rustradar_core::assets::{Entry, FILES};

use crate::bump;

/// What a copy or a check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Files of the list.
    pub files: usize,
    /// Their bytes.
    pub bytes: u64,
    /// Files written (a copy; files already equal are left alone, keeping Gradle's up-to-date check).
    pub written: usize,
    /// Files removed that the list lacks (a copy).
    pub removed: usize,
}

/// `cargo xtask android-assets DIR` or `--check APK`.
pub fn run(args: &[String]) -> Result<(), String> {
    let root = bump::repo_root();
    match args {
        [flag, apk] if flag == "--check" => {
            let s = check_apk(&root, Path::new(apk))?;
            println!("{apk}: assets/ holds the {} files, {:.1} MB, stored, as the repository's", s.files, s.bytes as f64 / 1e6);
            Ok(())
        }
        [dir] if !dir.starts_with('-') => {
            let s = copy(&root, Path::new(dir))?;
            println!(
                "{dir}: {} files, {:.1} MB ({} written, {} removed)",
                s.files,
                s.bytes as f64 / 1e6,
                s.written,
                s.removed
            );
            Ok(())
        }
        _ => Err("usage: cargo xtask android-assets DIR | --check APK".into()),
    }
}

fn source(root: &Path, entry: &Entry) -> PathBuf {
    root.join(entry.root).join(entry.key)
}

/// Every file under `dir`, by its path relative to it with `/` between the parts.
fn files_under(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<(), String> {
    let Ok(read) = fs::read_dir(dir) else { return Ok(()) };
    for item in read {
        let item = item.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = item.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        if item.file_type().map_err(|e| format!("{}: {e}", item.path().display()))?.is_dir() {
            files_under(&item.path(), &rel, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// Removes the folders under `dir` (not `dir`) that hold nothing.
fn remove_empty_dirs(dir: &Path) {
    for item in fs::read_dir(dir).into_iter().flatten().flatten() {
        if item.file_type().is_ok_and(|t| t.is_dir()) {
            remove_empty_dirs(&item.path());
            let _ = fs::remove_dir(item.path());
        }
    }
}

/// Copies exactly [`FILES`] from the repository at `root` into `dir` (see the module's docs).
pub fn copy(root: &Path, dir: &Path) -> Result<Summary, String> {
    let keys: BTreeSet<&str> = FILES.iter().map(|e| e.key).collect();
    let mut present = Vec::new();
    files_under(dir, "", &mut present)?;
    let mut summary = Summary { files: 0, bytes: 0, written: 0, removed: 0 };
    for stray in present.iter().filter(|p| !keys.contains(p.as_str())) {
        let path = dir.join(stray);
        fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        summary.removed += 1;
    }
    remove_empty_dirs(dir);
    for entry in FILES {
        let from = source(root, entry);
        let bytes = fs::read(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        let to = dir.join(entry.key);
        if fs::read(&to).ok().as_deref() != Some(bytes.as_slice()) {
            fs::create_dir_all(to.parent().unwrap_or(dir)).map_err(|e| format!("{}: {e}", to.display()))?;
            fs::write(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
            summary.written += 1;
        }
        summary.files += 1;
        summary.bytes += bytes.len() as u64;
    }
    Ok(summary)
}

/// One entry of a zip's central directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    /// 0 stored, 8 deflated.
    pub method: u16,
    pub crc: u32,
    pub size: u64,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The entries of the zip archive at `path` (an APK), from its central directory.
pub fn zip_entries(path: &Path) -> Result<Vec<ZipEntry>, String> {
    let what = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let mut file = fs::File::open(path).map_err(|e| what(&e))?;
    let len = file.metadata().map_err(|e| what(&e))?.len();
    // the end record: 22 bytes and a comment of at most 65535
    let tail_len = len.min(22 + 65_535);
    let mut tail = vec![0; tail_len as usize];
    file.seek(SeekFrom::Start(len - tail_len)).and_then(|_| file.read_exact(&mut tail)).map_err(|e| what(&e))?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(&tail, i) == Some(0x0605_4b50))
        .ok_or_else(|| what(&"not a zip archive (no end of central directory)"))?;
    let (count, cd_size, cd_offset) = (u16_at(&tail, end + 10), u32_at(&tail, end + 12), u32_at(&tail, end + 16));
    let (Some(count), Some(cd_size), Some(cd_offset)) = (count, cd_size, cd_offset) else {
        return Err(what(&"a truncated end record"));
    };
    if count == u16::MAX || cd_offset == u32::MAX || cd_size == u32::MAX {
        return Err(what(&"a zip64 archive (not read here)"));
    }
    let mut cd = vec![0; cd_size as usize];
    file.seek(SeekFrom::Start(u64::from(cd_offset))).and_then(|_| file.read_exact(&mut cd)).map_err(|e| what(&e))?;
    let mut entries = Vec::with_capacity(count.into());
    let mut p = 0;
    for _ in 0..count {
        let bad = || what(&format!("a broken central directory entry at {p}"));
        if u32_at(&cd, p) != Some(0x0201_4b50) {
            return Err(bad());
        }
        let method = u16_at(&cd, p + 10).ok_or_else(bad)?;
        let crc = u32_at(&cd, p + 16).ok_or_else(bad)?;
        let size = u32_at(&cd, p + 24).ok_or_else(bad)?;
        let n = usize::from(u16_at(&cd, p + 28).ok_or_else(bad)?);
        let extra = usize::from(u16_at(&cd, p + 30).ok_or_else(bad)?);
        let comment = usize::from(u16_at(&cd, p + 32).ok_or_else(bad)?);
        let name = cd.get(p + 46..p + 46 + n).ok_or_else(bad)?;
        entries.push(ZipEntry { name: String::from_utf8_lossy(name).into_owned(), method, crc, size: u64::from(size) });
        p += 46 + n + extra + comment;
    }
    Ok(entries)
}

/// What Gradle itself puts in a release APK's `assets/`: ART's baseline profiles
/// (`dexopt/baseline.prof` and `.profm`, from androidx's profile installer), as in v0.3.17's
/// dry-run APK before the app's files moved there.
const GRADLES_OWN: &str = "assets/dexopt/";

/// The CRC-32 of `bytes` (the zip format's).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(bytes);
    crc.sum()
}

/// Checks that the APK's `assets/` holds exactly [`FILES`], each stored and equal to the
/// repository's file at `root` (see the module's docs).
pub fn check_apk(root: &Path, apk: &Path) -> Result<Summary, String> {
    let entries = zip_entries(apk)?;
    let assets: Vec<&ZipEntry> = entries
        .iter()
        .filter(|e| e.name.starts_with("assets/") && !e.name.ends_with('/') && !e.name.starts_with(GRADLES_OWN))
        .collect();
    let mut problems = Vec::new();
    let mut summary = Summary { files: 0, bytes: 0, written: 0, removed: 0 };
    for entry in FILES {
        let name = format!("assets/{}", entry.key);
        let Some(found) = assets.iter().find(|e| e.name == name) else {
            problems.push(format!("{name} is missing"));
            continue;
        };
        let from = source(root, entry);
        let bytes = fs::read(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        if found.method != 0 {
            problems.push(format!("{name} is compressed (method {}), not stored", found.method));
        }
        if found.size != bytes.len() as u64 || found.crc != crc32(&bytes) {
            problems.push(format!("{name} is not {} ({} bytes, not {})", entry.path(), found.size, bytes.len()));
        }
        summary.files += 1;
        summary.bytes += bytes.len() as u64;
    }
    for extra in assets.iter().filter(|e| !FILES.iter().any(|f| e.name == format!("assets/{}", f.key))) {
        problems.push(format!("{} is not one of the app's files", extra.name));
    }
    if problems.is_empty() {
        Ok(summary)
    } else {
        Err(format!("{}: {}", apk.display(), problems.join("; ")))
    }
}

/// A zip of stored entries (a stand-in APK for the tests).
#[cfg(test)]
pub fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in entries {
        let offset = out.len() as u32;
        let (crc, size, n) = (crc32(data), data.len() as u32, name.len() as u16);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        for v in [20u16, 0, 0, 0, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, size, size] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        for v in [20u16, 20, 0, 0, 0, 0] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, size, size] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [n, 0, 0, 0, 0] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [0u32, offset] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        central.extend_from_slice(name.as_bytes());
    }
    let (start, len, count) = (out.len() as u32, central.len() as u32, entries.len() as u16);
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    for v in [0u16, 0, count, count] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in [len, start] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// The app's files as a stand-in APK holds them: `assets/<key>`, the repository's bytes.
#[cfg(test)]
pub fn apk_assets() -> Vec<(String, Vec<u8>)> {
    let root = bump::repo_root();
    FILES.iter().map(|e| (format!("assets/{}", e.key), fs::read(source(&root, e)).unwrap())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What stays `include_bytes!`/`include_str!` in the libraries on every platform, by path
    /// prefix from the repository root, and why: code, or a few kilobytes the UI draws at once.
    const KEPT_EMBEDDED: [(&str, &str); 3] = [
        ("crates/map/src/shaders/", "the WGSL shaders (code)"),
        ("assets/icons/lucide/", "the Lucide icons and their licence"),
        ("assets/fonts/OFL.txt", "the fonts' licence"),
    ];

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-android-assets-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_copy_is_exactly_the_list_and_takes_away_what_it_lacks() {
        let root = bump::repo_root();
        let dir = temp("copy");
        fs::create_dir_all(dir.join("realistic/old")).unwrap();
        fs::write(dir.join("realistic/old/gone.ktx2"), b"x").unwrap();
        fs::write(dir.join("stray.json"), b"{}").unwrap();
        fs::create_dir_all(dir.join("fonts")).unwrap();
        fs::write(dir.join("fonts/overpass-latin.ttf"), b"stale").unwrap();
        let first = copy(&root, &dir).unwrap();
        assert_eq!((first.files, first.written, first.removed), (FILES.len(), FILES.len(), 2));
        assert!((29_000_000..31_000_000).contains(&first.bytes), "{}", first.bytes);
        let mut present = Vec::new();
        files_under(&dir, "", &mut present).unwrap();
        present.sort();
        let mut keys: Vec<String> = FILES.iter().map(|e| e.key.to_string()).collect();
        keys.sort();
        assert_eq!(present, keys);
        assert!(!dir.join("realistic/old").exists(), "the emptied folder goes too");
        for entry in FILES {
            assert!(fs::read(dir.join(entry.key)).unwrap() == fs::read(source(&root, entry)).unwrap(), "{}", entry.key);
        }
        // again: nothing to write, so Gradle's inputs keep their times
        let again = copy(&root, &dir).unwrap();
        assert_eq!((again.written, again.removed, again.bytes), (0, 0, first.bytes));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_apk_is_checked_against_the_list() {
        let root = bump::repo_root();
        let dir = temp("check");
        let assets = apk_assets();
        let entries = |skip: Option<&str>, extra: &[(&str, &[u8])]| -> Vec<u8> {
            let mut list: Vec<(&str, &[u8])> = vec![
                ("AndroidManifest.xml", b"<manifest/>"),
                ("lib/arm64-v8a/librustradar_app.so", b"\x7fELF"),
                // Gradle's own, in every release APK
                ("assets/dexopt/baseline.prof", b"pro"),
                ("assets/dexopt/baseline.profm", b"prm"),
            ];
            list.extend(assets.iter().filter(|(n, _)| Some(n.as_str()) != skip).map(|(n, b)| (n.as_str(), b.as_slice())));
            list.extend_from_slice(extra);
            stored_zip(&list)
        };
        let apk = dir.join("app.apk");

        fs::write(&apk, entries(None, &[])).unwrap();
        let good = check_apk(&root, &apk).unwrap();
        assert_eq!(good.files, FILES.len());
        assert_eq!(zip_entries(&apk).unwrap().len(), FILES.len() + 4);

        fs::write(&apk, entries(Some("assets/realistic/ground.color.ktx2"), &[("assets/old.bin", b"x")])).unwrap();
        let err = check_apk(&root, &apk).unwrap_err();
        assert!(err.contains("assets/realistic/ground.color.ktx2 is missing"), "{err}");
        assert!(err.contains("assets/old.bin is not one of the app's files"), "{err}");

        // another file under a listed name
        let mut changed = entries(Some("assets/data/sth.json"), &[("assets/data/sth.json", b"{}")]);
        fs::write(&apk, &changed).unwrap();
        assert!(check_apk(&root, &apk).unwrap_err().contains("assets/data/sth.json is not crates/backend/data/sth.json"));

        // deflated: method 8 in the central directory
        changed = entries(None, &[]);
        let name = b"assets/fonts/overpass-latin.ttf";
        let at = changed.windows(name.len()).rposition(|w| w == name).unwrap() - 46;
        changed[at + 10] = 8;
        fs::write(&apk, &changed).unwrap();
        assert!(check_apk(&root, &apk).unwrap_err().contains("assets/fonts/overpass-latin.ttf is compressed (method 8), not stored"));

        fs::write(&apk, b"not a zip").unwrap();
        assert!(check_apk(&root, &apk).unwrap_err().contains("not a zip archive"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_command_line_is_read() {
        assert!(run(&[]).unwrap_err().starts_with("usage"));
        assert!(run(&["--check".into()]).unwrap_err().starts_with("usage"));
        assert!(run(&["--other".into()]).unwrap_err().starts_with("usage"));
    }

    /// Every `include_bytes!`/`include_str!` in the libraries' sources (`crates/*/src`, not the
    /// tests' folders or the xtask), as (file, path of the included file from the repository
    /// root, or the prefix a `concat!` starts with).
    fn includes() -> Vec<(String, String)> {
        let root = bump::repo_root();
        let mut found = Vec::new();
        for krate in fs::read_dir(root.join("crates")).unwrap().flatten() {
            if krate.file_name() == "xtask" {
                continue;
            }
            let mut sources = Vec::new();
            files_under(&krate.path().join("src"), "", &mut sources).unwrap();
            for rel in sources.iter().filter(|s| s.ends_with(".rs")) {
                let file = krate.path().join("src").join(rel);
                let text = fs::read_to_string(&file).unwrap();
                for (at, _) in text.match_indices("include_").filter(|(at, _)| {
                    let rest = &text[at + 8..];
                    rest.starts_with("bytes!(") || rest.starts_with("str!(")
                }) {
                    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
                    if text[line_start..at].trim_start().starts_with("//") {
                        continue;
                    }
                    let open = at + text[at..].find('(').unwrap();
                    let close = open + text[open..].find(')').unwrap();
                    let args = &text[open + 1..close];
                    let Some(q) = args.find('"') else { continue };
                    let literal = &args[q + 1..];
                    let literal = &literal[..literal.find('"').unwrap()];
                    // relative to the file's folder; `concat!("../../../", $root, …)` gives a prefix
                    let mut path: Vec<String> = file.parent().unwrap().strip_prefix(&root).unwrap().iter().map(|p| p.to_string_lossy().into_owned()).collect();
                    for part in literal.split('/') {
                        match part {
                            ".." => {
                                path.pop();
                            }
                            "" | "." => {}
                            other => path.push(other.to_string()),
                        }
                    }
                    let mut joined = path.join("/");
                    if literal.ends_with('/') {
                        joined.push('/');
                    }
                    found.push((file.strip_prefix(&root).unwrap().display().to_string(), joined));
                }
            }
        }
        found
    }

    #[test]
    fn no_library_embeds_a_file_but_through_the_list_or_the_few_kept() {
        let includes = includes();
        assert!(includes.len() > 20, "the scan finds the shaders at least: {includes:?}");
        let mut kept_bytes = 0;
        for (file, path) in &includes {
            if file == "crates/core/src/assets.rs" {
                // the list's own include_bytes!(concat!("../../../", $root, "/", $key))
                continue;
            }
            // the unit tests' own fixtures (`crates/<crate>/tests/data`), read in test modules only
            if path.starts_with("crates/") && path.contains("/tests/data/") {
                continue;
            }
            let kept = KEPT_EMBEDDED.iter().any(|(prefix, _)| path.starts_with(prefix));
            assert!(kept, "{file} embeds {path}: list it in rustradar_core::assets instead (risk K14)");
            if !path.ends_with('/') {
                kept_bytes += fs::metadata(bump::repo_root().join(path)).unwrap().len();
            }
        }
        let icons = bump::repo_root().join("assets/icons/lucide");
        kept_bytes += fs::read_dir(icons).unwrap().flatten().map(|e| e.metadata().unwrap().len()).sum::<u64>();
        // the shaders, icons and licences: a fraction of a megabyte in each library
        assert!(kept_bytes < 400_000, "{kept_bytes} bytes kept embedded");
    }

    #[test]
    fn the_android_build_copies_the_files_and_gradle_keeps_them_stored() {
        let root = bump::repo_root();
        let script = fs::read_to_string(root.join("platform/android/build-rust.sh")).unwrap();
        assert!(script.contains("android-assets \"$here/app/src/main/assets\""), "build-rust.sh copies the files");
        let gradle = fs::read_to_string(root.join("platform/android/app/build.gradle.kts")).unwrap();
        let extensions: BTreeSet<&str> = FILES.iter().map(|e| e.key.rsplit('.').next().unwrap()).collect();
        let line = gradle.lines().find(|l| l.contains("noCompress")).expect("a noCompress line");
        for extension in &extensions {
            assert!(line.contains(&format!("\"{extension}\"")), "{extension} is not kept stored: {line}");
        }
        let ignored = fs::read_to_string(root.join("platform/android/app/.gitignore")).unwrap();
        assert!(ignored.lines().any(|l| l == "/src/main/assets/"), "the copy is not committed");
    }
}
