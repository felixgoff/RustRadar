//! Aircraft labels (item A7; doc 03 §7, doc 01 §9.2; G:754-969, 3026-3035): each labelled
//! aircraft's callsign beside it and, closer in, its height under that, decluttered as today's
//! `aircraft-label-names` and `aircraft-label-heights` TextLayers are by deck's collision filter.
//! `rustradar_map::layers::aircraft::labels` lays the text out in its glyph atlas and draws it.
//!
//! What today's app does, and so this module:
//! - **Which aircraft** are labelled is the scene builder's loop ([`crate::aircraft`]'s
//!   [`AircraftScene::labelled`], doc 03 §7.2): the label setting on, everyone from zoom 4
//!   (`AIRCRAFT_LABEL_MIN_ZOOM`) and below that only the selected aircraft, at zoom ≤ 6 only those
//!   facing the camera (`cosAngle > 0.42`) or the locked one, and only with a name. Each at the
//!   very position its aircraft is drawn (a mesh raised onto its wheels).
//! - **Text and priority** (`core::labels`): `aircraftLabel(f)` (callsign, else flight number,
//!   else registration), `altitudeLabel(f.alt, f.onGround)` from the *reported* altitude for the
//!   height line (`GND`, `FL350`, `4,500 ft`), `labelPriority(size, alt, onGround)`; the selected
//!   aircraft takes `SELECTED_LABEL_PRIORITY` (1000).
//! - **The label set** ([`Labeller`], `updateLabelSet`, G:845-871). The texts, heights,
//!   priorities and sizes are worked out once per set and kept while the feed is the same
//!   (`flights`' identity), the height line is the same and the same aircraft are labelled; then
//!   only the positions move, put back in the set's own order. Otherwise a new set is made (its
//!   version one more) in this refresh's order. With the label setting off, a non-empty set is let
//!   go of (G:3031-3035). The globe's `flights` is the page's `shown`, a new array whenever the
//!   filtered feed or the `selected` row is a new object; [`Labeller::build`] reads that as the
//!   caller's feed key (the filtered feed's generation) and the selected aircraft's row, so a newer
//!   report from the follow stream lays the set out again and the selected aircraft's height line
//!   follows it, as today.
//! - **Layout** ([`layout`], [`AircraftLabels`], `aircraftLabelLayers`, G:873-969): both lines
//!   anchored at the aircraft, text anchor `start`, baseline `center`, Overpass 600 from the SDF
//!   atlas (64 px, buffer 6) with the 3-pixel halo in `[6,12,19,220]`, billboards, `sizeScale` the
//!   label size setting. The callsign 11 px at pixel offset `[gap, −line]`, the height 10 px at
//!   `[gap, +line]`, with `gap = screenPx(size)/2 + 5` (right of the aircraft as drawn) and
//!   `line = 6·scale` when the height line shows (zoom > 7), else 0. Colours
//!   `[241,238,231,235]` and `[162,172,183,220]`, the selected aircraft's
//!   `[55,142,240,255]` and `[55,142,240,205]`.
//! - **The claimed box** ([`layout`]): the callsign layer has a transparent background whose
//!   padding `[left, top, right, bottom]` is
//!   `[gap(largest) + reserve, padY, 4 + reserve − 60·scale, padY]`, with `half = 11·scale/2`,
//!   `spread = 1 + 0.6·clamp01((7 − zoom)/3)`, `padY = (half + 2·line + 4·scale)·spread` and
//!   `reserve = 60·scale·spread`: a box about one callsign wide left of the aircraft too, and the
//!   label's height again above and below, wider apart zoomed out.
//! - **Collision** ([`collision_fades`]): deck 9.4's `CollisionFilterExtension` for the group
//!   `"aircraft-labels"`, done on the CPU in [`CollisionMap`] as B4 does it for the place labels
//!   (`crate::labels_map`), not as the plan's whole-box greedy pass (`crate::declutter`). The
//!   collision pass draws, into a map at half the canvas's resolution with the depth test sorting
//!   by priority (between equals the one drawn later wins), the group's three sublayers in deck's
//!   order: every label's background box (`aircraft-label-names`' TextBackgroundLayer, drawn
//!   before its characters), then every callsign's glyph quads, then every height's; all at the
//!   collision test's `sizeScale`, which is the label size setting itself (`collisionTestProps`).
//!   Both lines and the box share their aircraft's row, as deck's picking colours do, so the two
//!   lines show and hide together. Each label's fade is then its share of the 5 × 5 patch round
//!   its aircraft, to the power 2.2. So, as today and unlike the greedy pass, only the aircraft's
//!   point is tested: an aircraft inside a more important label's box loses its label, and a label
//!   hidden this way still hides less important ones (the reserve counts once, not on both boxes).
//! - **Crisp fade** ([`alpha`]): the fill colours' alpha times `smoothstep(0.3, 0.65, fade)`
//!   instead of the fade itself (G:788-803); the halo is not faded (deck multiplies the instance
//!   colour only), and a label under a fade of 0.0001 is not drawn at all.
//!
//! Positions in the collision pass are the canvas's device pixels (CSS pixels × the map scale)
//! from its top-left corner, y down. A billboard's pixel offsets, its glyphs' and its background's
//! (padding included) shrink with distance by the focal depth's clip w over the anchor's (the
//! `perspective` of [`Placing`]), as `project_pixel_size_to_clipspace` scales them.
//!
//! Deviations, both invisible: the fade multiplies the colour's 8-bit alpha (deck multiplies the
//! float); and a feed holding one id twice, which today leaves a hole in `labelPositions`, here
//! leaves that label without a position, which draws nothing either.

use std::collections::HashMap;
use std::sync::Arc;

use rustradar_core::labels::{SELECTED_LABEL_PRIORITY, aircraft_label, altitude_label, label_priority};
use rustradar_core::model::LiveFlight;
use rustradar_core::num::{clamp01, js_round, smoothstep};

use crate::aircraft::{AircraftScene, Frame};
use crate::declutter::{CRISP_FROM, CRISP_TO, DRAWN_MIN_FADE};
use crate::labels_map::{CollisionMap, glyph_quads};
use crate::text::{LaidText, TextLayout};

