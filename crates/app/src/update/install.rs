//! Installing a downloaded update, relaunching into it, and the updates found while the app is
//! open (item P3; doc 07 §7.1, §12.2; `RS:updater.rs`'s `watch`, `pending_update`,
//! `restart_to_update` and `relaunch`, and `tauri-plugin-updater` 2.13.2's `install`, the version in
//! `Cargo.lock` that today's app runs). The launch's own check and its splash are [`super::splash`].
//!
//! **What this build can update** ([`Target::current`]):
//! - Windows: the downloaded installer is run ([`Target::Windows`]): an NSIS `-setup.exe` (bytes
//!   starting `MZ`) with `/S /UPDATE /R /ARGS <this run's arguments>`, an MSI (an OLE compound
//!   file) through `%SYSTEMROOT%\System32\msiexec.exe /i "<file>" /quiet /promptrestart
//!   AUTOLAUNCHAPP=True LAUNCHAPPARGS="<arguments>"` (today's `installMode: "quiet"`, the plugin's
//!   `restart_after_install`), both written to `%TEMP%\RustRadar-<version>-updater-XXXXXX\
//!   RustRadar-<version>-installer.<ext>` and started with `ShellExecuteW("open")`, the arguments
//!   escaped as the plugin escapes them. The app then closes; the installer restarts it.
//! - macOS: the `.app` the binary runs in (`…/RustRadar.app/Contents/MacOS/…`) is replaced by the
//!   `.app.tar.gz`'s ([`Target::App`]): unpacked, its top folder dropped, into a temp folder on the
//!   app's volume (the system's temp folder, else beside the app), made 0755, swapped in one step
//!   (`renamex_np(RENAME_SWAP)`), else moved with two renames that restore the old app if the
//!   second fails; a permission error runs the same swap or moves as administrator
//!   (`osascript … with administrator privileges`); an old app that could not be put back is kept
//!   beside the app or in the home folder as `RustRadar (previous version).app`; then `touch`.
//! - Linux: the AppImage at `$APPIMAGE` is rewritten ([`Target::AppImage`]): moved aside into a
//!   0700 temp folder on its file system (the temp folder, the cache folder, beside it), the new
//!   bytes written (or the `.AppImage` of a `.tar.gz`) with the old file's permissions, the old
//!   one put back on failure. A `.deb` or `.rpm` install ([`Target::Deb`], [`Target::Rpm`], from
//!   the bundle-type marker) writes the package to a temp folder and installs it with `pkexec
//!   dpkg -i` (`rpm -U`), else `sudo -S` with the password from `zenity` or `kdialog`, else `sudo`.
//!
//! **Relaunch** ([`relaunch`]): the binary the process started from, cached at start
//! ([`remember_binary`]; `current_exe` reads a deleted file once a package has replaced it), or
//! `$APPIMAGE`, started with this run's arguments less `--updated`, plus `--updated` (the launch
//! then skips its check, so a failed install cannot loop); the caller closes the app.
//!
//! **While the app is open** ([`start_watch`], today's `watch`): every [`WATCH_EVERY`] (an hour,
//! slept first), a check with the [`CHECK_TIMEOUT`]; a version other than the one already
//! downloaded is downloaded into memory ([`Ready`]) and announced as `BackendEvent::UpdateReady`,
//! which the state turns into `state.update_ready` for the toast. A failed check or download waits
//! for the next hour. [`pending_update`] is the version waiting. The toast's Restart ([`connect`],
//! [`Restart`]) runs [`restart_to_update`] off the UI thread: the waiting download taken, installed
//! (`Couldn't install the update: {e}`), the app relaunched and closed through [`on_exit`]'s hook
//! (the window's event loop, so the settings not yet written are flushed); before a download it
//! answers `No update is waiting`.
//!
//! **The version compared** ([`release_version`]): `src-tauri/tauri.conf.json`'s, the release
//! version CI writes into the version files before it builds (`app.package_info().version` in
//! today's app), not this crate's `0.1.0`, which would find every release newer.
//!
//! Deviations:
//! - Only builds an installer made update themselves: an unpatched bundle-type marker on Windows
//!   and Linux (a developer's `cargo build --release`), a Linux binary outside an AppImage and not
//!   from a package, or a macOS binary outside an `.app` look for no update ([`Target::detect`]).
//!   The plugin would replace the binary (or the folder holding it) with the download.
//! - Windows `.zip` updater archives (Tauri 1's format; today's releases publish the installers
//!   themselves) are refused as `invalid updater binary format`.
//! - macOS: the administrator step runs `osascript` as a child process rather than through
//!   `osakit` on the main thread; installs run off the UI thread (today's `restart_to_update` was
//!   a synchronous command on the main thread).
//! - The AppImage's backup is put back also when the `.tar.gz` cannot be read at all (the plugin
//!   returns early and loses it with its temp folder).
//! - Messages go to the log (`update download failed: {e}` at `warn`), not `eprintln!`.
//!
//! Trap for D2: [`release_version`] reads `src-tauri/tauri.conf.json` with `include_str!`; when
//! stage 12 deletes it and the package carries the release version, it becomes
//! [`super::current_version`].

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError, mpsc};
use std::time::Duration;

use rustradar_backend::{BackendEvent, Notifier};
use semver::Version;
use tokio::runtime::Handle;

use super::manifest::Installer;
use super::{CHECK_TIMEOUT, UPDATED_FLAG, Update, Updater, WATCH_EVERY};
use crate::state::aircraft::Answer;
use crate::ui::toast;

/// The product name (`tauri.conf.json` → `productName`, the plugin's `app_name`): the Windows
/// installer's temp names.
pub const APP_NAME: &str = "RustRadar";

/// Whether this build looks for updates: release builds only (`tauri dev` ran unsigned builds with
/// nothing to update to).
pub const CHECKS: bool = !cfg!(debug_assertions);

/// What [`restart_to_update`] answers before a version was downloaded.
pub const NOTHING_WAITING: &str = "No update is waiting";

// ------------------------------------------------------------------------------------------------
// the version and the binary

const TAURI_CONF: &str = include_str!("../../../../src-tauri/tauri.conf.json");

/// The version installed copies compare with the manifest's: `src-tauri/tauri.conf.json`'s
/// `version` (written by `cargo xtask bump` and by CI before a release build), else this crate's.
pub fn release_version() -> Version {
    static VERSION: LazyLock<Version> = LazyLock::new(|| conf_version(TAURI_CONF).unwrap_or_else(super::current_version));
    VERSION.clone()
}

fn conf_version(conf: &str) -> Option<Version> {
    let json: serde_json::Value = serde_json::from_str(conf).ok()?;
    Version::parse(json.get("version")?.as_str()?).ok()
}

/// The binary this process started from, canonicalised, as `tauri-utils`' `STARTING_BINARY`.
static STARTING_BINARY: LazyLock<Result<PathBuf, String>> = LazyLock::new(starting_binary);

fn starting_binary() -> Result<PathBuf, String> {
    let path = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    if let Some(link) = path.ancestors().find(|a| a.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink())) {
        return Err(format!(
            "StartingBinary found current_exe() that contains a symlink on a non-allowed platform: {}",
            link.display()
        ));
    }
    path.canonicalize().map_err(|e| e.to_string())
}

/// Reads the starting binary now, before an install can replace it (call at start).
pub fn remember_binary() {
    LazyLock::force(&STARTING_BINARY);
}

/// The binary to relaunch (`tauri::process::current_binary`): `$APPIMAGE` on Linux when set, else
/// the starting binary.
pub fn current_binary() -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(app_image) = std::env::var_os("APPIMAGE") {
        return Ok(PathBuf::from(app_image));
    }
    STARTING_BINARY.clone().map_err(io::Error::other)
}

/// The relaunched app's arguments: `args` (with the program first) less the program and
/// `--updated`, then `--updated`.
pub fn relaunch_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut out: Vec<OsString> = args.into_iter().skip(1).filter(|a| a != UPDATED_FLAG).collect();
    out.push(UPDATED_FLAG.into());
    out
}

/// Starts the freshly installed app (`relaunch` without its `app.exit(0)`, which is the caller's).
pub fn relaunch() -> io::Result<()> {
    std::process::Command::new(current_binary()?).args(relaunch_args(std::env::args_os())).spawn()?;
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// errors

/// Why an install failed; the texts are the plugin's.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The app's install path could not be told from the binary's.
    FailedToDetermineExtractPath,
    /// The download is not a package this system installs.
    InvalidUpdaterFormat,
    /// The archive holds no AppImage.
    BinaryNotFoundInArchive,
    /// No temp folder on the app's file system, where the renames must happen.
    TempDirNotOnSameMountPoint,
    /// No temp folder took the package.
    TempDirNotFound,
    /// The install failed and the old app could not be put back; it was kept here.
    PreviousAppNotRestored(PathBuf),
    /// The password prompt failed or was dismissed.
    AuthenticationFailed,
    /// The package manager failed.
    PackageInstallFailed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::FailedToDetermineExtractPath => f.write_str("Failed to determine updater package extract path."),
            Error::InvalidUpdaterFormat => f.write_str("invalid updater binary format"),
            Error::BinaryNotFoundInArchive => f.write_str("binary for the current target not found in the archive"),
            Error::TempDirNotOnSameMountPoint => f.write_str("temp directory is not on the same mount point as the app"),
            Error::TempDirNotFound => f.write_str("failed to create temporary directory"),
            Error::PreviousAppNotRestored(path) => {
                write!(f, "failed to install the update and to restore the previous app, which was left at {}", path.display())
            }
            Error::AuthenticationFailed => f.write_str("Authentication failed or was cancelled"),
            Error::PackageInstallFailed => f.write_str("Failed to install package"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

// ------------------------------------------------------------------------------------------------
// what this build updates

/// What an install replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Windows: the downloaded installer is run (NSIS or MSI, told apart by its bytes).
    Windows,
    /// macOS: this `.app` bundle.
    App(PathBuf),
    /// Linux: this AppImage (`$APPIMAGE`).
    AppImage(PathBuf),
    /// Linux, installed from the `.deb`: the binary's path.
    Deb(PathBuf),
    /// Linux, installed from the `.rpm`: the binary's path.
    Rpm(PathBuf),
}

/// What [`Target::install`] leaves to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handover {
    /// The new version is in place: relaunch, then close.
    Relaunch,
    /// The installer took over and starts the new version itself (Windows): close.
    Installer,
}

