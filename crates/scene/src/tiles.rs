//! Map tiles (doc 01 §5.6, §5.8; plan 1.4.8): tile ids, bounds, parents and children; deck.gl's
//! zoom rule and its view-frustum selection for both viewports; the `best-available` refinement;
//! request priority by distance.
//!
//! Transliterated from deck.gl 9.4.0's `@deck.gl/geo-layers/dist/tileset-2d/` (`utils.js`,
//! `tile-2d-traversal.js`, `tileset-2d.js`), with the arithmetic in the same order. The culling it
//! is built on (math.gl 4.1.0's `@math.gl/culling`: planes, boxes, `computeVisibility`) is
//! [`crate::lod`]'s, which ports it for its own distance rule. `tests/data/deck/tiles.json`
//! (S1.32) pins the selection: deck.gl's own `Tileset2D.getTileIndices` for 24 views and six
//! tileset configurations.
//!
//! What is here, and what the tile runtime (`map::tiles`) does with it:
//! - [`TileIndex`] (shared with the rest of the crate) gets deck's id, parent, children and
//!   bounds;
//! - [`tile_zoom`] is the zoom rule: `round(zoom + log2(512 / tile_size) + zoom_offset)`, so 512 px
//!   tiles take `round(zoom)` and 256 px tiles `round(zoom + 1)`;
//! - [`tile_indices`] is the selection (`Tileset2D.getTileIndices`): a depth-first walk from tile
//!   0/0/0 that drops tiles outside the frustum (and, on the globe, tiles all on its far side);
//!   on the globe, and on the flat map above 60° pitch, a tile far from the camera is taken at a
//!   coarser zoom; otherwise every tile is at the rule's zoom;
//! - [`best_available`] is deck's default refinement: while a selected tile loads, its nearest
//!   loaded ancestor, or else its loaded descendants, are drawn in its place ([`link_tree`] builds
//!   the cache's tree it walks; `lod::keep_coarser_until_in` is the other strategy);
//! - [`request_priority`] orders the requests: selected tiles first, then the placeholders, each
//!   by how far their outline is on screen from the view's centre;
//! - `impl LodViewport for Viewport` lets [`crate::lod`]'s distance selection run on the camera's
//!   viewports.
//!
//! Differences from deck.gl:
//! - only geospatial viewports exist here (both of the camera's are), so `getIdentityTileIndices`
//!   (non-geospatial views) and the model matrix are not ported;
//! - the app's `GlobeView` never repeats the world, so the walk runs once, for world offset 0
//!   (deck walks the copies of a repeated `WebMercatorViewport` too);
//! - tiles are [`TileIndex`] values; [`TileIndex::id`] gives deck's `"x-y-z"` string.

use std::collections::HashMap;
use std::f64::consts::PI;

use rustradar_core::num::js_round;

pub use crate::bridges::TileIndex;
use crate::camera::{GlobeViewport, MercatorViewport, ViewState, Viewport, ViewportBase, lng_lat_to_world};
use crate::geom::{js_max, js_min};
use crate::lod::{
    AxisAlignedBox, BoundingVolume, FrustumPlane, Intersection, LodViewport, Plane, compute_visibility, culling_planes,
    oriented_box_from_points,
};
pub use crate::lod::{RefineTile, TILE_SIZE};

// where a tile on the globe is sampled for its bounding volume (tile-2d-traversal.js): the centre
// and 4 corners; plus the 4 edge midpoints below zoom 2; plus 2 points on the middle row at zoom 0
const REF_POINTS_5: [[f64; 2]; 5] = [[0.5, 0.5], [0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]];
const REF_POINTS_9: [[f64; 2]; 9] =
    [[0.5, 0.5], [0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.5], [0.5, 0.0], [1.0, 0.5], [0.5, 1.0]];
const REF_POINTS_11: [[f64; 2]; 11] = [
    [0.5, 0.5],
    [0.0, 0.0],
    [0.0, 1.0],
    [1.0, 0.0],
    [1.0, 1.0],
    [0.0, 0.5],
    [0.5, 0.0],
    [1.0, 0.5],
    [0.5, 1.0],
    [0.25, 0.5],
    [0.75, 0.5],
];

/// The flat map's frustum selection takes the rule's zoom everywhere up to this pitch; above it,
/// far tiles are coarser (`viewport.pitch <= 60`).
pub const FLAT_LOD_ABOVE_PITCH: f64 = 60.0;

// --- tile ids ------------------------------------------------------------------------------------

/// A tile's extent in degrees, as deck's `tileToBoundingBox` gives it for a geospatial viewport
/// (OSM scheme: `north` is the top edge, y counting from the north).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileBounds {
    pub west: f64,
    pub north: f64,
    pub east: f64,
    pub south: f64,
}

/// deck's `osmTile2lngLat(x, y, z)`: the `[lon, lat]` of a point in tile coordinates (fractional
/// `x` and `y` reach inside a tile) at zoom `z`.
pub fn osm_tile_to_lng_lat(x: f64, y: f64, z: u32) -> [f64; 2] {
    // getScale(z, TILE_SIZE) = 2^z · 512 / 512
    let scale = 2f64.powi(z as i32);
    let lng = (x / scale) * 360.0 - 180.0;
    let n = PI - (2.0 * PI * y) / scale;
    let lat = (180.0 / PI) * (0.5 * (n.exp() - (-n).exp())).atan();
    [lng, lat]
}

impl TileIndex {
    /// The tile covering the whole world.
    pub const ROOT: TileIndex = TileIndex { x: 0, y: 0, z: 0 };

    pub const fn new(x: u32, y: u32, z: u32) -> Self {
        TileIndex { x, y, z }
    }

    /// deck's `getTileId`: `"x-y-z"`.
    pub fn id(self) -> String {
        format!("{}-{}-{}", self.x, self.y, self.z)
    }

    /// The tile one zoom out that holds this one (deck's `getParentIndex`); none for the root.
    pub fn parent(self) -> Option<TileIndex> {
        (self.z > 0).then(|| TileIndex { x: self.x / 2, y: self.y / 2, z: self.z - 1 })
    }

