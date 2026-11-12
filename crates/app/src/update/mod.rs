//! The desktop updater's core (item P2): ask the endpoint for `latest.json`, pick this build's
//! download, compare versions, download with progress and check the signature with the public key
//! compiled in. Doc 07 §7, §12.
//!
//! Today's app does this through `tauri-plugin-updater` 2.13.2 (`RS:updater.rs` calls `check`,
//! `download` and `install`); this module ports the plugin's `check` and `download` with its
//! `verify_signature`, so installs made by the Tauri app and the native app read the same
//! manifest, the same keys and the same signatures (doc 07 §12.1). Installing, the splash, the
//! launch check and the hourly watch are item P3's (`install.rs`, `splash.rs`, declared here by
//! P3); the Android notice is P4's.
//!
//! ```text
//! let updater = Updater::new()?;                       // the endpoint, key and version compiled in
//! match tokio::time::timeout(CHECK_TIMEOUT, updater.check()).await {
//!     Ok(Ok(Some(update))) => {
//!         let bytes = update.download(|chunk, total| { /* progress */ }, || { /* "Installing…" */ }).await?;
//!         // verified: install it (P3)
//!     }
//!     _ => {} // no update, timeout or error: open the installed version
//! }
//! ```
//!
//! - [`Updater::check`]: GET the endpoint with `Accept: application/json`. 204 → no update; any
//!   other failure status is skipped (no release found); an answer that is not JSON is an error at
//!   once, one that is not a release is kept as the error. Then the keys are looked up
//!   ([`manifest`]) and the release is an update only when its version is strictly greater
//!   (semver, so `1.0.0-beta.1 < 1.0.0`). The lookup runs before the comparison, as in the plugin:
//!   a manifest without this platform is an error even when it offers nothing newer.
//! - [`Update::download`]: GET the URL with `Accept: application/octet-stream` (GitHub's API asset
//!   URLs answer the file only to that), `on_chunk(len, Content-Length)` for each chunk,
//!   `on_finish()`, then [`verify_signature`] before the bytes are returned.
//! - [`verify_signature`]: the public key and the signature are base64 of minisign's text files
//!   (Tauri's encoding); both pre-hashed and legacy signatures verify (`allow_legacy`, as the
//!   plugin); then the version in the signed trusted comment (`version:x.y.z`, written by the
//!   Tauri CLI since 2.x, absent in older signatures) must equal the announced one, compared as
//!   semver, so a forged manifest cannot pair a new version number with an older signed artifact.
//!
//! Deviations from the plugin: the user agent is the app's (`RustRadar/{version} (+repository)`)
//! instead of `tauri-plugin-updater/2.13.2`; the endpoint's `{{current_version}}`-style variables,
//! extra headers, the proxy settings and the `requireSignedVersion`/`allowDowngrades` options are
//! left out (today's config uses none of them, so both stay off); the `https` rule for endpoints is
//! the compiled-in endpoint's; an unknown architecture is refused by `check` rather than when the
//! updater is built; downloads have no time limit unless [`Updater::timeout`] sets one (the
//! plugin's default).
//!
//! Trap for P3 and D2: the current version is this crate's `CARGO_PKG_VERSION`, and `rustradar-app`
//! is still `0.1.0` while the Tauri app carries the release version. Until D2 carries the version
//! over (or P3 passes the right one to [`Updater::current_version`]), every published release looks
//! newer.

pub mod android;
pub mod manifest;

use std::fmt;
use std::time::Duration;

use base64::Engine;
use minisign_verify::{PublicKey, Signature};
use reqwest::header::{ACCEPT, CONTENT_LENGTH, HeaderValue};
use reqwest::{StatusCode, Url};
use semver::Version;

pub use manifest::{Installer, Manifest, Platform};

/// Where installed copies look for updates (`tauri.conf.json` → `plugins.updater.endpoints`).
pub const ENDPOINT: &str = "https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json";

/// The public key the release artifacts are signed for (`tauri.conf.json` →
/// `plugins.updater.pubkey`): base64 of the minisign public key file, key id 28F06F796981B11D.
pub const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDI4RjA2Rjc5Njk4MUIxMUQKUldRZHNZRnBlVy93S05xU3BUY2sxM0V1VHdHQ2h4amlxVE1TTXpZN2l5MzJGS1JBaDM1dytjdXIK";

