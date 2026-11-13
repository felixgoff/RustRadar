//! What is draped over the 3D terrain (item Z2; doc 04 §12, §14.3, §14.5; plan 1.4.4, 1.4.12): the
//! terrain tiles' cover textures, the passes that draw the draped layers into them, and the surface
//! drawn with them (`shaders/terrain.wgsl`).
//!
//! Today's app puts deck.gl's `TerrainExtension` on every layer but the earth, the surface and the
//! clouds once the *terrain* setting is on (`onTerrain`, G:3059-3067). The flat ones (no extrusion,
//! no instance positions) are draped: `TerrainEffect` keeps a `TerrainCover` for each terrain tile,
//! draws them into it from straight above (`TerrainPass.renderTerrainCover`: an orthographic Web
//! Mercator viewport over the tile's square, a texture cleared transparent, blending on, depth
//! `always`), skips them in the frame itself (`TERRAIN_MODE_SKIP`), and the tile's mesh samples its
//! cover (`TERRAIN_MODE_USE_COVER`). Here:
//!
//! - **Which layers** ([`Drape`](crate::layers::Drape)): the renderer leaves the
//!   [`Drape::Cover`](crate::layers::Drape::Cover) layers (the realistic land, land cover, ground and
//!   ground rails, the dark map's land and vector map, the imagery, the relief, the night) out of
//!   the map pass while the terrain is drawn, and draws them in each cover pass
//!   ([`Pass::Cover`](crate::layers::Pass::Cover)); a [`Drape::Part`](crate::layers::Drape::Part)
//!   layer draws its flat parts there and the rest in the map pass
//!   ([`DrawCx::draws_draped`](crate::layers::DrawCx::draws_draped)): the rail map's lines (its
//!   labels and stations stand), both building layers' shadows and the realistic one's foot shade
//!   (the buildings stand); and since item G3 the radar's ground layers (RainViewer's and the
//!   measured volumes' grounds; the lifted stack stands), the signal blocks' lines (their signals
//!   and texts stand), the selected train's route dashes and its way to the stop (its stops
//!   stand), the trail's shadow and the filed plan (the waypoints stand). A pass draws with the
//!   [`Globals`] of [`cover_globals`]: the frames the
//!   layers pushed for the map's camera are measured from the view centre, which these globals keep,
//!   so the same frames land in the tile's square. In a cover pass `globals.mode` is the flat map's,
//!   `globals.mercator.y` is 1 (`rl_clear` then gives 1: the draped ground takes no air, as
//!   `rlight.cover` does in deck's pass), and `globals.camera_render` is the map's own camera
//!   (`aimMapEye`, `rlight.eye`), so what the realistic ground lights by the view (water's sheen) is
//!   lit from where the map is seen. Pixels are the cover's texels (deck draws the pass at
//!   `devicePixelRatio` 1), so lines given in pixels are that many texels wide: at a tile's zoom
//!   plus one, thinner on the screen close in, as today.
//! - **The covers** ([`Covers`], `scene::terrain::cover`): one `Rgba8Unorm` texture a terrain tile
//!   drawn, its size by today's patched rule (1024 texels; the deepest tiles with the view's zoom,
//!   to 2048), kept within [`BUDGET`], drawn at most [`COVERS_PER_FRAME`] a frame, never-drawn
//!   first. A cover is drawn again when a cover layer's tiles drawn change within its square
//!   ([`crate::layers::drawn_changes`]: `update_tiles`, and `note_drawn_changes` for shadows recast
//!   under a new sun), where what a layer that is not tiled draws changed ([`DrawnArea`], noted by
//!   `note_drawn_area`: only the covers that show the change, item G3), and every cover when what
//!   the cover layers draw changes everywhere
//!   ([`CoverLook`]: the basemap, the projection, the weather and *daylight*, the ground's and the
//!   relief's settings and the relief's zoom-stepped opacity, the dark map's land, roads and
//!   runways, the night's minute, the realistic textures arriving). Animated effects (water, the
//!   clouds' shadows) stay as they were when the cover was drawn, as today. Textures are made when
//!   their first pass is due, so a terrain layer nobody draws holds none.
//! - **The passes** ([`CoverJob`]): encoded and submitted in the renderer's `prepare` (so a shot,
//!   which only prepares while it waits, has its covers), each into a [`CHUNK`]-sized scratch with
//!   the map pass's sample count and depth and stencil (the layers' pipelines are the map pass's),
//!   resolved and copied into the cover; a 2048 cover is four chunks. The stencil is cleared to 0,
//!   so the ground's tile clips and the shadows' rule work as in the map pass.
//! - **The surface** ([`TerrainPipeline`]): `terrain.wgsl`: the cover over `MESH_COLOR` and lit as
//!   the bare mesh is (Z1: deck's `material: false` lights the draped colour too), with the
//!   realistic map's air over it (`TERRAIN_AIR`, `fs_air`).
//!
//! As today (seen at Zermatt, `TERRAIN_VIEWS`): the draped layers pick their tiles for the map's
//! camera against the sea (deck's tile layers get no `zRange`; nor does `distanceTileIndices`), so
//! with the camera raised over high ground the 2D map's tiles round the camera are not asked for and
//! the covers there show what is drawn everywhere (the land) until the camera's view of the sea
//! reaches them. Giving those selections the terrain's range would be a change from today's app.
//!
//! Deviations, each with its reason:
//! - Budget and pace: deck keeps a cover for every cached terrain tile and draws all that need it
//!   at once; here at most two a frame (a tile shows the bare mesh colour, or its last picture, a
//!   few frames longer) and within the memory budget, the smallest on the screen halved first.
//! - The cover passes draw with the map pass's 4× MSAA (resolved), where deck's framebuffer has
//!   none: the layers' pipelines are built for the map pass. Edges in the cover are smoother.
//! - deck's cover framebuffer has no depth or stencil attachment, so its shadows darken twice where
//!   they overlap; here the stencil keeps the shadows' rule (plan 1.4.12). Depth is constant in a
//!   cover pass, so depth-tested and depth-writing draws all land, in order, as with `always`.
//! - Every cover layer draws all of its tiles in each cover pass (the pass's viewport clips them);
//!   deck hands a cover only the tile sublayers whose bounds meet it (`getIntersectingLayers`).
//! - A draped layer that is not tiled and changes (the train route as the train moves, the trail's
//!   last leg, the blocks as trains take them, a new set of radar volumes): deck draws every cover
//!   it is in again (`layerNeedsRedraw`), here only those its change shows in ([`DrawnArea`]), so
//!   such a layer costs a cover or two a scene refresh, within [`COVERS_PER_FRAME`] (item G3).
//! - Not draped (deck drapes them): the selected flight's trail and its route's estimates keep
//!   their true heights (plan section 6, item 3), and so do the radar's lifted layers and walls
//!   (deck would flatten the stack onto the ground, `terrain::ground`). The platforms (an extruded
//!   area, deck: offset) are drawn flat into the cover with the rest of the ground rails.
//! - What changes a cover layer's picture without changing its tiles or its settings (a style a
//!   layer works out from the zoom other than the relief's opacity) shows at the cover's next
//!   drawing.

use std::collections::HashMap;

use bytemuck::Zeroable;
use glam::{DMat4, DVec3};
use rustradar_core::geo::Basemap;
use rustradar_scene::camera::{WORLD_SIZE, lng_lat_to_world, mercator_units_per_meter, world_to_lng_lat};
use rustradar_scene::hillshade::HillshadeLayer;
pub use rustradar_scene::terrain::cover::{
    BUDGET, COVERS_PER_FRAME, Cover, CoverCache, CoverState, MAX_COVER_SIZE, Want, cover_bytes, cover_size, cover_zoom,
};
use rustradar_scene::tess::vector::true_aeroways;

use super::mesh::{SurfaceGpu, VERTEX_LAYOUT, style_uniform};
use crate::frame::{FrameSlot, Globals, MODE_FLAT, ViewFrame, tile_extent};
use crate::gpu::{DEPTH_FORMAT, Gpu};
use crate::layers::{DrawCx, Init, PrepareCx, TileIndex, TileSet, ground, hillshade, night, sky};
use crate::look::{RLight, frame_look, light_origin};
use crate::pipelines::mesh::LightsUniform;
use crate::pipelines::{Desc, build};
use crate::renderer::SceneInput;
use crate::shaders;
use crate::state::DrawState;
use crate::target::MAP_FORMAT;

/// `shaders/terrain.wgsl`.
pub const SHADER: &str = include_str!("../../shaders/terrain.wgsl");

/// The bind group of the light (binding 0), the rig (1) and the style (2).
pub const STYLE_GROUP: u32 = 2;
/// The bind group of a tile's cover (texture 0, sampler 1).
pub const COVER_GROUP: u32 = 3;
/// The side of the scratch a cover pass draws into, texels: a cover up to this size is one pass,
/// a larger one (2048) is cut into passes of this size.
pub const CHUNK: u32 = 1024;
/// `globals.mercator.y` in a cover pass (`lib/rlight.wgsl`'s `rl_clear` reads it).
pub const COVER_PASS: f32 = 1.0;
/// The deepest zoom of the surface's tiles (`maxZoom: SURFACE_LOD.levels.length − 1`).
pub const MAX_ZOOM: u32 = 16;

/// What the cover layers draw everywhere: when it changes, every cover is drawn again (deck's
/// `layerNeedsRedraw` of a draped layer that is not tiled, or whose props change).
#[derive(Debug, Clone, PartialEq)]
pub struct CoverLook {
    basemap: Basemap,
    globe: bool,
    sky: sky::Input,
    ground: ground::Input,
    /// The relief as the layer draws it, opacity in its zoom's steps (`hillshade::layer_for`).
    hillshade: Option<HillshadeLayer>,
    /// The dark map's land.
    land: bool,
    /// The dark map's roads and water, and whether its runways are at their true width.
    vector_map: Option<bool>,
    /// The night's minute on the dark and satellite maps (its bands are made each minute).
    night: Option<i64>,
    /// The realistic textures are still loading (the ground draws with stand-ins until then).
    textures_pending: bool,
}

/// [`CoverLook`] of `scene` seen in `view`, the realistic textures still loading or not.
pub fn cover_look(scene: &SceneInput, view: &ViewFrame, textures_pending: bool) -> CoverLook {
    let globe = view.is_globe();
    let dark = scene.basemap == Basemap::Dark;
    CoverLook {
        basemap: scene.basemap,
        globe,
        sky: scene.sky,
        ground: scene.ground.clone(),
        hillshade: scene.hillshade.shown.then(|| hillshade::layer_for(scene.basemap, scene.hillshade.terrain, !globe, view.view.zoom)).flatten(),
        land: dark && scene.earth.shown,
        vector_map: (dark && scene.vector_map.shown).then(|| true_aeroways(view.view.zoom, view.view.latitude)),
        night: night::drawn(scene).then(|| (scene.feed_now_ms / 60_000.0).floor() as i64),
        textures_pending,
    }
}

/// The map's camera in `view`: world Mercator `[x, y]` and metres above the sea (`aimMapEye`:
/// `viewport.unprojectPosition(viewport.cameraPosition)`).
pub fn map_eye(view: &ViewFrame) -> ([f64; 2], f64) {
    let [lng, lat, h] = view.viewport.unproject_position(view.viewport.camera_position);
    (lng_lat_to_world([lng, lat]), h)
}

