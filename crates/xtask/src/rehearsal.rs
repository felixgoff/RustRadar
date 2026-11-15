//! The upgrade rehearsal's tools (plan stage 11, row P11; `docs/port/upgrade-rehearsal.md` is the
//! procedure). Before a native build may reach `releases/latest`, copies of today's Tauri app
//! (tauri-plugin-updater 2.13.2 in every release from v0.2.0 to v0.3.16, read off each tag's
//! `Cargo.lock`) update themselves to a native build published as a pre-release in a test
//! repository. Two commands make that safer. Neither publishes anything, and neither accepts
//! felixgoff/RustRadar, the repository installed copies update from, as the test's:
//!
//! - `cargo xtask latest-json <dir> --rehearsal OWNER/REPO [--version x.y.z] [--pubkey KEY] [--out FILE]`
//!   (an option of `latest-json`, [`crate::manifest::run`]): the manifest for the test repository's
//!   release `v<version>`, its URLs `https://github.com/OWNER/REPO/releases/download/v<version>/<name>`
//!   ([`download_base`]). Every signature is checked against the key installed copies hold
//!   ([`installed_key`], `update::PUBKEY`, `tauri.conf.json`'s) unless `--pubkey` names another, and
//!   the version each was signed for must be the manifest's ([`check_signed_version`]), as
//!   installed copies require. Then it prints the `gh` command that makes the test repository's
//!   pre-release (`--repo` written out) and the proxy rule that points installed copies at it.
//! - `cargo xtask rehearsal-check <latest.json URL or file> [--pubkey KEY] [--current x.y.z]`
//!   ([`run_check`]): reads a manifest and downloads what it offers as installed copies do it: the
//!   manifest with `Accept: application/json`, each package with `Accept: application/octet-stream`,
//!   both with the updater's user agent ([`INSTALLED_AGENT`]) and reqwest's default of up to ten
//!   redirects, each one printed (gap 7 of the stages 8-10 status: the native manifest links the
//!   release's download URLs where tauri-action linked GitHub's API asset URLs). For every kind of
//!   install today's release makes ([`INSTALLS`]) it finds the entry the updater picks
//!   (`<os>-<arch>-<installer>`, then `<os>-<arch>`: [`lookup`]), checks its signature with the key,
//!   the version the signature was made for, and that the bytes are what that updater installs for
//!   that kind ([`install_problem`]: the NSIS installer, the MSI, a `.app.tar.gz` holding the
//!   executable the old app relaunches, an AppImage, a deb, an rpm). It fails, naming each problem,
//!   when a copy of some kind would refuse the release or find nothing in it.
//!
//! The manifest is read as `RemoteRelease` reads it (`version`, or its alias `name`, with a leading
//! `v` trimmed; `pub_date` RFC 3339 when present; every platform entry a URL and a signature; other
//! fields ignored), except that versions must be `x.y.z`, the only form the releases carry.

use crate::manifest::{Entry, NOTES, REPOSITORY};
use crate::package::plist_entries;
use crate::sign;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::read::GzDecoder;
use minisign::SignatureBox;
use regex::Regex;
use reqwest::header::{ACCEPT, CONTENT_LENGTH};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The user agent installed copies' updater sends: tauri-plugin-updater's crate name and version.
pub const INSTALLED_AGENT: &str = "tauri-plugin-updater/2.13.2";

/// Where this repository keeps the key installed copies check updates with (`update::PUBKEY`, the
/// value of `tauri.conf.json`'s `plugins.updater.pubkey`, which stage 12 deletes).
pub const KEY_SOURCE: &str = "crates/app/src/update/mod.rs";

/// The endpoint compiled into every installed copy (doc 07 §13.6).
pub const ENDPOINT: &str = "https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json";

/// Every kind of install today's release makes: what it is, then its os, architecture and installer
/// as the updater names them (`updater_os`, `updater_arch`, `Installer::name`; tauri-utils
/// 2.10.1's `bundle_type` reads the kind patched into the old binary, `app` on macOS).
pub const INSTALLS: [(&str, &str, &str, &str); 7] = [
    ("Windows, NSIS installer", "windows", "x86_64", "nsis"),
    ("Windows, MSI", "windows", "x86_64", "msi"),
    ("macOS Apple silicon, .app", "darwin", "aarch64", "app"),
    ("macOS Intel, .app", "darwin", "x86_64", "app"),
    ("Linux, AppImage", "linux", "x86_64", "appimage"),
    ("Linux, deb", "linux", "x86_64", "deb"),
    ("Linux, rpm", "linux", "x86_64", "rpm"),
];

/// The executable the old macOS app relaunches after the swap: its own path inside the bundle.
const MACOS_EXECUTABLE: &str = "Contents/MacOS/rustradar";

/// The test repository `arg` names (`OWNER/REPO`, or its github.com URL), refused when it is the
/// repository installed copies update from, in any spelling.
pub fn test_repository(arg: &str) -> Result<String, String> {
    let mut repo = arg.trim();
    for prefix in ["https://", "http://", "www.", "github.com/"] {
        repo = repo.strip_prefix(prefix).unwrap_or(repo);
    }
    let repo = repo.trim_end_matches('/');
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let usage = || format!("--rehearsal takes the test repository as OWNER/REPO, not `{arg}`");
    let (owner, name) = repo.split_once('/').ok_or_else(usage)?;
    let owner_ok = !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let name_ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !owner_ok || !name_ok {
        return Err(usage());
    }
    if repo.eq_ignore_ascii_case(REPOSITORY) {
        return Err(format!(
            "{REPOSITORY} is the repository installed copies update from: a rehearsal never publishes there. \
             Name the test repository (docs/port/upgrade-rehearsal.md)"
        ));
    }
    Ok(format!("{owner}/{name}"))
}

/// Where the test repository's release `v<version>` serves its files.
pub fn download_base(repo: &str, version: &str) -> String {
    format!("https://github.com/{repo}/releases/download/v{version}")
}

