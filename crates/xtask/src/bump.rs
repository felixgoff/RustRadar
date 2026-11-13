//! `cargo xtask bump` and `cargo xtask version`: the app's version, which lives in several files that
//! have to agree (CI builds with `--locked`, and the updater compares the built version with the latest
//! release). A port of `scripts/version.mjs` (doc 07 §9.3, §15.1):
//!
//! ```text
//! cargo xtask bump            show the version in each file
//! cargo xtask bump patch      0.1.22 -> 0.1.23   (or minor, major)
//! cargo xtask bump 0.2.0      set it exactly
//! cargo xtask bump --check    fail unless they all agree (used by CI)
//! cargo xtask version         print the version alone (scripts/release-version.sh)
//! ```
//!
//! The places, in the order they are listed ([`places`]):
//! - `package.json` and `src-tauri/tauri.conf.json` (the first `"version"` of each), while they exist;
//! - the `[package]` version of the workspace member whose package is [`APP_PACKAGE`] (`src-tauri`
//!   now, `crates/app` once item D2 renames `rustradar-app` to it);
//! - while a member is still the package [`NATIVE_PACKAGE`] (`crates/app` until D2), its `[package]`
//!   version too (item F5): the native app compares its own `CARGO_PKG_VERSION` with the releases'
//!   (`update::current_version`, `update::android::VERSION`) as today's app compares
//!   `tauri.conf.json`'s, so it must carry the version CI writes before a release build;
//! - `Cargo.lock`'s entry for [`APP_PACKAGE`], then, listed as `Cargo.lock (rustradar-app)`, its entry
//!   for [`NATIVE_PACKAGE`] while that package is a member.
//!
//! That is six places on `rust-port`, today's four in a tree without the native crate (`main`), and
//! two after stage 12 (`crates/app/Cargo.toml` and `Cargo.lock`), so the tool needs no change when
//! stage 12 lands.
//!
//! Deviations from `version.mjs`: the native crate's two places (the script knows today's four, so
//! `bun run bump` and the workflow steps that still run the script leave `crates/app` behind, which
//! `cargo xtask bump --check` then reports); an exact version may repair files that disagree (the
//! script refused every change then, while its message said to set an exact version); a current
//! version that is not `x.y.z` is an error instead of `NaN` arithmetic; the closing `Updated …` line
//! names the files written, by their paths where two share a name, instead of a fixed four.

use regex::{Captures, Regex};
use std::fs;
use std::path::{Path, PathBuf};

/// The app's package, wherever it lives: before and after stage 12 it is called this.
pub const APP_PACKAGE: &str = "rustradar";

/// The native app's package until item D2 renames it to [`APP_PACKAGE`]; until then it carries the
/// release version beside the Tauri app's files (item F5).
pub const NATIVE_PACKAGE: &str = "rustradar-app";

/// The repository root: `crates/xtask` is two levels below it.
pub(crate) fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}

/// One place the version appears: where it is, how to read it, how to replace it.
pub struct Place {
    /// the file, relative to the repository root
    pub file: String,
    /// as the place is listed: the file, with the package for the native crate's `Cargo.lock` entry
    pub name: String,
    text: String,
    pattern: Regex,
    pub version: String,
}

/// A pattern whose groups are (before, version, after); only the first match counts.
fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("a valid pattern")
}

/// The JSON files of the Tauri app; each counts while it exists.
const JSON_FILES: [&str; 2] = ["package.json", "src-tauri/tauri.conf.json"];
/// `"version": "…"`, the first one in the file.
const JSON_VERSION: &str = r#"(?m)^(\s*"version":\s*")([^"]+)(")"#;
/// The `[package]` table's version, the first one in the file.
const TOML_VERSION: &str = r#"(?m)^(version\s*=\s*")([^"]+)(")"#;

