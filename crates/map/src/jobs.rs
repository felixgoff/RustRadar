//! The worker pool (plan 2.1): plain threads for the CPU work that must not run on the main thread
//! (MVT decoding and tessellation, radar contouring, hillshade, tree planting, RTIN meshing, KTX2
//! transcoding, image decoding).
//!
//! - [`Pool::spawn`] queues a job with a priority; a free worker takes the queued job with the
//!   lowest priority value first (as loaders.gl's `RequestScheduler`, which the tile runtime's
//!   priorities come from: lower loads first), and among equal ones the one queued first.
//! - Every job has a [`Cancel`] flag. A job cancelled while it waits is dropped without running; a
//!   running job can read the flag and stop early. Dropping the [`Job`] handle cancels it: a result
//!   nobody holds is not worth computing.
//! - The result comes back over an `std::sync::mpsc` channel ([`Job::poll`] each frame, or
//!   [`Job::wait`]); after each job the pool calls its [`Wake`], so the app's event loop wakes up
//!   to collect it. The wake can be set after the pool is made ([`Pool::set_wake`]: the map's
//!   renderer makes its pool before the app hands it the event loop's wake).
//! - The pool has [`default_threads`] workers on the app: `clamp(cores − 2, 2, 6)`, which leaves a
//!   core to the main thread and one to the tokio runtime.
//!
//! No `rayon` (plan 1.3): its jobs have neither priorities nor cancellation.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// What wakes the main thread when work it waits for has finished (the app's event-loop proxy,
/// plan 2.1). Called from worker and runtime threads.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A cancel flag shared by whoever started some work and the work itself (a job, a fetch).
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the work to stop; a job still queued is dropped without running.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// The pool's size for a machine with `cores` hardware threads: `clamp(cores − 2, 2, 6)`.
pub fn threads_for(cores: usize) -> usize {
    cores.saturating_sub(2).clamp(2, 6)
}

/// The pool's size on this machine ([`threads_for`] its available parallelism; 4 cores when that
/// is unknown).
pub fn default_threads() -> usize {
    threads_for(std::thread::available_parallelism().map_or(4, |n| n.get()))
}

/// A queued job.
struct Entry {
    priority: f64,
    seq: u64,
    cancel: Cancel,
    run: Box<dyn FnOnce() + Send>,
}

#[derive(Default)]
struct State {
    queue: Vec<Entry>,
    seq: u64,
    running: usize,
    shutdown: bool,
}

impl State {
    /// Whether anything is left to run: a job running or one queued and not cancelled.
    fn busy(&self) -> bool {
        self.running > 0 || self.queue.iter().any(|e| !e.cancel.is_cancelled())
    }
}

struct Shared {
    state: Mutex<State>,
    /// Workers wait here for jobs.
    work: Condvar,
    /// [`Pool::wait_idle`] waits here.
    idle: Condvar,
    wake: Mutex<Option<Wake>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // jobs run outside the lock, so it is never poisoned by one; recover anyway
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The worker pool. Dropping it stops the workers once their running jobs return; queued jobs are
/// dropped (their handles read [`JobPoll::Cancelled`]).
pub struct Pool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool").field("threads", &self.threads.len()).finish_non_exhaustive()
    }
}

impl Pool {
    /// A pool of `threads` workers (at least one) that wakes nobody.
    pub fn new(threads: usize) -> Self {
        Self::with_wake(threads, None)
    }

    /// A pool of `threads` workers (at least one) that calls `wake` after every job.
    pub fn with_wake(threads: usize, wake: Option<Wake>) -> Self {
        let shared = Arc::new(Shared { state: Mutex::default(), work: Condvar::new(), idle: Condvar::new(), wake: Mutex::new(wake) });
        let threads = (0..threads.max(1))
            .map(|i| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("rustradar-job-{i}"))
                    .spawn(move || worker(&shared))
                    .expect("spawn a worker thread")
            })
            .collect();
        Self { shared, threads }
    }

    /// Queues `job`; lower `priority` values run first, equal ones in the order queued. The job
    /// gets its own cancel flag.
    pub fn spawn<R, F>(&self, priority: f64, job: F) -> Job<R>
    where
        R: Send + 'static,
        F: FnOnce(&Cancel) -> R + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let cancel = Cancel::new();
        let flag = cancel.clone();
        let run = Box::new(move || {
            // (a receiver that is gone wants nothing)
            let _ = tx.send(job(&flag));
        });
        let mut state = self.shared.lock();
        let seq = state.seq;
        state.seq += 1;
        state.queue.push(Entry { priority, seq, cancel: cancel.clone(), run });
        drop(state);
        self.shared.work.notify_one();
        Job { result: rx, cancel }
    }

    /// Calls `wake` (none: nobody) after every job from now on.
    pub fn set_wake(&self, wake: Option<Wake>) {
        *self.shared.wake.lock().unwrap_or_else(PoisonError::into_inner) = wake;
    }

    /// The number of workers.
    pub fn threads(&self) -> usize {
        self.threads.len()
    }

    /// Jobs waiting for a worker (not counting cancelled ones).
    pub fn queued(&self) -> usize {
        self.shared.lock().queue.iter().filter(|e| !e.cancel.is_cancelled()).count()
    }

    /// Jobs running now.
    pub fn running(&self) -> usize {
        self.shared.lock().running
    }

    /// Blocks until no job runs and none waits (headless rendering, tests). Jobs cancelled while
    /// queued are dropped by then, so their handles read [`JobPoll::Cancelled`].
    pub fn wait_idle(&self) {
        let mut state = self.shared.lock();
        loop {
            state.queue.retain(|e| !e.cancel.is_cancelled());
            if !state.busy() {
                return;
            }
            // (a job cancelled while queued notifies nobody: look again now and then)
            state = self.shared.idle.wait_timeout(state, Duration::from_millis(10)).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.work.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        // what never ran goes now, so its handles see the channel closed
        self.shared.lock().queue.clear();
        self.shared.idle.notify_all();
    }
}