    /// The four tiles one zoom in, in deck's order: (2x, 2y), (2x, 2y + 1), (2x + 1, 2y),
    /// (2x + 1, 2y + 1).
    pub fn children(self) -> [TileIndex; 4] {
        let (x, y, z) = (self.x * 2, self.y * 2, self.z + 1);
        [TileIndex { x, y, z }, TileIndex { x, y: y + 1, z }, TileIndex { x: x + 1, y, z }, TileIndex { x: x + 1, y: y + 1, z }]
    }

    /// Whether `other` is this tile or lies inside it.
    pub fn contains(self, other: TileIndex) -> bool {
        other.z >= self.z && other.x >> (other.z - self.z) == self.x && other.y >> (other.z - self.z) == self.y
    }

    /// Its extent in degrees (deck's `tileToBoundingBox` for a geospatial viewport).
    pub fn bounds(self) -> TileBounds {
        let (x, y) = (f64::from(self.x), f64::from(self.y));
        let [west, north] = osm_tile_to_lng_lat(x, y, self.z);
        let [east, south] = osm_tile_to_lng_lat(x + 1.0, y + 1.0, self.z);
        TileBounds { west, north, east, south }
    }
}

// --- the selection -------------------------------------------------------------------------------

/// A tileset's options that the selection reads (deck's `Tileset2D` props of the same names).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TilesetOptions {
    /// The tiles' size in pixels (deck's default 512).
    pub tile_size: f64,
    /// No tiles below this zoom (the tileset uses its ceiling); none: no limit.
    pub min_zoom: Option<f64>,
    /// Tiles above this zoom are over-zoomed from it (the tileset uses its floor); none: no limit.
    pub max_zoom: Option<f64>,
    /// The lowest and highest ground in metres, for the tiles' bounding volumes; none: 0.
    pub z_range: Option<[f64; 2]>,
    /// `[min lon, min lat, max lon, max lat]` the tiles must overlap; none: the world. With one,
    /// a view below `min_zoom` takes tiles at `min_zoom` instead of none.
    pub extent: Option<[f64; 4]>,
    /// Added to the zoom rule (deck's `zoomOffset`).
    pub zoom_offset: f64,
    /// No tiles while the view's zoom is below this (`visibleMinZoom`).
    pub visible_min_zoom: Option<f64>,
    /// No tiles while the view's zoom is above this (`visibleMaxZoom`).
    pub visible_max_zoom: Option<f64>,
}

impl TilesetOptions {
    /// A tileset of `tile_size` px tiles from `min_zoom` to `max_zoom`, every other option deck's
    /// default.
    pub const fn new(tile_size: f64, min_zoom: f64, max_zoom: f64) -> Self {
        TilesetOptions { tile_size, min_zoom: Some(min_zoom), max_zoom: Some(max_zoom), ..Self::DEFAULT }
    }

    /// deck's defaults: 512 px tiles, no zoom limits, no elevation, no extent, no offset.
    pub const DEFAULT: TilesetOptions = TilesetOptions {
        tile_size: 512.0,
        min_zoom: None,
        max_zoom: None,
        z_range: None,
        extent: None,
        zoom_offset: 0.0,
        visible_min_zoom: None,
        visible_max_zoom: None,
    };
}

