//! Label decluttering (docs 01 §5.9, 03 §7.5): the CPU stand-in for deck.gl's
//! `CollisionFilterExtension`.
//!
//! deck draws every label's collision footprint to an off-screen map in priority order and hides
//! a label whose anchor a more important label covers. Its `collision_fade` is how much of a 5 × 5
//! patch round the anchor the label holds, so it is spatial, not timed. The plan replaces that
//! (doc 00 §1.4.10, deviation 4) with a greedy pass per collision group: highest priority first,
//! a label is kept when its claimed rectangle meets no rectangle kept before it. Each label's fade
//! then eases towards shown or hidden over [`FADE_MS`], so labels still ease in and out as things
//! move.
//!
//! - A [`Group`] is one collision group (deck's `collisionGroup`): [`Group::labels`] for place
//!   names and airport codes (`"labels"`, G:749), [`Group::aircraft_labels`] for callsigns
//!   (`"aircraft-labels"`, G:781). Groups never hide one another's labels, and each keeps its own
//!   fades.
//! - [`claim`] builds a label's rectangle the way deck sizes its footprint in the collision pass:
//!   the text box at the test's size scale, moved by the pixel offset, grown by the background
//!   padding. Place labels are tested at [`LABELS_TEST_SCALE`]; aircraft labels at the label size
//!   setting, with the padding of doc 03 §7.5.
//! - [`Group::place`] runs at a scene refresh with the local clock's `now_ms`. The [`Placement`]
//!   it returns gives each label's alpha at any later frame without placing again, so fades stay
//!   smooth when refreshes are 150 or 500 ms apart (doc 00 §1.4.9).
//! - Alpha is the fade itself for place labels ([`Curve::Linear`], deck's `color.a *=
//!   collision_fade`), and `smoothstep(0.3, 0.65, fade)` for aircraft labels ([`Curve::Crisp`],
//!   G:788-803).
//!
//! Where this differs from deck, as the plan accepts:
//! - the whole claimed box is tested, not the patch round the anchor, so kept labels never
//!   overlap;
//! - only kept labels hide others (in deck a hidden label's footprint still covers);
//! - between two equal priorities the label already shown wins, then the one given first (deck:
//!   whichever its depth test lets through);
//! - a label seen for the first time is shown or hidden at once, as deck shows it; only a change of
//!   verdict fades.

use std::collections::HashMap;
use std::hash::Hash;

use rustradar_core::num::smoothstep;

/// How long a label takes to fade all the way in or out, ms.
pub const FADE_MS: f64 = 150.0;
/// Where the aircraft labels' "crisp" curve starts to show a label: alpha ×
/// `smoothstep(CRISP_FROM, CRISP_TO, fade)` (G:795).
pub const CRISP_FROM: f64 = 0.3;
/// Where the crisp curve reaches full alpha.
pub const CRISP_TO: f64 = 0.65;
/// Place labels claim 1.3 × their own size, whatever the label size setting
/// (`LABEL_COLLISION_PROPS`, G:727).
pub const LABELS_TEST_SCALE: f64 = 1.3;
/// Below this fade deck throws the label's vertices out of clip space: it is not drawn at all,
/// so not pickable either (`collision_fade < 0.0001`, `collision-filter/shader-module.js`).
pub const DRAWN_MIN_FADE: f64 = 0.0001;
/// No background padding.
pub const NO_PADDING: [f64; 4] = [0.0; 4];

