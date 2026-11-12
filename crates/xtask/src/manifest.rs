//! `cargo xtask latest-json`: the updater manifest every installed copy asks for at
//! `https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json` (doc 07 §7.2, §12.1;
//! tauri-action wrote it until the native release job). Run on the folder of a release's artifacts:
//!
//! ```text
//! cargo xtask latest-json <dir> [--version x.y.z] [--url-base URL] [--notes TEXT] [--pubkey KEY] [--out FILE]
//! ```
//!
//! Every updater artifact under `<dir>` (searched recursively) with its `.sig` beside it gets a
//! `<os>-<arch>-<bundle>` entry, and each `<os>-<arch>` the plain key old updaters read. The shape is
//! v0.3.15's published `latest.json`, checked on 2026-11-12: keys `version`, `notes`, `pub_date` (as
//! JavaScript's `toISOString`) and `platforms`, each entry `signature` (the `.sig` file's text) then `url`,
//! indented by two spaces with no final newline (`JSON.stringify(m, null, 2)`); reading that file into
//! [`Manifest`] and writing it back gives its bytes with the platforms sorted (tauri-action merged them
//! in the order its matrix legs finished). Its eleven keys come out of its asset names as this module
//! reads them ([`bundle`], [`arch`]), the plain keys pointing at the MSI, the AppImage and the
//! `.app.tar.gz` as there ([`PLAIN`]); and its `.deb` checked against `tauri.conf.json`'s public key
//! through [`sign::verify`].
//!
//! Deviation: tauri-action's URLs were GitHub's API asset URLs (`api.github.com/…/releases/assets/<id>`,
//! only known after the upload, and only downloaded with `Accept: application/octet-stream`, which the
//! Tauri updater sends); these are the release's download URLs
//! (`github.com/<repo>/releases/download/v<version>/<name>`), known before the upload and served to any
//! client through the same kind of redirect. The upgrade rehearsal (P11) checks that old copies follow it.
//!
//! `--pubkey` (Tauri's form, base64 of the `.pub` file, or the bare minisign key) checks every
//! signature against the artifact's bytes before anything is written, so that a release signed with
//! another key never reaches `releases/latest`.

use crate::bump;
use crate::sign;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The repository the releases are published in (doc 07 §13.6), unless `GITHUB_REPOSITORY` names
/// another (a workflow in a test repository).
pub const REPOSITORY: &str = "felixgoff/RustRadar";

/// The release body, which tauri-action also used as the manifest's notes.
pub const NOTES: &str = "Installed copies update to this version automatically the next time they open.";

/// `latest.json`.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: String,
    pub notes: String,
    pub pub_date: String,
    pub platforms: BTreeMap<String, Entry>,
}

/// One platform's download.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// the `.sig` file's text: base64 of the minisign signature
    pub signature: String,
    pub url: String,
}

/// An updater artifact: its file name and its `.sig` file's text.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub name: String,
    pub signature: String,
}

/// The kind of updater artifact a file name is, as (os, bundle) in the Tauri updater's words.
pub fn bundle(name: &str) -> Option<(&'static str, &'static str)> {
    const KINDS: [(&str, &str, &str); 6] = [
        ("-setup.exe", "windows", "nsis"),
        (".msi", "windows", "msi"),
        (".app.tar.gz", "darwin", "app"),
        (".AppImage", "linux", "appimage"),
        (".deb", "linux", "deb"),
        (".rpm", "linux", "rpm"),
    ];
    KINDS.iter().find(|(suffix, ..)| name.ends_with(suffix)).map(|&(_, os, bundle)| (os, bundle))
}

