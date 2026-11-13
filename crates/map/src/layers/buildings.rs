//! Plain buildings on the dark and satellite maps (item B7; doc 01 §9.3; doc 04 §7.1, §7.6, §13;
//! row 7 of doc 03 §3): OpenFreeMap's buildings as today's app draws them through `BuildingLayer`
//! (`building-layer.ts`, `cityBuildingLayer`, G:1039-1092), extruded and lit close in, flat far off,
//! with their shadows on the ground. The tessellation is `rustradar_scene::tess::buildings`; the
//! realistic map's buildings (façades, roofs, foot shade) are items R4–R6.
//!
//! What today's app does, and so this layer:
//! - **When.** With the *buildings* setting on, on the flat map (zoom > 12), once the TileJSON has
//!   given the tile URL (G:2694: `buildings && flat && tileUrl`), on the dark and satellite maps.
//!   The scene builder sets [`Input::shown`] from the setting (`app::scene_builder::base`); this
//!   layer works out the projection and the basemap. When it stops drawing it drops its tiles and
//!   their shadows, as deck finalizes a layer that leaves the list.
//! - **Tiles** by their distance from the camera (`LodMVTLayer`): [`LodSelect`] over
//!   `tess::city_lod()` (levels 13, 14, 16, bias 0.5, 25 km) with the look-ahead while zooming,
//!   OpenFreeMap at zooms 13 and 14 ([`tess::SOURCE_ZOOMS`]; a tile of 16 has no source on these
//!   maps and settles empty at once), 8 requests at once, refined by `keepCoarserUntilIn`
//!   ([`Refinement::KeepCoarserUntilIn`]: a coarser tile is drawn in place of finer ones until they
//!   are all in, never with them, and stays for a finer one that came back empty). Tiles are
//!   decoded on the worker pool and drawn in deck's order, coarse to fine ([`TileSet::visible`]).
//!   So close in, where the selection wants zoom-16 tiles, buildings show only where a zoom-14 or
//!   zoom-13 tile over them is cached: arriving there by zooming in (as one does), it is; in a view
//!   opened straight at zoom 15.5 the near ground has none, as today.
//! - **Each tile**, as `BuildingLayer.renderSubLayers` makes it: a far tile (zoom 13, under
//!   `CITY_CLOSE_ZOOM`) is its footprints flat on the ground in `BUILDING_COLOR` `[58,68,84]`,
//!   unlit (`extruded: false`); a near tile (zoom 14) first its shadows on the ground (`-shadow`, a
//!   SolidPolygonLayer in `SHADOW_COLOR` `[4,8,16]` at the sun's alpha, with the `SHADOW` draw state
//!   of the stencil rule: a pixel is darkened once per frame, whichever shadow comes first), then
//!   its buildings extruded and lit in `BUILDING_COLOR` with `BUILDING_MATERIAL` ([`MATERIAL`]:
//!   ambient 0.42, diffuse 0.6, shininess 16, specular `[60,66,80]`) by the scene's lights
//!   ([`PrepareCx::set_lights`]; the scene builder aims the key light at the sun over buildings,
//!   G:2723), per vertex (SolidPolygonLayer's Gouraud shading). Buildings are drawn with `NO_CULL`
//!   (depth tested and written), shadows depth tested but not written, so buildings drawn before
//!   hide later tiles' shadows behind them, and a tile's own shadows are under its buildings.
//! - **Clipped to the tile's square** (MVTLayer's `ClipExtension` on the flat map, by fragment:
//!   SolidPolygonLayer has no `instancePositions`): a fragment of a building whose position on the
//!   map, straight down, lies outside its tile's square is discarded, so a building on a tile's edge
//!   is cut by an upright plane there and each tile draws its own part. The shadows are not clipped
//!   (deck adds the extension to the buildings' sublayer only): a shadow reaches into the next tile.
//!   Here the clip is a test in the buildings' fragment shader ([`SHADER`]: `mesh.wgsl`'s lighting
//!   with an entry of this layer's own), as deck's, on the fragment's tile units.
//! - **Shadows** (doc 04 §7.6): cast per tile with the sun held until it has moved a quarter of a
//!   degree (`tess::ShadowSun`, at the feed clock: `shadowSun(motion.now())`), the sun taken at the
//!   tile's middle. A new tile gets its shadows at once; when the sun has moved, tiles on screen are
//!   recast within [`tess::RECAST_BUDGET_MS`] of main-thread time a frame (at least one a frame),
//!   the rest keeping their old shadows meanwhile (`renderLayers`' recasting budget). The layer is
//!   [`Layer::pending`] while any is left to recast, so a shot waits for them.
//!
//! How it is checked: V4 against today's picture (97.7 % of today's building pixels are buildings
//! here and 98.9 % the other way, 0.7 levels apart on average); V5 for the lit faces and the
//! shadows (each darkens the land or the water exactly once); the far tiles at zoom 12.1; the cut
//! at a tile's square; the recast as the sun moves.
//!
//! Deviation, invisible: the walls are vertices of their own, not deck's instanced quads (the
//! same triangles, lit at the same corners). The clip is per pixel, as deck's discard is (aliased,
//! where the vector map's stencil clip is antialiased).

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use bytemuck::Zeroable;
use rustradar_core::geo::Basemap;
use rustradar_scene::lod::TILE_SIZE;
use rustradar_scene::tess::buildings::{self as tess, BuildingTile, BuildingVertex, Footprints, ShadowSun, TileShadows};

