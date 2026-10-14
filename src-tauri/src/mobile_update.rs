//! Updates on phones. Android installs come from the APK on each GitHub
//! release, which the app can't replace itself. Instead, a newer release
//! raises the same "update ready" toast as on desktop, and its button opens the
//! APK download for Android to install. iOS builds update through TestFlight,
//! so there is nothing to check there.

use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::AppState;

const LATEST: &str = "https://api.github.com/repos/felixgoff/RustRadar/releases/latest";
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
const READY_EVENT: &str = "update-ready";

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

/// The newer version and where its APK is.
fn ready() -> &'static Mutex<Option<(String, String)>> {
    static READY: Mutex<Option<(String, String)>> = Mutex::new(None);
    &READY
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// The release's version and APK, if it is newer than `current` and has one.
fn newer_apk(body: &str, current: &str) -> Option<(String, String)> {
    let release: Release = serde_json::from_str(body).ok()?;
    if parse_version(&release.tag_name)? <= parse_version(current)? {
        return None;
    }
    let apk = release.assets.into_iter().find(|a| a.name.ends_with(".apk"))?;
    Some((release.tag_name.trim_start_matches('v').to_owned(), apk.browser_download_url))
}

async fn check(app: &AppHandle) -> Result<(), String> {
    let http = app.state::<AppState>().client().http().clone();
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
    let current = app.package_info().version.to_string();
    if let Some((version, url)) = newer_apk(&body, &current) {
        *ready().lock().unwrap() = Some((version.clone(), url));
        let _ = app.emit(READY_EVENT, version);
    }
    Ok(())
}

/// Shows the window, then looks for a newer release now and every hour.
pub async fn run(app: AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
    }
    if cfg!(debug_assertions) || cfg!(target_os = "ios") {
        return;
    }
    loop {
        if let Err(e) = check(&app).await {
            eprintln!("update check failed: {e}");
        }
        tokio::time::sleep(CHECK_EVERY).await;
    }
}

#[cfg_attr(mobile, tauri::command)]
pub fn pending_update() -> Option<String> {
    ready().lock().ok()?.as_ref().map(|(v, _)| v.clone())
}

/// Opens the APK download; Android takes it from there.
#[cfg_attr(mobile, tauri::command)]
pub fn restart_to_update(app: AppHandle) -> Result<(), String> {
    let url = ready()
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .map(|(_, url)| url.clone())
        .ok_or("No update is waiting")?;
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
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
