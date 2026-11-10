//! RustRadar's backend without Tauri: HTTP, caches, feeds and streams.
//!
//! The modules move here from `src-tauri/src` in S1.2 and S1.3 (plan 1.2, doc 07 §10). This file holds
//! what replaces the Tauri facilities they leaned on: the folders ([`Dirs`]), events to the UI
//! ([`BackendEvent`], [`Notifier`]), ordered streams to one caller ([`Following`]) and the runtime
//! background work is spawned on ([`rt`]).

pub mod adsblol;
pub mod board;
pub mod building_lod;
pub mod devlog;
pub mod dwd;
pub mod flow;
pub mod geocode;
pub mod lines;
pub mod liveries;
pub mod mrms;
pub mod opensky;
pub mod osm_platforms;
pub mod radar;
pub mod rail;
pub mod rail_motion;
pub mod reference;
pub mod runways;
pub mod session;
pub mod ships;
pub mod sth;
pub mod train_class;
pub mod train_media;
pub mod weather;

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::AbortHandle;

/// What every request answers with: the UI prints the error's text as it is.
pub type CmdResult<T> = Result<T, String>;

/// Whether the track, signal and platform stores stay off the network and only answer from what they
/// hold: in this crate's tests, and in the tests of a crate that asks for the `test-support` feature.
pub(crate) const OFFLINE: bool = cfg!(any(test, feature = "test-support"));

/// The app's identifier: its folders are named after it, as Tauri named them.
pub const APP_ID: &str = "com.felix.rustradar";

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or_default()
}

/// The folders the backend reads and writes, resolved once by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    /// Everything that can be fetched again: reference data, tiles, colours.
    pub cache: PathBuf,
    /// What the user entered: `keys.json`.
    pub config: PathBuf,
    /// Where exports are saved; none on phones.
    pub downloads: Option<PathBuf>,
}

impl Dirs {
    /// The folders of a desktop install, the same ones the Tauri build used, so the keys file and
    /// every cache carry over. None when the system names no cache or config folder (and on phones,
    /// which have their own).
    pub fn desktop() -> Option<Self> {
        Some(Self {
            cache: dirs::cache_dir()?.join(APP_ID),
            config: dirs::config_dir()?.join(APP_ID),
            downloads: dirs::download_dir().or_else(dirs::home_dir),
        })
    }

    /// A file in the cache folder.
    pub fn cache_file(&self, name: &str) -> PathBuf {
        self.cache.join(name)
    }

    /// A file in the config folder.
    pub fn config_file(&self, name: &str) -> PathBuf {
        self.config.join(name)
    }

    /// Tells the modules that keep files between runs where to: the rail network's tiles (and beside
    /// them the signals'), the far-off buildings and the stations' platforms. Only the first call counts.
    pub fn share_cache(&self) {
        osm_platforms::set_cache_dir(self.cache.clone());
        building_lod::set_cache_dir(self.cache.clone());
        rail::set_cache_dir(self.cache.clone());
    }
}

/// Something background work tells the UI without having been asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    /// A live stream moved trains in the cache: ask `trains_in_view` again (throttle 1500 ms).
    TrainsChanged,
    /// A newer version is ready: downloaded (desktop) or downloadable (Android).
    UpdateReady { version: String },
}

/// Called after every event or finished request, so the UI thread draws again.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// How background work reaches the UI: an event on the queue, then a wake. Cheap to clone.
#[derive(Clone)]
pub struct Notifier {
    events: std::sync::mpsc::Sender<BackendEvent>,
    wake: Wake,
}

impl Notifier {
    pub fn new(events: std::sync::mpsc::Sender<BackendEvent>, wake: Wake) -> Self {
        Self { events, wake }
    }

    /// Queues `event` and wakes the UI. False once the UI's end of the queue is gone.
    pub fn notify(&self, event: BackendEvent) -> bool {
        let sent = self.events.send(event).is_ok();
        (self.wake)();
        sent
    }

    /// Wakes the UI without an event: a request finished, a stream delivered.
    pub fn wake(&self) {
        (self.wake)();
    }

    /// The wake on its own, for a [`Following`].
    pub fn waker(&self) -> Wake {
        self.wake.clone()
    }
}

/// The sending end of a [`Following`]: what a `tauri::ipc::Channel` was to the stream's task.
pub struct FollowSender<T> {
    tx: UnboundedSender<T>,
    wake: Wake,
}

impl<T> Clone for FollowSender<T> {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone(), wake: self.wake.clone() }
    }
}

impl<T> FollowSender<T> {
    /// Sends `event` and wakes the UI. An error (with the event back) once the receiver is gone,
    /// which is when the task should end.
    pub fn send(&self, event: T) -> Result<(), T> {
        let sent = self.tx.send(event).map_err(|e| e.0);
        if sent.is_ok() {
            (self.wake)();
        }
        sent
    }

    /// Whether the receiver is gone.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// A running stream: events arrive in order; dropping it (or calling [`Following::stop`]) aborts
/// the task.
pub struct Following<T> {
    pub rx: UnboundedReceiver<T>,
    task: AbortHandle,
}

impl<T: Send + 'static> Following<T> {
    /// Starts `task` on the backend's runtime ([`rt`]), handing it the sending end. `wake` is called
    /// after every event sent.
    pub fn spawn<F, Fut>(wake: Wake, task: F) -> Self
    where
        F: FnOnce(FollowSender<T>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self::spawn_on(rt(), wake, task)
    }

