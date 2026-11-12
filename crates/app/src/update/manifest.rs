//! `latest.json`, the manifest the Tauri updater reads today (doc 07 §7.2, §12.1), and the key of
//! this build's download in it.
//!
//! Ported from `tauri-plugin-updater` 2.13.2 (the version in `Cargo.lock` that today's app runs):
//! `RemoteRelease`'s deserializer (`version`, also read as `name`, with a leading `v` dropped;
//! `notes`; `pub_date` in RFC 3339; either `platforms` keyed by target or one `url` and `signature`,
//! the "dynamic" answer), `updater_os`, `updater_arch`, `Installer::name` and `get_urls`' order:
//! `{os}-{arch}-{installer}` first when the installer is known, then `{os}-{arch}`.
//!
//! The published manifest (v0.3.15) has these keys, each with its own artifact and `.sig`:
//! `darwin-aarch64`, `darwin-aarch64-app`, `darwin-x86_64`, `darwin-x86_64-app`, `linux-x86_64`,
//! `linux-x86_64-appimage`, `linux-x86_64-deb`, `linux-x86_64-rpm`, `windows-x86_64`,
//! `windows-x86_64-msi`, `windows-x86_64-nsis`. The URLs are GitHub API asset URLs, which answer
//! the file only to `Accept: application/octet-stream` ([`super::Update::download`] sends it).
//!
//! **Which installer this build came from.** Tauri's bundler writes it into the executable: the
//! `&str` static `__TAURI_BUNDLE_TYPE` starts as `"__TAURI_BUNDLE_TYPE_VAR_UNK"` and the bundler
//! overwrites those bytes, in place and at the same length, with `…_VAR_DEB`, `_RPM`, `_APP` (the
//! AppImage), `_MSI` or `_NSS` (NSIS) for each package it makes. [`Installer::current`] reads a
//! static of the same name and value, so the packaging items (P5–P7) patch the native binary the
//! same way, once per package, before packing it. An unpatched build knows no installer and looks
//! up `{os}-{arch}` alone, except on macOS where it is the `.app` (as `tauri-utils`'
//! `bundle_type`). The marker's text appears nowhere else in the binary; keep it so.

use std::collections::HashMap;

use reqwest::Url;
use semver::Version;
use serde::Deserialize;

use super::Error;

/// One platform's download in the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    /// Where the artifact is.
    pub url: Url,
    /// The artifact's signature as the Tauri CLI writes it: base64 of the minisign signature file.
    pub signature: String,
}

/// The two shapes an answer comes in (`RemoteReleaseInner`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    /// The server picked the platform itself and answered one `url` and `signature`.
    Dynamic(Platform),
    /// `latest.json`: every platform, keyed by target (`darwin-aarch64`, `windows-x86_64-nsis`, …).
    Static {
        /// The platforms by key.
        platforms: HashMap<String, Platform>,
    },
}

/// A release as the endpoint describes it (`RemoteRelease`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The version on offer.
    pub version: Version,
    /// The release notes.
    pub notes: Option<String>,
    /// When it was published.
    pub pub_date: Option<jiff::Timestamp>,
    /// The downloads.
    pub data: Data,
}

#[derive(Deserialize)]
struct PlatformJson {
    url: String,
    signature: String,
}

#[derive(Deserialize)]
struct ManifestJson {
    #[serde(alias = "name")]
    version: String,
    notes: Option<String>,
    pub_date: Option<String>,
    platforms: Option<HashMap<String, PlatformJson>>,
    url: Option<String>,
    signature: Option<String>,
}

fn url(text: &str) -> Result<Url, Error> {
    Url::parse(text).map_err(|e| Error::Manifest(format!("invalid value for `url`: {e}")))
}

impl PlatformJson {
    fn parse(self) -> Result<Platform, Error> {
        Ok(Platform { url: url(&self.url)?, signature: self.signature })
    }
}