/// The architecture a file name names (a whole `_`, `-` or `.` separated part), in the Tauri updater's
/// words: `RustRadar_0.3.15_x64-setup.exe`, `RustRadar_0.3.15_amd64.AppImage`,
/// `RustRadar-0.3.15-1.x86_64.rpm`, `RustRadar_0.3.15_aarch64.app.tar.gz`.
pub fn arch(name: &str) -> Option<&'static str> {
    // x86_64 before x86, which is a part of it
    const ARCHS: [(&str, &str); 10] = [
        ("x86_64", "x86_64"),
        ("amd64", "x86_64"),
        ("x64", "x86_64"),
        ("aarch64", "aarch64"),
        ("arm64", "aarch64"),
        ("i686", "i686"),
        ("i386", "i686"),
        ("x86", "i686"),
        ("armv7", "armv7"),
        ("armhf", "armv7"),
    ];
    let separator = |c: Option<char>| c.is_none_or(|c| matches!(c, '_' | '-' | '.'));
    ARCHS.iter().find_map(|&(part, arch)| {
        name.match_indices(part)
            .any(|(at, _)| separator(name[..at].chars().next_back()) && separator(name[at + part.len()..].chars().next()))
            .then_some(arch)
    })
}

/// Which bundle each operating system's plain `<os>-<arch>` key points at, first present wins: the MSI,
/// the AppImage and the `.app.tar.gz`, as in v0.3.15's manifest.
pub const PLAIN: [(&str, &[&str]); 3] =
    [("windows", &["msi", "nsis"]), ("linux", &["appimage", "deb", "rpm"]), ("darwin", &["app"])];

/// The manifest for `artifacts`, downloaded from `url_base/<name>`.
pub fn manifest(version: &str, notes: &str, pub_date: &str, url_base: &str, artifacts: &[Artifact]) -> Result<Manifest, String> {
    let mut platforms = BTreeMap::new();
    let mut targets = Vec::new();
    for artifact in artifacts {
        let name = &artifact.name;
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')) {
            return Err(format!("{name}: GitHub renames assets with such characters, so the URL would not be the asset's"));
        }
        let (os, bundle) = bundle(name).ok_or_else(|| format!("{name} is not an updater artifact"))?;
        let arch = arch(name).ok_or_else(|| format!("{name}: no architecture in the file name (x64, x86_64, amd64, aarch64, …)"))?;
        let signature = artifact.signature.trim();
        if signature.is_empty() {
            return Err(format!("{name}.sig is empty"));
        }
        let key = format!("{os}-{arch}-{bundle}");
        let entry = Entry { signature: signature.to_owned(), url: format!("{}/{name}", url_base.trim_end_matches('/')) };
        if platforms.insert(key.clone(), entry).is_some() {
            return Err(format!("two artifacts for {key}"));
        }
        targets.push((os, arch));
    }
    if platforms.is_empty() {
        return Err("no updater artifacts".to_owned());
    }
    for (os, arch) in targets {
        let preferred = PLAIN.iter().find(|(o, _)| *o == os).map_or(&[][..], |(_, b)| b);
        let chosen = preferred.iter().find_map(|bundle| platforms.get(&format!("{os}-{arch}-{bundle}")).cloned());
        if let Some(entry) = chosen {
            platforms.insert(format!("{os}-{arch}"), entry);
        }
    }
    Ok(Manifest { version: version.to_owned(), notes: notes.to_owned(), pub_date: pub_date.to_owned(), platforms })
}

/// The manifest's text: `JSON.stringify(m, null, 2)`.
pub fn to_json(manifest: &Manifest) -> String {
    serde_json::to_string_pretty(manifest).expect("strings and maps serialise")
}

/// Unix milliseconds as JavaScript's `Date.prototype.toISOString` writes them.
pub fn iso_string(ms: i64) -> Result<String, String> {
    let at = jiff::Timestamp::from_millisecond(ms).map_err(|e| e.to_string())?;
    Ok(format!("{}.{:03}Z", at.strftime("%Y-%m-%dT%H:%M:%S"), ms.rem_euclid(1000)))
}