    /// As [`Following::spawn`], on a runtime of the caller's choosing.
    pub fn spawn_on<F, Fut>(rt: &Handle, wake: Wake, task: F) -> Self
    where
        F: FnOnce(FollowSender<T>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = unbounded_channel();
        let task = rt.spawn(task(FollowSender { tx, wake })).abort_handle();
        Self { rx, task }
    }
}

impl<T> Following<T> {
    /// The next event if one is waiting: what the UI drains each frame.
    pub fn try_recv(&mut self) -> Option<T> {
        self.rx.try_recv().ok()
    }

    /// The handle the task can be aborted with by whoever replaces this stream.
    pub fn abort_handle(&self) -> AbortHandle {
        self.task.clone()
    }

    /// Whether the task has ended, by itself or aborted.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Ends the stream.
    pub fn stop(self) {}
}

impl<T> Drop for Following<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn rt_cell() -> &'static OnceLock<Handle> {
    static RT: OnceLock<Handle> = OnceLock::new();
    &RT
}

/// Sets the runtime background work is spawned on. Call it once at start, before anything else in
/// this crate; a second call is ignored (false).
pub fn set_rt(handle: Handle) -> bool {
    rt_cell().set(handle).is_ok()
}

/// The runtime background work is spawned on, from any thread: several spawns happen in synchronous
/// functions (`rail::tracks_of`, `osm_platforms::near`, the signal tiles), which the UI thread may
/// call, and `tokio::spawn` panics outside a runtime.
///
/// Panics when [`set_rt`] was never called and the caller isn't inside a runtime either (inside
/// one, that runtime is taken, which is what a `#[tokio::test]` gets).
pub fn rt() -> &'static Handle {
    rt_cell().get_or_init(|| {
        Handle::try_current().expect("rustradar_backend::set_rt must be called before background work starts")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn counter() -> (Arc<AtomicUsize>, Wake) {
        let n = Arc::new(AtomicUsize::new(0));
        let m = n.clone();
        (n, Arc::new(move || {
            m.fetch_add(1, Ordering::SeqCst);
        }))
    }

    #[test]
    fn a_notifier_queues_the_event_and_wakes_the_ui() {
        let (woken, wake) = counter();
        let (tx, rx) = std::sync::mpsc::channel();
        let notifier = Notifier::new(tx, wake);
        assert!(notifier.clone().notify(BackendEvent::TrainsChanged));
        assert!(notifier.notify(BackendEvent::UpdateReady { version: "1.2.3".into() }));
        assert_eq!(rx.try_recv(), Ok(BackendEvent::TrainsChanged));
        assert_eq!(rx.try_recv(), Ok(BackendEvent::UpdateReady { version: "1.2.3".into() }));
        assert_eq!(woken.load(Ordering::SeqCst), 2);
        notifier.wake();
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        // the UI gone: nothing to queue on, said so
        drop(rx);
        assert!(!notifier.notify(BackendEvent::TrainsChanged));
    }

    #[tokio::test]
    async fn a_following_delivers_in_order_and_wakes_after_each() {
        let (woken, wake) = counter();
        let mut following = Following::spawn_on(&Handle::current(), wake, |tx| async move {
            for i in 0..3 {
                tx.send(i).unwrap();
            }
        });
        let mut got = Vec::new();
        while let Some(v) = following.rx.recv().await {
            got.push(v);
        }
        assert_eq!(got, [0, 1, 2]);
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        assert_eq!(following.try_recv(), None);
    }

    #[tokio::test]
    async fn dropping_a_following_aborts_its_task() {
        let (_, wake) = counter();
        let (ended_tx, ended_rx) = tokio::sync::oneshot::channel::<()>();
        let following = Following::<u32>::spawn_on(&Handle::current(), wake, |tx| async move {
            // held until the task is dropped, which closes the other end
            let _held = ended_tx;
            tx.send(1).unwrap();
            std::future::pending::<()>().await;
        });
        let task = following.abort_handle();
        drop(following);
        tokio::time::timeout(Duration::from_secs(5), ended_rx).await.expect("the task was aborted").unwrap_err();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn sending_fails_once_the_receiver_is_gone() {
        let (woken, wake) = counter();
        let (tx, rx) = unbounded_channel();
        let sender = FollowSender { tx, wake };
        assert_eq!(sender.send(1), Ok(()));
        drop(rx);
        assert!(sender.is_closed());
        assert_eq!(sender.send(2), Err(2));
        assert_eq!(woken.load(Ordering::SeqCst), 1, "no wake for an event nobody gets");
    }

    #[test]
    fn desktop_folders_are_named_after_the_app() {
        // CI containers may have no home at all: then there is nothing to check
        let Some(dirs) = Dirs::desktop() else { return };
        assert!(dirs.cache.ends_with(APP_ID) && dirs.config.ends_with(APP_ID));
        assert_eq!(dirs.cache_file("reference.json"), dirs.cache.join("reference.json"));
        assert_eq!(dirs.config_file("keys.json"), dirs.config.join("keys.json"));
    }

    #[tokio::test]
    async fn the_runtime_is_the_callers_until_one_is_set() {
        // no test sets one, so the first use inside a runtime takes that runtime
        let n = rt().spawn(async { 7 }).await.unwrap();
        assert_eq!(n, 7);
    }
}
