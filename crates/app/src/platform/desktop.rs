//! The desktop's platform glue (plan 1.2, 1.3; doc 07 §11.2): opening a link in the browser,
//! showing a file in the file manager, and the clipboard, on Windows, macOS and Linux (and the
//! BSDs, which the Tauri build's opener served as it serves Linux).
//!
//! Call sites use the platform-free [`super::open_url`] and [`super::reveal`]; this module is
//! what they do on the desktop.
//!
//! - **Links** ([`open_url`]): the system's browser through the `webbrowser` crate (plan 1.3),
//!   which egui-winit already opens every link of the UI with (`ctx.open_url`, its `links`
//!   feature), so a link opens the same way whoever asks. GUI browsers are started in the
//!   background: the call does not wait for the browser.
//! - **Showing a file** ([`reveal`], `revealItemInDir(path)`): the path made absolute and
//!   checked to exist first (`std::fs::canonicalize`, as the opener does; on Windows without the
//!   `\\?\` prefix canonicalize adds, [`windows_simplified`]), then:
//!   - Windows: `explorer /select,"<path>"` ([`windows_command`]): Explorer opens the folder with
//!     the file selected;
//!   - macOS: `open -R <path>` ([`macos_command`]): Finder does the same;
//!   - Linux and the BSDs: the file manager's D-Bus method `org.freedesktop.FileManager1.ShowItems`
//!     with the file's URI, through `dbus-send` ([`dbus_command`], waiting for the reply so a
//!     missing service counts as a failure), else the folder opened with `xdg-open`
//!     ([`xdg_open_command`]).
//!
//!   The call blocks until the file manager answers (D-Bus may first have to start it): call it
//!   off the UI thread, as [`super::reveal`] does.
//! - **The clipboard**: egui-winit's (`arboard` on Windows, macOS and X11, `smithay-clipboard` on
//!   Wayland: its `clipboard` feature, set for the desktop in the crate's manifest), which takes
//!   egui's `OutputCommand::CopyText` when the window hands egui's output over
//!   (`handle_platform_output`). Copying is `ctx.copy_text(text)` everywhere (the map error's
//!   Copy details, the dev panel's Copy); there is nothing to call here.
//!
//! Deviations from the Tauri build's opener (`tauri-plugin-opener` 2.7):
//! - Links go through `webbrowser`, not the `open` crate (`open::that_detached`): both hand an
//!   http(s) URL to the system's handler (ShellExecute, Launch Services, `xdg-open`).
//! - Windows asks Explorer by its command line rather than through `SHOpenFolderAndSelectItems`
//!   (no Windows API binding in the app); Explorer shows the same window.
//! - macOS asks Finder through `open -R` rather than `NSWorkspace`'s
//!   `activateFileViewerSelectingURLs`, which `open -R` calls.
//! - Linux falls back to `xdg-open` on the folder. Today's fallback calls the desktop portal's
//!   `OpenDirectory` on the wrong interface (`org.freedesktop.portal.Desktop` of the service
//!   `org.freedesktop.portal.OpenURI`, the two swapped) with a URI where the method takes a file
//!   descriptor, so it never succeeds and the file is not shown at all where no file manager
//!   offers `FileManager1`.
//! - D-Bus is reached through the `dbus-send` tool (part of every desktop's D-Bus installation)
//!   rather than a D-Bus library (`zbus`).
//!
//! **Checked**: the commands' arguments by unit tests on Linux; the module compiles for
//! `x86_64-pc-windows-msvc`, `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`. Opening a
//! browser or a file manager has not been tried on any desktop (**person check**: Windows,
//! macOS, a GNOME and a KDE session).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// D-Bus's name for the file manager's service, its object and its method that shows files.
pub const FILE_MANAGER: &str = "org.freedesktop.FileManager1";
pub const FILE_MANAGER_PATH: &str = "/org/freedesktop/FileManager1";
pub const SHOW_ITEMS: &str = "org.freedesktop.FileManager1.ShowItems";

