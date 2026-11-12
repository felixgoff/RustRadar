//! The realistic map's foot shade and building shadows (item R5; doc 04 §7.4, §7.6, §12; doc 01
//! §9.3; rows 9.1 and 9.2 of doc 04 §1): what `BuildingLayer.renderSubLayers` (`building-layer.ts`)
//! lays on the ground under each near tile's buildings in `buildings-city-realistic` (G:1056-1092),
//! the `-foot` PathLayer with `FootShadeExtension` and the `-shadow` SolidPolygonLayer with
//! `HazeFadeExtension` (`realistic.ts`, R:921-965). [`super::buildings_realistic::BuildingsRealistic`]
//! holds a [`BuildingShadows`] and draws each tile's through it before the tile's buildings; the
//! shader is `shaders/overlay.wgsl`.
//!
//! What today's app does, and so this module:
//! - **Which tiles.** The near ones only (zoom ≥ 14, `CITY_CLOSE_ZOOM`, the extruded ones); a far
//!   tile is flat and gets neither (`if (far) return buildings`).
//! - **The foot shade** ([`foot_shade`]): every ring of the tile's footprints (outlines and holes,
//!   `primitivePolygonIndices`) an open path (`_pathType: "open"`: capped at both ends where the ring
//!   closes) 5 m wide ([`FOOT_SHADE_M`], `widthUnits: "meters"`), centred on the wall so half of it
//!   is under the building, in the shadow colour at alpha `round(0.42 · 255)` ([`FOOT_SHADE_ALPHA`],
//!   [`foot_color`]); the shader fades it from its middle line to nothing at its edges and into the
//!   distance's air. Drawn whatever the sun (at night too), as today.
//! - **The shadows** (doc 04 §7.6): the tile's footprints swept away from the sun
//!   (`rustradar_scene::tess::buildings::tile_shadows`: the sun at the tile's middle, Streets GL's
//!   buildings from half way up their roofs, `castHeight`), in [`SHADOW_COLOR`] `[6,12,28]` at
//!   `round(min(1, alpha · strength) · 255)` ([`shadow_color`]), `alpha = 0.35 · sunlight(up)`
//!   and the weather's [`shadow_strength`] `round(sunStrength · 1.55 · 20) / 20` (`sunStrength =
//!   max(look.sunColor) / 3.2`, this frame's light); none drawn while the strength is 0. Their
//!   alpha fades into the distance's air (`rl_clear`). Not clipped to the tile: a shadow reaches
//!   into the next one.
//! - **The sun** ([`RealisticShadowSun`], `realisticShadowSun`, G:1021-1029): with *daylight* on
//!   the real sun at the feed clock held until it has moved a quarter of a degree (`shadowSun`,
//!   `tess::ShadowSun`); with it off the fixed afternoon sun (`sceneSun(false)`, azimuth 215°,
//!   altitude 35° at the view's centre) as a direction from the Earth's centre, held the same way.
//!   A new tile gets its shadows at once; when the sun has moved, the tiles on screen are recast
//!   within `RECAST_BUDGET_MS` of main-thread time a frame (at least one a frame), the others
//!   keeping their old shadows meanwhile, and the layer is pending until all are recast (as B7's
//!   plain buildings, `renderLayers` and `shadowsOf`).
//! - **Order and the stencil**: per tile the foot shade, then the shadows, then the buildings
//!   (`[...ground, buildings, ...over]`), both with `SHADOW_PARAMETERS` ([`DrawState::SHADOW`]):
//!   depth tested, not written, no culling, a pixel darkened once per frame by whichever shadow
//!   comes first (so within 2.5 m of a wall the foot shade, not the cast shadow; overlapping
//!   parallelograms and neighbouring buildings' shadows never darken twice).
//!
//! How it is checked (the tests below): V5 from the fixtures at a low morning sun (9.4° up),
//! drawn over the land under the sky with the ground's tiles left out and again without the
//! buildings, so a pixel that is the bare one blended once with the shadow colour is shade, its
//! alpha read back: the shadows' alpha is the tile's (strength 0.8 at that sun) times the CPU
//! twin's `rl_clear`, within 0.01 on average (0.0003 here); the sun whose shadows best hold the shadow pixels
//! on the ground (every azimuth tried against the footprints) lies within 3° of the real one
//! (0.08° here), the opposite sun holding far fewer; where two or more of the CPU's
//! parallelograms overlap the picture is darkened once, and nowhere is it darker than one layer of
//! shade. Over the whole city ground (whose tile clips use the stencil and leave it at 0) the
//! same holds. Close in, the foot shade's alpha falls from 0.42 at the wall to nothing 2.5 m out as
//! `FootShadeExtension` has it. With *daylight* off the shadows are the afternoon sun's (215°);
//! when the sun moves the tiles are recast to exactly what a fresh cast draws; at night the
//! strength is 0 and no tile casts.
//!
//! Deviations, invisible: the foot shade's path is `rustradar_scene::geom::path`'s segments of
//! each ring (deck's PathLayer tessellation, made on the main thread when the tile is first drawn,
//! as deck makes its attributes); a tile's shadow colour is a uniform of its own (deck's constant
//! `getFillColor`), written again only when it changes; the real sun is held by this layer alone
//! (today's `buildingSun` is one for the dark map's buildings and these, so after a switch of
//! basemap it may be up to a quarter of a degree older there).
//!
//! Cost (K8): V5's 65 Streets GL tiles hold 34 516 foot-shade segments (2.9 MB of instances) and
//! 17 252 shadow parallelograms at the low sun (1 MB); making all of them takes 18 ms and 10 ms in a
//! debug build (1.3 ms for the worst tile), on the main thread as the tiles arrive (at most four a
//! frame) and when the sun has moved (within the recasting budget). Two draws a near tile.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use rustradar_core::num::js_round;
use rustradar_scene::geom::path::{LocalMercator, Paint, PathBuilder, PathSegment, PathType};
use rustradar_scene::tess::buildings::{self as tess, Footprints, RealisticTile, ShadowSun};

use super::buildings_realistic::TileMeshes;
use super::{DrawCx, Init, PrepareCx, TileIndex, TileSet};
use crate::frame::FrameSlot;
use crate::look::{LightUniform, Look, from_local, scene_sun};
use crate::pipelines::fill::{self, uniform_group, uniform_layout};
use crate::pipelines::{Desc, build, path};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/overlay.wgsl`.
pub const SHADER: &str = include_str!("../shaders/overlay.wgsl");

/// The bind group of a tile's [`ShadowUniform`] (after the light's, group 2).
pub const SHADOW_GROUP: u32 = 3;

/// Shadows in sunshine: a deep blue, the sky's light being all that is left in them
/// (`REALISTIC_SHADOW_COLOR`, G:1012).
pub const SHADOW_COLOR: [u8; 3] = [6, 12, 28];
/// How dark a building's shadow gets in full sun, against its usual (`REALISTIC_SHADOW_STRENGTH`,
/// G:1014).
pub const SHADOW_STRENGTH: f64 = 1.55;
/// How dark the ground gets at the foot of a wall (`FOOT_SHADE_ALPHA`, G:1016).
pub const FOOT_SHADE_ALPHA: f64 = 0.42;
/// The band of shade along the foot of a wall, metres across, half under the building
/// (`FOOT_SHADE_M`, `BL:325`).
pub const FOOT_SHADE_M: f64 = 5.0;

/// How much of the sun gets through now, 0 to 1 (`sunStrength`, G:1031: `Math.max(...look.sunColor)
/// / 3.2`).
pub fn sun_strength(look: &Look) -> f64 {
    look.sun_color.iter().copied().fold(f64::NEG_INFINITY, f64::max) / 3.2
}

