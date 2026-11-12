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
//! The files are today's four while the Tauri app ships, and two after stage 12: `package.json` and
//! `src-tauri/tauri.conf.json` count only while they exist; the manifest is that of the workspace member
//! whose package is [`APP_PACKAGE`] (`src-tauri` now, `crates/app` once item D2 renames `rustradar-app`
//! to it), and `Cargo.lock` has that package's entry. So the tool needs no change when stage 12 lands.
//! The native app's crate is not counted while it is still `rustradar-app`.
//!
//! Deviations from `version.mjs`: an exact version may repair files that disagree (the script refused
//! every change then, while its message said to set an exact version), and a current version that is
//! not `x.y.z` is an error instead of `NaN` arithmetic.

use regex::{Captures, Regex};
use std::fs;
use std::path::{Path, PathBuf};

/// The app's package, wherever it lives: before and after stage 12 it is called this.
pub const APP_PACKAGE: &str = "rustradar";

/// The repository root: `crates/xtask` is two levels below it.
pub(crate) fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}

/// One place the version appears: where it is, how to read it, how to replace it.
pub struct Place {
    /// relative to the repository root, as it is printed
    pub file: String,
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

/// Every place the version appears under `root`, in the order `version.mjs` lists them.
pub fn places(root: &Path) -> Result<Vec<Place>, String> {
    let mut found = Vec::new();
    for file in JSON_FILES {
        if root.join(file).is_file() {
            found.push(place(root, file.to_owned(), pattern(JSON_VERSION))?);
        }
    }
    found.push(place(root, app_manifest(root)?, pattern(TOML_VERSION))?);
    // only this app's entry, not a dependency that happens to share a number
    let lock = format!(r#"(name = "{}"\r?\nversion = ")([^"]+)(")"#, regex::escape(APP_PACKAGE));
    found.push(place(root, "Cargo.lock".to_owned(), pattern(&lock))?);
    Ok(found)
}

fn place(root: &Path, file: String, pattern: Regex) -> Result<Place, String> {
    let text = fs::read_to_string(root.join(&file)).map_err(|e| format!("{file}: {e}"))?;
    let version = pattern.captures(&text).map(|c| c[2].to_owned()).ok_or_else(|| format!("no version found in {file}"))?;
    Ok(Place { file, text, pattern, version })
}

/// The `Cargo.toml` of the workspace member whose package is [`APP_PACKAGE`], relative to `root`.
fn app_manifest(root: &Path) -> Result<String, String> {
    let workspace = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
    let members = pattern(r"(?s)\bmembers\s*=\s*\[(.*?)\]");
    let list = members.captures(&workspace).ok_or("no [workspace] members in Cargo.toml")?;
    let name = pattern(r#"(?m)^name\s*=\s*"([^"]+)""#);
    let mut found = Vec::new();
    for member in pattern(r#""([^"]+)""#).captures_iter(&list[1]) {
        let file = format!("{}/Cargo.toml", &member[1]);
        let Ok(manifest) = fs::read_to_string(root.join(&file)) else { continue };
        if name.captures(&manifest).is_some_and(|c| &c[1] == APP_PACKAGE) {
            found.push(file);
        }
    }
    match <[String; 1]>::try_from(found) {
        Ok([file]) => Ok(file),
        Err(found) if found.is_empty() => Err(format!("no workspace member is the package {APP_PACKAGE}")),
        Err(found) => Err(format!("more than one workspace member is the package {APP_PACKAGE}: {}", found.join(", "))),
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

/// Each place's text with its version replaced by `next` (the first match only, as `String.replace`).
pub fn rewritten(places: &[Place], next: &str) -> Vec<(String, String)> {
    places
        .iter()
        .map(|p| {
            let text = p.pattern.replacen(&p.text, 1, |c: &Captures| format!("{}{next}{}", &c[1], &c[3]));
            (p.file.clone(), text.into_owned())
        })
        .collect()
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
        println!("{:<10} {}", p.version, p.file);
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
    let names: Vec<&str> = places.iter().map(|p| p.file.rsplit('/').next().unwrap_or(&p.file)).collect();
    println!("{current} -> {next}");
    println!("Updated {}.", and_list(&names));
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

    /// A repository root under the system's temp folder with today's four version files at `version`s.
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str, versions: [&str; 4]) -> Tree {
            let root = std::env::temp_dir().join(format!("xtask-bump-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("src-tauri")).unwrap();
            fs::create_dir_all(root.join("crates/app")).unwrap();
            let [package, conf, toml, lock] = versions;
            let files = [
                ("Cargo.toml", "[workspace]\nresolver = \"3\"\nmembers = [\n    \"src-tauri\",\n    \"crates/app\",\n]\n".to_owned()),
                ("package.json", format!("{{\n  \"name\": \"rustradar\",\n  \"version\": \"{package}\",\n  \"type\": \"module\"\n}}\n")),
                ("src-tauri/tauri.conf.json", format!("{{\n  \"productName\": \"RustRadar\",\n  \"version\": \"{conf}\",\n  \"plugins\": {{ \"x\": {{ \"version\": \"9.9.9\" }} }}\n}}\n")),
                ("src-tauri/Cargo.toml", format!("[package]\nname = \"rustradar\"\nversion = \"{toml}\"\nedition = \"2024\"\n\n[lib]\nname = \"rustradar_lib\"\n\n[dependencies]\nserde = {{ version = \"1\" }}\n")),
                ("crates/app/Cargo.toml", "[package]\nname = \"rustradar-app\"\nversion = \"0.1.0\"\n".to_owned()),
                ("Cargo.lock", format!("version = 4\n\n[[package]]\nname = \"rustradar\"\r\nversion = \"{lock}\"\n\n[[package]]\nname = \"rustradar-app\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"other\"\nversion = \"{lock}\"\n")),
            ];
            for (file, text) in files {
                fs::write(root.join(file), text).unwrap();
            }
            Tree(root)
        }

        fn read(&self, file: &str) -> String {
            fs::read_to_string(self.0.join(file)).unwrap()
        }

        fn versions(&self) -> Vec<(String, String)> {
            places(&self.0).unwrap().into_iter().map(|p| (p.file, p.version)).collect()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
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
    fn finds_todays_four_places() {
        let tree = Tree::new("four", ["0.3.0"; 4]);
        let found = tree.versions();
        let files: Vec<&str> = found.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(files, ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "Cargo.lock"]);
        assert!(found.iter().all(|(_, v)| v == "0.3.0"));
        assert_eq!(agreed(&places(&tree.0).unwrap()).unwrap(), "0.3.0");
    }

    #[test]
    fn bump_rewrites_only_the_apps_version() {
        let tree = Tree::new("rewrite", ["0.3.0"; 4]);
        let before: Vec<String> = ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "Cargo.lock"].iter().map(|f| tree.read(f)).collect();
        bump(&tree.0, Some("patch")).unwrap();
        assert!(tree.versions().iter().all(|(_, v)| v == "0.3.1"));
        // nothing else changed: the nested "version", the other packages, the CRLF entry
        let after: Vec<String> = ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "Cargo.lock"].iter().map(|f| tree.read(f)).collect();
        for (b, a) in before.iter().zip(&after) {
            assert_eq!(b.replacen("0.3.0", "0.3.1", 1), *a);
        }
        assert!(tree.read("Cargo.lock").contains("name = \"rustradar\"\r\nversion = \"0.3.1\""));
        assert!(tree.read("Cargo.lock").contains("name = \"other\"\nversion = \"0.3.0\""));
        assert!(tree.read("src-tauri/tauri.conf.json").contains("\"version\": \"9.9.9\""));
        assert_eq!(tree.read("crates/app/Cargo.toml"), "[package]\nname = \"rustradar-app\"\nversion = \"0.1.0\"\n");
        bump(&tree.0, Some("minor")).unwrap();
        assert!(tree.versions().iter().all(|(_, v)| v == "0.4.0"));
        bump(&tree.0, Some("major")).unwrap();
        assert!(tree.versions().iter().all(|(_, v)| v == "1.0.0"));
        bump(&tree.0, Some("0.3.16")).unwrap();
        assert!(tree.versions().iter().all(|(_, v)| v == "0.3.16"));
        assert!(bump(&tree.0, Some("--check")).is_ok());
        assert!(bump(&tree.0, None).is_ok());
    }

    #[test]
    fn disagreeing_files_fail_the_check_and_take_only_an_exact_version() {
        let tree = Tree::new("disagree", ["0.3.0", "0.3.0", "0.3.1", "0.3.0"]);
        assert!(bump(&tree.0, Some("--check")).unwrap_err().contains("The versions disagree"));
        assert!(bump(&tree.0, None).is_err());
        assert!(current_under(&tree.0).is_err(), "`version` prints nothing the files do not agree on");
        let refused = bump(&tree.0, Some("patch")).unwrap_err();
        assert_eq!(refused, "The versions disagree; fix that first with an exact version: `cargo xtask bump <version>`.");
        assert_eq!(tree.versions()[2].1, "0.3.1", "a refused bump writes nothing");
        bump(&tree.0, Some("0.3.2")).unwrap();
        assert!(tree.versions().iter().all(|(_, v)| v == "0.3.2"));
        assert!(bump(&tree.0, Some("--check")).is_ok());
    }

    fn current_under(root: &Path) -> Result<String, String> {
        places(root).and_then(|p| agreed(&p).map(str::to_owned))
    }

    #[test]
    fn after_stage_12_two_places_follow_the_renamed_package() {
        // D2: the JSON files are gone and crates/app is the package `rustradar`
        let tree = Tree::new("stage12", ["0.3.0"; 4]);
        fs::remove_file(tree.0.join("package.json")).unwrap();
        fs::remove_file(tree.0.join("src-tauri/tauri.conf.json")).unwrap();
        fs::write(tree.0.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\n").unwrap();
        fs::write(tree.0.join("crates/app/Cargo.toml"), "[package]\nname = \"rustradar\"\nversion = \"0.3.20\"\nedition = \"2024\"\n\n[[bin]]\nname = \"rustradar\"\n").unwrap();
        fs::write(tree.0.join("Cargo.lock"), "version = 4\n\n[[package]]\nname = \"rustradar\"\nversion = \"0.3.20\"\n").unwrap();
        let found = tree.versions();
        assert_eq!(found, [("crates/app/Cargo.toml".to_owned(), "0.3.20".to_owned()), ("Cargo.lock".to_owned(), "0.3.20".to_owned())]);
        bump(&tree.0, Some("patch")).unwrap();
        assert!(tree.read("crates/app/Cargo.toml").contains("version = \"0.3.21\""));
        assert!(tree.read("Cargo.lock").contains("version = \"0.3.21\""));
    }

    #[test]
    fn a_missing_package_or_version_is_an_error() {
        let tree = Tree::new("missing", ["0.3.0"; 4]);
        fs::write(tree.0.join("Cargo.lock"), "version = 4\n").unwrap();
        assert_eq!(places(&tree.0).err().unwrap(), "no version found in Cargo.lock");
        fs::write(tree.0.join("src-tauri/Cargo.toml"), "[package]\nname = \"rustradar-shim\"\n").unwrap();
        assert_eq!(places(&tree.0).err().unwrap(), "no workspace member is the package rustradar");
    }

    #[test]
    fn the_repository_agrees_on_one_version() {
        // what CI's `bump --check` asserts, here so that `cargo test` catches it first
        let root = repo_root();
        let found = places(&root).unwrap();
        assert!(found.iter().any(|p| p.file == "Cargo.lock"));
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
