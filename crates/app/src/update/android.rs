//! The Android update notice (item P4; doc 07 §7.3, §12.3; `RS:mobile_update.rs`). Android installs
//! come from the APK on each GitHub release, which the app cannot replace itself. Instead a newer
//! release raises the same update toast as on the desktop, and its button (`Download` there) opens
//! the APK's download for Android to install. iOS builds update through TestFlight, so there is
//! nothing to check there.
//!
//! What a release says is read by `rustradar_backend::mobile_update` (moved there in stage 1 with
//! its test `finds_a_newer_apk`): [`latest_apk`] asks
//! `https://api.github.com/repos/felixgoff/RustRadar/releases/latest` with `User-Agent: RustRadar`
//! and `Accept: application/vnd.github+json`, and `newer_apk` keeps the release's version (its `v`
//! dropped) and the first asset whose name ends in `.apk`, when the tag is strictly newer than the
//! running version as three integers (anything else: no update). This module keeps the rest, as
//! `mobile_update.rs` did around its `AppHandle`:
//!
//! - [`start`] (today's `run`): nothing in debug builds or on iOS ([`CHECKS`]); else, on the
//!   backend's runtime, [`watch`]: a check now and every [`CHECK_EVERY`] (an hour), a failure
//!   logged (`update check failed: {e}`) and the next hour waited for. The HTTP client is the
//!   backend's Flightradar24 client's, read again at each check (`Backend::client().http()`).
//! - [`found`] (today's `check`): a newer release is remembered as `(version, url)` in [`Ready`]
//!   (today's `ready()` static) and announced as `BackendEvent::UpdateReady` (today's
//!   `update-ready` event), which `AppState::poll` turns into `state.update_ready` for the toast.
//!   An hour later the same release is announced again, as today; the toast's `Later` is kept per
//!   version, so it stays hidden.
//! - [`pending_update`]: the remembered version (the toast's `pending_update` at mount).
//! - [`Ready::restart_to_update`]: opens the remembered URL ([`platform::open_url`], the opener's
//!   `open_url`; `webbrowser` on Android), or `No update is waiting`. [`connect`] hands it to the
//!   toast's button ([`Download`], a `toast::Updater`).
//!
//! Wiring (`app.rs`, Android only): [`start`] with a `Notifier` on the state's event queue right
//! after the backend has started, `state.update_ready` from [`pending_update`], and [`connect`]
//! once the window's egui context exists.
//!
//! Deviations:
//! - The failure is logged with `log::warn!` (logcat), not `eprintln!`.
//! - Android may run `android_main` again for a new activity in the same process: [`start`] then
//!   stops the previous activity's loop (its queue and wake are gone) and starts one for the new
//!   one; what was found is kept for the process ([`ready`]), so the new state starts from it. The
//!   Tauri app started one loop per process.
//! - Without a backend (no folders or no HTTP client; the app then runs without feeds) there is no
//!   check: today's app always had its backend.
//!
//! The running version is this crate's `CARGO_PKG_VERSION` ([`VERSION`]), the release version,
//! which `cargo xtask bump` keeps with `tauri.conf.json`'s (item F5; [`super`]): the same text as
//! today's `app.package_info().version.to_string()`. Debug builds never check.

use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use rustradar_backend::mobile_update::latest_apk;
use rustradar_backend::{Backend, BackendEvent, Notifier};
use tokio::task::AbortHandle;

use crate::platform;
use crate::state::aircraft::Answer;
use crate::ui::toast;

pub use rustradar_backend::mobile_update::{CHECK_EVERY, LATEST};

/// The running app's version, compared with the release's tag (`app.package_info().version`): the
/// release version `crates/app/Cargo.toml` carries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether this build looks for releases: not in debug builds, not on iOS (TestFlight updates it).
pub const CHECKS: bool = !(cfg!(debug_assertions) || cfg!(target_os = "ios"));

/// What [`Ready::restart_to_update`] answers before a newer release was found.
pub const NOTHING_WAITING: &str = "No update is waiting";

