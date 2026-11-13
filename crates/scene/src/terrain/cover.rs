//! The 3D terrain's cover textures, CPU side (item Z2; doc 04 §14.3, §14.5; plan 1.4.12): how big
//! each terrain tile's cover is, which covers are kept within the memory budget, which are drawn
//! again when what is draped over them changes, and in what order, a few each frame.
//!
//! Today's app drapes the map's flat layers over the 3D surface with deck.gl's `TerrainExtension`:
//! `TerrainEffect` keeps one `TerrainCover` (a framebuffer) per terrain tile, into which every
//! draped layer is drawn from straight above (`TerrainPass.renderTerrainCover`), and the tile's
//! mesh samples it. What this module ports, and what it adds:
//!
//! - **The size** ([`cover_zoom`], [`cover_size`]): `TerrainCover._updateViewport` with today's
//!   patch (`patches/@deck.gl%2Fextensions@9.4.0.patch`): a tile's texture is drawn at its own
//!   zoom plus one (1024 texels across a 512 px tile, whatever its distance), the layer's deepest
//!   tiles at the view's zoom rounded when that is deeper; `makeViewport` caps the side at 2048
//!   ([`MAX_COVER_SIZE`]). A tile's size is fixed when its cover is first made, as deck makes the
//!   render viewport once for a tile (`if (this.tile) …`: only the first update redraws).
//! - **What makes a cover draw again** ([`CoverCache::invalidate`], [`CoverCache::invalidate_all`]):
//!   deck redraws a cover when the list of draped sublayers whose tiles meet it changes
//!   (`_updateLayers` with `getIntersectingLayers`), and every cover when a draped layer that is
//!   not tiled changes (`layerNeedsRedraw`). Two tiles' squares meet when one holds the other.
//! - **The budget** ([`CoverCache::plan`], plan 1.4.12): deck has none and keeps a cover for every
//!   terrain tile it caches. Here the covers' texels stay within [`BUDGET`] (256 MB on desktop, 96
//!   MB on phones): covers of tiles no longer drawn go first, the longest unseen first; then the
//!   covers drawn the finest for their size on the screen ([`Want::weight`]) are halved, down to
//!   [`MIN_COVER_SIZE`]; a cover cut down grows back once there is room again. Sizes change only
//!   when that makes room or uses it, so they settle while the view does.
//! - **A few a frame** ([`CoverCache::next`], plan 1.4.4): at most [`COVERS_PER_FRAME`] covers are
//!   drawn in a frame, those never drawn first (their tiles show the bare mesh colour until then),
//!   then those out of date (they show their last picture until then), the largest on the screen
//!   first. deck draws every cover that needs it in the frame it notices.

use std::collections::HashMap;

use rustradar_core::num::js_round;

use crate::tiles::TileIndex;

/// deck's `MAX_VIEWPORT_SIZE` (`projection-utils.js`): no cover is wider, texels.
pub const MAX_COVER_SIZE: u32 = 2048;
/// The smallest side a cover is cut down to for the budget, texels.
pub const MIN_COVER_SIZE: u32 = 64;
/// Covers drawn (or drawn again) in one frame (plan 1.4.4).
pub const COVERS_PER_FRAME: usize = 2;
/// The covers' texels on desktop, bytes (plan 1.4.12).
pub const DESKTOP_BUDGET: usize = 256 * 1024 * 1024;
/// The covers' texels on a phone, bytes (plan 1.4.12).
pub const PHONE_BUDGET: usize = 96 * 1024 * 1024;
/// The budget of this build: [`PHONE_BUDGET`] on Android and iOS, else [`DESKTOP_BUDGET`].
pub const BUDGET: usize = if cfg!(any(target_os = "android", target_os = "ios")) { PHONE_BUDGET } else { DESKTOP_BUDGET };
/// Bytes a texel (`Rgba8Unorm`).
pub const BYTES_PER_TEXEL: usize = 4;
/// The common units a tile spans at zoom 0 (deck's 512-unit world).
const WORLD_SIZE: f64 = 512.0;

/// The zoom a terrain tile's cover is drawn at (`TerrainCover._updateViewport`, today's patch):
/// `tileZoom < maxZoom ? tileZoom + 1 : max(tileZoom + 1, round(viewport.zoom))`, `max_zoom` the
/// terrain layer's deepest zoom (16).
pub fn cover_zoom(tile_z: u32, max_zoom: u32, view_zoom: f64) -> f64 {
    let next = f64::from(tile_z) + 1.0;
    if tile_z < max_zoom { next } else { next.max(js_round(view_zoom)) }
}

