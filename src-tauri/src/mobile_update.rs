//! Updates on phones. Android installs come from the APK on each GitHub
//! release, which the app can't replace itself. Instead, a newer release
//! raises the same "update ready" toast as on desktop, and its button opens the
//! APK download for Android to install. iOS builds update through TestFlight,
//! so there is nothing to check there. What a release says is read by
//! `rustradar_backend::mobile_update`.

use std::sync::Mutex;

use rustradar_backend::Backend;
use rustradar_backend::mobile_update::{CHECK_EVERY, latest_apk};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

const READY_EVENT: &str = "update-ready";

/// The newer version and where its APK is.
fn ready() -> &'static Mutex<Option<(String, String)>> {
    static READY: Mutex<Option<(String, String)>> = Mutex::new(None);
    &READY
}

async fn check(app: &AppHandle) -> Result<(), String> {
    let http = app.state::<Backend>().client().http().clone();
    let current = app.package_info().version.to_string();
    if let Some((version, url)) = latest_apk(&http, &current).await? {
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

#[cfg_attr(any(target_os = "android", target_os = "ios"), tauri::command)]
pub fn pending_update() -> Option<String> {
    ready().lock().ok()?.as_ref().map(|(v, _)| v.clone())
}

/// Opens the APK download; Android takes it from there.
#[cfg_attr(any(target_os = "android", target_os = "ios"), tauri::command)]
pub fn restart_to_update(app: AppHandle) -> Result<(), String> {
    let url = ready()
        .lock()
        .map_err(|e| e.to_string())?
        .as_ref()
        .map(|(_, url)| url.clone())
        .ok_or("No update is waiting")?;
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}