use super::{ByteSize, DrawCx, Init, Layer, LodSelect, Pass, PrepareCx, Refinement, Source, TileIndex, TileSet, TileSetOptions};
use crate::frame::{FrameSlot, ViewFrame};
use crate::io;
use crate::layers::terrain::ground::{StandBudget, Standing};
use crate::jobs::Cancel;
use crate::pipelines::fill::{self, Fill};
use crate::pipelines::mesh::{self, LightRig, LightsUniform, Material, StyleUniform};
use crate::pipelines::{Desc, build, shadow};
use crate::shaders;
use crate::state::DrawState;

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// The *buildings* setting (G:2694). Off in a scene made with `SceneInput::new`; the layer also
    /// draws only on the flat map and not on the realistic map (items R4–R6).
    pub shown: bool,
}

impl Input {
    /// Buildings drawn.
    pub const SHOWN: Input = Input { shown: true };
}

/// `BUILDING_MATERIAL` (G:972-977).
pub const MATERIAL: Material = Material {
    ambient: tess::BUILDING_AMBIENT,
    diffuse: tess::BUILDING_DIFFUSE,
    shininess: tess::BUILDING_SHININESS,
    specular_color: tess::BUILDING_SPECULAR,
    unlit: false,
};

/// The buildings' colour as deck's `getFillColor`, opaque.
pub const COLOR: [u8; 4] = [tess::BUILDING_COLOR[0], tess::BUILDING_COLOR[1], tess::BUILDING_COLOR[2], 255];

/// The buildings' shader: `mesh.wgsl` (its lights, style and `shade`) with this layer's entry, a
/// vertex of position and normal lit per vertex as `vs_local` does, and the tile's square clip.
pub const SHADER: &str = concat!(
    include_str!("../shaders/mesh.wgsl"),
    r#"
// ---- the plain buildings (layers/buildings.rs) ----

// `tess::buildings::BuildingVertex`: (u, v) tile units (v south) and metres up; the normal east,
// north, up; and from a buffer of its own the 3D terrain's height under it, metres (Z3).
struct BuildingVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) ground: f32,
}

struct BuildingVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    // the tile units, for the clip
    @location(1) tile: vec2<f32>,
}

// deck's SolidPolygonLayer in the style's colour, lit per vertex as `vs_local` (an unlit style
// gives the colour as it is: the far tiles' flat footprints).
@vertex
fn vs_building(v: BuildingVertex) -> BuildingVaryings {
    // standing on the 3D terrain (Z3: deck's offset mode, each vertex by the ground under it)
    let p = project_local(v.position + vec3<f32>(0.0, 0.0, v.ground));
    let normal = unit(local_basis(v.position.xy) * v.normal);
    var out: BuildingVaryings;
    out.clip = p.clip;
    out.color = vec4<f32>(shade(style.color.rgb, globals.camera_render, p.rel, normal, lights.key_render), style.color.a * style.opacity);
    out.tile = v.position.xy;
    return out;
}

// deck's ClipExtension by fragment (`clip_isInBounds`, bounds [0, 0, 1, 1] in tile units): the
// map's x within [west, east), y within [south, north), so u in [0, 1) and v (south) in (0, 1].
@fragment
fn fs_building(in: BuildingVaryings) -> @location(0) vec4<f32> {
    if (in.tile.x < 0.0 || in.tile.x >= 1.0 || in.tile.y <= 0.0 || in.tile.y > 1.0) {
        discard;
    }
    return in.color;
}
"#
);

/// The vertex buffer layout of [`BuildingVertex`]: position and normal at locations 0 and 1.
pub const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<BuildingVertex>() as u64,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
};

/// Vertex buffer 1: the 3D terrain's height under each vertex ([`Standing`], item Z3), location 2.
pub const GROUND_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<f32>() as u64,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![2 => Float32],
};

impl ByteSize for BuildingTile {
    fn byte_size(&self) -> usize {
        BuildingTile::byte_size(self)
    }
}

/// A tile on the GPU: its buildings, and its footprints (for its shadows, recast on the CPU as the
/// sun moves).
pub struct TileMeshes {
    /// Standing and lit (zoom ≥ 14), or flat and unlit.
    pub extruded: bool,
    pub mesh: fill::Mesh<BuildingVertex>,
    pub footprints: Footprints,
    /// The 3D terrain's height under each vertex (item Z3; a far tile's stays at the sea: it is
    /// draped).
    pub standing: Standing,
    /// Which upload made it (a tile loaded again is a new one: its shadows are cast anew).
    serial: u64,
}

/// The upload step, on the main thread.
pub fn upload(device: &wgpu::Device, decoded: BuildingTile, serial: u64) -> TileMeshes {
    TileMeshes {
        extruded: decoded.extruded,
        mesh: fill::Mesh::new(device, "buildings", &decoded.vertices, &decoded.indices),
        standing: Standing::new(device, if decoded.extruded { &decoded.vertices } else { &[] }, |v| v.position),
        footprints: decoded.footprints,
        serial,
    }
}

/// The decode step, on the worker pool (`None` for a tile with no buildings).
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<BuildingTile>, String> {
    match tess::tessellate(tile, bytes, tess::CITY_CLOSE_ZOOM, &|| cancel.is_cancelled()) {
        Ok(decoded) if decoded.is_empty() => Ok(None),
        Ok(decoded) => Ok(Some(decoded)),
        Err(tess::Cancelled) => Err("cancelled".into()),
    }
}

/// A tile's shadows as the fill pipeline draws them: `positions` on the ground in tile units, each
/// vertex in [`tess::shadow_color`].
pub fn shadow_vertices(shadows: &TileShadows) -> Vec<fill::Vertex> {
    let color = tess::shadow_color(shadows.alpha, tess::SHADOW_STRENGTH);
    shadows.positions.chunks_exact(2).map(|p| fill::Vertex { position: [p[0], p[1], 0.0], color }).collect()
}