/// The public key installed copies check updates with, read from [`KEY_SOURCE`] under `root`.
pub fn installed_key(root: &Path) -> Result<String, String> {
    let path = root.join(KEY_SOURCE);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let pattern = Regex::new(r#"(?m)^pub const PUBKEY: &str = "([A-Za-z0-9+/=]+)";$"#).expect("a valid pattern");
    let key = pattern.captures(&text).map(|c| c[1].to_owned()).ok_or_else(|| format!("{}: no `pub const PUBKEY`", path.display()))?;
    sign::public_key(&key)?;
    Ok(key)
}

/// The version a `.sig` file's text was signed for: its trusted comment's `version:` field, which
/// the Tauri CLI and `cargo xtask sign` write (signatures made before Tauri 2 have none). Only
/// meaningful once the signature has verified: the trusted comment is what it signs.
pub fn signed_version(signature: &str) -> Result<Option<String>, String> {
    let decoded = STANDARD.decode(signature.trim()).map_err(|_| "the signature isn't base64".to_owned())?;
    let text = String::from_utf8(decoded).map_err(|_| "the signature isn't text".to_owned())?;
    let signature = SignatureBox::from_string(&text).map_err(|e| format!("not a minisign signature: {e}"))?;
    let comment = signature.trusted_comment().map_err(|e| format!("no trusted comment: {e}"))?;
    Ok(comment.split('\t').find_map(|field| field.strip_prefix("version:")).map(str::to_owned))
}

/// `x.y.z`, a leading `v` trimmed as the updater trims it.
pub fn version(text: &str) -> Result<(u64, u64, u64), String> {
    let bad = || format!("`{text}` is not a version x.y.z");
    let parts: Vec<u64> = text.trim().trim_start_matches('v').split('.').map(|p| p.parse().map_err(|_| bad())).collect::<Result<_, _>>()?;
    match parts[..] {
        [major, minor, patch] => Ok((major, minor, patch)),
        _ => Err(bad()),
    }
}

/// tauri-plugin-updater 2.13.2's `verify_signed_version`: a signature that names the version it was
/// made for must name the announced one (compared as versions), or installed copies refuse it.
pub fn check_signed_version(signature: &str, announced: &str) -> Result<(), String> {
    match signed_version(signature)? {
        None => Ok(()),
        Some(signed) if version(&signed).ok() == Some(version(announced)?) || signed == announced => Ok(()),
        Some(signed) => Err(format!("signed for version {signed}, announced as {announced}: installed copies refuse it")),
    }
}

/// The entry a copy installed as `installer` takes: `<os>-<arch>-<installer>`, else `<os>-<arch>`
/// (the updater's `get_urls`), with the key it was found under.
pub fn lookup<'a>(platforms: &'a BTreeMap<String, Entry>, os: &str, arch: &str, installer: &str) -> Option<(String, &'a Entry)> {
    [format!("{os}-{arch}-{installer}"), format!("{os}-{arch}")].into_iter().find_map(|key| platforms.get(&key).map(|entry| (key, entry)))
}

/// A manifest as installed copies read it.
#[derive(Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, Entry>,
}

/// `latest.json`'s text read as `RemoteRelease` reads it (see the module docs).
pub fn read_release(text: &str) -> Result<Release, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let field = |name: &str| value.get(name).filter(|v| !v.is_null());
    let string = |name: &str| -> Result<Option<String>, String> {
        match field(name) {
            None => Ok(None),
            Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("`{name}` is not a string")),
        }
    };
    let version = match string("version")? {
        Some(v) => v,
        None => string("name")?.ok_or("no `version`")?,
    };
    self::version(&version)?;
    let pub_date = string("pub_date")?;
    if let Some(date) = &pub_date {
        date.parse::<jiff::Timestamp>().map_err(|e| format!("invalid value for `pub_date`: {e}"))?;
    }
    let notes = string("notes")?;
    let entries = field("platforms").ok_or("no `platforms`: a server-format answer, which today's releases are not")?;
    let entries = entries.as_object().ok_or("`platforms` is not an object")?;
    let mut platforms = BTreeMap::new();
    for (key, entry) in entries {
        let text = |name: &str| entry.get(name).and_then(|v| v.as_str()).map(str::to_owned).ok_or_else(|| format!("{key}: no `{name}`"));
        let (url, signature) = (text("url")?, text("signature")?);
        reqwest::Url::parse(&url).map_err(|e| format!("{key}: `{url}` is not a URL: {e}"))?;
        platforms.insert(key.clone(), Entry { signature, url });
    }
    Ok(Release { version, notes, pub_date, platforms })
}

/// What would go wrong installing `bytes` into a copy installed as `installer`, as
/// tauri-plugin-updater 2.13.2 installs it (it picks the Windows installer by the bytes, unpacks a
/// gzip'd macOS archive into the bundle with each path's first part dropped, writes an AppImage over
/// the running one, hands a deb or rpm to pkexec), or `None`. `version` is the manifest's.
pub fn install_problem(installer: &str, bytes: &[u8], version: &str) -> Option<String> {
    // the updater's tests (infer 0.22)
    let exe = bytes.len() > 1 && bytes.starts_with(b"MZ");
    let msi = bytes.len() > 7 && bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    let zip = bytes.len() > 3 && bytes.starts_with(b"PK") && matches!(bytes[2..4], [3, 4] | [5, 6] | [7, 8]);
    let gz = bytes.len() > 2 && bytes.starts_with(&[0x1F, 0x8B, 0x08]);
    match installer {
        "nsis" | "msi" if zip => Some("a zip: the updater would run whichever .exe or .msi it finds in it, not a release package".into()),
        "nsis" if exe => None,
        "msi" if msi => None,
        "nsis" if msi => Some("an MSI: an NSIS install would run msiexec, a second copy per machine beside the per-user one".into()),
        "msi" if exe => Some("an NSIS installer: an MSI install would get a second copy per user beside the per-machine one".into()),
        "nsis" | "msi" => Some("neither an installer nor an MSI: the updater refuses it".into()),
        "app" if !gz => Some("not a gzip archive: the updater cannot unpack it into the bundle".into()),
        "app" => app_problem(bytes, version),
        "appimage" if gz => Some("a gzip archive: the updater would unpack an .AppImage from it, not write it over the running one".into()),
        "appimage" => {
            let elf = bytes.len() > 52 && bytes.starts_with(&[0x7F, b'E', b'L', b'F']);
            (!(elf && bytes[8..11] == [b'A', b'I', 2])).then(|| "not a type 2 AppImage: the running one would be overwritten with it".into())
        }
        "deb" => (!(bytes.len() > 20 && bytes.starts_with(b"!<arch>\ndebian-binary"))).then(|| "not a Debian package: the updater refuses it".into()),
        "rpm" => (!(bytes.len() > 96 && bytes.starts_with(&[0xED, 0xAB, 0xEE, 0xDB]))).then(|| "not an rpm: the updater refuses it".into()),
        other => Some(format!("no installer `{other}` in today's release")),
    }
}