/// From here a second, dimmer line gives the height (`AIRCRAFT_LABEL_HEIGHT_ZOOM`): zoom > 7.
pub const AIRCRAFT_LABEL_HEIGHT_ZOOM: f64 = 7.0;
/// The callsign's size, px before the label size setting (`AIRCRAFT_LABEL_SIZE`).
pub const AIRCRAFT_LABEL_SIZE: f64 = 11.0;
/// The height line's size, px (`AIRCRAFT_LABEL_HEIGHT_SIZE`).
pub const AIRCRAFT_LABEL_HEIGHT_SIZE: f64 = 10.0;
/// The callsign's colour (`AIRCRAFT_LABEL_COLOR`).
pub const AIRCRAFT_LABEL_COLOR: [u8; 4] = [241, 238, 231, 235];
/// The selected aircraft's callsign: the selection blue (`AIRCRAFT_LABEL_SELECTED`).
pub const AIRCRAFT_LABEL_SELECTED: [u8; 4] = [55, 142, 240, 255];
/// The height line's colour (`AIRCRAFT_LABEL_HEIGHT_COLOR`).
pub const AIRCRAFT_LABEL_HEIGHT_COLOR: [u8; 4] = [162, 172, 183, 220];
/// The selected aircraft's height line (`AIRCRAFT_LABEL_HEIGHT_SELECTED`).
pub const AIRCRAFT_LABEL_HEIGHT_SELECTED: [u8; 4] = [55, 142, 240, 205];
/// Between the aircraft as drawn and its label, px (`AIRCRAFT_LABEL_GAP_PX`).
pub const AIRCRAFT_LABEL_GAP_PX: f64 = 5.0;
/// Half the distance between the two lines' middles, px at the default size
/// (`AIRCRAFT_LABEL_LINE_PX`).
pub const AIRCRAFT_LABEL_LINE_PX: f64 = 6.0;
/// How far left of its aircraft a label's claimed box reaches, px at the default size and zoomed
/// in (`AIRCRAFT_LABEL_RESERVE_PX`): about one callsign wide.
pub const AIRCRAFT_LABEL_RESERVE_PX: f64 = 60.0;
/// The labels' collision group (`AIRCRAFT_LABEL_GROUP`); place labels are in `"labels"`.
pub const AIRCRAFT_LABEL_GROUP: &str = "aircraft-labels";
/// The callsign layer's background: invisible, it only claims the box (`NO_BACKGROUND`).
pub const NO_BACKGROUND: [u8; 4] = [0; 4];
/// Both lines' text layout: anchor `start`, baseline `center`, deck's defaults otherwise.
pub const LAYOUT: TextLayout = TextLayout::START;

/// Whether the height line shows at `zoom` (`zoom > AIRCRAFT_LABEL_HEIGHT_ZOOM`, G:3029).
pub fn with_height(zoom: f64) -> bool {
    zoom > AIRCRAFT_LABEL_HEIGHT_ZOOM
}

/// `Math.max(a, b)`: NaN when either is.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

// ---------------------------------------------------------------------------------------------
// The label set

/// One laid-out set of labels (`LabelSet`, G:819-833): who is labelled and what each label says,
/// kept while the feed, the height line and the aircraft labelled stay the same.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LabelSet {
    /// The feed the labels were made from (`feed`); `None` for an empty set.
    pub feed: Option<u64>,
    pub with_height: bool,
    /// The labelled flights' ids, in the set's order (`items`).
    pub ids: Vec<u32>,
    /// Each id's place in the set (`index`).
    pub index: HashMap<u32, usize>,
    /// `aircraftLabel(f)`.
    pub texts: Vec<String>,
    /// `altitudeLabel(f.alt, f.onGround)` with the height line, else `""`.
    pub heights: Vec<String>,
    /// `labelPriority(size, f.alt, f.onGround)`.
    pub priorities: Vec<f64>,
    /// Each aircraft's size, metres, for the gap to its label.
    pub sizes: Vec<f64>,
    /// The largest of [`Self::sizes`] (0 for none).
    pub largest: f64,
    /// Bumped whenever the set is laid out again: the texts change only with it.
    pub version: u64,
}

impl LabelSet {
    /// `emptyLabelSet(version)`.
    pub fn empty(version: u64) -> LabelSet {
        LabelSet { version, ..LabelSet::default() }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

/// The aircraft labels' state across scene refreshes (`labelSet`, `labelPositions`): the scene
/// builder keeps one.
#[derive(Debug, Clone, Default)]
pub struct Labeller {
    set: Arc<LabelSet>,
    /// Where each label's aircraft is drawn this refresh, by the set's index.
    positions: Vec<Option<[f64; 3]>>,
    /// The feed key and the selected aircraft's row [`Self::build`] last saw (`shown`'s identity),
    /// and the key it handed [`Self::update`] for them, one more each time either changed.
    shown: Option<(u64, Option<LiveFlight>)>,
    shown_key: u64,
}

impl Labeller {
    pub fn new() -> Self {
        Self::default()
    }

    /// The set as it stands.
    pub fn set(&self) -> &Arc<LabelSet> {
        &self.set
    }

    /// Where each label's aircraft is drawn, by the set's index.
    pub fn positions(&self) -> &[Option<[f64; 3]>] {
        &self.positions
    }

    /// `updateLabelSet(drawn, positions, sizes, withHeight)`: `drawn` is each labelled flight with
    /// where its aircraft is drawn and its size (metres), in this refresh's order; `feed` names the
    /// flights they come from (today, the `flights` array's identity). Keeps the set while it is
    /// the same and points each label at its aircraft's new position; else makes a new one.
    /// Whether a new set was made.
    pub fn update<'a>(&mut self, feed: u64, drawn: impl IntoIterator<Item = (&'a LiveFlight, [f64; 3], f64)>, with_height: bool) -> bool {
        let drawn: Vec<(&LiveFlight, [f64; 3], f64)> = drawn.into_iter().collect();
        let set = &self.set;
        let same = set.feed == Some(feed)
            && set.with_height == with_height
            && set.ids.len() == drawn.len()
            && drawn.iter().all(|(f, _, _)| set.index.contains_key(&f.id));
        if same {
            let mut placed = vec![None; drawn.len()];
            for (f, position, _) in &drawn {
                placed[set.index[&f.id]] = Some(*position);
            }
            self.positions = placed;
            return false;
        }
        let mut set = LabelSet::empty(self.set.version + 1);
        set.feed = Some(feed);
        set.with_height = with_height;
        for (i, &(f, _, size)) in drawn.iter().enumerate() {
            set.ids.push(f.id);
            set.index.insert(f.id, i);
            set.texts.push(aircraft_label(f));
            set.heights.push(if with_height { altitude_label(f64::from(f.alt), f.on_ground) } else { String::new() });
            set.priorities.push(label_priority(size, f64::from(f.alt), f.on_ground));
            set.sizes.push(size);
            set.largest = js_max(set.largest, size);
        }
        self.set = Arc::new(set);
        self.positions = drawn.iter().map(|&(_, position, _)| Some(position)).collect();
        true
    }

