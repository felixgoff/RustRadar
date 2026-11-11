//! Signal blocks and signals on the map (item T5; doc 02 §6; `Globe.svelte:2164-2413`,
//! `trains.ts:301-341`): what the six block layers draw, worked out from the block set the app
//! fetched round the view and the trains as drawn, at each scene refresh.
//!
//! - **Geometry** ([`BlockRuns`], `blockRuns`), once per block set: every line of every block cut
//!   into its stretches on the ground and up on bridges ([`BlockRuns::above`], `[lon, lat,
//!   metres]`) and in tunnels ([`BlockRuns::below`]) by [`on_structures`]; and each block's signal
//!   where its first line starts, with the blocks a train passing it goes on into ([`signals_of`]:
//!   those starting within [`BLOCK_JOIN_M`] of an end of one of its lines, found through a hash of
//!   the starts in cells of 0.001°, the 3 × 3 cells round each end).
//! - **Taken blocks** ([`taken_blocks`], `takenBlocks`): each train drawn inside the view's bounds
//!   is in the block [`block_at`] gives for where it is drawn, kept in the block it was in while it
//!   is still within [`rustradar_core::trains`]' 2 m of it (the hysteresis); worked out at most
//!   every [`TAKEN_EVERY_MS`], and at once for a new set ([`BlockBuilder`]).
//! - **What is drawn** ([`BlocksScene`]): nothing unless the blocks are on, a set is in and the
//!   camera is at zoom [`BLOCKS_MIN_ZOOM`] or closer (so always on the flat map). Then, in order:
//!   `train-blocks-casing` (every `above` run in [`BLOCK_CASING`], 3 px wider than its block),
//!   `train-blocks` (free [`BLOCK_FREE`] 3 px, taken [`BLOCK_TAKEN`] 5 px), `train-blocks-tunnels`
//!   (the `below` runs in their block's colour at half its alpha, dashed `[2.5, 2]` justified),
//!   `train-signals` (a 6 px disc per signal by its [`Aspect`]: stop when its own block is taken,
//!   caution when a block it leads into is, else clear; 2 px casing line), `train-signal-names`
//!   (from zoom [`SIGNAL_NAMES_ZOOM`], the signals with a name) and `train-block-labels` (one per
//!   taken block, the trains' names joined by ", ", at the middle vertex of its first line). The
//!   map's `layers::blocks` draws them; colours, widths and styles are here.
//! - **Picking and the tooltip** (for item T8): [`BlocksScene::picks`] (the signals, deck's
//!   `train-signals` is the only pickable block layer) and [`BlocksScene::signal_tip`] (its text).
//!
//! As today: the view's bounds are those of `new WebMercatorViewport({...view, width, height})`
//! ([`view_bounds`], none on the globe); a train's names join in the list's order; a block set
//! whose answer came back short while loading stays (the app's `state::blocks`).
//!
//! Deviations from today's app:
//! - `blocksTaken.version` changes only when which trains are in which blocks changes (today it
//!   is bumped at every recomputation): it is what the map layer remakes its lines for, and the
//!   picture is the same.
//! - A block without a line, or whose first line is empty, has no signal and no label (today's
//!   page would throw on it: the backend never sends one).

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Arc;

use rustradar_core::model::{Structure, TrainBlock};
use rustradar_core::num::{js_round, js_string};
use rustradar_core::trains::block_at;

use crate::bridges::on_structures;
use crate::camera::{FLAT_ABOVE_ZOOM, MercatorViewport, Viewport, ViewState};
use crate::geom::js_hypot2;
use crate::pick::{DepthTest, PickList};

