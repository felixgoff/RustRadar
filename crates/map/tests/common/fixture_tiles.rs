//! The tile fixtures of `tests/data/tiles` (plan E16) as a [`TileFetcher`]: what a headless shot
//! or a test draws from instead of the network.
//!
//! The folder holds one subfolder per source, named as the map's layers name their source in a
//! [`FetchRequest`] (`openfreemap`, `esri`, `terrarium`, `rainviewer`, `streets-gl`), with each
//! tile at `<source>/<z>/<x>/<y>.<ext>`: the bytes the server sent, the HTTP compression undone,
//! as `backend::tiles` would hand them over. An empty `<y>.none` marks a tile the server had
//! nothing for. `cargo xtask fixtures` writes the set; its `NOTICE` names the sources and their
//! terms.
//!
//! Answers, at once on the calling thread:
//! - a tile the set holds: `Ok(Some(bytes))`;
//! - a tile the server had nothing for: `Ok(None)`;
//! - anything else (another tile, another source): `Ok(None)` as well, so a shot simply does not
//!   draw it (plan section 8), and the request is noted in [`FixtureTiles::misses`] so a test can
//!   say the set lacks it;
//! - a file that cannot be read: `Err(FetchError::Failed)`.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Receiver;

use rustradar_map::io::{FetchError, FetchRequest, FetchResult, TileFetcher, answered};
use rustradar_scene::tiles::TileIndex;

/// The committed fixtures folder.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tiles")
}

/// What the set holds for a tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// The tile's file.
    Tile(PathBuf),
    /// The server had nothing there.
    Nothing,
    /// The set has no answer for it.
    NotHeld,
}

/// A [`TileFetcher`] over a fixtures folder.
#[derive(Debug)]
pub struct FixtureTiles {
    root: PathBuf,
    misses: Mutex<Vec<(String, TileIndex)>>,
}

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
        FixtureTiles { root: root.into(), misses: Mutex::new(Vec::new()) }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The sources the set holds tiles of.
    pub fn sources(&self) -> Vec<String> {
        let mut sources: Vec<String> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        sources.sort();
        sources
    }

    /// What the set holds for `tile` of `source`.
    pub fn held(&self, source: &str, tile: TileIndex) -> Held {
        // (a source name is a folder name: nothing that climbs out of the set)
        if source.is_empty() || source.starts_with('.') || source.contains(['/', '\\']) {
            return Held::NotHeld;
        }
        let dir = self.root.join(source).join(tile.z.to_string()).join(tile.x.to_string());
        let stem = tile.y.to_string();
        let Ok(entries) = fs::read_dir(&dir) else { return Held::NotHeld };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_stem().and_then(|s| s.to_str()) != Some(stem.as_str()) {
                continue;
            }
            return match path.extension().and_then(|e| e.to_str()) {
                Some("none") => Held::Nothing,
                _ => Held::Tile(path),
            };
        }
        Held::NotHeld
    }

    /// The answer for `tile` of `source` (see the module's list).
    pub fn read(&self, source: &str, tile: TileIndex) -> FetchResult {
        match self.held(source, tile) {
            Held::Tile(path) => {
                fs::read(&path).map(Some).map_err(|e| FetchError::Failed(format!("{}: {e}", path.display())))
            }
            Held::Nothing => Ok(None),
            Held::NotHeld => {
                self.misses.lock().unwrap_or_else(|e| e.into_inner()).push((source.to_owned(), tile));
                Ok(None)
            }
        }
    }

    /// The requests the set had no answer for, in the order they came.
    pub fn misses(&self) -> Vec<(String, TileIndex)> {
        self.misses.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Every tile the set holds of `source` (files and "nothing there" markers), sorted by zoom,
    /// column and row.
    pub fn tiles(&self, source: &str) -> Vec<TileIndex> {
        let numbered = |dir: &Path| -> Vec<(u32, PathBuf)> {
            fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| Some((e.file_name().to_str()?.parse().ok()?, e.path())))
                .collect()
        };
        let mut tiles = Vec::new();
        for (z, zdir) in numbered(&self.root.join(source)) {
            for (x, xdir) in numbered(&zdir) {
                for entry in fs::read_dir(&xdir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if let Some(y) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse().ok()) {
                        tiles.push(TileIndex::new(x, y, z));
                    }
                }
            }
        }
        tiles.sort_by_key(|t| (t.z, t.x, t.y));
        tiles
    }
}

impl TileFetcher for FixtureTiles {
    fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
        answered(self.read(&request.source, request.tile))
    }
}
