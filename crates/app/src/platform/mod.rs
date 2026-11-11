//! What differs between the platforms (plan 1.2): folders now; opening links, revealing files,
//! the clipboard, the soft keyboard, safe areas and the back gesture as their items land (E19 for
//! `android` and `ios`, U20 for `desktop`). Call sites stay free of `cfg`.

use rustradar_backend::Dirs;

pub mod android;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod desktop;
pub mod ios;

/// Whether this build runs on a phone (`cfg(mobile)` in the Tauri build; doc 07 §11.7).
pub const PHONE: bool = cfg!(any(target_os = "android", target_os = "ios"));

/// What [`reveal`] answers on the phones, where there is no file manager to show a file in (the
/// opener's `UnsupportedPlatform`; doc 07 §11.3).
pub const NOT_ON_THIS_PLATFORM: &str = "Not available on this platform.";

/// Opens `url` in the system's browser (`openUrl(url)`; doc 07 §11.2): [`desktop::open_url`] on
/// the desktop; on the phones the `webbrowser` crate too, as egui-winit opens the UI's links
/// there (an Android intent, `UIApplication.open`), not yet tried on a device.
pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    return desktop::open_url(url);
    #[cfg(any(target_os = "android", target_os = "ios"))]
    return webbrowser::open(url).map_err(|e| format!("Couldn't open {url}: {e}"));
}

/// Shows `path` in the file manager (`revealItemInDir(path)`, whose failures every caller ignores):
/// [`desktop::reveal`] on a thread of its own, a failure logged. The phones have none
/// ([`NOT_ON_THIS_PLATFORM`]).
pub fn reveal(path: impl Into<std::path::PathBuf>) {
    let path = path.into();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    std::thread::spawn(move || {
        if let Err(e) = desktop::reveal(&path) {
            log::warn!("couldn't show {} in the file manager: {e}", path.display());
        }
    });
    #[cfg(any(target_os = "android", target_os = "ios"))]
    log::info!("couldn't show {}: {NOT_ON_THIS_PLATFORM}", path.display());
}

/// The folders the backend reads and writes, resolved once at start (doc 07 §11.1).
///
/// On the desktop: Tauri's own folders (`<cache>/com.felix.rustradar`, `<config>/com.felix.rustradar`,
/// downloads or home), so `keys.json` and every cache carry over from the Tauri build. On the phones
/// the Tauri build's folders there (`android::dirs` from the activity, after `android::init`;
/// [`ios::dirs`] inside the app's sandbox), without downloads.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn dirs() -> Option<Dirs> {
    Dirs::desktop()
}

/// The folders the backend reads and writes: the Tauri build's on Android (`None` before `init`).
#[cfg(target_os = "android")]
pub fn dirs() -> Option<Dirs> {
    android::dirs()
}

/// The folders the backend reads and writes: the Tauri build's inside the app's container.
#[cfg(target_os = "ios")]
pub fn dirs() -> Option<Dirs> {
    ios::dirs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_folders_are_tauris() {
        if PHONE {
            return;
        }
        let Some(dirs) = dirs() else {
            eprintln!("skipped: this system names no cache or config folder");
            return;
        };
        assert!(dirs.cache.ends_with("com.felix.rustradar"), "{dirs:?}");
        assert!(dirs.config.ends_with("com.felix.rustradar"), "{dirs:?}");
        assert_eq!(dirs.config_file("keys.json"), dirs.config.join("keys.json"));
    }
}
