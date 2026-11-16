//! `cargo xtask bump` and `cargo xtask version`: the app's version, which lives in several files that
//! have to agree (CI builds with `--locked`, and the updater compares the built version with the latest
//! release):
//!
//! ```text
//! cargo xtask bump            show the version in each file
//! cargo xtask bump patch      0.1.22 -> 0.1.23   (or minor, major)
//! cargo xtask bump 0.2.0      set it exactly
//! cargo xtask bump --check    fail unless they all agree (used by CI)
//! cargo xtask version         print the version alone (scripts/release-version.sh)
//! ```
//!
//! The places, in the order they are listed ([`places`]): the `[package]` version of the workspace
//! member whose package is [`APP_PACKAGE`] (`crates/app`; the app compares its `CARGO_PKG_VERSION`
//! with the releases'), the one of [`BACKEND_PACKAGE`] (its requests' User-Agent is
//! `RustRadar/{its version}`), and their two `Cargo.lock` entries.

use regex::{Captures, Regex};
use std::fs;
use std::path::{Path, PathBuf};

/// The app's package (`crates/app`).
pub const APP_PACKAGE: &str = "rustradar";

/// The backend the app links: its version is its requests' User-Agent's.
pub const BACKEND_PACKAGE: &str = "rustradar-backend";

/// The repository root: `crates/xtask` is two levels below it.
pub(crate) fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}

/// One place the version appears: where it is, how to read it, how to replace it.
pub struct Place {
    /// the file, relative to the repository root
    pub file: String,
    /// as the place is listed: the file, with the package for a `Cargo.lock` entry
    pub name: String,
    text: String,
    pattern: Regex,
    pub version: String,
}

/// A pattern whose groups are (before, version, after); only the first match counts.
fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("a valid pattern")
}

/// The `[package]` table's version, the first one in the file.
const TOML_VERSION: &str = r#"(?m)^(version\s*=\s*")([^"]+)(")"#;