fn worker(shared: &Shared) {
    loop {
        let entry = {
            let mut state = shared.lock();
            loop {
                if state.shutdown {
                    return;
                }
                // a cancelled job never runs: dropping it closes its channel
                state.queue.retain(|e| !e.cancel.is_cancelled());
                let best = state
                    .queue
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.priority.total_cmp(&b.priority).then(a.seq.cmp(&b.seq)))
                    .map(|(i, _)| i);
                if let Some(i) = best {
                    state.running += 1;
                    break state.queue.swap_remove(i);
                }
                if !state.busy() {
                    shared.idle.notify_all();
                }
                state = shared.work.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
        };
        // a job that panics loses its result (its handle reads `Lost`), not the worker
        let _ = catch_unwind(AssertUnwindSafe(entry.run));
        let mut state = shared.lock();
        state.running -= 1;
        let busy = state.busy();
        drop(state);
        if !busy {
            shared.idle.notify_all();
        }
        // (called outside the lock: a wake may take its time)
        let wake = shared.wake.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(wake) = wake {
            wake();
        }
    }
}

/// What a job's handle holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobPoll<R> {
    /// Queued or running.
    Pending,
    /// Its result (also when it was cancelled while running and returned anyway). Polling again
    /// afterwards reads `Cancelled` or `Lost`: the result is taken once.
    Done(R),
    /// Cancelled before it gave a result.
    Cancelled,
    /// It panicked (or its pool was dropped before it ran) without being cancelled.
    Lost,
}

/// A queued or running job's handle. Dropping it cancels the job.
#[derive(Debug)]
pub struct Job<R> {
    result: Receiver<R>,
    cancel: Cancel,
}

impl<R> Job<R> {
    /// The result if it is there, without blocking.
    pub fn poll(&mut self) -> JobPoll<R> {
        match self.result.try_recv() {
            Ok(r) => JobPoll::Done(r),
            Err(TryRecvError::Empty) => JobPoll::Pending,
            Err(TryRecvError::Disconnected) => self.gone(),
        }
    }

    /// Blocks until the job has finished (or was dropped unrun).
    pub fn wait(&mut self) -> JobPoll<R> {
        match self.result.recv() {
            Ok(r) => JobPoll::Done(r),
            Err(_) => self.gone(),
        }
    }

    fn gone(&self) -> JobPoll<R> {
        if self.cancel.is_cancelled() { JobPoll::Cancelled } else { JobPoll::Lost }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl<R> Drop for Job<R> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A job that holds the pool's only worker until `release` is sent.
    fn hold(pool: &Pool) -> (mpsc::Sender<()>, Job<()>) {
        let (release, wait) = mpsc::channel::<()>();
        let (started, running) = mpsc::channel::<()>();
        let job = pool.spawn(f64::NEG_INFINITY, move |_| {
            started.send(()).unwrap();
            let _ = wait.recv();
        });
        running.recv().unwrap();
        (release, job)
    }

    #[test]
    fn threads_are_cores_less_two_from_two_to_six() {
        assert_eq!(threads_for(0), 2);
        assert_eq!(threads_for(1), 2);
        assert_eq!(threads_for(4), 2);
        assert_eq!(threads_for(5), 3);
        assert_eq!(threads_for(8), 6);
        assert_eq!(threads_for(64), 6);
        assert!((2..=6).contains(&default_threads()));
        assert_eq!(Pool::new(0).threads(), 1);
    }

    #[test]
    fn the_lowest_priority_runs_first_and_ties_in_order() {
        let pool = Pool::new(1);
        let (release, _held) = hold(&pool);
        let order = Arc::new(Mutex::new(Vec::new()));
        let jobs: Vec<_> = [(5.0, "five"), (1.0, "one"), (3.0, "three"), (1.0, "one again"), (-2.0, "minus two")]
            .into_iter()
            .map(|(priority, name)| {
                let order = Arc::clone(&order);
                pool.spawn(priority, move |_| order.lock().unwrap().push(name))
            })
            .collect();
        assert_eq!(pool.queued(), 5);
        release.send(()).unwrap();
        pool.wait_idle();
        assert_eq!(*order.lock().unwrap(), ["minus two", "one", "one again", "three", "five"]);
        for mut job in jobs {
            assert_eq!(job.poll(), JobPoll::Done(()));
        }
    }

    #[test]
    fn a_job_cancelled_while_queued_never_runs() {
        let pool = Pool::new(1);
        let (release, _held) = hold(&pool);
        let ran = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&ran);
        let mut job = pool.spawn(0.0, move |_| counted.fetch_add(1, Ordering::SeqCst));
        assert_eq!(job.poll(), JobPoll::Pending);
        job.cancel();
        assert!(job.is_cancelled());
        assert_eq!(pool.queued(), 0);
        release.send(()).unwrap();
        pool.wait_idle();
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        assert_eq!(job.poll(), JobPoll::Cancelled);
    }