/// Opens `url` in the system's browser (`openUrl(url)`; doc 07 §11.2).
pub fn open_url(url: &str) -> Result<(), String> {
    webbrowser::open(url).map_err(|e| format!("Couldn't open {url}: {e}"))
}

/// Shows `path` in the system's file manager, selected where it can be (`revealItemInDir(path)`;
/// see the module documentation). Fails when the path does not exist or no file manager could be
/// asked. Blocks until the file manager answers.
pub fn reveal(path: &Path) -> Result<(), String> {
    let path = absolute(path)?;
    reveal_absolute(&path)
}

/// `path` made absolute with every link resolved, as the opener's `canonicalize`; an error when it
/// does not exist.
pub fn absolute(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if cfg!(windows) {
        return Ok(PathBuf::from(windows_simplified(&path.to_string_lossy())));
    }
    Ok(path)
}

#[cfg(windows)]
fn reveal_absolute(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let (program, arg) = windows_command(&path.to_string_lossy());
    // Explorer reads its own command line (`/select,"…"` as one token) and exits with 1 even
    // when it opened the window: started, not waited for
    Command::new(program).raw_arg(arg).spawn().map(drop).map_err(|e| format!("Couldn't start {program}: {e}"))
}

#[cfg(target_os = "macos")]
fn reveal_absolute(path: &Path) -> Result<(), String> {
    let (program, args) = macos_command(path);
    let status = Command::new(program).args(&args).stdin(std::process::Stdio::null()).status().map_err(|e| format!("Couldn't start {program}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{program} -R failed ({status})")) }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_absolute(path: &Path) -> Result<(), String> {
    let (program, args) = dbus_command(path);
    use std::process::Stdio;

    let shown = Command::new(program).args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    match shown {
        Ok(status) if status.success() => return Ok(()),
        Ok(status) => log::info!("no file manager answered {SHOW_ITEMS} ({status}): opening the folder instead"),
        Err(e) => log::info!("couldn't start {program} ({e}): opening the folder instead"),
    }
    let (program, args) = xdg_open_command(path);
    let mut child = Command::new(program).args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("Couldn't start {program}: {e}"))?;
    // xdg-open may run the file manager in the foreground: reaped where it ends
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// A Windows path without the `\\?\` prefix `canonicalize` gives it (`dunce::simplified`, which the
/// opener uses): `\\?\C:\x` is `C:\x`, `\\?\UNC\server\share\x` is `\\server\share\x`; any other
/// path as it is. Explorer does not take the prefixed form.
pub fn windows_simplified(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        let drive = rest.as_bytes();
        if drive.len() >= 2 && drive[0].is_ascii_alphabetic() && drive[1] == b':' {
            return rest.to_owned();
        }
    }
    path.to_owned()
}

/// What shows `path` in Explorer: the program and its whole command line after the name,
/// `/select,"<path>"`, passed as it is (no quoting of Rust's own).
pub fn windows_command(path: &str) -> (&'static str, String) {
    ("explorer", format!("/select,\"{path}\""))
}

/// What shows `path` in Finder: `open -R <path>`.
pub fn macos_command(path: &Path) -> (&'static str, Vec<OsString>) {
    ("open", vec![OsString::from("-R"), path.as_os_str().to_owned()])
}

/// What asks the session's file manager to show `path` over D-Bus (`ShowItems([uri], "")`),
/// waiting for its reply: `dbus-send` with the method's two arguments.
pub fn dbus_command(path: &Path) -> (&'static str, Vec<OsString>) {
    let args = [
        "--session".to_owned(),
        "--print-reply".to_owned(),
        format!("--dest={FILE_MANAGER}"),
        "--type=method_call".to_owned(),
        FILE_MANAGER_PATH.to_owned(),
        SHOW_ITEMS.to_owned(),
        format!("array:string:{}", file_uri(path)),
        // the startup id: none
        "string:".to_owned(),
    ];
    ("dbus-send", args.into_iter().map(OsString::from).collect())
}