/// Signal blocks show from this zoom (the signals are only mapped close in).
pub const BLOCKS_MIN_ZOOM: f64 = 13.0;
/// A free block's line.
pub const BLOCK_FREE: [u8; 4] = [60, 200, 110, 230];
/// A taken block's line.
pub const BLOCK_TAKEN: [u8; 4] = [235, 64, 52, 255];
/// Under every block's line, so it reads on any map; also the signals' outline.
pub const BLOCK_CASING: [u8; 4] = [16, 18, 24, 200];
/// A free block's width, px.
pub const FREE_WIDTH_PX: f64 = 3.0;
/// A taken block's width, px.
pub const TAKEN_WIDTH_PX: f64 = 5.0;
/// The casing is this much wider than its block, px.
pub const CASING_EXTRA_PX: f64 = 3.0;
/// A block's track in a tunnel: at this much of its colour's alpha, dashed.
pub const BLOCK_TUNNEL_ALPHA: f64 = 0.5;
/// The tunnels' dashes: dash and gap, in deck's dash units (half line widths, as
/// `pipelines::path` reads them).
pub const BLOCK_TUNNEL_DASHES: [f64; 2] = [2.5, 2.0];
/// Signals' names show from this zoom.
pub const SIGNAL_NAMES_ZOOM: f64 = 15.5;
/// A block's end this near another's start leads into it, metres.
pub const BLOCK_JOIN_M: f64 = 6.0;
/// The taken blocks are worked out again at most this often, ms.
pub const TAKEN_EVERY_MS: f64 = 500.0;
/// The signals' discs: radius and outline width, px.
pub const SIGNAL_RADIUS_PX: f64 = 6.0;
pub const SIGNAL_LINE_PX: f64 = 2.0;
/// The signals' names: size px, colour, offset px (right of the signal, anchored at their start),
/// box colour and padding (x, y) px.
pub const SIGNAL_NAME_SIZE: f64 = 10.0;
pub const SIGNAL_NAME_COLOR: [u8; 4] = [235, 238, 245, 255];
pub const SIGNAL_NAME_OFFSET: [f64; 2] = [10.0, 0.0];
pub const SIGNAL_NAME_BACKGROUND: [u8; 4] = [16, 18, 24, 200];
pub const SIGNAL_NAME_PADDING: [f64; 2] = [3.0, 1.0];
/// The taken blocks' labels: size px, colour, box colour and padding (x, y) px.
pub const BLOCK_LABEL_SIZE: f64 = 11.0;
pub const BLOCK_LABEL_COLOR: [u8; 4] = [255, 255, 255, 255];
pub const BLOCK_LABEL_BACKGROUND: [u8; 4] = [150, 30, 24, 230];
pub const BLOCK_LABEL_PADDING: [f64; 2] = [4.0, 2.0];
/// The signal tooltip's last line.
pub const SIGNAL_TIP_NOTE: &str = "From where the trains are, not the signal itself";

/// A signal's aspect, as worked out from the blocks: its own taken, the next one taken, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aspect {
    Stop,
    Caution,
    Clear,
}

impl Aspect {
    /// `ASPECT_COLOR`: the signal's disc.
    pub fn color(self) -> [u8; 4] {
        match self {
            Aspect::Stop => [235, 64, 52, 255],
            Aspect::Caution => [250, 196, 40, 255],
            Aspect::Clear => [60, 200, 110, 255],
        }
    }

    /// The tooltip's word for it.
    pub fn word(self) -> &'static str {
        match self {
            Aspect::Stop => "Stop",
            Aspect::Caution => "Caution",
            Aspect::Clear => "Clear",
        }
    }
}

/// A main signal, where its block starts, and the blocks a train passing it goes on into
/// (`BlockSignal`).
#[derive(Debug, Clone, PartialEq)]
pub struct BlockSignal {
    /// Its block's id.
    pub block: String,
    pub name: Option<String>,
    /// `[lon, lat]`: the start of its block's first line.
    pub at: [f64; 2],
    /// The ids of the blocks whose start one of its block's lines ends at, in the order found.
    pub next: Vec<String>,
}

/// A stretch of a block's track as drawn: in a tunnel or not, up on the bridges along it
/// (`BlockRun`).
#[derive(Debug, Clone, PartialEq)]
pub struct BlockRun {
    /// Its block's id.
    pub block: String,
    /// `[lon, lat, metres up]`.
    pub path: Vec<[f64; 3]>,
}

/// One block set's track as drawn and its signals, worked out once for each set (`blockRuns`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockRuns {
    /// On the ground and on bridges.
    pub above: Vec<BlockRun>,
    /// In tunnels.
    pub below: Vec<BlockRun>,
    pub signals: Vec<BlockSignal>,
}

/// Metres between two places, `[lon, lat]` (flat: fine over a few km; `metresBetween`).
pub fn metres_between(a: [f64; 2], b: [f64; 2]) -> f64 {
    let k = (a[1] * PI / 180.0).cos();
    js_hypot2((b[1] - a[1]) * 111_320.0, (b[0] - a[0]) * 111_320.0 * k)
}

/// The start of a block (its signal's place): the first point of its first line.
fn start_of(block: &TrainBlock) -> Option<[f64; 2]> {
    block.lines.first().and_then(|line| line.first()).copied()
}

/// `${Math.round(x · 1000) + dx},${Math.round(y · 1000) + dy}`: a cell of about 100 m.
fn cell_key(p: [f64; 2], dx: f64, dy: f64) -> String {
    format!("{},{}", js_string(js_round(p[0] * 1000.0) + dx), js_string(js_round(p[1] * 1000.0) + dy))
}

/// Each block's signal, and the blocks whose start one of its track's ends meets (`signalsOf`).
pub fn signals_of(list: &[TrainBlock]) -> Vec<BlockSignal> {
    // the blocks by where they start, in cells of about 100 m
    let mut starts: HashMap<String, Vec<&TrainBlock>> = HashMap::new();
    for b in list {
        if let Some(start) = start_of(b) {
            starts.entry(cell_key(start, 0.0, 0.0)).or_default().push(b);
        }
    }
    list.iter()
        .filter_map(|b| {
            let at = start_of(b)?;
            let mut next: Vec<String> = Vec::new();
            for line in &b.lines {
                let Some(&end) = line.last() else { continue };
                for dx in [-1.0, 0.0, 1.0] {
                    for dy in [-1.0, 0.0, 1.0] {
                        for o in starts.get(&cell_key(end, dx, dy)).into_iter().flatten() {
                            let Some(o_start) = start_of(o) else { continue };
                            if o.id != b.id && metres_between(end, o_start) <= BLOCK_JOIN_M && !next.contains(&o.id) {
                                next.push(o.id.clone());
                            }
                        }
                    }
                }
            }
            Some(BlockSignal { block: b.id.clone(), name: b.signal.clone(), at, next })
        })
        .collect()
}