impl Target {
    /// What this build updates, or why it updates nothing.
    pub fn current() -> Result<Target, String> {
        let os = super::manifest::os().unwrap_or("");
        let exe = STARTING_BINARY.clone()?;
        Target::detect(os, Installer::current(), &exe, std::env::var_os("APPIMAGE").as_deref())
    }

    /// [`Target::current`] for `os` (the manifest's name), the installer the marker names, the
    /// binary's path and `$APPIMAGE`.
    pub fn detect(os: &str, installer: Option<Installer>, exe: &Path, appimage: Option<&std::ffi::OsStr>) -> Result<Target, String> {
        match os {
            "windows" => match installer {
                Some(Installer::Nsis | Installer::Msi) => Ok(Target::Windows),
                _ => Err("this build was not made by an installer (its bundle-type marker is not patched)".into()),
            },
            "darwin" => {
                let macos = exe.parent().ok_or_else(|| Error::FailedToDetermineExtractPath.to_string())?;
                // the plugin's test, `…/<app>.app/Contents/MacOS/<binary>` → `…/<app>.app`
                if !macos.display().to_string().contains("Contents/MacOS") {
                    return Err(format!("{} is not inside an .app bundle", exe.display()));
                }
                macos
                    .parent()
                    .and_then(Path::parent)
                    .map(|bundle| Target::App(bundle.to_path_buf()))
                    .ok_or_else(|| Error::FailedToDetermineExtractPath.to_string())
            }
            "linux" => match (installer, appimage) {
                (Some(Installer::Deb), _) => Ok(Target::Deb(exe.to_path_buf())),
                (Some(Installer::Rpm), _) => Ok(Target::Rpm(exe.to_path_buf())),
                (_, Some(path)) => Ok(Target::AppImage(PathBuf::from(path))),
                (_, None) => Err("not run from an AppImage ($APPIMAGE is unset) and not installed from a package".into()),
            },
            _ => Err("this system has no self-update".into()),
        }
    }

    /// Installs `bytes`, the verified download of `version`.
    pub fn install(&self, version: &str, bytes: &[u8]) -> Result<Handover, Error> {
        let _ = (version, bytes);
        match self {
            #[cfg(windows)]
            Target::Windows => {
                windows::install(version, bytes, &std::env::args_os().collect::<Vec<_>>())?;
                Ok(Handover::Installer)
            }
            #[cfg(target_os = "macos")]
            Target::App(bundle) => {
                bundle::install(bundle, bytes, &bundle::Ops::system(bundle))?;
                Ok(Handover::Relaunch)
            }
            #[cfg(target_os = "linux")]
            Target::AppImage(path) => {
                linux::install_appimage(path, bytes, &linux::tmp_locations(path))?;
                Ok(Handover::Relaunch)
            }
            #[cfg(target_os = "linux")]
            Target::Deb(exe) => {
                linux::install_package(bytes, &linux::tmp_locations(exe), linux::Package::Deb, linux::install_with_privileges)?;
                Ok(Handover::Relaunch)
            }
            #[cfg(target_os = "linux")]
            Target::Rpm(exe) => {
                linux::install_package(bytes, &linux::tmp_locations(exe), linux::Package::Rpm, linux::install_with_privileges)?;
                Ok(Handover::Relaunch)
            }
            #[allow(unreachable_patterns)]
            _ => Err(Error::InvalidUpdaterFormat),
        }
    }
}

// ------------------------------------------------------------------------------------------------
// temp folders

/// A folder removed when dropped unless kept (`tempfile::TempDir`): the name is the prefix and six
/// random characters; 0700 on Unix.
#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
struct TempDir {
    path: PathBuf,
    keep: bool,
}

#[cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
impl TempDir {
    fn new_in(dir: &Path, prefix: &str) -> io::Result<TempDir> {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let random = RandomState::new();
        let mut last = None;
        for _ in 0..64 {
            let mut x = random.hash_one((std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed), std::time::SystemTime::now()));
            let suffix: String = (0..6)
                .map(|_| {
                    let c = CHARS[(x % CHARS.len() as u64) as usize];
                    x /= CHARS.len() as u64;
                    c as char
                })
                .collect();
            let path = dir.join(format!("{prefix}{suffix}"));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&path) {
                Ok(()) => return Ok(TempDir { path, keep: false }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("no free temp folder name")))
    }

    #[cfg_attr(not(any(target_os = "macos", target_os = "linux", test)), allow(dead_code))]
    fn path(&self) -> &Path {
        &self.path
    }

    /// Kept: not removed when dropped.
    #[cfg_attr(not(any(windows, target_os = "macos", test)), allow(dead_code))]
    fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// `infer`'s checks of a download's first bytes.
#[cfg_attr(not(test), allow(dead_code))]
mod magic {
    pub fn is_exe(buf: &[u8]) -> bool {
        buf.len() > 1 && buf[0] == 0x4D && buf[1] == 0x5A
    }

    pub fn is_msi(buf: &[u8]) -> bool {
        buf.len() > 7 && buf[..8] == [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]
    }

    pub fn is_gz(buf: &[u8]) -> bool {
        buf.len() > 2 && buf[0] == 0x1F && buf[1] == 0x8B && buf[2] == 0x8
    }

    pub fn is_deb(buf: &[u8]) -> bool {
        buf.len() > 20 && &buf[..21] == b"!<arch>\ndebian-binary"
    }

    pub fn is_rpm(buf: &[u8]) -> bool {
        buf.len() > 96 && buf[..4] == [0xED, 0xAB, 0xEE, 0xDB]
    }
}

// ------------------------------------------------------------------------------------------------
// Windows