/// A rectangle on the screen in pixels, y growing downwards: `x0 <= x1`, `y0 <= y1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub const fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Rect { x0, y0, x1, y1 }
    }

    /// From deck's `[x, y, width, height]` (the text layer's `getBoundingRect`, relative to the
    /// point the text is anchored at).
    pub fn from_xywh([x, y, w, h]: [f64; 4]) -> Self {
        Rect::new(x, y, x + w, y + h)
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    /// Whether the two share some area. Rectangles that only touch along an edge do not.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x0 < other.x1 && other.x0 < self.x1 && self.y0 < other.y1 && other.y0 < self.y1
    }

    /// The smallest rectangle holding both: a label of two lines claims both.
    pub fn union(&self, other: &Rect) -> Rect {
        Rect::new(self.x0.min(other.x0), self.y0.min(other.y0), self.x1.max(other.x1), self.y1.max(other.y1))
    }

    /// Finite, and not turned inside out. Only such a rectangle can be kept.
    pub fn is_valid(&self) -> bool {
        [self.x0, self.y0, self.x1, self.y1].iter().all(|v| v.is_finite()) && self.x0 <= self.x1 && self.y0 <= self.y1
    }
}

/// The rectangle a label claims in its collision group, as deck draws its footprint in the
/// collision pass (`text-background-layer-vertex.glsl`, `icon-layer-vertex.glsl`).
///
/// - `anchor`: the label's projected position, screen pixels.
/// - `offset`: deck's `getPixelOffset`, pixels; the size scale does not touch it.
/// - `text`: the text's box at size scale 1 (its `getSize` in pixels), relative to the point the
///   text is anchored at (E10's layout, deck's `getBoundingRect`).
/// - `scale`: the collision test's size scale (`collisionTestProps.sizeScale`):
///   [`LABELS_TEST_SCALE`] for place labels, the label size setting for aircraft labels.
/// - `padding`: `[left, top, right, bottom]` pixels round the scaled box (deck's
///   `backgroundPadding`); the size scale does not touch it either. [`NO_PADDING`] for a label
///   without a background.
pub fn claim(anchor: [f64; 2], offset: [f64; 2], text: Rect, scale: f64, padding: [f64; 4]) -> Rect {
    let [left, top, right, bottom] = padding;
    let x = anchor[0] + offset[0];
    let y = anchor[1] + offset[1];
    Rect::new(
        x + text.x0 * scale - left,
        y + text.y0 * scale - top,
        x + text.x1 * scale + right,
        y + text.y1 * scale + bottom,
    )
}

/// One label offered to a placement.
#[derive(Clone, Debug, PartialEq)]
pub struct Label<K = u64> {
    /// Stays the same while the same thing is labelled, so its fade carries over from one
    /// placement to the next (a flight id, a place's key). Unique within one placement.
    pub id: K,
    /// Higher wins (deck's `getCollisionPriority`, -1000 to 1000).
    pub priority: f64,
    /// See [`claim`].
    pub claim: Rect,
}

/// How a label's fade becomes its alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Curve {
    /// Alpha is the fade (deck's stock `color.a *= collision_fade`): place labels.
    #[default]
    Linear,
    /// `smoothstep(0.3, 0.65, fade)`: aircraft labels snap between shown and hidden, so a label
    /// on its way out does not linger as a faint ghost.
    Crisp,
}

impl Curve {
    /// The alpha factor for a fade of 0 (hidden) to 1 (shown).
    pub fn alpha(self, fade: f64) -> f64 {
        match self {
            Curve::Linear => fade,
            // GLSL's smoothstep(edge0, edge1, x) clamps (x - edge0) / (edge1 - edge0) to [0, 1]
            Curve::Crisp => smoothstep((fade - CRISP_FROM) / (CRISP_TO - CRISP_FROM)),
        }
    }
}

/// One label's fade: where it started easing from, when, and towards which end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    from: f64,
    since_ms: f64,
    shown: bool,
}

impl Fade {
    /// At rest: fully shown or fully hidden.
    fn settled(shown: bool, now_ms: f64) -> Self {
        Fade { from: if shown { 1.0 } else { 0.0 }, since_ms: now_ms, shown }
    }

    /// Turned towards `shown` at `now_ms`, from wherever it is then.
    fn toward(self, shown: bool, now_ms: f64) -> Self {
        if self.shown == shown {
            self
        } else {
            Fade { from: self.value(now_ms), since_ms: now_ms, shown }
        }
    }