impl Default for TilesetOptions {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// OpenFreeMap's vector tiles (`map-globe`, `map-flat`, `aeroway-true-*`, `label-tiles`; doc 01
/// §5.6): 512 px, zooms 0 to 14.
pub const VECTOR_TILES: TilesetOptions = TilesetOptions::new(512.0, 0.0, 14.0);
/// Esri's imagery (the satellite basemap; doc 01 §5.8): 256 px, zooms 0 to 19.
pub const IMAGERY_TILES: TilesetOptions = TilesetOptions::new(256.0, 0.0, 19.0);
/// The weather radar's tiles (doc 01 §5.11, G:1341-1354): 256 px, zooms 0 to 7.
pub const RADAR_TILES: TilesetOptions = TilesetOptions::new(256.0, 0.0, 7.0);

/// deck's zoom rule for a geospatial viewport: the tiles' zoom for a view at `zoom`,
/// `round(zoom + log2(512 / tile_size) + zoom_offset)` with JavaScript's rounding (a half rounds
/// up), before the tileset's limits.
pub fn tile_zoom(zoom: f64, tile_size: f64, zoom_offset: f64) -> f64 {
    js_round(zoom + (TILE_SIZE / tile_size).log2() + zoom_offset)
}

/// The tiles a tileset selects for a view (deck's `Tileset2D.getTileIndices`, which `TileLayer`
/// and `MVTLayer` refine), in deck's order: a depth-first walk from tile 0/0/0.
///
/// The zoom is [`tile_zoom`]; above the tileset's `floor(max_zoom)` it is that; below its
/// `ceil(min_zoom)` nothing is selected (with an `extent`, tiles at `min_zoom` are). Nothing either
/// while the view's zoom is outside `visible_min_zoom`…`visible_max_zoom`.
pub fn tile_indices(viewport: &Viewport, opts: &TilesetOptions) -> Vec<TileIndex> {
    // Tileset2D.setOptions keeps floor(maxZoom) and ceil(minZoom) when they are finite
    let max_zoom = opts.max_zoom.filter(|z| z.is_finite()).map(f64::floor);
    let min_zoom = opts.min_zoom.filter(|z| z.is_finite()).map(f64::ceil);
    // utils.js getTileIndices: as Google Maps and MapLibre, a view at 9.5 takes tiles at 10
    let mut z = tile_zoom(viewport.zoom, opts.tile_size, opts.zoom_offset);
    if let Some(min_zoom) = min_zoom
        && z < min_zoom
    {
        if opts.extent.is_none() {
            return Vec::new();
        }
        z = min_zoom;
    }
    if let Some(max_zoom) = max_zoom
        && z > max_zoom
    {
        z = max_zoom;
    }
    if opts.visible_min_zoom.is_some_and(|v| viewport.zoom < v) || opts.visible_max_zoom.is_some_and(|v| viewport.zoom > v) {
        return Vec::new();
    }
    osm_tile_indices(viewport, z, opts.z_range, opts.extent)
}

/// deck's `getOSMTileIndices(viewport, maxZ, zRange, bounds)`: the walk itself, for tiles at zoom
/// `max_z`. `z_range` is in metres; `bounds` is `[min lon, min lat, max lon, max lat]`.
pub fn osm_tile_indices(viewport: &Viewport, max_z: f64, z_range: Option<[f64; 2]>, bounds: Option<[f64; 4]>) -> Vec<TileIndex> {
    let globe = viewport.globe();
    // the culling volume of the camera, its normals turned inward
    let planes = culling_planes(&frustum_planes(viewport));
    // the elevation range from metres to common units (`(zRange && zRange[0] * unitsPerMeter) || 0`)
    let units_per_meter = viewport.distance_scales.units_per_meter[2];
    let elevation_min = or0(z_range.map_or(0.0, |r| r[0] * units_per_meter));
    let elevation_max = or0(z_range.map_or(0.0, |r| r[1] * units_per_meter));
    // always load at the current zoom level if the pitch is small
    let min_z = if !viewport.is_globe() && viewport.pitch <= FLAT_LOD_ABOVE_PITCH { max_z } else { 0.0 };
    // the extent to OSM positions (y down from the north, 512 units round)
    let bounds = bounds.map(|[min_lng, min_lat, max_lng, max_lat]| {
        let top_left = lng_lat_to_world([min_lng, max_lat]);
        let bottom_right = lng_lat_to_world([max_lng, min_lat]);
        [top_left[0], TILE_SIZE - top_left[1], bottom_right[0], TILE_SIZE - bottom_right[1]]
    });
    let traversal = Traversal {
        globe,
        planes,
        elevation: [elevation_min, elevation_max],
        min_z,
        max_z,
        bounds,
        camera: viewport.camera_position.to_array(),
        scale: viewport.scale,
        height: viewport.height,
    };
    let mut selected = Vec::new();
    traversal.update(TileIndex::ROOT, &mut selected);
    selected
}

/// JavaScript's `x || 0`: 0 for 0, −0 and NaN.
fn or0(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() { 0.0 } else { x }
}

/// The camera's frustum planes as [`crate::lod`] reads them (deck's `getFrustumPlanes`, in its
/// object order: left, right, bottom, top, near, far; normals pointing out).
fn frustum_planes(viewport: &Viewport) -> [FrustumPlane; 6] {
    // (the camera's own planes: `viewport.frustum_planes()` would be the `LodViewport` method)
    ViewportBase::frustum_planes(viewport).all().map(|p| FrustumPlane { normal: p.normal.to_array(), distance: p.distance })
}

/// What one walk reads (deck's `traversalParams`).
struct Traversal<'a> {
    /// The globe's viewport (its `projectPosition` makes the bounding volumes); none on the flat map.
    globe: Option<&'a GlobeViewport>,
    planes: [Plane; 6],
    /// The lowest and highest ground, common units.
    elevation: [f64; 2],
    min_z: f64,
    max_z: f64,
    /// The extent in OSM positions.
    bounds: Option<[f64; 4]>,
    camera: [f64; 3],
    scale: f64,
    height: f64,
}

impl Traversal<'_> {
    /// `OSMNode.update`: take the tile, or walk its children, or leave it out.
    fn update(&self, tile: TileIndex, selected: &mut Vec<TileIndex>) {
        let volume = self.bounding_volume(tile);
        // first, check if this tile is visible
        if let Some(bounds) = self.bounds
            && !inside_bounds(tile, bounds)
        {
            return;
        }
        if compute_visibility(&self.planes, &volume) == Intersection::Outside {
            return;
        }
        // globe: reject tiles occluded by the near hemisphere
        if let Some(globe) = self.globe
            && beyond_horizon(tile, self.camera, globe, self.elevation[1])
        {
            return;
        }
        let mut z = f64::from(tile.z);
        if z < self.max_z && z >= self.min_z {
            // if the tile is far enough from the camera, accept a lower zoom level
            let distance = (volume.distance_to(self.camera) * self.scale) / self.height;
            z += distance.log2().floor();
        }
        if z >= self.max_z {
            // the level of detail is acceptable
            selected.push(tile);
            return;
        }
        // not enough: test the child tiles
        for child in tile.children() {
            self.update(child, selected);
        }
    }

    /// `OSMNode.getBoundingVolume`: on the globe the oriented box of sample points of the tile at
    /// both elevations (heights in common units handed to `projectPosition`, which takes metres:
    /// deck's own slip, kept; both are 0 without an elevation range); on the flat map the box of
    /// its square.
    fn bounding_volume(&self, tile: TileIndex) -> BoundingVolume {
        let [z0, z1] = self.elevation;
        if let Some(globe) = self.globe {
            // at low zoom more samples are needed to bound the tile
            let refs: &[[f64; 2]] = if tile.z < 1 {
                &REF_POINTS_11
            } else if tile.z < 2 {
                &REF_POINTS_9
            } else {
                &REF_POINTS_5
            };
            let mut points = Vec::with_capacity(refs.len() * 2);
            for &[px, py] in refs {
                let [lng, lat] = osm_tile_to_lng_lat(f64::from(tile.x) + px, f64::from(tile.y) + py, tile.z);
                points.push(globe.project_position([lng, lat, z0]).to_array());
                if z0 != z1 {
                    // account for the elevation volume
                    points.push(globe.project_position([lng, lat, z1]).to_array());
                }
            }
            return BoundingVolume::Oriented(oriented_box_from_points(&points));
        }
        // web mercator: deck's common space is y-flipped
        let extent = TILE_SIZE / 2f64.powi(tile.z as i32);
        let origin_x = f64::from(tile.x) * extent;
        let origin_y = TILE_SIZE - (f64::from(tile.y) + 1.0) * extent;
        BoundingVolume::Aligned(AxisAlignedBox::new([origin_x, origin_y, z0], [origin_x + extent, origin_y + extent, z1]))
    }
}