/// Every updater artifact under `dir` with its `.sig` beside it, by path, in name order.
pub fn artifacts(dir: &Path) -> Result<Vec<(PathBuf, Artifact)>, String> {
    let mut files = Vec::new();
    walk(dir, &mut files)?;
    let mut found = Vec::new();
    for path in files {
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        if bundle(&name).is_none() {
            continue;
        }
        let sig = path.with_file_name(format!("{name}.sig"));
        match fs::read_to_string(&sig) {
            Ok(signature) => found.push((path, Artifact { name, signature })),
            Err(_) => eprintln!("skipping {}: no {name}.sig beside it", path.display()),
        }
    }
    found.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    Ok(found)
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            walk(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

const USAGE: &str =
    "usage: cargo xtask latest-json <dir> [--version x.y.z] [--url-base URL] [--notes TEXT] [--pubkey KEY] [--out FILE]";

pub fn run(args: &[String]) -> Result<(), String> {
    let (mut dir, mut version, mut url_base, mut notes, mut pubkey, mut out) = (None, None, None, NOTES.to_owned(), None, PathBuf::from("latest.json"));
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().cloned().ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--version" => version = Some(value()?),
            "--url-base" => url_base = Some(value()?),
            "--notes" => notes = value()?,
            "--pubkey" => pubkey = Some(value()?),
            "--out" => out = PathBuf::from(value()?),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}\n{USAGE}")),
            path if dir.is_none() => dir = Some(PathBuf::from(path)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let dir = dir.ok_or(USAGE)?;
    let version = match version {
        Some(v) => v,
        None => bump::current()?,
    };
    let url_base = url_base.unwrap_or_else(|| {
        let repo = std::env::var("GITHUB_REPOSITORY").ok().filter(|r| !r.is_empty()).unwrap_or_else(|| REPOSITORY.to_owned());
        format!("https://github.com/{repo}/releases/download/v{version}")
    });
    let found = artifacts(&dir)?;
    if let Some(key) = pubkey {
        let key = sign::public_key(&key)?;
        for (path, artifact) in &found {
            let data = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            sign::verify(&key, &artifact.signature, data).map_err(|e| format!("{}: {e}", artifact.name))?;
            println!("signature verified: {}", artifact.name);
        }
    }
    let list: Vec<Artifact> = found.into_iter().map(|(_, a)| a).collect();
    let manifest = manifest(&version, &notes, &iso_string(rustradar_backend::now_ms())?, &url_base, &list)?;
    fs::write(&out, to_json(&manifest)).map_err(|e| format!("{}: {e}", out.display()))?;
    for (key, entry) in &manifest.platforms {
        println!("{key:<24} {}", entry.url);
    }
    println!("wrote {} for v{version}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v0.3.15's assets with a `.sig`, and the key each one has in its published `latest.json`.
    const PUBLISHED: [(&str, &[&str]); 7] = [
        ("RustRadar-0.3.15-1.x86_64.rpm", &["linux-x86_64-rpm"]),
        ("RustRadar_0.3.15_aarch64.app.tar.gz", &["darwin-aarch64", "darwin-aarch64-app"]),
        ("RustRadar_0.3.15_amd64.AppImage", &["linux-x86_64", "linux-x86_64-appimage"]),
        ("RustRadar_0.3.15_amd64.deb", &["linux-x86_64-deb"]),
        ("RustRadar_0.3.15_x64-setup.exe", &["windows-x86_64-nsis"]),
        ("RustRadar_0.3.15_x64.app.tar.gz", &["darwin-x86_64", "darwin-x86_64-app"]),
        ("RustRadar_0.3.15_x64_en-US.msi", &["windows-x86_64", "windows-x86_64-msi"]),
    ];

    fn published() -> Vec<Artifact> {
        PUBLISHED.iter().map(|(name, _)| Artifact { name: (*name).to_owned(), signature: format!("sig-of-{name}") }).collect()
    }

    const BASE: &str = "https://github.com/felixgoff/RustRadar/releases/download/v0.3.15";

    #[test]
    fn the_published_assets_get_the_published_keys() {
        let m = manifest("0.3.15", NOTES, "2026-11-11T08:21:15.913Z", BASE, &published()).unwrap();
        let mut expected: Vec<(&str, &str)> = PUBLISHED.iter().flat_map(|(name, keys)| keys.iter().map(move |k| (*k, *name))).collect();
        expected.sort();
        let got: Vec<(&str, &str)> = m.platforms.iter().map(|(k, e)| (k.as_str(), e.url.rsplit('/').next().unwrap())).collect();
        assert_eq!(got, expected);
        assert_eq!(m.platforms.len(), 11);
        for (key, entry) in &m.platforms {
            let name = entry.url.rsplit('/').next().unwrap();
            assert_eq!(entry.url, format!("{BASE}/{name}"), "{key}");
            assert_eq!(entry.signature, format!("sig-of-{name}"), "{key}: the signature is the artifact's own");
        }
    }

    #[test]
    fn manifest_shape_is_the_published_one() {
        let artifacts = [
            Artifact { name: "RustRadar_0.3.16_x64-setup.exe".into(), signature: "c2lnIG9mIG5zaXM=".into() },
            Artifact { name: "RustRadar_0.3.16_x64_en-US.msi".into(), signature: "c2lnIG9mIG1zaQ==\n".into() },
        ];
        let m = manifest("0.3.16", NOTES, "2026-11-12T09:30:00.042Z", "https://example.test/dl/", &artifacts).unwrap();
        let expected = r#"{
  "version": "0.3.16",
  "notes": "Installed copies update to this version automatically the next time they open.",
  "pub_date": "2026-11-12T09:30:00.042Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "c2lnIG9mIG1zaQ==",
      "url": "https://example.test/dl/RustRadar_0.3.16_x64_en-US.msi"
    },
    "windows-x86_64-msi": {
      "signature": "c2lnIG9mIG1zaQ==",
      "url": "https://example.test/dl/RustRadar_0.3.16_x64_en-US.msi"
    },
    "windows-x86_64-nsis": {
      "signature": "c2lnIG9mIG5zaXM=",
      "url": "https://example.test/dl/RustRadar_0.3.16_x64-setup.exe"
    }
  }
}"#;
        assert_eq!(to_json(&m), expected);
        // and it reads back, every field known
        let back: Manifest = serde_json::from_str(expected).unwrap();
        assert_eq!(back, m);
        assert!(serde_json::from_str::<Manifest>(&expected.replace("\"notes\"", "\"note\"")).is_err());
    }

    #[test]
    fn plain_keys_fall_back_to_the_bundle_there_is() {
        let only = |name: &str| vec![Artifact { name: name.to_owned(), signature: "s".to_owned() }];
        let m = manifest("1.0.0", NOTES, "d", BASE, &only("RustRadar_1.0.0_x64-setup.exe")).unwrap();
        assert_eq!(m.platforms["windows-x86_64"].url, m.platforms["windows-x86_64-nsis"].url);
        let m = manifest("1.0.0", NOTES, "d", BASE, &only("rustradar_1.0.0_amd64.deb")).unwrap();
        assert_eq!(m.platforms.keys().collect::<Vec<_>>(), ["linux-x86_64", "linux-x86_64-deb"]);
        let m = manifest("1.0.0", NOTES, "d", BASE, &only("RustRadar_1.0.0_aarch64.AppImage")).unwrap();
        assert_eq!(m.platforms.keys().collect::<Vec<_>>(), ["linux-aarch64", "linux-aarch64-appimage"]);
    }

    #[test]
    fn bad_artifact_lists_are_refused() {
        let one = |name: &str, sig: &str| vec![Artifact { name: name.to_owned(), signature: sig.to_owned() }];
        assert!(manifest("1.0.0", NOTES, "d", BASE, &[]).unwrap_err().contains("no updater artifacts"));
        // tauri's own name for the macOS archive says no architecture: it must be renamed first
        assert!(manifest("1.0.0", NOTES, "d", BASE, &one("RustRadar.app.tar.gz", "s")).unwrap_err().contains("no architecture"));
        assert!(manifest("1.0.0", NOTES, "d", BASE, &one("RustRadar 1.0.0_x64-setup.exe", "s")).unwrap_err().contains("renames"));
        assert!(manifest("1.0.0", NOTES, "d", BASE, &one("RustRadar_1.0.0_x64.dmg", "s")).unwrap_err().contains("not an updater artifact"));
        assert!(manifest("1.0.0", NOTES, "d", BASE, &one("RustRadar_1.0.0_x64.msi", " \n")).unwrap_err().contains("empty"));
        let twice = [one("A_x64.msi", "s"), one("B_x86_64.msi", "t")].concat();
        assert_eq!(manifest("1.0.0", NOTES, "d", BASE, &twice).unwrap_err(), "two artifacts for windows-x86_64-msi");
    }

    #[test]
    fn names_say_what_they_are() {
        assert_eq!(bundle("RustRadar_0.3.15_x64-setup.exe"), Some(("windows", "nsis")));
        assert_eq!(bundle("RustRadar_0.3.15_x64_en-US.msi"), Some(("windows", "msi")));
        assert_eq!(bundle("RustRadar_0.3.15_x64.app.tar.gz"), Some(("darwin", "app")));
        assert_eq!(bundle("RustRadar_0.3.15_amd64.AppImage"), Some(("linux", "appimage")));
        assert_eq!(bundle("RustRadar_0.3.15_amd64.deb"), Some(("linux", "deb")));
        assert_eq!(bundle("RustRadar-0.3.15-1.x86_64.rpm"), Some(("linux", "rpm")));
        for other in ["RustRadar_0.3.15_x64.dmg", "RustRadar_0.3.15.apk", "RustRadar_0.3.15.ipa", "x.msi.sig", "latest.json", "a.tar.gz"] {
            assert_eq!(bundle(other), None, "{other}");
        }
        assert_eq!(arch("RustRadar-0.3.15-1.x86_64.rpm"), Some("x86_64"));
        assert_eq!(arch("RustRadar_0.3.15_x64_en-US.msi"), Some("x86_64"));
        assert_eq!(arch("RustRadar_0.3.15_amd64.deb"), Some("x86_64"));
        assert_eq!(arch("RustRadar_0.3.15_aarch64.app.tar.gz"), Some("aarch64"));
        assert_eq!(arch("rustradar_0.3.15_arm64.deb"), Some("aarch64"));
        assert_eq!(arch("RustRadar_0.3.15_x86-setup.exe"), Some("i686"));
        assert_eq!(arch("RustRadar-0.3.15-1.i686.rpm"), Some("i686"));
        assert_eq!(arch("RustRadar_0.3.15_armhf.deb"), Some("armv7"));
        // only whole parts count
        assert_eq!(arch("RustRadar.app.tar.gz"), None);
        assert_eq!(arch("Max64_1.0.msi"), None);
        assert_eq!(arch("RustRadar_x640.msi"), None);
    }

    #[test]
    fn dates_are_written_as_to_iso_string() {
        assert_eq!(iso_string(1_794_385_275_913).unwrap(), "2026-11-11T08:21:15.913Z");
        assert_eq!(iso_string(1_794_385_275_000).unwrap(), "2026-11-11T08:21:15.000Z");
        assert_eq!(iso_string(0).unwrap(), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_string(5).unwrap(), "1970-01-01T00:00:00.005Z");
    }

    #[test]
    fn artifacts_are_found_with_their_signatures() {
        let dir = std::env::temp_dir().join(format!("xtask-latest-json-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bundle-windows-x86_64")).unwrap();
        fs::create_dir_all(dir.join("bundle-macos-aarch64")).unwrap();
        let files = [
            ("bundle-windows-x86_64/RustRadar_1.0.0_x64-setup.exe", "exe"),
            ("bundle-windows-x86_64/RustRadar_1.0.0_x64-setup.exe.sig", "sig-exe"),
            ("bundle-windows-x86_64/RustRadar_1.0.0_x64_en-US.msi", "msi"),
            ("bundle-macos-aarch64/RustRadar_1.0.0_aarch64.app.tar.gz", "tgz"),
            ("bundle-macos-aarch64/RustRadar_1.0.0_aarch64.app.tar.gz.sig", "sig-tgz"),
            ("bundle-macos-aarch64/RustRadar_1.0.0_aarch64.dmg", "dmg"),
        ];
        for (file, text) in files {
            fs::write(dir.join(file), text).unwrap();
        }
        let found = artifacts(&dir).unwrap();
        let names: Vec<(&str, &str)> = found.iter().map(|(_, a)| (a.name.as_str(), a.signature.as_str())).collect();
        // the MSI has no .sig: left out; the dmg is no updater artifact
        assert_eq!(names, [("RustRadar_1.0.0_aarch64.app.tar.gz", "sig-tgz"), ("RustRadar_1.0.0_x64-setup.exe", "sig-exe")]);
        let _ = fs::remove_dir_all(&dir);
    }
}