/// A tile's shadows under one sun.
struct Cast {
    /// The upload whose footprints cast them.
    serial: u64,
    sun: [f64; 3],
    /// `None` when they cast none (the sun too low, nothing tall).
    mesh: Option<fill::Mesh<fill::Vertex>>,
}

/// The buildings' pipeline: [`SHADER`]'s entry with a draw state, and its lights buffer (group 2:
/// the lights and a [`StyleUniform`], as the mesh pipeline's).
pub struct BuildingPipeline {
    pipeline: wgpu::RenderPipeline,
    group_layout: wgpu::BindGroupLayout,
    lights: wgpu::Buffer,
}

/// One draw style of a [`BuildingPipeline`].
pub struct Style {
    group: wgpu::BindGroup,
}

impl BuildingPipeline {
    pub fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        let device = &init.gpu.device;
        let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(size as u64),
            },
            count: None,
        };
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("building lights and style"),
            entries: &[uniform(0, size_of::<LightsUniform>()), uniform(1, size_of::<StyleUniform>())],
        });
        let lights = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("building lights"),
                contents: bytemuck::bytes_of(&LightsUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let module = shaders::module(device, label, SHADER);
        let pipeline = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry: "vs_building",
                fragment_entry: "fs_building",
                buffers: &[Some(VERTEX_LAYOUT), Some(GROUND_LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state,
                groups: &[&group_layout],
            },
        );
        BuildingPipeline { pipeline, group_layout, lights }
    }

    /// This frame's lights (every frame: the render frame turns with the camera).
    pub fn set_lights(&self, queue: &wgpu::Queue, rig: &LightRig, view: &ViewFrame) {
        queue.write_buffer(&self.lights, 0, bytemuck::bytes_of(&LightsUniform::new(rig, view)));
    }

    /// A style: `material` with the fill colour `color`.
    pub fn style(&self, device: &wgpu::Device, material: &Material, color: [u8; 4]) -> Style {
        let buffer = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("building style"),
                contents: bytemuck::bytes_of(&StyleUniform::new(material, 1.0, color)),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("building lights and style"),
            layout: &self.group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: buffer.as_entire_binding() },
            ],
        });
        Style { group }
    }

    /// Draws `mesh` in the tile frame `frame` with `style`, standing on the ground `standing`
    /// says is under its vertices.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, frame: FrameSlot, style: &Style, mesh: &fill::Mesh<BuildingVertex>, standing: &Standing) {
        if mesh.index_count() == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(mesh::STYLE_GROUP, &style.group, &[]);
        cx.bind_frame(pass, frame);
        standing.bind(pass, 1);
        mesh.draw(pass);
    }
}

/// Row 7: `buildings-city` (dark and satellite).
pub struct Buildings {
    tiles: TileSet<BuildingTile, TileMeshes>,
    pipeline: BuildingPipeline,
    /// Near tiles: lit in `BUILDING_COLOR` with [`MATERIAL`].
    lit: Style,
    /// Far tiles: `BUILDING_COLOR` unlit.
    flat: Style,
    shadows: Fill,
    shadow_style: fill::Style,
    /// The sun the shadows are cast with.
    sun: ShadowSun,
    /// Each tile's shadows, by tile (deck's `shadowCache`, by content).
    casts: HashMap<TileIndex, Cast>,
    /// The last upload's number.
    serial: u64,
    /// Whether this frame draws.
    shown: bool,
    /// Tiles on screen left with shadows of an older sun this frame.
    stale: usize,
    /// Tiles on screen left standing on an older ground this frame (Z3).
    unstood: usize,
    /// This frame's tiles in draw order, each with its frame.
    drawn: Vec<(FrameSlot, TileIndex)>,
}

impl Buildings {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let tiles = init.tile_set(
            vec![Source::at(io::OPENFREEMAP, tess::SOURCE_ZOOMS.to_vec())],
            LodSelect::new(tess::city_lod(), TILE_SIZE),
            TileSetOptions { max_requests: tess::MAX_REQUESTS, refinement: Refinement::KeepCoarserUntilIn, ..Default::default() },
            |tile, bytes, cancel| decode(tile, &bytes, cancel),
        );
        let pipeline = BuildingPipeline::new(init, "buildings", DrawState::NO_CULL);
        let lit = pipeline.style(device, &MATERIAL, COLOR);
        let flat = pipeline.style(device, &Material::UNLIT, COLOR);
        let shadows = shadow::fill(init, "building shadows");
        let shadow_style = shadows.style(device, fill::OPAQUE);
        Buildings {
            tiles,
            pipeline,
            lit,
            flat,
            shadows,
            shadow_style,
            sun: ShadowSun::default(),
            casts: HashMap::new(),
            serial: 0,
            shown: false,
            stale: 0,
            unstood: 0,
            drawn: Vec::new(),
        }
    }

    /// The tile set (for tests).
    pub fn tiles(&self) -> &TileSet<BuildingTile, TileMeshes> {
        &self.tiles
    }

    /// Casts the shadows of the tiles on screen under `sun` (`renderLayers` and `shadowsOf`): a
    /// tile without shadows of its own gets them at once; one with an older sun's is recast while
    /// the frame's budget lasts (at least one a frame); the others wait for the next frame.
    fn cast(&mut self, device: &wgpu::Device, sun: [f64; 3]) {
        let until = Instant::now() + Duration::from_secs_f64(tess::RECAST_BUDGET_MS / 1000.0);
        let mut recast_any = false;
        self.stale = 0;
        for (tile, meshes) in self.tiles.visible() {
            if !meshes.extruded {
                continue;
            }
            let cached = self.casts.get(&tile).filter(|c| c.serial == meshes.serial);
            if cached.is_some_and(|c| c.sun == sun) {
                continue;
            }
            if cached.is_some() && recast_any && Instant::now() > until {
                self.stale += 1;
                continue;
            }
            recast_any = true;
            let mesh = tess::tile_shadows(tile, &meshes.footprints, sun)
                .map(|s| fill::Mesh::new(device, "building shadows", &shadow_vertices(&s), &s.indices));
            self.casts.insert(tile, Cast { serial: meshes.serial, sun, mesh });
            // (draped over the 3D terrain, stage 9: the covers under it are drawn again)
            crate::layers::note_drawn_changes([tile]);
        }
        // (deck's are keyed by content: gone with it)
        let tiles = &self.tiles;
        self.casts.retain(|&tile, cast| tiles.get(tile).is_some_and(|m| m.serial == cast.serial));
    }
}

