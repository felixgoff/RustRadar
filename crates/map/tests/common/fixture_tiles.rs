//! The tile fixtures of `tests/data/tiles` (plan E16) as a [`TileFetcher`]: what a headless shot
//! or a test draws from instead of the network.
//!
//! The fetcher itself is the map's [`DirTiles`] (`rustradar_map::io`), which the app's headless
//! path uses too (item T1); this wrapper keeps the tests' names for it. The folder holds one
//! subfolder per source, named as the map's layers name their source in a
//! [`rustradar_map::io::FetchRequest`] (`openfreemap`, `esri`, `terrarium`, `rainviewer`,
//! `streets-gl`), with each tile at `<source>/<z>/<x>/<y>.<ext>`: the bytes the server sent, the
//! HTTP compression undone, as `backend::tiles` would hand them over. An empty `<y>.none` marks a
//! tile the server had nothing for. `cargo xtask fixtures` writes the set; its `NOTICE` names the
//! sources and their terms.
//!
//! Answers, at once on the calling thread:
//! - a tile the set holds: `Ok(Some(bytes))`;
//! - a tile the server had nothing for: `Ok(None)`;
//! - anything else (another tile, another source): `Ok(None)` as well, so a shot simply does not
//!   draw it (plan section 8), and the request is noted in [`DirTiles::misses`] so a test can
//!   say the set lacks it;
//! - a file that cannot be read: `Err(FetchError::Failed)`.
#![allow(dead_code)]

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

#[allow(unused_imports)]
pub use rustradar_map::io::Held;
use rustradar_map::io::{DirTiles, FetchRequest, FetchResult, TileFetcher};

/// The committed fixtures folder, `crates/map/tests/data/tiles`. Found from the `crates` folder,
/// so that another crate's tests can take this file in with
/// `#[path = "../../map/tests/common/fixture_tiles.rs"]` (or the like) and still find the set.
pub fn fixtures_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).join("map/tests/data/tiles")
}

/// A [`TileFetcher`] over a fixtures folder: [`DirTiles`] under the tests' name.
#[derive(Debug)]
pub struct FixtureTiles(DirTiles);

impl Default for FixtureTiles {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureTiles {
    /// The committed set.
    pub fn new() -> Self {
        Self::at(fixtures_dir())
    }

    /// A set in another folder of the same layout.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        FixtureTiles(DirTiles::at(root))
    }
}

impl Deref for FixtureTiles {
    type Target = DirTiles;

    fn deref(&self) -> &DirTiles {
        &self.0
    }
}

impl TileFetcher for FixtureTiles {
    fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
        self.0.fetch(request)
    }
}