    /// 0 (hidden) to 1 (shown) at `now_ms`, moving by 1 every [`FADE_MS`] towards the verdict.
    /// A clock that runs backwards (or NaN) holds the fade where it started.
    pub fn value(&self, now_ms: f64) -> f64 {
        // f64::max drops a NaN step
        let step = ((now_ms - self.since_ms) / FADE_MS).max(0.0);
        if self.shown { (self.from + step).min(1.0) } else { (self.from - step).max(0.0) }
    }

    /// The placement's verdict: kept, so fading in or shown.
    pub fn shown(&self) -> bool {
        self.shown
    }

    /// When the fade reaches its end, ms.
    pub fn settles_at(&self) -> f64 {
        let left = if self.shown { 1.0 - self.from } else { self.from };
        self.since_ms + left * FADE_MS
    }
}

/// The outcome of one [`Group::place`], indexed like the labels given to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placement {
    fades: Vec<Fade>,
    curve: Curve,
}

impl Placement {
    pub fn len(&self) -> usize {
        self.fades.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fades.is_empty()
    }

    /// Whether label `i` won its place.
    pub fn kept(&self, i: usize) -> bool {
        self.fades[i].shown
    }

    /// Label `i`'s fade at `now_ms`, 0 to 1.
    pub fn fade(&self, i: usize, now_ms: f64) -> f64 {
        self.fades[i].value(now_ms)
    }

    /// The factor label `i`'s alpha is multiplied by at `now_ms`, through the group's curve.
    pub fn alpha(&self, i: usize, now_ms: f64) -> f64 {
        self.curve.alpha(self.fade(i, now_ms))
    }

    /// Whether label `i` is drawn (and so pickable) at `now_ms`: deck drops a label whose fade is
    /// under [`DRAWN_MIN_FADE`].
    pub fn drawn(&self, i: usize, now_ms: f64) -> bool {
        self.fade(i, now_ms) >= DRAWN_MIN_FADE
    }

    /// Whether some fade is still running at `now_ms`: the frame loop keeps redrawing until none is.
    pub fn animating(&self, now_ms: f64) -> bool {
        self.fades.iter().any(|f| f.settles_at() > now_ms)
    }

    pub fn fades(&self) -> &[Fade] {
        &self.fades
    }

    pub fn curve(&self) -> Curve {
        self.curve
    }
}

/// One collision group: its curve and the fade of every label of its last placement.
#[derive(Clone, Debug)]
pub struct Group<K = u64> {
    curve: Curve,
    fades: HashMap<K, Fade>,
}

impl<K: Eq + Hash + Clone> Group<K> {
    pub fn new(curve: Curve) -> Self {
        Group { curve, fades: HashMap::new() }
    }

    /// The `"labels"` group: place names and airport codes, linear fade.
    pub fn labels() -> Self {
        Group::new(Curve::Linear)
    }

    /// The `"aircraft-labels"` group: callsigns and height lines, crisp fade.
    pub fn aircraft_labels() -> Self {
        Group::new(Curve::Crisp)
    }

    pub fn curve(&self) -> Curve {
        self.curve
    }

    /// Places `labels` at `now_ms`.
    ///
    /// Highest priority first (between equals: the label already shown, then the one given
    /// first), a label is kept when its claim overlaps no claim kept before it. A claim that is
    /// not [`Rect::is_valid`] is never kept and hides nothing. A label kept now and hidden before,
    /// or the other way round, starts fading from where its fade stands at `now_ms`; a label this
    /// group has not seen before starts at its end, fully shown or fully hidden. Labels missing
    /// from `labels` are forgotten: if one comes back, it is new again.
    pub fn place(&mut self, labels: &[Label<K>], now_ms: f64) -> Placement {
        let was_shown: Vec<bool> =
            labels.iter().map(|l| self.fades.get(&l.id).is_some_and(|f| f.shown)).collect();
        let mut order: Vec<usize> = (0..labels.len()).collect();
        // `+ 0.0` turns -0 into +0, which total_cmp would otherwise rank below it; the sort is
        // stable, so input order settles what is left
        order.sort_by(|&a, &b| {
            (labels[b].priority + 0.0)
                .total_cmp(&(labels[a].priority + 0.0))
                .then(was_shown[b].cmp(&was_shown[a]))
        });
        let mut kept = vec![false; labels.len()];
        let mut grid = Grid::default();
        for i in order {
            let claim = labels[i].claim;
            if claim.is_valid() && !grid.hits(&claim) {
                grid.insert(claim);
                kept[i] = true;
            }
        }
        let mut next = HashMap::with_capacity(labels.len());
        let fades = labels
            .iter()
            .zip(kept)
            .map(|(label, kept)| {
                let fade = match self.fades.get(&label.id) {
                    Some(old) => old.toward(kept, now_ms),
                    None => Fade::settled(kept, now_ms),
                };
                next.insert(label.id.clone(), fade);
                fade
            })
            .collect();
        self.fades = next;
        Placement { fades, curve: self.curve }
    }