impl Layer for Buildings {
    fn name(&self) -> &'static str {
        "buildings"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.drawn.clear();
        self.shown = cx.scene.buildings.shown && !cx.view.is_globe() && cx.scene.basemap != Basemap::Realistic;
        if !self.shown {
            // deck drops a layer that leaves the list, its tiles and their shadows with it
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            self.casts.clear();
            self.stale = 0;
            return;
        }
        let serial = &mut self.serial;
        cx.update_tiles(&mut self.tiles, |gpu, _tile, decoded| {
            *serial += 1;
            upload(&gpu.device, decoded, *serial)
        });
        let sun = self.sun.at(cx.scene.feed_now_ms);
        self.cast(&cx.gpu.device, sun);
        // standing on the 3D terrain where it is drawn (Z3)
        let mut budget = StandBudget::default();
        let visible: Vec<TileIndex> = self.tiles.visible().map(|(t, _)| t).collect();
        for tile in visible {
            if let Some(m) = self.tiles.get_mut(tile)
                && m.extruded
                && m.standing.stale(tile, &cx.scene.stands_on)
                && budget.take()
            {
                m.standing.stand(&cx.gpu.queue, tile, &cx.scene.stands_on);
            }
        }
        self.unstood = budget.left;
        self.pipeline.set_lights(&cx.gpu.queue, cx.lights(), cx.view);
        for (tile, _) in self.tiles.visible() {
            self.drawn.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !cx.pass.draws(self.drape()) {
            return;
        }
        // stage 9: the shadows draped over the 3D terrain, the buildings standing
        let (draped, standing) = (cx.draws_draped(), cx.pass == Pass::Map);
        for &(slot, tile) in &self.drawn {
            let Some(meshes) = self.tiles.get(tile) else { continue };
            if !meshes.extruded {
                // far off, flat: only the roofs show from there (draped over the 3D terrain, Z3:
                // a SolidPolygonLayer that is not extruded has no anchors to stand on)
                if draped {
                    self.pipeline.draw(pass, cx, slot, &self.flat, &meshes.mesh, &meshes.standing);
                }
                continue;
            }
            // under the tile's buildings, on the ground; not clipped to the tile
            if draped && let Some(shadows) = self.casts.get(&tile).filter(|c| c.serial == meshes.serial).and_then(|c| c.mesh.as_ref()) {
                self.shadows.draw(pass, cx, slot, &self.shadow_style, shadows);
            }
            if standing {
                self.pipeline.draw(pass, cx, slot, &self.lit, &meshes.mesh, &meshes.standing);
            }
        }
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Part
    }

    fn pending(&self) -> bool {
        self.shown && (self.tiles.loading() || self.stale > 0 || self.unstood > 0)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path as FilePath, PathBuf};
    use std::sync::Arc;

    use std::sync::Mutex;
    use std::sync::mpsc::Receiver;

