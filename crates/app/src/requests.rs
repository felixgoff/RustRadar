//! Requests from the main thread to the backend (plan 2.1, doc 07 §10.4): the tokio runtime, the
//! wake that brings the main thread back, and [`Pending`], a request whose answer is polled.
//!
//! The main thread never awaits and never blocks on the network: it starts a future on the runtime
//! with [`Requests::request`], keeps the [`Pending`] and polls it each frame. When the answer
//! arrives the task wakes the main thread, which draws a frame and polls again. Where today's page
//! ignores a stale answer (a `gone` flag or a request counter in an `$effect`), drop the `Pending`
//! and make a new one: the old task still finishes, and its send fails silently.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};

use rustradar_backend::Wake;
use tokio::runtime::{Handle, Runtime};

/// Most worker threads the runtime gets (plan 2.1: `min(4, cores)`).
pub const MAX_RUNTIME_WORKERS: usize = 4;

/// The process's tokio runtime: multi-thread, `min(4, cores)` workers, made on the first call and
/// kept for the process's lifetime (doc 07 §11.4). Everything in `backend` runs on it.
pub fn runtime() -> Result<&'static Runtime, String> {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    if let Some(rt) = RUNTIME.get() {
        return Ok(rt);
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cores.min(MAX_RUNTIME_WORKERS))
        .thread_name("rustradar-rt")
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the async runtime: {e}"))?;
    // two threads racing here: the first one's runtime is kept, the other one is dropped unused
    Ok(RUNTIME.get_or_init(|| rt))
}

/// Coalesces wakes (plan 2.1): `wake()` sends one event to the main thread unless one is already
/// on its way, so a burst of answers costs one redraw. The main thread calls [`WakeLatch::reset`]
/// when the event arrives, before it polls, so a wake raised while it polls is sent again.
#[derive(Debug, Clone, Default)]
pub struct WakeLatch(Arc<AtomicBool>);

impl WakeLatch {
    /// The wake for the backend and every request: calls `send` (the event loop proxy's
    /// `send_event`) unless a wake is pending. `send` answers whether the event went out; when the
    /// event loop is gone the latch stays set and nothing more is sent.
    pub fn wake_with(&self, send: impl Fn() -> bool + Send + Sync + 'static) -> Wake {
        let pending = self.0.clone();
        Arc::new(move || {
            if !pending.swap(true, Ordering::AcqRel) && !send() {
                log::debug!("wake after the event loop ended");
            }
        })
    }

    /// The main thread took the wake: the next one is sent again.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Release);
    }

    /// Whether a wake has been sent and not yet taken.
    pub fn is_pending(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// A wake that does nothing (tests, the headless path, which draws its frames in a loop).
pub fn no_wake() -> Wake {
    Arc::new(|| {})
}

/// A request in flight: its answer, once, when it has arrived (doc 07 §10.4).
#[derive(Debug)]
pub struct Pending<T>(Receiver<T>);

impl<T> Pending<T> {
    /// The answer if it has arrived (it is handed out once; later polls give `None`).
    pub fn poll(&self) -> Option<T> {
        self.0.try_recv().ok()
    }

    /// A request already answered with `value` (fixtures, tests).
    pub fn ready(value: T) -> Self {
        let (tx, rx) = mpsc::channel();
        // the receiver is right here: the send cannot fail
        let _ = tx.send(value);
        Pending(rx)
    }
}

/// Starts `fut` on `rt` and returns at once; the answer is sent back, then `wake` brings the main
/// thread round to poll it (doc 07 §10.4 `request`).
pub fn request<T: Send + 'static>(rt: &Handle, wake: Wake, fut: impl Future<Output = T> + Send + 'static) -> Pending<T> {
    let (tx, rx) = mpsc::channel();
    rt.spawn(async move {
        // the Pending may have been dropped (a stale request): nobody waits, which is fine
        let _ = tx.send(fut.await);
        wake();
    });
    Pending(rx)
}