/// How long the launch waits for the check before opening the installed version (`updater.rs`).
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
/// While the app is open, how often it looks for a newer release (`updater.rs`).
pub const WATCH_EVERY: Duration = Duration::from_secs(60 * 60);
/// Passed to the relaunched app so a failed install cannot loop (`updater.rs`).
pub const UPDATED_FLAG: &str = "--updated";

const USER_AGENT: &str = concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)");

/// What went wrong; the texts are the plugin's.
#[derive(Debug)]
pub enum Error {
    /// Not Windows, macOS or Linux.
    UnsupportedOs,
    /// An architecture the manifest has no name for.
    UnsupportedArch,
    /// No endpoint answered a release.
    ReleaseNotFound,
    /// The answer is not a release: its message.
    Manifest(String),
    /// The request failed.
    Http(reqwest::Error),
    /// The download answered a failure status.
    Network(String),
    /// The forced key is not in the manifest.
    TargetNotFound(String),
    /// None of the keys is in the manifest.
    TargetsNotFound(Vec<String>),
    /// The key or the signature is not base64.
    Base64(base64::DecodeError),
    /// The key or the signature decodes to something that is not text.
    SignatureUtf8(String),
    /// minisign refused the key or the signature, or the bytes do not match it.
    Minisign(minisign_verify::Error),
    /// The signature was made for another version than the manifest announces.
    SignedVersionMismatch {
        /// The version in the signed trusted comment.
        signed: String,
        /// The version the manifest announced.
        announced: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnsupportedOs => f.write_str("Unsupported OS, expected one of `linux`, `darwin` or `windows`."),
            Error::UnsupportedArch => {
                f.write_str("Unsupported application architecture, expected one of `x86`, `x86_64`, `arm` or `aarch64`.")
            }
            Error::ReleaseNotFound => f.write_str("Could not fetch a valid release JSON from the remote"),
            Error::Manifest(e) => f.write_str(e),
            Error::Http(e) => write!(f, "{e}"),
            Error::Network(e) => write!(f, "`{e}`"),
            Error::TargetNotFound(t) => write!(f, "the platform `{t}` was not found in the response `platforms` object"),
            Error::TargetsNotFound(t) => {
                write!(f, "None of the fallback platforms `{t:?}` were found in the response `platforms` object")
            }
            Error::Base64(e) => write!(f, "{e}"),
            Error::SignatureUtf8(s) => write!(
                f,
                "The signature {s} could not be decoded, please check if it is a valid base64 string. The signature must be the contents of the `.sig` file generated by the Tauri bundler, as a string."
            ),
            Error::Minisign(e) => write!(f, "{e}"),
            Error::SignedVersionMismatch { signed, announced } => write!(
                f,
                "The update was signed for version {signed} but the update endpoint announced version {announced}. The endpoint response may have been tampered with to force installing a different release."
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Error {
        Error::Http(e)
    }
}

/// The updater's HTTP client: `fr24::client_builder()` (doc 07 §2.3) with the app's user agent.
pub fn client_builder() -> reqwest::ClientBuilder {
    fr24::client_builder().user_agent(USER_AGENT)
}

/// The version of this build.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("cargo checks that the package version is semver")
}

/// Asks the endpoint for a newer release.
#[derive(Debug, Clone)]
pub struct Updater {
    client: reqwest::Client,
    endpoints: Vec<Url>,
    pubkey: String,
    current_version: Version,
    target: Option<String>,
    installer: Option<Installer>,
    timeout: Option<Duration>,
}

impl Updater {
    /// The app's updater: [`ENDPOINT`], [`PUBKEY`], [`current_version`], this build's
    /// [`Installer::current`], a client from [`client_builder`].
    pub fn new() -> Result<Updater, Error> {
        Ok(Updater::with_client(client_builder().build()?))
    }

    /// As [`Updater::new`] with another HTTP client (tests use one without the proxy).
    pub fn with_client(client: reqwest::Client) -> Updater {
        Updater {
            client,
            endpoints: vec![Url::parse(ENDPOINT).expect("the endpoint is a URL")],
            pubkey: PUBKEY.to_owned(),
            current_version: current_version(),
            target: None,
            installer: Installer::current(),
            timeout: None,
        }
    }

    /// The endpoints, asked in order until one answers a release.
    pub fn endpoints(mut self, endpoints: Vec<Url>) -> Updater {
        self.endpoints = endpoints;
        self
    }

    /// The public key, in the Tauri config's encoding.
    pub fn pubkey(mut self, pubkey: impl Into<String>) -> Updater {
        self.pubkey = pubkey.into();
        self
    }

    /// The version to compare with.
    pub fn current_version(mut self, version: Version) -> Updater {
        self.current_version = version;
        self
    }