/// `package`'s entry in `Cargo.lock`: only that package's, not a dependency that happens to share a
/// number, nor a package whose name starts with it.
fn lock_entry(package: &str) -> Regex {
    pattern(&format!(r#"(name = "{}"\r?\nversion = ")([^"]+)(")"#, regex::escape(package)))
}

/// Every place the version appears under `root`, in the order of the module docs.
pub fn places(root: &Path) -> Result<Vec<Place>, String> {
    let mut found = Vec::new();
    for file in JSON_FILES {
        if root.join(file).is_file() {
            found.push(place(root, file, file.to_owned(), pattern(JSON_VERSION))?);
        }
    }
    let members = members(root)?;
    let app = member(&members, APP_PACKAGE)?.ok_or_else(|| format!("no workspace member is the package {APP_PACKAGE}"))?;
    let native = member(&members, NATIVE_PACKAGE)?;
    found.push(place(root, &app, app.clone(), pattern(TOML_VERSION))?);
    if let Some(native) = &native {
        found.push(place(root, native, native.clone(), pattern(TOML_VERSION))?);
    }
    found.push(place(root, "Cargo.lock", "Cargo.lock".to_owned(), lock_entry(APP_PACKAGE))?);
    if native.is_some() {
        found.push(place(root, "Cargo.lock", format!("Cargo.lock ({NATIVE_PACKAGE})"), lock_entry(NATIVE_PACKAGE))?);
    }
    Ok(found)
}

fn place(root: &Path, file: &str, name: String, pattern: Regex) -> Result<Place, String> {
    let text = fs::read_to_string(root.join(file)).map_err(|e| format!("{file}: {e}"))?;
    let version = pattern.captures(&text).map(|c| c[2].to_owned()).ok_or_else(|| format!("no version found in {name}"))?;
    Ok(Place { file: file.to_owned(), name, text, pattern, version })
}

/// Each workspace member's `Cargo.toml`, relative to `root`, with its package's name (members whose
/// manifest cannot be read are left out).
fn members(root: &Path) -> Result<Vec<(String, String)>, String> {
    let workspace = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let members = pattern(r"(?s)\bmembers\s*=\s*\[(.*?)\]");
    let list = members.captures(&workspace).ok_or("no [workspace] members in Cargo.toml")?;
    let name = pattern(r#"(?m)^name\s*=\s*"([^"]+)""#);
    let mut found = Vec::new();
    for member in pattern(r#""([^"]+)""#).captures_iter(&list[1]) {
        let file = format!("{}/Cargo.toml", &member[1]);
        let Ok(manifest) = fs::read_to_string(root.join(&file)) else { continue };
        if let Some(package) = name.captures(&manifest) {
            let package = package[1].to_owned();
            found.push((file, package));
        }
    }
    Ok(found)
}

/// The `Cargo.toml` of the workspace member whose package is `package`, if one is.
fn member(members: &[(String, String)], package: &str) -> Result<Option<String>, String> {
    let found: Vec<&str> = members.iter().filter(|(_, p)| p == package).map(|(file, _)| file.as_str()).collect();
    match found[..] {
        [] => Ok(None),
        [file] => Ok(Some(file.to_owned())),
        _ => Err(format!("more than one workspace member is the package {package}: {}", found.join(", "))),
    }
}

/// The version every place agrees on, or why there is none.
pub fn agreed(places: &[Place]) -> Result<&str, String> {
    let current = places.first().map(|p| p.version.as_str()).ok_or("no version files")?;
    if places.iter().all(|p| p.version == current) {
        Ok(current)
    } else {
        Err("The versions disagree. Run `cargo xtask bump <version>` to make them match.".to_owned())
    }
}

/// The version the files agree on under the repository root (`cargo xtask version`, the signatures'
/// trusted comment, `latest-json`'s default).
pub fn current() -> Result<String, String> {
    places(&repo_root()).and_then(|p| agreed(&p).map(str::to_owned))
}

/// Whether `arg` is an exact `x.y.z` (ASCII digits only, as JavaScript's `\d`).
fn is_exact(arg: &str) -> bool {
    pattern(r"^[0-9]+\.[0-9]+\.[0-9]+$").is_match(arg)
}

/// The version after `current` for a `bump` argument: `patch`, `minor`, `major` or an exact `x.y.z`.
pub fn next_version(current: &str, arg: &str) -> Result<String, String> {
    if is_exact(arg) {
        return Ok(arg.to_owned());
    }
    let unknown = || format!("Unknown version \"{arg}\". Use patch, minor, major or an exact x.y.z.");
    if !matches!(arg, "major" | "minor" | "patch") {
        return Err(unknown());
    }
    let parts: Vec<u64> = current.split('.').map(str::parse).collect::<Result<_, _>>().map_err(|_| not_xyz(current))?;
    let [major, minor, patch] = parts[..] else { return Err(not_xyz(current)) };
    Ok(match arg {
        "major" => format!("{}.0.0", major + 1),
        "minor" => format!("{major}.{}.0", minor + 1),
        _ => format!("{major}.{minor}.{}", patch + 1),
    })
}

fn not_xyz(current: &str) -> String {
    format!("The current version {current} isn't x.y.z; set one exactly: `cargo xtask bump <version>`.")
}

/// Each file's text with the version of every place in it replaced by `next` (the first match of each
/// place's pattern, as `String.replace`), in the order the files first appear among the places.
pub fn rewritten(places: &[Place], next: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    for p in places {
        let at = match files.iter().position(|(file, _)| *file == p.file) {
            Some(at) => at,
            None => {
                files.push((p.file.clone(), p.text.clone()));
                files.len() - 1
            }
        };
        let text = &mut files[at].1;
        *text = p.pattern.replacen(text.as_str(), 1, |c: &Captures| format!("{}{next}{}", &c[1], &c[3])).into_owned();
    }
    files
}

/// The last part of a path written with `/`.
fn file_name(file: &str) -> &str {
    file.rsplit('/').next().unwrap_or(file)
}

/// The files the places are in, once each in order, by their names alone unless two share a name
/// (then by their paths): `package.json, tauri.conf.json, Cargo.toml and Cargo.lock` as the script
/// printed them for today's four.
fn written(places: &[Place]) -> Vec<&str> {
    let mut files: Vec<&str> = Vec::new();
    for p in places {
        if !files.contains(&p.file.as_str()) {
            files.push(&p.file);
        }
    }
    let shared = |file: &str| files.iter().filter(|other| file_name(other) == file_name(file)).count() > 1;
    files.iter().map(|&file| if shared(file) { file } else { file_name(file) }).collect()
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn listing(places: &[Place]) {
    for p in places {
        println!("{:<10} {}", p.version, p.name);
    }
}

/// `cargo xtask bump [patch|minor|major|x.y.z|--check]` under `root`.
pub fn bump(root: &Path, arg: Option<&str>) -> Result<(), String> {
    let places = places(root)?;
    let agree = agreed(&places);
    let arg = match arg {
        None | Some("--check") => {
            listing(&places);
            let current = agree.inspect_err(|_| eprintln!())?;
            if arg.is_none() {
                println!("\nVersion {current}. Run `cargo xtask bump patch` to ship the next one.");
            }
            return Ok(());
        }
        Some(arg) => arg,
    };
    let current = places[0].version.as_str();
    if agree.is_err() && !is_exact(arg) {
        return Err("The versions disagree; fix that first with an exact version: `cargo xtask bump <version>`.".to_owned());
    }
    let next = next_version(current, arg)?;
    for (file, text) in rewritten(&places, &next) {
        fs::write(root.join(&file), text).map_err(|e| format!("{file}: {e}"))?;
    }
    println!("{current} -> {next}");
    println!("Updated {}.", and_list(&written(&places)));
    println!("Commit and merge to main to publish v{next}; installed apps update on their next launch.");
    Ok(())
}

pub fn run_bump(args: &[String]) -> Result<(), String> {
    match args {
        [] => bump(&repo_root(), None),
        [arg] => bump(&repo_root(), Some(arg)),
        _ => Err("usage: cargo xtask bump [patch|minor|major|x.y.z|--check]".to_owned()),
    }
}

pub fn run_version(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("usage: cargo xtask version".to_owned());
    }
    println!("{}", current()?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six places of `rust-port`, as they are listed.
    const SIX: [&str; 6] = [
        "package.json",
        "src-tauri/tauri.conf.json",
        "src-tauri/Cargo.toml",
        "crates/app/Cargo.toml",
        "Cargo.lock",
        "Cargo.lock (rustradar-app)",
    ];
    /// The files they are in.
    const FILES: [&str; 5] = ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "crates/app/Cargo.toml", "Cargo.lock"];

    /// A folder of its own under the system's temp folder, removed when dropped.
    struct Tree(PathBuf);

    impl Tree {
        fn empty(name: &str) -> Tree {
            let root = std::env::temp_dir().join(format!("xtask-bump-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Tree(root)
        }

        /// A repository root with today's six places at `versions`: `package.json`, `tauri.conf.json`,
        /// `src-tauri/Cargo.toml`, `crates/app/Cargo.toml`, and `Cargo.lock`'s `rustradar` and
        /// `rustradar-app` entries (the first with a CRLF, as a checkout on Windows may have it).
        fn new(name: &str, versions: [&str; 6]) -> Tree {
            let tree = Tree::empty(name);
            let [package, conf, toml, app, lock, app_lock] = versions;
            let files = [
                ("Cargo.toml", "[workspace]\nresolver = \"3\"\nmembers = [\n    \"src-tauri\",\n    \"crates/core\",\n    \"crates/app\",\n]\n".to_owned()),
                ("package.json", format!("{{\n  \"name\": \"rustradar\",\n  \"version\": \"{package}\",\n  \"type\": \"module\"\n}}\n")),
                ("src-tauri/tauri.conf.json", format!("{{\n  \"productName\": \"RustRadar\",\n  \"version\": \"{conf}\",\n  \"plugins\": {{ \"x\": {{ \"version\": \"9.9.9\" }} }}\n}}\n")),
                ("src-tauri/Cargo.toml", format!("[package]\nname = \"rustradar\"\nversion = \"{toml}\"\nedition = \"2024\"\n\n[lib]\nname = \"rustradar_lib\"\n\n[dependencies]\nserde = {{ version = \"1\" }}\n")),
                (
                    "crates/app/Cargo.toml",
                    format!("[package]\nname = \"rustradar-app\"\nversion = \"{app}\"\nedition = \"2024\"\n\n[lib]\nname = \"rustradar_app\"\n\n[[bin]]\nname = \"rustradar-app\"\n\n[dependencies]\nwinit = {{ version = \"0.30.13\" }}\n\n[dependencies.png]\nversion = \"0.18\"\n"),
                ),
                (
                    "Cargo.lock",
                    format!("version = 4\n\n[[package]]\nname = \"rustradar\"\r\nversion = \"{lock}\"\n\n[[package]]\nname = \"rustradar-app\"\nversion = \"{app_lock}\"\n\n[[package]]\nname = \"rustradar-backend\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"other\"\nversion = \"{lock}\"\n"),
                ),
            ];
            for (file, text) in files {
                tree.write(file, &text);
            }
            tree
        }

        fn read(&self, file: &str) -> String {
            fs::read_to_string(self.0.join(file)).unwrap()
        }

        fn write(&self, file: &str, text: &str) {
            let path = self.0.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        /// Each place's name and version.
        fn versions(&self) -> Vec<(String, String)> {
            places(&self.0).unwrap().into_iter().map(|p| (p.name, p.version)).collect()
        }

        fn names(&self) -> Vec<String> {
            self.versions().into_iter().map(|(name, _)| name).collect()
        }

        fn all_at(&self, version: &str) -> bool {
            self.versions().iter().all(|(_, v)| v == version)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn current_under(root: &Path) -> Result<String, String> {
        places(root).and_then(|p| agreed(&p).map(str::to_owned))
    }

    #[test]
    fn version_arithmetic() {
        assert_eq!(next_version("0.1.22", "patch").unwrap(), "0.1.23");
        assert_eq!(next_version("0.1.22", "minor").unwrap(), "0.2.0");
        assert_eq!(next_version("0.1.22", "major").unwrap(), "1.0.0");
        assert_eq!(next_version("0.3.9", "patch").unwrap(), "0.3.10");
        assert_eq!(next_version("0.9.15", "minor").unwrap(), "0.10.0");
        assert_eq!(next_version("0.3.0", "0.2.0").unwrap(), "0.2.0", "an exact version is taken as it is, even a lower one");
        assert_eq!(next_version("0.3.0", "10.20.30").unwrap(), "10.20.30");
        // the current version does not matter for an exact one
        assert_eq!(next_version("garbage", "1.2.3").unwrap(), "1.2.3");
        assert!(next_version("garbage", "patch").is_err());
        assert!(next_version("1.2", "patch").is_err());
        for bad in ["", "1.2", "1.2.3.4", "v1.2.3", "1.2.3-beta", "Patch", "1.2.x", "١.٢.٣", " 1.2.3"] {
            assert_eq!(
                next_version("0.3.0", bad).unwrap_err(),
                format!("Unknown version \"{bad}\". Use patch, minor, major or an exact x.y.z.")
            );
        }
    }

    #[test]
    fn finds_the_six_places_with_the_native_crate() {
        let tree = Tree::new("six", ["0.3.0"; 6]);
        assert_eq!(tree.names(), SIX);
        let found = places(&tree.0).unwrap();
        assert_eq!(found.iter().map(|p| p.file.as_str()).collect::<Vec<_>>(), [&FILES[..], &["Cargo.lock"]].concat());
        assert!(tree.all_at("0.3.0"));
        assert_eq!(agreed(&found).unwrap(), "0.3.0");
        // the closing line names the two manifests by their folders
        assert_eq!(written(&found), ["package.json", "tauri.conf.json", "src-tauri/Cargo.toml", "crates/app/Cargo.toml", "Cargo.lock"]);
        assert_eq!(and_list(&written(&found)), "package.json, tauri.conf.json, src-tauri/Cargo.toml, crates/app/Cargo.toml and Cargo.lock");
    }

    #[test]
    fn without_the_native_crate_todays_four_places_count() {
        // main's tree: no crates/app member, so version.mjs's four places and its closing line
        let tree = Tree::new("four", ["0.3.0", "0.3.0", "0.3.0", "0.1.0", "0.3.0", "0.1.0"]);
        tree.write("Cargo.toml", "[workspace]\nresolver = \"3\"\nmembers = [\n    \"src-tauri\",\n]\n");
        assert_eq!(tree.names(), ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "Cargo.lock"]);
        let found = places(&tree.0).unwrap();
        assert_eq!(and_list(&written(&found)), "package.json, tauri.conf.json, Cargo.toml and Cargo.lock");
        let (app, lock) = (tree.read("crates/app/Cargo.toml"), tree.read("Cargo.lock"));
        bump(&tree.0, Some("patch")).unwrap();
        assert!(tree.all_at("0.3.1"));
        assert_eq!(tree.read("crates/app/Cargo.toml"), app, "not a member: not counted");
        assert_eq!(tree.read("Cargo.lock"), lock.replacen("0.3.0", "0.3.1", 1), "nor its lock entry");
    }

    #[test]
    fn bump_rewrites_only_the_apps_versions() {
        let tree = Tree::new("rewrite", ["0.3.0"; 6]);
        let before: Vec<String> = FILES.iter().map(|f| tree.read(f)).collect();
        bump(&tree.0, Some("patch")).unwrap();
        assert!(tree.all_at("0.3.1"));
        // nothing else changed: the nested "version", the dependencies' versions, the other packages,
        // the CRLF entry; Cargo.lock's two entries are its first two 0.3.0s
        let after: Vec<String> = FILES.iter().map(|f| tree.read(f)).collect();
        for ((file, b), a) in FILES.iter().zip(&before).zip(&after) {
            let places_in_it = if *file == "Cargo.lock" { 2 } else { 1 };
            assert_eq!(b.replacen("0.3.0", "0.3.1", places_in_it), *a, "{file}");
        }
        let lock = tree.read("Cargo.lock");
        assert!(lock.contains("name = \"rustradar\"\r\nversion = \"0.3.1\""));
        assert!(lock.contains("name = \"rustradar-app\"\nversion = \"0.3.1\""));
        assert!(lock.contains("name = \"rustradar-backend\"\nversion = \"0.1.0\""));
        assert!(lock.contains("name = \"other\"\nversion = \"0.3.0\""));
        assert!(tree.read("src-tauri/tauri.conf.json").contains("\"version\": \"9.9.9\""));
        let app = tree.read("crates/app/Cargo.toml");
        assert!(app.starts_with("[package]\nname = \"rustradar-app\"\nversion = \"0.3.1\"\n"));
        assert!(app.ends_with("winit = { version = \"0.30.13\" }\n\n[dependencies.png]\nversion = \"0.18\"\n"));
        bump(&tree.0, Some("minor")).unwrap();
        assert!(tree.all_at("0.4.0"));
        bump(&tree.0, Some("major")).unwrap();
        assert!(tree.all_at("1.0.0"));
        bump(&tree.0, Some("0.3.16")).unwrap();
        assert!(tree.all_at("0.3.16"));
        assert!(bump(&tree.0, Some("--check")).is_ok());
        assert!(bump(&tree.0, None).is_ok());
        // back to where it started: every file as it was
        bump(&tree.0, Some("0.3.0")).unwrap();
        assert_eq!(FILES.iter().map(|f| tree.read(f)).collect::<Vec<_>>(), before);
    }

    #[test]
    fn disagreeing_files_fail_the_check_and_take_only_an_exact_version() {
        let tree = Tree::new("disagree", ["0.3.0", "0.3.0", "0.3.1", "0.3.0", "0.3.0", "0.3.0"]);
        assert!(bump(&tree.0, Some("--check")).unwrap_err().contains("The versions disagree"));
        assert!(bump(&tree.0, None).is_err());
        assert!(current_under(&tree.0).is_err(), "`version` prints nothing the files do not agree on");
        let refused = bump(&tree.0, Some("patch")).unwrap_err();
        assert_eq!(refused, "The versions disagree; fix that first with an exact version: `cargo xtask bump <version>`.");
        assert_eq!(tree.versions()[2].1, "0.3.1", "a refused bump writes nothing");
        bump(&tree.0, Some("0.3.2")).unwrap();
        assert!(tree.all_at("0.3.2"));
        assert!(bump(&tree.0, Some("--check")).is_ok());
    }

    #[test]
    fn the_check_catches_any_one_place_out_of_step() {
        // each of the six, the native crate's two among them (what `bun run bump` leaves behind)
        for (i, name) in SIX.iter().enumerate() {
            let mut versions = ["0.3.0"; 6];
            versions[i] = "0.1.0";
            let tree = Tree::new(&format!("step-{i}"), versions);
            assert_eq!(tree.versions()[i], (name.to_string(), "0.1.0".to_owned()));
            let e = bump(&tree.0, Some("--check")).unwrap_err();
            assert_eq!(e, "The versions disagree. Run `cargo xtask bump <version>` to make them match.", "{name}");
            assert!(bump(&tree.0, Some("minor")).is_err(), "{name}");
            bump(&tree.0, Some("0.3.0")).unwrap();
            assert!(tree.all_at("0.3.0"), "{name}");
            assert!(bump(&tree.0, Some("--check")).is_ok(), "{name}");
        }
    }

    /// A copy of the repository's own version files (the workspace manifest, each member's
    /// `Cargo.toml`, `Cargo.lock`, and the JSON files while they exist) in a temp folder.
    fn repository_copy(name: &str) -> Tree {
        let (root, copy) = (repo_root(), Tree::empty(name));
        let mut files = vec!["Cargo.toml".to_owned(), "Cargo.lock".to_owned()];
        files.extend(JSON_FILES.iter().filter(|f| root.join(f).is_file()).map(|f| f.to_string()));
        files.extend(members(&root).unwrap().into_iter().map(|(file, _)| file));
        for file in files {
            copy.write(&file, &fs::read_to_string(root.join(&file)).unwrap());
        }
        copy
    }

    /// The lines of `after` that differ from `before`'s, each the old line with `from` made `to`;
    /// returns how many differ.
    fn changed_lines(file: &str, before: &str, after: &str, from: &str, to: &str) -> usize {
        let (b, a): (Vec<&str>, Vec<&str>) = (before.split('\n').collect(), after.split('\n').collect());
        assert_eq!(b.len(), a.len(), "{file}: as many lines");
        let changed: Vec<(&str, &str)> = b.into_iter().zip(a).filter(|(b, a)| b != a).collect();
        for (b, a) in &changed {
            assert_eq!(b.replacen(from, to, 1), *a, "{file}");
            assert!(b.trim_start().starts_with("version") || b.trim_start().starts_with("\"version\""), "{file}: {b}");
        }
        changed.len()
    }

    #[test]
    fn bumps_a_copy_of_the_repositorys_files() {
        let tree = repository_copy("copy");
        let start = current().unwrap();
        let names: Vec<String> = places(&repo_root()).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(tree.names(), names);
        assert!(names.iter().any(|n| n == "crates/app/Cargo.toml"), "the native app carries the release version (F5)");
        let mut paths: Vec<String> = Vec::new();
        for p in places(&tree.0).unwrap() {
            if !paths.contains(&p.file) {
                paths.push(p.file);
            }
        }
        let original: Vec<String> = paths.iter().map(|f| tree.read(f)).collect();

        let mut from = start.clone();
        for arg in ["patch", "minor", "major", start.as_str()] {
            let next = next_version(&from, arg).unwrap();
            let before: Vec<String> = paths.iter().map(|f| tree.read(f)).collect();
            let counts: Vec<usize> = {
                let found = places(&tree.0).unwrap();
                paths.iter().map(|f| found.iter().filter(|p| &p.file == f).count()).collect()
            };
            bump(&tree.0, Some(arg)).unwrap();
            assert!(tree.all_at(&next), "{arg}: {:?}", tree.versions());
            for ((file, b), count) in paths.iter().zip(&before).zip(counts) {
                // only the places' lines, each its version made the next one
                assert_eq!(changed_lines(file, b, &tree.read(file), &from, &next), count, "{arg}: {file}");
            }
            bump(&tree.0, Some("--check")).unwrap();
            from = next;
        }
        // set back exactly: every file byte for byte as in the repository
        assert_eq!(paths.iter().map(|f| tree.read(f)).collect::<Vec<_>>(), original);

        // one place out of step (the native crate left behind first): the check fails, an exact
        // version puts it right
        for i in (0..names.len()).rev() {
            let found = places(&tree.0).unwrap();
            let (file, text) = rewritten(std::slice::from_ref(&found[i]), "0.0.1").remove(0);
            tree.write(&file, &text);
            assert!(bump(&tree.0, Some("--check")).unwrap_err().contains("The versions disagree"), "{}", names[i]);
            assert!(bump(&tree.0, Some("patch")).is_err());
            bump(&tree.0, Some(&start)).unwrap();
            bump(&tree.0, Some("--check")).unwrap();
            assert_eq!(paths.iter().map(|f| tree.read(f)).collect::<Vec<_>>(), original, "{}", names[i]);
        }
    }

    #[test]
    fn after_stage_12_two_places_follow_the_renamed_package() {
        // D2: the JSON files are gone and crates/app is the package `rustradar`
        let tree = Tree::new("stage12", ["0.3.0"; 6]);
        fs::remove_file(tree.0.join("package.json")).unwrap();
        fs::remove_file(tree.0.join("src-tauri/tauri.conf.json")).unwrap();
        tree.write("Cargo.toml", "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\n");
        tree.write("crates/app/Cargo.toml", "[package]\nname = \"rustradar\"\nversion = \"0.3.20\"\nedition = \"2024\"\n\n[[bin]]\nname = \"rustradar\"\n");
        tree.write("Cargo.lock", "version = 4\n\n[[package]]\nname = \"rustradar\"\nversion = \"0.3.20\"\n");
        let found = tree.versions();
        assert_eq!(found, [("crates/app/Cargo.toml".to_owned(), "0.3.20".to_owned()), ("Cargo.lock".to_owned(), "0.3.20".to_owned())]);
        assert_eq!(and_list(&written(&places(&tree.0).unwrap())), "Cargo.toml and Cargo.lock");
        bump(&tree.0, Some("patch")).unwrap();
        assert!(tree.read("crates/app/Cargo.toml").contains("version = \"0.3.21\""));
        assert!(tree.read("Cargo.lock").contains("version = \"0.3.21\""));
    }

    #[test]
    fn a_missing_package_or_version_is_an_error() {
        let tree = Tree::new("missing", ["0.3.0"; 6]);
        tree.write("Cargo.lock", "version = 4\n\n[[package]]\nname = \"rustradar\"\nversion = \"0.3.0\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no version found in Cargo.lock (rustradar-app)");
        tree.write("Cargo.lock", "version = 4\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no version found in Cargo.lock");
        tree.write("crates/app/Cargo.toml", "[package]\nname = \"rustradar-app\"\nedition = \"2024\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no version found in crates/app/Cargo.toml");
        tree.write("crates/core/Cargo.toml", "[package]\nname = \"rustradar-app\"\nversion = \"0.3.0\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "more than one workspace member is the package rustradar-app: crates/core/Cargo.toml, crates/app/Cargo.toml");
        tree.write("src-tauri/Cargo.toml", "[package]\nname = \"rustradar-shim\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no workspace member is the package rustradar");
    }

    #[test]
    fn the_repository_agrees_on_one_version() {
        // what CI's `bump --check` asserts, here so that `cargo test` catches it first
        let root = repo_root();
        let found = places(&root).unwrap();
        assert!(found.iter().any(|p| p.file == "Cargo.lock"));
        // the native app's crate is among them, before and after stage 12 (F5)
        assert!(found.iter().any(|p| p.file == "crates/app/Cargo.toml"));
        assert_eq!(agreed(&found).unwrap(), current().unwrap());
        assert!(is_exact(&current().unwrap()));
    }

    #[test]
    fn lists_read_as_english() {
        assert_eq!(and_list(&["Cargo.toml", "Cargo.lock"]), "Cargo.toml and Cargo.lock");
        assert_eq!(
            and_list(&["package.json", "tauri.conf.json", "Cargo.toml", "Cargo.lock"]),
            "package.json, tauri.conf.json, Cargo.toml and Cargo.lock"
        );
    }
}