/// A cover's side in texels (`makeViewport` given a zoom): the tile's `512 / 2^z` common units at
/// `2^zoom` texels a unit, rounded, and if that is wider than [`MAX_COVER_SIZE`] scaled down to it.
pub fn cover_size(tile_z: u32, zoom: f64) -> u32 {
    let extent = WORLD_SIZE / 2f64.powi(tile_z as i32);
    let width = js_round(extent * 2f64.powf(zoom));
    let max = f64::from(MAX_COVER_SIZE);
    let width = if width > max { js_round(width * (max / width)) } else { width };
    width.max(1.0) as u32
}

/// The bytes a cover of `size` texels a side holds.
pub fn cover_bytes(size: u32) -> usize {
    size as usize * size as usize * BYTES_PER_TEXEL
}

/// Whether two tiles' squares meet: in the quadtree one holds the other, or they do not meet.
pub fn tiles_meet(a: TileIndex, b: TileIndex) -> bool {
    a.contains(b) || b.contains(a)
}

/// A terrain tile drawn this frame, and the cover it would have.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Want {
    pub tile: TileIndex,
    /// The side its cover is made at ([`cover_size`]).
    pub size: u32,
    /// How large it is on the screen (its extent over its distance from the camera): of two covers
    /// of one size the one with the smaller weight is drawn finer for what it shows.
    pub weight: f64,
}

/// Where a cover's picture stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverState {
    /// Not drawn yet (made, or made again at another size): the tile shows the mesh colour.
    Fresh,
    /// Drawn and up to date.
    Drawn,
    /// Drawn, but something draped over it has changed since.
    Stale,
}

/// One terrain tile's cover.
#[derive(Debug, Clone, PartialEq)]
pub struct Cover {
    /// The side it was made for ([`Want::size`] when first made), texels.
    pub wanted: u32,
    /// The side it has: `wanted`, or less for the budget; 0 for none.
    pub size: u32,
    pub state: CoverState,
    /// Its tile's [`Want::weight`] the last time it was drawn.
    pub weight: f64,
    /// The plan it was last drawn in.
    seen: u64,
    /// Drawn in the last plan.
    visible: bool,
}

impl Cover {
    /// Whether its tile was drawn in the last plan.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Whether it waits to be drawn: its tile is drawn, it has texels, its picture is not current.
    pub fn waiting(&self) -> bool {
        self.visible && self.size > 0 && self.state != CoverState::Drawn
    }
}

/// What the GPU side does after a [`CoverCache::plan`]: textures to let go and textures to make
/// (a tile in both is made again at its new size).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub free: Vec<TileIndex>,
    /// Tiles and the sides to make their textures at.
    pub allocate: Vec<(TileIndex, u32)>,
}

/// The covers kept: their sizes, states and the budget they share.
#[derive(Debug, Clone)]
pub struct CoverCache {
    covers: HashMap<TileIndex, Cover>,
    budget: usize,
    clock: u64,
}

impl CoverCache {
    /// No covers, within `budget` bytes ([`BUDGET`] in the app).
    pub fn new(budget: usize) -> Self {
        CoverCache { covers: HashMap::new(), budget, clock: 0 }
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// From the next [`Self::plan`] on, the covers fit `budget` bytes.
    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
    }

    /// The bytes the covers' textures hold now: never more than the budget after a plan.
    pub fn bytes(&self) -> usize {
        self.covers.values().map(|c| cover_bytes(c.size)).sum()
    }

    pub fn get(&self, tile: TileIndex) -> Option<&Cover> {
        self.covers.get(&tile)
    }

    pub fn len(&self) -> usize {
        self.covers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.covers.is_empty()
    }

    /// Every cover, in no order.
    pub fn iter(&self) -> impl Iterator<Item = (TileIndex, &Cover)> {
        self.covers.iter().map(|(&t, c)| (t, c))
    }

    /// The covers whose tiles are drawn and whose pictures are not current ([`Cover::waiting`]).
    pub fn waiting(&self) -> usize {
        self.covers.values().filter(|c| c.waiting()).count()
    }

    /// Forgets every cover (the terrain turned off: deck finalizes the layer and its covers); the
    /// tiles whose textures go.
    pub fn clear(&mut self) -> Vec<TileIndex> {
        let mut freed: Vec<TileIndex> = self.covers.iter().filter(|(_, c)| c.size > 0).map(|(&t, _)| t).collect();
        freed.sort_by_key(|t| (t.z, t.x, t.y));
        self.covers.clear();
        freed
    }