    /// Look up this key alone instead of this platform's (the plugin's `target`).
    pub fn target(mut self, key: impl Into<String>) -> Updater {
        self.target = Some(key.into());
        self
    }

    /// The installer whose key is looked up first.
    pub fn installer(mut self, installer: Option<Installer>) -> Updater {
        self.installer = installer;
        self
    }

    /// A time limit for each request, the download included.
    pub fn timeout(mut self, timeout: Duration) -> Updater {
        self.timeout = Some(timeout);
        self
    }

    fn get(&self, url: Url, accept: &'static str) -> reqwest::RequestBuilder {
        let request = self.client.get(url).header(ACCEPT, HeaderValue::from_static(accept));
        match self.timeout {
            Some(t) => request.timeout(t),
            None => request,
        }
    }

    /// The newer release, if the endpoint has one for this platform.
    pub async fn check(&self) -> Result<Option<Update>, Error> {
        let keys = match &self.target {
            Some(key) => vec![key.clone()],
            None => {
                let os = manifest::os().ok_or(Error::UnsupportedOs)?;
                let arch = manifest::arch().ok_or(Error::UnsupportedArch)?;
                manifest::keys(os, arch, self.installer)
            }
        };

        let mut release = None;
        let mut last_error = None;
        for url in &self.endpoints {
            log::debug!("checking for updates {url}");
            match self.get(url.clone(), "application/json").send().await {
                Ok(res) if res.status().is_success() => {
                    if res.status() == StatusCode::NO_CONTENT {
                        log::debug!("update endpoint returned 204 No Content");
                        return Ok(None);
                    }
                    let bytes = res.bytes().await?;
                    let json: serde_json::Value =
                        serde_json::from_slice(&bytes).map_err(|e| Error::Manifest(e.to_string()))?;
                    match Manifest::from_value(json) {
                        Ok(m) => {
                            last_error = None;
                            release = Some(m);
                            break;
                        }
                        Err(e) => {
                            log::error!("failed to deserialize update response: {e}");
                            last_error = Some(e);
                        }
                    }
                }
                Ok(res) => log::error!("update endpoint did not respond with a successful status code ({})", res.status()),
                Err(e) => {
                    log::error!("failed to check for updates: {e}");
                    last_error = Some(Error::Http(e));
                }
            }
        }
        if let Some(e) = last_error {
            return Err(e);
        }
        let release = release.ok_or(Error::ReleaseNotFound)?;

        let newer = release.version > self.current_version;
        let (key, platform) = match &self.target {
            Some(key) => (key.as_str(), release.platform(key)?),
            None => release.entry(&keys)?,
        };
        Ok(newer.then(|| Update {
            current_version: self.current_version.clone(),
            version: release.version.to_string(),
            notes: release.notes.clone(),
            date: release.pub_date,
            key: key.to_owned(),
            installer: self.installer,
            download_url: platform.url.clone(),
            signature: platform.signature.clone(),
            updater: self.clone(),
        }))
    }
}

/// A newer release for this platform.
#[derive(Debug, Clone)]
pub struct Update {
    /// The running version.
    pub current_version: Version,
    /// The version on offer, as semver writes it.
    pub version: String,
    /// The release notes.
    pub notes: Option<String>,
    /// When it was published.
    pub date: Option<jiff::Timestamp>,
    /// The manifest key the download came from (`windows-x86_64-nsis`, `linux-x86_64`, …).
    pub key: String,
    /// The installer this build came from, as the key was looked up.
    pub installer: Option<Installer>,
    /// The artifact.
    pub download_url: Url,
    /// Its signature, in the Tauri CLI's encoding.
    pub signature: String,
    updater: Updater,
}

impl Update {
    /// Downloads the artifact and checks its signature: `on_chunk(bytes, Content-Length)` for each
    /// chunk, `on_finish()` once the last has come, the bytes only if the signature holds.
    pub async fn download<C: FnMut(usize, Option<u64>), D: FnOnce()>(
        &self,
        mut on_chunk: C,
        on_finish: D,
    ) -> Result<Vec<u8>, Error> {
        let mut response = self.updater.get(self.download_url.clone(), "application/octet-stream").send().await?;
        if !response.status().is_success() {
            return Err(Error::Network(format!("Download request failed with status: {}", response.status())));
        }
        let total: Option<u64> =
            response.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
        let mut buffer = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            on_chunk(chunk.len(), total);
            buffer.extend_from_slice(&chunk);
        }
        on_finish();
        verify_signature(&buffer, &self.signature, &self.updater.pubkey, &self.version)?;
        Ok(buffer)
    }
}