/// `OSMNode.insideBounds`: whether the tile overlaps the extent (OSM positions).
fn inside_bounds(tile: TileIndex, [min_x, min_y, max_x, max_y]: [f64; 4]) -> bool {
    let extent = TILE_SIZE / 2f64.powi(tile.z as i32);
    let (x, y) = (f64::from(tile.x), f64::from(tile.y));
    x * extent < max_x && y * extent < max_y && (x + 1.0) * extent > min_x && (y + 1.0) * extent > min_y
}

/// `OSMNode.beyondHorizon`: whether the tile's point closest to the camera's direction (at
/// `elevation`) lies on the globe's far side, where the camera cannot see it.
fn beyond_horizon(tile: TileIndex, [cx, cy, cz]: [f64; 3], globe: &GlobeViewport, elevation: f64) -> bool {
    let c_mag = (cx * cx + cy * cy + cz * cz).sqrt();
    let cam_lng = (cx.atan2(-cy) * 180.0) / PI;
    let cam_lat = ((cz / c_mag).asin() * 180.0) / PI;
    let TileBounds { west, north, east, south } = tile.bounds();
    let tile_center_lng = (west + east) / 2.0;
    let wrapped_cam_lng = tile_center_lng + (((cam_lng - tile_center_lng + 540.0) % 360.0) - 180.0);
    let closest_lng = js_max(west, js_min(wrapped_cam_lng, east));
    let closest_lat = js_max(south, js_min(cam_lat, north));
    let closest = globe.project_position([closest_lng, closest_lat, elevation]);
    let dot = closest.x * cx + closest.y * cy + closest.z * cz;
    let mag_sq = closest.x * closest.x + closest.y * closest.y + closest.z * closest.z;
    dot <= mag_sq
}

// --- refinement ----------------------------------------------------------------------------------

const TILE_STATE_VISIBLE: u8 = 2;

/// `Tileset2D._rebuildTree`: each cached tile's parent is its nearest ancestor in the cache, down
/// to zoom `min_zoom` (the tileset's `ceil(minZoom)`, else 0), and each tile lists the tiles whose
/// parent it is, in cache order. `tiles[i]` is the tile `indices[i]`; every tile gets a list of
/// children (empty if none), as deck's tiles always have one.
pub fn link_tree(tiles: &mut [RefineTile], indices: &[TileIndex], min_zoom: u32) {
    assert_eq!(tiles.len(), indices.len(), "one index per tile");
    let at: HashMap<TileIndex, usize> = indices.iter().enumerate().map(|(i, &index)| (index, i)).collect();
    for tile in tiles.iter_mut() {
        tile.parent = None;
        tile.children = Some(Vec::new());
    }
    for (i, &index) in indices.iter().enumerate() {
        // _getNearestAncestor
        let mut up = index;
        let mut parent = None;
        while up.z > min_zoom {
            let Some(p) = up.parent() else { break };
            up = p;
            if let Some(&j) = at.get(&up) {
                parent = Some(j);
                break;
            }
        }
        tiles[i].parent = parent;
        if let Some(j) = parent {
            tiles[j].children.get_or_insert_with(Vec::new).push(i);
        }
    }
}

/// deck's default refinement, `best-available` (`updateTileStateDefault`): a selected tile is drawn
/// once it is ready; until then its nearest ready ancestor stands in, or, with none, its nearest
/// ready descendants. Sets each tile's `is_visible` (and `state`); reads `is_selected`, `ready`,
/// `parent` and `children` ([`link_tree`]).
///
/// deck's test for a stand-in is `tile.isLoaded || tile.content`, and a tile whose request failed
/// or came back empty counts as loaded (deck 9.4 settles it so the view can finish loading): it
/// then hides its ancestors and leaves a hole. Set `ready` that way to match deck; a runtime that
/// wants a failed tile's loaded parent to stay visible sets `ready` only for tiles with content.
pub fn best_available(tiles: &mut [RefineTile]) {
    for tile in tiles.iter_mut() {
        tile.state = 0;
    }
    for i in 0..tiles.len() {
        if tiles[i].is_selected && !placeholder_in_ancestors(tiles, i) {
            placeholder_in_children(tiles, i);
        }
    }
    for tile in tiles.iter_mut() {
        tile.is_visible = tile.state & TILE_STATE_VISIBLE != 0;
    }
}

/// `getPlaceholderInAncestors`: walk up from the tile itself until one is ready; whether one was.
fn placeholder_in_ancestors(tiles: &mut [RefineTile], start: usize) -> bool {
    let mut tile = Some(start);
    while let Some(i) = tile {
        if tiles[i].ready {
            tiles[i].state |= TILE_STATE_VISIBLE;
            return true;
        }
        tile = tiles[i].parent;
    }
    false
}

/// `getPlaceholderInChildren`: each child that is ready stands in, the others hand down to theirs.
fn placeholder_in_children(tiles: &mut [RefineTile], tile: usize) {
    let children = tiles[tile].children.clone().unwrap_or_default();
    for child in children {
        if tiles[child].ready {
            tiles[child].state |= TILE_STATE_VISIBLE;
        } else {
            placeholder_in_children(tiles, child);
        }
    }
}

// --- request priority ----------------------------------------------------------------------------

/// The priority of a selected tile's request, before its distance (`SELECTED_TILE_PRIORITY`).
pub const SELECTED_TILE_PRIORITY: f64 = 0.0;
/// The priority of a visible (stand-in) tile's request, before its distance
/// (`VISIBLE_TILE_PRIORITY`).
pub const VISIBLE_TILE_PRIORITY: f64 = 1e8;
/// The largest distance priority, so the two tiers never mix (`MAX_TILE_DISTANCE_PRIORITY`).
pub const MAX_TILE_DISTANCE_PRIORITY: f64 = VISIBLE_TILE_PRIORITY - SELECTED_TILE_PRIORITY - 1.0;