    /// This frame's terrain tiles `drawn` (each with the cover it would have); `kept` says whether
    /// the terrain still caches a tile not drawn (its cover is forgotten when not). Returns what to
    /// change on the GPU so that every drawn tile has a cover and all of them fit the budget.
    pub fn plan(&mut self, drawn: &[Want], kept: impl Fn(TileIndex) -> bool) -> Plan {
        self.clock += 1;
        let clock = self.clock;
        for cover in self.covers.values_mut() {
            cover.visible = false;
        }
        for want in drawn {
            let cover = self.covers.entry(want.tile).or_insert(Cover {
                wanted: want.size,
                size: 0,
                state: CoverState::Fresh,
                weight: want.weight,
                seen: clock,
                visible: true,
            });
            cover.weight = want.weight;
            cover.seen = clock;
            cover.visible = true;
        }
        let mut plan = Plan::default();
        // the covers of tiles the terrain has let go of
        let mut gone: Vec<TileIndex> = self.covers.iter().filter(|(t, c)| !c.visible && !kept(**t)).map(|(&t, _)| t).collect();
        gone.sort_by_key(|t| (t.z, t.x, t.y));
        for tile in gone {
            if self.covers.remove(&tile).is_some_and(|c| c.size > 0) {
                plan.free.push(tile);
            }
        }

        // what each drawn tile's cover would hold: what it has, or what it wants when it has none
        let mut targets: Vec<(TileIndex, u32, f64, u32)> = drawn
            .iter()
            .filter_map(|w| {
                let c = self.covers.get(&w.tile)?;
                Some((w.tile, if c.size > 0 { c.size } else { c.wanted }, c.weight.max(f64::MIN_POSITIVE), c.wanted))
            })
            .collect();
        // (one tile drawn twice in the list counts once)
        targets.sort_by_key(|&(t, ..)| (t.z, t.x, t.y));
        targets.dedup_by_key(|&mut (t, ..)| t);
        let idle_bytes: usize = self.covers.values().filter(|c| !c.visible).map(|c| cover_bytes(c.size)).sum();
        let mut total = idle_bytes + targets.iter().map(|&(_, s, ..)| cover_bytes(s)).sum::<usize>();

        // room first from the covers not drawn, the longest unseen first
        if total > self.budget {
            let mut idle: Vec<(u64, TileIndex)> = self.covers.iter().filter(|(_, c)| !c.visible && c.size > 0).map(|(&t, c)| (c.seen, t)).collect();
            idle.sort_by_key(|&(seen, t)| (seen, t.z, t.x, t.y));
            for (_, tile) in idle {
                if total <= self.budget {
                    break;
                }
                if let Some(cover) = self.covers.remove(&tile) {
                    total -= cover_bytes(cover.size);
                    plan.free.push(tile);
                }
            }
        }
        // then halve the covers drawn finest for their size on the screen
        let oversampled = |&(_, size, weight, _): &(TileIndex, u32, f64, u32)| f64::from(size) / weight;
        while total > self.budget {
            let Some(i) = argmax(&targets, |t| (t.1 > MIN_COVER_SIZE).then(|| oversampled(t))) else { break };
            let size = targets[i].1;
            total -= cover_bytes(size) - cover_bytes(size / 2);
            targets[i].1 = size / 2;
        }
        // and if that is not room enough, the smallest on the screen go without
        while total > self.budget {
            let Some(i) = argmax(&targets, |t| (t.1 > 0).then(|| -t.2)) else { break };
            total -= cover_bytes(targets[i].1);
            targets[i].1 = 0;
        }
        // what was cut grows back where there is room, the coarsest for its size on the screen first
        loop {
            let mut order: Vec<usize> = (0..targets.len()).filter(|&i| targets[i].1 < targets[i].3).collect();
            order.sort_by(|&a, &b| oversampled(&targets[a]).total_cmp(&oversampled(&targets[b])).then(targets[a].0.cmp_key(targets[b].0)));
            let grown = order.into_iter().find_map(|i| {
                let size = targets[i].1;
                let next = if size == 0 { MIN_COVER_SIZE.min(targets[i].3) } else { (size * 2).min(targets[i].3) };
                let more = cover_bytes(next) - cover_bytes(size);
                (total + more <= self.budget).then_some((i, next, more))
            });
            let Some((i, next, more)) = grown else { break };
            targets[i].1 = next;
            total += more;
        }

        for (tile, target, ..) in targets {
            let Some(cover) = self.covers.get_mut(&tile) else { continue };
            if cover.size == target {
                continue;
            }
            if cover.size > 0 {
                plan.free.push(tile);
            }
            if target > 0 {
                plan.allocate.push((tile, target));
            }
            cover.size = target;
            cover.state = CoverState::Fresh;
        }
        plan
    }