/// What one check answers: the newer release's version and APK URL, if there is one.
pub type Latest = Result<Option<(String, String)>, String>;

/// The newer version and where its APK is (today's `ready()`), shared by the check and the toast.
/// Cheap to clone; every clone is the same slot.
#[derive(Clone, Default, Debug)]
pub struct Ready(Arc<Mutex<Option<(String, String)>>>);

impl Ready {
    /// Remembers `version` and its APK's `url`.
    pub fn set(&self, version: String, url: String) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some((version, url));
    }

    /// `pending_update`: the version found, if any.
    pub fn pending_update(&self) -> Option<String> {
        self.0.lock().ok()?.as_ref().map(|(v, _)| v.clone())
    }

    /// `restart_to_update`: opens the APK's download with `open`; Android takes it from there.
    pub fn restart_to_update(&self, open: impl FnOnce(&str) -> Result<(), String>) -> Result<(), String> {
        let url = self
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .as_ref()
            .map(|(_, url)| url.clone())
            .ok_or(NOTHING_WAITING)?;
        open(&url)
    }
}

/// The process's [`Ready`]: what [`start`]'s checks found, what [`connect`]'s button opens.
pub fn ready() -> &'static Ready {
    static READY: LazyLock<Ready> = LazyLock::new(Ready::default);
    &READY
}

/// `pending_update`: the version the process's checks found, if any.
pub fn pending_update() -> Option<String> {
    ready().pending_update()
}

/// Today's `check` after the request: a newer release remembered in `ready` and announced to the
/// UI through `notifier`; a failed request is handed back.
pub fn found(ready: &Ready, notifier: &Notifier, latest: Latest) -> Result<(), String> {
    if let Some((version, url)) = latest? {
        ready.set(version.clone(), url);
        notifier.notify(BackendEvent::UpdateReady { version });
    }
    Ok(())
}