    /// The label setting is off: let go of the last feed's labels (G:3031-3035), if there are any.
    pub fn clear(&mut self) {
        if !self.set.is_empty() {
            self.set = Arc::new(LabelSet::empty(self.set.version + 1));
            self.positions.clear();
        }
    }

    /// What the label layers draw this refresh (`aircraftLabelLayers(screenPx, …, selectedId,
    /// labelScale, zoom)`) from the set as it stands, for the refresh's `frame` with `selected` the
    /// selected flight and `scale` the label size setting.
    pub fn labels(&self, frame: &Frame, selected: Option<u32>, scale: f64) -> AircraftLabels {
        let set = &self.set;
        if set.is_empty() {
            return AircraftLabels { scale, ..AircraftLabels::default() };
        }
        let gaps = set.sizes.iter().map(|&m| gap_for(frame, m)).collect();
        AircraftLabels {
            set: set.clone(),
            positions: self.positions.clone(),
            gaps,
            selected,
            scale,
            layout: layout(scale, frame.zoom, set.with_height, gap_for(frame, set.largest)),
        }
    }

    /// The tail of `buildLayers()` (G:3027-3035) for one refresh's `scene`: with `labelling` (the
    /// label setting, the font ready) the set is updated from the scene's labelled aircraft
    /// (`flight(row)` is the flight at `row` of the flights handed to the scene builder, `feed`
    /// names the filtered feed they come from) and the labels returned; else the set is let go of
    /// and nothing is labelled.
    ///
    /// Today the flights are the page's `shown`, a new array whenever the filtered feed or the
    /// selected row is a new object, and a new array is a new set. So the set is also made again
    /// when the selected aircraft's row differs from the one last seen (the follow stream's newer
    /// report: its height line follows it). Only a labelled selected aircraft's row is looked at:
    /// a new set for the same feed is otherwise the same set, so nothing else would show it.
    pub fn build<'a>(&mut self, scene: &AircraftScene, labelling: bool, feed: u64, flight: impl Fn(usize) -> Option<&'a LiveFlight>, scale: f64) -> AircraftLabels {
        if !labelling {
            self.clear();
            return AircraftLabels { scale, ..AircraftLabels::default() };
        }
        let selected_row = scene.selected.and_then(|id| scene.labelled.iter().find(|l| l.id == id)).and_then(|l| flight(l.row));
        if self.shown.as_ref().is_none_or(|(f, row)| *f != feed || row.as_ref() != selected_row) {
            self.shown = Some((feed, selected_row.cloned()));
            self.shown_key += 1;
        }
        let drawn = scene.labelled.iter().filter_map(|l| flight(l.row).map(|f| (f, l.position, l.size_m)));
        self.update(self.shown_key, drawn, with_height(scene.frame.zoom));
        self.labels(&scene.frame, scene.selected, scale)
    }
}

// ---------------------------------------------------------------------------------------------
// Layout

/// The gap from an aircraft of `metres` to its label, px (`gapFor`): half its size on screen and
/// [`AIRCRAFT_LABEL_GAP_PX`].
pub fn gap_for(frame: &Frame, metres: f64) -> f64 {
    frame.screen_px(metres) / 2.0 + AIRCRAFT_LABEL_GAP_PX
}

/// The terms of `aircraftLabelLayers` that hold for every label of a refresh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// Half the distance between the two lines' middles, px: `6·scale` with the height line, else
    /// 0 (the callsign centred on its aircraft).
    pub line: f64,
    /// Half the callsign's height, px: `11·scale·0.5`.
    pub half: f64,
    /// `1 + 0.6·clamp01((7 − zoom)/3)`: zoomed out, where traffic is densest, labels keep further
    /// apart.
    pub spread: f64,
    /// `(half + 2·line + 4·scale)·spread`: the claimed box above and below the callsign.
    pub pad_y: f64,
    /// `60·scale·spread`.
    pub reserve: f64,
    /// The callsign layer's `backgroundPadding`, `[left, top, right, bottom]` px:
    /// `[gap(largest) + reserve, padY, 4 + reserve − 60·scale, padY]`.
    pub padding: [f64; 4],
}

impl Default for Layout {
    fn default() -> Self {
        layout(1.0, AIRCRAFT_LABEL_HEIGHT_ZOOM, false, AIRCRAFT_LABEL_GAP_PX)
    }
}

/// The layout at label size `scale` and `zoom`, with or without the height line, for a set whose
/// largest aircraft's gap is `largest_gap` px.
pub fn layout(scale: f64, zoom: f64, with_height: bool, largest_gap: f64) -> Layout {
    let line = if with_height { AIRCRAFT_LABEL_LINE_PX * scale } else { 0.0 };
    // the claimed box: from left of the largest aircraft's gap, as tall again as the label above
    // and below
    let half = AIRCRAFT_LABEL_SIZE * scale * 0.5;
    let spread = 1.0 + 0.6 * clamp01((AIRCRAFT_LABEL_HEIGHT_ZOOM - zoom) / 3.0);
    let pad_y = (half + 2.0 * line + 4.0 * scale) * spread;
    let reserve = AIRCRAFT_LABEL_RESERVE_PX * scale * spread;
    let padding = [largest_gap + reserve, pad_y, 4.0 + reserve - AIRCRAFT_LABEL_RESERVE_PX * scale, pad_y];
    Layout { line, half, spread, pad_y, reserve, padding }
}

