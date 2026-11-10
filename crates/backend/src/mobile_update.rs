//! Updates on phones, the part without the app around it. Android installs
//! come from the APK on each GitHub release, which the app can't replace
//! itself: a newer release with an APK is announced (as on desktop) and its
//! download opened for Android to install. iOS builds update through
//! TestFlight, so there is nothing to check there.
//!
//! The app keeps the rest: when to check, remembering the answer, and
//! opening the download.

use std::time::Duration;

use serde::Deserialize;

/// The latest release, as GitHub's API describes it.
pub const LATEST: &str = "https://api.github.com/repos/felixgoff/RustRadar/releases/latest";
/// How often to look for a newer release while the app is open.
pub const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// `v1.2.3` or `1.2.3` as numbers; anything else is none.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// The release's version and APK, if it is newer than `current` and has one.
pub fn newer_apk(body: &str, current: &str) -> Option<(String, String)> {
    let release: Release = serde_json::from_str(body).ok()?;
    if parse_version(&release.tag_name)? <= parse_version(current)? {
        return None;
    }
    let apk = release.assets.into_iter().find(|a| a.name.ends_with(".apk"))?;
    Some((release.tag_name.trim_start_matches('v').to_owned(), apk.browser_download_url))
}

/// Asks GitHub for the latest release: its version and APK if newer than
/// `current` (the app's version, not this crate's).
pub async fn latest_apk(http: &reqwest::Client, current: &str) -> Result<Option<(String, String)>, String> {
    let body = http
        .get(LATEST)
        .header("User-Agent", "RustRadar")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    Ok(newer_apk(&body, current))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{"tag_name":"v0.4.1","assets":[
        {"name":"RustRadar_0.4.1_amd64.AppImage","browser_download_url":"https://x/a"},
        {"name":"RustRadar_0.4.1.apk","browser_download_url":"https://x/b.apk"}]}"#;

    #[test]
    fn finds_a_newer_apk() {
        assert_eq!(newer_apk(RELEASE, "0.4.0"), Some(("0.4.1".into(), "https://x/b.apk".into())));
        assert_eq!(newer_apk(RELEASE, "0.4.1"), None);
        assert_eq!(newer_apk(RELEASE, "0.10.0"), None);
        assert_eq!(newer_apk(r#"{"tag_name":"v0.5.0","assets":[]}"#, "0.4.0"), None);
    }
}