/// The set's track as drawn and its signals: every line of every block on the bridges and tunnels
/// of `structures` ([`on_structures`]).
pub fn runs_of(list: &[TrainBlock], structures: &[Structure]) -> BlockRuns {
    let mut runs = BlockRuns { signals: signals_of(list), ..Default::default() };
    for b in list {
        for line in &b.lines {
            for run in on_structures(line, structures) {
                let drawn = BlockRun { block: b.id.clone(), path: run.path };
                if run.tunnel { runs.below.push(drawn) } else { runs.above.push(drawn) }
            }
        }
    }
    runs
}

/// The view's bounds close in (the flat projection), `[west, south, east, north]` (`viewBounds`);
/// none on the globe.
pub fn view_bounds(view: &ViewState, size: [f64; 2]) -> Option<[f64; 4]> {
    if view.zoom <= FLAT_ABOVE_ZOOM {
        return None;
    }
    Some(MercatorViewport::new(view, size[0], size[1]).bounds(0.0))
}

/// A train as drawn, for [`taken_blocks`]: its id, its name and where it is drawn, `[lon, lat]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawnTrain<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub at: [f64; 2],
}

/// Which trains are in which blocks: by block id the trains' names joined by ", " (`by`), and by
/// train id its block (`of`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Taken {
    pub by: HashMap<String, String>,
    pub of: HashMap<String, String>,
}

/// Each block a train is drawn in (`blockAt`: it stays in the one it was in, `was` by train id),
/// and each train's block (`takenBlocks`). Only trains drawn inside `bounds` count; none without
/// bounds.
pub fn taken_blocks(trains: &[DrawnTrain<'_>], list: &[TrainBlock], was: &HashMap<String, String>, bounds: Option<[f64; 4]>) -> Taken {
    let mut taken = Taken::default();
    let Some([west, south, east, north]) = bounds else { return taken };
    for train in trains {
        let p = train.at;
        if p[0] < west || p[0] > east || p[1] < south || p[1] > north {
            continue;
        }
        let Some(block) = block_at(p, list, was.get(train.id).map(String::as_str)) else { continue };
        taken.of.insert(train.id.to_owned(), block.to_owned());
        taken
            .by
            .entry(block.to_owned())
            .and_modify(|names| {
                names.push_str(", ");
                names.push_str(train.name);
            })
            .or_insert_with(|| train.name.to_owned());
    }
    taken
}

/// A taken block's label: the trains in it, at the middle vertex of its first line.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockLabel {
    pub text: String,
    /// `[lon, lat]`.
    pub at: [f64; 2],
}

/// The signal tooltip's text (`tooltip` for `train-signals`, G:3400-3411): the head line, and the
/// line under it; [`SIGNAL_TIP_NOTE`] comes last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalTip {
    pub head: String,
    pub sub: String,
}

/// What the block layers draw at one scene refresh. The default draws nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlocksScene {
    /// The set's track and signals (shared by the scene refreshes of one set).
    pub runs: Arc<BlockRuns>,
    /// Changes with each new set (the map makes its lines again).
    pub runs_generation: u64,
    /// The trains' names by the id of the block they are in.
    pub taken: Arc<HashMap<String, String>>,
    /// Changes whenever `taken` does (the lines' colours and widths follow it).
    pub taken_version: u64,
    /// `train-signal-names`: the signals' names are shown (zoom ≥ [`SIGNAL_NAMES_ZOOM`]).
    pub names: bool,
    /// `train-block-labels`, in the set's order.
    pub labels: Vec<BlockLabel>,
}