/// A world x difference moved into `[−256, 256)`.
fn wrap_world_x(dx: f64) -> f64 {
    dx - WORLD_SIZE * ((dx + WORLD_SIZE / 2.0) / WORLD_SIZE).floor()
}

fn mat(m: DMat4) -> [[f32; 4]; 4] {
    m.as_mat4().to_cols_array_2d()
}

/// The globals of a cover pass over `tile`'s square, its cover `size` texels a side drawn in `n × n`
/// chunks, this one `i` east and `j` south of the north-west one, for the frames of `view` (whose
/// view centre local offsets are measured from, as theirs are).
///
/// An orthographic camera straight down over the chunk's square, north up (row 0 north, as the
/// tile's `v` runs south); every depth 0.5, so depth tests and writes keep the drawing order. The
/// flat map's terms at the tile's middle: its latitude, common units per metre and the Mercator y
/// from the view centre to it (heights scale from there, `flat_rel`); pixels are texels; the camera
/// is the map's ([`map_eye`]); `mercator.y` is [`COVER_PASS`]. `view_proj` is the same camera:
/// common offsets (the map's dynamic instances) are not drawn into covers.
pub fn cover_globals(view: &ViewFrame, tile: TileIndex, size: u32, n: u32, i: u32, j: u32) -> Globals {
    let reference = light_origin(view);
    let frame = view.tile(tile, 0.0);
    // the tile's north-west corner from the view centre, as its frame has it
    let ax = f64::from(frame.offset_hi[0]) + f64::from(frame.offset_lo[0]);
    let ay = f64::from(frame.offset_hi[1]) + f64::from(frame.offset_lo[1]);
    let extent = tile_extent(tile.z);
    let chunk = extent / f64::from(n);
    let west = ax + chunk * f64::from(i);
    let south = ay - chunk * f64::from(j + 1);
    let s = 2.0 / chunk;
    let ortho = DMat4::from_cols_array(&[s, 0.0, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -s * west - 1.0, -s * south - 1.0, 0.5, 1.0]);
    let middle_y = ay - extent / 2.0;
    let latitude = world_to_lng_lat([reference.x + ax + extent / 2.0, reference.y + middle_y])[1];
    let units_per_metre = mercator_units_per_meter(latitude);
    let (sin, cos) = latitude.to_radians().sin_cos();
    let (eye_world, eye_height) = map_eye(view);
    let eye = DVec3::new(wrap_world_x(eye_world[0] - reference.x), eye_world[1] - reference.y, eye_height * units_per_metre);
    let texels = f64::from(size / n);
    let pixels_per_unit = f64::from(size) / extent;
    Globals {
        view_proj: mat(ortho),
        centre_cam: mat(ortho),
        centre_offset: [0.0; 3],
        mode: MODE_FLAT,
        centre: [cos as f32, sin as f32, units_per_metre as f32, 0.0],
        camera_common: eye.as_vec3().into(),
        pixels_per_unit: pixels_per_unit as f32,
        camera_render: eye.as_vec3().into(),
        metres_per_pixel: (1.0 / (pixels_per_unit * units_per_metre)) as f32,
        viewport: [texels as f32, texels as f32, 1.0, view.globals.viewport[3]],
        mercator: [(-middle_y) as f32, COVER_PASS, 0.0, 0.0],
    }
}

/// How large `tile` is on the screen of `view` ([`Want::weight`]): its extent over its middle's
/// distance from the map's camera (at the middle of `z_range`, its heights).
pub fn screen_weight(view: &ViewFrame, tile: TileIndex, z_range: [f64; 2]) -> f64 {
    let extent = tile_extent(tile.z);
    let middle = [(f64::from(tile.x) + 0.5) * extent, WORLD_SIZE - (f64::from(tile.y) + 0.5) * extent];
    let (eye, height) = map_eye(view);
    let latitude = world_to_lng_lat(middle)[1];
    let ground = 0.5 * (z_range[0].max(0.0) + z_range[1].max(0.0));
    let d = DVec3::new(wrap_world_x(eye[0] - middle[0]), eye[1] - middle[1], (height - ground) * mercator_units_per_meter(latitude));
    extent / d.length().max(extent * 1e-6)
}

/// Where what a layer draws into the covers changed other than through its tiles (item G3): a box
/// in world Mercator (deck's 512 units, `y` north) and how far what is drawn there reaches past it,
/// in a cover's texels (a line's joints and caps; covers draw pixels as texels). deck redraws every
/// cover a changed draped layer is in (`layerNeedsRedraw`, a layer that is not a tile's is in all
/// of them); a cover that does not show the box draws the same picture again, so here only those
/// that do are put out of date ([`Covers::invalidate_areas`]), which keeps a layer whose content
/// changes at each scene refresh (a moving train's way to its stop, the trail's last leg) to the
/// one or two covers it changes in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawnArea {
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub reach_px: f64,
}

/// How far a line `width_px` wide reaches past its points, texels: deck's miter joints to 4 half
/// widths (its `miterLimit`), and a texel for the edge's antialiasing.
pub fn line_reach(width_px: f64) -> f64 {
    2.0 * width_px + 1.0
}

impl DrawnArea {
    /// The box round `points` (`[lon, lat]`; NaN read as 0, as the layers draw them), none for none.
    pub fn round(points: impl IntoIterator<Item = [f64; 2]>, reach_px: f64) -> Option<DrawnArea> {
        let or0 = |x: f64| if x.is_nan() { 0.0 } else { x };
        let mut area: Option<DrawnArea> = None;
        for [lon, lat] in points {
            let w = lng_lat_to_world([or0(lon), or0(lat)]);
            if !(w[0].is_finite() && w[1].is_finite()) {
                continue;
            }
            let a = area.get_or_insert(DrawnArea { min: w, max: w, reach_px });
            a.min = [a.min[0].min(w[0]), a.min[1].min(w[1])];
            a.max = [a.max[0].max(w[0]), a.max[1].max(w[1])];
        }
        area
    }

    /// A box in world Mercator, `[west, south]` to `[east, north]`.
    pub fn world(min: [f64; 2], max: [f64; 2], reach_px: f64) -> DrawnArea {
        DrawnArea { min, max, reach_px }
    }

    /// The box round both, reaching as far as the farther.
    pub fn join(self, other: DrawnArea) -> DrawnArea {
        DrawnArea {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
            reach_px: self.reach_px.max(other.reach_px),
        }
    }

    /// `a` and `b` joined, either missing.
    pub fn either(a: Option<DrawnArea>, b: Option<DrawnArea>) -> Option<DrawnArea> {
        match (a, b) {
            (Some(a), Some(b)) => Some(a.join(b)),
            (a, b) => a.or(b),
        }
    }

    /// `tile`'s square grown by what the area reaches in a cover of `size` texels over it
    /// (`[west, south, east, north]`, world Mercator).
    fn grown_square(&self, tile: TileIndex, size: u32) -> [f64; 4] {
        let extent = tile_extent(tile.z);
        let pad = self.reach_px * extent / f64::from(size.max(1));
        let west = f64::from(tile.x) * extent;
        let north = WORLD_SIZE - f64::from(tile.y) * extent;
        [west - pad, north - extent - pad, west + extent + pad, north + pad]
    }

    /// Whether a cover of `size` texels over `tile`'s square shows something drawn in the area (on
    /// either side of the antimeridian).
    pub fn meets(&self, tile: TileIndex, size: u32) -> bool {
        let [west, south, east, north] = self.grown_square(tile, size);
        [-WORLD_SIZE, 0.0, WORLD_SIZE].iter().any(|&shift| {
            self.min[0] + shift < east && self.max[0] + shift > west && self.min[1] < north && self.max[1] > south
        })
    }

    /// A tile at [`PROBE_ZOOM`] in `tile`'s square nearest the area: what [`CoverCache::invalidate`]
    /// is handed for the cover of `tile`, so that it puts out of date that cover (and those over or
    /// under it at that spot, which show the place too) and not every cover under a coarse one.
    fn probe(&self, tile: TileIndex) -> TileIndex {
        let extent = tile_extent(tile.z);
        let west = f64::from(tile.x) * extent;
        let north = WORLD_SIZE - f64::from(tile.y) * extent;
        let middle = [(self.min[0] + self.max[0]) / 2.0, (self.min[1] + self.max[1]) / 2.0];
        // just inside the square, so the probe is in it whatever the rounding
        let inset = extent * 1e-6;
        let x = middle[0].clamp(west + inset, west + extent - inset);
        let y = middle[1].clamp(north - extent + inset, north - inset);
        let probe = tile_extent(PROBE_ZOOM);
        let side = 1u64 << PROBE_ZOOM;
        let column = ((x / probe).floor() as u64).min(side - 1) as u32;
        let row = (((WORLD_SIZE - y) / probe).floor() as u64).min(side - 1) as u32;
        TileIndex::new(column, row, PROBE_ZOOM)
    }
}

/// The zoom of [`DrawnArea::probe`]'s tiles: deeper than any terrain tile's.
const PROBE_ZOOM: u32 = 28;

/// What of a path (`[lon, lat]` points, drawn when it has two or more) drew differently from `old`
/// to `new`, none when nothing did: a line dashed from its start (deck's `highPrecisionDash`, the
/// dashes measured along the whole path) differs from where the two part to its end; a solid one
/// only between where they part and where they meet again, the vertices either side included
/// (their joints change). `reach_px` as in [`DrawnArea`].
pub fn path_changes(old: &[[f64; 2]], new: &[[f64; 2]], dashed: bool, reach_px: f64) -> Option<DrawnArea> {
    let drawn = |p: &'_ [[f64; 2]]| -> usize { if p.len() >= 2 { p.len() } else { 0 } };
    let (old, new) = (&old[..drawn(old)], &new[..drawn(new)]);
    if old == new {
        return None;
    }
    let shorter = old.len().min(new.len());
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = if dashed { 0 } else { old.iter().rev().zip(new.iter().rev()).take(shorter - prefix).take_while(|(a, b)| a == b).count() };
    let part = |p: &[[f64; 2]]| {
        let start = prefix.saturating_sub(1);
        let end = if suffix > 0 { p.len() - suffix + 1 } else { p.len() };
        DrawnArea::round(p[start..end.max(start)].iter().copied(), reach_px)
    };
    DrawnArea::either(part(old), part(new))
}

/// [`path_changes`] over two lists of paths drawn as one layer's rows, row by row (a row only one
/// list has changed whole).
pub fn paths_changes(old: &[Vec<[f64; 2]>], new: &[Vec<[f64; 2]>], dashed: bool, reach_px: f64) -> Option<DrawnArea> {
    let rows = old.len().max(new.len());
    (0..rows).fold(None, |area, i| {
        let (a, b) = (old.get(i).map_or(&[][..], Vec::as_slice), new.get(i).map_or(&[][..], Vec::as_slice));
        DrawnArea::either(area, path_changes(a, b, dashed, reach_px))
    })
}

/// A cover on the GPU.
struct CoverTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
}