/// How dark the shadows are against their own, in steps of a twentieth so the tiles are not redrawn
/// for every flicker of the light (`Math.round(sunStrength() * REALISTIC_SHADOW_STRENGTH * 20) /
/// 20`, G:1081).
pub fn shadow_strength(look: &Look) -> f64 {
    js_round(sun_strength(look) * SHADOW_STRENGTH * 20.0) / 20.0
}

/// A tile's shadow colour (`[...shadowColor, Math.round(Math.min(1, shadow.alpha *
/// shadowStrength) * 255)]`).
pub fn shadow_color(alpha: f64, strength: f64) -> [u8; 4] {
    let a = js_round((alpha * strength).min(1.0) * 255.0);
    [SHADOW_COLOR[0], SHADOW_COLOR[1], SHADOW_COLOR[2], a.clamp(0.0, 255.0) as u8]
}

/// The foot shade's colour (`[...shadowColor, Math.round(footShade * 255)]`): alpha 107.
pub fn foot_color() -> [u8; 4] {
    [SHADOW_COLOR[0], SHADOW_COLOR[1], SHADOW_COLOR[2], js_round(FOOT_SHADE_ALPHA * 255.0) as u8]
}

/// The foot shade of a tile's footprints: each ring an open path [`FOOT_SHADE_M`] wide in
/// [`foot_color`], in the tile's units (the `-foot` PathLayer's data: `startIndices:
/// primitivePolygonIndices`, `getPath: positions`, `positionFormat: "XY"`, `_pathType: "open"`).
pub fn foot_shade(tile: TileIndex, footprints: &Footprints) -> Vec<PathSegment> {
    let mut builder = PathBuilder::new(LocalMercator::tile(tile));
    let paint = Paint { path_type: PathType::Open, ..Paint::solid(foot_color(), FOOT_SHADE_M) };
    let mut ring = Vec::new();
    for w in footprints.ring_starts.windows(2) {
        ring.clear();
        ring.extend((w[0] as usize..w[1] as usize).map(|i| [f64::from(footprints.positions[2 * i]), f64::from(footprints.positions[2 * i + 1]), 0.0]));
        builder.push(&ring, &paint);
    }
    builder.finish()
}

/// The sun the realistic map's building shadows are cast with (`realisticShadowSun`, G:1021-1029):
/// with *daylight* on the real one (`shadowSun`: held until it has moved a quarter of a degree);
/// with it off the fixed afternoon sun over the view's centre, as a direction from the Earth's
/// centre, held the same way.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RealisticShadowSun {
    real: ShadowSun,
    fixed: Option<[f64; 3]>,
}

impl RealisticShadowSun {
    /// The sun to cast with at `now_ms` (the feed clock) with the view's centre at `lat`, `lon`.
    pub fn at(&mut self, daylight: bool, now_ms: f64, lat: f64, lon: f64) -> [f64; 3] {
        if daylight {
            return self.real.at(now_ms);
        }
        let sun = from_local(scene_sun(false, [0.0, 0.0, 1.0]), lat, lon);
        match self.fixed {
            Some(held) if sun[0] * held[0] + sun[1] * held[1] + sun[2] * held[2] >= tess::shadow_sun_step() => held,
            _ => {
                self.fixed = Some(sun);
                sun
            }
        }
    }
}

/// `overlay.wgsl`'s `TileShadow`: a tile's shadow colour, 0–1.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ShadowUniform {
    pub color: [f32; 4],
}

impl ShadowUniform {
    pub fn new(color: [u8; 4]) -> Self {
        ShadowUniform { color: color.map(|c| f32::from(c) / 255.0) }
    }
}

/// A shadow corner: `(u, v)` tile units, as `TileShadows::positions` holds them.
const SHADOW_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 8,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
};

/// The corners of a segment (deck's `SEGMENT_POSITIONS`, as `pipelines::path`'s).
const CORNERS: [[f32; 2]; 6] = [[0.0, 0.0], [0.0, -1.0], [0.0, 1.0], [1.0, -1.0], [1.0, 1.0], [1.0, 0.0]];
/// deck's `SEGMENT_INDICES`.
const CORNER_INDICES: [u16; 12] = [0, 1, 2, 1, 4, 2, 1, 3, 4, 3, 5, 4];
const CORNER_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 8,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
};

/// A tile's shadows under one sun on the GPU, with their colour.
struct Shadows {
    mesh: fill::Mesh<[f32; 2]>,
    /// `0.35 · sunlight(up)` at the tile's middle.
    alpha: f64,
    /// What the uniform holds.
    color: [u8; 4],
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

/// A tile's shadows cast under `sun` (`None`: it casts none, the sun too low or nothing tall).
struct Cast {
    sun: [f64; 3],
    shadows: Option<Shadows>,
}

/// What this module keeps of one near tile: its foot shade, and its shadows once cast.
struct Shade {
    /// Of the footprints it was made from: their polygons and vertices (a tile loaded again gets
    /// the same; one that differs is made anew).
    key: (usize, usize),
    /// The foot shade's segments (an instance buffer; `None` with none).
    foot: Option<wgpu::Buffer>,
    foot_count: u32,
    cast: Option<Cast>,
}

fn key_of(footprints: &Footprints) -> (usize, usize) {
    (footprints.len(), footprints.positions.len())
}

/// The foot shade and shadows of [`super::buildings_realistic::BuildingsRealistic`]'s near tiles:
/// [`Self::prepare`] each frame the layer draws (after its tiles are updated), [`Self::draw_tile`]
/// in its per-tile loop before the tile's buildings, [`Self::clear`] when it stops drawing.
pub struct BuildingShadows {
    light: LightUniform,
    foot: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    shadow_layout: wgpu::BindGroupLayout,
    corners: wgpu::Buffer,
    corner_indices: wgpu::Buffer,
    sun: RealisticShadowSun,
    tiles: HashMap<TileIndex, Shade>,
    /// This frame's [`shadow_strength`].
    strength: f64,
    /// Tiles on screen left with shadows of an older sun this frame.
    stale: usize,
}

impl BuildingShadows {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let light = LightUniform::new(device);
        let shadow_layout = uniform_layout(device, "building shadow colour", size_of::<ShadowUniform>());
        let module = shaders::module(device, "building shadows", SHADER);
        let pipeline = |label, vertex_entry, fragment_entry, buffers: &[Option<wgpu::VertexBufferLayout<'_>>], groups: &[&wgpu::BindGroupLayout]| {
            build(
                init,
                &Desc { label, module: &module, vertex_entry, fragment_entry, buffers, topology: wgpu::PrimitiveTopology::TriangleList, state: DrawState::SHADOW, groups },
            )
        };
        let foot = pipeline("building foot shade", "vs_foot", "fs_foot", &[Some(CORNER_LAYOUT), Some(path::SEGMENT_LAYOUT)], &[light.layout()]);
        let shadow = pipeline("building shadows", "vs_shadow", "fs_shadow", &[Some(SHADOW_LAYOUT)], &[light.layout(), &shadow_layout]);
        let buffer = |contents: &[u8], usage| {
            wgpu::util::DeviceExt::create_buffer_init(device, &wgpu::util::BufferInitDescriptor { label: Some("foot shade corners"), contents, usage })
        };
        BuildingShadows {
            light,
            foot,
            shadow,
            shadow_layout,
            corners: buffer(bytemuck::cast_slice(&CORNERS), wgpu::BufferUsages::VERTEX),
            corner_indices: buffer(bytemuck::cast_slice(&CORNER_INDICES), wgpu::BufferUsages::INDEX),
            sun: RealisticShadowSun::default(),
            tiles: HashMap::new(),
            strength: 0.0,
            stale: 0,
        }
    }