impl BlocksScene {
    /// Nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.runs.above.is_empty() && self.runs.below.is_empty() && self.runs.signals.is_empty()
    }

    /// Whether the block of id `block` has a train in it.
    pub fn is_taken(&self, block: &str) -> bool {
        self.taken.contains_key(block)
    }

    /// A run's colour (`color(r)`): taken or free.
    pub fn run_color(&self, run: &BlockRun) -> [u8; 4] {
        if self.is_taken(&run.block) { BLOCK_TAKEN } else { BLOCK_FREE }
    }

    /// A run's width, px (`width(r)`).
    pub fn run_width(&self, run: &BlockRun) -> f64 {
        if self.is_taken(&run.block) { TAKEN_WIDTH_PX } else { FREE_WIDTH_PX }
    }

    /// A run's casing width, px.
    pub fn casing_width(&self, run: &BlockRun) -> f64 {
        self.run_width(run) + CASING_EXTRA_PX
    }

    /// A tunnel run's colour: its block's at half the alpha (free 115, taken 128).
    pub fn tunnel_color(&self, run: &BlockRun) -> [u8; 4] {
        let [r, g, b, a] = self.run_color(run);
        [r, g, b, js_round(f64::from(a) * BLOCK_TUNNEL_ALPHA) as u8]
    }

    /// A signal's aspect (`aspect(s)`).
    pub fn aspect(&self, signal: &BlockSignal) -> Aspect {
        if self.is_taken(&signal.block) {
            Aspect::Stop
        } else if signal.next.iter().any(|n| self.is_taken(n)) {
            Aspect::Caution
        } else {
            Aspect::Clear
        }
    }

    /// The signals whose names are drawn (`train-signal-names`' data): with a name, from zoom
    /// [`SIGNAL_NAMES_ZOOM`].
    pub fn named_signals(&self) -> impl Iterator<Item = (&BlockSignal, &str)> {
        let shown = self.names;
        self.runs.signals.iter().filter(move |_| shown).filter_map(|s| s.name.as_deref().filter(|n| !n.is_empty()).map(|n| (s, n)))
    }

    /// The signals' pick records, in draw order (`train-signals`, `pickable: true`, depth always):
    /// each a disc of its outer radius (the radius and half the outline), keyed by its index in
    /// [`BlockRuns::signals`].
    pub fn picks(&self, viewport: &Viewport) -> PickList<usize> {
        let mut list = PickList::new();
        for (i, s) in self.runs.signals.iter().enumerate() {
            let at = viewport.project([s.at[0], s.at[1], 0.0]);
            list.point(i, at, SIGNAL_RADIUS_PX + SIGNAL_LINE_PX / 2.0, DepthTest::Always);
        }
        list
    }

    /// The tooltip of the signal at `index` of [`BlockRuns::signals`] (for item T8).
    pub fn signal_tip(&self, index: usize) -> Option<SignalTip> {
        let signal = self.runs.signals.get(index)?;
        let by = &self.taken;
        let ahead: Vec<&str> = signal.next.iter().filter_map(|n| by.get(n).map(String::as_str)).collect();
        let (aspect, sub) = if let Some(trains) = by.get(&signal.block) {
            (Aspect::Stop, format!("{trains} in the block after it"))
        } else if !ahead.is_empty() {
            (Aspect::Caution, format!("{} in the next block", ahead.join(", ")))
        } else {
            (Aspect::Clear, "No train in the block after it".to_owned())
        };
        let name = match signal.name.as_deref() {
            Some(name) if !name.is_empty() => format!("Signal {name}"),
            _ => "Main signal".to_owned(),
        };
        Some(SignalTip { head: format!("{name} · {}", aspect.word()), sub })
    }
}

/// A block set as the builder takes it: its generation (new with each set the app keeps), its
/// blocks and the bridges and tunnels round them.
#[derive(Debug, Clone, Copy)]
pub struct SetRef<'a> {
    pub generation: u64,
    pub list: &'a [TrainBlock],
    pub structures: &'a [Structure],
}

/// The block layers' part of the scene refresh (`blockLayers` less its fetching): the set's
/// geometry and the taken blocks, kept between refreshes.
#[derive(Debug, Clone, Default)]
pub struct BlockBuilder {
    runs: Arc<BlockRuns>,
    /// The set `runs` was made for.
    runs_for: Option<u64>,
    /// `blocksTaken.at`: when the taken blocks were last worked out (local ms); 0 forces it.
    taken_at: f64,
    taken: Taken,
    by: Arc<HashMap<String, String>>,
    version: u64,
}

