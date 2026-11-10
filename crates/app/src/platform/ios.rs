//! iOS (plan 1.2; doc 07 §11.1, §13.5): the folders the Tauri build used, and the log.
//!
//! The rest of what a phone needs comes from winit and egui-winit on iOS: the safe area
//! (egui-winit fills `RawInput::safe_area_insets` from the key window's `safeAreaInsets`), the
//! soft keyboard (shown and hidden through `Window::set_ime_allowed`) and typed text (winit's key
//! and IME events). The app is started by `platform/ios/Sources/main.m`, which calls
//! `rustradar_ios_main`.
//!
//! The frame loop's part (on iOS only): `init()` first thing in `rustradar_ios_main`, then
//! [`dirs`] for the backend.

use std::path::Path;

use rustradar_backend::{APP_ID, Dirs};

/// The folders inside an app container whose home folder is `home`, as the Tauri build named them
/// on iOS (its desktop path code with the `dirs` crate: `$HOME/Library/Caches` and
/// `$HOME/Library/Application Support`, each joined with the app id). No downloads: exports are
/// desktop only in the first native release (plan section 7).
pub fn folders(home: &Path) -> Dirs {
    Dirs {
        cache: home.join("Library/Caches").join(APP_ID),
        config: home.join("Library/Application Support").join(APP_ID),
        downloads: None,
    }
}

/// The app's folders. iOS sets `HOME` to the app's sandbox container, so this is
/// [`Dirs::desktop`] (exactly what Tauri resolved) without downloads; [`folders`] of `HOME` is its
/// spelled-out twin.
pub fn dirs() -> Option<Dirs> {
    Some(Dirs { downloads: None, ..Dirs::desktop()? })
}

/// The log to stderr, which Xcode's console and `log stream` show (the desktop binary does the
/// same in `main.rs`; `RUST_LOG` is read from the scheme's environment).
pub fn init() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,rustradar_app=info")).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folders_inside_the_container() {
        let home = Path::new("/var/mobile/Containers/Data/Application/0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0");
        let dirs = folders(home);
        assert_eq!(dirs.cache, home.join("Library/Caches/com.felix.rustradar"));
        assert_eq!(dirs.config_file("keys.json"), home.join("Library/Application Support/com.felix.rustradar/keys.json"));
        assert_eq!(dirs.downloads, None, "no exports on phones");
    }

    #[cfg(target_os = "ios")]
    #[test]
    fn dirs_are_the_containers() {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("iOS sets HOME"));
        assert_eq!(dirs(), Some(folders(&home)));
    }
}