    /// The fade of label `id` at `now_ms` as its last placement left it; 0 for a label that was
    /// not in it.
    pub fn fade(&self, id: &K, now_ms: f64) -> f64 {
        self.fades.get(id).map_or(0.0, |f| f.value(now_ms))
    }

    /// Forgets every label (labelling turned off, the label set emptied).
    pub fn clear(&mut self) {
        self.fades.clear();
    }
}

/// Side of a grid cell, pixels.
const CELL_PX: f64 = 64.0;
/// A claim spanning more cells than this goes on a list that every test checks instead.
const MAX_CELLS: i64 = 64;

/// The claims kept so far, bucketed by grid cell, so each test only meets its neighbours.
#[derive(Default)]
struct Grid {
    rects: Vec<Rect>,
    cells: HashMap<(i32, i32), Vec<u32>>,
    wide: Vec<u32>,
}

impl Grid {
    /// The cells `[x0, y0, x1, y1]` (inclusive) a valid rectangle touches, or `None` when there
    /// are more than [`MAX_CELLS`].
    fn span(r: &Rect) -> Option<[i32; 4]> {
        // `as` saturates, and the rectangle is finite
        let cell = |v: f64| (v / CELL_PX).floor() as i32;
        let [x0, y0, x1, y1] = [cell(r.x0), cell(r.y0), cell(r.x1), cell(r.y1)];
        let count = (x1 as i64 - x0 as i64 + 1) * (y1 as i64 - y0 as i64 + 1);
        (count <= MAX_CELLS).then_some([x0, y0, x1, y1])
    }

    fn hits(&self, r: &Rect) -> bool {
        let overlaps = |i: &u32| self.rects[*i as usize].overlaps(r);
        if self.wide.iter().any(overlaps) {
            return true;
        }
        match Grid::span(r) {
            Some([x0, y0, x1, y1]) => (x0..=x1).any(|x| {
                (y0..=y1).any(|y| self.cells.get(&(x, y)).is_some_and(|ids| ids.iter().any(overlaps)))
            }),
            None => self.rects.iter().any(|k| k.overlaps(r)),
        }
    }

    fn insert(&mut self, r: Rect) {
        let i = self.rects.len() as u32;
        self.rects.push(r);
        match Grid::span(&r) {
            Some([x0, y0, x1, y1]) => {
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        self.cells.entry((x, y)).or_default().push(i);
                    }
                }
            }
            None => self.wide.push(i),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A label whose claim is the `w` × `h` box centred on `[x, y]`.
    fn label(id: u64, priority: f64, [x, y]: [f64; 2], [w, h]: [f64; 2]) -> Label {
        Label { id, priority, claim: Rect::new(x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0) }
    }

    fn kept(p: &Placement) -> Vec<bool> {
        (0..p.len()).map(|i| p.kept(i)).collect()
    }