/// The installer's command line, as the plugin builds it (pure, so it is tested everywhere).
#[cfg(any(windows, test))]
mod windows_args {
    /// The two installers a Windows release ships.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Kind {
        Nsis,
        Msi,
    }

    /// Which installer `bytes` are.
    pub fn kind(bytes: &[u8]) -> Option<Kind> {
        if super::magic::is_exe(bytes) {
            Some(Kind::Nsis)
        } else if super::magic::is_msi(bytes) {
            Some(Kind::Msi)
        } else {
            None
        }
    }

    /// `installMode: "quiet"`: NSIS's `/S` and `/R` (restart), msiexec's `/quiet` and
    /// `AUTOLAUNCHAPP=True`.
    const NSIS_ARGS: &[&str] = &["/S"];
    const NSIS_RESTART: &[&str] = &["/R"];
    const MSIEXEC_ARGS: &[&str] = &["/quiet"];
    const MSI_RESTART: &[&str] = &["AUTOLAUNCHAPP=True"];

    /// The installer's parameters (`updater_parameters`, with `restart_after_install`): `path` is
    /// the installer's file, `current` this run's arguments after the program.
    pub fn parameters(kind: Kind, path: &str, current: &[String]) -> String {
        let mut args: Vec<String> = Vec::new();
        match kind {
            Kind::Nsis => {
                args.extend(NSIS_ARGS.iter().map(|a| (*a).to_owned()));
                args.push("/UPDATE".into());
                args.extend(NSIS_RESTART.iter().map(|a| (*a).to_owned()));
                args.push("/ARGS".into());
                args.extend(current.iter().map(|a| escape_nsis_current_exe_arg(a)));
            }
            Kind::Msi => {
                args.push("/i".into());
                args.push(format!("\"{path}\""));
                args.extend(MSIEXEC_ARGS.iter().map(|a| (*a).to_owned()));
                args.push("/promptrestart".into());
                args.extend(MSI_RESTART.iter().map(|a| (*a).to_owned()));
                let launch: Vec<String> = current.iter().map(|a| escape_msi_property_arg(a)).collect();
                args.push(format!("LAUNCHAPPARGS=\"{}\"", launch.join(" ")));
            }
        }
        args.join(" ")
    }

    /// One argument for NSIS's `/ARGS` (std's Windows quoting, `/` quoted too so NSIS does not
    /// read it as one of its own).
    pub fn escape_nsis_current_exe_arg(arg: &str) -> String {
        let quote = arg.bytes().any(|c| c == b' ' || c == b'\t' || c == b'/') || arg.is_empty();
        let mut cmd = String::new();
        if quote {
            cmd.push('"');
        }
        let mut backslashes: usize = 0;
        for x in arg.chars() {
            if x == '\\' {
                backslashes += 1;
            } else {
                if x == '"' {
                    // n + 1 backslashes, 2n + 1 in all, before an inner quote
                    cmd.extend(std::iter::repeat_n('\\', backslashes + 1));
                }
                backslashes = 0;
            }
            cmd.push(x);
        }
        if quote {
            // n more before the closing quote, 2n in all
            cmd.extend(std::iter::repeat_n('\\', backslashes));
            cmd.push('"');
        }
        cmd
    }

    /// One argument inside `LAUNCHAPPARGS="…"`.
    pub fn escape_msi_property_arg(arg: &str) -> String {
        let mut arg = arg.to_owned();
        // otherwise it is lost in ShellExecute
        if arg.is_empty() {
            return "\"\"\"\"".to_owned();
        } else if !arg.contains(' ') && !arg.contains('"') {
            return arg;
        }
        if arg.contains('"') {
            arg = arg.replace('"', r#""""""#);
        }
        if arg.starts_with('-') {
            if let Some((a1, a2)) = arg.split_once('=') { format!("{a1}=\"\"{a2}\"\"") } else { format!("\"\"{arg}\"\"") }
        } else {
            format!("\"\"{arg}\"\"")
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::{OsStr, OsString, c_void};
    use std::fs;
    use std::io;
    use std::os::windows::ffi::OsStrExt;

    use super::windows_args::{self, Kind};
    use super::{APP_NAME, Error, TempDir};

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut c_void;
    }

    const SW_SHOW: i32 = 5;

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Writes the installer to a temp folder of its own (kept) and starts it.
    pub fn install(version: &str, bytes: &[u8], args: &[OsString]) -> Result<(), Error> {
        let kind = windows_args::kind(bytes).ok_or(Error::InvalidUpdaterFormat)?;
        let dir = TempDir::new_in(&std::env::temp_dir(), &format!("{APP_NAME}-{version}-updater-"))?.keep();
        let ext = match kind {
            Kind::Nsis => ".exe",
            Kind::Msi => ".msi",
        };
        let path = dir.join(format!("{APP_NAME}-{version}-installer{ext}"));
        fs::write(&path, bytes)?;
        let current: Vec<String> = args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
        let parameters = windows_args::parameters(kind, &path.to_string_lossy(), &current);
        let file: OsString = match kind {
            Kind::Nsis => path.into_os_string(),
            Kind::Msi => std::env::var("SYSTEMROOT")
                .map_or_else(|_| OsString::from("msiexec.exe"), |p| OsString::from(format!("{p}\\System32\\msiexec.exe"))),
        };
        log::debug!("Executing updater {file:?} with parameters: {parameters:?}");
        let (open, file, parameters) = (wide(OsStr::new("open")), wide(&file), wide(OsStr::new(&parameters)));
        // SAFETY: the three strings are NUL-terminated UTF-16 that outlive the call; the window
        // and the folder may be null
        let result = unsafe {
            ShellExecuteW(std::ptr::null_mut(), open.as_ptr(), file.as_ptr(), parameters.as_ptr(), std::ptr::null(), SW_SHOW)
        };
        if result as isize <= 32 {
            return Err(Error::Io(io::Error::last_os_error()));
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------
// macOS (the swap and its rollback are plain file moves, tested on any Unix)

#[cfg(any(target_os = "macos", all(test, unix)))]
mod bundle {
    use std::fs;
    use std::io;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    use super::{Error, TempDir};

    /// What the install needs from the system, replaceable in tests.
    pub struct Ops<'a> {
        /// Where the temp folders may go, in order; the first on the app's volume is taken.
        pub tmp_roots: Vec<PathBuf>,
        /// Exchanges the two paths in one step.
        pub swap: &'a dyn Fn(&Path, &Path) -> io::Result<()>,
        /// The swap or moves as administrator: (app, staged, backup).
        pub privileged: &'a dyn Fn(&Path, &Path, &Path) -> Result<(), String>,
        /// The last place for an old app that could not be put back.
        pub home: Option<PathBuf>,
    }

    #[cfg(target_os = "macos")]
    impl Ops<'static> {
        /// The system's: the temp folder or beside the app, `renamex_np`, `osascript`, `$HOME`.
        pub fn system(bundle: &Path) -> Ops<'static> {
            let tmp_roots = [Some(std::env::temp_dir()), bundle.parent().map(Path::to_path_buf)].into_iter().flatten().collect();
            Ops { tmp_roots, swap: &swap_bundle, privileged: &privileged_install, home: std::env::var_os("HOME").map(PathBuf::from) }
        }
    }

    /// Replaces the bundle at `target` with the `.app.tar.gz` in `bytes`.
    pub fn install(target: &Path, bytes: &[u8], ops: &Ops<'_>) -> Result<(), Error> {
        // renames work within one file system only, and act on the path itself (no symlink
        // followed)
        let app_dev = fs::symlink_metadata(target)?.dev();
        let tmp_root = ops
            .tmp_roots
            .iter()
            .find(|dir| dir.metadata().is_ok_and(|m| m.dev() == app_dev))
            .ok_or(Error::TempDirNotOnSameMountPoint)?;
        let tmp_backup_dir = TempDir::new_in(tmp_root, "tauri_current_app")?;
        let tmp_extract_dir = TempDir::new_in(tmp_root, "tauri_updated_app")?;

        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        for entry in archive.entries()? {
            let mut entry = entry?;
            let collected: PathBuf = entry.path()?.iter().skip(1).collect();
            let extraction_path = tmp_extract_dir.path().join(&collected);
            if let Some(parent) = extraction_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Err(err) = entry.unpack(&extraction_path) {
                fs::remove_dir_all(tmp_extract_dir.path()).ok();
                return Err(err.into());
            }
        }
        // the temp folder becomes the bundle: 0700 would keep the machine's other users out
        fs::set_permissions(tmp_extract_dir.path(), fs::Permissions::from_mode(0o755))?;

        // one swap where the file system can (the old app then sits in the extraction folder,
        // removed on drop), else two moves with a restore; a permission error needs the
        // administrator instead
        let backup = tmp_backup_dir.path().join("current_app");
        let moved = match (ops.swap)(target, tmp_extract_dir.path()) {
            Err(err) if err.kind() != io::ErrorKind::PermissionDenied => {
                log::debug!("cannot swap the app bundles ({err}), moving them instead");
                replace_bundle(target, tmp_extract_dir.path(), &backup)
            }
            other => other,
        };
        let need_authorization = match moved {
            Ok(()) => false,
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => true,
            Err(err) => {
                if let Some(kept) = keep_backup_if_not_restored(target, tmp_backup_dir, &backup, ops.home.as_deref()) {
                    log::error!("failed to install the update: {err}");
                    return Err(Error::PreviousAppNotRestored(kept));
                }
                fs::remove_dir_all(tmp_extract_dir.path()).ok();
                return Err(err.into());
            }
        };

        if need_authorization {
            log::debug!("app installation needs admin privileges");
            if let Err(err) = (ops.privileged)(target, tmp_extract_dir.path(), &backup) {
                log::error!("failed to move the new app into place: {err}");
                if let Some(kept) = keep_backup_if_not_restored(target, tmp_backup_dir, &backup, ops.home.as_deref()) {
                    return Err(Error::PreviousAppNotRestored(kept));
                }
                fs::remove_dir_all(tmp_extract_dir.path()).ok();
                return Err(Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, "Failed to move the new app into place")));
            }
        }

        let _ = std::process::Command::new("touch").arg(target).status();
        Ok(())
    }

    /// Exchanges `target` and `staged` atomically (APFS); other file systems refuse.
    #[cfg(target_os = "macos")]
    pub fn swap_bundle(target: &Path, staged: &Path) -> io::Result<()> {
        use std::ffi::{CString, c_char, c_int, c_uint};
        use std::os::unix::ffi::OsStrExt;

        unsafe extern "C" {
            fn renamex_np(from: *const c_char, to: *const c_char, flags: c_uint) -> c_int;
        }
        const RENAME_SWAP: c_uint = 0x0000_0002;

        let from = CString::new(staged.as_os_str().as_bytes())?;
        let to = CString::new(target.as_os_str().as_bytes())?;
        // SAFETY: both are NUL-terminated strings that outlive the call, which only reads them
        let result = unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_SWAP) };
        if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    /// Moves `staged` to `target`, keeping the old `target` in `backup` until the move has worked;
    /// a failed second move puts the old one back.
    pub fn replace_bundle(target: &Path, staged: &Path, backup: &Path) -> io::Result<()> {
        fs::rename(target, backup)?;
        if let Err(err) = fs::rename(staged, target) {
            // the error worth reporting is the move's
            let _ = fs::rename(backup, target);
            return Err(err);
        }
        Ok(())
    }

    /// `osascript`'s JavaScript for Automation calling `renamex_np` with `RENAME_SWAP` (2); prints
    /// `swapped` or `failed`.
    pub const SWAP_BUNDLE_JXA: &str = r#"function run(argv) {
  ObjC.import("stdio");
  return $.renamex_np(argv[0], argv[1], 2) === 0 ? "swapped" : "failed";
}"#;

    /// Where `osascript` is.
    pub const OSASCRIPT: &str = "/usr/bin/osascript";

    /// The shell command of the administrator's install: the swap through `osascript`, and only
    /// once it is known not to have happened, the moves with their restore, the old app removed
    /// once the new one is in place.
    pub fn privileged_install_command(target: &Path, staged: &Path, backup: &Path, osascript: &Path) -> String {
        let osascript = sh_quote(osascript);
        let swap = sh_quote(Path::new(SWAP_BUNDLE_JXA));
        let target = sh_quote(target);
        let staged = sh_quote(staged);
        let backup = sh_quote(backup);
        format!(
            "case \"$({osascript} -l JavaScript -e {swap} {staged} {target} 2>/dev/null)\" in swapped) rm -rf {staged};; failed) mv -f {target} {backup} && {{ mv -f {staged} {target} || {{ mv -f {backup} {target}; exit 1; }}; }} && rm -rf {backup};; *) exit 1;; esac"
        )
    }

    /// `path` as one `sh` word.
    pub fn sh_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
    }

    /// `s` as an AppleScript string literal.
    pub fn applescript_string(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
    }

    /// Runs [`privileged_install_command`] as administrator (the system's password prompt).
    #[cfg(target_os = "macos")]
    pub fn privileged_install(target: &Path, staged: &Path, backup: &Path) -> Result<(), String> {
        let command = privileged_install_command(target, staged, backup, Path::new(OSASCRIPT));
        let script = format!("do shell script {} with administrator privileges", applescript_string(&command));
        let output = std::process::Command::new(OSASCRIPT).arg("-e").arg(script).output().map_err(|e| e.to_string())?;
        if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()) }
    }

    /// After a failed install: where the old app is when it did not make it back to `target`
    /// (`backup` is then the only copy, in a temp folder macOS empties): moved beside `target`, or
    /// into `fallback_dir`, as `<name> (previous version).app`; if neither works `backup_dir` is
    /// kept instead of removed.
    pub fn keep_backup_if_not_restored(
        target: &Path,
        backup_dir: TempDir,
        backup: &Path,
        fallback_dir: Option<&Path>,
    ) -> Option<PathBuf> {
        if fs::symlink_metadata(target).is_ok() || fs::symlink_metadata(backup).is_err() {
            return None;
        }
        let mut name = target.file_stem().unwrap_or_default().to_os_string();
        name.push(" (previous version)");
        if let Some(extension) = target.extension() {
            name.push(".");
            name.push(extension);
        }
        let kept = [target.parent(), fallback_dir]
            .into_iter()
            .flatten()
            .map(|dir| dir.join(&name))
            // `rename` replaces an empty folder: never move onto anything already there
            .filter(|kept| fs::symlink_metadata(kept).is_err())
            .find(|kept| fs::rename(backup, kept).is_ok());
        match kept {
            Some(kept) => Some(kept),
            None => {
                let _ = backup_dir.keep();
                Some(backup.to_path_buf())
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Linux

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::OsStr;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use super::{Error, TempDir, magic};

    /// The cache folder (`dirs::cache_dir` on Linux): `$XDG_CACHE_HOME` when absolute, else
    /// `~/.cache`.
    fn cache_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }

    /// Where temp folders go, in order: the temp folder, the cache folder, beside `beside`.
    pub fn tmp_locations(beside: &Path) -> Vec<Option<PathBuf>> {
        vec![Some(std::env::temp_dir()), cache_dir(), beside.parent().map(Path::to_path_buf)]
    }

    /// Replaces the AppImage at `path` with `bytes` (an AppImage, or a `.tar.gz` holding one).
    pub fn install_appimage(path: &Path, bytes: &[u8], locations: &[Option<PathBuf>]) -> Result<(), Error> {
        let path_dev = path.metadata()?.dev();
        for location in locations.iter().flatten() {
            let tmp_dir = TempDir::new_in(location, "tauri_current_app")?;
            if tmp_dir.path().metadata()?.dev() != path_dev {
                continue;
            }
            fs::set_permissions(tmp_dir.path(), fs::Permissions::from_mode(0o700))?;
            let tmp_app_image = tmp_dir.path().join("current_app.AppImage");
            let permissions = fs::metadata(path)?.permissions();
            // the current AppImage kept aside until the new one is in place
            fs::rename(path, &tmp_app_image)?;

            if magic::is_gz(bytes) {
                log::debug!("extracting AppImage");
                let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
                let entries = match archive.entries() {
                    Ok(entries) => entries,
                    Err(err) => {
                        fs::rename(&tmp_app_image, path)?;
                        return Err(err.into());
                    }
                };
                for mut entry in entries.flatten() {
                    if let Ok(entry_path) = entry.path()
                        && entry_path.extension() == Some(OsStr::new("AppImage"))
                    {
                        // a failed extraction puts the old one back
                        if let Err(err) = entry.unpack(path) {
                            fs::rename(&tmp_app_image, path)?;
                            return Err(err.into());
                        }
                        return Ok(());
                    }
                }
                fs::rename(&tmp_app_image, path)?;
                return Err(Error::BinaryNotFoundInArchive);
            }

            log::debug!("rewriting AppImage");
            return match fs::write(path, bytes).and_then(|()| fs::set_permissions(path, permissions)) {
                Err(err) => {
                    fs::rename(&tmp_app_image, path)?;
                    Err(err.into())
                }
                Ok(()) => Ok(()),
            };
        }
        Err(Error::TempDirNotOnSameMountPoint)
    }

    /// A Linux package and how it is installed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Package {
        Deb,
        Rpm,
    }

    impl Package {
        /// The installer, its argument and the file's extension.
        pub fn command(self) -> (&'static str, &'static str, &'static str) {
            match self {
                Package::Deb => ("dpkg", "-i", "deb"),
                Package::Rpm => ("rpm", "-U", "rpm"),
            }
        }
    }

    /// Writes the package to the first temp folder that takes it and installs it with `install`
    /// (package, command, argument); the folder goes once the install has ended.
    pub fn install_package(
        bytes: &[u8],
        locations: &[Option<PathBuf>],
        package: Package,
        install: impl Fn(&Path, &str, &str) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let valid = match package {
            Package::Deb => magic::is_deb(bytes),
            Package::Rpm => magic::is_rpm(bytes),
        };
        if !valid {
            if package == Package::Deb {
                log::warn!("update is not a valid deb package");
            }
            return Err(Error::InvalidUpdaterFormat);
        }
        let (command, arg, ext) = package.command();
        for location in locations.iter().flatten() {
            if let Ok(tmp_dir) = TempDir::new_in(location, &format!("tauri_{ext}_update")) {
                let pkg_path = tmp_dir.path().join(format!("package.{ext}"));
                if fs::write(&pkg_path, bytes).is_ok() {
                    return install(&pkg_path, command, arg);
                }
            }
        }
        Err(Error::TempDirNotFound)
    }

    /// `pkexec`, else `sudo -S` with the password from `zenity` or `kdialog`, else `sudo`.
    pub fn install_with_privileges(pkg_path: &Path, command: &str, arg: &str) -> Result<(), Error> {
        if let Ok(status) = Command::new("pkexec").arg(command).arg(arg).arg(pkg_path).status()
            && status.success()
        {
            log::debug!("installed {pkg_path:?} with pkexec");
            return Ok(());
        }
        if let Ok(password) = password_graphically()
            && install_with_sudo(pkg_path, &password, command, arg)?
        {
            log::debug!("installed {pkg_path:?} with GUI sudo");
            return Ok(());
        }
        let status = Command::new("sudo").arg(command).arg(arg).arg(pkg_path).status()?;
        if status.success() {
            log::debug!("installed {pkg_path:?} with sudo");
            Ok(())
        } else {
            Err(Error::PackageInstallFailed)
        }
    }

    fn password_graphically() -> Result<String, Error> {
        let zenity = Command::new("zenity")
            .args(["--password", "--title=Authentication Required", "--text=Enter your password to install the update:"])
            .output();
        if let Ok(output) = zenity
            && output.status.success()
        {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
        }
        let kdialog = Command::new("kdialog").args(["--password", "Enter your password to install the update:"]).output();
        if let Ok(output) = kdialog
            && output.status.success()
        {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
        }
        Err(Error::AuthenticationFailed)
    }

    fn install_with_sudo(pkg_path: &Path, password: &str, command: &str, arg: &str) -> Result<bool, Error> {
        let mut child = Command::new("sudo")
            .arg("-S")
            .arg(command)
            .arg(arg)
            .arg(pkg_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{password}")?;
        }
        Ok(child.wait()?.success())
    }
}