fn base64_to_string(text: &str) -> Result<String, Error> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(text).map_err(Error::Base64)?;
    String::from_utf8(bytes).map_err(|_| Error::SignatureUtf8(text.to_owned()))
}

/// Checks `data` against a signature with a public key, both in Tauri's encoding (base64 of the
/// minisign files), then the signed version against `announced_version`.
pub fn verify_signature(data: &[u8], signature: &str, pubkey: &str, announced_version: &str) -> Result<(), Error> {
    let public_key = PublicKey::decode(&base64_to_string(pubkey)?).map_err(Error::Minisign)?;
    let signature = Signature::decode(&base64_to_string(signature)?).map_err(Error::Minisign)?;
    public_key.verify(data, &signature, true).map_err(Error::Minisign)?;
    // only now is the trusted comment worth reading: the global signature checked above covers it
    verify_signed_version(signature.trusted_comment(), announced_version)
}

/// The `version:` field of a trusted comment (`timestamp:…\tfile:…\tversion:1.2.3`).
pub fn signed_version(trusted_comment: &str) -> Option<&str> {
    trusted_comment.split('\t').find_map(|field| field.strip_prefix("version:"))
}

fn verify_signed_version(trusted_comment: &str, announced: &str) -> Result<(), Error> {
    // signatures made before the Tauri CLI recorded the version carry none; today's config does not
    // require one (`requireSignedVersion` off)
    let Some(signed) = signed_version(trusted_comment) else { return Ok(()) };
    let matches = match (Version::parse(signed.trim_start_matches('v')), Version::parse(announced.trim_start_matches('v'))) {
        (Ok(s), Ok(a)) => s == a,
        _ => signed == announced,
    };
    if matches {
        Ok(())
    } else {
        Err(Error::SignedVersionMismatch { signed: signed.to_owned(), announced: announced.to_owned() })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    use super::manifest::{arch, os};
    use super::*;

    const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

    // A key pair made once for these tests with the `minisign` crate the Tauri CLI signs with: its
    // secret half was never stored. Public key "minisign public key: 0ACADE49B1707967".
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDBBQ0FERTQ5QjE3MDc5NjcKUldSbmVYQ3hTZDdLQ2gvZ09mRzIzdW0rQXppSzBQVlZXV3RzTUhHWkdET1lYQTFuUDNGYlJITk8K";
    // `payload()` signed as the Tauri CLI signs, trusted comment
    // "timestamp:1794385122\tfile:RustRadar_9.0.0_x64-setup.exe\tversion:9.0.0".
    const SIG_9: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSbmVYQ3hTZDdLQ3VkZHNnMGt2V1RiekdmRlUzRnJtbEZGUVR4RVRtVFpNTzZrcmwxYStpZWpRdzdYWG9BWStJUWRRaWpqcEhCR0ZwUnViQWFIWk5lUjU1MzdpSHYzV3dFPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MTIyCWZpbGU6UnVzdFJhZGFyXzkuMC4wX3g2NC1zZXR1cC5leGUJdmVyc2lvbjo5LjAuMApublhhbmk5Q2dUckpHc3FmY3VGdTBFb1FrMkFRdldxQlRyZ1BxWXJuclFOOHZyMXBwbHJIVnJ0RUY3REtsa21jNmtzbXE1OEZrU3hOUjZFN01qakJEUT09Cg==";
    // The same without the version: "timestamp:1794385122\tfile:RustRadar_9.0.0_x64-setup.exe".
    const SIG_NO_VERSION: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSbmVYQ3hTZDdLQ2orL2duVUdXRHkzcysrWmF3eExVZjJOaWJ6ajYycWd3RmNTZklFeTZlNlFqdXlpa21lVlFkZVJVaUhUMUIzdnQ5OGVFRHErbUZOakFGbTRTbFBzZ0FNPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MTIyCWZpbGU6UnVzdFJhZGFyXzkuMC4wX3g2NC1zZXR1cC5leGUKRUdnTmpvR3hTRm4zVWk4MUpoamFkT1BxZ24wZ3hJQUtPaHVpVlFxWk9ldjRjK3kxQlgzSnlqSUh4dmhVTWE4NVNVVXZSNnprWVhwZEQ0Qmk5cStDQ3c9PQo=";

    /// The tests' artifact: 200 000 bytes of a 32-bit LCG's high bytes (what the signatures sign).
    fn payload() -> Vec<u8> {
        let mut x: u32 = 0x2545_F491;
        (0..200_000)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect()
    }

    fn tampered() -> Vec<u8> {
        let mut bytes = payload();
        bytes[123_456] ^= 1;
        bytes
    }

    #[derive(Debug, Clone)]
    struct Seen {
        path: String,
        accept: String,
        user_agent: String,
    }

    type Routes = Arc<Mutex<HashMap<String, (u16, Vec<u8>)>>>;

    /// An HTTP/1.1 server on 127.0.0.1 answering fixed routes, the body in pieces.
    struct Server {
        base: String,
        routes: Routes,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Server {
        fn start() -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let routes: Routes = Arc::default();
            let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
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
            self.routes.lock().unwrap().insert(path.to_owned(), (status, body.into()));
        }

        fn url(&self, path: &str) -> Url {
            Url::parse(&format!("{}{path}", self.base)).unwrap()
        }

        fn seen(&self, path: &str) -> Vec<Seen> {
            self.seen.lock().unwrap().iter().filter(|s| s.path == path).cloned().collect()
        }

        /// A manifest at `path` offering `version` with `platforms` (key → (artifact path, signature)).
        fn manifest(&self, path: &str, version: &str, platforms: &[(&str, &str, &str)]) {
            let platforms: serde_json::Map<String, serde_json::Value> = platforms
                .iter()
                .map(|(key, artifact, sig)| {
                    ((*key).to_owned(), serde_json::json!({ "signature": sig, "url": self.url(artifact).as_str() }))
                })
                .collect();
            let json = serde_json::json!({
                "version": version,
                "notes": "Installed copies update to this version automatically the next time they open.",
                "pub_date": "2026-11-11T08:21:15.913Z",
                "platforms": platforms,
            });
            self.route(path, 200, json.to_string());
        }

        fn updater(&self, endpoint: &str) -> Updater {
            let client = client_builder().no_proxy().timeout(Duration::from_secs(20)).build().unwrap();
            Updater::with_client(client)
                .endpoints(vec![self.url(endpoint)])
                .pubkey(TEST_PUBKEY)
                .current_version(Version::new(8, 1, 0))
                .installer(Some(Installer::AppImage))
        }
    }

    fn answer(stream: TcpStream, routes: &Routes, seen: &Mutex<Vec<Seen>>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
        let mut headers = HashMap::new();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        let header = |k: &str| headers.get(k).cloned().unwrap_or_default();
        seen.lock().unwrap().push(Seen { path: path.clone(), accept: header("accept"), user_agent: header("user-agent") });
        let (status, body) = routes.lock().unwrap().get(&path).cloned().unwrap_or((404, b"Not Found".to_vec()));
        let reason = match status {
            200 => "OK",
            204 => "No Content",
            404 => "Not Found",
            _ => "Status",
        };
        let mut stream = stream;
        let head = if status == 204 {
            format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n\r\n")
        } else {
            format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
        };
        if stream.write_all(head.as_bytes()).is_err() {
            return;
        }
        // in pieces with pauses, so the client sees several chunks
        for piece in body.chunks(body.len().div_ceil(8).max(1)) {
            if stream.write_all(piece).and_then(|_| stream.flush()).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    fn this_key() -> String {
        format!("{}-{}", os().unwrap(), arch().unwrap())
    }

    #[tokio::test]
    async fn a_correctly_signed_download_verifies() {
        let server = Server::start();
        server.route("/RustRadar_9.0.0.AppImage", 200, payload());
        server.route("/other", 200, tampered());
        let plain = this_key();
        let specific = format!("{plain}-appimage");
        server.manifest(
            "/latest.json",
            "9.0.0",
            &[(&plain, "/other", SIG_9), (&specific, "/RustRadar_9.0.0.AppImage", SIG_9), ("plan9-mips", "/other", SIG_9)],
        );

        let update = server.updater("/latest.json").check().await.unwrap().expect("9.0.0 is newer than 8.1.0");
        assert_eq!(update.version, "9.0.0");
        assert_eq!(update.current_version, Version::new(8, 1, 0));
        assert_eq!(update.key, specific, "the installer's key comes before the plain one");
        assert_eq!(update.installer, Some(Installer::AppImage));
        assert_eq!(update.download_url, server.url("/RustRadar_9.0.0.AppImage"));
        assert_eq!(update.signature, SIG_9);
        assert_eq!(update.notes.as_deref(), Some("Installed copies update to this version automatically the next time they open."));
        assert_eq!(update.date, Some("2026-11-11T08:21:15.913Z".parse().unwrap()));

        let mut chunks = Vec::new();
        let mut finished_after = None;
        let bytes = update
            .download(|len, total| chunks.push((len, total)), || finished_after = Some(()))
            .await
            .unwrap();
        assert_eq!(bytes, payload());
        assert!(finished_after.is_some());
        assert!(chunks.len() > 1, "progress comes chunk by chunk: {}", chunks.len());
        assert_eq!(chunks.iter().map(|c| c.0).sum::<usize>(), 200_000);
        assert!(chunks.iter().all(|c| c.1 == Some(200_000)), "each chunk knows the total");

        // the plugin's headers: JSON for the manifest, the file itself for the artifact
        let asked = server.seen("/latest.json");
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].accept, "application/json");
        assert!(asked[0].user_agent.starts_with("RustRadar/"), "{}", asked[0].user_agent);
        let fetched = server.seen("/RustRadar_9.0.0.AppImage");
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].accept, "application/octet-stream");
        assert!(server.seen("/other").is_empty());
    }

    #[tokio::test]
    async fn a_tampered_download_is_refused() {
        let server = Server::start();
        server.route("/RustRadar_9.0.0.AppImage", 200, tampered());
        server.manifest("/latest.json", "9.0.0", &[(&this_key(), "/RustRadar_9.0.0.AppImage", SIG_9)]);
        let update = server.updater("/latest.json").installer(None).check().await.unwrap().unwrap();
        assert_eq!(update.key, this_key(), "no installer: the plain key");
        let mut got = 0;
        let e = update.download(|len, _| got += len, || {}).await.unwrap_err();
        assert!(matches!(e, Error::Minisign(minisign_verify::Error::InvalidSignature)), "{e:?}");
        assert_eq!(got, 200_000, "the whole file came, then was refused");

        // a signature made with another key is refused before the bytes are looked at
        let update = server.updater("/latest.json").pubkey(PUBKEY).check().await.unwrap().unwrap();
        let e = update.download(|_, _| {}, || {}).await.unwrap_err();
        assert!(matches!(e, Error::Minisign(minisign_verify::Error::UnexpectedKeyId)), "{e:?}");
    }

    #[tokio::test]
    async fn a_manifest_announcing_another_version_than_signed_is_refused() {
        // a forged manifest pairing a higher version with a genuine signed artifact
        let server = Server::start();
        server.route("/a", 200, payload());
        server.manifest("/latest.json", "9.0.1", &[(&this_key(), "/a", SIG_9)]);
        let update = server.updater("/latest.json").check().await.unwrap().unwrap();
        let e = update.download(|_, _| {}, || {}).await.unwrap_err();
        assert!(matches!(&e, Error::SignedVersionMismatch { signed, announced } if signed == "9.0.0" && announced == "9.0.1"), "{e:?}");
        // a signature that names no version is taken for any (today's config)
        server.manifest("/old.json", "9.0.1", &[(&this_key(), "/a", SIG_NO_VERSION)]);
        let update = server.updater("/old.json").check().await.unwrap().unwrap();
        assert_eq!(update.download(|_, _| {}, || {}).await.unwrap(), payload());
    }

    #[tokio::test]
    async fn only_a_greater_version_is_an_update() {
        let server = Server::start();
        server.manifest("/latest.json", "9.0.0", &[(&this_key(), "/a", SIG_9)]);
        let check = |current: &str| {
            let updater = server.updater("/latest.json").current_version(Version::parse(current).unwrap());
            async move { updater.check().await.unwrap().map(|u| u.version) }
        };
        assert_eq!(check("8.9.9").await.as_deref(), Some("9.0.0"));
        assert_eq!(check("9.0.0-beta.2").await.as_deref(), Some("9.0.0"), "semver: a pre-release is older");
        assert_eq!(check("9.0.0").await, None);
        assert_eq!(check("9.0.1").await, None);
        assert_eq!(check("10.0.0").await, None);
        assert!(server.seen("/a").is_empty(), "checking downloads nothing");
    }

    #[tokio::test]
    async fn the_platform_is_looked_up_before_the_versions_are_compared() {
        let server = Server::start();
        server.manifest("/latest.json", "1.0.0", &[("plan9-mips", "/a", SIG_9)]);
        let updater = server.updater("/latest.json");
        let e = updater.clone().current_version(Version::new(2, 0, 0)).check().await.unwrap_err();
        assert!(matches!(&e, Error::TargetsNotFound(k) if *k == [format!("{}-appimage", this_key()), this_key()]), "{e:?}");
        // a forced key is looked up alone
        let update = updater.clone().current_version(Version::new(0, 9, 0)).target("plan9-mips").check().await.unwrap().unwrap();
        assert_eq!(update.key, "plan9-mips");
        let e = updater.target("windows-x86_64-nsis").check().await.unwrap_err();
        assert!(matches!(&e, Error::TargetNotFound(k) if k == "windows-x86_64-nsis"), "{e:?}");
    }

    #[tokio::test]
    async fn answers_without_a_release() {
        let server = Server::start();
        server.route("/empty", 204, Vec::new());
        server.route("/broken", 200, "{\"version\":");
        server.route("/odd", 200, "{\"version\":\"1.0.0\"}");
        server.manifest("/latest.json", "9.0.0", &[(&this_key(), "/a", SIG_9)]);
        assert!(server.updater("/empty").check().await.unwrap().is_none(), "204: no update");
        assert!(matches!(server.updater("/missing").check().await, Err(Error::ReleaseNotFound)), "a failure status: no release");
        assert!(matches!(server.updater("/broken").check().await, Err(Error::Manifest(_))), "not JSON");
        let e = server.updater("/odd").check().await.unwrap_err();
        assert_eq!(e.to_string(), "the `url` field was not set on the updater response");
        // endpoints are asked in order until one answers a release
        let updater = server.updater("/missing").endpoints(vec![server.url("/missing"), server.url("/odd"), server.url("/latest.json")]);
        assert_eq!(updater.check().await.unwrap().unwrap().version, "9.0.0");
        // nobody listening
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let updater = server.updater("/x").endpoints(vec![Url::parse(&format!("http://{closed}/latest.json")).unwrap()]);
        assert!(matches!(updater.check().await, Err(Error::Http(_))));
        // the download's failure status
        let update = server.updater("/latest.json").check().await.unwrap().unwrap();
        let e = update.download(|_, _| {}, || {}).await.unwrap_err();
        assert_eq!(e.to_string(), "`Download request failed with status: 404 Not Found`");
    }

    #[test]
    fn signatures_verify_as_the_plugin_verifies_them() {
        verify_signature(&payload(), SIG_9, TEST_PUBKEY, "9.0.0").unwrap();
        verify_signature(&payload(), SIG_9, TEST_PUBKEY, "v9.0.0").unwrap();
        verify_signature(&payload(), SIG_NO_VERSION, TEST_PUBKEY, "1.2.3").unwrap();
        assert!(matches!(
            verify_signature(&tampered(), SIG_9, TEST_PUBKEY, "9.0.0"),
            Err(Error::Minisign(minisign_verify::Error::InvalidSignature))
        ));
        assert!(matches!(verify_signature(&payload(), SIG_9, TEST_PUBKEY, "9.0.0-rc.1"), Err(Error::SignedVersionMismatch { .. })));
        assert!(matches!(
            verify_signature(&payload(), SIG_9, PUBKEY, "9.0.0"),
            Err(Error::Minisign(minisign_verify::Error::UnexpectedKeyId))
        ));
        // not base64, base64 of bytes that are not text, text that is not a signature
        assert!(matches!(verify_signature(b"x", "not base64!", TEST_PUBKEY, "1.0.0"), Err(Error::Base64(_))));
        assert!(matches!(verify_signature(b"x", &BASE64.encode([0xff, 0xfe]), TEST_PUBKEY, "1.0.0"), Err(Error::SignatureUtf8(_))));
        assert!(matches!(
            verify_signature(b"x", &BASE64.encode("hello"), TEST_PUBKEY, "1.0.0"),
            Err(Error::Minisign(minisign_verify::Error::InvalidEncoding))
        ));
    }

    #[test]
    fn legacy_signatures_verify_too() {
        // minisign-verify's own legacy (not pre-hashed) vector, in Tauri's encoding
        let pubkey = BASE64.encode("untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n");
        let signature = BASE64.encode(
            "untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==\n",
        );
        verify_signature(b"test", &signature, &pubkey, "0.0.1").unwrap();
        assert!(matches!(
            verify_signature(b"Test", &signature, &pubkey, "0.0.1"),
            Err(Error::Minisign(minisign_verify::Error::InvalidSignature))
        ));
    }

    #[test]
    fn the_compiled_in_key_signed_the_published_release() {
        let text = base64_to_string(PUBKEY).unwrap();
        assert_eq!(text, "untrusted comment: minisign public key: 28F06F796981B11D\nRWQdsYFpeW/wKNqSpTck13EuTwGChxjiqTMSMzY7iy32FKRAh35w+cur\n");
        // tauri.conf.json's key, while src-tauri is in the tree (D2 removes it)
        let conf = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/tauri.conf.json");
        if let Ok(conf) = std::fs::read_to_string(conf) {
            let conf: serde_json::Value = serde_json::from_str(&conf).unwrap();
            assert_eq!(conf["plugins"]["updater"]["pubkey"], PUBKEY);
            assert_eq!(conf["plugins"]["updater"]["endpoints"][0], ENDPOINT);
        }
        // every signature of v0.3.15 has the key's id (other bytes fail the signature, not the id)
        // and the version the manifest announces
        let published = Manifest::from_slice(manifest::tests::PUBLISHED.as_bytes()).unwrap();
        let manifest::Data::Static { platforms } = &published.data else { panic!() };
        for (key, p) in platforms {
            let e = verify_signature(b"not the artifact", &p.signature, PUBKEY, "0.3.15").unwrap_err();
            assert!(matches!(e, Error::Minisign(minisign_verify::Error::InvalidSignature)), "{key}: {e:?}");
            let sig = Signature::decode(&base64_to_string(&p.signature).unwrap()).unwrap();
            assert_eq!(signed_version(sig.trusted_comment()), Some("0.3.15"), "{key}");
            assert_eq!(sig.untrusted_comment(), "untrusted comment: signature from tauri secret key");
        }
    }

    #[test]
    fn the_signed_version_is_read_from_the_trusted_comment() {
        assert_eq!(signed_version("timestamp:1794385017\tfile:RustRadar_0.3.15_amd64.AppImage\tversion:0.3.15"), Some("0.3.15"));
        assert_eq!(signed_version("timestamp:1794385017\tfile:a"), None);
        assert!(verify_signed_version("version:v1.2.3", "1.2.3").is_ok());
        assert!(verify_signed_version("version:1.2.3", "1.2.4").is_err());
        // not semver on either side: compared as text
        assert!(verify_signed_version("version:next", "next").is_ok());
        assert!(verify_signed_version("version:next", "1.2.3").is_err());
    }

    #[test]
    fn the_constants_are_todays() {
        assert_eq!(CHECK_TIMEOUT, Duration::from_secs(8));
        assert_eq!(WATCH_EVERY, Duration::from_secs(3600));
        assert_eq!(UPDATED_FLAG, "--updated");
        assert_eq!(current_version().to_string(), env!("CARGO_PKG_VERSION"));
        let updater = Updater::new().unwrap();
        assert_eq!(updater.endpoints, [Url::parse(ENDPOINT).unwrap()]);
        assert_eq!(updater.pubkey, PUBKEY);
        assert_eq!(updater.installer, Installer::current());
    }

    #[tokio::test]
    #[ignore = "needs the network: downloads the published release's NSIS installer (29 MB) and its .sig from GitHub"]
    async fn a_published_release_verifies_with_the_compiled_in_key() {
        let updater = Updater::new().unwrap().current_version(Version::new(0, 0, 0)).target("windows-x86_64-nsis");
        let update = updater.check().await.unwrap().expect("a published release is newer than 0.0.0");
        let mut got = 0;
        let bytes = update.download(|len, _| got += len, || {}).await.expect("verified with the compiled-in key");
        assert_eq!(bytes.len(), got);
        assert!(bytes.starts_with(b"MZ"), "an installer");

        // the release's own .sig is the manifest's signature and verifies the same bytes
        let sig = Signature::decode(&base64_to_string(&update.signature).unwrap()).unwrap();
        let file = sig.trusted_comment().split('\t').find_map(|f| f.strip_prefix("file:")).unwrap().to_owned();
        let url = format!("https://github.com/felixgoff/RustRadar/releases/download/v{}/{file}.sig", update.version);
        let client = client_builder().build().unwrap();
        let text = client.get(url).send().await.unwrap().error_for_status().unwrap().text().await.unwrap();
        assert_eq!(text.trim(), update.signature);
        verify_signature(&bytes, text.trim(), PUBKEY, &update.version).unwrap();
        let mut changed = bytes;
        changed[1000] ^= 0x80;
        assert!(matches!(
            verify_signature(&changed, text.trim(), PUBKEY, &update.version),
            Err(Error::Minisign(minisign_verify::Error::InvalidSignature))
        ));
    }
}