    #[test]
    fn the_higher_priority_wins_an_overlap() {
        let low = label(1, 100.0, [100.0, 100.0], [60.0, 14.0]);
        let high = label(2, 800.0, [130.0, 105.0], [60.0, 14.0]);
        // whichever order they come in
        for labels in [vec![low.clone(), high.clone()], vec![high.clone(), low.clone()]] {
            let mut group = Group::labels();
            let p = group.place(&labels, 0.0);
            for (i, l) in labels.iter().enumerate() {
                assert_eq!(p.kept(i), l.id == 2, "label {}", l.id);
            }
        }
        // a negative priority loses to zero, and the lowest of a chain is placed against what was
        // kept: 3 hides 4, and 5, which only 4 covered, stays
        let mut group = Group::labels();
        let chain = [
            label(5, -10.0, [200.0, 0.0], [60.0, 14.0]),
            label(4, 0.0, [150.0, 0.0], [60.0, 14.0]),
            label(3, 450.0, [100.0, 0.0], [60.0, 14.0]),
        ];
        assert_eq!(kept(&group.place(&chain, 0.0)), [true, false, true]);
        // the selected aircraft's label outranks any other
        let mut group = Group::aircraft_labels();
        let p = group.place(
            &[
                label(7, 450.0, [0.0, 0.0], [80.0, 30.0]),
                label(8, rustradar_core::labels::SELECTED_LABEL_PRIORITY, [10.0, 10.0], [80.0, 30.0]),
            ],
            0.0,
        );
        assert_eq!(kept(&p), [false, true]);
    }

    #[test]
    fn groups_do_not_affect_each_other() {
        // the same ids and the same boxes in both groups
        let a = label(1, 900.0, [100.0, 100.0], [80.0, 20.0]);
        let b = label(2, 100.0, [110.0, 100.0], [80.0, 20.0]);
        let mut places = Group::labels();
        let mut aircraft = Group::aircraft_labels();
        assert_eq!(kept(&places.place(std::slice::from_ref(&a), 0.0)), [true]);
        assert_eq!(kept(&aircraft.place(std::slice::from_ref(&b), 0.0)), [true]);
        // b, alone in its group, is never hidden by a in the other
        let p = places.place(std::slice::from_ref(&a), 50.0);
        let q = aircraft.place(std::slice::from_ref(&b), 50.0);
        assert!(p.kept(0) && q.kept(0));
        assert_eq!(q.alpha(0, 50.0), 1.0);
        // within one group the same pair collides; the other group's fades stay where they were
        let p = places.place(&[a.clone(), b.clone()], 100.0);
        assert_eq!(kept(&p), [true, false]);
        assert_eq!(places.fade(&2, 100.0), 0.0);
        assert_eq!(aircraft.fade(&2, 100.0), 1.0);
        assert_eq!(aircraft.fade(&1, 100.0), 0.0);
        let q = aircraft.place(&[a, b], 100.0);
        // in this group b was shown and a is new: a still wins on priority, and b fades out here
        assert_eq!(kept(&q), [true, false]);
        assert_eq!(q.fade(1, 100.0), 1.0);
        assert_eq!(places.fade(&1, 175.0), 1.0);
    }