/// The macOS archive: one `<name>.app` folder whose contents replace the installed bundle's, with the
/// executable the old app relaunches (its own path, `Contents/MacOS/rustradar`) and today's identity.
fn app_problem(bytes: &[u8], version: &str) -> Option<String> {
    let mut archive = tar::Archive::new(GzDecoder::new(Cursor::new(bytes)));
    let mut tops = Vec::new();
    let (mut executable, mut plist) = (None, None);
    let entries = match archive.entries() {
        Ok(entries) => entries,
        Err(e) => return Some(format!("not a tar archive: {e}")),
    };
    for entry in entries {
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(e) => return Some(format!("the archive breaks off: {e}")),
        };
        let Ok(path) = entry.path().map(|p| p.into_owned()) else { return Some("an entry's path is not readable".into()) };
        let mut parts = path.iter();
        let Some(top) = parts.next().map(|t| t.to_string_lossy().into_owned()) else { continue };
        if !tops.contains(&top) {
            tops.push(top);
        }
        let inner: PathBuf = parts.collect();
        if inner == Path::new(MACOS_EXECUTABLE) {
            executable = Some(entry.header().mode().unwrap_or(0));
        } else if inner == Path::new("Contents/Info.plist") {
            let mut text = String::new();
            if entry.read_to_string(&mut text).is_err() {
                return Some("Contents/Info.plist is not text".into());
            }
            plist = Some(text);
        }
    }
    if tops.len() != 1 || !tops[0].ends_with(".app") {
        return Some(format!("the updater drops each path's first part, so the archive must hold one .app folder, not {tops:?}"));
    }
    match executable {
        None => return Some(format!("no {MACOS_EXECUTABLE}: the old app relaunches that path after the swap")),
        Some(mode) if mode & 0o111 == 0 => return Some(format!("{MACOS_EXECUTABLE} is not executable")),
        Some(_) => {}
    }
    let Some(plist) = plist else { return Some("no Contents/Info.plist".into()) };
    let entries = match plist_entries(&plist) {
        Ok(entries) => entries,
        Err(e) => return Some(format!("Contents/Info.plist: {e}")),
    };
    let get = |key: &str| entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    for (key, want) in [("CFBundleIdentifier", "com.felix.rustradar"), ("CFBundleExecutable", "rustradar"), ("CFBundleShortVersionString", version)] {
        if get(key) != Some(want) {
            return Some(format!("Info.plist's {key} is {:?}, not {want}", get(key)));
        }
    }
    None
}

/// One answer, after the redirects: each hop (the status that sent it on, and where), the final
/// status, the `Content-Length` header the updater's progress reads, and the body.
#[derive(Debug)]
pub struct Fetched {
    pub hops: Vec<(u16, String)>,
    pub status: u16,
    pub length: Option<u64>,
    pub body: Vec<u8>,
}

/// A URL without its query (GitHub's signed asset links), for the log.
pub fn short(url: &str) -> String {
    url.split_once('?').map_or(url, |(base, _)| base).to_owned()
}

/// GET `url` as the updater does: a client of reqwest's defaults (system proxy, platform
/// certificates) with the updater's user agent, `Accept: accept`, up to ten redirects.
pub async fn fetch(url: &str, accept: &str) -> Result<Fetched, String> {
    let hops = Arc::new(Mutex::new(Vec::new()));
    let seen = hops.clone();
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        seen.lock().expect("not poisoned").push((attempt.status().as_u16(), short(attempt.url().as_str())));
        // reqwest's Policy::limited(10), which the updater's client has
        if attempt.previous().len() > 10 { attempt.error("too many redirects") } else { attempt.follow() }
    });
    let client = reqwest::Client::builder().user_agent(INSTALLED_AGENT).redirect(policy).build().map_err(|e| e.to_string())?;
    let response = client.get(url).header(ACCEPT, accept).send().await.map_err(|e| format!("{}: {e}", short(url)))?;
    let status = response.status().as_u16();
    let length = response.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    let body = response.bytes().await.map_err(|e| format!("{}: {e}", short(url)))?.to_vec();
    let hops = hops.lock().expect("not poisoned").clone();
    Ok(Fetched { hops, status, length, body })
}

/// What [`check`] found: the lines of its log and the problems (none: every kind of copy would
/// install the release).
#[derive(Debug, Default)]
pub struct Report {
    pub log: String,
    pub problems: Vec<String>,
}

