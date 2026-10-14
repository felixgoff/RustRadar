//! Self-update before the app opens.
//!
//! The main window starts hidden. On launch this checks GitHub for a newer
//! build; if there is one, a small progress window appears, the update is
//! downloaded and installed, and the app relaunches as the new version. The
//! user never sees the old version's interface and never presses a button. With
//! no update, nothing is shown and the main window opens straight away.
//!
//! An update is never a reason not to start: offline, a slow or failing
//! download, a bad signature all fall through to opening the version that is
//! already installed.

use std::error::Error;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::window::Color;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_updater::{Update, UpdaterExt};

/// How long to wait for the update check before opening the current version.
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
/// Passed to the relaunched app so a failed install can't loop forever.
const UPDATED_FLAG: &str = "--updated";
const SPLASH: &str = "updater";

type BoxError = Box<dyn Error + Send + Sync>;

/// What the progress window shows; see `static/updater.html`.
#[derive(Serialize)]
struct Progress<'a> {
    status: &'a str,
    /// 0 to 1, or `None` while the size isn't known.
    fraction: Option<f64>,
}

/// While the app is open, how often to look for a newer release.
const WATCH_EVERY: Duration = Duration::from_secs(60 * 60);
/// The event the window hears once a newer version is downloaded and ready.
const READY_EVENT: &str = "update-ready";

/// A newer version found while the app was open, downloaded and waiting for
/// a restart to install it.
struct Ready {
    update: Update,
    bytes: Vec<u8>,
}

fn ready() -> &'static Mutex<Option<Ready>> {
    static READY: Mutex<Option<Ready>> = Mutex::new(None);
    &READY
}

/// While the app is open: look for a newer release every hour, download it
/// quietly, and tell the window it's ready (it offers a restart). Installing
/// waits for the user; a failed check or download just waits for the next.
async fn watch(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    loop {
        tokio::time::sleep(WATCH_EVERY).await;
        let Ok(updater) = app.updater() else { return };
        let Ok(Ok(Some(update))) = tokio::time::timeout(CHECK_TIMEOUT, updater.check()).await else { continue };
        let known = ready().lock().ok().and_then(|r| r.as_ref().map(|r| r.update.version.clone()));
        if known.as_deref() == Some(update.version.as_str()) {
            continue;
        }
        match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => {
                let version = update.version.clone();
                if let Ok(mut r) = ready().lock() {
                    *r = Some(Ready { update, bytes });
                }
                let _ = app.emit(READY_EVENT, version);
            }
            Err(e) => eprintln!("update download failed: {e}"),
        }
    }
}

/// The version downloaded and waiting for a restart, if any (for a window
/// that missed the event).
#[tauri::command]
pub fn pending_update() -> Option<String> {
    ready().lock().ok()?.as_ref().map(|r| r.update.version.clone())
}

/// Installs the waiting version and starts it. On Windows the installer takes
/// over and relaunches the app itself.
#[tauri::command]
pub fn restart_to_update(app: AppHandle) -> Result<(), String> {
    let Some(Ready { update, bytes }) = ready().lock().map_err(|e| e.to_string())?.take() else {
        return Err("No update is waiting".into());
    };
    update.install(bytes).map_err(|e| format!("Couldn't install the update: {e}"))?;
    relaunch(&app).map_err(|e| e.to_string())
}

/// Update if there is something newer, then open the main window.
pub async fn run(app: AppHandle) {
    if let Err(e) = update(&app).await {
        eprintln!("update skipped: {e}");
    }
    if let Some(splash) = app.get_webview_window(SPLASH) {
        let _ = splash.close();
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    watch(app).await;
}

async fn update(app: &AppHandle) -> Result<(), BoxError> {
    // `tauri dev` runs unsigned builds with nothing to update to
    if cfg!(debug_assertions) || std::env::args().any(|a| a == UPDATED_FLAG) {
        return Ok(());
    }
    let updater = app.updater()?;
    let Some(update) = tokio::time::timeout(CHECK_TIMEOUT, updater.check()).await?? else {
        return Ok(());
    };

    let splash = open_splash(app)?;
    show(&splash, &Progress { status: "Updating RustRadar…", fraction: None });

    let mut downloaded: u64 = 0;
    let mut shown = 0.0;
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                let Some(total) = total.filter(|t| *t > 0) else { return };
                let fraction = (downloaded as f64 / total as f64).min(1.0);
                // a download is thousands of chunks; a script call per percent is plenty
                if fraction - shown >= 0.01 || fraction >= 1.0 {
                    shown = fraction;
                    show(&splash, &Progress { status: "Downloading update…", fraction: Some(fraction) });
                }
            },
            || show(&splash, &Progress { status: "Installing…", fraction: Some(1.0) }),
        )
        .await?;

    // Windows has already handed over to the installer, which relaunches the
    // app itself, and never gets here. Elsewhere the new version is in place.
    relaunch(app)
}

fn open_splash(app: &AppHandle) -> Result<WebviewWindow, BoxError> {
    Ok(WebviewWindowBuilder::new(app, SPLASH, WebviewUrl::App("updater.html".into()))
        .title("RustRadar")
        .inner_size(380.0, 150.0)
        .resizable(false)
        .decorations(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .center()
        .background_color(Color(4, 6, 11, 255))
        .build()?)
}

/// Hand progress to the window's page. It may not have finished loading for the
/// first few calls; the page shows a sensible state until one lands.
fn show(splash: &WebviewWindow, progress: &Progress) {
    if let Ok(json) = serde_json::to_string(progress) {
        let _ = splash.eval(format!("window.__update && window.__update({json})"));
    }
}

/// Start the freshly installed app and quit this one. Goes through
/// `current_binary` rather than `current_exe`, which is stale after an AppImage
/// has been replaced underneath itself.
fn relaunch(app: &AppHandle) -> Result<(), BoxError> {
    let binary = tauri::process::current_binary(&app.env())?;
    Command::new(binary)
        .args(std::env::args_os().skip(1).filter(|a| a != UPDATED_FLAG))
        .arg(UPDATED_FLAG)
        .spawn()?;
    app.exit(0);
    Ok(())
}