// ------------------------------------------------------------------------------------------------
// while the app is open

/// A version downloaded and waiting for a restart.
pub struct Waiting {
    pub update: Update,
    pub bytes: Vec<u8>,
}

/// The version downloaded while the app was open (today's `ready()`), shared by the watch and the
/// toast. Cheap to clone; every clone is the same slot.
#[derive(Clone, Default)]
pub struct Ready(Arc<Mutex<Option<Waiting>>>);

impl fmt::Debug for Ready {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Ready").field(&self.pending_update()).finish()
    }
}

impl Ready {
    /// Keeps `bytes`, the verified download of `update`.
    pub fn set(&self, update: Update, bytes: Vec<u8>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Waiting { update, bytes });
    }

    /// `pending_update`: the version waiting, if any.
    pub fn pending_update(&self) -> Option<String> {
        self.0.lock().ok()?.as_ref().map(|w| w.update.version.clone())
    }

    /// Takes the waiting download out.
    pub fn take(&self) -> Result<Option<Waiting>, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.take())
    }
}

/// The process's [`Ready`].
pub fn ready() -> &'static Ready {
    static READY: LazyLock<Ready> = LazyLock::new(Ready::default);
    &READY
}

/// `pending_update`: the version the process's watch downloaded, if any (for a window that missed
/// the event).
pub fn pending_update() -> Option<String> {
    ready().pending_update()
}

/// Today's `watch`: every `every`, `updater.check()` within [`CHECK_TIMEOUT`]; a version other
/// than the one waiting is downloaded, kept in `ready` and announced through `notifier`. Never
/// ends; abort its task to stop it.
pub async fn watch(ready: Ready, notifier: Notifier, every: Duration, updater: Updater) {
    loop {
        tokio::time::sleep(every).await;
        let Ok(Ok(Some(update))) = tokio::time::timeout(CHECK_TIMEOUT, updater.check()).await else { continue };
        if ready.pending_update().as_deref() == Some(update.version.as_str()) {
            continue;
        }
        match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => {
                let version = update.version.clone();
                ready.set(update, bytes);
                notifier.notify(BackendEvent::UpdateReady { version });
            }
            Err(e) => log::warn!("update download failed: {e}"),
        }
    }
}

/// Starts [`watch`] on `rt` with the app's updater and the release version, every
/// [`WATCH_EVERY`]. Nothing in debug builds, or when this build updates nothing
/// ([`Target::current`]). Returns whether it started.
pub fn start_watch(rt: &Handle, notifier: Notifier) -> bool {
    if !CHECKS {
        return false;
    }
    remember_binary();
    if let Err(why) = Target::current() {
        log::info!("no update watch: {why}");
        return false;
    }
    let updater = match Updater::new() {
        Ok(updater) => updater.current_version(release_version()),
        Err(e) => {
            log::warn!("no update watch: {e}");
            return false;
        }
    };
    rt.spawn(watch(ready().clone(), notifier, WATCH_EVERY, updater));
    true
}

/// `restart_to_update`: the waiting download taken out and installed with `install`, then
/// `relaunch` unless an installer took over. The caller closes the app on `Ok`.
pub fn restart_to_update(
    ready: &Ready,
    install: impl FnOnce(&Update, &[u8]) -> Result<Handover, Error>,
    relaunch: impl FnOnce() -> io::Result<()>,
) -> Result<(), String> {
    let Some(Waiting { update, bytes }) = ready.take()? else {
        return Err(NOTHING_WAITING.into());
    };
    let handover = install(&update, &bytes).map_err(|e| format!("Couldn't install the update: {e}"))?;
    match handover {
        Handover::Relaunch => relaunch().map_err(|e| e.to_string()),
        Handover::Installer => Ok(()),
    }
}

type ExitHook = Box<dyn Fn() + Send + Sync>;

static EXIT: OnceLock<ExitHook> = OnceLock::new();

/// How the app closes once an update has taken over (the window's event loop, which then writes
/// the settings still waiting). Set once, at start; without it the process exits at once.
pub fn on_exit(hook: impl Fn() + Send + Sync + 'static) {
    let _ = EXIT.set(Box::new(hook));
}

/// Closes the app through [`on_exit`]'s hook.
pub fn exit() {
    match EXIT.get() {
        Some(hook) => hook(),
        None => std::process::exit(0),
    }
}