/// What a cover pass draws into: a [`CHUNK`]² colour of the map pass's sample count, its
/// depth/stencil, and (with MSAA) a single-sample one to resolve into before copying.
struct Scratch {
    colour: wgpu::Texture,
    colour_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl Scratch {
    fn new(device: &wgpu::Device, samples: u32) -> Self {
        let texture = |label, format, sample_count, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: CHUNK, height: CHUNK, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let colour = texture("terrain cover scratch", MAP_FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let depth = texture("terrain cover depth", DEPTH_FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = (samples > 1).then(|| {
            let t = texture("terrain cover resolve", MAP_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
            let v = t.create_view(&Default::default());
            (t, v)
        });
        Scratch { colour_view: colour.create_view(&Default::default()), colour, depth: depth.create_view(&Default::default()), resolve }
    }

    fn bytes(samples: u32) -> usize {
        let texels = CHUNK as usize * CHUNK as usize;
        // colour and depth/stencil at the sample count, 4 bytes each, and the resolve
        texels * 4 * 2 * samples as usize + if samples > 1 { texels * 4 } else { 0 }
    }
}

/// Where one chunk's pass ends up.
enum Target {
    /// Resolved (or drawn) straight into the cover: the chunk is the whole cover and [`CHUNK`]
    /// wide.
    Cover(wgpu::TextureView),
    /// Copied from the scratch into the cover at this texel.
    Copy { texture: wgpu::Texture, origin: [u32; 2] },
}

/// One chunk of a cover pass.
struct Chunk {
    globals: wgpu::BindGroup,
    texels: u32,
    into: Target,
}

/// One cover to draw this frame (its chunks), made by [`Covers::jobs`].
pub struct CoverJob {
    pub tile: TileIndex,
    chunks: Vec<Chunk>,
    colour: wgpu::Texture,
    colour_view: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl CoverJob {
    /// Records the cover's passes: each chunk cleared transparent (depth 1, stencil 0) and drawn by
    /// `draw` with the chunk's globals (bind them as group 0; the stencil reference and the zero
    /// frame are the caller's, as in the map pass), resolved or copied into the cover.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, draw: &mut dyn FnMut(&mut wgpu::RenderPass<'_>, &wgpu::BindGroup)) {
        for chunk in &self.chunks {
            let msaa = self.resolve.is_some();
            let (view, resolve_target) = match (&chunk.into, &self.resolve) {
                (Target::Cover(cover), Some(_)) => (&self.colour_view, Some(cover)),
                (Target::Cover(cover), None) => (cover, None),
                (Target::Copy { .. }, Some((_, resolve))) => (&self.colour_view, Some(resolve)),
                (Target::Copy { .. }, None) => (&self.colour_view, None),
            };
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("terrain cover"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: if msaa { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                        stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                    }),
                    ..Default::default()
                });
                let texels = chunk.texels as f32;
                pass.set_viewport(0.0, 0.0, texels, texels, 0.0, 1.0);
                draw(&mut pass, &chunk.globals);
            }
            if let Target::Copy { texture, origin } = &chunk.into {
                let source = self.resolve.as_ref().map_or(&self.colour, |(t, _)| t);
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: source, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: origin[0], y: origin[1], z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d { width: chunk.texels, height: chunk.texels, depth_or_array_layers: 1 },
                );
            }
        }
    }
}

/// The terrain tiles' covers: their textures, their bookkeeping, and what their passes need.
pub struct Covers {
    cache: CoverCache,
    textures: HashMap<TileIndex, CoverTexture>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// A transparent texel, for a tile with no cover (yet).
    none: wgpu::BindGroup,
    samples: u32,
    globals_layout: wgpu::BindGroupLayout,
    /// The cover passes' globals, reused frame to frame.
    globals: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    scratch: Option<Scratch>,
    look: Option<CoverLook>,
    /// Covers drawn since made (for tests and logs).
    drawn: usize,
    /// Textures planned and not made yet: made before the covers' next passes ([`Self::jobs`]), so
    /// covers nobody draws hold none.
    planned: Vec<(TileIndex, u32)>,
    /// A renderer draws these covers (it has asked for their passes): only then does a tile drawn
    /// wait for its cover ([`Self::waiting`]).
    attached: bool,
}

impl Covers {
    pub fn new(init: &Init<'_>) -> Self {
        Self::with_budget(init, BUDGET)
    }

    /// Covers within `budget` bytes (tests).
    pub fn with_budget(init: &Init<'_>, budget: usize) -> Self {
        let device = &init.gpu.device;
        let layout = cover_layout(device);
        // deck's cover texture: linear, no mipmaps; clamped at the tile's edges (its overlap and skirt)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain cover"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain no cover"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MAP_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        init.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &blank, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &[0, 0, 0, 0],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: None },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let none = cover_group(device, &layout, &blank.create_view(&Default::default()), &sampler);
        Covers {
            cache: CoverCache::new(budget),
            textures: HashMap::new(),
            layout,
            sampler,
            none,
            samples: init.samples,
            globals_layout: init.bindings.globals_layout.clone(),
            globals: Vec::new(),
            scratch: None,
            look: None,
            drawn: 0,
            planned: Vec::new(),
            attached: false,
        }
    }

    /// The bookkeeping (sizes, states, the budget).
    pub fn cache(&self) -> &CoverCache {
        &self.cache
    }

    /// From the next frame on, the covers fit `budget` bytes (tests; the app keeps [`BUDGET`]).
    pub fn set_budget(&mut self, budget: usize) {
        self.cache.set_budget(budget);
    }

    /// The bytes the covers' textures will hold once those planned are made (within the budget).
    pub fn bytes(&self) -> usize {
        self.cache.bytes()
    }

    /// The bytes the cover textures on the GPU hold now (within the budget).
    pub fn texture_bytes(&self) -> usize {
        self.textures.values().map(|t| cover_bytes(t.texture.width())).sum()
    }

    /// The cover textures on the GPU now.
    pub fn textures(&self) -> usize {
        self.textures.len()
    }

    /// The bytes the cover passes' scratch holds while the terrain is drawn (outside the budget, as
    /// the map target's attachments are).
    pub fn scratch_bytes(&self) -> usize {
        if self.scratch.is_some() { Scratch::bytes(self.samples) } else { 0 }
    }

    /// Covers drawn since made.
    pub fn drawn(&self) -> usize {
        self.drawn
    }

    /// Drawn tiles whose covers are not yet as they should be, once a renderer draws these covers:
    /// the headless path waits for them.
    pub fn waiting(&self) -> usize {
        if self.attached { self.cache.waiting() } else { 0 }
    }

    /// Lets every cover go (the terrain turned off), and the passes' scratch.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.textures.clear();
        self.planned.clear();
        self.scratch = None;
        self.globals.clear();
        self.look = None;
    }

    /// This frame's terrain `tiles` drawn (with their content): a cover for each, within the budget
    /// (the tile set says which of the others it still keeps).
    pub fn prepare(&mut self, cx: &PrepareCx<'_>, tiles: &TileSet<super::mesh::SurfaceMesh, SurfaceGpu>) {
        let zoom = cx.view.view.zoom;
        let wants: Vec<Want> = tiles
            .visible()
            .map(|(tile, gpu)| Want {
                tile,
                size: cover_size(tile.z, cover_zoom(tile.z, MAX_ZOOM, zoom)),
                weight: screen_weight(cx.view, tile, gpu.z_range()),
            })
            .collect();
        let plan = self.cache.plan(&wants, |tile| tiles.state(tile).is_some());
        for tile in plan.free {
            self.textures.remove(&tile);
            self.planned.retain(|&(t, _)| t != tile);
        }
        for (tile, size) in plan.allocate {
            self.planned.retain(|&(t, _)| t != tile);
            self.planned.push((tile, size));
        }
    }

    /// Makes the textures planned.
    fn allocate(&mut self, device: &wgpu::Device) {
        for (tile, size) in std::mem::take(&mut self.planned) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrain cover"),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: MAP_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let group = cover_group(device, &self.layout, &view, &self.sampler);
            self.textures.insert(tile, CoverTexture { texture, view, group });
        }
    }

    /// What the cover layers draw everywhere this frame: a change puts every cover out of date.
    pub fn set_look(&mut self, look: CoverLook) {
        if self.look.as_ref().is_some_and(|l| *l != look) {
            self.cache.invalidate_all();
        }
        self.look = Some(look);
    }

    /// The tiles of cover layers drawn differently this frame: the covers they meet are out of date.
    pub fn invalidate(&mut self, changed: &[TileIndex]) {
        self.cache.invalidate(changed);
    }

    /// What the cover layers draw changed within `areas` (item G3, [`DrawnArea`]): each cover that
    /// shows some of it (its square grown by what the area reaches at its own texel size) is out
    /// of date. Covers kept for tiles not drawn now too, so they are right when drawn again.
    pub fn invalidate_areas(&mut self, areas: &[DrawnArea]) {
        if areas.is_empty() {
            return;
        }
        let probes: Vec<TileIndex> = self
            .cache
            .iter()
            .filter(|(_, cover)| cover.size > 0)
            .filter_map(|(tile, cover)| areas.iter().find(|a| a.meets(tile, cover.size)).map(|a| a.probe(tile)))
            .collect();
        self.cache.invalidate(&probes);
    }

    /// The covers to draw now (at most `max`), with their passes' globals for `view` written; the
    /// textures planned are made first. From the first call on, drawn tiles wait for their covers.
    pub fn jobs(&mut self, gpu: &Gpu, view: &ViewFrame, max: usize) -> Vec<CoverJob> {
        self.attached = true;
        self.allocate(&gpu.device);
        let tiles = self.cache.next(max);
        if tiles.is_empty() {
            return Vec::new();
        }
        let scratch = self.scratch.get_or_insert_with(|| Scratch::new(&gpu.device, self.samples));
        let mut used = 0;
        let mut jobs = Vec::new();
        for tile in tiles {
            let (Some(cover), Some(entry)) = (self.textures.get(&tile), self.cache.get(tile)) else { continue };
            let size = entry.size;
            let n = size.div_ceil(CHUNK).max(1);
            let texels = size / n;
            let mut chunks = Vec::new();
            for j in 0..n {
                for i in 0..n {
                    if used == self.globals.len() {
                        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("terrain cover globals"),
                            size: size_of::<Globals>() as u64,
                            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                        let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("terrain cover globals"),
                            layout: &self.globals_layout,
                            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
                        });
                        self.globals.push((buffer, group));
                    }
                    let (buffer, group) = &self.globals[used];
                    used += 1;
                    gpu.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&cover_globals(view, tile, size, n, i, j)));
                    let into = if n == 1 && texels == CHUNK {
                        Target::Cover(cover.view.clone())
                    } else {
                        Target::Copy { texture: cover.texture.clone(), origin: [i * texels, j * texels] }
                    };
                    chunks.push(Chunk { globals: group.clone(), texels, into });
                }
            }
            self.drawn += 1;
            jobs.push(CoverJob {
                tile,
                chunks,
                colour: scratch.colour.clone(),
                colour_view: scratch.colour_view.clone(),
                depth: scratch.depth.clone(),
                resolve: scratch.resolve.clone(),
            });
        }
        jobs
    }

    /// The bind group of `tile`'s cover, or of no cover.
    pub fn group(&self, tile: TileIndex) -> &wgpu::BindGroup {
        match (self.textures.get(&tile), self.cache.get(tile)) {
            (Some(texture), Some(cover)) if cover.state != CoverState::Fresh => &texture.group,
            _ => &self.none,
        }
    }

    /// `tile`'s cover texture, if it has one (tests).
    pub fn texture(&self, tile: TileIndex) -> Option<&wgpu::Texture> {
        self.textures.get(&tile).map(|t| &t.texture)
    }
}

fn cover_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain cover"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn cover_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain cover"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