/// What the aircraft label layers draw at one scene refresh: the set, where each label's aircraft
/// is, and the refresh's layout. Empty when nothing is labelled.
#[derive(Debug, Clone, PartialEq)]
pub struct AircraftLabels {
    pub set: Arc<LabelSet>,
    /// `[lon, lat, metres]` of each label's aircraft as drawn, by the set's index (`None`: not
    /// drawn).
    pub positions: Vec<Option<[f64; 3]>>,
    /// Each label's gap from its aircraft, px ([`gap_for`] of its size).
    pub gaps: Vec<f64>,
    /// The selected flight (`selectedId`).
    pub selected: Option<u32>,
    /// The label size setting (`labelScale`): both layers' `sizeScale`, and the collision test's.
    pub scale: f64,
    pub layout: Layout,
}

impl Default for AircraftLabels {
    fn default() -> Self {
        AircraftLabels { set: Arc::default(), positions: Vec::new(), gaps: Vec::new(), selected: None, scale: 1.0, layout: Layout::default() }
    }
}

impl AircraftLabels {
    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// No labels: neither layer is drawn (`if (!set.items.length) return []`).
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Whether the height layer is drawn.
    pub fn with_height(&self) -> bool {
        self.set.with_height
    }

    /// Whether label `i` is the selected aircraft's.
    pub fn is_selected(&self, i: usize) -> bool {
        self.selected.is_some() && self.selected == self.set.ids.get(i).copied()
    }

    /// `getCollisionPriority`: the selected aircraft's [`SELECTED_LABEL_PRIORITY`], else the set's.
    pub fn priority(&self, i: usize) -> f64 {
        if self.is_selected(i) { SELECTED_LABEL_PRIORITY } else { self.set.priorities[i] }
    }

    /// The callsign's colour before the fade.
    pub fn name_color(&self, i: usize) -> [u8; 4] {
        if self.is_selected(i) { AIRCRAFT_LABEL_SELECTED } else { AIRCRAFT_LABEL_COLOR }
    }

    /// The height line's colour before the fade.
    pub fn height_color(&self, i: usize) -> [u8; 4] {
        if self.is_selected(i) { AIRCRAFT_LABEL_HEIGHT_SELECTED } else { AIRCRAFT_LABEL_HEIGHT_COLOR }
    }

    /// The callsign's `getPixelOffset`, px: `[gap(i), −line]`.
    pub fn name_offset(&self, i: usize) -> [f64; 2] {
        [self.gaps[i], -self.layout.line]
    }

    /// The height line's `getPixelOffset`, px: `[gap(i), +line]`.
    pub fn height_offset(&self, i: usize) -> [f64; 2] {
        [self.gaps[i], self.layout.line]
    }

    /// The callsign's height in px as drawn and tested: `getSize · sizeScale`.
    pub fn name_px(&self) -> f64 {
        AIRCRAFT_LABEL_SIZE * self.scale
    }

    /// The height line's, px.
    pub fn height_px(&self) -> f64 {
        AIRCRAFT_LABEL_HEIGHT_SIZE * self.scale
    }

    /// Label `i`'s background box (the claimed box) for its callsign `name` laid out with
    /// [`LAYOUT`], anchored at `anchor` (px from the canvas's top left, y down) and shrunk by
    /// `perspective`: `[x0, y0, x1, y1]` px (TextBackgroundLayer's vertex shader: the text's box at
    /// its size, grown by the padding, moved by the pixel offset).
    pub fn background(&self, i: usize, anchor: [f64; 2], perspective: f64, name: &LaidText) -> [f64; 4] {
        let k = self.name_px() / name.font_size;
        let [x, y, w, h] = name.rect;
        let [left, top, right, bottom] = self.layout.padding;
        let [dx, dy] = self.name_offset(i);
        [
            anchor[0] + (x * k - left + dx) * perspective,
            anchor[1] + (y * k - top + dy) * perspective,
            anchor[0] + ((x + w) * k + right + dx) * perspective,
            anchor[1] + ((y + h) * k + bottom + dy) * perspective,
        ]
    }

    /// Label `i`'s callsign glyph quads (`[x0, y0, x1, y1]` px, each glyph's whole atlas frame).
    pub fn name_quads<'t>(&self, i: usize, anchor: [f64; 2], perspective: f64, name: &'t LaidText) -> impl Iterator<Item = [f64; 4]> + 't {
        glyph_quads(name, offset_anchor(anchor, self.name_offset(i), perspective), self.name_px(), perspective)
    }

    /// Label `i`'s height line glyph quads.
    pub fn height_quads<'t>(&self, i: usize, anchor: [f64; 2], perspective: f64, height: &'t LaidText) -> impl Iterator<Item = [f64; 4]> + 't {
        glyph_quads(height, offset_anchor(anchor, self.height_offset(i), perspective), self.height_px(), perspective)
    }
}

/// `anchor` moved by a billboard's pixel offset, which shrinks with distance as its glyphs do.
fn offset_anchor(anchor: [f64; 2], [dx, dy]: [f64; 2], perspective: f64) -> [f64; 2] {
    [anchor[0] + dx * perspective, anchor[1] + dy * perspective]
}

// ---------------------------------------------------------------------------------------------
// Collision

/// One label for [`collision_fades`], by the set's index.
#[derive(Clone, Copy, Debug)]
pub struct Placing<'a> {
    /// Where its aircraft is drawn: CSS px from the canvas's top left, y down; `None` when it is
    /// not in front of the camera (or has no position).
    pub anchor: Option<[f64; 2]>,
    /// The focal depth's clip w over the anchor's.
    pub perspective: f64,
    /// Its callsign laid out with [`LAYOUT`].
    pub name: &'a LaidText,
    /// Its height line, when the height layer is drawn.
    pub height: Option<&'a LaidText>,
}