/// What the toast's Restart runs: [`restart_to_update`] on the process's target, then [`exit`].
type Run = Arc<dyn Fn(&Ready) -> Result<(), String> + Send + Sync>;

/// The toast's Restart on the desktop: [`restart_to_update`] off the UI thread (unpacking takes a
/// moment, a package's install waits for the password prompt); the answer comes only when it
/// failed, as the app closes otherwise.
pub struct Restart {
    ready: Ready,
    run: Run,
}

impl Restart {
    /// The real install, relaunch and exit.
    pub fn new(ready: Ready) -> Restart {
        Restart::with(ready, |ready| {
            let target = Target::current().map_err(|why| format!("Couldn't install the update: {why}"))?;
            restart_to_update(ready, |update, bytes| target.install(&update.version, bytes), relaunch)?;
            exit();
            Ok(())
        })
    }

    /// With `run` in place of the install (tests).
    pub fn with(ready: Ready, run: impl Fn(&Ready) -> Result<(), String> + Send + Sync + 'static) -> Restart {
        Restart { ready, run: Arc::new(run) }
    }
}

impl toast::Updater for Restart {
    fn restart_to_update(&mut self) -> Answer<Result<(), String>> {
        let (tx, rx) = mpsc::channel();
        let (ready, run) = (self.ready.clone(), self.run.clone());
        let spawned = std::thread::Builder::new().name("rustradar-update".into()).spawn(move || {
            let _ = tx.send(run(&ready));
        });
        match spawned {
            Ok(_) => Answer::Channel(rx),
            Err(e) => Answer::ready(Err(e.to_string())),
        }
    }
}