    /// Something draped over the squares of `changed` tiles (of any layer) changed: the covers they
    /// meet are out of date.
    pub fn invalidate(&mut self, changed: &[TileIndex]) {
        if changed.is_empty() {
            return;
        }
        for (&tile, cover) in &mut self.covers {
            if cover.state == CoverState::Drawn && changed.iter().any(|&c| tiles_meet(tile, c)) {
                cover.state = CoverState::Stale;
            }
        }
    }

    /// Something draped everywhere changed: every cover is out of date.
    pub fn invalidate_all(&mut self) {
        for cover in self.covers.values_mut() {
            if cover.state == CoverState::Drawn {
                cover.state = CoverState::Stale;
            }
        }
    }

    /// The covers to draw now, at most `max`: the drawn tiles' covers never drawn, then those out of
    /// date, the largest on the screen first. They count as drawn from here.
    pub fn next(&mut self, max: usize) -> Vec<TileIndex> {
        let mut waiting: Vec<(TileIndex, &Cover)> = self.covers.iter().filter(|(_, c)| c.waiting()).map(|(&t, c)| (t, c)).collect();
        waiting.sort_by(|(ta, a), (tb, b)| {
            (a.state != CoverState::Fresh).cmp(&(b.state != CoverState::Fresh)).then(b.weight.total_cmp(&a.weight)).then(ta.cmp_key(*tb))
        });
        let chosen: Vec<TileIndex> = waiting.into_iter().take(max).map(|(t, _)| t).collect();
        for tile in &chosen {
            if let Some(cover) = self.covers.get_mut(tile) {
                cover.state = CoverState::Drawn;
            }
        }
        chosen
    }
}

/// The index whose `key` is largest (`None` keys skipped; ties to the first).
fn argmax<T>(items: &[T], key: impl Fn(&T) -> Option<f64>) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (i, item) in items.iter().enumerate() {
        if let Some(k) = key(item)
            && best.is_none_or(|(_, b)| k > b)
        {
            best = Some((i, k));
        }
    }
    best.map(|(i, _)| i)
}

/// A total order on tiles for ties.
trait TileOrder {
    fn cmp_key(self, other: Self) -> std::cmp::Ordering;
}

