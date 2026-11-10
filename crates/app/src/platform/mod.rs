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
/// the same rule is used until E19 takes the folders from the platform: on iOS it resolves inside the
/// app's sandbox; on Android the system names no such folders and this is `None`, so the app runs the
/// map without the backend.
pub fn dirs() -> Option<Dirs> {
    Dirs::desktop()
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