    use rustradar_scene::camera::{ViewState, viewport_for};
    use rustradar_scene::lod::distance_tile_indices;

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchRequest, FetchResult, TileFetcher};
    use crate::layers::{earth, vector_map};
    use crate::renderer::{MapRenderer, SceneInput};
    use crate::shaders;

    /// The reference pictures' clock: 2026-06-21 10:00 UTC, about noon in Stockholm.
    const NOW: f64 = 1_782_036_000_000.0;

    fn view(latitude: f64, longitude: f64, zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { latitude, longitude, zoom, pitch, bearing, position: None }
    }

    fn v4() -> ViewState {
        view(59.6519, 17.9186, 13.5, 45.0, 30.0)
    }

    fn v5() -> ViewState {
        view(59.3293, 18.0686, 15.5, 60.0, 20.0)
    }

    /// The dark map with its buildings at `view` on a `size` canvas, at the reference clock, the key
    /// light aimed as the scene builder aims it.
    fn dark(view: ViewState, size: [f64; 2]) -> SceneInput {
        dark_at(view, size, NOW)
    }

    /// [`dark`] at the clock `now`.
    fn dark_at(view: ViewState, size: [f64; 2], now: f64) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        scene.local_now_ms = now;
        scene.feed_now_ms = now;
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = vector_map::Input::SHOWN;
        scene.buildings = Input::SHOWN;
        scene.aim_lights(true);
        scene
    }

    fn shots_dir() -> PathBuf {
        let dir = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    fn reference(name: &str) -> Image {
        let path = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        let rgba = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).into_rgba8();
        Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
    }

    /// Renders `scenes` one after another with one renderer on the fixtures (as the camera moves
    /// from one to the next), saving the last as `name`.
    fn shots(gpu: &crate::gpu::Gpu, scenes: &[SceneInput], name: &str) -> (Image, Arc<DirTiles>) {
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let mut image = None;
        for scene in scenes {
            image = Some(render_with(gpu, &mut renderer, scene, 1.0).expect("rendered"));
            assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        }
        let image = image.expect("a scene");
        image.save_png(&shots_dir().join(name)).expect("the shot written");
        (image, fixtures)
    }

    /// Whether a pixel is a building's: bluish, as the building colour lit any way is, and no
    /// ground of the dark map nor a shadow on it is.
    fn is_building(p: [u8; 4]) -> bool {
        i32::from(p[2]) - i32::from(p[0]) >= 9
    }

    #[test]
    fn the_shader_validates_and_its_vertex_matches_the_scene_s() {
        shaders::tests::validate("buildings", &shaders::expand(SHADER).expect("libraries known"));
        assert_eq!(VERTEX_LAYOUT.array_stride, 24);
        assert_eq!(std::mem::offset_of!(BuildingVertex, normal), 12);
    }

    #[test]
    fn a_fixture_tile_decodes_to_buildings() {
        let tile = TileIndex::new(9014, 4818, 14);
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, tile).expect("read").expect("a V5 tile");
        let decoded = decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("buildings");
        assert!(decoded.extruded && !decoded.footprints.is_empty());
        assert_eq!(ByteSize::byte_size(&decoded), decoded.byte_size());
        assert_eq!(decode(tile, &[], &Cancel::new()), Ok(None));
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(decode(tile, &bytes, &cancel).is_err());
    }

    /// The map in today's pictures at 1440 × 900: right of the sidebar, left of the map controls,
    /// under the mode bar and the dev readout.
    const MAP: [u32; 4] = [360, 80, 1370, 880];

    /// The share of `today`'s pixels inside [`MAP`] that `class` takes which this shot's pixel
    /// also falls in `class` for, at the best shift within 6 px (today's pictures stand about
    /// 4.5 px higher than deck's own viewport puts the map; T1), with that shift.
    fn agreement(ours: &Image, today: &Image, class: impl Fn([u8; 4]) -> bool) -> (f64, [i32; 2], usize) {
        let [x0, y0, x1, y1] = MAP;
        let theirs: Vec<(u32, u32)> = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| class(today.pixel(x, y))).collect();
        let mut best = (0.0, [0, 0]);
        for dy in -6..=6 {
            for dx in -6..=6 {
                let kept = theirs.iter().filter(|&&(x, y)| class(ours.pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32))).count();
                let share = kept as f64 / theirs.len().max(1) as f64;
                if share > best.0 {
                    best = (share, [dx, dy]);
                }
            }
        }
        (best.0, best.1, theirs.len())
    }

    /// The fixture tiles, with every request noted.
    struct Noted {
        tiles: DirTiles,
        asked: Mutex<Vec<(String, TileIndex)>>,
    }

    impl TileFetcher for Noted {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).push((request.source.to_string(), request.tile));
            self.tiles.fetch(request)
        }
    }

    /// The tiles of `scene` (whose only tile layer is the buildings) the layer asks for.
    fn asked(gpu: &crate::gpu::Gpu, scene: &SceneInput) -> Vec<(String, TileIndex)> {
        let noted = Arc::new(Noted { tiles: DirTiles::fixtures(), asked: Mutex::new(Vec::new()) });
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(noted.clone(), None);
        render_with(gpu, &mut renderer, scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        noted.asked.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The buildings draw on the dark and satellite maps, on the flat map only, with the setting
    /// on (G:2694); the realistic map's are items R4–R6. Their tiles are OpenFreeMap's at zooms 13
    /// and 14, picked by distance: at V4 both (far flat roofs, near standing buildings).
    #[test]
    fn they_load_on_the_flat_dark_and_satellite_maps_with_the_setting_on() {
        let Some(gpu) = gpu() else { return };
        let size = [1440.0, 900.0];
        let only = |view: ViewState, basemap: Basemap, shown: bool| {
            let mut scene = SceneInput::new(view, size, 1.0, basemap);
            scene.feed_now_ms = NOW;
            scene.buildings = Input { shown };
            scene
        };
        let viewport = viewport_for(&v4(), size);
        let wanted: Vec<TileIndex> = distance_tile_indices(&viewport, &tess::city_lod(), None, 0.0);
        assert!(wanted.iter().any(|t| t.z == 13) && wanted.iter().any(|t| t.z == 14) && wanted.iter().all(|t| t.z != 16), "{wanted:?}");
        let mut on_dark = Vec::new();
        for basemap in [Basemap::Dark, Basemap::Satellite] {
            // (the satellite map's imagery asks for Esri's tiles: only OpenFreeMap's are the buildings')
            let asked: Vec<_> = asked(&gpu, &only(v4(), basemap, true)).into_iter().filter(|(source, _)| source == io::OPENFREEMAP).collect();
            if basemap == Basemap::Dark {
                on_dark = asked.iter().map(|&(_, t)| t).collect();
            }
            assert!(asked.iter().all(|(_, t)| tess::SOURCE_ZOOMS.contains(&t.z)), "{asked:?}");
            for tile in &wanted {
                assert!(asked.iter().any(|(_, t)| t == tile), "{basemap:?}: {tile:?} not asked");
            }
        }
        let openfreemap = |scene: &SceneInput| asked(&gpu, scene).into_iter().filter(|(source, _)| source == io::OPENFREEMAP).count();
        // on the realistic map these draw nothing: what OpenFreeMap is asked for there is the
        // realistic buildings' (`buildings_realistic`, R4), the same tiles once each, not twice
        let mut on_realistic: Vec<TileIndex> =
            asked(&gpu, &only(v4(), Basemap::Realistic, true)).into_iter().filter(|(source, _)| source == io::OPENFREEMAP).map(|(_, t)| t).collect();
        let key = |t: &TileIndex| (t.z, t.x, t.y);
        on_dark.sort_by_key(key);
        on_realistic.sort_by_key(key);
        assert_eq!(on_realistic, on_dark);
        assert_eq!(openfreemap(&only(v4(), Basemap::Dark, false)), 0);
        // on the globe, none
        assert_eq!(openfreemap(&only(ViewState { zoom: 12.0, ..v4() }, Basemap::Dark, true)), 0);
    }

    /// Opened straight at V5 (zoom 15.5, pitched 60°), the selection wants zoom-16 tiles everywhere
    /// in view: on the dark map they have no source, and no coarser tile over them is cached, so no
    /// building shows; as today (`LodMVTLayer.getTileData` resolves `null` there and
    /// `keepCoarserUntilIn` keeps only a cached tile over it). Arriving by zooming in, they show:
    /// see `v5_shows_lit_buildings_and_their_shadows_darkening_once`.
    #[test]
    fn opened_straight_at_v5_the_dark_map_has_no_tile_for_the_buildings_close_in() {
        let size = [1440.0, 900.0];
        let viewport = viewport_for(&v5(), size);
        let wanted = distance_tile_indices(&viewport, &tess::city_lod(), None, 0.0);
        assert!(!wanted.is_empty() && wanted.iter().all(|t| t.z == 16), "{wanted:?}");
        let Some(gpu) = gpu() else { return };
        let (image, fixtures) = shots(&gpu, &[dark(v5(), size)], "b7-dark-v5-straight-1440x900.png");
        assert!(fixtures.misses().is_empty(), "{:?}", fixtures.misses());
        let buildings = image.rgba.chunks_exact(4).filter(|p| is_building([p[0], p[1], p[2], p[3]])).count();
        assert_eq!(buildings, 0);
    }

    /// V4 (Arlanda, zoom 13.5 pitched 45° and turned 30°: zoom-14 tiles standing nearly
    /// everywhere, a few far zoom-13 ones flat at the top) against today's picture: the buildings
    /// where today's are, with today's colours (`dark-v4-1440x900.png`).
    #[test]
    fn v4_draws_the_buildings_where_today_s_app_does() {
        let Some(gpu) = gpu() else { return };
        let (image, fixtures) = shots(&gpu, &[dark(v4(), [1440.0, 900.0])], "b7-dark-v4-1440x900.png");
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
        let today = reference("dark-v4-1440x900.png");
        let (share, shift, count) = agreement(&image, &today, is_building);
        let (back, back_shift, ours) = agreement(&today, &image, is_building);
        eprintln!("V4: today's buildings {count} px, {share:.3} kept at {shift:?}; ours {ours} px, {back:.3} in today's at {back_shift:?}");
        assert!(count > 10_000, "{count}");
        assert!(share > 0.95 && back > 0.95, "buildings: {share}, back {back}");
        // where both have a building, the same colour: the lit roofs and walls of today's
        let [dx, dy] = shift;
        let [x0, y0, x1, y1] = MAP;
        let (mut both, mut off) = (0usize, 0.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let (theirs, mine) = (today.pixel(x, y), image.pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32));
                if is_building(theirs) && is_building(mine) {
                    both += 1;
                    off += (0..3).map(|i| f64::from(theirs[i].abs_diff(mine[i]))).sum::<f64>() / 3.0;
                }
            }
        }
        let mean = off / both as f64;
        eprintln!("V4: {both} building pixels in both, {mean:.2} levels apart on average");
        assert!(mean < 2.5, "{mean}");
        // the hangar roof E7 checks the light rig on (`building_roofs_match_the_reference_picture`)
        let (theirs, mine) = (today.pixel(1060, 760), image.pixel((1060 + dx) as u32, (760 + dy) as u32));
        assert!((0..3).all(|i| theirs[i].abs_diff(mine[i]) <= 1), "{theirs:?} against {mine:?}");
    }

    /// V4 at zoom 12.1, just onto the flat map: every tile the distance LOD takes is a far one of
    /// zoom 13, so every building lies flat on the ground in `BUILDING_COLOR`, unlit; no wall, no
    /// lit roof, no shadow.
    #[test]
    fn far_tiles_lie_flat_in_the_building_colour() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { zoom: 12.1, ..v4() };
        let size = [1440.0, 900.0];
        let wanted = distance_tile_indices(&viewport_for(&view, size), &tess::city_lod(), None, 0.0);
        assert!(!wanted.is_empty() && wanted.iter().all(|t| t.z == 13), "{wanted:?}");
        let (image, fixtures) = shots(&gpu, &[dark(view, size)], "b7-dark-v4-z12.1-1440x900.png");
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
        let pixels: Vec<[u8; 4]> = image.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
        let flat = pixels.iter().filter(|p| p[..3] == tess::BUILDING_COLOR).count();
        let buildings = pixels.iter().filter(|p| is_building(**p)).count();
        // (the rest are their edges, blended with the ground)
        let lit = pixels.iter().filter(|p| is_building(**p) && p[2] > tess::BUILDING_COLOR[2]).count();
        let shadow = pixels.iter().filter(|p| p[..3] == shadowed([50, 50, 50], 1)).count();
        eprintln!("V4 at 12.1: {flat} px in the building colour of {buildings} building px; {lit} brighter, {shadow} shadowed");
        assert!(flat > 2_000 && flat * 2 > buildings, "{flat} of {buildings}");
        assert_eq!((lit, shadow), (0, 0));
    }

    /// The ink of `ground` with the buildings' shadow blended over it `times` times at the full
    /// sun's alpha, as the GPU rounds it.
    fn shadowed(ground: [u8; 3], times: usize) -> [u8; 3] {
        let [r, g, b, a] = tess::shadow_color(rustradar_scene::building_shadow::BUILDING_SHADOW_ALPHA, tess::SHADOW_STRENGTH);
        let a = f64::from(a) / 255.0;
        (0..times).fold(ground, |c, _| std::array::from_fn(|i| (f64::from(c[i]) + (f64::from([r, g, b][i]) - f64::from(c[i])) * a).round() as u8))
    }

    /// V5 (Stockholm city, zoom 15.5 pitched 60° and turned 20°), arrived at as one does, from
    /// zoom 14 looking straight down (so its zoom-14 tiles stand in for the zoom-16 ones the dark
    /// map has no source for): the buildings stand, their faces lit in different brightnesses by
    /// the sun-aimed key light; their shadows lie on the ground, the land and the water each
    /// darkened exactly once, and no pixel darker than the darkest single shadow, though the
    /// shadows' parallelograms overlap (the stencil rule).
    #[test]
    fn v5_shows_lit_buildings_and_their_shadows_darkening_once() {
        let Some(gpu) = gpu() else { return };
        let size = [1440.0, 900.0];
        let arriving = ViewState { zoom: 14.0, pitch: 0.0, ..v5() };
        let (image, _) = shots(&gpu, &[dark(arriving, size), dark(v5(), size)], "b7-dark-v5-1440x900.png");
        let pixels: Vec<[u8; 4]> = image.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
        // lit faces: building pixels over a range of brightness, in several well-filled bands
        let buildings: Vec<u32> = pixels.iter().filter(|p| is_building(**p)).map(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])).collect();
        let mut bands = [0usize; 64];
        for &sum in &buildings {
            bands[(sum / 12) as usize] += 1;
        }
        let filled = bands.iter().filter(|&&n| n * 30 > buildings.len()).count();
        eprintln!("V5: {} building pixels; brightness bands {:?}", buildings.len(), bands.iter().enumerate().filter(|(_, n)| **n > 0).collect::<Vec<_>>());
        assert!(buildings.len() > 200_000, "{}", buildings.len());
        assert!(filled >= 5, "{filled} bands");
        // the CPU twin: a roof (up), a wall facing the sun and one facing away, at the centre
        let vf = ViewFrame::new(&v5(), size, 1.0, 0.0);
        let rig = crate::renderer::scene_lights(&v5(), NOW, true);
        let camera = glam::Vec3::from(vf.globals.camera_render).as_dvec3();
        let to_sun = rustradar_core::sun::local_sun(rustradar_core::sun::sun_direction(NOW), v5().latitude, v5().longitude);
        let toward = glam::DVec3::new(to_sun[0], to_sun[1], 0.0).normalize();
        let color = tess::BUILDING_COLOR.map(|c| f64::from(c) / 255.0);
        let face = |n: glam::DVec3| rig.shade(&MATERIAL, color, n, camera, glam::DVec3::from_array(rig.key_direction)).map(|c| c * 255.0);
        let (roof, sunny, shaded) = (face(glam::DVec3::Z), face(toward), face(-toward));
        eprintln!("V5 twin: roof {roof:.1?}, a wall facing the sun {sunny:.1?}, one facing away {shaded:.1?}");
        assert!(shaded[2] + 10.0 < sunny[2] && shaded[2] + 10.0 < roof[2]);
        // shadows: once on the land and on the water, never twice
        let count = |rgb: [u8; 3]| pixels.iter().filter(|p| p[..3] == rgb).count();
        let (land, water) = ([50, 50, 50], [30, 30, 30]);
        let (on_land, on_water) = (count(shadowed(land, 1)), count(shadowed(water, 1)));
        let twice = count(shadowed(land, 2)) + count(shadowed(water, 2));
        let darkest = shadowed(water, 1).iter().map(|&c| u32::from(c)).sum::<u32>();
        let darker = pixels.iter().filter(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) < darkest).count();
        eprintln!("V5 shadows: {on_land} px on the land {:?}, {on_water} on the water {:?}; twice {twice}; darker than the darkest single one {darker}", shadowed(land, 1), shadowed(water, 1));
        assert!(on_land > 5_000 && on_water > 50, "{on_land}, {on_water}");
        assert_eq!(twice, 0);
        assert_eq!(darker, 0);
        // and they do overlap: the centre tile's shadows under this sun cover places twice
        let tile = TileIndex::new(9014, 4818, 14);
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, tile).expect("read").expect("the centre tile");
        let decoded = decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("buildings");
        let sun = ShadowSun::default().at(NOW);
        let shadows = tess::tile_shadows(tile, &decoded.footprints, sun).expect("shadows at noon");
        assert!(overlapping_cells(&shadows, 2048) > 1_000);
    }

    /// Once the sun has moved a quarter of a degree the tiles on screen are recast (within the
    /// frame's budget, the layer pending until all are): half an hour on, V5 is exactly what a
    /// renderer that cast them at that time draws, and its shadows have moved.
    #[test]
    fn shadows_are_recast_when_the_sun_moves() {
        let Some(gpu) = gpu() else { return };
        let size = [1440.0, 900.0];
        let arriving = ViewState { zoom: 14.0, pitch: 0.0, ..v5() };
        let later = NOW + 1_800_000.0;
        let (before, _) = shots(&gpu, &[dark(arriving, size), dark(v5(), size)], "b7-dark-v5-1440x900.png");
        let (recast, _) = shots(&gpu, &[dark(arriving, size), dark(v5(), size), dark_at(v5(), size, later)], "b7-dark-v5-recast-1440x900.png");
        let (fresh, _) = shots(&gpu, &[dark_at(arriving, size, later), dark_at(v5(), size, later)], "b7-dark-v5-later-1440x900.png");
        let differ = |a: &Image, b: &Image| a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).filter(|(p, q)| p != q).count();
        let shade = shadowed([50, 50, 50], 1);
        let moved = before.rgba.chunks_exact(4).zip(recast.rgba.chunks_exact(4)).filter(|(p, q)| (p[..3] == shade) != (q[..3] == shade)).count();
        eprintln!("recast: {} px differ from the fresh cast, {} from half an hour before; {moved} px in or out of a shadow on the land", differ(&recast, &fresh), differ(&recast, &before));
        assert_eq!(differ(&recast, &fresh), 0);
        assert!(moved > 1_000, "{moved}");
    }

    /// Cells of an `n` × `n` grid over the tile whose centre two or more of `shadows`' triangles
    /// cover.
    fn overlapping_cells(shadows: &TileShadows, n: usize) -> usize {
        let mut cover = vec![0u8; n * n];
        let p = |i: u32| [f64::from(shadows.positions[2 * i as usize]), f64::from(shadows.positions[2 * i as usize + 1])];
        for t in shadows.indices.chunks_exact(3) {
            let [a, b, c] = [p(t[0]), p(t[1]), p(t[2])];
            let lo = |k: usize| ((a[k].min(b[k]).min(c[k]) * n as f64).floor().max(0.0) as usize).min(n);
            let hi = |k: usize| ((a[k].max(b[k]).max(c[k]) * n as f64).ceil().max(0.0) as usize).min(n);
            let edge = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
            let area = edge(a, b, c);
            if area == 0.0 {
                continue;
            }
            for y in lo(1)..hi(1) {
                for x in lo(0)..hi(0) {
                    let q = [(x as f64 + 0.5) / n as f64, (y as f64 + 0.5) / n as f64];
                    let (w0, w1, w2) = (edge(b, c, q) / area, edge(c, a, q) / area, edge(a, b, q) / area);
                    if w0 > 0.0 && w1 > 0.0 && w2 > 0.0 {
                        cover[y * n + x] = cover[y * n + x].saturating_add(1);
                    }
                }
            }
        }
        cover.iter().filter(|&&c| c >= 2).count()
    }

    /// On the flat map each tile's buildings are cut at its square, upright (deck's
    /// `ClipExtension`): a building straddling the edge of two tiles is drawn once, half by each,
    /// and with only one of the two tiles served it is cut there.
    #[test]
    fn a_tile_s_buildings_are_cut_at_its_square() {
        let Some(gpu) = gpu() else { return };
        // V4's middle at zoom 13.2, looking straight down: its z14 tile (300 px across), the
        // others not served
        let view = ViewState { zoom: 13.2, pitch: 0.0, bearing: 0.0, ..v4() };
        let size = [600.0, 600.0];
        let centre = TileIndex::new(9007, 4789, 14);
        struct One(DirTiles, TileIndex);
        impl TileFetcher for One {
            fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
                let (tx, rx) = std::sync::mpsc::channel();
                let answer = if request.tile == self.1 { self.0.read(&request.source, request.tile) } else { Ok(None) };
                let _ = tx.send(answer);
                rx
            }
        }
        // the tile holds buildings reaching more than 3 px past its square (into its buffer)
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, centre).expect("read").expect("V4's middle");
        let decoded = decode(centre, &bytes, &Cancel::new()).expect("decoded").expect("buildings");
        let past = decoded.vertices.iter().filter(|v| v.position[..2].iter().any(|&c| !(-0.01..=1.01).contains(&c))).count();
        assert!(past > 10, "{past}");
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(Arc::new(One(DirTiles::fixtures(), centre)), None);
        let mut scene = dark(view, size);
        scene.vector_map = vector_map::Input::default();
        let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        image.save_png(&shots_dir().join("b7-one-tile.png")).expect("written");
        // the tile's square on the screen (straight down, north up)
        let vf = ViewFrame::new(&view, size, 1.0, 0.0);
        let corner = |x: u32, y: u32| {
            let lnglat = rustradar_scene::tiles::osm_tile_to_lng_lat(f64::from(x), f64::from(y), 14);
            let p = vf.viewport.project([lnglat[0], lnglat[1], 0.0]);
            [p[0], p[1]]
        };
        let [west, north] = corner(centre.x, centre.y);
        let [east, south] = corner(centre.x + 1, centre.y + 1);
        let (mut inside, mut outside) = (0, 0);
        for y in 0..600u32 {
            for x in 0..600u32 {
                if !is_building(image.pixel(x, y)) {
                    continue;
                }
                let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                // (a roof up to 2 px past the line: the walls lean out of a view straight down only
                // a little, and the camera is 1.5 heights away)
                if px >= west - 2.0 && px <= east + 2.0 && py >= north - 2.0 && py <= south + 2.0 {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
        eprintln!("one tile [{west:.1}, {north:.1}]–[{east:.1}, {south:.1}]: {inside} building pixels inside, {outside} outside");
        assert!(inside > 1_000, "{inside}");
        assert_eq!(outside, 0);
    }
}