/// deck's collision filter for the group `"aircraft-labels"` in one frame: `placings` (one per
/// label of `labels`, in the set's order) drawn into `map` over a canvas of `canvas` CSS pixels at
/// `map_scale` device pixels each, the backgrounds first, then the callsigns, then the heights;
/// then each label's fade (0 to 1; under [`DRAWN_MIN_FADE`] it is not drawn).
pub fn collision_fades(map: &mut CollisionMap, canvas: [f64; 2], map_scale: f64, labels: &AircraftLabels, placings: &[Placing<'_>]) -> Vec<f64> {
    let device = [(canvas[0] * map_scale).round(), (canvas[1] * map_scale).round()];
    map.begin(device);
    let (kx, ky) = (device[0] / canvas[0], device[1] / canvas[1]);
    let to_device = move |[x0, y0, x1, y1]: [f64; 4]| [x0 * kx, y0 * ky, x1 * kx, y1 * ky];
    let rows = || placings.iter().enumerate().filter_map(|(i, p)| p.anchor.map(|anchor| (i, anchor, p)));
    // `aircraft-label-names-background`
    for (i, anchor, p) in rows() {
        map.draw(i, labels.priority(i), [to_device(labels.background(i, anchor, p.perspective, p.name))]);
    }
    // `aircraft-label-names-characters`
    for (i, anchor, p) in rows() {
        map.draw(i, labels.priority(i), labels.name_quads(i, anchor, p.perspective, p.name).map(to_device));
    }
    // `aircraft-label-heights-characters`
    if labels.with_height() {
        for (i, anchor, p) in rows() {
            if let Some(height) = p.height {
                map.draw(i, labels.priority(i), labels.height_quads(i, anchor, p.perspective, height).map(to_device));
            }
        }
    }
    placings.iter().enumerate().map(|(i, p)| p.anchor.map_or(0.0, |[x, y]| map.fade(i, [x * kx, y * ky]))).collect()
}

/// The "crisp" curve (`CrispCollisionFilterExtension`): the alpha factor of a label of `fade`,
/// `smoothstep(0.3, 0.65, fade)`.
pub fn alpha(fade: f64) -> f64 {
    smoothstep((fade - CRISP_FROM) / (CRISP_TO - CRISP_FROM))
}

/// `color` with its alpha times [`alpha`]`(fade)` (deck's `color.a *= …` on the instance colour).
pub fn faded(color: [u8; 4], fade: f64) -> [u8; 4] {
    let [r, g, b, a] = color;
    [r, g, b, js_round(f64::from(a) * alpha(fade)).clamp(0.0, 255.0) as u8]
}

/// Whether a label of `fade` is drawn at all: deck throws a vertex under a fade of 0.0001 out of
/// clip space.
pub fn drawn(fade: f64) -> bool {
    fade >= DRAWN_MIN_FADE
}

#[cfg(test)]
mod tests {
    use rustradar_core::assert_close;

    use super::*;
    use crate::aircraft::Labelled;
    use crate::camera::ViewState;
    use crate::text::LaidGlyph;

    fn flight(id: u32, callsign: &str, alt: i32, on_ground: bool) -> LiveFlight {
        LiveFlight { id, callsign: callsign.into(), alt, on_ground, ..Default::default() }
    }

    fn frame_at(zoom: f64) -> Frame {
        let view = ViewState { longitude: 18.07, latitude: 59.33, zoom, bearing: 0.0, pitch: 0.0, position: None };
        Frame::new(&view, [1440.0, 900.0], 1.0, true)
    }

    /// Plan A7: the padding formula of doc 03 §7.5 at zoom 4, 7 and 10, worked by hand.
    #[test]
    fn the_claimed_box_at_zoom_4_7_and_10() {
        // zoom 4, no height line (zoom ≤ 7): spread 1 + 0.6·clamp01(3/3) = 1.6
        let l = layout(1.0, 4.0, with_height(4.0), 9.0);
        assert_eq!(l.line, 0.0);
        assert_close!(l.half, 5.5, 12);
        assert_close!(l.spread, 1.6, 12);
        // (5.5 + 0 + 4)·1.6
        assert_close!(l.pad_y, 15.2, 12);
        assert_close!(l.reserve, 96.0, 12);
        // [9 + 96, 15.2, 4 + 96 − 60, 15.2]
        for (v, expected) in l.padding.iter().zip([105.0, 15.2, 40.0, 15.2]) {
            assert_close!(*v, expected, 12);
        }
        // zoom 7: still no height line, spread 1
        let l = layout(1.0, 7.0, with_height(7.0), 9.0);
        assert_eq!((l.line, l.spread), (0.0, 1.0));
        for (v, expected) in l.padding.iter().zip([69.0, 9.5, 4.0, 9.5]) {
            assert_close!(*v, expected, 12);
        }
        // zoom 10, the height line on: line 6, padY (5.5 + 12 + 4)·1 = 21.5
        let l = layout(1.0, 10.0, with_height(10.0), 9.0);
        assert_eq!((l.line, l.spread, l.reserve), (6.0, 1.0, 60.0));
        for (v, expected) in l.padding.iter().zip([69.0, 21.5, 4.0, 21.5]) {
            assert_close!(*v, expected, 12);
        }
        // the label size setting scales everything but the gap; the reserve's right side stays 4
        let l = layout(1.5, 10.0, true, 9.0);
        assert_close!(l.line, 9.0, 12);
        assert_close!(l.pad_y, (8.25 + 18.0 + 6.0) * 1.0, 12);
        for (v, expected) in l.padding.iter().zip([9.0 + 90.0, 32.25, 4.0, 32.25]) {
            assert_close!(*v, expected, 12);
        }
        // zoom 5.5: halfway into the spread
        let l = layout(1.0, 5.5, false, 0.0);
        assert_close!(l.spread, 1.3, 12);
        assert_close!(l.padding[2], 4.0 + 78.0 - 60.0, 12);
    }

    #[test]
    fn the_height_line_shows_past_zoom_7() {
        assert!(!with_height(7.0));
        assert!(with_height(7.000_001));
        assert!(!with_height(4.0));
    }

    #[test]
    fn the_gap_is_half_the_aircraft_on_screen_and_5_px() {
        // zoomed out every aircraft sits at the floor, which still says big from small
        let frame = frame_at(4.0);
        assert_close!(gap_for(&frame, 38.0), frame.screen_px(38.0) / 2.0 + 5.0, 12);
        assert!(gap_for(&frame, 73.0) > gap_for(&frame, 38.0));
        // close in, its true size
        let frame = frame_at(17.0);
        assert_close!(gap_for(&frame, 38.0), 38.0 / frame.mpp / 2.0 + 5.0, 9);
    }

    fn drawn_at(f: &LiveFlight, x: f64) -> (&LiveFlight, [f64; 3], f64) {
        (f, [x, 59.0, 1000.0], 38.0)
    }

    #[test]
    fn a_set_is_kept_while_the_feed_the_height_line_and_the_aircraft_are_the_same() {
        let a = flight(1, "SAS1", 3_000, false);
        let b = flight(2, "", 37_000, false);
        let b = LiveFlight { flight: "LH400".into(), ..b };
        let c = flight(3, "BAW7", 0, true);
        let mut labeller = Labeller::new();
        assert!(labeller.update(7, [drawn_at(&a, 1.0), drawn_at(&b, 2.0)], true));
        let set = labeller.set().clone();
        assert_eq!(set.version, 1);
        assert_eq!(set.ids, [1, 2]);
        assert_eq!(set.texts, ["SAS1", "LH400"]);
        assert_eq!(set.heights, ["3,000 ft", "FL370"]);
        assert_eq!(set.priorities, [label_priority(38.0, 3_000.0, false), label_priority(38.0, 37_000.0, false)]);
        assert_eq!((set.sizes.clone(), set.largest), (vec![38.0, 38.0], 38.0));

        // the same aircraft in another order: the set stays, each position put in its own place
        assert!(!labeller.update(7, [drawn_at(&b, 20.0), drawn_at(&a, 10.0)], true));
        assert!(Arc::ptr_eq(labeller.set(), &set));
        assert_eq!(labeller.positions(), [Some([10.0, 59.0, 1000.0]), Some([20.0, 59.0, 1000.0])]);
        // the texts are not looked at again: a report's new altitude waits for the next feed
        let a_higher = LiveFlight { alt: 5_000, ..a.clone() };
        assert!(!labeller.update(7, [drawn_at(&a_higher, 1.0), drawn_at(&b, 2.0)], true));
        assert_eq!(labeller.set().heights[0], "3,000 ft");

        // a new feed, the height line going, an aircraft more or another one: a new set
        assert!(labeller.update(8, [drawn_at(&a_higher, 1.0), drawn_at(&b, 2.0)], true));
        assert_eq!((labeller.set().version, labeller.set().heights[0].as_str()), (2, "5,000 ft"));
        assert!(labeller.update(8, [drawn_at(&a, 1.0), drawn_at(&b, 2.0)], false));
        assert_eq!(labeller.set().heights, ["", ""], "no height line, no heights");
        assert!(labeller.update(8, [drawn_at(&a, 1.0), drawn_at(&b, 2.0), drawn_at(&c, 3.0)], false));
        assert!(labeller.update(8, [drawn_at(&a, 1.0), drawn_at(&c, 3.0)], false));
        assert_eq!(labeller.set().ids, [1, 3], "in this refresh's order");
        assert_eq!(labeller.set().version, 5);
        assert_eq!(labeller.set().priorities[1], label_priority(38.0, 0.0, true));

        // labels off: the set is let go of, once
        labeller.clear();
        assert_eq!((labeller.set().len(), labeller.set().version), (0, 6));
        assert!(labeller.positions().is_empty());
        labeller.clear();
        assert_eq!(labeller.set().version, 6, "nothing to let go of");
        // an empty set for the same feed: one more version, still empty
        assert!(labeller.update(8, [], false));
        assert_eq!(labeller.set().version, 7);
        assert!(!labeller.update(8, [], false));
    }

    #[test]
    fn the_largest_aircraft_sets_the_reach_left() {
        let (a, b) = (flight(1, "A", 0, true), flight(2, "B", 0, true));
        let mut labeller = Labeller::new();
        labeller.update(1, [(&a, [0.0; 3], 38.0), (&b, [0.0; 3], 73.0)], true);
        assert_eq!(labeller.set().largest, 73.0);
        let frame = frame_at(9.0);
        let labels = labeller.labels(&frame, None, 1.0);
        assert_close!(labels.layout.padding[0], gap_for(&frame, 73.0) + 60.0, 12);
        assert_eq!(labels.gaps, [gap_for(&frame, 38.0), gap_for(&frame, 73.0)]);
    }

    #[test]
    fn the_selected_aircraft_is_blue_and_wins() {
        let (a, b) = (flight(1, "SAS1", 3_000, false), flight(2, "SAS2", 3_000, false));
        let mut labeller = Labeller::new();
        labeller.update(1, [drawn_at(&a, 1.0), drawn_at(&b, 2.0)], true);
        let frame = frame_at(9.0);
        let labels = labeller.labels(&frame, Some(2), 1.0);
        assert_eq!((labels.priority(0), labels.priority(1)), (labeller.set().priorities[0], 1000.0));
        assert_eq!((labels.name_color(0), labels.name_color(1)), ([241, 238, 231, 235], [55, 142, 240, 255]));
        assert_eq!((labels.height_color(0), labels.height_color(1)), ([162, 172, 183, 220], [55, 142, 240, 205]));
        // above zoom 7 the callsign sits 6 px above the aircraft's middle, the height 6 below
        let gap = gap_for(&frame, 38.0);
        assert_eq!((labels.name_offset(0), labels.height_offset(0)), ([gap, -6.0], [gap, 6.0]));
        assert_eq!((labels.name_px(), labels.height_px()), (11.0, 10.0));
        let labels = labeller.labels(&frame, None, 1.2);
        assert_eq!(labels.priority(1), labeller.set().priorities[1]);
        assert_close!(labels.name_px(), 13.2, 12);
        assert_close!(labels.height_px(), 12.0, 12);
        assert_close!(labels.layout.line, 7.2, 12);
    }

    #[test]
    fn build_reads_the_scenes_labelled_aircraft_and_lets_go_with_the_setting_off() {
        let flights = [flight(1, "SAS1", 3_000, false), flight(2, "SAS2", 9_000, false), flight(3, "SAS3", 0, true)];
        let frame = frame_at(5.0);
        let scene = AircraftScene {
            frame,
            selected: Some(3),
            labelled: vec![
                Labelled { id: 3, row: 2, position: [3.0, 59.0, 0.0], size_m: 73.0 },
                Labelled { id: 1, row: 0, position: [1.0, 59.0, 914.4], size_m: 38.0 },
            ],
            ..Default::default()
        };
        let mut labeller = Labeller::new();
        let labels = labeller.build(&scene, true, 1, |row| flights.get(row), 1.0);
        assert_eq!(labels.set.ids, [3, 1]);
        assert!(!labels.with_height(), "zoom 5");
        assert_eq!(labels.positions, [Some([3.0, 59.0, 0.0]), Some([1.0, 59.0, 914.4])]);
        assert_eq!(labels.priority(0), 1000.0);
        assert_eq!(labels.layout, layout(1.0, 5.0, false, gap_for(&frame, 73.0)));
        let labels = labeller.build(&scene, false, 1, |row| flights.get(row), 1.0);
        assert!(labels.is_empty());
        assert_eq!(labeller.set().version, 2);
    }

    /// Today's `shown` is a new array whenever the selected row is a new object (the follow
    /// stream's newer report), and a new array is a new set: the selected aircraft's height line
    /// follows its row, the others' wait for the next feed.
    #[test]
    fn a_newer_row_for_the_selected_aircraft_lays_the_set_out_again() {
        let frame = frame_at(9.0);
        let scene = |selected: Option<u32>| AircraftScene {
            frame,
            selected,
            labelled: vec![
                Labelled { id: 1, row: 0, position: [1.0, 59.0, 914.4], size_m: 38.0 },
                Labelled { id: 2, row: 1, position: [2.0, 59.0, 2743.2], size_m: 38.0 },
            ],
            ..Default::default()
        };
        let feed = [flight(1, "SAS1", 3_000, false), flight(2, "SAS2", 9_000, false)];
        let mut labeller = Labeller::new();
        let labels = labeller.build(&scene(Some(2)), true, 7, |row| feed.get(row), 1.0);
        assert_eq!((labels.set.version, labels.set.heights.clone()), (1, vec!["3,000 ft".to_owned(), "9,000 ft".to_owned()]));
        // the same rows again: kept
        assert_eq!(labeller.build(&scene(Some(2)), true, 7, |row| feed.get(row), 1.0).set.version, 1);

        // the stream's report for the selected one, newer than the feed's
        let newer = [feed[0].clone(), LiveFlight { alt: 9_500, lat: 59.1, ..feed[1].clone() }];
        let labels = labeller.build(&scene(Some(2)), true, 7, |row| newer.get(row), 1.0);
        assert_eq!(labels.set.version, 2);
        assert_eq!(labels.set.heights, ["3,000 ft", "9,500 ft"]);
        assert_eq!(labeller.build(&scene(Some(2)), true, 7, |row| newer.get(row), 1.0).set.version, 2, "the same report: kept");

        // another aircraft's row on its own is not looked at before the next feed
        let other = [LiveFlight { alt: 3_300, ..feed[0].clone() }, newer[1].clone()];
        let labels = labeller.build(&scene(Some(2)), true, 7, |row| other.get(row), 1.0);
        assert_eq!((labels.set.version, labels.set.heights[0].as_str()), (2, "3,000 ft"));
        let labels = labeller.build(&scene(Some(2)), true, 8, |row| other.get(row), 1.0);
        assert_eq!((labels.set.version, labels.set.heights[0].as_str()), (3, "3,300 ft"), "a new feed");

        // selecting another aircraft or none is a new `shown` too
        assert_eq!(labeller.build(&scene(Some(1)), true, 8, |row| other.get(row), 1.0).set.version, 4);
        assert_eq!(labeller.build(&scene(None), true, 8, |row| other.get(row), 1.0).set.version, 5);
        assert_eq!(labeller.build(&scene(None), true, 8, |row| other.get(row), 1.0).set.version, 5);
        // a selected aircraft without a label changes nothing anyone sees: the set is kept
        assert_eq!(labeller.build(&scene(Some(9)), true, 8, |row| other.get(row), 1.0).set.version, 5);
    }

    /// A text laid out as the atlas would: glyphs 40 × 76 atlas px every 32 px from x = 20, the
    /// box 32·n wide and 64 tall from the anchor's middle row, at font size 64.
    fn laid(n: usize) -> LaidText {
        let glyphs = (0..n).map(|i| LaidGlyph { ch: 'A', frame: [0.0, 0.0, 40.0, 76.0], offset: [20.0 + 32.0 * i as f64, 0.0] }).collect();
        LaidText { glyphs, rect: [0.0, -32.0, 32.0 * n as f64, 64.0], font_size: 64.0, generation: 0 }
    }

    /// Labels of `n` letters for the flights `drawn` at zoom 9 (height line on), all 38 m.
    fn labels_for(ids: &[u32], selected: Option<u32>) -> AircraftLabels {
        let flights: Vec<LiveFlight> = ids.iter().map(|&id| flight(id, "SAS", 3_000, false)).collect();
        let mut labeller = Labeller::new();
        labeller.update(1, flights.iter().map(|f| (f, [0.0; 3], 38.0)), true);
        labeller.labels(&frame_at(9.0), selected, 1.0)
    }

    #[test]
    fn the_background_reaches_left_of_the_aircraft_and_above_and_below_both_lines() {
        let labels = labels_for(&[1], None);
        let name = laid(4);
        let gap = labels.gaps[0];
        let [left, top, right, bottom] = labels.layout.padding;
        let [x0, y0, x1, y1] = labels.background(0, [500.0, 300.0], 1.0, &name);
        // 4 letters of 32 atlas px at 11/64
        let width = 128.0 * 11.0 / 64.0;
        assert_close!(x0, 500.0 + gap - left, 9);
        assert_close!(x0, 500.0 - 60.0, 9);
        assert_close!(x1, 500.0 + gap + width + right, 9);
        assert_close!(x1, 500.0 + gap + width + 4.0, 9);
        // the callsign's middle 6 px up, 5.5 px each way, padY round that
        assert_close!(y0, 300.0 - 6.0 - 5.5 - top, 9);
        assert_close!(y1, 300.0 - 6.0 + 5.5 + bottom, 9);
        assert_close!(y1 - y0, 11.0 + 2.0 * 21.5, 9);
        // farther away everything shrinks about the anchor
        let [fx0, fy0, fx1, fy1] = labels.background(0, [500.0, 300.0], 0.5, &name);
        assert_close!(fx0, 500.0 + (x0 - 500.0) / 2.0, 9);
        assert_close!(fy0, 300.0 + (y0 - 300.0) / 2.0, 9);
        assert_close!(fx1, 500.0 + (x1 - 500.0) / 2.0, 9);
        assert_close!(fy1, 300.0 + (y1 - 300.0) / 2.0, 9);
        // the glyphs: the callsign above, the height below, both starting a gap right
        let names: Vec<_> = labels.name_quads(0, [500.0, 300.0], 1.0, &name).collect();
        let k = 11.0 / 64.0;
        assert_close!(names[0][0], 500.0 + gap + (20.0 - 20.0) * k, 9);
        assert_close!(names[0][1], 300.0 - 6.0 - 38.0 * k, 9);
        let heights: Vec<_> = labels.height_quads(0, [500.0, 300.0], 1.0, &name).collect();
        assert_close!(heights[0][1], 300.0 + 6.0 - 38.0 * 10.0 / 64.0, 9);
        assert_close!(heights[3][2], 500.0 + gap + (116.0 + 20.0) * 10.0 / 64.0, 9);
    }

    const CANVAS: [f64; 2] = [800.0, 400.0];

    fn fades(labels: &AircraftLabels, anchors: &[[f64; 2]], name: &LaidText) -> Vec<f64> {
        let placings: Vec<Placing<'_>> = anchors.iter().map(|&a| Placing { anchor: Some(a), perspective: 1.0, name, height: Some(name) }).collect();
        collision_fades(&mut CollisionMap::new(), CANVAS, 1.0, labels, &placings)
    }

    #[test]
    fn an_aircraft_inside_a_more_important_labels_box_loses_its_label() {
        let name = laid(4);
        // the second aircraft 40 px left of the first, which is selected: inside its reserve
        let labels = labels_for(&[1, 2], Some(1));
        assert_eq!(fades(&labels, &[[400.0, 200.0], [360.0, 200.0]], &name), [1.0, 0.0]);
        // 70 px left: past the reserve, both shown although the second's text runs into the first's box
        assert_eq!(fades(&labels, &[[400.0, 200.0], [330.0, 200.0]], &name), [1.0, 1.0]);
        // 30 px right of it: under its callsign, hidden
        assert_eq!(fades(&labels, &[[400.0, 200.0], [430.0, 200.0]], &name), [1.0, 0.0]);
        // 42 px right: the box ends 4 px after the text, at 441.9, so of the patch's columns (one
        // canvas pixel apart, at the half-resolution map's texels 220, 220, 221, 221, 222) the
        // last three are the second's own
        let f = fades(&labels, &[[400.0, 200.0], [442.0, 200.0]], &name);
        assert_close!(f[1], 0.6f64.powf(2.2), 12);
        // 16 px below: inside the box, which reaches 5.5 + 21.5 below the callsign's middle, 6 px
        // above the aircraft (so 21 px below it)
        assert_eq!(fades(&labels, &[[400.0, 200.0], [400.0, 216.0]], &name), [1.0, 0.0]);
        // 30 px below: clear (its own box covers the first aircraft, which is more important)
        assert_eq!(fades(&labels, &[[400.0, 200.0], [400.0, 230.0]], &name), [1.0, 1.0]);
        // the other way round: the second selected, the first loses
        let labels = labels_for(&[1, 2], Some(2));
        assert_eq!(fades(&labels, &[[400.0, 200.0], [360.0, 200.0]], &name), [0.0, 1.0]);
    }

    #[test]
    fn a_hidden_label_still_hides_less_important_ones() {
        let name = laid(4);
        // 1 (selected) hides 2, 40 px left; 2 (a larger priority than 3) hides 3, 40 px further
        // left, outside 1's box: in deck the hidden label's box still covers
        let flights = [flight(1, "SAS", 3_000, false), flight(2, "SAS", 3_000, false), flight(3, "SAS", 40_000, false)];
        let mut labeller = Labeller::new();
        labeller.update(1, flights.iter().map(|f| (f, [0.0; 3], 38.0)), true);
        let labels = labeller.labels(&frame_at(9.0), Some(1), 1.0);
        assert!(labels.priority(1) > labels.priority(2));
        assert_eq!(fades(&labels, &[[400.0, 200.0], [360.0, 200.0], [320.0, 200.0]], &name), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn between_equal_priorities_the_later_drawn_wins_and_glyphs_beat_boxes() {
        let name = laid(4);
        let labels = labels_for(&[1, 2], None);
        assert_eq!(labels.priority(0), labels.priority(1));
        // both on the same spot: the later row's box covers the earlier's, and its glyphs
        assert_eq!(fades(&labels, &[[400.0, 200.0], [400.0, 200.0]], &name), [0.0, 1.0]);
        // the second's aircraft under the first's callsign: the first's glyphs (drawn after every
        // box) win there, so the second loses its label though its own box came later; and the
        // second's box, drawn after the first's, covers the first aircraft: both lose theirs
        let f = fades(&labels, &[[400.0, 206.0], [420.0, 200.0]], &name);
        assert_eq!(f, [0.0, 0.0]);
        // with the first selected, it keeps its label and the second still loses
        let labels = labels_for(&[1, 2], Some(1));
        assert_eq!(fades(&labels, &[[400.0, 206.0], [420.0, 200.0]], &name), [1.0, 0.0]);
    }

    #[test]
    fn a_label_half_covered_fades_crisply() {
        let name = laid(4);
        let labels = labels_for(&[1, 2], Some(1));
        // the second's anchor on the left edge of the first's box: of the 5 × 5 patch, columns
        // left of the edge stay its own
        let [x0, ..] = labels.background(0, [400.0, 200.0], 1.0, &name);
        let f = fades(&labels, &[[400.0, 200.0], [x0, 200.0]], &name);
        assert!(f[1] > 0.0 && f[1] < 1.0, "{f:?}");
        // the crisp curve: nothing under 0.3, everything from 0.65
        assert_eq!((alpha(0.0), alpha(0.3), alpha(0.65), alpha(1.0)), (0.0, 0.0, 1.0, 1.0));
        assert_close!(alpha(0.475), 0.5, 12);
        assert_eq!(faded([241, 238, 231, 235], 1.0), [241, 238, 231, 235]);
        assert_eq!(faded([241, 238, 231, 235], 0.2), [241, 238, 231, 0]);
        assert_eq!(faded([241, 238, 231, 235], 0.4)[3], js_round(235.0 * alpha(0.4)) as u8);
        assert_eq!(faded([241, 238, 231, 235], 0.4)[3], 47);
        assert!(drawn(0.0001) && !drawn(0.00009));
    }

    #[test]
    fn a_label_off_the_screen_or_without_a_position_claims_nothing() {
        let name = laid(4);
        let labels = labels_for(&[1, 2], Some(1));
        let placings = [Placing { anchor: None, perspective: 1.0, name: &name, height: None }, Placing { anchor: Some([400.0, 200.0]), perspective: 1.0, name: &name, height: None }];
        assert_eq!(collision_fades(&mut CollisionMap::new(), CANVAS, 2.0, &labels, &placings), [0.0, 1.0]);
    }
}