/// `shaders/terrain.wgsl`'s two pipelines (`fs`, and `fs_air` for the realistic map) with the
/// light, the rig and the style.
pub struct TerrainPipeline {
    plain: wgpu::RenderPipeline,
    air: wgpu::RenderPipeline,
    rlight: wgpu::Buffer,
    lights: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl TerrainPipeline {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(size as u64),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain light, rig and style"),
            entries: &[
                uniform(0, size_of::<RLight>()),
                uniform(1, size_of::<LightsUniform>()),
                uniform(2, size_of::<crate::pipelines::mesh::StyleUniform>()),
            ],
        });
        let covers = cover_layout(device);
        let module = shaders::module(device, "terrain", SHADER);
        let pipeline = |label, fragment_entry| {
            build(
                init,
                &Desc {
                    label,
                    module: &module,
                    vertex_entry: "vs",
                    fragment_entry,
                    buffers: &[Some(VERTEX_LAYOUT)],
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    state: DrawState::NO_CULL,
                    groups: &[&layout, &covers],
                },
            )
        };
        let buffer = |label, contents: &[u8]| {
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST },
            )
        };
        let rlight = buffer("terrain light", bytemuck::bytes_of(&RLight::zeroed()));
        let lights = buffer("terrain rig", bytemuck::bytes_of(&LightsUniform::zeroed()));
        let style = buffer("terrain style", bytemuck::bytes_of(&style_uniform()));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain light, rig and style"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: rlight.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: style.as_entire_binding() },
            ],
        });
        TerrainPipeline { plain: pipeline("terrain", "fs"), air: pipeline("terrain air", "fs_air"), rlight, lights, group }
    }

    /// This frame's rig (turned for the camera) and light (the realistic map's look for this frame,
    /// `TerrainAirExtension.draw`'s `rlightProps`).
    pub fn prepare(&self, cx: &PrepareCx<'_>) {
        let queue = &cx.gpu.queue;
        queue.write_buffer(&self.lights, 0, bytemuck::bytes_of(&LightsUniform::new(cx.lights(), cx.view)));
        let look = frame_look(&cx.scene.sky, &cx.view.view, cx.view.size, cx.local_now_ms);
        queue.write_buffer(&self.rlight, 0, bytemuck::bytes_of(&RLight::new(&look, cx.view, f64::from(cx.view.globals.viewport[3]))));
    }

    /// Draws each tile in its frame with its cover; with the air over it on the realistic map.
    pub fn draw<'t>(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, covers: &Covers, tiles: impl IntoIterator<Item = (FrameSlot, TileIndex, &'t SurfaceGpu)>) {
        pass.set_pipeline(if cx.scene.basemap == Basemap::Realistic { &self.air } else { &self.plain });
        pass.set_bind_group(STYLE_GROUP, &self.group, &[]);
        for (slot, tile, gpu) in tiles {
            cx.bind_frame(pass, slot);
            pass.set_bind_group(COVER_GROUP, covers.group(tile), &[]);
            gpu.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rustradar_core::geo::Basemap;
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::{ViewState, lng_lat_to_world, viewport_for};

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{self, DirTiles, FnFetcher, TileFetcher};
    use crate::layers::{earth, ground, hillshade, sky, terrain, vector_map};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::pipelines::mesh::{self, Light};
    use crate::renderer::MapRenderer;

    /// `TERRAIN_VIEWS`' Zermatt (X1): looking north down the Mattertal at pitch 60, the target
    /// raised to the village's ground (1608 m, as the terrain's camera aim has it, Z4), on the
    /// 800 × 500 canvas the fixtures were fetched for.
    const ZERMATT: ViewState =
        ViewState { latitude: 46.0207, longitude: 7.7491, zoom: 16.0, pitch: 60.0, bearing: 0.0, position: Some([0.0, 0.0, 1608.0]) };
    const ZERMATT_SIZE: [f64; 2] = [800.0, 500.0];
    /// V5 (plan section 8): Stockholm's centre, pitched, on the shots' 1100 × 720 canvas.
    const V5: ViewState = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
    const V5_SIZE: [f64; 2] = [1100.0, 720.0];
    /// 2026-06-21 10:00 UTC (the reference pictures' summer morning).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    const MIB: usize = 1024 * 1024;

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// A renderer whose tiles come from `tiles`.
    fn renderer_on(gpu: &crate::Gpu, tiles: Arc<dyn TileFetcher>) -> MapRenderer {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(tiles, None);
        renderer
    }

    /// The dark map (its land and vector map) at `view`, the terrain `terrain`.
    fn dark(view: ViewState, size: [f64; 2], terrain: bool) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        scene.local_now_ms = MIDSUMMER;
        scene.feed_now_ms = MIDSUMMER;
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = vector_map::Input::SHOWN;
        scene.terrain = terrain::Input { shown: terrain };
        scene
    }

    /// The realistic map (sky, earth, ground, relief) at `view`, sunny at midsummer, the terrain
    /// `terrain`, as the app's scene builder has it.
    fn realistic(view: ViewState, size: [f64; 2], terrain: bool) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        scene.feed_now_ms = MIDSUMMER;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        scene.hillshade = hillshade::Input { shown: true, terrain };
        scene.terrain = terrain::Input { shown: terrain };
        scene
    }

    /// A rig that leaves a colour as it is (the ambient light alone, `1 / 0.35` of white against
    /// the surface's ambient 0.35): the surface then shows its cover's colours exactly.
    fn neutral() -> mesh::LightRig {
        let off = Light { color: [0.0; 3], intensity: 0.0 };
        mesh::LightRig { ambient: Light { color: [255.0; 3], intensity: 1.0 / 0.35 }, camera: off, key: off, ..mesh::LightRig::new() }
    }

    /// The fixtures, but every Mapterhorn tile a 256 px terrarium picture of `metres` (of a
    /// place's world Mercator `[x, y]`), its pixels where the loader puts its grid's points (the
    /// tile's square grown by a pixel of 512 on every side).
    fn fixtures_on(metres: impl Fn([f64; 2]) -> f64 + Send + Sync + 'static) -> Arc<dyn TileFetcher> {
        let tiles = DirTiles::fixtures();
        Arc::new(FnFetcher(move |source: &str, tile: TileIndex| {
            if source != io::MAPTERHORN {
                return tiles.read(source, tile);
            }
            let extent = tile_extent(tile.z);
            let pad = 1.0 / 512.0;
            let at = |i: u32| -pad + f64::from(i) * (1.0 + 2.0 * pad) / 256.0;
            let image = image::RgbImage::from_fn(256, 256, |i, j| {
                let world = [(f64::from(tile.x) + at(i)) * extent, WORLD_SIZE - (f64::from(tile.y) + at(j)) * extent];
                let v = ((metres(world) + 32768.0) * 256.0).round() as u32;
                image::Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8])
            });
            let mut png = Vec::new();
            image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("a PNG");
            Ok(Some(png))
        }))
    }

    /// The dark map's fills: its water, its land, anything else (roads, borders, edges).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Fill {
        Water,
        Land,
        Other,
    }

    fn fill_of(p: [u8; 4]) -> Fill {
        let near = |c: [u8; 4]| (0..3).all(|i| (i32::from(p[i]) - i32::from(c[i])).abs() <= 3);
        if near(rustradar_scene::tess::vector::WATER) {
            Fill::Water
        } else if near(rustradar_scene::tess::vector::LAND) {
            Fill::Land
        } else {
            Fill::Other
        }
    }

    /// The fill of `image` at `[x, y]` when its 3 × 3 pixels round it all are the same water or
    /// land (`None` near an edge, a road or the picture's border).
    fn plain_fill(image: &Image, [x, y]: [f64; 2]) -> Option<Fill> {
        let (x, y) = (x.round() as i64, y.round() as i64);
        if x < 1 || y < 1 || x + 1 >= i64::from(image.width) || y + 1 >= i64::from(image.height) {
            return None;
        }
        let fill = fill_of(image.pixel(x as u32, y as u32));
        let same = (-1..=1).all(|dy| (-1..=1).all(|dx| fill_of(image.pixel((x + dx) as u32, (y + dy) as u32)) == fill));
        (same && fill != Fill::Other).then_some(fill)
    }

    /// Where the ray of pixel `p` of `view` first meets the ground `metres` (of world Mercator):
    /// `[lon, lat]`. The ray is walked down from the camera in steps of 2 m of height, then the
    /// crossing halved down to a centimetre.
    fn ground_at(view: &ViewState, size: [f64; 2], p: [f64; 2], metres: &dyn Fn([f64; 2]) -> f64) -> Option<[f64; 2]> {
        let viewport = viewport_for(view, size);
        let camera = viewport.unproject_position(viewport.camera_position);
        let above = |h: f64| metres(lng_lat_to_world(viewport.unproject(p, h))) < h;
        let mut high = camera[2] - 1.0;
        let mut low = high - 2.0;
        while above(low) {
            high = low;
            low -= 2.0;
            if low < -100.0 {
                return None;
            }
        }
        while high - low > 0.01 {
            let mid = (high + low) / 2.0;
            if above(mid) { high = mid } else { low = mid }
        }
        Some(viewport.unproject(p, low))
    }

    // ------------------------------------------------------------------ the CPU side

    /// `terrain.wgsl` takes the mesh pipeline's rig and style and `lib/rlight.wgsl`'s light;
    /// what makes a cover pass one (`globals.mercator.y`) is [`COVER_PASS`].
    #[test]
    fn the_shader_takes_the_rig_the_style_and_the_light() {
        for name in ["Lights", "Style"] {
            assert_eq!(wgsl_members(SHADER, name), wgsl_members(mesh::SHADER, name), "{name}");
        }
        assert_eq!(wgsl_members(SHADER, "RLight").0, size_of::<RLight>() as u32);
        assert_eq!(COVER_PASS, 1.0);
        assert!(crate::shaders::LIBRARIES.iter().any(|(p, src)| *p == "lib/rlight.wgsl" && src.contains("globals.mercator.y > 0.5")));
    }

    /// A cover pass's camera looks straight down on its tile's square: the square's north-west
    /// corner (the tile frame's origin) lands on the pass's top left, its south-east corner on its
    /// bottom right, every depth at 0.5; a 2048 cover's four chunks each take a quarter, the
    /// first the north-west; pixels are texels; the flat map's terms are the tile middle's; the
    /// camera the map's; the pass says it is one.
    #[test]
    fn a_cover_pass_sees_its_tile_from_straight_above() {
        let view = ViewFrame::new(&ZERMATT, ZERMATT_SIZE, 1.0, 0.0);
        let tile = TileIndex::new(34178, 23309, 16);
        let frame = view.tile(tile, 0.0);
        let anchor = [f64::from(frame.offset_hi[0]) + f64::from(frame.offset_lo[0]), f64::from(frame.offset_hi[1]) + f64::from(frame.offset_lo[1])];
        let extent = tile_extent(16);
        let clip = |g: &Globals, x: f64, y: f64| {
            let m = glam::Mat4::from_cols_array_2d(&g.centre_cam).as_dmat4();
            m * glam::DVec4::new(x, y, 123.0, 1.0)
        };
        let whole = cover_globals(&view, tile, 1024, 1, 0, 0);
        let nw = clip(&whole, anchor[0], anchor[1]);
        let se = clip(&whole, anchor[0] + extent, anchor[1] - extent);
        for (got, want) in [(nw, [-1.0, 1.0]), (se, [1.0, -1.0])] {
            assert!((got.x - want[0]).abs() < 1e-4 && (got.y - want[1]).abs() < 1e-4 && got.z == 0.5 && got.w == 1.0, "{got:?} against {want:?}");
        }
        assert_eq!(whole.view_proj, whole.centre_cam);
        assert_eq!((whole.mode, whole.mercator[1]), (MODE_FLAT, COVER_PASS));
        assert_eq!(whole.viewport[..3], [1024.0, 1024.0, 1.0]);
        assert!((f64::from(whole.pixels_per_unit) - 1024.0 / extent).abs() / (1024.0 / extent) < 1e-6);
        let (lat, upm) = (46.0207f64, mercator_units_per_meter(46.0207));
        assert!((f64::from(whole.centre[2]) - upm).abs() / upm < 1e-3 && (f64::from(whole.centre[1]) - lat.to_radians().sin()).abs() < 1e-3);
        assert!((f64::from(whole.metres_per_pixel) - extent / 1024.0 / upm).abs() < 1e-3, "{}", whole.metres_per_pixel);
        // the map's camera, as `aimMapEye` has it: south of the target and above it
        let (eye, height) = map_eye(&view);
        assert!(height > 1608.0 && eye[1] < lng_lat_to_world([ZERMATT.longitude, ZERMATT.latitude])[1]);
        // a 2048 cover in four chunks: the south-east one takes the square's south-east quarter
        let chunk = cover_globals(&view, tile, 2048, 2, 1, 1);
        let middle = clip(&chunk, anchor[0] + extent / 2.0, anchor[1] - extent / 2.0);
        let corner = clip(&chunk, anchor[0] + extent, anchor[1] - extent);
        assert!((middle.x + 1.0).abs() < 1e-4 && (middle.y - 1.0).abs() < 1e-4 && (corner.x - 1.0).abs() < 1e-4 && (corner.y + 1.0).abs() < 1e-4);
        assert_eq!(chunk.viewport[..2], [1024.0, 1024.0]);
    }

    /// What every cover holds changes with the basemap, the projection, the weather, the relief's
    /// opacity step and the night's minute; not with the camera moving within them.
    #[test]
    fn the_covers_look_changes_with_what_is_draped_everywhere() {
        let view = |v: &ViewState| ViewFrame::new(v, V5_SIZE, 1.0, 0.0);
        let mut scene = realistic(V5, V5_SIZE, true);
        let look = cover_look(&scene, &view(&V5), false);
        let panned = ViewState { longitude: V5.longitude + 0.01, bearing: 40.0, ..V5 };
        assert_eq!(cover_look(&scene, &view(&panned), false), look);
        assert_ne!(cover_look(&scene, &view(&V5), true), look, "the textures arriving");
        scene.sky.weather = Some(PRESETS.rain);
        assert_ne!(cover_look(&scene, &view(&V5), false), look);
        let globe = ViewState { zoom: 9.0, ..V5 };
        let scene = realistic(V5, V5_SIZE, true);
        assert_ne!(cover_look(&scene, &view(&globe), false), look);
        let mut dark_scene = dark(V5, V5_SIZE, true);
        dark_scene.night = crate::layers::night::Input::SHOWN;
        let at = |ms: f64| {
            let mut s = dark_scene.clone();
            s.feed_now_ms = ms;
            cover_look(&s, &view(&V5), false)
        };
        assert_eq!(at(MIDSUMMER), at(MIDSUMMER + 59_000.0));
        assert_ne!(at(MIDSUMMER), at(MIDSUMMER + 60_000.0), "the night's minute");
    }

    // ------------------------------------------------------------------ on the GPU

    /// The registration: over a surface at sea level the draped map is the flat map where it is
    /// plain water or plain land (the covers' colours shown as they are under a rig that leaves
    /// them so); then the acceptance: over ridges 150 m high (running north and south, 700 m
    /// apart) at V5 the dark map's water and land follow the relief: each pixel shows what the flat
    /// map has where the pixel's ray meets the ridges (cast on the CPU), not what lies under it on
    /// the sea.
    #[test]
    fn the_map_follows_the_relief() {
        let Some(gpu) = gpu() else { return };
        let off = render_with(&gpu, &mut renderer_on(&gpu, Arc::new(DirTiles::fixtures())), &dark(V5, V5_SIZE, false), 1.0).expect("rendered");
        let shoot = |metres: fn([f64; 2]) -> f64| {
            let mut scene = dark(V5, V5_SIZE, true);
            scene.lights = neutral();
            let mut renderer = renderer_on(&gpu, fixtures_on(metres));
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            image
        };
        let sea = shoot(|_| 0.0);
        sea.save_png(&shots_dir().join("z2-v5-dark-sea-level.png")).expect("saved");
        let ridges: fn([f64; 2]) -> f64 = |[x, _]| 75.0 * (1.0 - (2.0 * std::f64::consts::PI * x / (700.0 * mercator_units_per_meter(59.3293))).cos());
        let relief = shoot(ridges);
        relief.save_png(&shots_dir().join("z2-v5-dark-ridges.png")).expect("saved");
        off.save_png(&shots_dir().join("z2-v5-dark-off.png")).expect("saved");

        let viewport = viewport_for(&V5, V5_SIZE);
        let (mut same, mut plain) = (0, 0);
        let (mut followed, mut stayed, mut moved) = (0, 0, 0);
        for y in (8..712).step_by(6) {
            for x in (8..1092).step_by(6) {
                let p = [f64::from(x), f64::from(y)];
                // over the sea-level surface
                if let (Some(want), got) = (plain_fill(&off, p), fill_of(sea.pixel(x, y))) {
                    plain += 1;
                    same += usize::from(got == want);
                }
                // over the ridges: what the flat map has where the ray meets them, against what it
                // has under the pixel
                let Some(ground) = ground_at(&V5, V5_SIZE, p, &ridges) else { continue };
                let [gx, gy, _] = viewport.project([ground[0], ground[1], 0.0]);
                let (Some(want), Some(under)) = (plain_fill(&off, [gx, gy]), plain_fill(&off, p)) else { continue };
                if want == under {
                    continue;
                }
                moved += 1;
                let got = fill_of(relief.pixel(x, y));
                followed += usize::from(got == want);
                stayed += usize::from(got == under);
            }
        }
        eprintln!("sea level: {same} of {plain} plain pixels as the flat map; ridges: of {moved} where the relief moves the fill, {followed} follow it, {stayed} stay");
        assert!(plain > 5000 && same * 1000 >= plain * 995, "sea level: {same} of {plain}");
        assert!(moved > 300, "the ridges move the map's fills: {moved}");
        assert!(followed * 100 >= moved * 95 && stayed * 100 <= moved * 3, "{followed} and {stayed} of {moved}");
    }

    fn differing(a: &Image, b: &Image) -> usize {
        a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).filter(|(a, b)| a != b).count()
    }

    /// Zermatt on the realistic map from the committed fixtures (the acceptance's shot: the
    /// Mattertal's meadows, rock, paths and the Vispa draped over its slopes, the air over them):
    /// with the terrain on, then off (the covers let go, the picture the flat map's as a renderer
    /// that never had the terrain draws it), then on again (the same picture as the first).
    #[test]
    fn turning_the_terrain_off_and_on_gives_the_same_picture() {
        let Some(gpu) = gpu() else { return };
        let fixtures: Arc<dyn TileFetcher> = Arc::new(DirTiles::fixtures());
        let on = realistic(ZERMATT, ZERMATT_SIZE, true);
        // terrain off, the camera's target back on the sea (Z4)
        let off = realistic(ViewState { position: None, ..ZERMATT }, ZERMATT_SIZE, false);
        let mut renderer = renderer_on(&gpu, Arc::clone(&fixtures));
        let first = render_with(&gpu, &mut renderer, &on, 1.0).expect("rendered");
        first.save_png(&shots_dir().join("z2-zermatt-realistic.png")).expect("saved");
        let covers = renderer.terrain_covers().expect("the terrain's covers");
        let (textures, drawn) = (covers.textures(), covers.drawn());
        eprintln!("{textures} covers, {} MB, drawn {drawn} times", covers.texture_bytes() / MIB);
        assert!(textures >= 8 && covers.waiting() == 0 && drawn >= textures);
        let flat = render_with(&gpu, &mut renderer, &off, 1.0).expect("rendered");
        assert!(renderer.terrain_covers().is_none(), "no covers with the terrain off");
        let never = render_with(&gpu, &mut renderer_on(&gpu, Arc::clone(&fixtures)), &off, 1.0).expect("rendered");
        assert_eq!(differing(&flat, &never), 0, "off, the flat map as it always is");
        let again = render_with(&gpu, &mut renderer, &on, 1.0).expect("rendered");
        assert_eq!(differing(&first, &again), 0, "on again, the same picture");
        let changed = differing(&first, &flat);
        assert!(changed * 2 > first.rgba.len() / 4, "the terrain changes the picture: {changed} px");
    }

    /// The covers' memory (the bookkeeping and the textures on the GPU) stays within the budget:
    /// the desktop's 256 MB at the Zermatt skyline (pitch 75: 65 tiles, 260 MB at 1024 texels
    /// each, so the two smallest on the screen are halved), and a budget of 12 MB at Zermatt,
    /// where the covers are cut down to fit and the map is still draped (coarser).
    #[test]
    fn the_covers_stay_within_their_memory_budget() {
        let Some(gpu) = gpu() else { return };
        let fixtures: Arc<dyn TileFetcher> = Arc::new(DirTiles::fixtures());
        let skyline = ViewState { pitch: 75.0, ..ZERMATT };
        let mut renderer = renderer_on(&gpu, Arc::clone(&fixtures));
        render_with(&gpu, &mut renderer, &dark(skyline, ZERMATT_SIZE, true), 1.0).expect("rendered");
        let covers = renderer.terrain_covers().expect("the terrain's covers");
        let sizes = |covers: &Covers| covers.cache().iter().map(|(_, c)| c.size).collect::<Vec<u32>>();
        eprintln!("skyline: {} covers, {} MB of {} MB, sizes {:?}", covers.textures(), covers.texture_bytes() / MIB, covers.cache().budget() / MIB, sizes(covers));
        assert_eq!(covers.cache().budget(), BUDGET);
        assert!(covers.bytes() <= BUDGET && covers.texture_bytes() <= BUDGET);
        assert_eq!(covers.texture_bytes(), covers.bytes(), "every cover planned is made");
        assert!(covers.textures() >= 60 && covers.waiting() == 0, "{} covers", covers.textures());

        let full = render_with(&gpu, &mut renderer_on(&gpu, Arc::clone(&fixtures)), &realistic(ZERMATT, ZERMATT_SIZE, true), 1.0).expect("rendered");
        let budget = 12 * MIB;
        let mut renderer = renderer_on(&gpu, Arc::clone(&fixtures));
        let scene = realistic(ZERMATT, ZERMATT_SIZE, true);
        // the first frame makes the covers; from the next they fit the small budget
        renderer.update(scene.clone());
        renderer.prepare(&gpu, scene.local_now_ms);
        let mut frames = 0;
        while renderer.terrain_covers().is_none() {
            renderer.prepare(&gpu, scene.local_now_ms);
            frames += 1;
            assert!(frames < 1000, "the terrain never shows");
        }
        renderer.terrain_covers().expect("covers").set_budget(budget);
        let small = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        small.save_png(&shots_dir().join("z2-zermatt-12mb.png")).expect("saved");
        let covers = renderer.terrain_covers().expect("the terrain's covers");
        eprintln!("12 MB: {} covers, {} KB, sizes {:?}", covers.textures(), covers.texture_bytes() / 1024, sizes(covers));
        assert!(covers.bytes() <= budget && covers.texture_bytes() <= budget);
        assert!(sizes(covers).iter().any(|&s| s < 1024) && covers.waiting() == 0);
        // coarser, but the same map: most pixels within a few levels of the full covers'
        let close = full.rgba.chunks_exact(4).zip(small.rgba.chunks_exact(4)).filter(|(a, b)| (0..3).all(|c| a[c].abs_diff(b[c]) <= 12)).count();
        eprintln!("{close} of {} px within 12 levels", full.rgba.len() / 4);
        assert!(close * 10 >= full.rgba.len() / 4 * 8, "{close} px");
    }

    /// A layer draped in part ([`crate::layers::Drape::Part`]): over a surface at sea level the
    /// dark map's buildings at V4 (Arlanda, zoom 13.5, pitched 45°) stand as they do on the flat
    /// map and their shadows lie on the ground as there, drawn into the covers once (not again in
    /// the map pass, not left out).
    #[test]
    fn the_buildings_stand_and_their_shadows_are_draped() {
        let Some(gpu) = gpu() else { return };
        let v4 = ViewState { latitude: 59.6519, longitude: 17.9186, zoom: 13.5, pitch: 45.0, bearing: 30.0, position: None };
        // 2026-06-21 05:20 UTC: a low sun over Stockholm, long shadows
        let low_sun = 1_782_019_200_000.0;
        let scene = |terrain: bool| {
            let mut scene = dark(v4, [1440.0, 900.0], terrain);
            scene.local_now_ms = low_sun;
            scene.feed_now_ms = low_sun;
            scene.vector_map = vector_map::Input::default();
            scene.buildings = crate::layers::buildings::Input::SHOWN;
            scene.lights = neutral();
            scene
        };
        let fixtures: Arc<dyn TileFetcher> = Arc::new(DirTiles::fixtures());
        let off = render_with(&gpu, &mut renderer_on(&gpu, Arc::clone(&fixtures)), &scene(false), 1.0).expect("rendered");
        let on = render_with(&gpu, &mut renderer_on(&gpu, fixtures_on(|_| 0.0)), &scene(true), 1.0).expect("rendered");
        off.save_png(&shots_dir().join("z2-v4-buildings-off.png")).expect("saved");
        on.save_png(&shots_dir().join("z2-v4-buildings-on.png")).expect("saved");
        let land = rustradar_scene::tess::vector::LAND;
        let near = |a: [u8; 4], b: [u8; 4], d: u8| (0..3).all(|c| a[c].abs_diff(b[c]) <= d);
        // the shadows on the land: darker than it in every channel
        let shadowed = |p: [u8; 4]| (0..3).all(|c| p[c] + 6 < land[c]);
        let darkest = off.rgba.chunks_exact(4).filter(|p| shadowed([p[0], p[1], p[2], p[3]])).map(|p| p[0]).min().expect("shadows");
        let (mut shadow, mut kept, mut twice, mut lost, mut same) = (0, 0, 0, 0, 0);
        for y in 0..off.height {
            for x in 0..off.width {
                let (a, b) = (off.pixel(x, y), on.pixel(x, y));
                same += usize::from(near(a, b, 3));
                if shadowed(a) {
                    shadow += 1;
                    kept += usize::from(near(a, b, 4));
                    // darker than any single shadow: darkened again in the map pass
                    twice += usize::from(b[0] + 3 < darkest);
                    // back to the land: left out
                    lost += usize::from(!shadowed(b));
                }
            }
        }
        let total = (off.width * off.height) as usize;
        eprintln!("{same} of {total} px alike; of {shadow} shadow px {kept} kept, {twice} darker than one shadow ({darkest}), {lost} lost");
        assert!(shadow > 5000, "shadows on the land: {shadow} px");
        assert!(kept * 100 >= shadow * 95 && twice == 0 && lost * 100 <= shadow, "{kept}, {twice} and {lost} of {shadow}");
        assert!(same * 100 >= total * 97, "{same} of {total}");
    }

    /// On the globe (V3, zoom 9) the covers are drawn on the flat map's terms all the same: over a
    /// surface at sea level the dark map's water and land are where the globe draws them.
    #[test]
    fn on_the_globe_the_draped_map_is_the_globe_s() {
        let Some(gpu) = gpu() else { return };
        let v3 = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };
        let off = render_with(&gpu, &mut renderer_on(&gpu, Arc::new(DirTiles::fixtures())), &dark(v3, V5_SIZE, false), 1.0).expect("rendered");
        let mut scene = dark(v3, V5_SIZE, true);
        scene.lights = neutral();
        let mut renderer = renderer_on(&gpu, fixtures_on(|_| 0.0));
        let on = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        on.save_png(&shots_dir().join("z2-v3-globe-sea-level.png")).expect("saved");
        assert!(renderer.terrain_covers().is_some_and(|c| c.textures() > 0));
        let (mut same, mut plain) = (0, 0);
        for y in (2..718).step_by(3) {
            for x in (2..1098).step_by(3) {
                if let Some(want) = plain_fill(&off, [f64::from(x), f64::from(y)]) {
                    plain += 1;
                    same += usize::from(fill_of(on.pixel(x, y)) == want);
                }
            }
        }
        eprintln!("globe: {same} of {plain} plain pixels as the globe draws them");
        assert!(plain > 20_000 && same * 1000 >= plain * 995, "{same} of {plain}");
    }

    /// The layers draped since item G3: the radar's ground layers, the signal blocks' lines, the
    /// selected train's route, the trail's shadow and the filed plan.
    mod draped {
        use std::sync::Arc;

        use rustradar_core::model::{RadarBounds, RadarVolume, TrainBlock};
        use rustradar_scene::blocks::{BlockBuilder, DrawnTrain, SetRef};
        use rustradar_scene::camera::Viewport;
        use rustradar_scene::radar::BandPolygon;

        use super::*;
        use crate::layers::radar::{measured, tiles};
        use crate::layers::{blocks, radar, route, train_route};

        // -------------------------------------------------------------- the CPU side

        /// A box meets the covers whose squares it lies in, and the neighbours' its strokes reach
        /// into at their own texel size: a coarse cover's texels reach further; either side of
        /// the antimeridian.
        #[test]
        fn a_changed_area_meets_the_covers_that_show_it() {
            let tile = TileIndex::new(9000, 4600, 14);
            let extent = tile_extent(14);
            let east = (f64::from(tile.x) + 1.0) * extent;
            let north = WORLD_SIZE - f64::from(tile.y) * extent;
            // a point inside, and one an eighth of the tile east of its edge
            let inside = DrawnArea::world([east - extent / 2.0, north - extent / 2.0], [east - extent / 2.0, north - extent / 2.0], 5.0);
            assert!(inside.meets(tile, 1024) && inside.meets(tile, 64));
            let beyond = DrawnArea::world([east + extent / 8.0, north - extent / 2.0], [east + extent / 8.0, north - extent / 2.0], 5.0);
            assert!(!beyond.meets(tile, 1024), "5 texels of a 1024 cover do not reach an eighth of it");
            assert!(beyond.meets(tile, 32), "5 texels of a 32 one do (5/32 > 1/8)");
            assert!(beyond.meets(TileIndex::new(4500, 2300, 13), 1024), "the parent holds it");
            // the place a cover is put out of date by: in its square, at a deep zoom
            let probe = inside.probe(tile);
            assert!(tile.contains(probe) && probe.z == PROBE_ZOOM);
            assert!(tile.contains(beyond.probe(tile)), "clamped into the square");
            // across the antimeridian: a stroke at the world's west edge reaches the east edge's tile
            let last = TileIndex::new((1 << 14) - 1, 4600, 14);
            let west_edge = DrawnArea::world([1e-6, north - extent / 2.0], [2e-6, north - extent / 2.0], 2.0);
            assert!(west_edge.meets(last, 1024));
            // the box round points, `|| 0`, and joined
            let round = DrawnArea::round([[18.0, 60.0], [f64::NAN, 59.0]], 3.0).expect("a box");
            assert_eq!(round.min[0], lng_lat_to_world([0.0, 59.0])[0]);
            assert_eq!(round.max, [lng_lat_to_world([18.0, 60.0])[0], lng_lat_to_world([18.0, 60.0])[1]]);
            assert!(DrawnArea::round(std::iter::empty(), 1.0).is_none());
            let joined = inside.join(DrawnArea { reach_px: 9.0, ..beyond });
            assert_eq!((joined.min[0], joined.max[0], joined.reach_px), (inside.min[0], beyond.max[0], 9.0));
            assert_eq!(line_reach(3.0), 7.0);
        }

        /// A path redraws where its points part and meet again when solid, from where they part to
        /// its end when dashed along its whole length; the same path, or one too short to draw on
        /// both sides, nothing; rows of paths compared row by row.
        #[test]
        fn a_path_changes_where_its_points_do() {
            let p = |i: f64| [18.0 + i * 0.001, 60.0];
            let old = [p(0.0), p(1.0), p(2.0), p(3.0), p(4.0)];
            let box_of = |points: &[[f64; 2]], reach: f64| DrawnArea::round(points.iter().copied(), reach).expect("a box");
            assert_eq!(path_changes(&old, &old, false, 7.0), None);
            assert_eq!(path_changes(&[p(0.0)], &[], false, 7.0), None, "one point draws nothing");
            // the train moved on: its first point
            let mut moved = old;
            moved[0] = p(0.5);
            assert_eq!(path_changes(&old, &moved, false, 7.0), Some(box_of(&[p(0.0), p(1.0)], 7.0)), "solid: its first leg");
            assert_eq!(path_changes(&old, &moved, true, 7.0), Some(box_of(&old, 7.0)), "dashed from its start: all of it");
            // a point added at the end (the trail grows)
            let grown = [p(0.0), p(1.0), p(2.0), p(3.0), p(4.0), p(5.0)];
            assert_eq!(path_changes(&old, &grown, false, 4.0), Some(box_of(&[p(4.0), p(5.0)], 4.0)));
            assert_eq!(path_changes(&old, &grown, true, 4.0), Some(box_of(&[p(4.0), p(5.0)], 4.0)), "the dashes before it stay");
            // one in the middle
            let mut bent = old;
            bent[2] = [18.002, 60.001];
            assert_eq!(path_changes(&old, &bent, false, 4.0), Some(box_of(&[p(1.0), p(2.0), p(3.0), bent[2]], 4.0)));
            // drawn, then gone
            assert_eq!(path_changes(&old, &[], false, 4.0), Some(box_of(&old, 4.0)));
            // rows
            let rows = vec![old.to_vec(), vec![p(10.0), p(11.0)]];
            let fewer = vec![old.to_vec()];
            assert_eq!(paths_changes(&rows, &rows, true, 7.0), None);
            assert_eq!(paths_changes(&rows, &fewer, true, 7.0), Some(box_of(&[p(10.0), p(11.0)], 7.0)));
        }

        // -------------------------------------------------------------- on the GPU

        /// G3's place, north-east of Uppsala, where the fixtures' RainViewer frame has echo (its
        /// tile 7/70/37) with land to the west of it.
        const PLACE: [f64; 2] = [18.5, 60.0];
        const SIZE: [f64; 2] = [640.0, 400.0];
        /// The synthetic hillside: this high at the place, rising 0.3 m a metre to the east (17°),
        /// level north to south, down to the sea in the west (where the drawn surface stops, the
        /// mesh's sea-level clamp). Low enough that a line left at sea level would be on the picture.
        const PLACE_M: f64 = 150.0;
        const GRADE: f64 = 0.3;
        const FRAME: &str = "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png";
        const LAND: [u8; 4] = rustradar_scene::tess::vector::LAND;

        /// The hillside's height at world Mercator `[x, y]`, metres.
        fn hillside([x, _]: [f64; 2]) -> f64 {
            let metres_per_unit = 40_075_016.686 * PLACE[1].to_radians().cos() / WORLD_SIZE;
            (PLACE_M + GRADE * (x - lng_lat_to_world(PLACE)[0]) * metres_per_unit).max(0.0)
        }

        fn hillside_at(lon_lat: [f64; 2]) -> f64 {
            hillside(lng_lat_to_world(lon_lat))
        }

        /// The place `east` and `north` metres from [`PLACE`].
        fn spot(east: f64, north: f64) -> [f64; 2] {
            [PLACE[0] + east / (111_320.0 * PLACE[1].to_radians().cos()), PLACE[1] + north / 111_320.0]
        }

        /// A line from `half` metres west of the place's meridian to `half` east of it, `north`
        /// metres north of the place, a point every 20 m.
        fn line(north: f64, half: f64) -> Vec<[f64; 2]> {
            let n = (2.0 * half / 20.0).round() as usize;
            (0..=n).map(|k| spot(-half + 2.0 * half * k as f64 / n as f64, north)).collect()
        }

        /// The view at `zoom` and `pitch` from the south, its target on the hillside with the
        /// terrain on (the camera's ground aim, Z4) and at sea level with it off.
        fn view_of(zoom: f64, pitch: f64, terrain: bool) -> ViewState {
            ViewState { longitude: PLACE[0], latitude: PLACE[1], zoom, pitch, bearing: 0.0, position: terrain.then_some([0.0, 0.0, PLACE_M]) }
        }

        /// The dark map's land alone at `view`, the terrain `terrain`, under a rig that leaves the
        /// covers' colours as they are.
        fn bare(view: ViewState, terrain: bool) -> SceneInput {
            let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Dark);
            scene.local_now_ms = MIDSUMMER;
            scene.feed_now_ms = MIDSUMMER;
            scene.earth = earth::Input::SHOWN;
            scene.terrain = terrain::Input { shown: terrain };
            scene.lights = neutral();
            scene
        }

        /// How far north of the place the lines lie, metres: the block, the route's dashes and its way
        /// to the stop, the trail's shadow, the filed plan.
        const BLOCK_N: f64 = 200.0;
        const DASHES_N: f64 = 110.0;
        const TO_STOP_N: f64 = 20.0;
        const SHADOW_N: f64 = -70.0;
        const FILED_N: f64 = -160.0;

        /// One block from 300 m west to 300 m east, [`BLOCK_N`] north of the place: free, or taken by
        /// a train in its middle.
        fn blocks_scene(view: &ViewState, taken: bool) -> blocks::Input {
            let list = [TrainBlock { id: "b".into(), signal: Some("121".into()), lines: vec![line(BLOCK_N, 300.0)] }];
            let trains = if taken { vec![DrawnTrain { id: "t", name: "SJ 523", at: spot(0.0, BLOCK_N) }] } else { Vec::new() };
            let scene = BlockBuilder::new().build(Some(SetRef { generation: 1, list: &list, structures: &[] }), &trains, view, SIZE, MIDSUMMER);
            blocks::Input { scene }
        }

        /// The selected train's route: its dashes [`DASHES_N`] north of the place, its way to the stop
        /// [`TO_STOP_N`], starting `moved` metres east of 300 m west.
        fn train_route(moved: f64) -> train_route::Input {
            let mut to_stop = line(TO_STOP_N, 300.0);
            to_stop[0] = spot(-300.0 + moved, TO_STOP_N);
            train_route::Input { rest: Arc::new(vec![line(DASHES_N, 300.0)]), head: Vec::new(), to_stop, stations: Vec::new(), ring: None }
        }

        /// The selected flight: its trail 2 km up over [`SHADOW_N`], drawn invisible (only its shadow
        /// shows), with `extra` points on east; the filed plan over [`FILED_N`].
        fn flight(extra: usize) -> route::Input {
            let mut ground = line(SHADOW_N, 300.0);
            ground.extend((1..=extra).map(|k| spot(300.0 + 20.0 * k as f64, SHADOW_N)));
            let path: Vec<[f64; 3]> = ground.iter().map(|&[lon, lat]| [lon, lat, 2000.0]).collect();
            let colors = vec![[0, 0, 0, 0]; path.len()];
            route::Input { trail: Some(route::Trail { path, colors }), filed: line(FILED_N, 300.0), before: Vec::new(), ahead: Vec::new() }
        }

        /// A measured volume's ground: one square of band 9, `size` degrees, its south-west corner
        /// at `[lon, lat]`, scanned at `time`; no heights, so no lifted layers.
        fn volume(time: i64, [lon, lat]: [f64; 2], size: f64) -> Arc<measured::Volume> {
            let bounds = RadarBounds { west: lon, south: lat, east: lon + size, north: lat + size };
            let square = BandPolygon { band: 9, positions: vec![lon, lat, lon + size, lat, lon + size, lat + size, lon, lat + size], hole_indices: Vec::new() };
            Arc::new(measured::Volume {
                frame: RadarVolume { source: "mrms".into(), time, bounds, levels: Vec::new(), coverage_width: 2, coverage_height: 2 },
                ground: vec![square],
                levels: Vec::new(),
                coverage: vec![1; 4],
            })
        }

        /// The scene of the line layers at zoom 15, pitched 50°.
        fn lines_scene(terrain: bool) -> SceneInput {
            let view = view_of(15.0, 50.0, terrain);
            let mut scene = bare(view, terrain);
            scene.blocks = blocks_scene(&view, false);
            scene.trains.route = train_route(0.0);
            scene.route = flight(0);
            scene
        }

        fn hillside_tiles() -> Arc<dyn TileFetcher> {
            fixtures_on(hillside)
        }

        /// Which pixels a layer draws.
        type Class<'a> = &'a dyn Fn([u8; 4]) -> bool;

        /// Whether a pixel of `image` round `[x, y]` (±2 across, ±3 up and down) is `class`.
        fn near(image: &Image, [x, y]: [f64; 2], class: &dyn Fn([u8; 4]) -> bool) -> bool {
            let (x, y) = (x.round() as i64, y.round() as i64);
            (-3..=3).any(|dy| {
                (-2..=2).any(|dx| {
                    let (px, py) = (x + dx, y + dy);
                    px >= 0 && py >= 0 && px < i64::from(image.width) && py < i64::from(image.height) && class(image.pixel(px as u32, py as u32))
                })
            })
        }

        fn on_screen([x, y]: [f64; 2]) -> bool {
            x >= 4.0 && y >= 4.0 && x < SIZE[0] - 4.0 && y < SIZE[1] - 4.0
        }

        /// The line layers on a 17° hillside rising east (the item's acceptance): seen from the
        /// south at pitch 50 from a camera whose target stands on the ground 150 m up, the signal
        /// block's line, the train route's dashes and its way to the stop, the trail's shadow and
        /// the filed plan each lie where the CPU puts their points on the hillside, rising to the
        /// east as the ground does, and nowhere else: not also at sea level (some 100 px lower,
        /// where the route, drawn over everything, would show through the hillside as before).
        #[test]
        fn the_line_layers_follow_the_hillside() {
            let Some(gpu) = gpu() else { return };
            let scene = lines_scene(true);
            let mut renderer = renderer_on(&gpu, hillside_tiles());
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            image.save_png(&shots_dir().join("g3-lines-on-the-hillside.png")).expect("saved");
            assert!(!renderer.ground().is_none(), "the scene stands on the terrain");
            let covers = renderer.terrain_covers().expect("covers");
            eprintln!("{} covers, zooms {:?}", covers.textures(), covers.cache().iter().map(|(t, c)| (t.z, c.size)).collect::<Vec<_>>());
            let viewport = viewport_for(&scene.view, SIZE);
            let i = |p: [u8; 4]| p.map(i32::from);
            let land = i(LAND);
            let lighter = |p: [u8; 4]| (0..3).all(|c| i(p)[c] > land[c] + 15);
            let darker = |p: [u8; 4]| (0..3).all(|c| i(p)[c] + 15 < land[c]);
            let green = |p: [u8; 4]| i(p)[1] > i(p)[0] + 60 && i(p)[1] > i(p)[2] + 40;
            let blue = |p: [u8; 4]| i(p)[2] > i(p)[0] + 80 && i(p)[2] > i(p)[1] + 40;
            let layers: [(&str, f64, Class<'_>, usize); 5] = [
                ("train-blocks", BLOCK_N, &green, 90),
                ("train-route-dashes", DASHES_N, &darker, 70),
                ("train-route-to-stop", TO_STOP_N, &blue, 90),
                ("trail-shadow", SHADOW_N, &lighter, 80),
                ("route-filed", FILED_N, &lighter, 90),
            ];
            for (name, north, class, percent) in layers {
                let (mut hits, mut seen) = (0, 0);
                let mut rise = Vec::new();
                for k in 0..=20 {
                    let at = spot(-250.0 + 25.0 * f64::from(k), north);
                    let [x, y, _] = viewport.project([at[0], at[1], hillside_at(at)]);
                    if !on_screen([x, y]) {
                        continue;
                    }
                    seen += 1;
                    hits += usize::from(near(&image, [x, y], class));
                    rise.push(y);
                }
                let climb = rise.first().zip(rise.last()).map_or(0.0, |(a, b)| a - b);
                eprintln!("{name}: {hits} of {seen} points drawn where they lie on the hillside, rising {climb:.0} px west to east");
                assert!(seen >= 15, "{name}: {seen} points on the picture");
                assert!(hits * 100 >= seen * percent, "{name}: {hits} of {seen}");
                assert!(climb > 40.0, "{name} rises with the ground: {climb:.0} px");
            }
            // each colour only on the lines that draw in it (and the discs on them): nothing of
            // them anywhere else, at sea level in particular (some 100 px lower, on the picture)
            let sunk = viewport.project([PLACE[0], PLACE[1], 0.0]);
            let raised = viewport.project([PLACE[0], PLACE[1], PLACE_M]);
            assert!(sunk[1] - raised[1] > 60.0 && on_screen([sunk[0], sunk[1]]), "sea level is on the picture, lower: {sunk:?} against {raised:?}");
            let draped = |north: f64| -> Vec<[f64; 2]> {
                (0..=120)
                    .map(|k| {
                        let at = spot(-300.0 + 5.0 * f64::from(k), north);
                        let [x, y, _] = viewport.project([at[0], at[1], hillside_at(at)]);
                        [x, y]
                    })
                    .collect()
            };
            let classes: [(&str, Class<'_>, Vec<f64>); 4] = [
                ("green", &green, vec![BLOCK_N]),
                ("blue", &blue, vec![TO_STOP_N]),
                ("dark", &darker, vec![BLOCK_N, DASHES_N]),
                ("light", &lighter, vec![TO_STOP_N, SHADOW_N, FILED_N]),
            ];
            for (name, class, lines) in classes {
                let allowed: Vec<[f64; 2]> = lines.iter().flat_map(|&n| draped(n)).collect();
                let (mut drawn, mut stray) = (0, 0);
                for y in 0..image.height {
                    for x in 0..image.width {
                        if !class(image.pixel(x, y)) {
                            continue;
                        }
                        drawn += 1;
                        let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                        stray += usize::from(!allowed.iter().any(|&[ax, ay]| (ax - px).hypot(ay - py) <= 7.0));
                    }
                }
                eprintln!("{name}: {drawn} px, {stray} away from its lines");
                assert!(drawn > 50 && stray == 0, "{name}: {stray} of {drawn} px away from its lines");
            }
        }

        /// The radar's ground layers on the hillside, looking straight down at zoom 12.5 (no
        /// lifted layers): RainViewer's echo from the fixtures and a measured volume's square
        /// each show where the pixels' rays meet the hillside what the flat map has there (cast
        /// on the CPU), not what it has under the pixel at sea level.
        #[test]
        fn the_radar_s_ground_layers_follow_the_hillside() {
            let Some(gpu) = gpu() else { return };
            let scene = |terrain: bool| {
                let mut scene = bare(view_of(12.5, 0.0, terrain), terrain);
                scene.radar = radar::Input {
                    tiles: tiles::Input { url: Some(FRAME.into()), ..Default::default() },
                    measured: measured::Input { volumes: vec![volume(1, [18.47, 60.002], 0.012)], exaggeration: 1.0 },
                };
                scene
            };
            let off = render_with(&gpu, &mut renderer_on(&gpu, Arc::new(DirTiles::fixtures())), &scene(false), 1.0).expect("rendered");
            let on_scene = scene(true);
            let mut renderer = renderer_on(&gpu, hillside_tiles());
            let on = render_with(&gpu, &mut renderer, &on_scene, 1.0).expect("rendered");
            off.save_png(&shots_dir().join("g3-radar-flat.png")).expect("saved");
            on.save_png(&shots_dir().join("g3-radar-on-the-hillside.png")).expect("saved");
            let radar = |p: [u8; 4]| (0..3).any(|c| p[c].abs_diff(LAND[c]) > 10);
            let plain = |image: &Image, [x, y]: [f64; 2]| -> Option<bool> {
                let (x, y) = (x.round() as i64, y.round() as i64);
                if x < 1 || y < 1 || x + 1 >= i64::from(image.width) || y + 1 >= i64::from(image.height) {
                    return None;
                }
                let class = radar(image.pixel(x as u32, y as u32));
                let same = (-1..=1).all(|dy| (-1..=1).all(|dx| radar(image.pixel((x + dx) as u32, (y + dy) as u32)) == class));
                same.then_some(class)
            };
            let on_view = viewport_for(&on_scene.view, SIZE);
            let off_view = viewport_for(&scene(false).view, SIZE);
            let (mut echo, mut moved, mut followed, mut stayed) = (0, 0, 0, 0);
            for y in (4..396).step_by(4) {
                for x in (4..636).step_by(4) {
                    let p = [f64::from(x), f64::from(y)];
                    echo += usize::from(radar(on.pixel(x, y)));
                    let Some(ground) = hit(&on_view, p, &hillside) else { continue };
                    let [gx, gy, _] = off_view.project([ground[0], ground[1], 0.0]);
                    let (Some(want), Some(under)) = (plain(&off, [gx, gy]), plain(&off, p)) else { continue };
                    if want == under {
                        continue;
                    }
                    moved += 1;
                    let got = radar(on.pixel(x, y));
                    followed += usize::from(got == want);
                    stayed += usize::from(got == under);
                }
            }
            eprintln!("radar: {echo} sampled echo pixels; of {moved} where the hillside moves the echo's edge, {followed} follow it, {stayed} stay");
            assert!(echo > 2000, "the radar is drawn: {echo}");
            assert!(moved > 150, "the hillside moves the echo's edges: {moved}");
            assert!(followed * 100 >= moved * 93 && stayed * 100 <= moved * 7, "{followed} and {stayed} of {moved}");
        }

        /// Where the ray of pixel `p` of `viewport` first meets the ground `metres` (of world
        /// Mercator): `[lon, lat]`; walked down from the camera 20 m at a time, then halved to a
        /// centimetre.
        fn hit(viewport: &Viewport, p: [f64; 2], metres: &dyn Fn([f64; 2]) -> f64) -> Option<[f64; 2]> {
            let camera = viewport.unproject_position(viewport.camera_position);
            let above = |h: f64| metres(lng_lat_to_world(viewport.unproject(p, h))) < h;
            let mut high = camera[2] - 1.0;
            let mut low = high - 20.0;
            while above(low) {
                high = low;
                low -= 20.0;
                if low < -100.0 {
                    return None;
                }
            }
            while high - low > 0.01 {
                let mid = (high + low) / 2.0;
                if above(mid) { high = mid } else { low = mid }
            }
            Some(viewport.unproject(p, low))
        }

        /// Every draped layer at once on the hillside (the lines, RainViewer, a measured volume):
        /// with the terrain on, then off (the picture a renderer that never had the terrain draws),
        /// on again (the first picture) and off again (the second).
        #[test]
        fn turning_the_terrain_off_and_on_gives_the_same_picture_with_every_draped_layer() {
            let Some(gpu) = gpu() else { return };
            let scene = |terrain: bool| {
                let mut scene = lines_scene(terrain);
                scene.radar = radar::Input {
                    tiles: tiles::Input { url: Some(FRAME.into()), ..Default::default() },
                    measured: measured::Input { volumes: vec![volume(1, spot(-150.0, 80.0), 0.002)], exaggeration: 1.0 },
                };
                scene
            };
            let mut renderer = renderer_on(&gpu, hillside_tiles());
            let on = render_with(&gpu, &mut renderer, &scene(true), 1.0).expect("rendered");
            let off = render_with(&gpu, &mut renderer, &scene(false), 1.0).expect("rendered");
            assert!(renderer.terrain_covers().is_none(), "no covers with the terrain off");
            let again = render_with(&gpu, &mut renderer, &scene(true), 1.0).expect("rendered");
            let off_again = render_with(&gpu, &mut renderer, &scene(false), 1.0).expect("rendered");
            let never = render_with(&gpu, &mut renderer_on(&gpu, hillside_tiles()), &scene(false), 1.0).expect("rendered");
            on.save_png(&shots_dir().join("g3-every-draped-layer-on.png")).expect("saved");
            off.save_png(&shots_dir().join("g3-every-draped-layer-off.png")).expect("saved");
            assert_eq!(differing(&off, &never), 0, "off, the flat map as it always is");
            assert_eq!(differing(&on, &again), 0, "on again, the same picture");
            assert_eq!(differing(&off, &off_again), 0, "off again, the same picture");
            // (the land is the same colour on the terrain as on the flat map: the lines, the radar,
            // the discs and the volume moved)
            let changed = differing(&on, &off);
            eprintln!("{changed} px differ between the terrain on and off");
            assert!(changed > 5000, "the terrain changes the picture: {changed} px");
        }

        /// The covers a scene refresh draws again, counted (the per-frame budget): with nothing
        /// changed none; as the train moves on along its way to the stop, the trail grows, the
        /// block is taken and a new scan of the volume comes in, only the covers that show the
        /// change, never more than [`COVERS_PER_FRAME`] a frame, where deck draws every cover the
        /// layer is in again.
        #[test]
        fn the_covers_are_drawn_again_only_where_the_draped_layers_change() {
            let Some(gpu) = gpu() else { return };
            let with_volume = |mut scene: SceneInput, time: i64| {
                scene.radar.measured = measured::Input { volumes: vec![volume(time, spot(-150.0, 80.0), 0.002)], exaggeration: 1.0 };
                scene
            };
            let first = with_volume(lines_scene(true), 1);
            let mut renderer = renderer_on(&gpu, hillside_tiles());
            render_with(&gpu, &mut renderer, &first, 1.0).expect("rendered");
            let total = renderer.terrain_covers().expect("covers").textures();
            // frames of `scene` until its covers are drawn: the covers drawn again, and the most a frame
            let redraw = |renderer: &mut MapRenderer, scene: &SceneInput| -> (usize, usize) {
                renderer.update(scene.clone());
                let before = renderer.terrain_covers().expect("covers").drawn();
                let mut most = 0;
                for _ in 0..400 {
                    let was = renderer.terrain_covers().expect("covers").drawn();
                    renderer.prepare(&gpu, scene.local_now_ms);
                    gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("polled");
                    let covers = renderer.terrain_covers().expect("covers");
                    most = most.max(covers.drawn() - was);
                    // (a new scan is meshed on the worker pool first)
                    if covers.waiting() == 0 && covers.drawn() == was && !renderer.pending() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("polled");
                (renderer.terrain_covers().expect("covers").drawn() - before, most)
            };
            // the covers whose squares the change shows in (`DrawnArea::meets`)
            let meeting = |renderer: &mut MapRenderer, area: Option<DrawnArea>| -> usize {
                let area = area.expect("a change");
                renderer.terrain_covers().expect("covers").cache().iter().filter(|(t, c)| c.visible() && area.meets(*t, c.size)).count()
            };

            let (same, _) = redraw(&mut renderer, &first);
            let mut moved = first.clone();
            moved.trains.route = train_route(4.0);
            let train_area = train_route::route_changes(&(Arc::clone(&first.trains.route.rest), Vec::new(), first.trains.route.to_stop.clone()), &moved.trains.route);
            let train_meets = meeting(&mut renderer, train_area);
            let (train, train_most) = redraw(&mut renderer, &moved);
            let mut grown = moved.clone();
            grown.route = flight(1);
            let trail_area = path_changes(&route::draped_paths(&moved.route)[0], &route::draped_paths(&grown.route)[0], false, line_reach(route::TRAIL_SHADOW_WIDTH));
            let trail_meets = meeting(&mut renderer, trail_area);
            let (trail, trail_most) = redraw(&mut renderer, &grown);
            let mut taken = grown.clone();
            taken.blocks = blocks_scene(&taken.view, true);
            let block_meets = meeting(&mut renderer, DrawnArea::round(line(BLOCK_N, 300.0), blocks::lines_reach()));
            let (block, block_most) = redraw(&mut renderer, &taken);
            let rescanned = with_volume(taken.clone(), 2);
            let (scan, scan_most) = redraw(&mut renderer, &rescanned);
            eprintln!(
                "{total} covers (deck draws them all again for each change); drawn again: nothing changed {same}, the train moved {train} (meeting {train_meets}), \
                 the trail grew {trail} (meeting {trail_meets}), the block taken {block} (meeting {block_meets}), a new scan {scan}; at most {} a frame",
                [train_most, trail_most, block_most, scan_most].iter().max().unwrap_or(&0)
            );
            assert!(total >= 6, "{total} covers");
            assert_eq!(same, 0, "nothing changed, nothing drawn again");
            for (what, n, meets) in [("the train", train, train_meets), ("the trail", trail, trail_meets), ("the block", block, block_meets)] {
                assert!(n >= 1 && n <= meets && meets < total, "{what}: {n} drawn again, {meets} meet the change, of {total}");
            }
            assert!(train <= 2 && trail <= 2, "a train's or a trail's step costs a cover or two: {train}, {trail}");
            assert!((1..total).contains(&scan), "a new scan: {scan} of {total}");
            for most in [train_most, trail_most, block_most, scan_most] {
                assert!(most <= COVERS_PER_FRAME, "{most} a frame");
            }
        }
    }
}
