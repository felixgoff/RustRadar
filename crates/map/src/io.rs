//! Where tile bytes come from (plan 1.1, 1.4.8): `map` never depends on `backend`. A tile set asks
//! a [`TileFetcher`] for a tile's bytes; `app` implements it on its tokio runtime with
//! `backend::tiles` (disk cache, freshness, per-host limits), and headless tests with files on
//! disk.
//!
//! The contract:
//! - [`TileFetcher::fetch`] returns at once; the answer arrives later on the receiver it hands
//!   back, from any thread, and whoever sends it also wakes the main thread (plan 2.1) so the tile
//!   runtime collects it in its next frame.
//! - The answer is `Ok(Some(bytes))` for a tile, `Ok(None)` when the source has nothing there (an
//!   HTTP 404 or 204: an expected hole, settled and not asked again), or `Err` when the request
//!   failed.
//! - The request carries a [`Cancel`] flag: once it is set, nobody waits for the answer, and the
//!   fetcher may stop early and drop the sender without answering.
//!
//! [`Fetch`] is a request in flight as the tile runtime holds it; [`FnFetcher`] answers from a
//! function on the calling thread.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use rustradar_scene::tiles::TileIndex;

use crate::jobs::Cancel;

/// Why a fetch gave no bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The request failed (no connection, a server error, a broken file); the text says how.
    Failed(String),
    /// The request was cancelled, or the fetcher dropped it without an answer.
    Cancelled,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Failed(why) => write!(f, "tile request failed: {why}"),
            FetchError::Cancelled => f.write_str("tile request cancelled"),
        }
    }
}

impl std::error::Error for FetchError {}

/// A fetch's answer: the bytes, `None` for nothing there (404), or why there are none.
pub type FetchResult = Result<Option<Vec<u8>>, FetchError>;

/// One tile asked of a source.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// The source's name as the layer knows it (`"openfreemap"`, `"esri"`, …): the fetcher maps it
    /// to a URL template and a cache folder.
    pub source: Arc<str>,
    pub tile: TileIndex,
    /// Set once nobody waits for the answer.
    pub cancel: Cancel,
}

/// The tile runtime's way to the network or the disk.
pub trait TileFetcher: Send + Sync {
    /// Starts fetching `request.tile` from `request.source` and returns at once; the answer comes
    /// on the receiver (see the module's contract).
    fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult>;
}

/// A receiver that already holds `result` (for fetchers that answer at once).
pub fn answered(result: FetchResult) -> Receiver<FetchResult> {
    let (tx, rx) = mpsc::channel();
    // (the receiver is right here: the send cannot fail)
    let _ = tx.send(result);
    rx
}

/// A fetcher that answers at once, on the calling thread, with what a function returns for the
/// source's name and the tile (files on disk, tests).
pub struct FnFetcher<F>(pub F);

impl<F> TileFetcher for FnFetcher<F>
where
    F: Fn(&str, TileIndex) -> FetchResult + Send + Sync,
{
    fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
        answered((self.0)(&request.source, request.tile))
    }
}

/// A request in flight. Dropping it cancels the request.
#[derive(Debug)]
pub struct Fetch {
    answer: Receiver<FetchResult>,
    cancel: Cancel,
}

impl Fetch {
    /// Asks `fetcher` for `tile` from `source`.
    pub fn start(fetcher: &dyn TileFetcher, source: Arc<str>, tile: TileIndex) -> Self {
        let cancel = Cancel::new();
        let answer = fetcher.fetch(FetchRequest { source, tile, cancel: cancel.clone() });
        Fetch { answer, cancel }
    }

    /// The answer if it has come, without blocking. A fetcher that dropped the request without an
    /// answer reads as [`FetchError::Cancelled`] once.
    pub fn poll(&mut self) -> Option<FetchResult> {
        match self.answer.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(FetchError::Cancelled)),
        }
    }

    /// Tells the fetcher nobody waits for the answer.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl Drop for Fetch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc::Sender;

    #[test]
    fn a_fn_fetcher_answers_at_once_with_the_source_and_tile() {
        let fetcher = FnFetcher(|source: &str, tile: TileIndex| match tile.z {
            0 => Ok(Some(format!("{source} {}", tile.id()).into_bytes())),
            1 => Ok(None),
            _ => Err(FetchError::Failed("no such file".into())),
        });
        let mut fetch = Fetch::start(&fetcher, "dark".into(), TileIndex::new(0, 0, 0));
        assert_eq!(fetch.poll(), Some(Ok(Some(b"dark 0-0-0".to_vec()))));
        let mut fetch = Fetch::start(&fetcher, "dark".into(), TileIndex::new(1, 0, 1));
        assert_eq!(fetch.poll(), Some(Ok(None)));
        let mut fetch = Fetch::start(&fetcher, "dark".into(), TileIndex::new(1, 0, 2));
        let answer = fetch.poll().unwrap();
        assert_eq!(answer, Err(FetchError::Failed("no such file".into())));
        assert_eq!(answer.unwrap_err().to_string(), "tile request failed: no such file");
    }

    /// Holds every request so a test answers (or drops) them itself.
    #[derive(Default)]
    struct Held(Mutex<Vec<(FetchRequest, Sender<FetchResult>)>>);

    impl TileFetcher for Held {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().push((request, tx));
            rx
        }
    }

    #[test]
    fn an_answer_arrives_later_from_another_thread() {
        let fetcher = Held::default();
        let mut fetch = Fetch::start(&fetcher, "esri".into(), TileIndex::new(3, 4, 5));
        assert_eq!(fetch.poll(), None);
        let (request, tx) = fetcher.0.lock().unwrap().pop().unwrap();
        assert_eq!((&*request.source, request.tile), ("esri", TileIndex::new(3, 4, 5)));
        std::thread::spawn(move || tx.send(Ok(Some(vec![1, 2, 3]))).unwrap()).join().unwrap();
        assert_eq!(fetch.poll(), Some(Ok(Some(vec![1, 2, 3]))));
    }

    #[test]
    fn cancelling_reaches_the_fetcher_and_a_dropped_request_reads_cancelled() {
        let fetcher = Held::default();
        let cancelled = Fetch::start(&fetcher, "esri".into(), TileIndex::new(0, 0, 1));
        let dropped = Fetch::start(&fetcher, "esri".into(), TileIndex::new(1, 0, 1));
        let mut abandoned = Fetch::start(&fetcher, "esri".into(), TileIndex::new(0, 1, 1));
        let mut requests = std::mem::take(&mut *fetcher.0.lock().unwrap());
        assert!(requests.iter().all(|(r, _)| !r.cancel.is_cancelled()));
        cancelled.cancel();
        assert!(cancelled.is_cancelled());
        assert!(requests[0].0.cancel.is_cancelled());
        // dropping the handle cancels too
        drop(dropped);
        assert!(requests[1].0.cancel.is_cancelled());
        // a fetcher that gives up on a request without answering
        assert_eq!(abandoned.poll(), None);
        requests.pop();
        assert_eq!(abandoned.poll(), Some(Err(FetchError::Cancelled)));
        assert!(!abandoned.is_cancelled());
    }
}