/// What opens the folder holding `path`: `xdg-open <folder>`.
pub fn xdg_open_command(path: &Path) -> (&'static str, Vec<OsString>) {
    let folder = path.parent().unwrap_or(path);
    ("xdg-open", vec![folder.as_os_str().to_owned()])
}

/// The `file://` URI of the absolute path `path`: every byte but the unreserved characters and `/`
/// percent-encoded (`%2C` for a comma, which `dbus-send` would take for the next element of the
/// array; a file manager decodes any of them back to the same name).
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in os_bytes(path.as_os_str()).iter() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// The bytes of a file name as the system has them (Unix), or its UTF-8 (elsewhere).
fn os_bytes(s: &OsStr) -> std::borrow::Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::borrow::Cow::Borrowed(s.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::borrow::Cow::Owned(s.to_string_lossy().into_owned().into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn explorer_selects_the_file_in_its_folder() {
        assert_eq!(windows_command(r"C:\Users\Ann Lee\Downloads\rustradar-keys.json"), ("explorer", r#"/select,"C:\Users\Ann Lee\Downloads\rustradar-keys.json""#.to_owned()));
    }

    #[test]
    fn canonicalizes_prefix_is_dropped_for_explorer() {
        assert_eq!(windows_simplified(r"\\?\C:\Users\me\Downloads\a.csv"), r"C:\Users\me\Downloads\a.csv");
        assert_eq!(windows_simplified(r"\\?\UNC\nas\home\a.csv"), r"\\nas\home\a.csv");
        assert_eq!(windows_simplified(r"C:\plain"), r"C:\plain");
        // a verbatim path that is no drive's stays verbatim (Explorer could not take it either way)
        assert_eq!(windows_simplified(r"\\?\Volume{0b1c}\a"), r"\\?\Volume{0b1c}\a");
    }

    #[test]
    fn finder_reveals_with_open_r() {
        let (program, args) = macos_command(Path::new("/Users/me/Downloads/flight KLM76F.kml"));
        assert_eq!(program, "open");
        assert_eq!(strings(&args), ["-R", "/Users/me/Downloads/flight KLM76F.kml"]);
    }

    #[test]
    fn the_file_manager_is_asked_over_dbus_then_the_folder_opened() {
        let path = Path::new("/home/me/Downloads/KLM76F, Amsterdam.csv");
        let (program, args) = dbus_command(path);
        assert_eq!(program, "dbus-send");
        assert_eq!(
            strings(&args),
            [
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                // the comma would split dbus-send's array
                "array:string:file:///home/me/Downloads/KLM76F%2C%20Amsterdam.csv",
                "string:",
            ]
        );
        let (program, args) = xdg_open_command(path);
        assert_eq!(program, "xdg-open");
        assert_eq!(strings(&args), ["/home/me/Downloads"]);
    }

    #[test]
    fn file_uris_encode_all_but_the_unreserved() {
        assert_eq!(file_uri(Path::new("/tmp/a-b_c.d~e/f")), "file:///tmp/a-b_c.d~e/f");
        assert_eq!(file_uri(Path::new("/tmp/Ström #1?.json")), "file:///tmp/Str%C3%B6m%20%231%3F.json");
    }

    #[test]
    fn a_missing_file_is_an_error_before_anything_starts() {
        let missing = std::env::temp_dir().join("rustradar-u20-no-such-file.json");
        let _ = std::fs::remove_file(&missing);
        let err = reveal(&missing).expect_err("nothing to show");
        assert!(err.contains("rustradar-u20-no-such-file.json"), "{err}");
    }

    #[test]
    fn absolute_resolves_relative_paths_and_links() {
        let dir = std::env::temp_dir().join(format!("rustradar-u20-abs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("export.json");
        std::fs::write(&file, "{}").unwrap();
        let through = dir.join(".").join("export.json");
        assert_eq!(absolute(&through).unwrap(), std::fs::canonicalize(&file).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