/// The rehearsal check of the manifest at `source` (an `http(s)` URL, or a file whose URLs are
/// fetched), signatures against `pubkey` (Tauri's form); `current`, when given, must be older.
pub async fn check(source: &str, pubkey: &str, current: Option<&str>) -> Report {
    let mut report = Report::default();
    let key = match sign::public_key(pubkey) {
        Ok(key) => key,
        Err(e) => {
            report.problems.push(format!("the public key: {e}"));
            return report;
        }
    };
    let text = if source.starts_with("http://") || source.starts_with("https://") {
        let _ = writeln!(report.log, "manifest {}", short(source));
        match fetch(source, "application/json").await {
            Ok(answer) => {
                hop_lines(&mut report.log, &answer);
                if !(200..300).contains(&answer.status) || answer.status == 204 {
                    report.problems.push(format!("the manifest answers {}: installed copies find no update there", answer.status));
                    return report;
                }
                String::from_utf8_lossy(&answer.body).into_owned()
            }
            Err(e) => {
                report.problems.push(format!("the manifest: {e}"));
                return report;
            }
        }
    } else {
        let _ = writeln!(report.log, "manifest {source} (a file)");
        match fs::read_to_string(source) {
            Ok(text) => text,
            Err(e) => {
                report.problems.push(format!("{source}: {e}"));
                return report;
            }
        }
    };
    let release = match read_release(&text) {
        Ok(release) => release,
        Err(e) => {
            report.problems.push(format!("installed copies cannot read the manifest: {e}"));
            return report;
        }
    };
    let _ = writeln!(report.log, "version {}, pub_date {}, {} platform keys", release.version, release.pub_date.as_deref().unwrap_or("none"), release.platforms.len());
    if let Some(current) = current {
        match (version(current), version(&release.version)) {
            (Ok(installed), Ok(offered)) if offered > installed => {}
            (Ok(_), Ok(_)) => report.problems.push(format!("{} is not newer than {current}: installed copies of {current} would not update", release.version)),
            (Err(e), _) | (_, Err(e)) => report.problems.push(e),
        }
    }

    // which entry each kind of install takes, grouped by the URL it downloads
    let mut by_url: BTreeMap<String, Vec<(&str, &str, String)>> = BTreeMap::new();
    let mut signatures: BTreeMap<String, String> = BTreeMap::new();
    for (what, os, arch, installer) in INSTALLS {
        match lookup(&release.platforms, os, arch, installer) {
            Some((found, entry)) => {
                let _ = writeln!(report.log, "{what:<28} reads {found:<24} {}", entry.url.rsplit('/').next().unwrap_or_default());
                by_url.entry(entry.url.clone()).or_default().push((what, installer, found));
                if signatures.insert(entry.url.clone(), entry.signature.clone()).is_some_and(|s| s != entry.signature) {
                    report.problems.push(format!("{}: two signatures for one URL", short(&entry.url)));
                }
            }
            None => report.problems.push(format!("{what}: no {os}-{arch}-{installer} and no {os}-{arch}: installed copies find no update (the check fails, the installed version opens)")),
        }
    }

    for (url, installs) in &by_url {
        let name = url.rsplit('/').next().unwrap_or(url);
        let _ = writeln!(report.log, "download {}", short(url));
        let answer = match fetch(url, "application/octet-stream").await {
            Ok(answer) => answer,
            Err(e) => {
                report.problems.push(format!("{name}: {e}"));
                continue;
            }
        };
        hop_lines(&mut report.log, &answer);
        if !(200..300).contains(&answer.status) {
            report.problems.push(format!("{name}: the download answers {}", answer.status));
            continue;
        }
        let size = answer.body.len() as f64 / 1_048_576.0;
        let length = match answer.length {
            Some(n) if n == answer.body.len() as u64 => "Content-Length given",
            Some(_) => "Content-Length differs from the body",
            None => "no Content-Length (the splash shows no fraction)",
        };
        let _ = writeln!(report.log, "    {size:.1} MB, {length}");
        let signature = &signatures[url];
        match sign::verify(&key, signature, Cursor::new(&answer.body)).and_then(|()| check_signed_version(signature, &release.version)) {
            Ok(()) => {
                let signed = signed_version(signature).ok().flatten().unwrap_or_else(|| "no version".to_owned());
                let _ = writeln!(report.log, "    signature verifies with the key, signed for {signed}");
            }
            Err(e) => report.problems.push(format!("{name}: {e}")),
        }
        for (what, installer, _) in installs {
            match install_problem(installer, &answer.body, &release.version) {
                None => {
                    let _ = writeln!(report.log, "    installs into: {what}");
                }
                Some(problem) => report.problems.push(format!("{name} for {what}: {problem}")),
            }
        }
    }
    report
}

fn hop_lines(log: &mut String, answer: &Fetched) {
    for (status, to) in &answer.hops {
        let _ = writeln!(log, "    {status} -> {to}");
    }
    let _ = writeln!(log, "    {} ({} bytes)", answer.status, answer.body.len());
}

/// `latest-json --rehearsal`'s last words: the commands that come next (docs/port/upgrade-rehearsal.md).
pub fn next_steps(repo: &str, version: &str, files: &[PathBuf]) -> String {
    let quoted: Vec<String> = files
        .iter()
        .map(|f| {
            let f = f.display().to_string();
            if f.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) { f } else { format!("'{}'", f.replace('\'', r"'\''")) }
        })
        .collect();
    let mut text = String::new();
    let _ = writeln!(text, "\nrehearsal: latest.json for {repo}'s release v{version}, every signature checked with the key installed copies hold");
    let _ = writeln!(text, "next, the pre-release in the test repository (nothing else: --repo names it):");
    let _ = writeln!(
        text,
        "  gh release create v{version} --repo {repo} --prerelease --title \"RustRadar v{version}\" --notes \"{NOTES}\" \\\n    {}",
        quoted.join(" \\\n    ")
    );
    let _ = writeln!(text, "then read it as installed copies will:");
    let _ = writeln!(text, "  cargo xtask rehearsal-check {}/latest.json --current <the version the copies run>", download_base(repo, version));
    let _ = writeln!(text, "the proxy's rule that points installed copies at the test repository, once its release is marked latest:");
    let _ = writeln!(
        text,
        "  --map-remote '|^{}$|https://github.com/{repo}/releases/latest/download/latest.json'",
        regex::escape(ENDPOINT).replace("\\/", "/")
    );
    text
}

const USAGE: &str = "usage: cargo xtask rehearsal-check <latest.json URL or file> [--pubkey KEY] [--current x.y.z]";