/// Hands the toast's Restart the process's [`ready`] download (the window, once its egui context
/// exists, and again after a rebuilt one).
pub fn connect(ctx: &egui::Context) {
    toast::install_updater(ctx, Box::new(Restart::new(ready().clone())));
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicUsize;

    use reqwest::Url;

    use super::*;

    /// A temp folder for one test, removed afterwards.
    fn scratch() -> TempDir {
        TempDir::new_in(&std::env::temp_dir(), "rustradar-pk3-test-").unwrap()
    }

    #[test]
    fn the_release_version_is_the_tauri_configs() {
        let conf = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/tauri.conf.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&conf).unwrap();
        let version = json["version"].as_str().unwrap();
        assert_eq!(release_version(), Version::parse(version).unwrap());
        assert_ne!(release_version(), super::super::current_version(), "not this crate's 0.1.0");
        assert_eq!(conf_version(r#"{"productName":"RustRadar","version":"0.4.2"}"#), Some(Version::new(0, 4, 2)));
        assert_eq!(conf_version(r#"{"version":"four"}"#), None);
        assert_eq!(conf_version("not json"), None);
    }

    #[test]
    fn the_constants_are_todays() {
        assert_eq!(CHECKS, !cfg!(debug_assertions));
        assert_eq!(CHECK_TIMEOUT, Duration::from_secs(8));
        assert_eq!(WATCH_EVERY, Duration::from_secs(3600));
        assert_eq!(UPDATED_FLAG, "--updated");
        assert_eq!(APP_NAME, "RustRadar");
    }

    #[test]
    fn the_relaunch_passes_the_arguments_less_updated_plus_updated() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(relaunch_args(args(&["/opt/RustRadar"])), args(&["--updated"]));
        assert_eq!(
            relaunch_args(args(&["rustradar", "--view", "59,18,9", "--updated", "--demo"])),
            args(&["--view", "59,18,9", "--demo", "--updated"]),
        );
        // the relaunched app takes the flag (and so does the native app the Tauri updater starts)
        let options = crate::Options::parse(["--demo", "--updated"].map(String::from)).unwrap();
        assert!(options.updated && options.demo);
        assert!(!crate::Options::parse(Vec::new()).unwrap().updated);
    }

    #[test]
    fn the_starting_binary_is_remembered() {
        remember_binary();
        let exe = current_binary().unwrap();
        if std::env::var_os("APPIMAGE").is_none() {
            assert_eq!(exe, std::env::current_exe().unwrap().canonicalize().unwrap());
        }
    }

    #[test]
    fn only_builds_an_installer_made_update_themselves() {
        let exe = Path::new("/opt/x/rustradar");
        let detect = |os, installer, exe: &Path, appimage: Option<&str>| {
            Target::detect(os, installer, exe, appimage.map(std::ffi::OsStr::new))
        };
        // Windows: the NSIS or MSI build; an unpatched one does not
        assert_eq!(detect("windows", Some(Installer::Nsis), exe, None), Ok(Target::Windows));
        assert_eq!(detect("windows", Some(Installer::Msi), exe, None), Ok(Target::Windows));
        assert!(detect("windows", None, exe, None).is_err());
        // macOS: the bundle three levels up, only inside one
        let in_bundle = Path::new("/Applications/RustRadar.app/Contents/MacOS/rustradar");
        assert_eq!(detect("darwin", Some(Installer::App), in_bundle, None), Ok(Target::App("/Applications/RustRadar.app".into())));
        assert!(detect("darwin", Some(Installer::App), Path::new("/Users/me/target/release/rustradar-app"), None).is_err());
        // Linux: the package's marker, else only an AppImage
        assert_eq!(detect("linux", Some(Installer::Deb), exe, None), Ok(Target::Deb(exe.into())));
        assert_eq!(detect("linux", Some(Installer::Rpm), exe, Some("/x")), Ok(Target::Rpm(exe.into())));
        assert_eq!(
            detect("linux", Some(Installer::AppImage), exe, Some("/home/me/RustRadar.AppImage")),
            Ok(Target::AppImage("/home/me/RustRadar.AppImage".into()))
        );
        assert_eq!(detect("linux", None, exe, Some("/home/me/R.AppImage")), Ok(Target::AppImage("/home/me/R.AppImage".into())));
        assert!(detect("linux", Some(Installer::AppImage), exe, None).is_err());
        assert!(detect("linux", None, exe, None).is_err(), "a plain cargo build");
        // the phones
        assert!(detect("", None, exe, None).is_err());
    }

    #[test]
    fn a_temp_folder_goes_unless_kept() {
        let base = scratch();
        let a = TempDir::new_in(base.path(), "tauri_current_app").unwrap();
        let b = TempDir::new_in(base.path(), "tauri_current_app").unwrap();
        assert_ne!(a.path(), b.path());
        let name = a.path().file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("tauri_current_app") && name.len() == "tauri_current_app".len() + 6, "{name}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(a.path().metadata().unwrap().permissions().mode() & 0o777, 0o700);
        }
        fs::write(a.path().join("f"), "x").unwrap();
        let gone = a.path().to_path_buf();
        drop(a);
        assert!(!gone.exists());
        let kept = b.keep();
        assert!(kept.is_dir());
    }

    #[test]
    fn downloads_are_told_apart_by_their_first_bytes() {
        use windows_args::{Kind, kind};
        assert_eq!(kind(b"MZ\x90\x00"), Some(Kind::Nsis));
        assert_eq!(kind(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0]), Some(Kind::Msi));
        assert_eq!(kind(b"PK\x03\x04 a zip"), None, "Tauri 1's zips are not taken");
        assert!(magic::is_gz(&[0x1F, 0x8B, 0x08, 0]));
        assert!(!magic::is_gz(b"\x7FELF"));
        let mut deb = b"!<arch>\ndebian-binary   ".to_vec();
        assert!(magic::is_deb(&deb));
        deb[9] = b'x';
        assert!(!magic::is_deb(&deb));
        let mut rpm = vec![0u8; 100];
        rpm[..4].copy_from_slice(&[0xED, 0xAB, 0xEE, 0xDB]);
        assert!(magic::is_rpm(&rpm));
        assert!(!magic::is_rpm(&rpm[..96]));
    }

    #[test]
    fn the_installers_command_lines() {
        use windows_args::{Kind, parameters};
        let args: Vec<String> = ["--view", "59,18,9", "C:\\Users\\Some User\\x"].map(String::from).to_vec();
        assert_eq!(parameters(Kind::Nsis, "C:\\T\\RustRadar-0.4.0-installer.exe", &[]), "/S /UPDATE /R /ARGS");
        assert_eq!(parameters(Kind::Nsis, "ignored", &args), "/S /UPDATE /R /ARGS --view 59,18,9 \"C:\\Users\\Some User\\x\"");
        assert_eq!(
            parameters(Kind::Msi, "C:\\Users\\Some User\\T\\RustRadar-0.4.0-installer.msi", &args),
            "/i \"C:\\Users\\Some User\\T\\RustRadar-0.4.0-installer.msi\" /quiet /promptrestart AUTOLAUNCHAPP=True LAUNCHAPPARGS=\"--view 59,18,9 \"\"C:\\Users\\Some User\\x\"\"\""
        );
    }

    /// The plugin's `it_escapes_correctly_for_msi`.
    #[test]
    fn it_escapes_correctly_for_msi() {
        let cases = [
            "something",
            "--flag",
            "--empty=",
            "--arg=value",
            "some space",
            "--arg value",
            "--arg=unwrapped space",
            "--arg=\"wrapped\"",
            "--arg=\"wrapped space\"",
            "--arg=midword\"wrapped space\"",
            "",
        ];
        let escaped = [
            "something",
            "--flag",
            "--empty=",
            "--arg=value",
            "\"\"some space\"\"",
            "\"\"--arg value\"\"",
            "--arg=\"\"unwrapped space\"\"",
            r#"--arg=""""""wrapped"""""""#,
            r#"--arg=""""""wrapped space"""""""#,
            r#"--arg=""midword""""wrapped space"""""""#,
            "\"\"\"\"",
        ];
        for (orig, escaped) in cases.iter().zip(escaped) {
            assert_eq!(windows_args::escape_msi_property_arg(orig), escaped, "{orig:?}");
        }
    }

    /// The plugin's `it_escapes_correctly_for_nsis`.
    #[test]
    fn it_escapes_correctly_for_nsis() {
        let cases = [
            "something",
            "--flag",
            "--empty=",
            "--arg=value",
            "some space",
            "--arg value",
            "--arg=unwrapped space",
            "--arg=\"wrapped\"",
            "--arg=\"wrapped space\"",
            "--arg=midword\"wrapped space\"",
            "",
        ];
        let escaped = [
            "something",
            "--flag",
            "--empty=",
            "--arg=value",
            "\"some space\"",
            "\"--arg value\"",
            "\"--arg=unwrapped space\"",
            "--arg=\\\"wrapped\\\"",
            "\"--arg=\\\"wrapped space\\\"\"",
            "\"--arg=midword\\\"wrapped space\\\"\"",
            "\"\"",
        ];
        for (orig, escaped) in cases.iter().zip(escaped) {
            assert_eq!(windows_args::escape_nsis_current_exe_arg(orig), escaped, "{orig:?}");
        }
        // backslashes before a quote and at the end of a quoted argument are doubled
        assert_eq!(windows_args::escape_nsis_current_exe_arg("C:\\a b\\"), "\"C:\\a b\\\\\"");
        assert_eq!(windows_args::escape_nsis_current_exe_arg("a\\\"b"), "a\\\\\\\"b");
        assert_eq!(windows_args::escape_nsis_current_exe_arg("/x"), "\"/x\"");
    }

    // ---- the macOS bundle swap and its rollback, on temp folders --------------------------------

    #[cfg(unix)]
    mod bundles {
        use std::os::unix::fs::PermissionsExt;

        use super::super::bundle::{self, Ops};
        use super::*;

        /// A `.app.tar.gz` as the bundler makes it: `RustRadar.app/` with `Contents/…` inside.
        fn app_archive(version: &str) -> Vec<u8> {
            let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
            let mut dir = tar::Header::new_gnu();
            dir.set_entry_type(tar::EntryType::Directory);
            dir.set_mode(0o755);
            dir.set_size(0);
            for path in ["RustRadar.app/", "RustRadar.app/Contents/", "RustRadar.app/Contents/MacOS/"] {
                builder.append_data(&mut dir.clone(), path, std::io::empty()).unwrap();
            }
            let file = |builder: &mut tar::Builder<_>, path: &str, mode: u32, data: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_mode(mode);
                h.set_size(data.len() as u64);
                builder.append_data(&mut h, path, data).unwrap();
            };
            file(&mut builder, "RustRadar.app/Contents/Info.plist", 0o644, format!("<plist>{version}</plist>").as_bytes());
            file(&mut builder, "RustRadar.app/Contents/MacOS/rustradar", 0o755, format!("binary {version}").as_bytes());
            builder.into_inner().unwrap().finish().unwrap()
        }

        /// The installed app at `dir/RustRadar.app`, version `old`.
        fn installed(dir: &Path) -> PathBuf {
            let app = dir.join("RustRadar.app");
            fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
            fs::write(app.join("Contents/Info.plist"), "<plist>old</plist>").unwrap();
            fs::write(app.join("Contents/MacOS/rustradar"), "binary old").unwrap();
            app
        }

        fn version_of(app: &Path) -> String {
            fs::read_to_string(app.join("Contents/MacOS/rustradar")).unwrap()
        }

        fn no_swap(_: &Path, _: &Path) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }

        fn no_admin(_: &Path, _: &Path, _: &Path) -> Result<(), String> {
            panic!("the administrator is not asked")
        }

        /// The swap `renamex_np(RENAME_SWAP)` makes, with three renames.
        fn swap_by_renames(target: &Path, staged: &Path) -> io::Result<()> {
            let aside = staged.with_extension("swapping");
            fs::rename(target, &aside)?;
            fs::rename(staged, target)?;
            fs::rename(&aside, staged)
        }

        fn leftovers(dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        }

        #[test]
        fn the_bundle_is_replaced_by_moves_where_it_cannot_be_swapped() {
            let dir = scratch();
            let app = installed(dir.path());
            let tmp = TempDir::new_in(dir.path(), "tmp").unwrap();
            let ops = Ops { tmp_roots: vec![tmp.path().to_path_buf()], swap: &no_swap, privileged: &no_admin, home: None };
            bundle::install(&app, &app_archive("new"), &ops).unwrap();
            assert_eq!(version_of(&app), "binary new");
            assert_eq!(fs::read_to_string(app.join("Contents/Info.plist")).unwrap(), "<plist>new</plist>");
            let mode = |p: &Path| p.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&app), 0o755, "the bundle opens for the machine's other users");
            assert_eq!(mode(&app.join("Contents/MacOS/rustradar")), 0o755);
            assert!(leftovers(tmp.path()).is_empty(), "the old app and the temp folders are gone: {:?}", leftovers(tmp.path()));
        }

        #[test]
        fn the_bundle_is_swapped_in_one_step_where_it_can_be() {
            let dir = scratch();
            let app = installed(dir.path());
            let tmp = TempDir::new_in(dir.path(), "tmp").unwrap();
            let swapped = AtomicUsize::new(0);
            let swap = |a: &Path, b: &Path| {
                swapped.fetch_add(1, Ordering::SeqCst);
                swap_by_renames(a, b)
            };
            let ops = Ops { tmp_roots: vec![tmp.path().to_path_buf()], swap: &swap, privileged: &no_admin, home: None };
            bundle::install(&app, &app_archive("new"), &ops).unwrap();
            assert_eq!(swapped.load(Ordering::SeqCst), 1);
            assert_eq!(version_of(&app), "binary new");
            assert!(leftovers(tmp.path()).is_empty());
        }

        #[test]
        fn a_broken_download_leaves_the_app_as_it_was() {
            let dir = scratch();
            let app = installed(dir.path());
            let tmp = TempDir::new_in(dir.path(), "tmp").unwrap();
            let ops = Ops { tmp_roots: vec![tmp.path().to_path_buf()], swap: &no_swap, privileged: &no_admin, home: None };
            let mut archive = app_archive("new");
            archive.truncate(archive.len() / 2);
            assert!(bundle::install(&app, &archive, &ops).is_err());
            assert_eq!(version_of(&app), "binary old");
            assert!(leftovers(tmp.path()).is_empty());
            // and no temp folder on the app's volume: refused before anything moved
            let elsewhere = Ops { tmp_roots: vec![], swap: &no_swap, privileged: &no_admin, home: None };
            assert!(matches!(bundle::install(&app, &app_archive("new"), &elsewhere), Err(Error::TempDirNotOnSameMountPoint)));
            assert_eq!(version_of(&app), "binary old");
        }

        #[test]
        fn a_permission_error_asks_the_administrator() {
            let dir = scratch();
            let app = installed(dir.path());
            let tmp = TempDir::new_in(dir.path(), "tmp").unwrap();
            let denied = |_: &Path, _: &Path| -> io::Result<()> { Err(io::Error::from(io::ErrorKind::PermissionDenied)) };
            let asked = AtomicUsize::new(0);
            // the administrator's command, run here without the prompt
            let admin = |target: &Path, staged: &Path, backup: &Path| {
                asked.fetch_add(1, Ordering::SeqCst);
                swap_by_renames(target, staged).map_err(|e| e.to_string())?;
                let _ = backup;
                fs::remove_dir_all(staged).map_err(|e| e.to_string())
            };
            let ops = Ops { tmp_roots: vec![tmp.path().to_path_buf()], swap: &denied, privileged: &admin, home: None };
            bundle::install(&app, &app_archive("new"), &ops).unwrap();
            assert_eq!(asked.load(Ordering::SeqCst), 1);
            assert_eq!(version_of(&app), "binary new");

            // refused (the prompt dismissed): the app stays as it was, the error says so
            let refused = |_: &Path, _: &Path, _: &Path| Err("User canceled.".to_owned());
            let ops = Ops { tmp_roots: vec![tmp.path().to_path_buf()], swap: &denied, privileged: &refused, home: None };
            let err = bundle::install(&app, &app_archive("newer"), &ops).unwrap_err();
            assert_eq!(err.to_string(), "Failed to move the new app into place");
            assert_eq!(version_of(&app), "binary new");
            assert!(leftovers(tmp.path()).is_empty());
        }

        /// The plugin's `replace_bundle_moves_the_staged_bundle_into_place` and
        /// `replace_bundle_restores_the_current_bundle_when_the_move_fails`.
        #[test]
        fn replace_bundle_moves_and_restores() {
            let dir = scratch();
            let target = dir.path().join("App.app");
            let staged = dir.path().join("staged");
            let backup = dir.path().join("backup");
            fs::create_dir(&target).unwrap();
            fs::write(target.join("version"), "old").unwrap();
            fs::create_dir(&staged).unwrap();
            fs::write(staged.join("version"), "new").unwrap();
            bundle::replace_bundle(&target, &staged, &backup).unwrap();
            assert_eq!(fs::read_to_string(target.join("version")).unwrap(), "new");
            assert_eq!(fs::read_to_string(backup.join("version")).unwrap(), "old");
            assert!(!staged.exists());

            fs::remove_dir_all(&backup).unwrap();
            let err = bundle::replace_bundle(&target, &dir.path().join("missing"), &backup).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
            assert_eq!(fs::read_to_string(target.join("version")).unwrap(), "new", "put back");
            assert!(!backup.exists());
        }

        /// The plugin's four `keep_backup_if_not_restored` tests.
        #[test]
        fn an_old_app_that_was_not_put_back_is_kept() {
            let dir = scratch();
            let target = dir.path().join("App.app");
            let backup_in = |dir: &Path| {
                let backup_dir = TempDir::new_in(dir, "tauri_current_app").unwrap();
                let backup = backup_dir.path().join("current_app");
                fs::create_dir(&backup).unwrap();
                fs::write(backup.join("version"), "old").unwrap();
                (backup_dir, backup)
            };

            // beside the app
            let (backup_dir, backup) = backup_in(dir.path());
            let backup_dir_path = backup_dir.path().to_path_buf();
            let expected = dir.path().join("App (previous version).app");
            assert_eq!(bundle::keep_backup_if_not_restored(&target, backup_dir, &backup, None), Some(expected.clone()));
            assert_eq!(fs::read_to_string(expected.join("version")).unwrap(), "old");
            assert!(!backup_dir_path.exists());

            // that name taken (now a file): the fallback folder
            fs::remove_dir_all(&expected).unwrap();
            fs::write(&expected, "other").unwrap();
            let fallback = TempDir::new_in(dir.path(), "home").unwrap();
            let (backup_dir, backup) = backup_in(dir.path());
            let in_fallback = fallback.path().join("App (previous version).app");
            assert_eq!(bundle::keep_backup_if_not_restored(&target, backup_dir, &backup, Some(fallback.path())), Some(in_fallback.clone()));
            assert_eq!(fs::read_to_string(in_fallback.join("version")).unwrap(), "old");

            // neither: the backup stays where it is, its folder kept
            let (backup_dir, backup) = backup_in(dir.path());
            let missing = dir.path().join("missing");
            assert_eq!(bundle::keep_backup_if_not_restored(&target, backup_dir, &backup, Some(&missing)), Some(backup.clone()));
            assert_eq!(fs::read_to_string(backup.join("version")).unwrap(), "old");
            assert_eq!(fs::read_to_string(&expected).unwrap(), "other", "never moved onto");

            // the app back in place: nothing kept, the backup's folder removed
            fs::create_dir(&target).unwrap();
            let (backup_dir, backup) = backup_in(dir.path());
            let backup_dir_path = backup_dir.path().to_path_buf();
            assert_eq!(bundle::keep_backup_if_not_restored(&target, backup_dir, &backup, None), None);
            assert!(!backup_dir_path.exists());
        }

        /// The administrator's command with paths `sh` and AppleScript would misread unquoted, run
        /// by `sh` with an `osascript` that cannot swap: the moves' branch.
        #[test]
        fn the_administrators_command_quotes_every_path() {
            let base = scratch();
            let dir = TempDir::new_in(base.path(), "it's \"a\" \\ $(touch pwned) `dir`\n").unwrap();
            let target = dir.path().join("Bob's \"App\".app");
            let staged = dir.path().join("staged");
            let backup = dir.path().join("backup");
            fs::create_dir(&target).unwrap();
            fs::write(target.join("version"), "old").unwrap();
            fs::create_dir(&staged).unwrap();
            fs::write(staged.join("version"), "new").unwrap();
            let fake = base.path().join("osascript");
            fs::write(&fake, "#!/bin/sh\necho failed\n").unwrap();
            fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

            let command = bundle::privileged_install_command(&target, &staged, &backup, &fake);
            let status = std::process::Command::new("sh").arg("-c").arg(&command).current_dir(dir.path()).status().unwrap();
            assert!(status.success());
            assert_eq!(fs::read_to_string(target.join("version")).unwrap(), "new");
            assert!(!staged.exists() && !backup.exists());
            assert!(!dir.path().join("pwned").exists() && !base.path().join("pwned").exists());

            // an osascript that says it swapped: the staged folder (the old app) removed
            fs::create_dir(&staged).unwrap();
            fs::write(&fake, "#!/bin/sh\necho swapped\n").unwrap();
            let command = bundle::privileged_install_command(&target, &staged, &backup, &fake);
            assert!(std::process::Command::new("sh").arg("-c").arg(&command).status().unwrap().success());
            assert!(!staged.exists());
            // one that fails outright: the shell fails and nothing moves
            fs::create_dir(&staged).unwrap();
            fs::write(&fake, "#!/bin/sh\nexit 3\n").unwrap();
            let command = bundle::privileged_install_command(&target, &staged, &backup, &fake);
            assert!(!std::process::Command::new("sh").arg("-c").arg(&command).status().unwrap().success());
            assert!(staged.exists() && target.exists());

            assert_eq!(bundle::applescript_string("say \"hi\" \\ there"), r#""say \"hi\" \\ there""#);
            assert_eq!(bundle::sh_quote(Path::new("it's")), r"'it'\''s'");
            assert!(bundle::SWAP_BUNDLE_JXA.contains("renamex_np(argv[0], argv[1], 2)"));
            assert_eq!(bundle::OSASCRIPT, "/usr/bin/osascript");
        }
    }

    // ---- the AppImage and the packages, on temp folders ------------------------------------------

    #[cfg(target_os = "linux")]
    mod appimages {
        use std::os::unix::fs::PermissionsExt;

        use super::super::linux::{self, Package};
        use super::*;

        fn installed(dir: &Path) -> PathBuf {
            let path = dir.join("RustRadar_0.4.0_amd64.AppImage");
            fs::write(&path, "old AppImage").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
            path
        }

        fn mode(path: &Path) -> u32 {
            path.metadata().unwrap().permissions().mode() & 0o777
        }

        fn gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
            let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
            for (path, data) in entries {
                let mut h = tar::Header::new_gnu();
                h.set_mode(0o755);
                h.set_size(data.len() as u64);
                builder.append_data(&mut h, path, *data).unwrap();
            }
            builder.into_inner().unwrap().finish().unwrap()
        }

        #[test]
        fn the_appimage_is_rewritten_in_place_with_its_permissions() {
            let dir = scratch();
            let path = installed(dir.path());
            let tmp = TempDir::new_in(dir.path(), "tmp").unwrap();
            linux::install_appimage(&path, b"new AppImage", &[None, Some(tmp.path().to_path_buf())]).unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"new AppImage");
            assert_eq!(mode(&path), 0o751);
            assert!(fs::read_dir(tmp.path()).unwrap().next().is_none(), "the old one went with its temp folder");
        }

        #[test]
        fn the_appimage_of_a_tar_gz_is_unpacked_over_it() {
            let dir = scratch();
            let path = installed(dir.path());
            let tmp = TempDir::new_in(dir.path(), "tmp").unwrap();
            let archive = gz(&[("README", b"x"), ("RustRadar_0.4.1_amd64.AppImage", b"unpacked AppImage")]);
            linux::install_appimage(&path, &archive, &[Some(tmp.path().to_path_buf())]).unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"unpacked AppImage");
            assert_eq!(mode(&path), 0o755, "the archive's mode");
        }

        #[test]
        fn a_failed_replace_puts_the_old_appimage_back() {
            let dir = scratch();
            let path = installed(dir.path());
            let tmp = TempDir::new_in(dir.path(), "tmp").unwrap();
            let locations = [Some(tmp.path().to_path_buf())];
            let err = linux::install_appimage(&path, &gz(&[("README", b"no AppImage here")]), &locations).unwrap_err();
            assert!(matches!(err, Error::BinaryNotFoundInArchive), "{err}");
            assert_eq!(fs::read(&path).unwrap(), b"old AppImage");
            assert_eq!(mode(&path), 0o751);
            // no temp folder anywhere: refused before anything moved
            let err = linux::install_appimage(&path, b"new", &[None]).unwrap_err();
            assert!(matches!(err, Error::TempDirNotOnSameMountPoint));
            assert_eq!(fs::read(&path).unwrap(), b"old AppImage");
            // a missing AppImage is an error, not a write somewhere
            assert!(linux::install_appimage(&dir.path().join("gone.AppImage"), b"new", &locations).is_err());
            assert!(!dir.path().join("gone.AppImage").exists());
        }

        #[test]
        fn a_package_is_written_to_a_temp_folder_and_installed() {
            let dir = scratch();
            let mut deb = b"!<arch>\ndebian-binary   2.0\n".to_vec();
            deb.extend_from_slice(&[7; 64]);
            let seen = Mutex::new(Vec::new());
            let install = |pkg: &Path, command: &str, arg: &str| {
                seen.lock().unwrap().push((pkg.to_path_buf(), fs::read(pkg).unwrap(), command.to_owned(), arg.to_owned()));
                Ok(())
            };
            linux::install_package(&deb, &[None, Some(dir.path().to_path_buf())], Package::Deb, install).unwrap();
            let seen = seen.into_inner().unwrap();
            assert_eq!(seen.len(), 1);
            let (pkg, bytes, command, arg) = &seen[0];
            assert_eq!(pkg.file_name().unwrap(), "package.deb");
            assert!(pkg.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with("tauri_deb_update"));
            assert_eq!((bytes, command.as_str(), arg.as_str()), (&deb, "dpkg", "-i"));
            assert!(!pkg.exists(), "removed after the install");

            // not a package: refused without asking anyone
            let never = |_: &Path, _: &str, _: &str| -> Result<(), Error> { panic!("not installed") };
            assert!(matches!(linux::install_package(b"not a deb", &[Some(dir.path().into())], Package::Deb, never), Err(Error::InvalidUpdaterFormat)));
            assert!(matches!(linux::install_package(&deb, &[Some(dir.path().into())], Package::Rpm, never), Err(Error::InvalidUpdaterFormat)));
            assert!(matches!(linux::install_package(&deb, &[None], Package::Deb, never), Err(Error::TempDirNotFound)));
            assert_eq!(Package::Rpm.command(), ("rpm", "-U", "rpm"));
            // the package manager's failure comes back
            let failing = |_: &Path, _: &str, _: &str| Err(Error::PackageInstallFailed);
            let err = linux::install_package(&deb, &[Some(dir.path().into())], Package::Deb, failing).unwrap_err();
            assert_eq!(err.to_string(), "Failed to install package");
        }
    }

    // ---- the watch, restart_to_update and the toast, against a local listener --------------------

    // P2's throwaway key (its secret half was never stored) and its signature of `payload()` with
    // the trusted comment "timestamp:1794385122\tfile:RustRadar_9.0.0_x64-setup.exe\tversion:9.0.0".
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDBBQ0FERTQ5QjE3MDc5NjcKUldSbmVYQ3hTZDdLQ2gvZ09mRzIzdW0rQXppSzBQVlZXV3RzTUhHWkdET1lYQTFuUDNGYlJITk8K";
    const SIG_9: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSbmVYQ3hTZDdLQ3VkZHNnMGt2V1RiekdmRlUzRnJtbEZGUVR4RVRtVFpNTzZrcmwxYStpZWpRdzdYWG9BWStJUWRRaWpqcEhCR0ZwUnViQWFIWk5lUjU1MzdpSHYzV3dFPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0Mzg1MTIyCWZpbGU6UnVzdFJhZGFyXzkuMC4wX3g2NC1zZXR1cC5leGUJdmVyc2lvbjo5LjAuMApublhhbmk5Q2dUckpHc3FmY3VGdTBFb1FrMkFRdldxQlRyZ1BxWXJuclFOOHZyMXBwbHJIVnJ0RUY3REtsa21jNmtzbXE1OEZrU3hOUjZFN01qakJEUT09Cg==";

    /// The signed artifact: 200 000 bytes of a 32-bit LCG's high bytes.
    pub(in crate::update) fn payload() -> Vec<u8> {
        let mut x: u32 = 0x2545_F491;
        (0..200_000)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect()
    }

    type Routes = Arc<Mutex<HashMap<String, (u16, Vec<u8>)>>>;

    /// An HTTP/1.1 server on 127.0.0.1 answering fixed routes, bodies in eight pieces; a route
    /// whose status is 0 sends its headers and then nothing.
    pub(in crate::update) struct Server {
        base: String,
        routes: Routes,
        asked: Arc<Mutex<Vec<String>>>,
        /// Milliseconds between a body's eight pieces.
        pace: Arc<AtomicU64>,
    }

    impl Server {
        pub(in crate::update) fn start() -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (routes, asked): (Routes, Arc<Mutex<Vec<String>>>) = Default::default();
            let pace = Arc::new(AtomicU64::new(2));
            let (r, a, p) = (routes.clone(), asked.clone(), pace.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (r, a, ms) = (r.clone(), a.clone(), p.load(Ordering::Relaxed));
                    std::thread::spawn(move || answer(stream, &r, &a, ms));
                }
            });
            Server { base, routes, asked, pace }
        }

        pub(in crate::update) fn route(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
            self.routes.lock().unwrap().insert(path.to_owned(), (status, body.into()));
        }

        /// Bodies sent in pieces `ms` apart from now on (eight pieces a body).
        #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
        pub(in crate::update) fn pace(&self, ms: u64) {
            self.pace.store(ms, Ordering::Relaxed);
        }

        pub(in crate::update) fn asked(&self, path: &str) -> usize {
            self.asked.lock().unwrap().iter().filter(|p| *p == path).count()
        }

        fn url(&self, path: &str) -> Url {
            Url::parse(&format!("{}{path}", self.base)).unwrap()
        }

        /// `/latest.json` offering `version`'s `/artifact` for this platform, signed by [`SIG_9`].
        pub(in crate::update) fn release(&self, version: &str) {
            let key = format!("{}-{}", super::super::manifest::os().unwrap(), super::super::manifest::arch().unwrap());
            let json = serde_json::json!({
                "version": version,
                "pub_date": "2026-11-11T08:21:15.913Z",
                "platforms": { key: { "signature": SIG_9, "url": self.url("/artifact").as_str() } },
            });
            self.route("/latest.json", 200, json.to_string());
        }

        pub(in crate::update) fn updater(&self) -> Updater {
            let client = super::super::client_builder().no_proxy().timeout(Duration::from_secs(20)).build().unwrap();
            Updater::with_client(client)
                .endpoints(vec![self.url("/latest.json")])
                .pubkey(TEST_PUBKEY)
                .current_version(Version::new(8, 1, 0))
                .installer(None)
        }
    }

    fn answer(stream: TcpStream, routes: &Routes, asked: &Mutex<Vec<String>>, pace_ms: u64) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                break;
            }
        }
        asked.lock().unwrap().push(path.clone());
        let (status, body) = routes.lock().unwrap().get(&path).cloned().unwrap_or((404, b"Not Found".to_vec()));
        let mut stream = stream;
        if status == 0 {
            // the headers, then a body that never comes
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len().max(1));
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
        let reason = if status == 200 { "OK" } else { "Not Found" };
        if write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).is_err() {
            return;
        }
        for piece in body.chunks(body.len().div_ceil(8).max(1)) {
            if stream.write_all(piece).and_then(|()| stream.flush()).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(pace_ms));
        }
    }

    fn notifier() -> (Notifier, mpsc::Receiver<BackendEvent>) {
        let (tx, rx) = mpsc::channel();
        (Notifier::new(tx, Arc::new(|| {})), rx)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_watch_downloads_a_newer_version_once_and_announces_it() {
        let server = Server::start();
        server.route("/artifact", 200, payload());
        server.release("9.0.0");
        let (notifier, events) = notifier();
        let ready = Ready::default();
        let task = tokio::spawn(watch(ready.clone(), notifier, Duration::from_millis(30), server.updater()));
        let started = std::time::Instant::now();
        while server.asked("/latest.json") < 4 && started.elapsed() < Duration::from_secs(20) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        task.abort();
        assert!(server.asked("/latest.json") >= 4, "checked each turn");
        assert_eq!(server.asked("/artifact"), 1, "the version already waiting is not downloaded again");
        assert_eq!(events.try_iter().collect::<Vec<_>>(), [BackendEvent::UpdateReady { version: "9.0.0".into() }]);
        assert_eq!(ready.pending_update().as_deref(), Some("9.0.0"));
        let waiting = ready.take().unwrap().unwrap();
        assert_eq!(waiting.bytes, payload(), "verified and kept in memory");
        assert_eq!(waiting.update.version, "9.0.0");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_watch_waits_out_failures() {
        let server = Server::start();
        // a tampered artifact fails its signature; nothing is kept or announced
        let mut tampered = payload();
        tampered[123_456] ^= 1;
        server.route("/artifact", 200, tampered);
        server.release("9.0.0");
        let (notifier, events) = notifier();
        let ready = Ready::default();
        let task = tokio::spawn(watch(ready.clone(), notifier, Duration::from_millis(30), server.updater()));
        let started = std::time::Instant::now();
        while server.asked("/artifact") < 2 && started.elapsed() < Duration::from_secs(20) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // then the right one: found the next turn
        server.route("/artifact", 200, payload());
        while ready.pending_update().is_none() && started.elapsed() < Duration::from_secs(20) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        task.abort();
        assert_eq!(events.try_iter().collect::<Vec<_>>(), [BackendEvent::UpdateReady { version: "9.0.0".into() }]);
        assert_eq!(ready.take().unwrap().unwrap().bytes, payload());
    }

    /// A waiting download, as the watch leaves it.
    async fn waiting(server: &Server) -> Ready {
        server.route("/artifact", 200, payload());
        server.release("9.0.0");
        let update = server.updater().check().await.unwrap().unwrap();
        let bytes = update.download(|_, _| {}, || {}).await.unwrap();
        let ready = Ready::default();
        ready.set(update, bytes);
        ready
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restart_installs_the_waiting_version_then_relaunches() {
        let server = Server::start();
        let ready = waiting(&server).await;
        let installed = Mutex::new(None);
        let relaunched = AtomicUsize::new(0);
        let result = restart_to_update(
            &ready,
            |update, bytes| {
                *installed.lock().unwrap() = Some((update.version.clone(), bytes.len()));
                Ok(Handover::Relaunch)
            },
            || {
                relaunched.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(*installed.lock().unwrap(), Some(("9.0.0".to_owned(), 200_000)));
        assert_eq!(relaunched.load(Ordering::SeqCst), 1);
        assert_eq!(ready.pending_update(), None, "taken");
        // nothing waiting any more
        assert_eq!(restart_to_update(&ready, |_, _| Ok(Handover::Relaunch), || Ok(())), Err(NOTHING_WAITING.into()));

        // Windows: the installer takes over, nothing is relaunched
        let ready = waiting(&server).await;
        let never = || -> io::Result<()> { panic!("the installer starts the app") };
        assert_eq!(restart_to_update(&ready, |_, _| Ok(Handover::Installer), never), Ok(()));

        // a failed install, and a failed relaunch, are the toast's sub line
        let ready = waiting(&server).await;
        let result = restart_to_update(&ready, |_, _| Err(Error::InvalidUpdaterFormat), || Ok(()));
        assert_eq!(result, Err("Couldn't install the update: invalid updater binary format".into()));
        assert_eq!(ready.pending_update(), None, "taken even so, as today");
        let ready = waiting(&server).await;
        let result = restart_to_update(&ready, |_, _| Ok(Handover::Relaunch), || Err(io::Error::other("no binary")));
        assert_eq!(result, Err("no binary".into()));
    }

    #[test]
    fn the_toasts_restart_runs_off_the_ui_thread() {
        let ctx = egui::Context::default();
        let ready = Ready::default();
        let threads = Arc::new(Mutex::new(Vec::new()));
        let seen = threads.clone();
        let restart = Restart::with(ready, move |ready| {
            seen.lock().unwrap().push(std::thread::current().name().map(str::to_owned));
            restart_to_update(ready, |_, _| Ok(Handover::Relaunch), || Ok(()))
        });
        toast::install_updater(&ctx, Box::new(restart));
        let state = toast::toast(&ctx);
        let mut t = state.lock().unwrap();
        t.restart(false);
        assert!(t.restarting, "Restarting… while it runs");
        let started = std::time::Instant::now();
        while t.poll() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // nothing downloaded yet: the toast says so and enables Restart again
        assert_eq!(t.sub(false), NOTHING_WAITING);
        assert!(!t.restarting);
        assert_eq!(*threads.lock().unwrap(), [Some("rustradar-update".to_owned())]);
    }

    #[test]
    fn the_exit_hook_closes_the_app() {
        let closed = Arc::new(AtomicUsize::new(0));
        let count = closed.clone();
        on_exit(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        exit();
        assert_eq!(closed.load(Ordering::SeqCst), 1, "through the hook, not process::exit");
    }

    #[test]
    fn debug_builds_watch_nothing() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (notifier, _) = notifier();
        assert_eq!(start_watch(rt.handle(), notifier), CHECKS && Target::current().is_ok());
        assert!(pending_update().is_none());
    }

    #[test]
    fn the_messages_are_the_plugins() {
        assert_eq!(Error::TempDirNotOnSameMountPoint.to_string(), "temp directory is not on the same mount point as the app");
        assert_eq!(
            Error::PreviousAppNotRestored("/Applications/RustRadar (previous version).app".into()).to_string(),
            "failed to install the update and to restore the previous app, which was left at /Applications/RustRadar (previous version).app"
        );
        assert_eq!(Error::BinaryNotFoundInArchive.to_string(), "binary for the current target not found in the archive");
        assert_eq!(Error::AuthenticationFailed.to_string(), "Authentication failed or was cancelled");
        assert_eq!(Error::from(io::Error::other("disk full")).to_string(), "disk full");
    }
}