/// Today's loop in `run`: `latest()` now and then every `every`, each answer through [`found`], a
/// failure logged and the next turn waited for. Never ends; abort its task to stop it.
pub async fn watch<F, Fut>(ready: Ready, notifier: Notifier, every: Duration, mut latest: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Latest>,
{
    loop {
        let answer = latest().await;
        if let Err(e) = found(&ready, &notifier, answer) {
            log::warn!("update check failed: {e}");
        }
        tokio::time::sleep(every).await;
    }
}

/// Starts looking for a newer release on `backend`'s runtime, now and every hour, announcing it
/// through `notifier` (today's `run`). Nothing when [`CHECKS`] is off or there is no backend.
/// Returns whether a loop was started; one started earlier in the process is stopped first.
pub fn start(backend: Option<&Backend>, notifier: Notifier) -> bool {
    if !CHECKS {
        return false;
    }
    let Some(backend) = backend else {
        log::warn!("no update check: running without the backend");
        return false;
    };
    static RUNNING: Mutex<Option<AbortHandle>> = Mutex::new(None);
    let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(previous) = running.take() {
        previous.abort();
    }
    let http_of = backend.clone();
    let task = backend.rt().spawn(watch(ready().clone(), notifier, CHECK_EVERY, move || {
        let http = http_of.client().http().clone();
        async move { latest_apk(&http, VERSION).await }
    }));
    *running = Some(task.abort_handle());
    true
}

/// How [`Download`] opens a URL: [`platform::open_url`], or a test's recorder.
pub type Opener = Box<dyn FnMut(&str) -> Result<(), String> + Send>;

/// The toast's button on Android: opens the found release's APK ([`Ready::restart_to_update`]).
pub struct Download {
    ready: Ready,
    open: Opener,
}

impl Download {
    /// Opening with the system's browser ([`platform::open_url`]).
    pub fn new(ready: Ready) -> Self {
        Download::with_opener(ready, platform::open_url)
    }

    /// Opening with `open` (tests).
    pub fn with_opener(ready: Ready, open: impl FnMut(&str) -> Result<(), String> + Send + 'static) -> Self {
        Download { ready, open: Box::new(open) }
    }
}

impl toast::Updater for Download {
    fn restart_to_update(&mut self) -> Answer<Result<(), String>> {
        Answer::ready(self.ready.restart_to_update(&mut self.open))
    }
}

/// Hands the toast's `Download` the process's [`ready`] release (the window, once its egui
/// context exists).
pub fn connect(ctx: &egui::Context) {
    toast::install_updater(ctx, Box::new(Download::new(ready().clone())));
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use rustradar_backend::mobile_update::newer_apk;

    use super::*;

    /// The release of `mobile_update.rs`'s test: an AppImage and the APK.
    const RELEASE: &str = r#"{"tag_name":"v0.4.1","assets":[
        {"name":"RustRadar_0.4.1_amd64.AppImage","browser_download_url":"https://x/a"},
        {"name":"RustRadar_0.4.1.apk","browser_download_url":"https://x/b.apk"}]}"#;

    /// A notifier on a queue of its own, and how often it woke the UI.
    fn notifier() -> (Notifier, mpsc::Receiver<BackendEvent>, Arc<AtomicUsize>) {
        let (tx, rx) = mpsc::channel();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = wakes.clone();
        let wake = move || {
            counted.fetch_add(1, Ordering::SeqCst);
        };
        (Notifier::new(tx, Arc::new(wake)), rx, wakes)
    }

    /// The URLs a [`recorder`] was given.
    type Opened = Arc<Mutex<Vec<String>>>;

    /// An opener that records the URLs it was given.
    fn recorder() -> (Opener, Opened) {
        let opened = Opened::default();
        let into = opened.clone();
        let open = move |url: &str| {
            into.lock().unwrap().push(url.to_owned());
            Ok(())
        };
        (Box::new(open), opened)
    }

    #[test]
    fn finds_a_newer_apk() {
        // the backend's reading of a release, unchanged
        assert_eq!(newer_apk(RELEASE, "0.4.0"), Some(("0.4.1".into(), "https://x/b.apk".into())));
        assert_eq!(newer_apk(RELEASE, "0.4.1"), None);
        assert_eq!(newer_apk(RELEASE, "0.10.0"), None);
        assert_eq!(newer_apk(r#"{"tag_name":"v0.5.0","assets":[]}"#, "0.4.0"), None);

        // the app's part: an older or the same release, or one without an APK, is nothing
        let (notifier, events, wakes) = notifier();
        let ready = Ready::default();
        for current in ["0.4.1", "0.10.0"] {
            assert_eq!(found(&ready, &notifier, Ok(newer_apk(RELEASE, current))), Ok(()));
        }
        assert_eq!(found(&ready, &notifier, Ok(newer_apk(r#"{"tag_name":"v0.5.0","assets":[]}"#, "0.4.0"))), Ok(()));
        assert_eq!(events.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        assert_eq!(ready.pending_update(), None);
        let (open, opened) = recorder();
        assert_eq!(ready.restart_to_update(open), Err(NOTHING_WAITING.into()));
        assert!(opened.lock().unwrap().is_empty());

        // a newer one is remembered, announced and opened
        assert_eq!(found(&ready, &notifier, Ok(newer_apk(RELEASE, "0.4.0"))), Ok(()));
        assert_eq!(events.try_recv(), Ok(BackendEvent::UpdateReady { version: "0.4.1".into() }));
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "the UI is woken to show it");
        assert_eq!(ready.pending_update().as_deref(), Some("0.4.1"));
        let (open, opened) = recorder();
        assert_eq!(ready.restart_to_update(open), Ok(()));
        assert_eq!(*opened.lock().unwrap(), ["https://x/b.apk"]);

        // a failed request is handed back and changes nothing
        assert_eq!(found(&ready, &notifier, Err("offline".into())), Err("offline".into()));
        assert_eq!(events.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(ready.pending_update().as_deref(), Some("0.4.1"));
    }

    #[test]
    fn a_later_release_replaces_the_one_found() {
        let (notifier, events, _) = notifier();
        let ready = Ready::default();
        found(&ready, &notifier, Ok(Some(("0.4.1".into(), "https://x/b.apk".into())))).unwrap();
        found(&ready, &notifier, Ok(Some(("0.4.2".into(), "https://x/c.apk".into())))).unwrap();
        let versions: Vec<BackendEvent> = events.try_iter().collect();
        assert_eq!(
            versions,
            [BackendEvent::UpdateReady { version: "0.4.1".into() }, BackendEvent::UpdateReady { version: "0.4.2".into() }]
        );
        assert_eq!(ready.pending_update().as_deref(), Some("0.4.2"));
        let (open, opened) = recorder();
        ready.restart_to_update(open).unwrap();
        assert_eq!(*opened.lock().unwrap(), ["https://x/c.apk"]);
    }

    #[test]
    fn checks_now_and_again_after_a_failure() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let (notifier, events, _) = notifier();
        let ready = Ready::default();
        let asked = Arc::new(AtomicUsize::new(0));
        let count = asked.clone();
        // the first check fails (offline), the second finds the release, later ones find it again
        let task = rt.spawn(watch(ready.clone(), notifier, Duration::from_millis(20), move || {
            let n = count.fetch_add(1, Ordering::SeqCst);
            async move { if n == 0 { Err("offline".into()) } else { Ok(newer_apk(RELEASE, "0.4.0")) } }
        }));
        rt.block_on(async {
            for _ in 0..10 {
                if asked.load(Ordering::SeqCst) > 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(asked.load(Ordering::SeqCst), 1, "the first check runs at once");
            assert_eq!(ready.pending_update(), None);
            while asked.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        task.abort();
        assert_eq!(events.try_recv(), Ok(BackendEvent::UpdateReady { version: "0.4.1".into() }));
        assert_eq!(ready.pending_update().as_deref(), Some("0.4.1"));
        assert_eq!(rt.block_on(task).map_err(|e| e.is_cancelled()), Err(true), "it runs until aborted");
    }

    #[test]
    fn the_toast_downloads_the_apk() {
        let ctx = egui::Context::default();
        let ready = Ready::default();
        let (open, opened) = recorder();
        toast::install_updater(&ctx, Box::new(Download::with_opener(ready.clone(), open)));
        let state = toast::toast(&ctx);
        let mut t = state.lock().unwrap();
        // before a release was found, the button says so in the sub line
        t.restart(true);
        assert!(!t.poll());
        assert_eq!(t.sub(true), NOTHING_WAITING);
        assert!(opened.lock().unwrap().is_empty());
        // then it opens the APK and the toast stays as it is
        ready.set("0.4.1".into(), "https://x/b.apk".into());
        t.restart(true);
        assert!(!t.poll());
        assert_eq!((t.error.as_deref(), t.restarting), (None, false));
        assert_eq!(t.sub(true), toast::SUB_ANDROID);
        assert_eq!(*opened.lock().unwrap(), ["https://x/b.apk"]);
    }

    #[test]
    fn a_failed_open_is_shown() {
        let ready = Ready::default();
        ready.set("0.4.1".into(), "https://x/b.apk".into());
        let mut download = Download::with_opener(ready, |url: &str| Err(format!("Couldn't open {url}: no browser")));
        let answer = toast::Updater::restart_to_update(&mut download).poll();
        assert_eq!(answer, Some(Err("Couldn't open https://x/b.apk: no browser".into())));
    }

    #[test]
    fn debug_builds_and_ios_do_not_check() {
        assert_eq!(CHECKS, !cfg!(debug_assertions) && !cfg!(target_os = "ios"));
        let (notifier, _, _) = notifier();
        assert!(!start(None, notifier), "nothing started: a debug build, or no backend");
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert_eq!(LATEST, "https://api.github.com/repos/felixgoff/RustRadar/releases/latest");
        assert_eq!(CHECK_EVERY, Duration::from_secs(3600));
    }
}