    #[test]
    fn dropping_a_handle_cancels_its_job() {
        let pool = Pool::new(1);
        let (release, _held) = hold(&pool);
        let ran = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&ran);
        drop(pool.spawn(0.0, move |_| counted.fetch_add(1, Ordering::SeqCst)));
        let counted = Arc::clone(&ran);
        let mut kept = pool.spawn(1.0, move |_| counted.fetch_add(10, Ordering::SeqCst));
        release.send(()).unwrap();
        pool.wait_idle();
        assert_eq!(ran.load(Ordering::SeqCst), 10);
        assert_eq!(kept.poll(), JobPoll::Done(0));
    }

    #[test]
    fn a_running_job_reads_its_cancel_flag() {
        let pool = Pool::new(2);
        let (started, running) = mpsc::channel();
        let mut job = pool.spawn(0.0, move |cancel| {
            started.send(()).unwrap();
            let mut spins = 0u64;
            while !cancel.is_cancelled() {
                spins += 1;
                std::thread::yield_now();
            }
            spins
        });
        running.recv().unwrap();
        job.cancel();
        // it stopped because of the flag and still handed back what it had
        assert!(matches!(job.wait(), JobPoll::Done(_)));
    }

    #[test]
    fn results_come_back_off_the_calling_thread_and_wake_the_main_thread() {
        let woken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&woken);
        let pool = Pool::with_wake(2, Some(Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        })));
        let main = std::thread::current().id();
        let mut jobs: Vec<_> = (0..5).map(|i| pool.spawn(f64::from(i), move |_| (i, std::thread::current().id()))).collect();
        pool.wait_idle();
        for (i, job) in jobs.iter_mut().enumerate() {
            match job.poll() {
                JobPoll::Done((n, thread)) => {
                    assert_eq!(n as usize, i);
                    assert_ne!(thread, main);
                }
                other => panic!("job {i}: {other:?}"),
            }
        }
        // (the wake follows the result: wait for the last one)
        for _ in 0..500 {
            if woken.load(Ordering::SeqCst) == 5 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(woken.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn a_wake_set_later_is_called_after_the_next_jobs() {
        let pool = Pool::new(1);
        let mut first = pool.spawn(0.0, |_| 1);
        assert_eq!(first.wait(), JobPoll::Done(1));
        pool.wait_idle();
        let woken = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&woken);
        pool.set_wake(Some(Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        })));
        let mut second = pool.spawn(0.0, |_| 2);
        assert_eq!(second.wait(), JobPoll::Done(2));
        for _ in 0..500 {
            if woken.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(woken.load(Ordering::SeqCst), 1, "the job before the wake was set woke nobody");
        pool.set_wake(None);
        let mut third = pool.spawn(0.0, |_| 3);
        assert_eq!(third.wait(), JobPoll::Done(3));
        pool.wait_idle();
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panicking_job_is_reported_and_the_pool_keeps_working() {
        let pool = Pool::new(1);
        let mut bad = pool.spawn(0.0, |_| -> u32 { panic!("a job that fails") });
        let mut good = pool.spawn(1.0, |_| 7u32);
        assert_eq!(bad.wait(), JobPoll::Lost);
        assert_eq!(good.wait(), JobPoll::Done(7));
    }

    #[test]
    fn dropping_the_pool_drops_what_never_ran() {
        let pool = Pool::new(1);
        let (release, _held) = hold(&pool);
        let mut queued = pool.spawn(0.0, |_| 1);
        // let the held job finish while the pool shuts down
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let _ = release.send(());
        });
        drop(pool);
        releaser.join().unwrap();
        // shut down before it ran: closed without a result, and nobody cancelled it
        assert!(matches!(queued.poll(), JobPoll::Lost | JobPoll::Done(1)));
    }
}