    #[test]
    fn a_label_that_loses_fades_out_over_150_ms() {
        let mut group = Group::labels();
        let shown = label(1, 100.0, [100.0, 100.0], [60.0, 14.0]);
        let p = group.place(std::slice::from_ref(&shown), 1000.0);
        assert!(p.kept(0));
        assert_eq!(p.alpha(0, 1000.0), 1.0);
        assert!(!p.animating(1000.0));
        // something more important arrives on top of it
        let p = group.place(&[shown, label(2, 900.0, [110.0, 100.0], [60.0, 14.0])], 2000.0);
        assert!(!p.kept(0) && p.kept(1));
        // the newcomer shows at once; the loser eases out, linearly for place labels
        assert_eq!(p.alpha(1, 2000.0), 1.0);
        let steps = [(1990.0, 1.0), (2000.0, 1.0), (2030.0, 0.8), (2075.0, 0.5), (2120.0, 0.2), (2150.0, 0.0), (2400.0, 0.0)];
        for (t, fade) in steps {
            assert!((p.fade(0, t) - fade).abs() < 1e-12, "at {t}: {} != {fade}", p.fade(0, t));
            assert_eq!(p.alpha(0, t), p.fade(0, t));
        }
        assert!(p.drawn(0, 2149.0));
        assert!(!p.drawn(0, 2150.0));
        assert!(p.animating(2149.0));
        assert!(!p.animating(2150.0));
        assert_eq!(p.fades()[0].settles_at(), 2150.0);
        // the group answers the same between placements
        assert!((group.fade(&1, 2075.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_label_turned_round_mid_fade_eases_back_from_where_it_was() {
        let mut group = Group::labels();
        let a = label(1, 100.0, [0.0, 0.0], [60.0, 14.0]);
        let b = label(2, 900.0, [20.0, 0.0], [60.0, 14.0]);
        group.place(std::slice::from_ref(&a), 0.0);
        let p = group.place(&[a.clone(), b], 1000.0);
        assert!((p.fade(0, 1060.0) - 0.6).abs() < 1e-12);
        // the cover moves away at 1060: back in from 0.6, done 60 ms later
        let p = group.place(std::slice::from_ref(&a), 1060.0);
        assert!(p.kept(0));
        assert!((p.fade(0, 1060.0) - 0.6).abs() < 1e-12);
        assert!((p.fade(0, 1090.0) - 0.8).abs() < 1e-12);
        assert!((p.fades()[0].settles_at() - 1120.0).abs() < 1e-9);
        assert_eq!(p.fade(0, 1200.0), 1.0);
        // placing again with the same verdict leaves a running fade alone
        let p = group.place(std::slice::from_ref(&a), 1090.0);
        assert!((p.fade(0, 1090.0) - 0.8).abs() < 1e-12);
    }

    #[test]
    fn a_new_label_shows_or_hides_at_once() {
        let mut group = Group::labels();
        let p = group.place(
            &[label(1, 800.0, [0.0, 0.0], [50.0, 14.0]), label(2, 400.0, [10.0, 5.0], [50.0, 14.0])],
            500.0,
        );
        assert_eq!((p.fade(0, 500.0), p.fade(1, 500.0)), (1.0, 0.0));
        assert!(!p.drawn(1, 500.0));
        assert!(!p.animating(500.0));
        // one that left the input is forgotten: when it comes back losing, it is hidden at once
        group.place(&[label(1, 800.0, [0.0, 0.0], [50.0, 14.0])], 600.0);
        assert_eq!(group.fade(&2, 600.0), 0.0);
        let pair = [label(3, 900.0, [0.0, 0.0], [50.0, 14.0]), label(1, 800.0, [0.0, 0.0], [50.0, 14.0])];
        let p = group.place(&pair, 700.0);
        assert!(p.kept(0) && !p.kept(1));
        assert_eq!(p.fade(1, 700.0), 1.0, "1 was shown, so it fades");
        group.clear();
        let p = group.place(&pair, 710.0);
        assert_eq!(p.fade(1, 710.0), 0.0, "after clear 1 is new");
    }

    #[test]
    fn aircraft_labels_fade_through_the_crisp_curve() {
        assert_eq!(Curve::Crisp.alpha(0.0), 0.0);
        assert_eq!(Curve::Crisp.alpha(0.3), 0.0);
        assert_eq!(Curve::Crisp.alpha(0.65), 1.0);
        assert_eq!(Curve::Crisp.alpha(1.0), 1.0);
        // GLSL: t = (0.475 - 0.3) / 0.35 = 0.5, t·t·(3 − 2t) = 0.5
        assert!((Curve::Crisp.alpha(0.475) - 0.5).abs() < 1e-12);
        let t: f64 = (0.4 - 0.3) / (0.65 - 0.3);
        assert_eq!(Curve::Crisp.alpha(0.4), t * t * (3.0 - 2.0 * t));
        let mut last = 0.0;
        for i in 0..=100 {
            let a = Curve::Crisp.alpha(f64::from(i) / 100.0);
            assert!(a >= last);
            last = a;
        }
        assert_eq!(Curve::Linear.alpha(0.42), 0.42);
        // in the group: losing at 0, the fade is 0.65 at 52.5 ms and 0.3 at 105 ms
        let mut group = Group::aircraft_labels();
        let a = label(1, 0.0, [0.0, 0.0], [100.0, 40.0]);
        group.place(std::slice::from_ref(&a), -10.0);
        let p = group.place(&[a, label(2, 1000.0, [0.0, 0.0], [100.0, 40.0])], 0.0);
        assert_eq!(p.curve(), Curve::Crisp);
        assert_eq!(p.alpha(0, 30.0), 1.0);
        assert!((p.fade(0, 52.5) - 0.65).abs() < 1e-12);
        assert!((p.alpha(0, 52.5) - 1.0).abs() < 1e-9);
        assert!((p.alpha(0, 78.75) - 0.5).abs() < 1e-9);
        assert!(p.alpha(0, 105.0) < 1e-9);
        assert_eq!(p.alpha(0, 120.0), 0.0);
        // invisible, but deck still draws it until the fade itself is gone
        assert!(p.drawn(0, 120.0));
        assert!(!p.drawn(0, 150.0));
    }

    #[test]
    fn claim_scales_the_text_box_and_adds_the_offset_and_padding() {
        // a centred 40 × 12 px text (deck's getBoundingRect for middle / center) tested at 1.3
        let text = Rect::from_xywh([-20.0, -6.0, 40.0, 12.0]);
        assert_eq!((text.width(), text.height()), (40.0, 12.0));
        let r = claim([100.0, 50.0], [0.0, 0.0], text, LABELS_TEST_SCALE, NO_PADDING);
        let want = Rect::new(74.0, 42.2, 126.0, 57.8);
        for (got, want) in [(r.x0, want.x0), (r.y0, want.y0), (r.x1, want.x1), (r.y1, want.y1)] {
            assert!((got - want).abs() < 1e-12, "{r:?}");
        }
        // anchored at its start, moved right and up by the pixel offset, padded on every side;
        // neither the offset nor the padding grows with the scale
        let text = Rect::from_xywh([0.0, -5.5, 50.0, 11.0]);
        let r = claim([200.0, 300.0], [12.0, -6.0], text, 2.0, [70.0, 20.0, 4.0, 21.0]);
        assert_eq!(r, Rect::new(212.0 - 70.0, 294.0 - 11.0 - 20.0, 212.0 + 100.0 + 4.0, 294.0 + 11.0 + 21.0));
        // two lines claim both
        let height_line = claim([200.0, 300.0], [12.0, 6.0], Rect::from_xywh([0.0, -5.0, 30.0, 10.0]), 1.0, NO_PADDING);
        let name_line = claim([200.0, 300.0], [12.0, -6.0], Rect::from_xywh([0.0, -5.5, 50.0, 11.0]), 1.0, NO_PADDING);
        assert_eq!(name_line.union(&height_line), Rect::new(212.0, 288.5, 262.0, 311.0));
    }

    #[test]
    fn touching_boxes_do_not_collide() {
        let mut group = Group::labels();
        let p = group.place(
            &[
                Label { id: 1, priority: 2.0, claim: Rect::new(0.0, 0.0, 10.0, 10.0) },
                Label { id: 2, priority: 1.0, claim: Rect::new(10.0, 0.0, 20.0, 10.0) },
                Label { id: 3, priority: 1.0, claim: Rect::new(0.0, 10.0, 10.0, 20.0) },
                Label { id: 4, priority: 0.0, claim: Rect::new(9.999, 9.999, 10.001, 10.001) },
            ],
            0.0,
        );
        assert_eq!(kept(&p), [true, true, true, false]);
    }

    #[test]
    fn between_equal_priorities_the_label_already_shown_stays() {
        let mut group = Group::labels();
        let a = label(1, 500.0, [0.0, 0.0], [60.0, 14.0]);
        let b = label(2, 500.0, [30.0, 0.0], [60.0, 14.0]);
        // first come, first kept
        assert_eq!(kept(&group.place(&[a.clone(), b.clone()], 0.0)), [true, false]);
        // b first now, but a holds its place, so it does not flicker
        assert_eq!(kept(&group.place(&[b.clone(), a.clone()], 50.0)), [false, true]);
        // -0 and +0 tie too
        let mut group = Group::labels();
        let p = group.place(&[Label { priority: -0.0, ..a.clone() }, Label { priority: 0.0, ..b.clone() }], 0.0);
        assert_eq!(kept(&p), [true, false]);
        // a higher priority still beats a label already shown
        let p = group.place(&[Label { priority: -0.0, ..a }, Label { priority: 1.0, ..b }], 10.0);
        assert_eq!(kept(&p), [false, true]);
    }

    #[test]
    fn a_claim_that_is_not_valid_is_never_kept_and_hides_nothing() {
        let mut group = Group::labels();
        let p = group.place(
            &[
                Label { id: 1, priority: 900.0, claim: Rect::new(f64::NAN, 0.0, 10.0, 10.0) },
                Label { id: 2, priority: 800.0, claim: Rect::new(20.0, 0.0, -20.0, 10.0) },
                Label { id: 3, priority: 700.0, claim: Rect::new(0.0, 0.0, f64::INFINITY, 10.0) },
                Label { id: 4, priority: 600.0, claim: Rect::new(0.0, 0.0, 10.0, 10.0) },
            ],
            0.0,
        );
        assert_eq!(kept(&p), [false, false, false, true]);
        assert!(Rect::new(5.0, 5.0, 5.0, 5.0).is_valid());
        assert!(!Rect::new(0.0, 1.0, 0.0, 0.0).is_valid());
    }

    #[test]
    fn a_clock_running_backwards_holds_the_fade() {
        let mut group = Group::labels();
        let a = label(1, 1.0, [0.0, 0.0], [10.0, 10.0]);
        group.place(std::slice::from_ref(&a), 0.0);
        let p = group.place(&[a, label(2, 2.0, [0.0, 0.0], [10.0, 10.0])], 100.0);
        assert_eq!(p.fade(0, 50.0), 1.0);
        assert_eq!(p.fade(0, f64::NAN), 1.0);
    }

    #[test]
    fn the_grid_agrees_with_testing_every_kept_label() {
        // a small xorshift, so the labels are the same on every run
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut labels = Vec::new();
        for id in 0..3000 {
            let x = next() * 2400.0 - 600.0;
            let y = next() * 1600.0 - 400.0;
            // mostly label sized; a few wide enough for the wide list, a few on cell edges
            let (w, h) = if id % 97 == 0 {
                (next() * 2000.0, next() * 900.0)
            } else {
                (next() * 180.0, 8.0 + next() * 60.0)
            };
            let on_edge = |v: f64| if id % 31 == 0 { (v / CELL_PX).round() * CELL_PX } else { v };
            let (x, y) = (on_edge(x), on_edge(y));
            let priority = (next() * 20.0).floor() * 50.0 - 450.0;
            labels.push(Label { id, priority, claim: Rect::new(x, y, x + w, y + h) });
        }
        // the plain greedy pass this module stands for
        let mut order: Vec<usize> = (0..labels.len()).collect();
        order.sort_by(|&a, &b| labels[b].priority.total_cmp(&labels[a].priority));
        let mut want = vec![false; labels.len()];
        let mut placed: Vec<Rect> = Vec::new();
        for i in order {
            if !placed.iter().any(|r| r.overlaps(&labels[i].claim)) {
                placed.push(labels[i].claim);
                want[i] = true;
            }
        }
        let p = Group::labels().place(&labels, 0.0);
        assert_eq!(kept(&p), want);
        let count = want.iter().filter(|&&k| k).count();
        assert!(count > 50 && count < 2000, "{count} kept");
        // and no two kept claims overlap
        for (i, a) in placed.iter().enumerate() {
            for b in &placed[i + 1..] {
                assert!(!a.overlaps(b));
            }
        }
    }
}