impl Manifest {
    /// Reads a manifest from the endpoint's JSON.
    pub fn from_value(value: serde_json::Value) -> Result<Manifest, Error> {
        let json: ManifestJson = serde_json::from_value(value).map_err(|e| Error::Manifest(e.to_string()))?;
        let version = Version::parse(json.version.trim_start_matches('v')).map_err(|e| Error::Manifest(e.to_string()))?;
        let pub_date = match json.pub_date {
            Some(date) => Some(
                date.parse::<jiff::Timestamp>()
                    .map_err(|e| Error::Manifest(format!("invalid value for `pub_date`: {e}")))?,
            ),
            None => None,
        };
        let data = match json.platforms {
            Some(platforms) => Data::Static {
                platforms: platforms
                    .into_iter()
                    .map(|(key, p)| Ok((key, p.parse()?)))
                    .collect::<Result<_, Error>>()?,
            },
            None => Data::Dynamic(Platform {
                url: url(&json
                    .url
                    .ok_or_else(|| Error::Manifest("the `url` field was not set on the updater response".into()))?)?,
                signature: json
                    .signature
                    .ok_or_else(|| Error::Manifest("the `signature` field was not set on the updater response".into()))?,
            }),
        };
        Ok(Manifest { version, notes: json.notes, pub_date, data })
    }

    /// Reads a manifest from the endpoint's bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Manifest, Error> {
        Manifest::from_value(serde_json::from_slice(bytes).map_err(|e| Error::Manifest(e.to_string()))?)
    }

    /// The download for one key (`download_url` and `signature`); a dynamic answer has one for
    /// every key.
    pub fn platform(&self, key: &str) -> Result<&Platform, Error> {
        match &self.data {
            Data::Dynamic(platform) => Ok(platform),
            Data::Static { platforms } => platforms.get(key).ok_or_else(|| Error::TargetNotFound(key.to_owned())),
        }
    }

    /// The first of `keys` the manifest has, with its download (`get_urls`' loop).
    pub fn entry<'a>(&'a self, keys: &'a [String]) -> Result<(&'a str, &'a Platform), Error> {
        keys.iter()
            .find_map(|key| self.platform(key).ok().map(|p| (key.as_str(), p)))
            .ok_or_else(|| Error::TargetsNotFound(keys.to_vec()))
    }
}

/// The keys to look for, in order: `{os}-{arch}-{installer}` when the installer is known, then
/// `{os}-{arch}`.
pub fn keys(os: &str, arch: &str, installer: Option<Installer>) -> Vec<String> {
    let mut keys = Vec::with_capacity(2);
    if let Some(installer) = installer {
        keys.push(format!("{os}-{arch}-{}", installer.name()));
    }
    keys.push(format!("{os}-{arch}"));
    keys
}

/// The manifest's name for this operating system (`updater_os`): `linux`, `darwin` or `windows`;
/// none on the phones (Android has its own notice, iOS is updated by TestFlight).
pub fn os() -> Option<&'static str> {
    if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "macos") {
        Some("darwin")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else {
        None
    }
}

/// The manifest's name for this architecture (`updater_arch`).
pub fn arch() -> Option<&'static str> {
    if cfg!(target_arch = "x86") {
        Some("i686")
    } else if cfg!(target_arch = "x86_64") {
        Some("x86_64")
    } else if cfg!(target_arch = "arm") {
        Some("armv7")
    } else if cfg!(target_arch = "aarch64") {
        Some("aarch64")
    } else if cfg!(target_arch = "riscv64") {
        Some("riscv64")
    } else {
        None
    }
}

/// The kind of package this build was installed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installer {
    /// Linux AppImage, `appimage`.
    AppImage,
    /// Debian package, `deb`.
    Deb,
    /// RPM package, `rpm`.
    Rpm,
    /// macOS application bundle (also from the `.dmg`), `app`.
    App,
    /// Windows MSI, `msi`.
    Msi,
    /// Windows NSIS installer, `nsis`.
    Nsis,
}

// The packaging patches these bytes (see the module docs). `static mut`, as `tauri-utils` has it,
// so the value is read at run time and the static is not optimized away.
#[used]
static mut __TAURI_BUNDLE_TYPE: &str = "__TAURI_BUNDLE_TYPE_VAR_UNK";