/// `Tileset2D._getRequestPriority`: lower loads first. Selected tiles come first, then visible
/// ones (stand-ins), each by [`tile_distance_priority`]. `None` (deck's −1) for a tile that is
/// neither: the scheduler drops its request.
pub fn request_priority(viewport: &Viewport, tile: TileIndex, is_selected: bool, is_visible: bool) -> Option<f64> {
    if !is_selected && !is_visible {
        return None;
    }
    let distance = tile_distance_priority(viewport, tile);
    Some(if is_selected { SELECTED_TILE_PRIORITY + distance } else { VISIBLE_TILE_PRIORITY + distance })
}

/// `Tileset2D._getTileDistancePriority`: 0 when the view's centre pixel lies inside the tile's
/// outline on screen (its four corners projected), else the squared distance in pixels from the
/// centre to the nearest edge of that outline, at most [`MAX_TILE_DISTANCE_PRIORITY`] (also when a
/// corner does not project to a finite pixel).
pub fn tile_distance_priority(viewport: &Viewport, tile: TileIndex) -> f64 {
    let (width, height) = (viewport.width, viewport.height);
    if width == 0.0 || width.is_nan() || height == 0.0 || height.is_nan() {
        return 0.0;
    }
    let points = tile_screen_corners(viewport, tile.bounds());
    let center = [width / 2.0, height / 2.0];
    if points.len() == 4 {
        if point_in_polygon(center, &points) {
            return 0.0;
        }
        let mut distance = 9_007_199_254_740_991.0; // Number.MAX_SAFE_INTEGER
        for i in 0..points.len() {
            let next = points[(i + 1) % points.len()];
            distance = js_min(distance, point_to_segment_distance_squared(center, points[i], next));
        }
        return js_min(distance, MAX_TILE_DISTANCE_PRIORITY);
    }
    MAX_TILE_DISTANCE_PRIORITY
}

/// `_getTileScreenCorners`: south-west, south-east, north-east, north-west on screen, those that
/// project to finite pixels.
fn tile_screen_corners(viewport: &Viewport, b: TileBounds) -> Vec<[f64; 2]> {
    [[b.west, b.south], [b.east, b.south], [b.east, b.north], [b.west, b.north]]
        .into_iter()
        .map(|[lng, lat]| {
            let [x, y, _] = viewport.project([lng, lat, 0.0]);
            [x, y]
        })
        .filter(|[x, y]| x.is_finite() && y.is_finite())
        .collect()
}

