//! What differs between the platforms (plan 1.2): folders now; opening links, revealing files,
//! the clipboard, the soft keyboard, safe areas and the back gesture as their items land (E19 for
//! `android` and `ios`, U20 for `desktop`). Call sites stay free of `cfg`.

use rustradar_backend::Dirs;

pub mod android;
pub mod ios;

/// Whether this build runs on a phone (`cfg(mobile)` in the Tauri build; doc 07 §11.7).
pub const PHONE: bool = cfg!(any(target_os = "android", target_os = "ios"));

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