impl BlockBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The block layers at `now_ms` (local Unix ms) with the camera at `view` on a canvas of
    /// `size` CSS px: nothing without a set (the blocks off, or none fetched yet) or below zoom
    /// [`BLOCKS_MIN_ZOOM`]; else the set's track and signals, the blocks `trains` (as drawn) are
    /// in, worked out again when [`TAKEN_EVERY_MS`] have passed or the set is new.
    pub fn build(&mut self, set: Option<SetRef<'_>>, trains: &[DrawnTrain<'_>], view: &ViewState, size: [f64; 2], now_ms: f64) -> BlocksScene {
        let Some(set) = set else { return BlocksScene::default() };
        if view.zoom < BLOCKS_MIN_ZOOM {
            return BlocksScene::default();
        }
        if self.runs_for != Some(set.generation) {
            // a new set: its geometry, and the taken blocks at once (`blocksTaken.at = 0`)
            self.runs = Arc::new(runs_of(set.list, set.structures));
            self.runs_for = Some(set.generation);
            self.taken_at = 0.0;
        }
        if now_ms - self.taken_at > TAKEN_EVERY_MS {
            let taken = taken_blocks(trains, set.list, &self.taken.of, view_bounds(view, size));
            self.taken_at = now_ms;
            if taken.by != *self.by {
                self.by = Arc::new(taken.by.clone());
                self.version += 1;
            }
            self.taken = taken;
        }
        // each taken block named once, halfway along its first stretch
        let labels = set
            .list
            .iter()
            .filter_map(|b| {
                let text = self.by.get(&b.id)?;
                let first = b.lines.first()?;
                let at = *first.get(first.len() / 2)?;
                Some(BlockLabel { text: text.clone(), at })
            })
            .collect();
        BlocksScene {
            runs: self.runs.clone(),
            runs_generation: set.generation,
            taken: self.by.clone(),
            taken_version: self.version,
            names: view.zoom >= SIGNAL_NAMES_ZOOM,
            labels,
        }
    }

    /// The taken blocks as last worked out (`blocksTaken`): by block and by train.
    pub fn taken(&self) -> &Taken {
        &self.taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Klostergården, Lund: 55.68° N, 13.19° E.
    const LAT: f64 = 55.68;
    const LON: f64 = 13.19;

    /// The place `east` metres east and `north` metres north of [`LAT`], [`LON`].
    fn spot(east: f64, north: f64) -> [f64; 2] {
        [LON + east / (111_320.0 * (LAT * PI / 180.0).cos()), LAT + north / 111_320.0]
    }

    fn block(id: &str, signal: Option<&str>, lines: &[&[[f64; 2]]]) -> TrainBlock {
        TrainBlock { id: id.into(), signal: signal.map(Into::into), lines: lines.iter().map(|l| l.to_vec()).collect() }
    }

    /// Three blocks running east one after the other, `a` branching at its end into `b` and onto
    /// the track beside, where `c` starts 4 m off; `d` starts 8 m from `a`'s end.
    fn line_of_blocks() -> Vec<TrainBlock> {
        vec![
            block("a", Some("101"), &[&[spot(0.0, 0.0), spot(500.0, 0.0), spot(1000.0, 0.0)], &[spot(0.0, 0.0), spot(1000.0, 4.5)]]),
            block("b", Some("103"), &[&[spot(1003.0, 0.0), spot(2000.0, 0.0)]]),
            block("c", None, &[&[spot(1000.0, 8.5), spot(2000.0, 4.5)]]),
            block("d", Some(""), &[&[spot(1000.0, -8.0), spot(2000.0, -8.0)]]),
        ]
    }

    fn train<'a>(id: &'a str, name: &'a str, at: [f64; 2]) -> DrawnTrain<'a> {
        DrawnTrain { id, name, at }
    }

    /// Wide bounds round everything.
    const ALL: Option<[f64; 4]> = Some([-180.0, -90.0, 180.0, 90.0]);

    #[test]
    fn metres_between_is_flat_and_scales_longitude_at_the_first_place() {
        assert!((metres_between(spot(0.0, 0.0), spot(300.0, 400.0)) - 500.0).abs() < 1e-6);
        assert_eq!(metres_between([0.0, 0.0], [1.0, 0.0]), 111_320.0);
        let k = (60f64 * PI / 180.0).cos();
        assert!((metres_between([0.0, 60.0], [1.0, 60.0]) - 111_320.0 * k).abs() < 1e-9);
    }

    #[test]
    fn a_signal_leads_into_the_blocks_starting_within_six_metres_of_its_ends() {
        let signals = signals_of(&line_of_blocks());
        assert_eq!(signals.len(), 4);
        let a = &signals[0];
        assert_eq!((a.block.as_str(), a.name.as_deref(), a.at), ("a", Some("101"), spot(0.0, 0.0)));
        // `b` 3 m from the first line's end, `c` 4 m from the second's; `d` 8 m away is not next
        assert_eq!(a.next, ["b", "c"]);
        assert_eq!(signals[1].next, Vec::<String>::new());
        assert_eq!((signals[2].name.as_deref(), signals[2].at), (None, spot(1000.0, 8.5)));
        assert_eq!(signals[3].name.as_deref(), Some(""));
    }

    #[test]
    fn the_starts_are_found_across_cell_edges_and_never_the_block_itself() {
        // an end just west of a 0.001° cell line, the next block's start just east of it
        let end = [13.1904999, LAT];
        let start = [13.1905001, LAT];
        let list = vec![block("x", None, &[&[[13.18, LAT], end]]), block("y", None, &[&[start, [13.20, LAT]]]), block("x", None, &[&[end, [13.2, LAT]]])];
        let signals = signals_of(&list);
        assert_eq!(signals[0].next, ["y"], "the neighbouring cell's start; a block of the same id never");
        // a loop: a block whose end is its own start does not lead into itself
        let ring = vec![block("r", None, &[&[spot(0.0, 0.0), spot(100.0, 0.0), spot(0.0, 1.0)]])];
        assert!(signals_of(&ring)[0].next.is_empty());
        // found once even when two lines end there
        let twice = vec![block("p", None, &[&[spot(0.0, 0.0), spot(100.0, 0.0)], &[spot(0.0, 1.0), spot(100.0, 1.0)]]), block("q", None, &[&[spot(101.0, 0.0), spot(200.0, 0.0)]])];
        assert_eq!(signals_of(&twice)[0].next, ["q"]);
    }

    #[test]
    fn a_block_without_a_line_has_no_signal() {
        let list = vec![block("e", None, &[]), block("f", None, &[&[]]), block("g", None, &[&[spot(0.0, 0.0), spot(10.0, 0.0)]])];
        let signals = signals_of(&list);
        assert_eq!(signals.iter().map(|s| s.block.as_str()).collect::<Vec<_>>(), ["g"]);
        // each line is a run as `on_structures` gives it: the empty one with an empty path
        let runs = runs_of(&list, &[]);
        assert_eq!(runs.above.iter().map(|r| r.path.len()).collect::<Vec<_>>(), [0, 2]);
    }

    #[test]
    fn the_track_runs_on_the_ground_up_bridges_and_through_tunnels() {
        let list = line_of_blocks();
        let plain = runs_of(&list, &[]);
        assert_eq!(plain.above.len(), 5);
        assert!(plain.below.is_empty());
        assert_eq!(plain.above[0], BlockRun { block: "a".into(), path: vec![[spot(0.0, 0.0)[0], LAT, 0.0], [spot(500.0, 0.0)[0], LAT, 0.0], [spot(1000.0, 0.0)[0], LAT, 0.0]] });
        // a tunnel under the middle of a line with a point every 100 m: the stretches whose middle
        // is within 4 m of it are in the tunnel
        let line: Vec<[f64; 2]> = (0..=10).map(|k| spot(1000.0 + 100.0 * f64::from(k), 0.0)).collect();
        let long = vec![block("t", Some("7"), &[&line])];
        let tunnel = Structure { tunnel: true, line: vec![spot(1300.0, 0.0), spot(1700.0, 0.0)], joined: [false, false] };
        let runs = runs_of(&long, std::slice::from_ref(&tunnel));
        assert_eq!(runs.above.len(), 2, "before and after the tunnel");
        assert_eq!(runs.below.len(), 1);
        assert!(runs.below.iter().chain(&runs.above).all(|r| r.block == "t"));
        let inside: Vec<[f64; 2]> = runs.below[0].path.iter().map(|p| [p[0], p[1]]).collect();
        assert_eq!(inside, line[3..=7], "from 1300 to 1700 m");
        assert_eq!(runs.above[0].path.last(), runs.below[0].path.first(), "the runs meet at the portal");
        // a bridge lifts the track along it
        let bridge = Structure { tunnel: false, line: vec![spot(1300.0, 0.0), spot(1700.0, 0.0)], joined: [false, false] };
        let runs = runs_of(&long, &[bridge]);
        assert!(runs.below.is_empty());
        let top = runs.above.iter().flat_map(|r| r.path.iter()).map(|p| p[2]).fold(0.0, f64::max);
        assert!(top > 1.0, "up on the bridge: {top}");
        assert_eq!(runs.signals, signals_of(&long));
    }

    #[test]
    fn trains_inside_the_view_take_the_block_they_are_drawn_in_named_in_list_order() {
        let list = line_of_blocks();
        let trains = [
            train("t1", "SJ 523", spot(250.0, 0.5)),
            train("t2", "Pendeltåg 2245", spot(1500.0, 0.3)),
            train("t3", "Öresundståg 1087", spot(750.0, -0.4)),
            // off any track
            train("t4", "Far off", spot(500.0, 40.0)),
        ];
        let taken = taken_blocks(&trains, &list, &HashMap::new(), ALL);
        assert_eq!(taken.by.len(), 2);
        assert_eq!(taken.by["a"], "SJ 523, Öresundståg 1087");
        assert_eq!(taken.by["b"], "Pendeltåg 2245");
        assert_eq!(taken.of.len(), 3);
        assert_eq!((taken.of["t1"].as_str(), taken.of["t2"].as_str(), taken.of["t3"].as_str()), ("a", "b", "a"));
        // only the trains inside the bounds; none on the globe
        let west_half = Some([LON - 1.0, LAT - 1.0, spot(1000.0, 0.0)[0], LAT + 1.0]);
        let taken = taken_blocks(&trains, &list, &HashMap::new(), west_half);
        assert_eq!(taken.by.keys().collect::<Vec<_>>(), ["a"]);
        assert_eq!(taken_blocks(&trains, &list, &HashMap::new(), None), Taken::default());
    }

    /// Two tracks 4.5 m apart: `a` on the north one runs on through a crossover at 1 km onto the
    /// south one, where `b` already runs (as `core::trains`' `blockAt` tests).
    fn crossover() -> Vec<TrainBlock> {
        vec![
            block("a", Some("A"), &[&[spot(0.0, 4.5), spot(1_000.0, 4.5), spot(1_050.0, 0.0), spot(2_000.0, 0.0)]]),
            block("b", Some("B"), &[&[spot(0.0, 0.0), spot(2_000.0, 0.0)]]),
        ]
    }

    #[test]
    fn taken_blocks_hysteresis_keeps_a_train_in_the_block_it_was_in() {
        let blocks = crossover();
        // a train on the south track, drawn up to a metre off it: in `b` all the way, also past
        // 1050 m, where `a` runs on the same track and is as near
        let mut of = HashMap::new();
        for k in 0..100 {
            let jitter = if k % 2 == 0 { 0.8 } else { -0.8 };
            let taken = taken_blocks(&[train("s", "South", spot(20.0 * f64::from(k), jitter))], &blocks, &of, ALL);
            assert_eq!(taken.of.get("s").map(String::as_str), Some("b"), "at {} m", 20 * k);
            assert_eq!(taken.by.get("b").map(String::as_str), Some("South"));
            of = taken.of;
        }
        // one coming along the north track through the crossover: in `a` all the way
        let mut of = HashMap::new();
        for k in 0..100 {
            let east = 20.0 * f64::from(k);
            let north = if east <= 1_000.0 { 4.5 } else if east <= 1_050.0 { 4.5 * (1_050.0 - east) / 50.0 } else { 0.0 };
            let jitter = if k % 2 == 0 { 0.5 } else { -0.5 };
            let taken = taken_blocks(&[train("n", "North", spot(east, north + jitter))], &blocks, &of, ALL);
            assert_eq!(taken.of.get("n").map(String::as_str), Some("a"), "at {east} m");
            of = taken.of;
        }
        // more than 2 m nearer another block's track: it moves over
        let was = HashMap::from([("n".to_owned(), "a".to_owned())]);
        let moved = taken_blocks(&[train("n", "North", spot(500.0, 0.0))], &blocks, &was, ALL);
        assert_eq!(moved.of["n"], "b", "4.5 m from `a`, on `b`");
        let kept = taken_blocks(&[train("n", "North", spot(500.0, 1.5))], &blocks, &was, ALL);
        assert_eq!(kept.of["n"], "a", "3 m from `a`, 1.5 from `b`: kept, within 2 m of the nearest");
        assert_eq!(taken_blocks(&[train("n", "North", spot(500.0, 1.5))], &blocks, &HashMap::new(), ALL).of["n"], "b");
    }

    fn view(zoom: f64) -> ViewState {
        ViewState { longitude: spot(1000.0, 0.0)[0], latitude: LAT, zoom, pitch: 0.0, bearing: 0.0, position: None }
    }

    /// A canvas wide enough to hold every block at zoom 16 (±1.3 km round the view's centre).
    const SIZE: [f64; 2] = [4000.0, 3000.0];

    #[test]
    fn view_bounds_are_the_flat_viewports_and_none_on_the_globe() {
        assert_eq!(view_bounds(&view(12.0), SIZE), None);
        let [w, s, e, n] = view_bounds(&view(13.0), SIZE).unwrap();
        let v = view(13.0);
        assert!(w < v.longitude && e > v.longitude && s < v.latitude && n > v.latitude);
        // 4000 px at zoom 13: 4000 / (512 · 2^13) of the world's 360°
        assert!(((e - w) - 4000.0 / (512.0 * 8192.0) * 360.0).abs() < 1e-9, "{}", e - w);
    }

    #[test]
    fn the_builder_draws_nothing_off_without_a_set_or_further_out_than_zoom_13() {
        let list = line_of_blocks();
        let set = SetRef { generation: 1, list: &list, structures: &[] };
        let mut b = BlockBuilder::new();
        assert!(b.build(None, &[], &view(15.0), SIZE, 1000.0).is_empty());
        assert_eq!(b.build(Some(set), &[], &view(12.99), SIZE, 1000.0), BlocksScene::default());
        let scene = b.build(Some(set), &[], &view(13.0), SIZE, 1000.0);
        assert!(!scene.is_empty());
        assert_eq!((scene.runs.above.len(), scene.runs.signals.len(), scene.labels.len()), (5, 4, 0));
        assert!(!scene.names, "names from zoom 15.5");
        assert!(b.build(Some(set), &[], &view(15.5), SIZE, 1000.0).names);
    }

    #[test]
    fn taken_blocks_are_worked_out_every_half_second_and_at_once_for_a_new_set() {
        let list = line_of_blocks();
        let set = SetRef { generation: 1, list: &list, structures: &[] };
        let mut b = BlockBuilder::new();
        let v = view(15.5);
        let at_a = [train("t1", "SJ 523", spot(250.0, 0.5))];
        let at_b = [train("t1", "SJ 523", spot(1500.0, 0.5))];
        let t0 = 1.782e12;
        let first = b.build(Some(set), &at_a, &v, SIZE, t0);
        assert!(first.is_taken("a") && !first.is_taken("b"));
        // the train moved on into `b`: not seen until 500 ms have passed
        let soon = b.build(Some(set), &at_b, &v, SIZE, t0 + 500.0);
        assert!(soon.is_taken("a"), "500 ms is not more than 500 ms");
        assert_eq!(soon.taken_version, first.taken_version);
        assert!(Arc::ptr_eq(&soon.runs, &first.runs), "the geometry is kept for the set");
        let later = b.build(Some(set), &at_b, &v, SIZE, t0 + 501.0);
        assert!(later.is_taken("b") && !later.is_taken("a"));
        assert!(later.taken_version > first.taken_version);
        assert_eq!(b.taken().of["t1"], "b");
        // the same blocks taken again: the version stands
        let same = b.build(Some(set), &at_b, &v, SIZE, t0 + 1100.0);
        assert_eq!(same.taken_version, later.taken_version);
        // a new set is worked out at once
        let fewer = list[..1].to_vec();
        let new = SetRef { generation: 2, list: &fewer, structures: &[] };
        let scene = b.build(Some(new), &at_b, &v, SIZE, t0 + 1200.0);
        assert!(!scene.is_taken("b"));
        assert_eq!(scene.runs_generation, 2);
        assert_eq!(scene.runs.signals.len(), 1);
    }

    #[test]
    fn colours_widths_aspects_and_labels_follow_the_taken_blocks() {
        let list = line_of_blocks();
        let set = SetRef { generation: 7, list: &list, structures: &[] };
        let mut b = BlockBuilder::new();
        let trains = [train("t2", "Pendeltåg 2245", spot(1500.0, 0.3)), train("t5", "Snälltåget 74", spot(1800.0, 0.2))];
        let scene = b.build(Some(set), &trains, &view(16.0), SIZE, 5000.0);
        let run_a = &scene.runs.above[0];
        let run_b = scene.runs.above.iter().find(|r| r.block == "b").unwrap();
        assert_eq!((scene.run_color(run_a), scene.run_width(run_a), scene.casing_width(run_a)), (BLOCK_FREE, 3.0, 6.0));
        assert_eq!((scene.run_color(run_b), scene.run_width(run_b), scene.casing_width(run_b)), (BLOCK_TAKEN, 5.0, 8.0));
        assert_eq!(scene.tunnel_color(run_a), [60, 200, 110, 115]);
        assert_eq!(scene.tunnel_color(run_b), [235, 64, 52, 128]);
        let aspects: Vec<Aspect> = scene.runs.signals.iter().map(|s| scene.aspect(s)).collect();
        assert_eq!(aspects, [Aspect::Caution, Aspect::Stop, Aspect::Clear, Aspect::Clear]);
        assert_eq!(Aspect::Caution.color(), [250, 196, 40, 255]);
        // the label halfway along `b`'s first line (its second point of two: index 1)
        assert_eq!(scene.labels, [BlockLabel { text: "Pendeltåg 2245, Snälltåget 74".into(), at: spot(2000.0, 0.0) }]);
        // the names of the signals that have one
        let names: Vec<&str> = scene.named_signals().map(|(_, n)| n).collect();
        assert_eq!(names, ["101", "103"]);
        let further = b.build(Some(set), &trains, &view(15.0), SIZE, 5000.0);
        assert_eq!(further.named_signals().count(), 0);
    }

    #[test]
    fn the_signal_tooltip_says_what_the_blocks_say() {
        let list = line_of_blocks();
        let set = SetRef { generation: 1, list: &list, structures: &[] };
        let mut b = BlockBuilder::new();
        let scene = b.build(Some(set), &[train("t2", "Pendeltåg 2245", spot(1500.0, 0.3)), train("t6", "SJ 10", spot(1500.0, 8.0))], &view(16.0), SIZE, 1.782e12);
        assert_eq!(
            scene.signal_tip(0),
            Some(SignalTip { head: "Signal 101 · Caution".into(), sub: "Pendeltåg 2245, SJ 10 in the next block".into() })
        );
        assert_eq!(scene.signal_tip(1), Some(SignalTip { head: "Signal 103 · Stop".into(), sub: "Pendeltåg 2245 in the block after it".into() }));
        assert_eq!(scene.signal_tip(2), Some(SignalTip { head: "Main signal · Stop".into(), sub: "SJ 10 in the block after it".into() }));
        assert_eq!(scene.signal_tip(3), Some(SignalTip { head: "Main signal · Clear".into(), sub: "No train in the block after it".into() }));
        assert_eq!(scene.signal_tip(4), None);
    }

    #[test]
    fn the_signals_are_picked_as_discs_of_their_outer_radius_drawn_over_everything() {
        let list = line_of_blocks();
        let set = SetRef { generation: 1, list: &list, structures: &[] };
        let v = view(16.0);
        let scene = BlockBuilder::new().build(Some(set), &[], &v, SIZE, 1.0);
        let viewport = crate::camera::viewport_for(&v, SIZE);
        let picks = scene.picks(&viewport);
        assert_eq!(picks.len(), 4);
        let at = viewport.project([spot(0.0, 0.0)[0], LAT, 0.0]);
        assert_eq!(picks.items()[0].shape, crate::pick::Shape::Point { at, radius: 7.0 });
        assert_eq!(picks.items()[0].depth_test, DepthTest::Always);
        // 12 px off (7 + 6 of reach): still the signal; 14 px off: nothing
        let hit = picks.pick(&viewport, [at[0] + 12.0, at[1]], 1.0).map(|h| *h.key);
        assert_eq!(hit, Some(0));
        assert_eq!(picks.pick(&viewport, [at[0], at[1] - 14.0], 1.0).map(|h| *h.key), None);
    }
}