/// `_isPointInPolygon`: even-odd crossing test.
fn point_in_polygon([x, y]: [f64; 2], polygon: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let [xi, yi] = polygon[i];
        let [xj, yj] = polygon[j];
        if (yi > y) != (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// `_getPointToSegmentDistanceSquared`.
fn point_to_segment_distance_squared([x, y]: [f64; 2], [x1, y1]: [f64; 2], [x2, y2]: [f64; 2]) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let length_squared = dx * dx + dy * dy;
    // `lengthSquared ? … : 0`: 0 and NaN are falsy
    let t = if length_squared != 0.0 && !length_squared.is_nan() {
        js_max(0.0, js_min(1.0, ((x - x1) * dx + (y - y1) * dy) / length_squared))
    } else {
        0.0
    };
    let segment_x = x1 + t * dx;
    let segment_y = y1 + t * dy;
    let distance_x = x - segment_x;
    let distance_y = y - segment_y;
    distance_x * distance_x + distance_y * distance_y
}

// --- the camera's viewports for lod ------------------------------------------------------------------

/// The camera's viewports as [`crate::lod`]'s distance selection reads them.
impl LodViewport for Viewport {
    fn is_geospatial(&self) -> bool {
        true
    }

    fn is_globe(&self) -> bool {
        Viewport::is_globe(self)
    }

    fn project_position(&self, xyz: [f64; 3]) -> [f64; 3] {
        Viewport::project_position(self, xyz).to_array()
    }

    fn frustum_planes(&self) -> [FrustumPlane; 6] {
        frustum_planes(self)
    }

    fn camera_position(&self) -> [f64; 3] {
        self.camera_position.to_array()
    }

    fn units_per_meter(&self) -> f64 {
        self.distance_scales.units_per_meter[2]
    }

    fn scale(&self) -> f64 {
        self.scale
    }

    fn zoom(&self) -> f64 {
        self.zoom
    }

    fn height(&self) -> f64 {
        self.height
    }

    fn longitude(&self) -> f64 {
        self.longitude
    }

    fn latitude(&self) -> f64 {
        self.latitude
    }

    /// A viewport of the same kind (globe or flat, whatever the zoom), with this one's size, pitch
    /// and bearing and no `position`.
    fn ahead(&self, longitude: f64, latitude: f64, zoom: f64) -> Self {
        let view = ViewState { longitude, latitude, zoom, bearing: self.bearing, pitch: self.pitch, position: None };
        match self {
            Viewport::Globe(v) => Viewport::Globe(GlobeViewport::new(&view, v.width, v.height)),
            Viewport::Mercator(v) => Viewport::Mercator(MercatorViewport::new(&view, v.width, v.height)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::viewport_for;
    use crate::lod::{Lod, distance_tile_indices};
    use serde_json::Value;

    /// deck.gl's own `Tileset2D.getTileIndices` for the 24 views of `views.json` and six tileset
    /// configurations (S1.32; `scripts/port/deck-golden.ts` documents every field).
    fn golden() -> Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/deck/tiles.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("tiles.json")).expect("tiles.json parses")
    }

    fn f(v: &Value) -> f64 {
        v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
    }

    fn opt(v: &Value) -> Option<f64> {
        (!v.is_null()).then(|| f(v))
    }

    fn view_state(v: &Value) -> ViewState {
        let s = &v["viewState"];
        ViewState {
            longitude: f(&s["longitude"]),
            latitude: f(&s["latitude"]),
            zoom: f(&s["zoom"]),
            bearing: f(&s["bearing"]),
            pitch: f(&s["pitch"]),
            position: s.get("position").map(|p| [f(&p[0]), f(&p[1]), f(&p[2])]),
        }
    }

    fn options(c: &Value) -> TilesetOptions {
        TilesetOptions {
            tile_size: f(&c["tileSize"]),
            min_zoom: opt(&c["minZoom"]),
            max_zoom: opt(&c["maxZoom"]),
            z_range: (!c["zRange"].is_null()).then(|| [f(&c["zRange"][0]), f(&c["zRange"][1])]),
            ..TilesetOptions::DEFAULT
        }
    }

    fn tiles(list: &Value) -> Vec<TileIndex> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|t| TileIndex { x: f(&t[0]) as u32, y: f(&t[1]) as u32, z: f(&t[2]) as u32 })
            .collect()
    }

    fn view(lng: f64, lat: f64, zoom: f64, pitch: f64, bearing: f64) -> Viewport {
        let view = ViewState { longitude: lng, latitude: lat, zoom, bearing, pitch, position: None };
        viewport_for(&view, [1440.0, 900.0])
    }

    #[test]
    fn matches_every_golden_tile_list() {
        let golden = golden();
        let configs = golden["configs"].as_array().unwrap();
        let views = golden["views"].as_array().unwrap();
        assert_eq!(views.len(), 24);
        assert_eq!(configs.len(), 6);
        let mut lists = 0;
        let mut sizes = std::collections::BTreeSet::new();
        for v in views {
            let name = v["name"].as_str().unwrap();
            let vp = viewport_for(&view_state(v), [f(&v["width"]), f(&v["height"])]);
            assert_eq!(vp.is_globe(), v["viewport"] == "globe", "{name}: viewport");
            for c in configs {
                let config = c["name"].as_str().unwrap();
                let opts = options(c);
                sizes.insert(opts.tile_size as u32);
                let want = tiles(&v["tiles"][config]);
                let ours = tile_indices(&vp, &opts);
                if ours != want {
                    let missing: Vec<_> = want.iter().filter(|t| !ours.contains(t)).collect();
                    let extra: Vec<_> = ours.iter().filter(|t| !want.contains(t)).collect();
                    panic!(
                        "{name} / {config}: {} tiles, deck has {}; missing {missing:?}; extra {extra:?}{}",
                        ours.len(),
                        want.len(),
                        if missing.is_empty() && extra.is_empty() { " (same tiles, other order)" } else { "" },
                    );
                }
                lists += 1;
            }
        }
        assert_eq!(lists, 24 * 6);
        assert_eq!(sizes.into_iter().collect::<Vec<_>>(), vec![256, 512], "both tile sizes");
    }

    #[test]
    fn the_zoom_rule_rounds_and_halves_round_up() {
        // 512 px tiles: round(zoom); 256 px tiles: round(zoom + 1) (plan, facts checked)
        assert_eq!(tile_zoom(9.49, 512.0, 0.0), 9.0);
        assert_eq!(tile_zoom(9.5, 512.0, 0.0), 10.0);
        assert_eq!(tile_zoom(9.5, 256.0, 0.0), 11.0);
        assert_eq!(tile_zoom(2.4, 256.0, 0.0), 3.0);
        assert_eq!(tile_zoom(0.5, 512.0, 0.0), 1.0);
        assert_eq!(tile_zoom(-0.5, 512.0, 0.0), 0.0);
        assert_eq!(tile_zoom(-1.5, 512.0, 0.0), -1.0);
        assert_eq!(tile_zoom(9.0, 1024.0, 0.0), 8.0);
        assert_eq!(tile_zoom(9.0, 512.0, 1.0), 10.0);
    }

    #[test]
    fn the_tilesets_limits_clamp_the_zoom() {
        // flat map, looking straight down: every tile is at the rule's zoom, clamped
        let vp = view(18.07, 59.33, 16.2, 0.0, 0.0);
        assert!(tile_indices(&vp, &VECTOR_TILES).iter().all(|t| t.z == 14));
        assert!(tile_indices(&vp, &IMAGERY_TILES).iter().all(|t| t.z == 17));
        assert!(tile_indices(&vp, &RADAR_TILES).iter().all(|t| t.z == 7));
        // a fractional maxZoom is floored, a fractional minZoom ceiled
        let floored = TilesetOptions::new(512.0, 0.0, 13.7);
        assert!(tile_indices(&vp, &floored).iter().all(|t| t.z == 13));
        let ceiled = TilesetOptions::new(512.0, 16.2, 20.0);
        assert!(tile_indices(&vp, &ceiled).is_empty(), "16 is below ceil(16.2)");
    }

    #[test]
    fn below_min_zoom_nothing_unless_there_is_an_extent() {
        let vp = view(18.07, 59.33, 13.3, 0.0, 0.0);
        let opts = TilesetOptions::new(512.0, 15.0, 18.0);
        assert!(tile_indices(&vp, &opts).is_empty());
        // with an extent, tiles at min_zoom, only those overlapping it
        let extent = [18.0, 59.3, 18.1, 59.35];
        let with = tile_indices(&vp, &TilesetOptions { extent: Some(extent), ..opts });
        assert!(!with.is_empty());
        for t in &with {
            assert_eq!(t.z, 15);
            let b = t.bounds();
            assert!(b.west < extent[2] && b.east > extent[0] && b.south < extent[3] && b.north > extent[1], "{t:?}");
        }
        // and an extent narrows the selection at any zoom
        let all = tile_indices(&vp, &VECTOR_TILES);
        let narrowed = tile_indices(&vp, &TilesetOptions { extent: Some([18.05, 59.32, 18.06, 59.33]), ..VECTOR_TILES });
        assert!(!narrowed.is_empty() && narrowed.len() < all.len(), "{narrowed:?} of {all:?}");
        assert!(narrowed.iter().all(|t| all.contains(t)));
    }

    #[test]
    fn visible_zooms_hide_every_tile() {
        let vp = view(18.07, 59.33, 13.3, 0.0, 0.0);
        assert!(tile_indices(&vp, &TilesetOptions { visible_min_zoom: Some(13.5), ..VECTOR_TILES }).is_empty());
        assert!(tile_indices(&vp, &TilesetOptions { visible_max_zoom: Some(13.0), ..VECTOR_TILES }).is_empty());
        assert!(!tile_indices(&vp, &TilesetOptions { visible_min_zoom: Some(13.0), visible_max_zoom: Some(14.0), ..VECTOR_TILES }).is_empty());
    }

    #[test]
    fn far_off_the_globe_takes_coarser_tiles_and_a_low_pitch_flat_map_does_not() {
        // the globe tilted: tiles towards the horizon are coarser than the rule's zoom
        let globe = view(18.0, 59.0, 6.0, 55.0, 0.0);
        let zooms: Vec<u32> = tile_indices(&globe, &VECTOR_TILES).iter().map(|t| t.z).collect();
        assert!(zooms.contains(&6) && zooms.iter().any(|&z| z < 6) && zooms.iter().all(|&z| z <= 6), "{zooms:?}");
        // the flat map at 60°: all at the rule's zoom; at 75°: coarser far off
        let flat60 = view(18.0, 59.0, 13.0, 60.0, 0.0);
        assert!(tile_indices(&flat60, &VECTOR_TILES).iter().all(|t| t.z == 13));
        let flat75 = view(18.0, 59.0, 13.0, 75.0, 0.0);
        let zooms: Vec<u32> = tile_indices(&flat75, &VECTOR_TILES).iter().map(|t| t.z).collect();
        assert!(zooms.contains(&13) && zooms.iter().any(|&z| z < 13) && zooms.iter().all(|&z| z <= 13), "{zooms:?}");
    }

    #[test]
    fn selected_tiles_never_overlap() {
        for vp in [view(18.0, 59.0, 6.0, 55.0, 0.0), view(18.0, 59.0, 13.0, 75.0, 40.0), view(0.0, 0.0, 0.5, 0.0, 0.0)] {
            let tiles = tile_indices(&vp, &IMAGERY_TILES);
            assert!(!tiles.is_empty());
            for (i, a) in tiles.iter().enumerate() {
                for b in &tiles[i + 1..] {
                    assert!(!a.contains(*b) && !b.contains(*a), "{a:?} and {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_far_side_of_the_globe_is_left_out() {
        // looking at lon 0 straight down from far out: nothing around lon 180 is selected
        let vp = view(0.0, 0.0, 2.0, 0.0, 0.0);
        let tiles = tile_indices(&vp, &VECTOR_TILES);
        assert!(!tiles.is_empty());
        for t in &tiles {
            let b = t.bounds();
            assert!(b.east > -100.0 && b.west < 100.0, "{t:?} {b:?}");
        }
    }

    #[test]
    fn tile_ids_parents_children_and_bounds() {
        let t = TileIndex::new(4503, 2390, 13);
        assert_eq!(t.id(), "4503-2390-13");
        assert_eq!(t.parent(), Some(TileIndex::new(2251, 1195, 12)));
        assert_eq!(TileIndex::ROOT.parent(), None);
        assert_eq!(
            TileIndex::new(1, 2, 3).children(),
            [TileIndex::new(2, 4, 4), TileIndex::new(2, 5, 4), TileIndex::new(3, 4, 4), TileIndex::new(3, 5, 4)]
        );
        for c in t.children() {
            assert_eq!(c.parent(), Some(t));
            assert!(t.contains(c) && !c.contains(t));
        }
        assert!(TileIndex::ROOT.contains(t) && t.contains(t));
        assert!(!TileIndex::new(4502, 2390, 13).contains(TileIndex::new(9008, 4780, 14)));
        // the world, and its four quarters
        let w = TileIndex::ROOT.bounds();
        assert_eq!((w.west, w.east), (-180.0, 180.0));
        assert!((w.north - 85.051_128_779_806_59).abs() < 1e-12 && (w.south + 85.051_128_779_806_59).abs() < 1e-12);
        let ne = TileIndex::new(1, 0, 1).bounds();
        assert_eq!((ne.west, ne.east, ne.south), (0.0, 180.0, 0.0));
        // children tile their parent exactly
        let p = t.bounds();
        let [a, _, _, d] = t.children().map(|c| c.bounds());
        assert_eq!((a.west, a.north, d.east, d.south), (p.west, p.north, p.east, p.south));
        assert_eq!(a.east, d.west);
        assert_eq!(osm_tile_to_lng_lat(0.5, 0.5, 0), [0.0, 0.0]);
    }

    fn refine(n: usize) -> Vec<RefineTile> {
        vec![RefineTile::default(); n]
    }

    #[test]
    fn the_tree_links_each_tile_to_its_nearest_cached_ancestor() {
        // cached: a z3 tile, one of its z5 grandchildren (its z4 parent not cached), a z4 tile
        // elsewhere, and the root
        let indices = [TileIndex::new(1, 1, 3), TileIndex::new(4, 5, 5), TileIndex::new(9, 9, 4), TileIndex::ROOT];
        let mut tiles = refine(4);
        link_tree(&mut tiles, &indices, 0);
        assert_eq!(tiles[1].parent, Some(0));
        assert_eq!(tiles[0].parent, Some(3));
        assert_eq!(tiles[2].parent, Some(3));
        assert_eq!(tiles[3].parent, None);
        assert_eq!(tiles[3].children, Some(vec![0, 2]));
        assert_eq!(tiles[0].children, Some(vec![1]));
        assert_eq!(tiles[1].children, Some(vec![]));
        // the walk stops at the tileset's min zoom
        link_tree(&mut tiles, &indices, 3);
        assert_eq!(tiles[0].parent, None);
        assert_eq!(tiles[1].parent, Some(0));
        assert_eq!(tiles[2].parent, None);
    }

    #[test]
    fn best_available_shows_a_loaded_ancestor_while_a_tile_loads() {
        let indices = [TileIndex::new(0, 0, 1), TileIndex::new(0, 0, 2), TileIndex::new(0, 0, 3), TileIndex::new(1, 0, 3)];
        let mut tiles = refine(4);
        link_tree(&mut tiles, &indices, 0);
        // the two z3 tiles are selected, the z1 one is loaded, the z2 one not
        tiles[2].is_selected = true;
        tiles[3].is_selected = true;
        tiles[0].ready = true;
        best_available(&mut tiles);
        assert_eq!(tiles.iter().map(|t| t.is_visible).collect::<Vec<_>>(), [true, false, false, false]);
        // once one is in, it is drawn, and the ancestor still stands in for the other (both are
        // drawn: the runtime draws finer tiles over coarser ones)
        tiles[2].ready = true;
        best_available(&mut tiles);
        assert_eq!(tiles.iter().map(|t| t.is_visible).collect::<Vec<_>>(), [true, false, true, false]);
        // both in: the ancestor is no longer drawn
        tiles[3].ready = true;
        best_available(&mut tiles);
        assert_eq!(tiles.iter().map(|t| t.is_visible).collect::<Vec<_>>(), [false, false, true, true]);
    }

    #[test]
    fn best_available_falls_back_to_loaded_descendants() {
        // zooming out: the z2 tile is selected and loading; of its cached z3 and z4 descendants,
        // the loaded ones stand in, an unloaded z3 hands down to its loaded z4 child
        let indices = [TileIndex::new(0, 0, 2), TileIndex::new(0, 0, 3), TileIndex::new(1, 1, 3), TileIndex::new(2, 2, 4)];
        let mut tiles = refine(4);
        link_tree(&mut tiles, &indices, 0);
        tiles[0].is_selected = true;
        tiles[1].ready = true;
        tiles[3].ready = true;
        best_available(&mut tiles);
        assert_eq!(tiles.iter().map(|t| t.is_visible).collect::<Vec<_>>(), [false, true, false, true]);
        // nothing selected: nothing drawn
        tiles[0].is_selected = false;
        best_available(&mut tiles);
        assert!(tiles.iter().all(|t| !t.is_visible));
    }

    #[test]
    fn requests_go_selected_first_then_stand_ins_nearest_the_centre_first() {
        let vp = view(18.07, 59.33, 13.3, 0.0, 0.0);
        // the tile under the view's centre
        let n = 2f64.powi(13);
        let [wx, wy] = lng_lat_to_world([18.07, 59.33]);
        let centre = TileIndex::new((wx / 512.0 * n) as u32, ((512.0 - wy) / 512.0 * n) as u32, 13);
        assert_eq!(tile_distance_priority(&vp, centre), 0.0);
        assert_eq!(request_priority(&vp, centre, true, true), Some(0.0));
        assert_eq!(request_priority(&vp, centre, false, true), Some(VISIBLE_TILE_PRIORITY));
        assert_eq!(request_priority(&vp, centre, false, false), None);
        // farther tiles come later, in squared pixels
        let east = TileIndex::new(centre.x + 2, centre.y, 13);
        let farther = TileIndex::new(centre.x + 4, centre.y, 13);
        let (e, f) = (tile_distance_priority(&vp, east), tile_distance_priority(&vp, farther));
        assert!(0.0 < e && e < f && f < MAX_TILE_DISTANCE_PRIORITY, "{e} {f}");
        // one tile of 512 px at zoom 13.3 is 630 px wide: the edge of the tile two east is
        // between 1 and 2 tiles from the centre
        let tile_px = 512.0 * 2f64.powf(0.3);
        assert!(e.sqrt() > tile_px && e.sqrt() < 2.0 * tile_px, "{}", e.sqrt());
        // the distance never reaches the next tier
        let antipode = TileIndex::new(0, 0, 13);
        assert_eq!(tile_distance_priority(&vp, antipode), MAX_TILE_DISTANCE_PRIORITY);
        assert!(request_priority(&vp, antipode, true, false).unwrap() < VISIBLE_TILE_PRIORITY);
    }

    #[test]
    fn the_globe_projects_tiles_for_their_priority() {
        let vp = view(10.0, 35.0, 1.9, 0.0, 0.0);
        // the z2 tile with lon 0..90, lat 0..66.5 holds the centre
        assert_eq!(tile_distance_priority(&vp, TileIndex::new(2, 1, 2)), 0.0);
        let west = tile_distance_priority(&vp, TileIndex::new(1, 1, 2));
        assert!(west > 0.0 && west < 900.0 * 900.0, "{west}");
    }

    #[test]
    fn the_camera_viewports_drive_the_distance_selection() {
        // lod's distance rule over the camera's viewports: near tiles fine, far ones coarse
        let lod = Lod::new((10..=16).collect::<Vec<_>>());
        let flat = view(18.07, 59.33, 15.5, 60.0, 20.0);
        let tiles = distance_tile_indices(&flat, &lod, None, 0.0);
        assert!(!tiles.is_empty());
        let zooms: Vec<u32> = tiles.iter().map(|t| t.z).collect();
        assert!(zooms.iter().any(|&z| z >= 15) && zooms.iter().any(|&z| z < 15), "{zooms:?}");
        // the same view on the globe's side of the swap selects tiles too
        let globe = view(18.07, 59.33, 11.0, 30.0, 0.0);
        assert!(!distance_tile_indices(&globe, &lod, None, 0.0).is_empty());
        // `ahead` keeps the viewport's kind, size, pitch and bearing
        let ahead = LodViewport::ahead(&globe, 18.1, 59.4, 13.0);
        assert!(ahead.is_globe());
        assert_eq!((ahead.width, ahead.height, ahead.pitch, ahead.bearing, ahead.zoom), (1440.0, 900.0, 30.0, 0.0, 13.0));
        let ahead = LodViewport::ahead(&flat, 18.1, 59.4, 11.0);
        assert!(!ahead.is_globe());
        assert_eq!((ahead.pitch, ahead.bearing, ahead.zoom), (60.0, 20.0, 11.0));
        assert_eq!(LodViewport::units_per_meter(&flat), flat.distance_scales.units_per_meter[2]);
    }
}