    /// This frame: the light, the strength and the sun; the foot shade of every near tile drawn
    /// that has none yet, and the shadows of those whose sun is not this one's (a new tile at once,
    /// the others within the frame's recasting budget); the colours of the shadows. What is kept
    /// of tiles gone from `tiles` goes with them.
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, tiles: &TileSet<RealisticTile, TileMeshes>) {
        let look = self.light.prepare(cx);
        self.strength = shadow_strength(&look);
        let centre = &cx.view.view;
        let sun = self.sun.at(cx.scene.sky.daylight, cx.scene.feed_now_ms, centre.latitude, centre.longitude);
        // (deck's are keyed by content: gone with it)
        self.tiles.retain(|&tile, shade| tiles.get(tile).is_some_and(|m| m.extruded && key_of(&m.footprints) == shade.key));
        let device = &cx.gpu.device;
        let until = Instant::now() + Duration::from_secs_f64(tess::RECAST_BUDGET_MS / 1000.0);
        let mut recast_any = false;
        self.stale = 0;
        for (tile, meshes) in tiles.visible() {
            if !meshes.extruded {
                continue;
            }
            let shade = self.tiles.entry(tile).or_insert_with(|| {
                let segments = foot_shade(tile, &meshes.footprints);
                let foot = (!segments.is_empty()).then(|| {
                    wgpu::util::DeviceExt::create_buffer_init(
                        device,
                        &wgpu::util::BufferInitDescriptor { label: Some("foot shade"), contents: bytemuck::cast_slice(&segments), usage: wgpu::BufferUsages::VERTEX },
                    )
                });
                Shade { key: key_of(&meshes.footprints), foot, foot_count: segments.len() as u32, cast: None }
            });
            // a new tile always gets its shadows; one under an older sun keeps them if the frame is spent
            match &shade.cast {
                Some(cast) if cast.sun == sun => {}
                Some(_) if recast_any && Instant::now() > until => self.stale += 1,
                _ => {
                    recast_any = true;
                    let shadows = tess::tile_shadows(tile, &meshes.footprints, sun).map(|s| {
                        let color = shadow_color(s.alpha, self.strength);
                        let (buffer, group) = uniform_group(device, "building shadow colour", &self.shadow_layout, &ShadowUniform::new(color));
                        let mesh = fill::Mesh::new(device, "building shadows", bytemuck::cast_slice::<f32, [f32; 2]>(&s.positions), &s.indices);
                        Shadows { mesh, alpha: s.alpha, color, buffer, group }
                    });
                    shade.cast = Some(Cast { sun, shadows });
                }
            }
            // the weather's say, this frame
            if let Some(Cast { shadows: Some(shadows), .. }) = &mut shade.cast {
                let color = shadow_color(shadows.alpha, self.strength);
                if color != shadows.color {
                    shadows.color = color;
                    cx.gpu.queue.write_buffer(&shadows.buffer, 0, bytemuck::bytes_of(&ShadowUniform::new(color)));
                }
            }
        }
    }

    /// Draws `tile`'s foot shade, then its shadows (while the strength is above 0), in its frame
    /// `slot`; nothing for a tile it holds nothing of (a far one).
    pub fn draw_tile(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, slot: FrameSlot, tile: TileIndex) {
        let Some(shade) = self.tiles.get(&tile) else { return };
        if let Some(foot) = &shade.foot {
            pass.set_pipeline(&self.foot);
            self.light.bind(pass);
            cx.bind_frame(pass, slot);
            pass.set_vertex_buffer(0, self.corners.slice(..));
            pass.set_index_buffer(self.corner_indices.slice(..), wgpu::IndexFormat::Uint16);
            pass.set_vertex_buffer(1, foot.slice(..));
            pass.draw_indexed(0..CORNER_INDICES.len() as u32, 0, 0..shade.foot_count);
        }
        if self.strength <= 0.0 {
            return;
        }
        if let Some(Cast { shadows: Some(shadows), .. }) = &shade.cast {
            pass.set_pipeline(&self.shadow);
            self.light.bind(pass);
            pass.set_bind_group(SHADOW_GROUP, &shadows.group, &[]);
            cx.bind_frame(pass, slot);
            shadows.mesh.draw(pass);
        }
    }

    /// Drops every tile's (the layer stopped drawing).
    pub fn clear(&mut self) {
        self.tiles.clear();
        self.stale = 0;
    }

    /// Whether tiles on screen still wait for their shadows to be recast.
    pub fn pending(&self) -> bool {
        self.stale > 0
    }

    /// This frame's [`shadow_strength`].
    pub fn strength(&self) -> f64 {
        self.strength
    }

    /// The shadows `tile` holds: their sun and alpha (tests).
    pub fn cast_of(&self, tile: TileIndex) -> Option<([f64; 3], Option<f64>)> {
        let cast = self.tiles.get(&tile)?.cast.as_ref()?;
        Some((cast.sun, cast.shadows.as_ref().map(|s| s.alpha)))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path as FilePath, PathBuf};
    use std::sync::Arc;

    use glam::DVec2;
    use rustradar_core::geo::Basemap;
    use rustradar_core::sun::{local_sun, sun_direction, sunlight};
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::building_shadow::{BUILDING_SHADOW_ALPHA, shadow_step};
    use rustradar_scene::camera::{ViewState, lng_lat_to_world, mercator_units_per_meter};
    use rustradar_scene::geom::path::{END_CAP, START_CAP};
    use rustradar_scene::lod::distance_tile_indices;
    use rustradar_scene::streets_gl::SGL_ZOOM;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, render_with};
    use crate::io::{self, DirTiles};
    use crate::jobs::Cancel;
    use crate::layers::buildings_realistic::decode;
    use crate::layers::{buildings, earth, ground, sky};
    use crate::look::{RLight, frame_look, twin};
    use crate::renderer::{MapRenderer, SceneInput};

    /// V5 (plan section 8): Stockholm city, pitched, at today's pictures' size.
    const V5: ViewState = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
    const V5_SIZE: [f64; 2] = [1440.0, 900.0];
    /// 2026-06-21 03:20 UTC: 05:20 in Stockholm, the sun 9.4° up in the east-north-east.
    const LOW_SUN: f64 = 1_782_012_000_000.0;
    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// 2026-12-21 22:00 UTC: night over Stockholm.
    const MIDWINTER_NIGHT: f64 = 1_797_890_400_000.0;

    fn shots_dir() -> PathBuf {
        let dir = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// The realistic map at `view` on a `size` canvas at `now` on a fine day: the sky and the land
    /// under it, the buildings when `buildings`, the ground's tiles when `with_ground`.
    fn realistic(view: ViewState, size: [f64; 2], now: f64, buildings: bool, with_ground: bool) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Realistic);
        scene.local_now_ms = now;
        scene.feed_now_ms = now;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        if with_ground {
            scene.ground = ground::Input::SHOWN;
        }
        scene.buildings = buildings::Input { shown: buildings };
        scene
    }

    // ------------------------------------------------------------------ the numbers

    #[test]
    fn the_shader_validates_and_the_numbers_are_today_s() {
        shaders::tests::validate("overlay.wgsl", &shaders::expand(SHADER).expect("libraries known"));
        assert_eq!(SHADOW_COLOR, [6, 12, 28]);
        assert_eq!(foot_color(), [6, 12, 28, 107]);
        // `Math.round(Math.min(1, alpha * strength) * 255)`
        assert_eq!(shadow_color(0.35, 0.8), [6, 12, 28, 71]);
        assert_eq!(shadow_color(0.35, 1.55), [6, 12, 28, 138]);
        assert_eq!(shadow_color(0.35, 3.0), [6, 12, 28, 255]);
        assert_eq!(shadow_color(0.35, 0.0), [6, 12, 28, 0]);
        // `Math.round(Math.max(...look.sunColor) / 3.2 * 1.55 * 20) / 20`, here at the morning sun
        // and at noon of the test views (sunColor's largest 1.6527… and 3.0937…)
        let input = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        let mut look = frame_look(&input, &V5, V5_SIZE, LOW_SUN);
        assert_eq!(shadow_strength(&look), 0.8);
        assert_eq!(shadow_strength(&frame_look(&input, &V5, V5_SIZE, MIDSUMMER)), 1.5);
        look.sun_color = [3.2, 0.0, 1.0];
        assert_eq!(shadow_strength(&look), 1.55);
        look.sun_color = [0.5, 1.0, 0.2];
        // 1 / 3.2 · 1.55 · 20 = 9.6875
        assert_eq!(shadow_strength(&look), 0.5);
        look.sun_color = [0.0; 3];
        assert_eq!(shadow_strength(&look), 0.0);
        assert_eq!(shadow_strength(&frame_look(&input, &V5, V5_SIZE, MIDWINTER_NIGHT)), 0.0);
    }

    /// Each ring (outline and hole) is a path of its own, open (capped at both ends where it
    /// closes), 5 m wide in the foot shade's colour, on the ground in the tile's units.
    #[test]
    fn the_foot_shade_is_each_ring_an_open_path_five_metres_wide() {
        let tile = TileIndex::new(36057, 19267, 16);
        let outline = [[0.1f32, 0.1], [0.3, 0.1], [0.3, 0.3], [0.1, 0.3], [0.1, 0.1]];
        let hole = [[0.15f32, 0.15], [0.15, 0.2], [0.2, 0.2], [0.15, 0.15]];
        let other = [[0.5f32, 0.5], [0.6, 0.5], [0.6, 0.6], [0.5, 0.5]];
        let mut f = Footprints::default();
        for (rings, height) in [(vec![&outline[..], &hole[..]], 20.0), (vec![&other[..]], 0.0)] {
            f.polygon_starts.push((f.positions.len() / 2) as u32);
            for ring in rings {
                f.ring_starts.push((f.positions.len() / 2) as u32);
                f.positions.extend(ring.iter().flatten());
            }
            f.heights.push(height);
        }
        f.polygon_starts.push((f.positions.len() / 2) as u32);
        f.ring_starts.push((f.positions.len() / 2) as u32);
        let segments = foot_shade(tile, &f);
        // every edge of every ring, a building of no height too
        assert_eq!(segments.len(), 4 + 3 + 3);
        for (range, ring) in [(0..4, &outline[..]), (4..7, &hole[..]), (7..10, &other[..])] {
            let s = &segments[range.clone()];
            for (k, segment) in s.iter().enumerate() {
                assert_eq!(segment.start, [ring[k][0], ring[k][1], 0.0]);
                assert_eq!(segment.end, [ring[k + 1][0], ring[k + 1][1], 0.0]);
                assert_eq!(segment.width, 5.0);
                assert_eq!(segment.color, [6, 12, 28, 107]);
                assert_eq!(segment.dash_array, [0.0, 0.0]);
            }
            let last = s.len() - 1;
            assert_eq!(s[0].types & START_CAP, START_CAP);
            assert_eq!(s[last].types & END_CAP, END_CAP);
            assert!(s[1..last].iter().all(|seg| seg.types == 0));
            // joined to its neighbours in between, the ends left as they are
            assert_eq!(s[0].left, s[0].start);
            assert_eq!(s[last].right, s[last].end);
            assert_eq!(s[1].left, s[0].start);
            assert_eq!(s[0].right, s[1].end);
        }
        assert!(foot_shade(tile, &Footprints::default()).is_empty());
    }

    /// With daylight the real sun, held until it has moved a quarter of a degree (B7's
    /// `ShadowSun`); without, the fixed afternoon sun over the view's centre (south-west, 35° up),
    /// held the same way as the view moves.
    #[test]
    fn the_shadow_sun_is_the_real_one_or_the_fixed_afternoon_one() {
        let mut sun = RealisticShadowSun::default();
        let mut real = ShadowSun::default();
        for minutes in [0.0, 0.5, 1.0, 1.5, 30.0] {
            let now = MIDSUMMER + minutes * 60_000.0;
            assert_eq!(sun.at(true, now, V5.latitude, V5.longitude), real.at(now));
        }
        // the sun held, not the one now
        assert_eq!(sun.at(true, MIDSUMMER + 30.5 * 60_000.0, 0.0, 0.0), sun_direction(MIDSUMMER + 30.0 * 60_000.0));
        let fixed = sun.at(false, MIDSUMMER, V5.latitude, V5.longitude);
        let [e, n, u] = local_sun(fixed, V5.latitude, V5.longitude);
        let (az, alt) = (e.atan2(n).to_degrees(), u.asin().to_degrees());
        assert!((az - (215.0 - 360.0)).abs() < 1e-9 && (alt - 35.0).abs() < 1e-9, "{az} {alt}");
        // a tenth of a degree away the same; a degree away a new one; the clock changes nothing
        assert_eq!(sun.at(false, LOW_SUN, V5.latitude + 0.1, V5.longitude), fixed);
        let moved = sun.at(false, LOW_SUN, V5.latitude + 1.0, V5.longitude);
        assert_ne!(moved, fixed);
        let [e, n, u] = local_sun(moved, V5.latitude + 1.0, V5.longitude);
        assert!((e.atan2(n).to_degrees() + 145.0).abs() < 1e-9 && (u.asin().to_degrees() - 35.0).abs() < 1e-9);
    }

    // ------------------------------------------------------------------ the pictures

    /// Where `with` is `bare` blended once with the shadow colour at some alpha: that alpha (`bare`
    /// far enough from the colour on some channel to tell; each channel within 2 levels).
    fn blended(bare: [u8; 4], with: [u8; 4]) -> Option<f64> {
        let s = SHADOW_COLOR.map(f64::from);
        let b = [0, 1, 2].map(|i| f64::from(bare[i]));
        let w = [0, 1, 2].map(|i| f64::from(with[i]));
        let c = (0..3).max_by(|&i, &j| (b[i] - s[i]).abs().total_cmp(&(b[j] - s[j]).abs()))?;
        if (b[c] - s[c]).abs() < 40.0 || bare == with {
            return None;
        }
        let a = (b[c] - w[c]) / (b[c] - s[c]);
        (a > 0.0 && (0..3).all(|i| (b[i] + (s[i] - b[i]) * a - w[i]).abs() <= 2.0)).then_some(a)
    }

    /// Metres east and north of the view's centre on the ground (at the centre's scale).
    struct Ground {
        centre: DVec2,
        units_per_metre: f64,
    }

    impl Ground {
        fn new(view: &ViewState) -> Self {
            Ground { centre: DVec2::from(lng_lat_to_world([view.longitude, view.latitude])), units_per_metre: mercator_units_per_meter(view.latitude) }
        }

        fn world(&self, w: DVec2) -> DVec2 {
            (w - self.centre) / self.units_per_metre
        }

        fn lng_lat(&self, ll: [f64; 2]) -> DVec2 {
            self.world(DVec2::from(lng_lat_to_world(ll)))
        }
    }

    /// The near tiles of `view` (as the layer takes them, all in the fixtures) decoded.
    fn near_tiles(view: &ViewFrame) -> Vec<(TileIndex, RealisticTile)> {
        let fixtures = DirTiles::fixtures();
        let mut out = Vec::new();
        for tile in distance_tile_indices(&view.viewport, &tess::city_lod(), None, 0.0) {
            let source = if tile.z == SGL_ZOOM { io::STREETS_GL } else { io::OPENFREEMAP };
            let bytes = fixtures.read(source, tile).expect("read").expect("a fixture tile");
            if let Ok(Some(decoded)) = decode(tile, &bytes, &Cancel::new())
                && decoded.extruded
            {
                out.push((tile, decoded));
            }
        }
        out
    }

    /// The shadow parallelograms of `tiles` under `sun` (from the Earth's centre), in metres
    /// about the view's centre, with a grid of them for lookups.
    struct Quads {
        quads: Vec<[DVec2; 4]>,
        cells: HashMap<(i64, i64), Vec<usize>>,
    }

    const CELL_M: f64 = 25.0;

    impl Quads {
        fn new(tiles: &[(TileIndex, RealisticTile)], ground: &Ground, sun: [f64; 3]) -> Self {
            let mut quads = Vec::new();
            for (tile, decoded) in tiles {
                let local = LocalMercator::tile(*tile);
                let Some(shadows) = tess::tile_shadows(*tile, &decoded.footprints, sun) else { continue };
                for q in shadows.positions.chunks_exact(8) {
                    quads.push(std::array::from_fn(|k| ground.world(local.to_world([f64::from(q[2 * k]), f64::from(q[2 * k + 1])]))));
                }
            }
            let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
            for (i, q) in quads.iter().enumerate() {
                let lo = q.iter().fold(DVec2::splat(f64::INFINITY), |m, p| m.min(*p));
                let hi = q.iter().fold(DVec2::splat(f64::NEG_INFINITY), |m, p| m.max(*p));
                for cy in (lo.y / CELL_M).floor() as i64..=(hi.y / CELL_M).floor() as i64 {
                    for cx in (lo.x / CELL_M).floor() as i64..=(hi.x / CELL_M).floor() as i64 {
                        cells.entry((cx, cy)).or_default().push(i);
                    }
                }
            }
            Quads { quads, cells }
        }

        /// How many parallelograms hold `p`.
        fn count(&self, p: DVec2) -> usize {
            let cell = ((p.x / CELL_M).floor() as i64, (p.y / CELL_M).floor() as i64);
            let inside = |a: DVec2, b: DVec2, c: DVec2| {
                let d = [(b - a).perp_dot(p - a), (c - b).perp_dot(p - b), (a - c).perp_dot(p - c)];
                d.iter().all(|&x| x >= 0.0) || d.iter().all(|&x| x <= 0.0)
            };
            self.cells.get(&cell).map_or(0, |list| list.iter().filter(|&&i| {
                let [a, b, c, d] = self.quads[i];
                inside(a, b, c) || inside(a, c, d)
            }).count())
        }
    }

    /// The cast heights of `tiles`' buildings on a 1 m grid about the view's centre (`RADIUS_M`
    /// each way): the tallest footprint over each cell's middle (rings even-odd, as the tiles
    /// hold them), 0 where there is none.
    struct Heights {
        cells: Vec<f32>,
    }

    const RADIUS_M: i64 = 1_600;
    const SIDE: usize = 2 * RADIUS_M as usize;

    impl Heights {
        fn new(tiles: &[(TileIndex, RealisticTile)], ground: &Ground) -> Self {
            let mut cells = vec![0.0f32; SIDE * SIDE];
            for (tile, decoded) in tiles {
                let local = LocalMercator::tile(*tile);
                let f = &decoded.footprints;
                let point = |i: usize| ground.world(local.to_world([f64::from(f.positions[2 * i]), f64::from(f.positions[2 * i + 1])]));
                for (p, w) in f.polygon_starts.windows(2).enumerate() {
                    let height = f.heights[p];
                    if height <= 0.0 {
                        continue;
                    }
                    let rings: Vec<Vec<DVec2>> =
                        f.ring_starts.windows(2).filter(|r| r[0] >= w[0] && r[0] < w[1]).map(|r| (r[0] as usize..r[1] as usize).map(point).collect()).collect();
                    let lo = rings.iter().flatten().fold(DVec2::splat(f64::INFINITY), |m, p| m.min(*p));
                    let hi = rings.iter().flatten().fold(DVec2::splat(f64::NEG_INFINITY), |m, p| m.max(*p));
                    let (y0, y1) = ((lo.y.floor() as i64).max(-RADIUS_M), (hi.y.ceil() as i64).min(RADIUS_M - 1));
                    for cy in y0..=y1 {
                        let y = cy as f64 + 0.5;
                        let mut xs: Vec<f64> = Vec::new();
                        for ring in &rings {
                            for e in ring.windows(2) {
                                let (a, b) = (e[0], e[1]);
                                if (a.y <= y) != (b.y <= y) {
                                    xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
                                }
                            }
                        }
                        xs.sort_by(f64::total_cmp);
                        for pair in xs.chunks_exact(2) {
                            let (x0, x1) = (((pair[0] - 0.5).ceil() as i64).max(-RADIUS_M), ((pair[1] - 0.5).floor() as i64).min(RADIUS_M - 1));
                            for cx in x0..=x1 {
                                let cell = &mut cells[(cy + RADIUS_M) as usize * SIDE + (cx + RADIUS_M) as usize];
                                *cell = cell.max(height);
                            }
                        }
                    }
                }
            }
            Heights { cells }
        }

        fn at(&self, p: DVec2) -> f32 {
            let (cx, cy) = (p.x.floor() as i64, p.y.floor() as i64);
            if cx < -RADIUS_M || cy < -RADIUS_M || cx >= RADIUS_M || cy >= RADIUS_M {
                return 0.0;
            }
            self.cells[(cy + RADIUS_M) as usize * SIDE + (cx + RADIUS_M) as usize]
        }

        /// Whether `p` is in a building's shadow under a sun at `azimuth` (degrees) whose shadows
        /// are `per_metre` long for each metre of height: towards the sun from `p`, a footprint
        /// tall enough to reach it.
        fn shaded(&self, p: DVec2, azimuth: f64, per_metre: f64, reach: f64) -> bool {
            let t = azimuth.to_radians();
            let toward = DVec2::new(t.sin(), t.cos());
            let mut d = 0.5;
            while d < reach {
                if f64::from(self.at(p + toward * d)) * per_metre >= d {
                    return true;
                }
                d += 1.0;
            }
            false
        }
    }

    /// The sun's azimuth (degrees) whose buildings' shadows best hold `points` (metres east, north;
    /// shadowed in the picture): the share of them in the shadows of a sun at each azimuth every
    /// 2°, then every quarter of a degree within 3° of the best; the best azimuth and its share,
    /// and the share at the azimuth opposite.
    fn fitted_sun(heights: &Heights, points: &[DVec2], per_metre: f64) -> (f64, f64, f64) {
        let tallest = heights.cells.iter().copied().fold(0.0f32, f32::max);
        let reach = f64::from(tallest) * per_metre + 1.0;
        let share = |azimuth: f64| points.iter().filter(|&&p| heights.shaded(p, azimuth, per_metre, reach)).count() as f64 / points.len() as f64;
        let best = |candidates: Vec<f64>| candidates.into_iter().map(|a| (share(a), a)).max_by(|x, y| x.0.total_cmp(&y.0)).expect("candidates");
        let (_, coarse) = best((0..180).map(|k| f64::from(k) * 2.0).collect());
        let (fit, azimuth) = best((-12..=12).map(|k| coarse + f64::from(k) * 0.25).collect());
        (azimuth.rem_euclid(360.0), fit, share(azimuth + 180.0))
    }

    /// The sun's azimuth (degrees from north) and altitude at `view`'s centre at `now`.
    fn sun_at(view: &ViewState, now: f64) -> (f64, f64) {
        let [e, n, u] = local_sun(sun_direction(now), view.latitude, view.longitude);
        (e.atan2(n).to_degrees().rem_euclid(360.0), u.asin().to_degrees())
    }

    /// The acceptance of R5: V5 from the fixtures at a low morning sun (9.4° up in the east-north-
    /// east), drawn over the land under the sky alone (the ground's tiles left out) and again
    /// without the buildings: a pixel that is the bare one blended once with the shadow colour is
    /// a shadow's or the foot shade's, its alpha read back.
    /// - the shadows' alpha is the tile's, `round(min(1, 0.35 · 0.8) · 255)` (strength 0.8 at this
    ///   sun), times the CPU twin's `rl_clear` there;
    /// - they run along the sun's azimuth within 3° (the line-concentration scan of their places on
    ///   the ground), away from it: they lie in the CPU's parallelograms for this sun, not in those
    ///   of the sun opposite;
    /// - where two or more of the CPU's parallelograms overlap, the picture is darkened once, and
    ///   nowhere is it darker than one layer of shade (the foot shade's 0.42 the darkest);
    /// - the foot shade shows, darker than the shadows close to the walls.
    #[test]
    fn v5_at_a_low_sun_the_shadows_fall_away_from_it_and_darken_once() {
        let Some(gpu) = gpu() else { return };
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let mut shot = |buildings: bool, name: &str| {
            let image = render_with(&gpu, &mut renderer, &realistic(V5, V5_SIZE, LOW_SUN, buildings, false), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            image.save_png(&shots_dir().join(name)).expect("png");
            image
        };
        let with = shot(true, "r5-v5-low-sun.png");
        let bare = shot(false, "r5-v5-low-sun-bare.png");
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());

        let view = ViewFrame::new(&V5, V5_SIZE, 1.0, 0.0);
        let look = frame_look(&realistic(V5, V5_SIZE, LOW_SUN, true, false).sky, &V5, V5_SIZE, LOW_SUN);
        let light = RLight::new(&look, &view, 0.0);
        let strength = shadow_strength(&look);
        let (azimuth, altitude) = sun_at(&V5, LOW_SUN);
        assert_eq!(strength, 0.8);
        assert!((9.0..10.0).contains(&altitude), "{altitude}");
        let single = f64::from(shadow_color(BUILDING_SHADOW_ALPHA * sunlight(altitude.to_radians().sin()), strength)[3]) / 255.0;
        let foot = f64::from(foot_color()[3]) / 255.0;
        let ground = Ground::new(&V5);

        // every blended pixel: its place on the ground, its alpha, the air's `rl_clear` there
        let mut blends = Vec::new();
        for y in 0..V5_SIZE[1] as u32 {
            for x in 0..V5_SIZE[0] as u32 {
                let Some(a) = blended(bare.pixel(x, y), with.pixel(x, y)) else { continue };
                let ll = view.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                let clear = twin::clear(&look, &twin::vary(&light, &view, [ll[0], ll[1], 0.0]));
                blends.push((ground.lng_lat(ll), a, clear));
            }
        }
        let is_single = |a: f64, c: f64| (a - single * c).abs() <= 0.03;
        let is_double = |a: f64, c: f64| (a - (1.0 - (1.0 - single * c).powi(2))).abs() <= 0.03;
        let shadows: Vec<(DVec2, f64, f64)> = blends.iter().copied().filter(|&(_, a, c)| is_single(a, c)).collect();
        let darkest = blends.iter().map(|&(_, a, c)| a - foot * c).fold(f64::NEG_INFINITY, f64::max);
        let footed = blends.iter().filter(|&&(_, a, c)| a > single * c + 0.05).count();
        let (lo, hi) = shadows.iter().fold((1.0f64, 0.0f64), |(lo, hi), &(_, _, c)| (lo.min(c), hi.max(c)));
        let off = shadows.iter().map(|&(_, a, c)| a - single * c).sum::<f64>() / shadows.len() as f64;
        eprintln!(
            "V5 at {altitude:.2}° up, azimuth {azimuth:.2}°: {} px blended once with the shadow colour, {} at the shadows' {single:.4} × rl_clear ({lo:.3}–{hi:.3}, mean off {off:+.4}), {footed} darker (the foot shade), the darkest {darkest:+.4} past the foot shade's",
            blends.len(),
            shadows.len()
        );
        assert!(shadows.len() > 30_000, "{}", shadows.len());
        assert!(off.abs() < 0.01, "{off}");
        assert!(footed > 1_000, "{footed}");
        assert!(darkest <= 0.03, "{darkest}");

        // their direction on the ground: the sun whose shadows the picture's are lies at the sun's
        // azimuth, not opposite it
        let points: Vec<DVec2> = shadows.iter().map(|&(p, _, _)| p).filter(|p| p.x.abs() < 1_000.0 && p.y.abs() < 1_000.0).collect();
        let sample: Vec<DVec2> = points.iter().step_by((points.len() / 1_500).max(1)).copied().collect();
        let tiles = near_tiles(&view);
        let heights = Heights::new(&tiles, &ground);
        let per_metre = shadow_step(local_sun(sun_direction(LOW_SUN), V5.latitude, V5.longitude)).map(|[e, n]| e.hypot(n)).expect("the sun up");
        let (fitted, fit, opposite) = fitted_sun(&heights, &sample, per_metre);
        let error = ((fitted - azimuth + 180.0).rem_euclid(360.0) - 180.0).abs();
        eprintln!(
            "of {} shadow points within 1 km, {fit:.3} lie in the shadows of a sun at {fitted:.2}° ({error:.2}° from the sun's {azimuth:.2}°), {opposite:.3} in those of the sun opposite",
            sample.len()
        );
        assert!(error <= 3.0, "{fitted} against {azimuth}");
        assert!(fit > 0.9 && opposite < 0.5, "{fit} {opposite}");

        // the parallelograms the layer draws for this sun
        let cast = Quads::new(&tiles, &ground, sun_direction(LOW_SUN));
        eprintln!("{} parallelograms", cast.quads.len());

        // where the parallelograms overlap: darkened once
        let (mut once, mut twice) = (0, 0);
        for &(p, a, c) in &blends {
            if cast.count(p) >= 2 {
                once += usize::from(is_single(a, c));
                twice += usize::from(is_double(a, c));
            }
        }
        eprintln!("under two or more parallelograms: {once} px darkened once, {twice} twice");
        assert!(once > 2_000, "{once}");
        assert_eq!(twice, 0);
    }

    /// The same sun over the whole realistic ground (the city ground's tiles, whose passes clip
    /// each tile with the stencil and must leave it at 0, then the buildings): the shadows still
    /// lie on the roads, water and gardens at the tile's alpha times `rl_clear`, darkened once
    /// where parallelograms overlap, nowhere darker than one layer of shade (but for a stray
    /// pixel of a wall's foot whose colour happens to fit a blend).
    #[test]
    fn v5_over_the_city_ground_the_shadows_lie_on_it_once() {
        let Some(gpu) = gpu() else { return };
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let mut shot = |buildings: bool| {
            let image = render_with(&gpu, &mut renderer, &realistic(V5, V5_SIZE, LOW_SUN, buildings, true), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            image
        };
        let (with, bare) = (shot(true), shot(false));
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
        with.save_png(&shots_dir().join("r5-v5-low-sun-ground.png")).expect("png");

        let view = ViewFrame::new(&V5, V5_SIZE, 1.0, 0.0);
        let look = frame_look(&realistic(V5, V5_SIZE, LOW_SUN, true, true).sky, &V5, V5_SIZE, LOW_SUN);
        let light = RLight::new(&look, &view, 0.0);
        let (_, altitude) = sun_at(&V5, LOW_SUN);
        let single = f64::from(shadow_color(BUILDING_SHADOW_ALPHA * sunlight(altitude.to_radians().sin()), shadow_strength(&look))[3]) / 255.0;
        let foot = f64::from(foot_color()[3]) / 255.0;
        let ground = Ground::new(&V5);
        let cast = Quads::new(&near_tiles(&view), &ground, sun_direction(LOW_SUN));
        let (mut shadows, mut blends, mut darker, mut darkest) = (0, 0, 0, f64::NEG_INFINITY);
        let (mut once, mut twice) = (0, 0);
        for y in 0..V5_SIZE[1] as u32 {
            for x in 0..V5_SIZE[0] as u32 {
                let Some(a) = blended(bare.pixel(x, y), with.pixel(x, y)) else { continue };
                let ll = view.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                let c = twin::clear(&look, &twin::vary(&light, &view, [ll[0], ll[1], 0.0]));
                let is_single = (a - single * c).abs() <= 0.03;
                blends += 1;
                shadows += usize::from(is_single);
                darkest = darkest.max(a - foot * c);
                darker += usize::from(a - foot * c > 0.03);
                if cast.count(ground.lng_lat(ll)) >= 2 {
                    once += usize::from(is_single);
                    twice += usize::from((a - (1.0 - (1.0 - single * c).powi(2))).abs() <= 0.03);
                }
            }
        }
        eprintln!(
            "over the city ground: {blends} px blended once with the shadow colour, {shadows} at the shadows' {single:.4} × rl_clear; under two or more parallelograms {once} once, {twice} twice; {darker} darker than the foot shade (the darkest {darkest:+.4} past it)"
        );
        assert!(shadows > 30_000, "{shadows}");
        assert!(once > 2_000, "{once}");
        assert_eq!(twice, 0);
        // (a wall's antialiased foot over a road can pass for a blend now and then)
        assert!(darker * 10_000 < blends, "{darker} of {blends}");
    }

    /// The walls of `tiles`' buildings on the ground, metres about the view's centre: each ring's
    /// edges, by 10 m cell.
    struct Walls {
        /// Each edge's ends and its polygon.
        edges: Vec<(DVec2, DVec2, usize)>,
        /// Each polygon's rings.
        polygons: Vec<Vec<Vec<DVec2>>>,
        cells: HashMap<(i64, i64), Vec<usize>>,
    }

    const WALL_CELL_M: f64 = 10.0;

    impl Walls {
        fn new(tiles: &[(TileIndex, RealisticTile)], ground: &Ground) -> Self {
            let (mut edges, mut polygons) = (Vec::new(), Vec::new());
            for (tile, decoded) in tiles {
                let local = LocalMercator::tile(*tile);
                let f = &decoded.footprints;
                let point = |i: usize| ground.world(local.to_world([f64::from(f.positions[2 * i]), f64::from(f.positions[2 * i + 1])]));
                for w in f.polygon_starts.windows(2) {
                    let rings: Vec<Vec<DVec2>> =
                        f.ring_starts.windows(2).filter(|r| r[0] >= w[0] && r[0] < w[1]).map(|r| (r[0] as usize..r[1] as usize).map(point).collect()).collect();
                    for ring in &rings {
                        edges.extend(ring.windows(2).map(|e| (e[0], e[1], polygons.len())));
                    }
                    polygons.push(rings);
                }
            }
            let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
            for (i, &(a, b, _)) in edges.iter().enumerate() {
                let (lo, hi) = (a.min(b), a.max(b));
                for cy in (lo.y / WALL_CELL_M).floor() as i64..=(hi.y / WALL_CELL_M).floor() as i64 {
                    for cx in (lo.x / WALL_CELL_M).floor() as i64..=(hi.x / WALL_CELL_M).floor() as i64 {
                        cells.entry((cx, cy)).or_default().push(i);
                    }
                }
            }
            Walls { edges, polygons, cells }
        }

        /// The edges within a cell of `p`.
        fn near(&self, p: DVec2) -> impl Iterator<Item = usize> + '_ {
            let (cx, cy) = ((p.x / WALL_CELL_M).floor() as i64, (p.y / WALL_CELL_M).floor() as i64);
            let mut found: Vec<usize> = (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| (cx + dx, cy + dy))).filter_map(|c| self.cells.get(&c)).flatten().copied().collect();
            found.sort_unstable();
            found.dedup();
            found.into_iter()
        }

        /// Whether `p` is inside a footprint near it (even-odd over its polygon's rings).
        fn inside(&self, p: DVec2) -> bool {
            let mut polygons: Vec<usize> = self.near(p).map(|e| self.edges[e].2).collect();
            polygons.dedup();
            polygons.into_iter().any(|k| {
                let mut odd = false;
                for ring in &self.polygons[k] {
                    for e in ring.windows(2) {
                        let (a, b) = (e[0], e[1]);
                        if (a.y <= p.y) != (b.y <= p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                            odd = !odd;
                        }
                    }
                }
                odd
            })
        }

        /// The distance from `p` to the nearest wall, how far along it from its nearer end the
        /// point across from `p` lies, and the distance to the next nearest wall.
        fn nearest(&self, p: DVec2) -> Option<(f64, f64, f64)> {
            let mut best: Vec<(f64, f64)> = self
                .near(p)
                .map(|e| {
                    let (a, b, _) = self.edges[e];
                    let len = a.distance(b);
                    let t = if len > 0.0 { ((p - a).dot(b - a) / (len * len)).clamp(0.0, 1.0) } else { 0.0 };
                    (p.distance(a + (b - a) * t), (t * len).min((1.0 - t) * len))
                })
                .collect();
            best.sort_by(|x, y| x.0.total_cmp(&y.0));
            let &(d, along) = best.first()?;
            Some((d, along, best.get(1).map_or(f64::INFINITY, |x| x.0)))
        }
    }

    /// V5's place close in (zoom 18, a pixel some 0.3 m), at noon: on the ground outside the
    /// buildings, along a wall (away from its ends, no other wall within 3 m, no shadow there),
    /// the foot shade's alpha is `0.42 · (1 − d / 2.5)` at `d` metres from the wall (its line is
    /// the wall, its edge 2.5 m out), times the CPU twin's `rl_clear`; nothing past 2.5 m.
    #[test]
    fn close_in_the_foot_shade_fades_from_the_wall_to_nothing_two_and_a_half_metres_out() {
        let Some(gpu) = gpu() else { return };
        let close = ViewState { zoom: 18.0, ..V5 };
        let size = [1100.0, 720.0];
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let with = render_with(&gpu, &mut renderer, &realistic(close, size, MIDSUMMER, true, false), 1.0).expect("rendered");
        with.save_png(&shots_dir().join("r5-close-noon.png")).expect("png");
        let bare = render_with(&gpu, &mut renderer, &realistic(close, size, MIDSUMMER, false, false), 1.0).expect("rendered");
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());

        let view = ViewFrame::new(&close, size, 1.0, 0.0);
        let look = frame_look(&realistic(close, size, MIDSUMMER, true, false).sky, &close, size, MIDSUMMER);
        let light = RLight::new(&look, &view, 0.0);
        let foot = f64::from(foot_color()[3]) / 255.0;
        let ground = Ground::new(&close);
        let tiles = near_tiles(&view);
        let walls = Walls::new(&tiles, &ground);
        let cast = Quads::new(&tiles, &ground, sun_direction(MIDSUMMER));
        // (a, expected) along walls, and the alphas past the band
        let (mut band, mut past) = (Vec::new(), Vec::new());
        for y in 0..size[1] as u32 {
            for x in 0..size[0] as u32 {
                let ll = view.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                let p = ground.lng_lat(ll);
                let Some((d, along, next)) = walls.nearest(p) else { continue };
                if along < 3.0 || next < 3.0 || d > 3.5 || cast.count(p) > 0 || walls.inside(p) {
                    continue;
                }
                let a = blended(bare.pixel(x, y), with.pixel(x, y));
                let clear = twin::clear(&look, &twin::vary(&light, &view, [ll[0], ll[1], 0.0]));
                if (0.4..2.1).contains(&d) {
                    // (a pixel in the band not blended: a wall in front of it)
                    if let Some(a) = a {
                        band.push((d, a, foot * clear * (1.0 - d / 2.5)));
                    }
                } else if d > 2.8 {
                    past.push(a.unwrap_or(0.0));
                }
            }
        }
        let mut off: Vec<f64> = band.iter().map(|&(_, a, e)| (a - e).abs()).collect();
        off.sort_by(f64::total_cmp);
        let quantile = |q: f64| off[((off.len() - 1) as f64 * q) as usize];
        // the profile in half-metre steps
        let profile: Vec<(f64, f64, f64)> = (0..4)
            .map(|k| {
                let lo = 0.4 + 0.5 * f64::from(k);
                let inside: Vec<&(f64, f64, f64)> = band.iter().filter(|b| b.0 >= lo && b.0 < lo + 0.5).collect();
                let n = inside.len().max(1) as f64;
                (lo + 0.25, inside.iter().map(|b| b.1).sum::<f64>() / n, inside.iter().map(|b| b.2).sum::<f64>() / n)
            })
            .collect();
        eprintln!(
            "{} px along walls: alpha off the profile by {:.4} (median), {:.4} (90 %); by distance (m, alpha, expected) {profile:.3?}; {} px past the band, {} of them shaded",
            band.len(),
            quantile(0.5),
            quantile(0.9),
            past.len(),
            past.iter().filter(|&&a| a > 0.0).count()
        );
        assert!(band.len() > 2_000, "{}", band.len());
        assert!(quantile(0.5) < 0.015 && quantile(0.9) < 0.04, "{} {}", quantile(0.5), quantile(0.9));
        assert!(past.len() > 2_000 && past.iter().filter(|&&a| a > 0.0).count() * 100 < past.len(), "{}", past.len());
    }

    /// With *daylight* off the shadows are the fixed afternoon sun's (south-west, 35° up) whatever
    /// the clock: V5's place at the low morning sun's time, looked at straight down (zoom 16, so
    /// the shadows behind the buildings show and every tile is in the fixtures), over the land
    /// under the sky alone; the pixels at the shadows' alpha are best held by the shadows of a
    /// sun within 3° of 215°.
    #[test]
    fn with_daylight_off_the_shadows_are_the_fixed_afternoon_sun_s() {
        let Some(gpu) = gpu() else { return };
        let facing = ViewState { zoom: 16.0, pitch: 0.0, bearing: 0.0, ..V5 };
        let size = [800.0, 600.0];
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let input = |buildings: bool| {
            let mut scene = realistic(facing, size, LOW_SUN, buildings, false);
            scene.sky.daylight = false;
            scene
        };
        let mut shot = |buildings: bool| {
            let image = render_with(&gpu, &mut renderer, &input(buildings), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            image
        };
        let (with, bare) = (shot(true), shot(false));
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
        with.save_png(&shots_dir().join("r5-v5-afternoon-sun.png")).expect("png");
        let view = ViewFrame::new(&facing, size, 1.0, 0.0);
        let look = frame_look(&input(true).sky, &facing, size, LOW_SUN);
        let light = RLight::new(&look, &view, 0.0);
        let single = f64::from(shadow_color(BUILDING_SHADOW_ALPHA, shadow_strength(&look))[3]) / 255.0;
        let ground = Ground::new(&facing);
        let mut points = Vec::new();
        for y in 0..size[1] as u32 {
            for x in 0..size[0] as u32 {
                let Some(a) = blended(bare.pixel(x, y), with.pixel(x, y)) else { continue };
                let ll = view.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                let clear = twin::clear(&look, &twin::vary(&light, &view, [ll[0], ll[1], 0.0]));
                let p = ground.lng_lat(ll);
                if (a - single * clear).abs() <= 0.03 && p.x.abs() < 1_000.0 && p.y.abs() < 1_000.0 {
                    points.push(p);
                }
            }
        }
        let sample: Vec<DVec2> = points.iter().step_by((points.len() / 1_500).max(1)).copied().collect();
        let heights = Heights::new(&near_tiles(&view), &ground);
        let per_metre = 1.0 / 35f64.to_radians().tan();
        let (fitted, fit, opposite) = fitted_sun(&heights, &sample, per_metre);
        eprintln!(
            "daylight off: strength {}, of {} points at the shadows' {single:.4} {fit:.3} in the shadows of a sun at {fitted:.2}°, {opposite:.3} opposite",
            shadow_strength(&look),
            sample.len()
        );
        assert!(sample.len() > 1_000, "{}", sample.len());
        assert!(((fitted - 215.0 + 180.0).rem_euclid(360.0) - 180.0).abs() <= 3.0, "{fitted}");
        assert!(fit > 0.9 && opposite < 0.5, "{fit} {opposite}");
    }

    /// When the sun has moved a quarter of a degree the tiles on screen are recast (within the
    /// frame's budget, the layer pending until all are): half an hour on, V5 is exactly what a
    /// renderer that cast them at that time draws, and its shadows have moved. At night none is
    /// drawn (the strength 0, the sun down), the foot shade still is.
    #[test]
    fn shadows_are_recast_when_the_sun_moves_and_none_drawn_at_night() {
        let Some(gpu) = gpu() else { return };
        let later = LOW_SUN + 1_800_000.0;
        let shots = |times: &[f64]| {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(Arc::new(DirTiles::fixtures()), None);
            let mut image = None;
            for &now in times {
                image = Some(render_with(&gpu, &mut renderer, &realistic(V5, V5_SIZE, now, true, false), 1.0).expect("rendered"));
                assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            }
            image.expect("a time")
        };
        let before = shots(&[LOW_SUN]);
        let recast = shots(&[LOW_SUN, later]);
        let fresh = shots(&[later]);
        recast.save_png(&shots_dir().join("r5-v5-recast.png")).expect("png");
        let differ = |a: &crate::headless::Image, b: &crate::headless::Image| a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).filter(|(p, q)| p != q).count();
        eprintln!("recast: {} px differ from the fresh cast, {} from half an hour before", differ(&recast, &fresh), differ(&recast, &before));
        assert_eq!(differ(&recast, &fresh), 0);
        assert!(differ(&recast, &before) > 10_000);
        // at night: what the buildings change beyond themselves is the foot shade alone
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(Arc::new(DirTiles::fixtures()), None);
        let input = realistic(V5, V5_SIZE, MIDWINTER_NIGHT, true, false);
        let look = frame_look(&input.sky, &V5, V5_SIZE, MIDWINTER_NIGHT);
        assert_eq!(shadow_strength(&look), 0.0);
        let tile = TileIndex::new(36057, 19267, 16);
        let bytes = DirTiles::fixtures().read(io::STREETS_GL, tile).expect("read").expect("a V5 tile");
        let decoded = decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("buildings");
        assert!(tess::tile_shadows(tile, &decoded.footprints, sun_direction(MIDWINTER_NIGHT)).is_none());
        assert!(!foot_shade(tile, &decoded.footprints).is_empty());
        render_with(&gpu, &mut renderer, &input, 1.0).expect("rendered").save_png(&shots_dir().join("r5-v5-night.png")).expect("png");
    }
}