pub fn run_check(args: &[String]) -> Result<(), String> {
    let (mut source, mut pubkey, mut current) = (None, None, None);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().cloned().ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--pubkey" => pubkey = Some(value()?),
            "--current" => current = Some(value()?),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}\n{USAGE}")),
            other if source.is_none() => source = Some(other.to_owned()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let source = source.ok_or(USAGE)?;
    let pubkey = match pubkey {
        Some(key) => {
            println!("note: checking with the key given, not the one installed copies hold ({KEY_SOURCE})");
            key
        }
        None => installed_key(&crate::bump::repo_root())?,
    };
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let report = runtime.block_on(check(&source, &pubkey, current.as_deref()));
    print!("{}", report.log);
    if report.problems.is_empty() {
        println!("every kind of installed copy would install this release");
        Ok(())
    } else {
        Err(format!("{} problems:\n  {}", report.problems.len(), report.problems.join("\n  ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{self, Artifact};
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use minisign::KeyPair;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};

    /// A fresh key: the secret key and the public key as `tauri.conf.json` holds one.
    fn keys() -> (minisign::SecretKey, String) {
        let pair = KeyPair::generate_unencrypted_keypair().unwrap();
        (pair.sk, STANDARD.encode(pair.pk.to_box().unwrap().to_string()))
    }

    fn signed(key: &minisign::SecretKey, data: &[u8], name: &str, version: &str) -> String {
        sign::signature(key, data, name, version, 1_794_385_122).unwrap()
    }

    fn padded(head: &[u8]) -> Vec<u8> {
        let mut bytes = head.to_vec();
        bytes.resize(4096, 0x5A);
        bytes
    }

    fn info_plist(id: &str, version: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>CFBundleExecutable</key>\n\t<string>rustradar</string>\n\
             \t<key>CFBundleIdentifier</key>\n\t<string>{id}</string>\n\t<key>CFBundleShortVersionString</key>\n\t<string>{version}</string>\n</dict>\n</plist>\n"
        )
    }

    /// A `.app.tar.gz` holding `files` (path, mode, contents), as tauri-bundler's create_tar lays one out.
    fn app_archive(files: &[(&str, u32, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for (path, mode, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            tar.append_data(&mut header, path, *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    fn good_app(version: &str) -> Vec<u8> {
        let plist = info_plist("com.felix.rustradar", version);
        app_archive(&[
            ("RustRadar.app/Contents/Info.plist", 0o644, plist.as_bytes()),
            ("RustRadar.app/Contents/MacOS/rustradar", 0o755, b"\xcf\xfa\xed\xfe a Mach-O"),
            ("RustRadar.app/Contents/Resources/icon.icns", 0o644, b"icns"),
        ])
    }

    fn appimage() -> Vec<u8> {
        padded(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0, b'A', b'I', 2])
    }

    /// Today's release at `version`: each package's name and bytes.
    fn packages(version: &str) -> Vec<(String, Vec<u8>)> {
        vec![
            (format!("RustRadar_{version}_x64-setup.exe"), padded(b"MZ\x90\x00 NSIS")),
            (format!("RustRadar_{version}_x64_en-US.msi"), padded(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])),
            (format!("RustRadar_{version}_aarch64.app.tar.gz"), good_app(version)),
            (format!("RustRadar_{version}_x64.app.tar.gz"), good_app(version)),
            (format!("RustRadar_{version}_amd64.AppImage"), appimage()),
            (format!("RustRadar_{version}_amd64.deb"), padded(b"!<arch>\ndebian-binary   ")),
            (format!("RustRadar-{version}-1.x86_64.rpm"), padded(&[0xED, 0xAB, 0xEE, 0xDB, 3, 0])),
        ]
    }

    #[derive(Debug, Clone)]
    struct Seen {
        path: String,
        accept: String,
        agent: String,
    }

    type Routes = Arc<Mutex<HashMap<String, (u16, Option<String>, Vec<u8>)>>>;

    /// An HTTP/1.1 server on 127.0.0.1: fixed answers, redirects with a Location.
    struct Server {
        base: String,
        routes: Routes,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Server {
        fn start() -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (routes, seen): (Routes, Arc<Mutex<Vec<Seen>>>) = Default::default();
            let (r, s) = (routes.clone(), seen.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (r, s) = (r.clone(), s.clone());
                    std::thread::spawn(move || answer(stream, &r, &s));
                }
            });
            Server { base, routes, seen }
        }

        fn route(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
            self.routes.lock().unwrap().insert(path.to_owned(), (status, None, body.into()));
        }

        /// `from` answers 302 to `to` (a path on this server).
        fn redirect(&self, from: &str, to: &str) {
            let location = format!("{}{to}?sig=short-lived", self.base);
            self.routes.lock().unwrap().insert(from.to_owned(), (302, Some(location), Vec::new()));
        }

        fn seen(&self, path: &str) -> Vec<Seen> {
            self.seen.lock().unwrap().iter().filter(|s| s.path == path).cloned().collect()
        }

        /// A release as GitHub serves one: `/dl/<name>` sends the package on to `/cdn/<name>`, and
        /// `/latest/latest.json` to `/v/latest.json`. Returns the manifest's URL behind the redirect.
        fn publish(&self, files: &[(String, Vec<u8>)], manifest: &str) -> String {
            for (name, bytes) in files {
                self.redirect(&format!("/dl/{name}"), &format!("/cdn/{name}"));
                self.route(&format!("/cdn/{name}"), 200, bytes.clone());
            }
            self.route("/v/latest.json", 200, manifest);
            self.redirect("/latest/latest.json", "/v/latest.json");
            format!("{}/latest/latest.json", self.base)
        }
    }

    fn answer(stream: TcpStream, routes: &Routes, seen: &Mutex<Vec<Seen>>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let path = line.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_owned();
        let (mut accept, mut agent) = (String::new(), String::new());
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                match name.to_ascii_lowercase().as_str() {
                    "accept" => accept = value.trim().to_owned(),
                    "user-agent" => agent = value.trim().to_owned(),
                    _ => {}
                }
            }
        }
        seen.lock().unwrap().push(Seen { path: path.clone(), accept, agent });
        let (status, location, body) = routes.lock().unwrap().get(&path).cloned().unwrap_or((404, None, b"Not Found".to_vec()));
        let mut stream = stream;
        let location = location.map(|l| format!("Location: {l}\r\n")).unwrap_or_default();
        let head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n", body.len());
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body);
    }

    /// The manifest `latest-json` writes for `files`, signed with `key` (per file: the name and
    /// version it is signed for may be overridden), downloaded from `server`'s `/dl`.
    fn manifest_for(server: &Server, version: &str, files: &[(String, Vec<u8>)], sign_as: impl Fn(&str) -> (minisign::SecretKey, String)) -> String {
        let artifacts: Vec<Artifact> = files
            .iter()
            .map(|(name, bytes)| {
                let (key, signed_version) = sign_as(name);
                Artifact { name: name.clone(), signature: signed(&key, bytes, name, &signed_version) }
            })
            .collect();
        let m = manifest::manifest(version, NOTES, "2026-11-15T12:00:00.000Z", &format!("{}/dl", server.base), &artifacts).unwrap();
        manifest::to_json(&m)
    }

    fn run(source: &str, pubkey: &str, current: Option<&str>) -> Report {
        tokio::runtime::Runtime::new().unwrap().block_on(check(source, pubkey, current))
    }

    #[test]
    fn a_good_release_installs_into_every_kind_of_copy() {
        let (key, public) = keys();
        let server = Server::start();
        let files = packages("0.3.17");
        let text = manifest_for(&server, "0.3.17", &files, |_| (key.clone(), "0.3.17".to_owned()));
        let url = server.publish(&files, &text);
        let report = run(&url, &public, Some("0.3.16"));
        assert!(report.problems.is_empty(), "{:?}\n{}", report.problems, report.log);

        // each kind took the key the updater looks up first, and the package of its kind
        for (what, key, file) in [
            ("Windows, NSIS installer", "windows-x86_64-nsis", "x64-setup.exe"),
            ("Windows, MSI", "windows-x86_64-msi", "x64_en-US.msi"),
            ("macOS Apple silicon, .app", "darwin-aarch64-app", "aarch64.app.tar.gz"),
            ("macOS Intel, .app", "darwin-x86_64-app", "x64.app.tar.gz"),
            ("Linux, AppImage", "linux-x86_64-appimage", "amd64.AppImage"),
            ("Linux, deb", "linux-x86_64-deb", "amd64.deb"),
            ("Linux, rpm", "linux-x86_64-rpm", "x86_64.rpm"),
        ] {
            let line = report.log.lines().find(|l| l.starts_with(what)).unwrap_or_else(|| panic!("{what}\n{}", report.log));
            assert!(line.contains(&format!("reads {key} ")) && line.ends_with(file), "{line}");
            assert!(report.log.contains(&format!("installs into: {what}")), "{what}");
        }
        // the redirects are followed and logged without their query
        assert!(report.log.contains(&format!("302 -> {}/v/latest.json\n", server.base)), "{}", report.log);
        assert!(report.log.contains(&format!("302 -> {}/cdn/RustRadar_0.3.17_amd64.deb\n", server.base)));
        assert!(!report.log.contains("sig=short-lived"));
        assert_eq!(report.log.matches("signature verifies with the key, signed for 0.3.17").count(), 7);

        // asked as installed copies ask, each package once
        let manifest = server.seen("/latest/latest.json");
        assert_eq!(manifest.len(), 1);
        assert_eq!((manifest[0].accept.as_str(), manifest[0].agent.as_str()), ("application/json", INSTALLED_AGENT));
        for (name, _) in &files {
            let asked = server.seen(&format!("/dl/{name}"));
            assert_eq!(asked.len(), 1, "{name}");
            assert_eq!((asked[0].accept.as_str(), asked[0].agent.as_str()), ("application/octet-stream", INSTALLED_AGENT), "{name}");
            assert_eq!(server.seen(&format!("/cdn/{name}"))[0].accept, "application/octet-stream", "the Accept header survives the redirect");
        }

        // a file of the same manifest is checked the same way
        let dir = std::env::temp_dir().join(format!("xtask-rehearsal-file-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("latest.json");
        fs::write(&file, &text).unwrap();
        let from_file = run(file.to_str().unwrap(), &public, None);
        assert!(from_file.problems.is_empty(), "{:?}", from_file.problems);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_installed_copies_would_refuse_is_named() {
        let (key, public) = keys();
        let (other, _) = keys();
        let server = Server::start();
        let mut files = packages("0.3.17");
        // the deb signed with another key, the rpm signed for another version, the Intel archive
        // without the executable the old app relaunches, the AppImage gzip'd
        let text = manifest_for(&server, "0.3.17", &files, |name| match name {
            n if n.ends_with(".deb") => (other.clone(), "0.3.17".to_owned()),
            n if n.ends_with(".rpm") => (key.clone(), "0.3.16".to_owned()),
            _ => (key.clone(), "0.3.17".to_owned()),
        });
        let plist = info_plist("com.felix.rustradar", "0.3.17");
        files[3].1 = app_archive(&[("RustRadar.app/Contents/Info.plist", 0o644, plist.as_bytes()), ("RustRadar.app/Contents/MacOS/RustRadar", 0o755, b"x")]);
        let url = server.publish(&files, &text);
        let problems = run(&url, &public, Some("0.3.17")).problems;
        let has = |needle: &str| problems.iter().any(|p| p.contains(needle));
        assert!(has("0.3.17 is not newer than 0.3.17"), "{problems:?}");
        assert!(has("RustRadar_0.3.17_amd64.deb: the signature does not verify"), "{problems:?}");
        assert!(has("RustRadar-0.3.17-1.x86_64.rpm: signed for version 0.3.16, announced as 0.3.17"), "{problems:?}");
        // the archive was re-made after signing: its signature fails as well as its layout
        assert!(has("RustRadar_0.3.17_x64.app.tar.gz: the signature does not verify"), "{problems:?}");
        assert!(has("RustRadar_0.3.17_x64.app.tar.gz for macOS Intel, .app: no Contents/MacOS/rustradar"), "{problems:?}");
        assert_eq!(problems.len(), 5, "{problems:?}");

        // a kind with no entry, a package that is gone, an NSIS install handed the MSI
        let server = Server::start();
        let files = packages("0.3.17");
        let text = manifest_for(&server, "0.3.17", &files, |_| (key.clone(), "0.3.17".to_owned()));
        let mut release: serde_json::Value = serde_json::from_str(&text).unwrap();
        let platforms = release["platforms"].as_object_mut().unwrap();
        platforms.remove("darwin-x86_64");
        platforms.remove("darwin-x86_64-app");
        platforms.remove("windows-x86_64-nsis");
        let url = server.publish(&files, release.to_string().as_str());
        server.route("/cdn/RustRadar_0.3.17_amd64.AppImage", 404, "gone");
        let problems = run(&url, &public, None).problems;
        let has = |needle: &str| problems.iter().any(|p| p.contains(needle));
        assert!(has("macOS Intel, .app: no darwin-x86_64-app and no darwin-x86_64"), "{problems:?}");
        assert!(has("RustRadar_0.3.17_amd64.AppImage: the download answers 404"), "{problems:?}");
        assert!(has("RustRadar_0.3.17_x64_en-US.msi for Windows, NSIS installer: an MSI"), "{problems:?}");
        assert_eq!(problems.len(), 3, "{problems:?}");

        // no manifest at all: the releases/latest of a repository whose only release is a pre-release
        let problems = run(&format!("{}/none/latest.json", server.base), &public, None).problems;
        assert_eq!(problems, ["the manifest answers 404: installed copies find no update there"]);
    }

    #[test]
    fn packages_are_checked_as_the_updater_installs_them() {
        let v = "1.2.3";
        let (exe, msi) = (padded(b"MZ"), padded(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]));
        assert_eq!(install_problem("nsis", &exe, v), None);
        assert_eq!(install_problem("msi", &msi, v), None);
        assert!(install_problem("msi", &exe, v).unwrap().contains("NSIS installer"));
        assert!(install_problem("nsis", &padded(b"PK\x03\x04"), v).unwrap().contains("a zip"));
        assert!(install_problem("msi", &padded(b"%PDF"), v).unwrap().contains("refuses"));
        assert_eq!(install_problem("app", &good_app(v), v), None);
        assert!(install_problem("app", &good_app("1.2.2"), v).unwrap().contains("CFBundleShortVersionString"));
        assert!(install_problem("app", b"not gzip at all", v).unwrap().contains("not a gzip"));
        let plist = info_plist("com.felix.rustradar", v);
        // made inside the bundle (`tar -C RustRadar.app -czf … .`): the updater would drop `Contents`
        let inside = app_archive(&[("Contents/Info.plist", 0o644, plist.as_bytes()), ("Contents/MacOS/rustradar", 0o755, b"x")]);
        assert!(install_problem("app", &inside, v).unwrap().contains("one .app folder"));
        let two = app_archive(&[("RustRadar.app/Contents/Info.plist", 0o644, plist.as_bytes()), ("RustRadar.app/Contents/MacOS/rustradar", 0o755, b"x"), ("Extra.app/x", 0o644, b"x")]);
        assert!(install_problem("app", &two, v).unwrap().contains("one .app folder"));
        let other_id = info_plist("com.example.other", v);
        let foreign = app_archive(&[("RustRadar.app/Contents/Info.plist", 0o644, other_id.as_bytes()), ("RustRadar.app/Contents/MacOS/rustradar", 0o755, b"x")]);
        assert!(install_problem("app", &foreign, v).unwrap().contains("CFBundleIdentifier"));
        let not_executable = app_archive(&[("RustRadar.app/Contents/Info.plist", 0o644, plist.as_bytes()), ("RustRadar.app/Contents/MacOS/rustradar", 0o644, b"x")]);
        assert!(install_problem("app", &not_executable, v).unwrap().contains("not executable"));
        assert_eq!(install_problem("appimage", &appimage(), v), None);
        assert!(install_problem("appimage", &padded(&[0x7F, b'E', b'L', b'F']), v).unwrap().contains("type 2 AppImage"));
        assert!(install_problem("appimage", &good_app(v), v).unwrap().contains("gzip"));
        assert_eq!(install_problem("deb", &padded(b"!<arch>\ndebian-binary"), v), None);
        assert!(install_problem("deb", &padded(b"!<arch>\nnot-debian"), v).is_some());
        assert_eq!(install_problem("rpm", &padded(&[0xED, 0xAB, 0xEE, 0xDB]), v), None);
        assert!(install_problem("rpm", &exe, v).is_some());
    }

    #[test]
    fn the_manifest_is_read_as_installed_copies_read_it() {
        let entry = r#"{ "url": "https://example.test/a.msi", "signature": "c2ln", "extra": 1 }"#;
        let release = read_release(&format!(r#"{{ "name": "v1.2.3", "pub_date": "2026-11-12T04:34:38.648Z", "platforms": {{ "windows-x86_64": {entry} }}, "other": true }}"#)).unwrap();
        assert_eq!(release.version, "v1.2.3");
        assert_eq!(version(&release.version).unwrap(), (1, 2, 3));
        assert_eq!(release.notes, None);
        assert_eq!(release.platforms["windows-x86_64"], Entry { signature: "c2ln".into(), url: "https://example.test/a.msi".into() });
        // today's published manifest (v0.3.16's, its URLs GitHub's API asset links) reads
        let published = r#"{"version":"0.3.16","notes":"n","pub_date":"2026-11-12T04:34:38.648Z","platforms":{"linux-x86_64":{"signature":"s","url":"https://api.github.com/repos/felixgoff/RustRadar/releases/assets/738072902"}}}"#;
        assert_eq!(read_release(published).unwrap().pub_date.as_deref(), Some("2026-11-12T04:34:38.648Z"));
        for (bad, why) in [
            (r#"{ "platforms": {} }"#, "no `version`"),
            (r#"{ "version": "1.2", "platforms": {} }"#, "not a version"),
            (r#"{ "version": "1.2.3", "pub_date": "yesterday", "platforms": {} }"#, "pub_date"),
            (r#"{ "version": "1.2.3", "url": "https://example.test/a", "signature": "s" }"#, "server-format"),
            (r#"{ "version": "1.2.3", "platforms": { "k": { "url": "not a url", "signature": "s" } } }"#, "not a URL"),
            (r#"{ "version": "1.2.3", "platforms": { "k": { "url": "https://example.test/a" } } }"#, "no `signature`"),
            ("<html>", "not JSON"),
        ] {
            let error = read_release(bad).unwrap_err();
            assert!(error.contains(why), "{bad}: {error}");
        }
        // the plain key is the fallback, the installer's key first
        let mut platforms = BTreeMap::new();
        platforms.insert("linux-x86_64".to_owned(), Entry { signature: "a".into(), url: "u1".into() });
        assert_eq!(lookup(&platforms, "linux", "x86_64", "deb").unwrap().0, "linux-x86_64");
        platforms.insert("linux-x86_64-deb".to_owned(), Entry { signature: "b".into(), url: "u2".into() });
        assert_eq!(lookup(&platforms, "linux", "x86_64", "deb").unwrap().0, "linux-x86_64-deb");
        assert!(lookup(&platforms, "darwin", "aarch64", "app").is_none());
    }

    #[test]
    fn signatures_name_the_version_they_were_made_for() {
        let (key, _) = keys();
        let sig = signed(&key, b"x", "a.msi", "0.3.17");
        assert_eq!(signed_version(&sig).unwrap().as_deref(), Some("0.3.17"));
        assert!(check_signed_version(&sig, "0.3.17").is_ok());
        assert!(check_signed_version(&sig, "v0.3.17").is_ok(), "compared as versions");
        assert!(check_signed_version(&sig, "0.3.18").unwrap_err().contains("signed for version 0.3.17, announced as 0.3.18"));
        // a signature without a version (made before Tauri 2) is accepted, as the updater accepts it
        let old = minisign::sign(None, &key, &b"x"[..], Some("timestamp:1\tfile:a.msi"), Some(sign::UNTRUSTED_COMMENT)).unwrap();
        let old = STANDARD.encode(old.to_string());
        assert_eq!(signed_version(&old).unwrap(), None);
        assert!(check_signed_version(&old, "9.9.9").is_ok());
        assert!(signed_version("%%").is_err());
    }

    #[test]
    fn the_test_repository_is_never_the_one_installed_copies_read() {
        for this in ["felixgoff/RustRadar", "FelixGoff/rustradar", "https://github.com/felixgoff/RustRadar", "github.com/felixgoff/RustRadar.git", " felixgoff/RustRadar/ ", "https://www.github.com/felixgoff/rustradar"] {
            let error = test_repository(this).unwrap_err();
            assert!(error.contains("never publishes there"), "{this}: {error}");
        }
        for (arg, repo) in [
            ("me/RustRadar-rehearsal", "me/RustRadar-rehearsal"),
            ("https://github.com/me/RustRadar-rehearsal.git", "me/RustRadar-rehearsal"),
            ("felixgoff/RustRadar-rehearsal", "felixgoff/RustRadar-rehearsal"),
            ("felixgoff/RustRadar.test", "felixgoff/RustRadar.test"),
        ] {
            assert_eq!(test_repository(arg).unwrap(), repo, "{arg}");
        }
        for bad in ["RustRadar", "me/", "/x", "me/a/b", "me/a b", "m e/x", ""] {
            assert!(test_repository(bad).unwrap_err().contains("OWNER/REPO"), "{bad}");
        }
        assert_eq!(download_base("me/R", "0.3.17"), "https://github.com/me/R/releases/download/v0.3.17");
    }

    #[test]
    fn the_installed_key_is_the_apps() {
        let key = installed_key(&crate::bump::repo_root()).unwrap();
        let text = String::from_utf8(STANDARD.decode(&key).unwrap()).unwrap();
        assert!(text.starts_with("untrusted comment: minisign public key: 28F06F796981B11D\n"), "{text}");
        // while tauri.conf.json exists it holds the same key
        let conf = crate::bump::repo_root().join("src-tauri/tauri.conf.json");
        if let Ok(conf) = fs::read_to_string(conf) {
            let conf: serde_json::Value = serde_json::from_str(&conf).unwrap();
            assert_eq!(conf["plugins"]["updater"]["pubkey"], key.as_str());
            assert_eq!(conf["plugins"]["updater"]["endpoints"][0], ENDPOINT);
        }
    }

    #[test]
    fn latest_json_for_a_rehearsal_points_at_the_test_repository_only() {
        let (key, public) = keys();
        let dir = std::env::temp_dir().join(format!("xtask-rehearsal-json-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let files = &packages("9.0.0")[..2];
        for (name, bytes) in files {
            fs::write(dir.join(name), bytes).unwrap();
            fs::write(dir.join(format!("{name}.sig")), signed(&key, bytes, name, "9.0.0")).unwrap();
        }
        let out = dir.join("latest.json");
        let run = |extra: &[&str]| {
            let mut args: Vec<String> = [dir.to_str().unwrap(), "--version", "9.0.0", "--out", out.to_str().unwrap()].map(str::to_owned).to_vec();
            args.extend(extra.iter().map(|a| (*a).to_owned()));
            manifest::run(&args)
        };

        run(&["--rehearsal", "https://github.com/me/RustRadar-rehearsal", "--pubkey", &public]).unwrap();
        let written: manifest::Manifest = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(written.version, "9.0.0");
        assert_eq!(written.platforms.len(), 3, "nsis, msi and the plain windows key");
        for entry in written.platforms.values() {
            assert!(entry.url.starts_with("https://github.com/me/RustRadar-rehearsal/releases/download/v9.0.0/RustRadar_9.0.0_x64"), "{}", entry.url);
        }

        // never this repository, never two URL bases, nothing written when refused
        fs::remove_file(&out).unwrap();
        assert!(run(&["--rehearsal", "felixgoff/RustRadar", "--pubkey", &public]).unwrap_err().contains("never publishes there"));
        assert!(run(&["--rehearsal", "me/R", "--url-base", "https://example.test"]).unwrap_err().contains("leave out --url-base"));
        // by default the key installed copies hold, which did not sign these
        assert!(run(&["--rehearsal", "me/R"]).unwrap_err().contains("the signature does not verify"));
        // and each package signed for the version the manifest announces
        let (name, bytes) = &files[1];
        fs::write(dir.join(format!("{name}.sig")), signed(&key, bytes, name, "8.0.0")).unwrap();
        assert!(run(&["--rehearsal", "me/R", "--pubkey", &public]).unwrap_err().contains("signed for version 8.0.0, announced as 9.0.0"));
        assert!(!out.exists());
        // (without --rehearsal latest-json is as before: it does not read the signed version)
        run(&["--pubkey", &public, "--url-base", "https://example.test/dl"]).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_next_steps_name_the_test_repository_and_only_it() {
        let text = next_steps("me/RustRadar-rehearsal", "0.3.17", &[PathBuf::from("dist/RustRadar_0.3.17_amd64.deb"), PathBuf::from("my dist/latest.json")]);
        assert!(text.contains("gh release create v0.3.17 --repo me/RustRadar-rehearsal --prerelease --title \"RustRadar v0.3.17\""), "{text}");
        assert!(text.contains("    dist/RustRadar_0.3.17_amd64.deb \\\n    'my dist/latest.json'\n"), "{text}");
        assert!(text.contains("cargo xtask rehearsal-check https://github.com/me/RustRadar-rehearsal/releases/download/v0.3.17/latest.json"));
        let rule = text.lines().find(|l| l.contains("--map-remote")).unwrap();
        assert_eq!(
            rule,
            r"  --map-remote '|^https://github\.com/felixgoff/RustRadar/releases/latest/download/latest\.json$|https://github.com/me/RustRadar-rehearsal/releases/latest/download/latest.json'"
        );
        // the rule's pattern matches the endpoint installed copies ask for, and only it
        let pattern = Regex::new(rule.split('|').nth(1).unwrap()).unwrap();
        assert!(pattern.is_match(ENDPOINT));
        assert!(!pattern.is_match("https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json.sig"));
        assert!(!pattern.is_match("https://github.com/felixgoff/RustRadar/releases/download/v0.3.16/latest.json"));
        assert!(!text.contains("--repo felixgoff/RustRadar "));
    }
}