/// `package`'s entry in `Cargo.lock`: only that package's, not a dependency that happens to share a
/// number, nor a package whose name starts with it.
fn lock_entry(package: &str) -> Regex {
    pattern(&format!(r#"(name = "{}"\r?\nversion = ")([^"]+)(")"#, regex::escape(package)))
}

/// Every place the version appears under `root`, in the order of the module docs.
pub fn places(root: &Path) -> Result<Vec<Place>, String> {
    let members = members(root)?;
    let mut found = Vec::new();
    for package in [APP_PACKAGE, BACKEND_PACKAGE] {
        let file = member(&members, package)?.ok_or_else(|| format!("no workspace member is the package {package}"))?;
        found.push(place(root, &file, file.clone(), pattern(TOML_VERSION))?);
    }
    for package in [APP_PACKAGE, BACKEND_PACKAGE] {
        found.push(place(root, "Cargo.lock", format!("Cargo.lock ({package})"), lock_entry(package))?);
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

/// Whether `arg` is an exact `x.y.z` (ASCII digits only).
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
/// place's pattern), in the order the files first appear among the places.
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
/// (then by their paths).
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

    /// The four places, as they are listed.
    const FOUR: [&str; 4] = ["crates/app/Cargo.toml", "crates/backend/Cargo.toml", "Cargo.lock (rustradar)", "Cargo.lock (rustradar-backend)"];
    /// The files they are in.
    const FILES: [&str; 3] = ["crates/app/Cargo.toml", "crates/backend/Cargo.toml", "Cargo.lock"];

    /// A folder of its own under the system's temp folder, removed when dropped.
    struct Tree(PathBuf);

    impl Tree {
        fn empty(name: &str) -> Tree {
            let root = std::env::temp_dir().join(format!("xtask-bump-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Tree(root)
        }

        /// A repository root with the four places at `versions` (the app's lock entry with a CRLF, as
        /// a checkout on Windows may have it).
        fn new(name: &str, versions: [&str; 4]) -> Tree {
            let tree = Tree::empty(name);
            let [app, backend, lock, backend_lock] = versions;
            let files = [
                ("Cargo.toml", "[workspace]\nresolver = \"3\"\nmembers = [\n    \"crates/core\",\n    \"crates/backend\",\n    \"crates/app\",\n]\n".to_owned()),
                (
                    "crates/app/Cargo.toml",
                    format!("[package]\nname = \"rustradar\"\nversion = \"{app}\"\nedition = \"2024\"\n\n[lib]\nname = \"rustradar_app\"\n\n[[bin]]\nname = \"rustradar\"\n\n[dependencies]\nwinit = {{ version = \"0.30.13\" }}\n\n[dependencies.png]\nversion = \"0.18\"\n"),
                ),
                (
                    "crates/backend/Cargo.toml",
                    format!("[package]\nname = \"rustradar-backend\"\nversion = \"{backend}\"\nedition = \"2024\"\n\n[dependencies]\nfr24 = {{ workspace = true }}\n\n[dependencies.regex]\nversion = \"1\"\n"),
                ),
                ("crates/core/Cargo.toml", "[package]\nname = \"rustradar-core\"\nversion = \"0.1.0\"\n".to_owned()),
                (
                    "Cargo.lock",
                    format!("version = 4\n\n[[package]]\nname = \"rustradar\"\r\nversion = \"{lock}\"\n\n[[package]]\nname = \"rustradar-backend\"\nversion = \"{backend_lock}\"\n\n[[package]]\nname = \"rustradar-core\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"other\"\nversion = \"{lock}\"\n"),
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
    fn finds_the_four_places() {
        let tree = Tree::new("four", ["0.3.0"; 4]);
        assert_eq!(tree.names(), FOUR);
        let found = places(&tree.0).unwrap();
        assert_eq!(found.iter().map(|p| p.file.as_str()).collect::<Vec<_>>(), [&FILES[..], &["Cargo.lock"]].concat());
        assert_eq!(agreed(&found).unwrap(), "0.3.0");
        // the closing line names the two manifests by their folders
        assert_eq!(and_list(&written(&found)), "crates/app/Cargo.toml, crates/backend/Cargo.toml and Cargo.lock");
    }

    #[test]
    fn a_dry_runs_version_reaches_the_backend() {
        // CI's dry run writes the next patch before building: the backend's requests then say
        // `RustRadar/0.3.17` (backend::USER_AGENT)
        let tree = Tree::new("dry-run", ["0.3.0"; 4]);
        bump(&tree.0, Some("0.3.17")).unwrap();
        assert!(tree.all_at("0.3.17"));
        assert!(tree.read("crates/backend/Cargo.toml").starts_with("[package]\nname = \"rustradar-backend\"\nversion = \"0.3.17\"\n"));
        assert!(tree.read("Cargo.lock").contains("name = \"rustradar-backend\"\nversion = \"0.3.17\"\n"));
    }

    #[test]
    fn bump_rewrites_only_the_apps_versions() {
        let tree = Tree::new("rewrite", ["0.3.0"; 4]);
        let before: Vec<String> = FILES.iter().map(|f| tree.read(f)).collect();
        bump(&tree.0, Some("patch")).unwrap();
        assert!(tree.all_at("0.3.1"));
        // nothing else changed: the dependencies' versions, the other packages, the CRLF entry;
        // Cargo.lock's two entries are its first two 0.3.0s
        let after: Vec<String> = FILES.iter().map(|f| tree.read(f)).collect();
        for ((file, b), a) in FILES.iter().zip(&before).zip(&after) {
            let places_in_it = if *file == "Cargo.lock" { 2 } else { 1 };
            assert_eq!(b.replacen("0.3.0", "0.3.1", places_in_it), *a, "{file}");
        }
        let lock = tree.read("Cargo.lock");
        assert!(lock.contains("name = \"rustradar\"\r\nversion = \"0.3.1\""));
        assert!(lock.contains("name = \"rustradar-backend\"\nversion = \"0.3.1\""));
        assert!(lock.contains("name = \"rustradar-core\"\nversion = \"0.1.0\""));
        assert!(lock.contains("name = \"other\"\nversion = \"0.3.0\""));
        let app = tree.read("crates/app/Cargo.toml");
        assert!(app.starts_with("[package]\nname = \"rustradar\"\nversion = \"0.3.1\"\n"));
        assert!(app.ends_with("winit = { version = \"0.30.13\" }\n\n[dependencies.png]\nversion = \"0.18\"\n"));
        bump(&tree.0, Some("minor")).unwrap();
        assert!(tree.all_at("0.4.0"));
        bump(&tree.0, Some("major")).unwrap();
        assert!(tree.all_at("1.0.0"));
        assert!(bump(&tree.0, Some("--check")).is_ok());
        assert!(bump(&tree.0, None).is_ok());
        // back to where it started: every file as it was
        bump(&tree.0, Some("0.3.0")).unwrap();
        assert_eq!(FILES.iter().map(|f| tree.read(f)).collect::<Vec<_>>(), before);
    }

    #[test]
    fn disagreeing_files_fail_the_check_and_take_only_an_exact_version() {
        let tree = Tree::new("disagree", ["0.3.0", "0.3.1", "0.3.0", "0.3.0"]);
        assert!(bump(&tree.0, Some("--check")).unwrap_err().contains("The versions disagree"));
        assert!(bump(&tree.0, None).is_err());
        assert!(current_under(&tree.0).is_err(), "`version` prints nothing the files do not agree on");
        let refused = bump(&tree.0, Some("patch")).unwrap_err();
        assert_eq!(refused, "The versions disagree; fix that first with an exact version: `cargo xtask bump <version>`.");
        assert_eq!(tree.versions()[1].1, "0.3.1", "a refused bump writes nothing");
        bump(&tree.0, Some("0.3.2")).unwrap();
        assert!(tree.all_at("0.3.2"));
        assert!(bump(&tree.0, Some("--check")).is_ok());
    }

    #[test]
    fn the_check_catches_any_one_place_out_of_step() {
        for (i, name) in FOUR.iter().enumerate() {
            let mut versions = ["0.3.0"; 4];
            versions[i] = "0.1.0";
            let tree = Tree::new(&format!("step-{i}"), versions);
            assert_eq!(tree.versions()[i], (name.to_string(), "0.1.0".to_owned()));
            let e = bump(&tree.0, Some("--check")).unwrap_err();
            assert_eq!(e, "The versions disagree. Run `cargo xtask bump <version>` to make them match.", "{name}");
            assert!(bump(&tree.0, Some("minor")).is_err(), "{name}");
            bump(&tree.0, Some("0.3.0")).unwrap();
            assert!(tree.all_at("0.3.0"), "{name}");
        }
    }

    /// A copy of the repository's own version files (the workspace manifest, each member's
    /// `Cargo.toml` and `Cargo.lock`) in a temp folder.
    fn repository_copy(name: &str) -> Tree {
        let (root, copy) = (repo_root(), Tree::empty(name));
        let mut files = vec!["Cargo.toml".to_owned(), "Cargo.lock".to_owned()];
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
            assert!(b.trim_start().starts_with("version"), "{file}: {b}");
        }
        changed.len()
    }

    #[test]
    fn bumps_a_copy_of_the_repositorys_files() {
        let tree = repository_copy("copy");
        let start = current().unwrap();
        assert_eq!(tree.names(), FOUR);
        let original: Vec<String> = FILES.iter().map(|f| tree.read(f)).collect();
        let mut from = start.clone();
        for arg in ["patch", "minor", "major", start.as_str()] {
            let next = next_version(&from, arg).unwrap();
            let before: Vec<String> = FILES.iter().map(|f| tree.read(f)).collect();
            bump(&tree.0, Some(arg)).unwrap();
            assert!(tree.all_at(&next), "{arg}: {:?}", tree.versions());
            for (file, b) in FILES.iter().zip(&before) {
                let count = if *file == "Cargo.lock" { 2 } else { 1 };
                assert_eq!(changed_lines(file, b, &tree.read(file), &from, &next), count, "{arg}: {file}");
            }
            from = next;
        }
        // set back exactly: every file byte for byte as in the repository
        assert_eq!(FILES.iter().map(|f| tree.read(f)).collect::<Vec<_>>(), original);
    }

    #[test]
    fn a_missing_package_or_version_is_an_error() {
        let tree = Tree::new("missing", ["0.3.0"; 4]);
        tree.write("Cargo.lock", "version = 4\n\n[[package]]\nname = \"rustradar\"\nversion = \"0.3.0\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no version found in Cargo.lock (rustradar-backend)");
        tree.write("Cargo.lock", "version = 4\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no version found in Cargo.lock (rustradar)");
        tree.write("crates/backend/Cargo.toml", "[package]\nname = \"rustradar-backend\"\nedition = \"2024\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no version found in crates/backend/Cargo.toml");
        tree.write("crates/app/Cargo.toml", "[package]\nname = \"rustradar\"\nedition = \"2024\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no version found in crates/app/Cargo.toml");
        tree.write("crates/core/Cargo.toml", "[package]\nname = \"rustradar\"\nversion = \"0.3.0\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "more than one workspace member is the package rustradar: crates/core/Cargo.toml, crates/app/Cargo.toml");
        tree.write("crates/core/Cargo.toml", "[package]\nname = \"rustradar-core\"\n");
        tree.write("crates/app/Cargo.toml", "[package]\nname = \"rustradar-app\"\n");
        assert_eq!(places(&tree.0).err().unwrap(), "no workspace member is the package rustradar");
    }

    #[test]
    fn the_repository_agrees_on_one_version() {
        // what CI's `bump --check` asserts, here so that `cargo test` catches it first
        let found = places(&repo_root()).unwrap();
        assert_eq!(found.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), FOUR);
        assert_eq!(agreed(&found).unwrap(), current().unwrap());
        assert!(is_exact(&current().unwrap()));
    }

    #[test]
    fn lists_read_as_english() {
        assert_eq!(and_list(&["Cargo.toml", "Cargo.lock"]), "Cargo.toml and Cargo.lock");
        assert_eq!(and_list(&["a", "b", "c"]), "a, b and c");
    }
}