impl Installer {
    /// Its name in the manifest's keys.
    pub fn name(self) -> &'static str {
        match self {
            Installer::AppImage => "appimage",
            Installer::Deb => "deb",
            Installer::Rpm => "rpm",
            Installer::App => "app",
            Installer::Msi => "msi",
            Installer::Nsis => "nsis",
        }
    }

    /// The installer a patched marker names (`bundle_type` with `installer_for_bundle_type`); an
    /// unknown marker is the `.app` on macOS and nothing elsewhere.
    pub fn from_marker(marker: &str) -> Option<Installer> {
        match marker {
            "__TAURI_BUNDLE_TYPE_VAR_DEB" => Some(Installer::Deb),
            "__TAURI_BUNDLE_TYPE_VAR_RPM" => Some(Installer::Rpm),
            "__TAURI_BUNDLE_TYPE_VAR_APP" => Some(Installer::AppImage),
            "__TAURI_BUNDLE_TYPE_VAR_MSI" => Some(Installer::Msi),
            "__TAURI_BUNDLE_TYPE_VAR_NSS" => Some(Installer::Nsis),
            _ if cfg!(target_os = "macos") => Some(Installer::App),
            _ => None,
        }
    }

    /// This build's installer, from the marker the packaging patched.
    pub fn current() -> Option<Installer> {
        // SAFETY: nothing writes the static at run time; the volatile read keeps the compiler from
        // assuming its initial value.
        let marker = unsafe { std::ptr::read_volatile(&raw const __TAURI_BUNDLE_TYPE) };
        Installer::from_marker(marker)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// `latest.json` of the published release v0.3.15, as served.
    pub(in crate::update) const PUBLISHED: &str = r#"{
  "version": "0.3.15",
  "notes": "Installed copies update to this version automatically the next time they open.",
  "pub_date": "2026-11-11T08:21:15.913Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0tzeUpqaEdSTnhlTG1vVUZ6U3VRbjV6YXdGUFc3cjV6NVpjVWJzOG91cURVU0hzWS95cHZiTi9QRXpvZGdXU1huM0JHZEI5c0dsZ1Bza3BUblYyUXdzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg0Nzg4CWZpbGU6UnVzdFJhZGFyLmFwcC50YXIuZ3oJdmVyc2lvbjowLjMuMTUKWTNyVW5lOFRrRGdKTEdVMHNubVRUWldLM0JVL2M2Z2k5NGIvMXIxVFEvNTBydDMyNHlCQ0RzdFM2SWRnN0FTaWJRUko3LzlGVHRzU1NlQzN3bWVCRHc9PQo=",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734704647"
    },
    "darwin-aarch64-app": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0tzeUpqaEdSTnhlTG1vVUZ6U3VRbjV6YXdGUFc3cjV6NVpjVWJzOG91cURVU0hzWS95cHZiTi9QRXpvZGdXU1huM0JHZEI5c0dsZ1Bza3BUblYyUXdzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg0Nzg4CWZpbGU6UnVzdFJhZGFyLmFwcC50YXIuZ3oJdmVyc2lvbjowLjMuMTUKWTNyVW5lOFRrRGdKTEdVMHNubVRUWldLM0JVL2M2Z2k5NGIvMXIxVFEvNTBydDMyNHlCQ0RzdFM2SWRnN0FTaWJRUko3LzlGVHRzU1NlQzN3bWVCRHc9PQo=",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734704647"
    },
    "linux-x86_64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S1BFTUREeVY0K213bUZheERWS0JPWE1kT1FqNWZzVEJONVArV3hVWTJTdFNkTWRkU0h6WjVLZUdkMGVNTEVrWE5FUDB6Ui9UZStkRGxkbzRqNmpTMXcwPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MDE3CWZpbGU6UnVzdFJhZGFyXzAuMy4xNV9hbWQ2NC5BcHBJbWFnZQl2ZXJzaW9uOjAuMy4xNQpVMFkyaWk2Yy9tS1FoRVUvb05jK3RadXpFMXZCUy9BVHd2TVF2RmFrRmhMT2Rna2FhOWE5R1YxU3JFVExoM0pjMDYvMVVDU3pwNDBLVUFxU2YyaUNDUT09Cg==",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734715814"
    },
    "linux-x86_64-appimage": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S1BFTUREeVY0K213bUZheERWS0JPWE1kT1FqNWZzVEJONVArV3hVWTJTdFNkTWRkU0h6WjVLZUdkMGVNTEVrWE5FUDB6Ui9UZStkRGxkbzRqNmpTMXcwPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MDE3CWZpbGU6UnVzdFJhZGFyXzAuMy4xNV9hbWQ2NC5BcHBJbWFnZQl2ZXJzaW9uOjAuMy4xNQpVMFkyaWk2Yy9tS1FoRVUvb05jK3RadXpFMXZCUy9BVHd2TVF2RmFrRmhMT2Rna2FhOWE5R1YxU3JFVExoM0pjMDYvMVVDU3pwNDBLVUFxU2YyaUNDUT09Cg==",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734715814"
    },
    "linux-x86_64-deb": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0l0bGVPaTlITkdDcjR1TTU3QjAzeER1NG1KU0l3aVdnMXMxMGYvUzVDUitleEJDVWlGblQxbkhQeStHVGhMZjA2b09yaUlzNEp3NmMzdHJmQXNVcEE0PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MDE3CWZpbGU6UnVzdFJhZGFyXzAuMy4xNV9hbWQ2NC5kZWIJdmVyc2lvbjowLjMuMTUKcVlNKy9ncHl0UEVMbFBDMDJudzBranBQemJWa3prODRxVzRFWThCaHdyYTA5cGZkcWdnZ1BXendqUUJ4a216V1FRbmMwWFhMMTNaNWtzdFluUlc0RFE9PQo=",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734715608"
    },
    "linux-x86_64-rpm": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0ZhejY4a09YZGVpYjVielVWWXhRRTdTdjdLVUJhUkZNZjNxTnF1c2VhTFpwV1A4c3JuWkdhQWFSYlh3YTZnTVpEY3B2RzJjVUl1Z0wwejZkeWpUY3dJPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MDE3CWZpbGU6UnVzdFJhZGFyLTAuMy4xNS0xLng4Nl82NC5ycG0JdmVyc2lvbjowLjMuMTUKSlU4a2tHUnZpWmMxcUNOOVJpaTBwVlFJZTM5ZkJNU1lxdEZ6djNidnR1OHBqdlBLUHlVR1dJK25jTUpRUjZ1Q1hzWUxWUm1TMnB0NHoyaXJTbE81REE9PQo=",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734715735"
    },
    "windows-x86_64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0gvV01rdjdGQjNrT1pRTGN2ZnZIanRtL2p0OElSbzZUUEltakhiS1hsZjA4bEgweUdhY0hCWnFZY3NuZ04wSG1NNFl2TFlCamRyVHhLVzVKc21uSlFzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MTIyCWZpbGU6UnVzdFJhZGFyXzAuMy4xNV94NjRfZW4tVVMubXNpCXZlcnNpb246MC4zLjE1CmpLVFhBVUlyUHVUbUZZOSt0R2dKVHdNaWhMdTc2VDI5VkZKaWI2d0hpcjN6a3BLaVo3QmV3ZUpuMWpEVEdIMVZGUWFRcytVSU9qK2V1UW40WDJlWUNRPT0K",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734720229"
    },
    "windows-x86_64-msi": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0gvV01rdjdGQjNrT1pRTGN2ZnZIanRtL2p0OElSbzZUUEltakhiS1hsZjA4bEgweUdhY0hCWnFZY3NuZ04wSG1NNFl2TFlCamRyVHhLVzVKc21uSlFzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MTIyCWZpbGU6UnVzdFJhZGFyXzAuMy4xNV94NjRfZW4tVVMubXNpCXZlcnNpb246MC4zLjE1CmpLVFhBVUlyUHVUbUZZOSt0R2dKVHdNaWhMdTc2VDI5VkZKaWI2d0hpcjN6a3BLaVo3QmV3ZUpuMWpEVEdIMVZGUWFRcytVSU9qK2V1UW40WDJlWUNRPT0K",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734720229"
    },
    "windows-x86_64-nsis": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0F5NUNnMHVUb0xzKzRZM2U1djZiWDF5ZVlML3BuVGFISWlSdDgwb3o5dVhFSHM5MVR5Q1lBck9NdUZFdWhXTXBYM1V0ZjhmT205b3dpOHVVcE05MHdJPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MTIyCWZpbGU6UnVzdFJhZGFyXzAuMy4xNV94NjQtc2V0dXAuZXhlCXZlcnNpb246MC4zLjE1ClBDZkhaSjMvK2J6ZEpESnlyY2FQd2tHZnRZSkgyODN4aWZtQmQreDJzQkxzazdMSHJzZ3dIOTBHSDI4RnJZalladE5zK0M3enFCOUhIN2JoQ1pneUJRPT0K",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734720298"
    },
    "darwin-x86_64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0dqZFdQekkwaHZxZHpPMnRPWXRVYzhBVWZFc3pKUGVjSUVDTWtreW5BUU1udW12VjF5RDJJcUtyL2s3UnJHeVNpSEIzbTVpdlFjRDY5cW9MbFduWWdjPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MjY0CWZpbGU6UnVzdFJhZGFyLmFwcC50YXIuZ3oJdmVyc2lvbjowLjMuMTUKZU9TdktONGFFVEFKRk1VTTR3WjViR2MzM2FzOHlSa0V5Y1JGS1pqdmpmcTZPSkdrUEhpOE1teHp0eWpZc3dZaENDV01LYjlCL2JOemVHZmJQdjhYRGc9PQo=",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734726512"
    },
    "darwin-x86_64-app": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVRZHNZRnBlVy93S0dqZFdQekkwaHZxZHpPMnRPWXRVYzhBVWZFc3pKUGVjSUVDTWtreW5BUU1udW12VjF5RDJJcUtyL2s3UnJHeVNpSEIzbTVpdlFjRDY5cW9MbFduWWdjPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MjY0CWZpbGU6UnVzdFJhZGFyLmFwcC50YXIuZ3oJdmVyc2lvbjowLjMuMTUKZU9TdktONGFFVEFKRk1VTTR3WjViR2MzM2FzOHlSa0V5Y1JGS1pqdmpmcTZPSkdrUEhpOE1teHp0eWpZc3dZaENDV01LYjlCL2JOemVHZmJQdjhYRGc9PQo=",
      "url": "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734726512"
    }
  }
}"#;

    fn manifest(json: &str) -> Result<Manifest, Error> {
        Manifest::from_slice(json.as_bytes())
    }

    #[test]
    fn the_published_manifest_reads() {
        let m = manifest(PUBLISHED).unwrap();
        assert_eq!(m.version, Version::new(0, 3, 15));
        assert_eq!(m.notes.as_deref(), Some("Installed copies update to this version automatically the next time they open."));
        assert_eq!(m.pub_date, Some("2026-11-11T08:21:15.913Z".parse().unwrap()));
        let Data::Static { platforms } = &m.data else { panic!("a static manifest") };
        let mut keys: Vec<&str> = platforms.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "darwin-aarch64",
                "darwin-aarch64-app",
                "darwin-x86_64",
                "darwin-x86_64-app",
                "linux-x86_64",
                "linux-x86_64-appimage",
                "linux-x86_64-deb",
                "linux-x86_64-rpm",
                "windows-x86_64",
                "windows-x86_64-msi",
                "windows-x86_64-nsis",
            ]
        );
        let nsis = m.platform("windows-x86_64-nsis").unwrap();
        assert_eq!(nsis.url.as_str(), "https://api.github.com/repos/felixgoff/RustRadar/releases/assets/734720298");
        assert!(nsis.signature.starts_with("dW50cnVzdGVkIGNvbW1lbnQ6"));
    }

    #[test]
    fn the_specific_key_comes_first_then_the_plain_one() {
        let m = manifest(PUBLISHED).unwrap();
        let k = keys("windows", "x86_64", Some(Installer::Nsis));
        assert_eq!(k, ["windows-x86_64-nsis", "windows-x86_64"]);
        let (key, p) = m.entry(&k).unwrap();
        assert_eq!(key, "windows-x86_64-nsis");
        assert!(p.url.as_str().ends_with("/734720298"));
        // no installer known: the plain key, which today is the MSI
        let k = keys("windows", "x86_64", None);
        assert_eq!(k, ["windows-x86_64"]);
        let (key, p) = m.entry(&k).unwrap();
        assert_eq!(key, "windows-x86_64");
        assert!(p.url.as_str().ends_with("/734720229"));
        // an installer the manifest has no key for falls back to the plain key
        let k = keys("darwin", "aarch64", Some(Installer::Nsis));
        assert_eq!(m.entry(&k).unwrap().0, "darwin-aarch64");
        assert_eq!(m.entry(&keys("linux", "x86_64", Some(Installer::AppImage))).unwrap().0, "linux-x86_64-appimage");
        assert_eq!(m.entry(&keys("linux", "x86_64", Some(Installer::Deb))).unwrap().0, "linux-x86_64-deb");
    }

    #[test]
    fn a_platform_the_manifest_lacks_is_an_error() {
        let m = manifest(PUBLISHED).unwrap();
        let k = keys("linux", "aarch64", Some(Installer::AppImage));
        let e = m.entry(&k).unwrap_err();
        assert!(matches!(&e, Error::TargetsNotFound(t) if *t == ["linux-aarch64-appimage", "linux-aarch64"]));
        assert_eq!(
            e.to_string(),
            r#"None of the fallback platforms `["linux-aarch64-appimage", "linux-aarch64"]` were found in the response `platforms` object"#
        );
        assert_eq!(
            m.platform("linux-riscv64").unwrap_err().to_string(),
            "the platform `linux-riscv64` was not found in the response `platforms` object"
        );
    }

    #[test]
    fn versions_read_as_the_plugin_reads_them() {
        let plain = r#"{"version":"1.2.3","platforms":{}}"#;
        assert_eq!(manifest(plain).unwrap().version, Version::new(1, 2, 3));
        assert_eq!(manifest(r#"{"version":"v1.2.3","platforms":{}}"#).unwrap().version, Version::new(1, 2, 3));
        assert_eq!(manifest(r#"{"name":"v2.0.0-beta.1","platforms":{}}"#).unwrap().version, Version::parse("2.0.0-beta.1").unwrap());
        assert!(matches!(manifest(r#"{"version":"1.2","platforms":{}}"#), Err(Error::Manifest(_))));
        assert!(matches!(manifest(r#"{"platforms":{}}"#), Err(Error::Manifest(_))));
        // no notes, no date
        let m = manifest(plain).unwrap();
        assert_eq!((m.notes, m.pub_date), (None, None));
        assert!(matches!(&m.data, Data::Static { platforms } if platforms.is_empty()));
    }

    #[test]
    fn a_bad_date_or_url_refuses_the_manifest() {
        let e = manifest(r#"{"version":"1.0.0","pub_date":"yesterday","platforms":{}}"#).unwrap_err();
        assert!(e.to_string().starts_with("invalid value for `pub_date`"), "{e}");
        let e = manifest(r#"{"version":"1.0.0","platforms":{"linux-x86_64":{"url":"not a url","signature":"x"}}}"#).unwrap_err();
        assert!(e.to_string().starts_with("invalid value for `url`"), "{e}");
        let e = manifest(r#"{"version":"1.0.0","platforms":{"linux-x86_64":{"url":"https://example.com/a"}}}"#).unwrap_err();
        assert!(matches!(e, Error::Manifest(_)));
        assert!(matches!(manifest("[]"), Err(Error::Manifest(_))));
        assert!(matches!(manifest("{"), Err(Error::Manifest(_))));
    }

    #[test]
    fn a_dynamic_answer_serves_every_key() {
        let m = manifest(r#"{"version":"1.0.0","url":"https://example.com/app.tar.gz","signature":"c2ln"}"#).unwrap();
        let k = keys("darwin", "aarch64", Some(Installer::App));
        let (key, p) = m.entry(&k).unwrap();
        assert_eq!(key, "darwin-aarch64-app");
        assert_eq!((p.url.as_str(), p.signature.as_str()), ("https://example.com/app.tar.gz", "c2ln"));
        let e = manifest(r#"{"version":"1.0.0","url":"https://example.com/a"}"#).unwrap_err();
        assert_eq!(e.to_string(), "the `signature` field was not set on the updater response");
        let e = manifest(r#"{"version":"1.0.0","signature":"c2ln"}"#).unwrap_err();
        assert_eq!(e.to_string(), "the `url` field was not set on the updater response");
    }

    #[test]
    fn the_marker_names_the_installer() {
        assert_eq!(Installer::from_marker("__TAURI_BUNDLE_TYPE_VAR_NSS"), Some(Installer::Nsis));
        assert_eq!(Installer::from_marker("__TAURI_BUNDLE_TYPE_VAR_MSI"), Some(Installer::Msi));
        assert_eq!(Installer::from_marker("__TAURI_BUNDLE_TYPE_VAR_APP"), Some(Installer::AppImage));
        assert_eq!(Installer::from_marker("__TAURI_BUNDLE_TYPE_VAR_DEB"), Some(Installer::Deb));
        assert_eq!(Installer::from_marker("__TAURI_BUNDLE_TYPE_VAR_RPM"), Some(Installer::Rpm));
        let unknown = if cfg!(target_os = "macos") { Some(Installer::App) } else { None };
        assert_eq!(Installer::from_marker("__TAURI_BUNDLE_TYPE_VAR_UNK"), unknown);
        // a test binary is not patched
        assert_eq!(Installer::current(), unknown);
        let names: Vec<&str> =
            [Installer::AppImage, Installer::Deb, Installer::Rpm, Installer::App, Installer::Msi, Installer::Nsis].map(Installer::name).to_vec();
        assert_eq!(names, ["appimage", "deb", "rpm", "app", "msi", "nsis"]);
    }

    #[test]
    fn this_platform_has_its_names() {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!((os(), arch()), (Some("linux"), Some("x86_64")));
        }
        assert_eq!(os().is_some(), cfg!(any(target_os = "linux", target_os = "macos", target_os = "windows")));
    }
}