/// The runtime and the wake, as the state's feeds hold them to start requests. Cheap to clone.
#[derive(Clone)]
pub struct Requests {
    rt: Handle,
    wake: Wake,
}

impl std::fmt::Debug for Requests {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Requests").finish_non_exhaustive()
    }
}

impl Requests {
    pub fn new(rt: Handle, wake: Wake) -> Self {
        Requests { rt, wake }
    }

    /// Starts `fut` (see [`request`]).
    pub fn request<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> Pending<T> {
        request(&self.rt, self.wake.clone(), fut)
    }

    /// The runtime requests run on.
    pub fn rt(&self) -> &Handle {
        &self.rt
    }

    /// The main thread's wake, for streams started elsewhere.
    pub fn wake(&self) -> Wake {
        self.wake.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use super::*;

    /// Polls until an answer arrives or 5 s have passed.
    fn wait<T>(pending: &Pending<T>) -> Option<T> {
        let started = Instant::now();
        loop {
            if let Some(v) = pending.poll() {
                return Some(v);
            }
            if started.elapsed() > Duration::from_secs(5) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn the_runtime_is_made_once_with_at_most_four_workers() {
        let a = runtime().expect("runtime");
        let b = runtime().expect("runtime");
        assert!(std::ptr::eq(a, b));
        let workers = a.metrics().num_workers();
        assert!((1..=MAX_RUNTIME_WORKERS).contains(&workers), "{workers} workers");
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        assert_eq!(workers, cores.min(4));
    }

    #[test]
    fn a_request_answers_once_and_wakes_after_its_answer() {
        let rt = runtime().expect("runtime");
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        let requests = Requests::new(rt.handle().clone(), Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        let pending = requests.request(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            42
        });
        // not there yet: the main thread does not wait
        assert_eq!(pending.poll(), None);
        assert_eq!(wait(&pending), Some(42));
        // the wake comes right after the send
        let started = Instant::now();
        while woken.load(Ordering::SeqCst) == 0 && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(woken.load(Ordering::SeqCst), 1);
        assert_eq!(pending.poll(), None, "an answer is handed out once");
    }

    #[test]
    fn a_dropped_request_still_runs_to_its_end() {
        let rt = runtime().expect("runtime");
        let done = Arc::new(AtomicBool::new(false));
        let d = done.clone();
        drop(request(rt.handle(), no_wake(), async move {
            d.store(true, Ordering::SeqCst);
        }));
        let started = Instant::now();
        while !done.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(done.load(Ordering::SeqCst));
    }

    #[test]
    fn ready_answers_at_once() {
        let pending = Pending::ready("fixture");
        assert_eq!(pending.poll(), Some("fixture"));
        assert_eq!(pending.poll(), None);
    }

    #[test]
    fn wakes_are_coalesced_until_the_main_thread_takes_one() {
        let latch = WakeLatch::default();
        let sent = Arc::new(AtomicUsize::new(0));
        let s = sent.clone();
        let wake = latch.wake_with(move || {
            s.fetch_add(1, Ordering::SeqCst);
            true
        });
        assert!(!latch.is_pending());
        wake();
        wake();
        wake();
        assert_eq!(sent.load(Ordering::SeqCst), 1, "one event for a burst");
        assert!(latch.is_pending());
        latch.reset();
        wake();
        assert_eq!(sent.load(Ordering::SeqCst), 2, "sent again once taken");
    }

    #[test]
    fn a_closed_event_loop_is_not_sent_to_again() {
        let latch = WakeLatch::default();
        let tries = Arc::new(AtomicUsize::new(0));
        let t = tries.clone();
        let wake = latch.wake_with(move || {
            t.fetch_add(1, Ordering::SeqCst);
            false
        });
        wake();
        wake();
        assert_eq!(tries.load(Ordering::SeqCst), 1);
    }
}