impl TileOrder for TileIndex {
    fn cmp_key(self, other: Self) -> std::cmp::Ordering {
        (self.z, self.x, self.y).cmp(&(other.z, other.x, other.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1024 * 1024;

    fn want(x: u32, weight: f64) -> Want {
        Want { tile: TileIndex::new(x, 0, 14), size: 1024, weight }
    }

    /// Today's patch: a tile's cover one zoom deeper than the tile, 1024 texels for every tile but
    /// the deepest, which follow the view's zoom (rounded as `Math.round` rounds) when it is deeper,
    /// to deck's 2048 at most.
    #[test]
    fn a_cover_is_a_zoom_deeper_than_its_tile() {
        assert_eq!(cover_zoom(12, 16, 15.2), 13.0);
        assert_eq!(cover_size(12, 13.0), 1024);
        for z in 0..16 {
            assert_eq!(cover_size(z, cover_zoom(z, 16, 16.0)), 1024, "zoom {z}");
        }
        // the deepest tiles: zoom 17 until the view rounds past it
        assert_eq!(cover_zoom(16, 16, 16.4), 17.0);
        assert_eq!(cover_zoom(16, 16, 17.49), 17.0);
        assert_eq!(cover_zoom(16, 16, 17.5), 18.0);
        assert_eq!(cover_size(16, 17.0), 1024);
        assert_eq!(cover_size(16, 18.0), 2048);
        // past 2048 the viewport is scaled down to it
        assert_eq!(cover_size(16, 19.0), MAX_COVER_SIZE);
        assert_eq!(cover_size(16, 20.0), MAX_COVER_SIZE);
        assert_eq!(cover_size(3, 3.5), 724);
        assert_eq!(cover_bytes(1024), 4 * MIB);
    }

    /// The budgets of the plan.
    #[test]
    fn the_budgets_are_the_plans() {
        assert_eq!((DESKTOP_BUDGET, PHONE_BUDGET), (256 * MIB, 96 * MIB));
        assert_eq!(BUDGET, if cfg!(any(target_os = "android", target_os = "ios")) { PHONE_BUDGET } else { DESKTOP_BUDGET });
        assert_eq!(COVERS_PER_FRAME, 2);
    }

    /// Only tiles one inside the other meet.
    #[test]
    fn tiles_meet_when_one_holds_the_other() {
        let tile = TileIndex::new(4, 6, 4);
        assert!(tiles_meet(tile, tile));
        assert!(tiles_meet(tile, TileIndex::new(9, 13, 5)));
        assert!(tiles_meet(TileIndex::new(1, 1, 2), tile));
        assert!(!tiles_meet(tile, TileIndex::new(5, 6, 4)));
        assert!(!tiles_meet(tile, TileIndex::new(10, 13, 5)));
    }

    /// The skyline's 65 tiles at 1024 texels are 260 MB, more than the desktop's 256: the two
    /// smallest on the screen are halved (254 MB), and a second plan of the same view changes
    /// nothing.
    #[test]
    fn covers_too_many_for_the_budget_are_halved_smallest_on_the_screen_first() {
        let wants: Vec<Want> = (0..65).map(|x| want(x, 1.0 + f64::from(x))).collect();
        let mut cache = CoverCache::new(DESKTOP_BUDGET);
        let plan = cache.plan(&wants, |_| true);
        assert!(plan.free.is_empty());
        assert_eq!(plan.allocate.len(), 65);
        assert!(cache.bytes() <= DESKTOP_BUDGET, "{} MB", cache.bytes() / MIB);
        assert_eq!(cache.bytes(), 63 * 4 * MIB + 2 * MIB);
        let halved: Vec<u32> = (0..65).filter(|&x| cache.get(TileIndex::new(x, 0, 14)).unwrap().size == 512).collect();
        assert_eq!(halved, [0, 1]);
        assert_eq!(cache.plan(&wants, |_| true), Plan::default());
    }

    /// Covers of tiles no longer drawn make room first (the longest unseen first), and are
    /// forgotten as soon as the terrain lets go of their tiles; a cover cut down grows back when
    /// there is room again.
    #[test]
    fn idle_covers_go_first_and_cut_ones_grow_back() {
        let budget = 10 * 4 * MIB;
        let mut cache = CoverCache::new(budget);
        let first: Vec<Want> = (0..6).map(|x| want(x, 1.0)).collect();
        cache.plan(&first, |_| true);
        let second: Vec<Want> = (6..10).map(|x| want(x, 1.0)).collect();
        cache.plan(&second, |_| true);
        assert_eq!(cache.bytes(), 10 * 4 * MIB);
        // two more drawn: the two seen longest ago of the idle ones go
        let third: Vec<Want> = (6..12).map(|x| want(x, 1.0)).collect();
        let plan = cache.plan(&third, |_| true);
        assert_eq!(plan.free, [TileIndex::new(0, 0, 14), TileIndex::new(1, 0, 14)]);
        assert!(cache.get(TileIndex::new(2, 0, 14)).is_some());
        assert_eq!(cache.bytes(), budget);
        // twelve drawn at once: none idle to spare, so two are cut to fit
        let twelve: Vec<Want> = (0..12).map(|x| want(x, 1.0 + f64::from(x))).collect();
        cache.plan(&twelve, |_| true);
        assert!(cache.bytes() <= budget);
        let small: Vec<u32> = (0..12).filter(|&x| cache.get(TileIndex::new(x, 0, 14)).unwrap().size < 1024).collect();
        assert!(!small.is_empty() && small.iter().all(|&x| x < 4), "{small:?}");
        // a second plan of the same view keeps the sizes
        assert_eq!(cache.plan(&twelve, |_| true), Plan::default());
        assert_eq!(cache.next(usize::MAX).len(), 12);
        // the terrain lets go of four tiles: their covers go, and the cut ones grow back
        let eight: Vec<Want> = (0..8).map(|x| want(x, 1.0 + f64::from(x))).collect();
        let plan = cache.plan(&eight, |t| t.x < 8);
        assert!(plan.free.contains(&TileIndex::new(11, 0, 14)));
        assert!((0..8).all(|x| cache.get(TileIndex::new(x, 0, 14)).unwrap().size == 1024));
        assert_eq!(cache.len(), 8);
        // grown back, they are drawn again; the others keep their pictures
        let state = |x| cache.get(TileIndex::new(x, 0, 14)).unwrap().state;
        assert!(small.iter().all(|&x| state(x) == CoverState::Fresh));
        assert!((4..8).all(|x| state(x) == CoverState::Drawn));
    }

    /// A budget too small even for the smallest covers leaves the smallest on the screen without.
    #[test]
    fn a_tiny_budget_leaves_the_smallest_on_the_screen_without() {
        let mut cache = CoverCache::new(3 * cover_bytes(MIN_COVER_SIZE));
        let wants: Vec<Want> = (0..5).map(|x| want(x, 1.0 + f64::from(x))).collect();
        cache.plan(&wants, |_| true);
        assert!(cache.bytes() <= cache.budget());
        let sizes: Vec<u32> = (0..5).map(|x| cache.get(TileIndex::new(x, 0, 14)).unwrap().size).collect();
        assert_eq!(sizes, [0, 0, MIN_COVER_SIZE, MIN_COVER_SIZE, MIN_COVER_SIZE]);
        // the ones without are not waiting to be drawn
        assert_eq!(cache.waiting(), 3);
    }

    /// Never drawn first, then out of date, the largest on the screen first, a few at a time; a
    /// change draped over a tile's square (or one holding it, or inside it) puts its cover out of
    /// date, others not.
    #[test]
    fn covers_are_drawn_a_few_at_a_time_and_again_when_what_is_draped_changes() {
        let mut cache = CoverCache::new(DESKTOP_BUDGET);
        let wants: Vec<Want> = (0..5).map(|x| Want { tile: TileIndex::new(x, 0, 3), size: 1024, weight: f64::from(x) }).collect();
        cache.plan(&wants, |_| true);
        assert_eq!(cache.waiting(), 5);
        assert_eq!(cache.next(2), [TileIndex::new(4, 0, 3), TileIndex::new(3, 0, 3)]);
        assert_eq!(cache.next(2), [TileIndex::new(2, 0, 3), TileIndex::new(1, 0, 3)]);
        assert_eq!(cache.waiting(), 1);
        assert_eq!(cache.next(2), [TileIndex::new(0, 0, 3)]);
        assert!(cache.next(2).is_empty());
        // a z5 ground tile inside tile 1's square, and a z1 tile holding tiles 0 to 3
        cache.invalidate(&[TileIndex::new(5, 1, 5)]);
        assert_eq!(cache.get(TileIndex::new(1, 0, 3)).unwrap().state, CoverState::Stale);
        assert_eq!(cache.waiting(), 1);
        cache.invalidate(&[TileIndex::new(0, 0, 1)]);
        assert_eq!(cache.waiting(), 4);
        assert_eq!(cache.get(TileIndex::new(4, 0, 3)).unwrap().state, CoverState::Drawn);
        // a new tile comes before those out of date, whatever its size on the screen
        let mut more = wants.clone();
        more.push(Want { tile: TileIndex::new(7, 7, 3), size: 1024, weight: 0.5 });
        cache.plan(&more, |_| true);
        assert_eq!(cache.next(1), [TileIndex::new(7, 7, 3)]);
        assert_eq!(cache.next(1), [TileIndex::new(3, 0, 3)]);
        cache.invalidate_all();
        assert_eq!(cache.waiting(), 6);
        // a cover whose tile is not drawn waits for it to be drawn again
        cache.plan(&wants[..2], |_| true);
        assert_eq!(cache.waiting(), 2);
        // and turned off, every texture goes
        let freed = cache.clear();
        assert_eq!(freed.len(), 6);
        assert!(cache.is_empty() && cache.bytes() == 0);
    }

    /// A cover keeps the size it was first made at while its tile stays (deck makes a tile's
    /// render viewport once): zooming in past 17.5 does not remake the deepest tiles' covers.
    #[test]
    fn a_cover_keeps_its_first_size() {
        let mut cache = CoverCache::new(DESKTOP_BUDGET);
        let tile = TileIndex::new(34_000, 23_000, 16);
        cache.plan(&[Want { tile, size: cover_size(16, cover_zoom(16, 16, 16.2)), weight: 1.0 }], |_| true);
        let plan = cache.plan(&[Want { tile, size: cover_size(16, cover_zoom(16, 16, 18.4)), weight: 3.0 }], |_| true);
        assert_eq!(plan, Plan::default());
        assert_eq!(cache.get(tile).unwrap().size, 1024);
        assert_eq!(cache.get(tile).unwrap().wanted, 1024);
    }
}
