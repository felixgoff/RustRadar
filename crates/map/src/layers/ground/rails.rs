//! `ground-rails-*` and `ground-platforms` (item R9; doc 02 §8.6; doc 04 §6.9; `realistic-ground.ts`'s
//! `railGroundLayers`, `RG:865-924`, G:2663): on the realistic map, the railways the trains run on
//! (OpenRailwayMap's tracks on their ballast) as part of the ground, and close in on the flat map
//! the stations' platforms standing up beside them. The tiles are made on the worker pool by
//! `scene::ground_rails` (item T9); this layer fetches, uploads and draws them with the ground
//! shader.
//!
//! What today's app does, and so this layer:
//! - **When.** In the app (`OWN_RAILS = isTauri()`: always in the native app) on the realistic
//!   basemap while the scene shows the ground ([`super::Input`]), pushed right after the ground's
//!   tiles and before the relief (G:2663): [`crate::layers::ordered`] puts it after
//!   [`super::Ground`]. While it draws, the ground's own railways are left out but for trams' rails
//!   in the street (`scene::ground::OWN_RAILS`).
//! - **`ground-rails-flat` / `ground-rails-globe`**: the backend's rail tiles (the rail map's
//!   source, [`RAIL_SOURCE`]: `backend::rail::tile` through the app's fetcher; layer
//!   `railway_line_high`) by deck's own rule at tile zooms 7 to 12 ([`RAIL_TILESET`]: 512 px,
//!   `SPEED_TILE_ZOOMS`, over-zoomed past 12), one MVTLayer and cache per projection: the one the
//!   view is not in leaves deck's list, its tiles with it. deck's TileLayer defaults: 6 requests at
//!   a time, best-available refinement. Each tile's passes (`GroundTile.renderLayers`): the
//!   tracks' ballast (`-edges`), the tracks (`-lines`), and on the flat map its bridges
//!   (`-bridge-shade` in `BRIDGE_SHADE`, unlit; `-bridge-edges` and `-bridge-lines` raised to the
//!   rail map's 6 m decks as today's app raises them, [`bridge_lift`]: `bridgeParts` writes the
//!   heights in deck's common units and the `CARTESIAN` tiles read them as metres, so a deck
//!   stands 0.15 mm up and no ramp shows, item T7's finding; `-bridge-decks`, none wide on a
//!   railway, depth written). Widths in
//!   metres, at least 0.6 px ([`RAILS_MIN_PIXELS`]), round joints and caps; `SURFACE_NO_CULL` (no
//!   depth written but by the decks); on the flat map clipped to the tile's square (MVTLayer's
//!   `ClipExtension`), with the stencil as the city ground does.
//! - **`ground-platforms`** (the flat map only): the same backend's `standard_railway_platforms`
//!   at tile zoom 15 alone ([`PLATFORM_TILES`]), by distance (`LodMVTLayer` with `PLATFORM_LOD`:
//!   zoom 15 within 4 km of the camera, [`platform_lod`]; no tile while the view is under the
//!   layer's `minZoom` 15; `keepCoarserUntilIn`). Each platform's polygon stands 0.6 m tall, its
//!   top and sides in `AREAS.platform` (paint 9), unlit by deck (`material: false`): the ground
//!   shader lights a side by its facing (`rg_wall`, here `vs_platform`'s level normal) and the
//!   top as any area. A platform mapped as a line lies flat as `PATHS.pedestrian` (5 m, no
//!   minimum width). Drawn in `solid`'s parameters (`NO_CULL`: depth written, so it hides what
//!   is behind it), clipped to the tile's square.
//!
//! **Paints.** `scene::ground_rails` hands the ground's paints as deck carries them (RGB a tint,
//! alpha 255 less the surface id); the decode step turns the ground's passes into the shader's
//! explicit attributes (`tess::ground::explicit`, `[r, g, b, id]`) and leaves the shade and the
//! decks plain RGBA, as the city ground has them.
//!
//! How it is checked (the tests below): the tiles, zooms, widths and states are today's; a
//! fixture tile's tracks lie on their ballast in the ground's paints; and at Stockholm C from the
//! fixtures (the plan's station view) the tracks and their ballast are drawn where the CPU puts
//! OpenRailwayMap's lines, and the platforms stand where the CPU puts their outlines, lighter
//! than the tracks beside them; only the realistic map asks for the tiles, the platforms only on
//! the flat map from zoom 15.
//!
//! Deviations, invisible: as the city ground's (the stencil clips where the tile's square is
//! seen at the ground, so a platform's top 0.6 m up is cut where the square's edge is seen, not
//! where the top crosses it; both tiles hold the platform, so it shows once and whole).

use bytemuck::{Pod, Zeroable};
use rustradar_core::geo::Basemap;
use rustradar_scene::ground_rails::{self as scene, GroundLines, PlatformsTile};
use rustradar_scene::tess::ground::explicit;

pub use rustradar_scene::ground_rails::{PLATFORM_TILES, PLATFORMS_MIN_PIXELS, RAIL_TILES as RAIL_TILESET, RAILS_MIN_PIXELS, platform_lod};

use super::city::{DECKS, plain_style};
use super::{GroundShader, LineMesh, LineWidth, load_textures};
use crate::frame::FrameSlot;
use crate::jobs::Cancel;
use crate::layers::vector_map::{CLIP_MARK, CLIP_RESET, CLIPPED, MAX_REQUESTS};
use crate::layers::{ByteSize, DeckSelect, DrawCx, Init, Layer, LodSelect, Pass, PrepareCx, Refinement, Source, TileIndex, TileSet, TileSetOptions};
use crate::pipelines::fill;
use crate::pipelines::path::{self, Path, Segment as PathSegment};
use crate::state::DrawState;

/// The tiles' source: the backend's rail tiles, the rail map's (`railTilesUrl()`).
pub const RAIL_SOURCE: &str = crate::layers::rail::RAIL_TILES;

/// The platforms' `solid` parameters inside the tile's square: unculled, depth written.
pub const SOLID: DrawState = DECKS;

/// The rails' lines: hairlines far out (`lineWidthMinPixels: 0.6`).
pub const RAILS_WIDTH: LineWidth = LineWidth::Hairline;
/// The platforms' lines: as wide as they are (no minimum).
pub const PLATFORMS_WIDTH: LineWidth = LineWidth::Exact;

// ------------------------------------------------------------------ decoding

/// A rail tile's passes, the ground's in explicit paints (for the cache's budget).
pub struct Rails(pub GroundLines);

impl ByteSize for Rails {
    fn byte_size(&self) -> usize {
        self.0.byte_size()
    }
}

/// One vertex of the platforms' mesh as `vs_platform` reads it: `(u, v, h)` in the tile's frame,
/// the paint as the ground shader's explicit attributes (tint and surface id), the normal east,
/// north, up (level on a side, up on the top). 28 bytes, `#[repr(C)]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct PlatformVertex {
    pub position: [f32; 3],
    pub paint: [u8; 4],
    pub normal: [f32; 3],
}

/// [`PlatformVertex`]: position at location 0, paint (`Uint8x4`) at 1, normal at 2.
pub const PLATFORM_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<PlatformVertex>() as u64,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint8x4, 2 => Float32x3],
};

/// A platform tile, decoded: the standing platforms and the lines.
pub struct Platforms {
    pub vertices: Vec<PlatformVertex>,
    pub indices: Vec<u32>,
    pub lines: GroundLines,
}

impl ByteSize for Platforms {
    fn byte_size(&self) -> usize {
        size_of_val(self.vertices.as_slice()) + size_of_val(self.indices.as_slice()) + self.lines.byte_size()
    }
}

/// The ground's passes of `lines` in the shader's explicit paints; the shade and the decks stay
/// plain colours.
pub fn explicit_lines(lines: GroundLines) -> GroundLines {
    let paint = |segments: Vec<PathSegment>| segments.into_iter().map(|s| PathSegment { color: explicit(s.color), ..s }).collect();
    GroundLines {
        edges: paint(lines.edges),
        lines: paint(lines.lines),
        bridge_edges: paint(lines.bridge_edges),
        bridge_lines: paint(lines.bridge_lines),
        ..lines
    }
}

/// A platform tile in the shader's attributes.
pub fn explicit_platforms(tile: PlatformsTile) -> Platforms {
    let vertices = tile.vertices.iter().map(|v| PlatformVertex { position: v.position, paint: explicit(v.paint), normal: v.normal }).collect();
    Platforms { vertices, indices: tile.indices, lines: explicit_lines(tile.lines) }
}

/// How high today's app draws a tile's raised bridges for each metre of deck: `bridgeParts`
/// writes the heights in deck's common units (`× 512 / (circumference · cos φ)` at the tile's
/// middle) and the flat map's `CARTESIAN` tiles read `z` as metres, so a 6 m deck stands 0.15 mm
/// up at Stockholm (`scene::bridges::rail_map_lift`, item T7's finding for the rail map, the same
/// function here).
pub fn bridge_lift(tile: TileIndex) -> f64 {
    rustradar_scene::bridges::rail_map_lift(rustradar_scene::bridges::TileIndex { x: tile.x, y: tile.y, z: tile.z })
}

/// `lines`' raised passes at today's height ([`bridge_lift`] of their decks).
pub fn lifted(lines: GroundLines, lift: f64) -> GroundLines {
    let lift = |segments: Vec<PathSegment>| {
        segments
            .into_iter()
            .map(|s| {
                let up = |p: [f32; 3]| [p[0], p[1], (f64::from(p[2]) * lift) as f32];
                PathSegment { left: up(s.left), start: up(s.start), end: up(s.end), right: up(s.right), ..s }
            })
            .collect()
    };
    GroundLines { bridge_edges: lift(lines.bridge_edges), bridge_lines: lift(lines.bridge_lines), bridge_decks: lift(lines.bridge_decks), ..lines }
}

/// The rails' decode step, on the worker pool: the tile the way of the flat map (`flat`, its
/// bridges at today's height) or the globe (`None` for a tile with nothing to draw).
pub fn decode_rails(tile: TileIndex, bytes: &[u8], flat: bool, cancel: &Cancel) -> Result<Option<Rails>, String> {
    match scene::rails_tile(tile, bytes, flat, &|| cancel.is_cancelled()) {
        Ok(lines) if lines.is_empty() => Ok(None),
        Ok(lines) => Ok(Some(Rails(lifted(explicit_lines(lines), bridge_lift(tile))))),
        Err(scene::Cancelled) => Err("cancelled".into()),
    }
}

/// The platforms' decode step, on the worker pool.
pub fn decode_platforms(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<Platforms>, String> {
    match scene::platforms_tile(tile, bytes, &|| cancel.is_cancelled()) {
        Ok(decoded) if decoded.is_empty() => Ok(None),
        Ok(decoded) => {
            let mut platforms = explicit_platforms(decoded);
            platforms.lines = lifted(platforms.lines, bridge_lift(tile));
            Ok(Some(platforms))
        }
        Err(scene::Cancelled) => Err("cancelled".into()),
    }
}

// ------------------------------------------------------------------ on the GPU

/// A tile's line passes on the GPU.
pub struct LineMeshes {
    pub edges: LineMesh,
    pub lines: LineMesh,
    /// Plain RGBA segments (the plain path pipeline).
    pub bridge_shade: path::Mesh,
    pub bridge_edges: LineMesh,
    pub bridge_lines: LineMesh,
    /// Plain RGBA segments, alpha 0, for the depth buffer.
    pub bridge_decks: path::Mesh,
}

impl LineMeshes {
    pub fn new(device: &wgpu::Device, label: &str, l: &GroundLines) -> Self {
        LineMeshes {
            edges: LineMesh::new(device, label, &l.edges),
            lines: LineMesh::new(device, label, &l.lines),
            bridge_shade: path::Mesh::new(device, label, &l.bridge_shade),
            bridge_edges: LineMesh::new(device, label, &l.bridge_edges),
            bridge_lines: LineMesh::new(device, label, &l.bridge_lines),
            bridge_decks: path::Mesh::new(device, label, &l.bridge_decks),
        }
    }
}

/// A platform tile on the GPU.
pub struct PlatformMeshes {
    pub platforms: fill::Mesh<PlatformVertex>,
    pub lines: LineMeshes,
}

/// What one set of line passes is drawn with: the ground shader's lines pipeline, the plain
/// paths' for the shade and the decks with their styles, and the lines' minimum width.
struct LinePipelines<'a> {
    lines: &'a wgpu::RenderPipeline,
    shade: (&'a Path, &'a path::Style),
    decks: (&'a Path, &'a path::Style),
    width: LineWidth,
}

/// A tile's six line passes in its frame (`GroundTile.renderLayers`' order).
fn draw_passes(pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, shader: &GroundShader, slot: FrameSlot, m: &LineMeshes, p: &LinePipelines<'_>) {
    if !m.edges.is_empty() || !m.lines.is_empty() {
        shader.begin(pass, p.lines, p.width);
        shader.draw_lines(pass, cx, slot, &m.edges);
        shader.draw_lines(pass, cx, slot, &m.lines);
    }
    if !m.bridge_shade.is_empty() {
        p.shade.0.draw(pass, cx, slot, p.shade.1, &m.bridge_shade);
    }
    if !m.bridge_edges.is_empty() || !m.bridge_lines.is_empty() {
        shader.begin(pass, p.lines, p.width);
        shader.draw_lines(pass, cx, slot, &m.bridge_edges);
        shader.draw_lines(pass, cx, slot, &m.bridge_lines);
    }
    if !m.bridge_decks.is_empty() {
        p.decks.0.draw(pass, cx, slot, p.decks.1, &m.bridge_decks);
    }
}

/// `[Hairline, Exact]`'s index.
fn width_index(width: LineWidth) -> usize {
    match width {
        LineWidth::Hairline => 0,
        LineWidth::Exact => 1,
    }
}

/// The tile's square, `[0, 1]²` in tile units, and the stencil passes that clip a tile to it (as
/// the city ground's).
struct Clip {
    square: fill::Mesh<fill::Vertex>,
    mark: fill::Fill,
    mark_style: fill::Style,
    reset: fill::Fill,
    reset_style: fill::Style,
}

impl Clip {
    fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| fill::Vertex { position: [u, v, 0.0], color: [0; 4] });
        let square = fill::Mesh::new(device, "ground rails clip", &corners, &[0, 1, 2, 0, 2, 3]);
        let mark = fill::Fill::new(init, "ground rails clip", CLIP_MARK);
        let reset = fill::Fill::new(init, "ground rails clip reset", CLIP_RESET);
        let mark_style = mark.style(device, fill::OPAQUE);
        let reset_style = reset.style(device, fill::OPAQUE);
        Clip { square, mark, mark_style, reset, reset_style }
    }

    fn mark(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, slot: FrameSlot) {
        self.mark.draw(pass, cx, slot, &self.mark_style, &self.square);
    }

    fn reset(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, slot: FrameSlot) {
        self.reset.draw(pass, cx, slot, &self.reset_style, &self.square);
    }
}

// ------------------------------------------------------------------ the layer

/// Which of the layers deck holds this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// `ground-rails-globe`.
    Globe,
    /// `ground-rails-flat` and `ground-platforms`.
    Flat,
}

/// Row 3f of doc 04 §1 (`railGroundLayers`): the ground rails on either projection and the
/// platforms on the flat map.
pub struct GroundRails {
    shader: GroundShader,
    /// The tracks on the flat map (clipped) and on the globe (not).
    flat_lines: wgpu::RenderPipeline,
    globe_lines: wgpu::RenderPipeline,
    /// The plain path pipeline for the rails' bridges' shade (clipped) and for the decks and the
    /// platforms' shade ([`DECKS`], [`SOLID`]: the same state), each with [`plain_style`] for
    /// both [`LineWidth`]s.
    shade: Path,
    shade_styles: [path::Style; 2],
    decks: Path,
    deck_styles: [path::Style; 2],
    /// The platforms standing and their lines, in [`SOLID`].
    platform_areas: wgpu::RenderPipeline,
    platform_lines: wgpu::RenderPipeline,
    clip: Clip,
    flat: TileSet<Rails, LineMeshes>,
    globe: TileSet<Rails, LineMeshes>,
    platforms: TileSet<Platforms, PlatformMeshes>,
    shown: Option<Shown>,
    /// This frame's rail and platform tiles in draw order, each with its frame.
    rails_drawn: Vec<(FrameSlot, TileIndex)>,
    platforms_drawn: Vec<(FrameSlot, TileIndex)>,
}

impl GroundRails {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let shader = GroundShader::new(init, "ground-rails");
        let rails = |flat: bool| -> TileSet<Rails, LineMeshes> {
            init.tile_set(
                vec![Source::new(RAIL_SOURCE)],
                DeckSelect(RAIL_TILESET),
                TileSetOptions { max_requests: MAX_REQUESTS, ..Default::default() },
                move |tile, bytes, cancel| decode_rails(tile, &bytes, flat, cancel),
            )
        };
        let mut select = LodSelect::new(platform_lod(), PLATFORM_TILES.tile_size);
        select.min_zoom = PLATFORM_TILES.min_zoom;
        let platforms = init.tile_set(
            vec![Source::new(RAIL_SOURCE)],
            select,
            TileSetOptions { max_requests: MAX_REQUESTS, refinement: Refinement::KeepCoarserUntilIn, ..Default::default() },
            |tile, bytes, cancel| decode_platforms(tile, &bytes, cancel),
        );
        let shade = Path::new(init, "ground rails bridge shade", CLIPPED);
        let decks = Path::new(init, "ground rails bridge decks", DECKS);
        let styles = |p: &Path| [LineWidth::Hairline, LineWidth::Exact].map(|w| p.style(device, &plain_style(w)));
        GroundRails {
            flat_lines: shader.lines(init, "ground-rails-flat", CLIPPED),
            globe_lines: shader.lines(init, "ground-rails-globe", DrawState::SURFACE_NO_CULL),
            shade_styles: styles(&shade),
            deck_styles: styles(&decks),
            shade,
            decks,
            platform_areas: shader.platforms(init, "ground-platforms", SOLID),
            platform_lines: shader.lines(init, "ground-platforms lines", SOLID),
            shader,
            clip: Clip::new(init),
            flat: rails(true),
            globe: rails(false),
            platforms,
            shown: None,
            rails_drawn: Vec::new(),
            platforms_drawn: Vec::new(),
        }
    }

    /// Which layers the last frame drew.
    pub fn shown(&self) -> Option<Shown> {
        self.shown
    }

    /// `ground-rails-flat`'s tiles (tests).
    pub fn flat_tiles(&self) -> &TileSet<Rails, LineMeshes> {
        &self.flat
    }

    /// `ground-rails-globe`'s tiles (tests).
    pub fn globe_tiles(&self) -> &TileSet<Rails, LineMeshes> {
        &self.globe
    }

    /// `ground-platforms`' tiles (tests).
    pub fn platform_tiles(&self) -> &TileSet<Platforms, PlatformMeshes> {
        &self.platforms
    }
}

/// Drops a tile set's tiles (deck drops a layer that leaves its list, and its tiles with it).
fn drop_tiles<D: ByteSize + Send + 'static, T>(set: &mut TileSet<D, T>) {
    if !set.is_empty() {
        set.clear();
    }
}

impl Layer for GroundRails {
    fn name(&self) -> &'static str {
        "ground-rails"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.rails_drawn.clear();
        self.platforms_drawn.clear();
        let realistic = cx.scene.ground.shown && cx.scene.basemap == Basemap::Realistic;
        self.shown = realistic.then(|| if cx.view.is_globe() { Shown::Globe } else { Shown::Flat });
        match self.shown {
            None => {
                drop_tiles(&mut self.flat);
                drop_tiles(&mut self.globe);
                drop_tiles(&mut self.platforms);
                return;
            }
            Some(Shown::Globe) => {
                drop_tiles(&mut self.flat);
                drop_tiles(&mut self.platforms);
            }
            Some(Shown::Flat) => drop_tiles(&mut self.globe),
        }
        load_textures(cx);
        self.shader.prepare(cx);
        let flat = self.shown == Some(Shown::Flat);
        let rails = if flat { &mut self.flat } else { &mut self.globe };
        cx.update_tiles(rails, |gpu, _tile, decoded| LineMeshes::new(&gpu.device, "ground rails", &decoded.0));
        for (tile, _) in rails.visible() {
            self.rails_drawn.push((cx.tile_frame(tile, 0.0), tile));
        }
        if flat {
            cx.update_tiles(&mut self.platforms, |gpu, _tile, decoded| PlatformMeshes {
                platforms: fill::Mesh::new(&gpu.device, "ground-platforms", &decoded.vertices, &decoded.indices),
                lines: LineMeshes::new(&gpu.device, "ground-platforms lines", &decoded.lines),
            });
            for (tile, _) in self.platforms.visible() {
                self.platforms_drawn.push((cx.tile_frame(tile, 0.0), tile));
            }
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        let Some(shown) = self.shown else { return };
        if cx.pass != Pass::Map {
            return;
        }
        let shader = &self.shader;
        let rails_width = width_index(RAILS_WIDTH);
        match shown {
            Shown::Globe => {
                // (GeoJSON on the globe: nothing raised, nothing clipped)
                let p = LinePipelines {
                    lines: &self.globe_lines,
                    shade: (&self.shade, &self.shade_styles[rails_width]),
                    decks: (&self.decks, &self.deck_styles[rails_width]),
                    width: RAILS_WIDTH,
                };
                for &(slot, tile) in &self.rails_drawn {
                    if let Some(m) = self.globe.get(tile) {
                        draw_passes(pass, cx, shader, slot, m, &p);
                    }
                }
            }
            Shown::Flat => {
                let p = LinePipelines {
                    lines: &self.flat_lines,
                    shade: (&self.shade, &self.shade_styles[rails_width]),
                    decks: (&self.decks, &self.deck_styles[rails_width]),
                    width: RAILS_WIDTH,
                };
                for &(slot, tile) in &self.rails_drawn {
                    let Some(m) = self.flat.get(tile) else { continue };
                    self.clip.mark(pass, cx, slot);
                    draw_passes(pass, cx, shader, slot, m, &p);
                    self.clip.reset(pass, cx, slot);
                }
                // `ground-platforms`: its areas standing, then its lines, all in `solid`'s parameters
                let platforms_width = width_index(PLATFORMS_WIDTH);
                let p = LinePipelines {
                    lines: &self.platform_lines,
                    shade: (&self.decks, &self.deck_styles[platforms_width]),
                    decks: (&self.decks, &self.deck_styles[platforms_width]),
                    width: PLATFORMS_WIDTH,
                };
                for &(slot, tile) in &self.platforms_drawn {
                    let Some(m) = self.platforms.get(tile) else { continue };
                    self.clip.mark(pass, cx, slot);
                    if m.platforms.index_count() > 0 {
                        shader.begin(pass, &self.platform_areas, PLATFORMS_WIDTH);
                        cx.bind_frame(pass, slot);
                        m.platforms.draw(pass);
                    }
                    draw_passes(pass, cx, shader, slot, &m.lines, &p);
                    self.clip.reset(pass, cx, slot);
                }
            }
        }
    }

    fn drape(&self) -> crate::layers::Drape {
        crate::layers::Drape::Cover
    }

    fn pending(&self) -> bool {
        match self.shown {
            None => false,
            Some(Shown::Globe) => self.globe.loading(),
            Some(Shown::Flat) => self.flat.loading() || self.platforms.loading(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use glam::DVec2;
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::geom::path::LocalMercator;
    use rustradar_scene::ground::PLATFORM_M;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchResult, FnFetcher, TileFetcher};
    use crate::layers::{earth, ground, hillshade, sky};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// The plan's station view (`CLOSE_UP_VIEWS`): Stockholm C at zoom 16, pitched 45°.
    const STOCKHOLM_C: ViewState = ViewState { latitude: 59.3311, longitude: 18.0573, zoom: 16.0, pitch: 45.0, bearing: 0.0, position: None };
    /// The fixtures' close-up size.
    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// The z12 rail tile and the z15 platform tile under the station.
    const RAILS_AT_C: TileIndex = TileIndex::new(2253, 1204, 12);
    const PLATFORMS_AT_C: TileIndex = TileIndex::new(18027, 9636, 15);

    fn shots_dir() -> PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    fn fixture(source: &str, tile: TileIndex) -> Vec<u8> {
        match DirTiles::fixtures().read(source, tile) {
            Ok(Some(bytes)) => bytes,
            other => panic!("{source} {tile:?}: {other:?}"),
        }
    }

    /// The surface id an explicit paint carries.
    fn id(color: [u8; 4]) -> u8 {
        color[3]
    }

    #[test]
    fn the_layers_are_today_s() {
        // the rail map's tiles, zooms 7 to 12, 512 px; the platforms' at 15 alone within 4 km
        assert_eq!(RAIL_SOURCE, "openrailwaymap");
        assert_eq!((RAIL_TILESET.min_zoom, RAIL_TILESET.max_zoom, RAIL_TILESET.tile_size), (Some(7.0), Some(12.0), 512.0));
        assert_eq!((PLATFORM_TILES.min_zoom, PLATFORM_TILES.max_zoom, PLATFORM_TILES.tile_size), (Some(15.0), Some(15.0), 512.0));
        let lod = platform_lod();
        assert_eq!((lod.levels.as_slice(), lod.max_distance), (&[15u32][..], 4000.0));
        // hairlines far out on the rails, none on the platforms
        assert_eq!((RAILS_WIDTH.min_pixels(), PLATFORMS_WIDTH.min_pixels()), (RAILS_MIN_PIXELS, PLATFORMS_MIN_PIXELS));
        assert_eq!((RAILS_MIN_PIXELS, PLATFORMS_MIN_PIXELS), (0.6, 0.0));
        // the platforms in `solid`'s parameters: unculled, depth written, tested; clipped
        assert!(SOLID.cull.is_none() && SOLID.depth_write && SOLID.depth_compare == wgpu::CompareFunction::LessEqual);
        assert_eq!(SOLID.stencil, CLIPPED.stencil);
        // the rails as the rest of the ground: no depth written, unculled
        assert!(CLIPPED.cull.is_none() && !CLIPPED.depth_write);
        assert!(DrawState::SURFACE_NO_CULL.cull.is_none() && !DrawState::SURFACE_NO_CULL.depth_write);
    }

    /// Stockholm C's z12 tile: every track on its ballast (`ROADS.rail`, 2.6 m of paint 35 on 1 m
    /// a side of paint 38; light rail as `ROADS.transit`, 2.2 m on 0.6 m), in the ground's
    /// explicit paints, the same on both projections but for the bridges, raised on the flat map
    /// only.
    #[test]
    fn a_fixture_tile_s_tracks_lie_on_their_ballast_in_the_ground_s_paints() {
        let bytes = fixture(RAIL_SOURCE, RAILS_AT_C);
        let never = Cancel::new();
        let flat = decode_rails(RAILS_AT_C, &bytes, true, &never).unwrap().expect("tracks").0;
        let globe = decode_rails(RAILS_AT_C, &bytes, false, &never).unwrap().expect("tracks").0;
        for lines in [&flat, &globe] {
            assert!(lines.lines.len() > 500, "{}", lines.lines.len());
            assert!(lines.lines.iter().all(|s| id(s.color) == 35 && s.color[..3] == [255; 3]), "tracks");
            assert!(lines.edges.iter().all(|s| id(s.color) == 38 && s.color[..3] == [255; 3]), "ballast");
            assert!(lines.lines.iter().all(|s| s.width == 2.6 || s.width == 2.2));
            assert!(lines.edges.iter().all(|s| s.width == 4.6 || s.width == 3.4));
        }
        // every track's ballast (no railway here without an edge)
        assert_eq!(globe.lines.len(), globe.edges.len());
        assert!(globe.bridge_lines.is_empty() && globe.bridge_shade.is_empty());
        // Stockholm's bridges split off on the flat map: their ballast and tracks, shade and (none
        // wide) decks, the shade a plain colour; raised as today's are, their 6 m decks a 6 m
        // deck's common units up (0.15 mm)
        assert!(!flat.bridge_lines.is_empty(), "the tile's bridges");
        assert!(flat.bridge_lines.iter().all(|s| id(s.color) == 35));
        let lift = bridge_lift(RAILS_AT_C);
        assert!((lift * 6.0 - 1.5e-4).abs() < 1e-5, "{lift}");
        let top = flat.bridge_lines.iter().map(|s| f64::from(s.start[2])).fold(0.0, f64::max);
        assert!((top - 6.0 * lift).abs() < 1e-9, "{top}");
        assert!(flat.bridge_lines.iter().filter(|s| f64::from(s.start[2]) > 5.0 * lift).count() * 2 > flat.bridge_lines.len(), "up on their decks");
        assert!(flat.bridge_shade.iter().all(|s| s.color == scene::BRIDGE_SHADE));
        assert!(flat.bridge_decks.iter().all(|s| s.color == scene::NONE && s.width == 0.0));
        assert!(flat.lines.len() + flat.bridge_lines.len() >= globe.lines.len());
    }

    /// Stockholm C's platform tile: each platform stands 0.6 m tall in paint 9, its top lit as
    /// the ground is (normal up), its sides level and facing out of the outline.
    #[test]
    fn a_fixture_tile_s_platforms_stand_0_6_m_with_their_sides_facing_out() {
        let bytes = fixture(RAIL_SOURCE, PLATFORMS_AT_C);
        let platforms = decode_platforms(PLATFORMS_AT_C, &bytes, &Cancel::new()).unwrap().expect("platforms");
        assert!(platforms.indices.len() > 300, "{}", platforms.indices.len());
        assert!(platforms.vertices.iter().all(|v| v.paint == [255, 255, 255, 9]));
        let (mut tops, mut sides) = (0, 0);
        for t in platforms.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| platforms.vertices[i as usize]);
            if a.normal == [0.0, 0.0, 1.0] {
                tops += 1;
                assert!([a, b, c].iter().all(|v| v.position[2] == PLATFORM_M as f32 && v.normal == a.normal));
            } else {
                sides += 1;
                let n = DVec2::new(f64::from(a.normal[0]), f64::from(a.normal[1]));
                assert!((n.length() - 1.0).abs() < 1e-5 && a.normal[2] == 0.0, "{:?}", a.normal);
                assert!([a, b, c].iter().all(|v| v.normal == a.normal && (v.position[2] == 0.0 || v.position[2] == PLATFORM_M as f32)));
            }
        }
        assert!(tops > 20 && sides > 40, "{tops} tops, {sides} side triangles");
        // facing out: from a side's middle a step along its normal leaves its platform's top (the
        // triangles of the same platform's top: some platforms are mapped twice over), a step back
        // enters it. A platform's vertices: its sides' quads, then its top's.
        let mut polygon = vec![0usize; platforms.vertices.len()];
        for i in 1..platforms.vertices.len() {
            let (was_top, side) = (platforms.vertices[i - 1].normal[2] == 1.0, platforms.vertices[i].normal[2] == 0.0);
            polygon[i] = polygon[i - 1] + usize::from(was_top && side);
        }
        let at = |i: u32| {
            let v = platforms.vertices[i as usize];
            DVec2::new(f64::from(v.position[0]), f64::from(v.position[1]))
        };
        let tops: Vec<(usize, [DVec2; 3])> = platforms
            .indices
            .chunks_exact(3)
            .filter(|t| platforms.vertices[t[0] as usize].normal[2] == 1.0)
            .map(|t| (polygon[t[0] as usize], [at(t[0]), at(t[1]), at(t[2])]))
            .collect();
        let inside = |k: usize, p: DVec2| {
            tops.iter().filter(|(of, _)| *of == k).any(|&(_, [a, b, c])| {
                let s = |p1: DVec2, p2: DVec2| (p2 - p1).perp_dot(p - p1);
                let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
                !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
            })
        };
        let (mut out, mut checked) = (0, 0);
        // a step of 0.3 m in tile units, the normal's north turned to the tile's v (south)
        let metre = 1.0 / (rustradar_core::geo::EARTH_CIRCUMFERENCE_M * 59.3311f64.to_radians().cos() / 2f64.powi(15));
        // (a side is a quad's two triangles `b, b+1, b+2` and `b, b+2, b+3`, its edge `b` to `b+1`)
        for t in platforms.indices.chunks_exact(3) {
            let v = platforms.vertices[t[0] as usize];
            if v.normal[2] != 0.0 || t[1] != t[0] + 1 {
                continue;
            }
            let mid = (at(t[0]) + at(t[1])) / 2.0;
            let n = DVec2::new(f64::from(v.normal[0]), -f64::from(v.normal[1])) * 0.3 * metre;
            let k = polygon[t[0] as usize];
            checked += 1;
            if !inside(k, mid + n) && inside(k, mid - n) {
                out += 1;
            }
        }
        assert!(checked > 20 && out as f64 >= 0.9 * checked as f64, "{out} of {checked} sides face out");
    }

    // ------------------------------------------------------------------ on the GPU

    /// The fixtures, with the rail tiles there or not.
    fn fetcher(rails: bool) -> Arc<dyn TileFetcher> {
        let dir = DirTiles::fixtures();
        Arc::new(FnFetcher(move |source: &str, tile: TileIndex| -> FetchResult {
            if source == RAIL_SOURCE && !rails {
                return Ok(None);
            }
            dir.read(source, tile)
        }))
    }

    /// The realistic map of `view` as the app's scene builder has it (sky, earth, ground, relief),
    /// sunny at midsummer noon.
    fn realistic(view: ViewState) -> SceneInput {
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        scene.feed_now_ms = MIDSUMMER;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        scene.hillshade = hillshade::Input { shown: true, terrain: false };
        scene
    }

    fn shoot(gpu: &crate::gpu::Gpu, scene: &SceneInput, rails: bool) -> Image {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(fetcher(rails), None);
        let image = render_with(gpu, &mut renderer, scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        image
    }

    fn median(values: &mut [[u8; 3]]) -> [u8; 3] {
        std::array::from_fn(|c| {
            let mut v: Vec<u8> = values.iter().map(|p| p[c]).collect();
            v.sort_unstable();
            v[v.len() / 2]
        })
    }

    fn luma(c: [u8; 3]) -> f64 {
        0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])
    }

    /// A tile's segments (tile units), in cells of a 64th of the tile for looking up what is near
    /// a point.
    struct Near {
        cells: std::collections::HashMap<(i32, i32), Vec<(DVec2, DVec2)>>,
    }

    impl Near {
        const CELLS: f64 = 64.0;

        fn new(segments: impl Iterator<Item = (DVec2, DVec2)>) -> Self {
            let mut cells: std::collections::HashMap<(i32, i32), Vec<(DVec2, DVec2)>> = Default::default();
            for (a, b) in segments {
                let (lo, hi) = (a.min(b) * Self::CELLS, a.max(b) * Self::CELLS);
                for j in (lo.y.floor() as i32 - 1)..=(hi.y.floor() as i32 + 1) {
                    for i in (lo.x.floor() as i32 - 1)..=(hi.x.floor() as i32 + 1) {
                        cells.entry((i, j)).or_default().push((a, b));
                    }
                }
            }
            Near { cells }
        }

        /// The distance from `p` to the nearest segment, tile units (infinite past a cell).
        fn distance(&self, p: DVec2) -> f64 {
            let key = ((p.x * Self::CELLS).floor() as i32, (p.y * Self::CELLS).floor() as i32);
            self.cells.get(&key).map_or(f64::INFINITY, |segments| {
                segments
                    .iter()
                    .map(|&(a, b)| {
                        let ab = b - a;
                        let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                        (a + ab * t - p).length()
                    })
                    .fold(f64::INFINITY, f64::min)
            })
        }
    }

    /// The acceptance shot (plan R9): Stockholm C from the fixtures, the plan's station view.
    /// Against the same view without the rail tiles: the pixels on OpenRailwayMap's tracks (as the
    /// CPU puts their middles, within 0.6 m) and on their ballast (1.5 to 2.1 m out, no other track
    /// nearer) change, the ballast's colour not the tracks'; the pixels on the platforms' tops (the
    /// CPU's outlines 0.6 m up) change, lighter than the tracks; and the ground far from both is
    /// left as it was.
    #[test]
    fn stockholm_c_s_tracks_lie_on_their_ballast_beside_standing_platforms() {
        let Some(gpu) = gpu() else { return };
        let scene = realistic(STOCKHOLM_C);
        let (with, without) = (shoot(&gpu, &scene, true), shoot(&gpu, &scene, false));
        with.save_png(&shots_dir().join("r9-stockholm-c.png")).expect("png");
        without.save_png(&shots_dir().join("r9-stockholm-c-without.png")).expect("png");
        // the CPU's tracks (z12, on the ground) and platforms' tops (z15)
        let never = Cancel::new();
        let rails = decode_rails(RAILS_AT_C, &fixture(RAIL_SOURCE, RAILS_AT_C), true, &never).unwrap().unwrap().0;
        let at = |s: &PathSegment| (DVec2::new(f64::from(s.start[0]), f64::from(s.start[1])), DVec2::new(f64::from(s.end[0]), f64::from(s.end[1])));
        let tracks = Near::new(rails.lines.iter().map(at));
        let bridges = Near::new(rails.bridge_shade.iter().map(at));
        let platforms = decode_platforms(PLATFORMS_AT_C, &fixture(RAIL_SOURCE, PLATFORMS_AT_C), &never).unwrap().unwrap();
        let tops: Vec<[DVec2; 3]> = platforms
            .indices
            .chunks_exact(3)
            .filter(|t| platforms.vertices[t[0] as usize].normal[2] == 1.0)
            .map(|t| [t[0], t[1], t[2]].map(|i| {
                let v = platforms.vertices[i as usize];
                DVec2::new(f64::from(v.position[0]), f64::from(v.position[1]))
            }))
            .collect();
        let in_top = |p: DVec2| {
            tops.iter().any(|&[a, b, c]| {
                let s = |p1: DVec2, p2: DVec2| (p2 - p1).perp_dot(p - p1);
                let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
                !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
            })
        };
        let view = ViewFrame::new(&STOCKHOLM_C, SIZE, 1.0, 0.0);
        let (rails_frame, platforms_frame) = (LocalMercator::tile(RAILS_AT_C), LocalMercator::tile(PLATFORMS_AT_C));
        let in_tile = |frame: &LocalMercator, [lon, lat]: [f64; 2]| {
            let [x, y] = rustradar_scene::camera::lng_lat_to_world([lon, lat]);
            let [u, v] = frame.from_world(DVec2::new(x, y));
            DVec2::new(u, v)
        };
        // metres in the z12 tile's units
        let metre = 1.0 / (rustradar_core::geo::EARTH_CIRCUMFERENCE_M * STOCKHOLM_C.latitude.to_radians().cos() / 2f64.powi(12));
        let (mut track, mut ballast, mut tops_seen, mut elsewhere) = (Vec::new(), Vec::new(), Vec::new(), (0, 0));
        // (the map's pixels; the top rows are far, where a metre is under a pixel)
        for y in (240..720).step_by(2) {
            for x in (0..1100).step_by(2) {
                let pixel = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                let (a, b) = (with.pixel(x, y), without.pixel(x, y));
                let changed = (0..3).any(|c| a[c].abs_diff(b[c]) > 6);
                let colour = [a[0], a[1], a[2]];
                let top = in_top(in_tile(&platforms_frame, view.viewport.unproject(pixel, PLATFORM_M)));
                let ground = in_tile(&rails_frame, view.viewport.unproject(pixel, 0.0));
                let on_ground = !top && !in_top(in_tile(&platforms_frame, view.viewport.unproject(pixel, 0.0)));
                let d = tracks.distance(ground) / metre;
                let bridge = bridges.distance(ground) / metre < 12.0;
                if top {
                    tops_seen.push((changed, colour));
                } else if on_ground && !bridge && d < 0.6 {
                    track.push((changed, colour));
                } else if on_ground && !bridge && (1.5..2.1).contains(&d) {
                    ballast.push((changed, colour));
                } else if on_ground && !bridge && d > 4.0 {
                    elsewhere.0 += 1;
                    if changed {
                        elsewhere.1 += 1;
                    }
                }
            }
        }
        let share = |v: &[(bool, [u8; 3])]| v.iter().filter(|(c, _)| *c).count() as f64 / v.len().max(1) as f64;
        let colours = |v: &[(bool, [u8; 3])]| {
            let mut c: Vec<[u8; 3]> = v.iter().map(|&(_, c)| c).collect();
            median(&mut c)
        };
        let (t, b, p) = (colours(&track), colours(&ballast), colours(&tops_seen));
        eprintln!(
            "tracks {} px, {:.3} changed, median {t:?}; ballast {} px, {:.3} changed, median {b:?}; platform tops {} px, {:.3} changed, median {p:?}; elsewhere {} of {} changed",
            track.len(),
            share(&track),
            ballast.len(),
            share(&ballast),
            tops_seen.len(),
            share(&tops_seen),
            elsewhere.1,
            elsewhere.0
        );
        // the tracks and their ballast, in surfaces of their own
        assert!(track.len() > 1000 && share(&track) > 0.95, "tracks");
        assert!(ballast.len() > 500 && share(&ballast) > 0.95, "ballast");
        let apart = (0..3).map(|i| (f64::from(t[i]) - f64::from(b[i])).powi(2)).sum::<f64>().sqrt();
        assert!(apart > 20.0, "the ballast {b:?} against the tracks {t:?}");
        // the platforms standing over the ground, paved, lighter than the tracks
        assert!(tops_seen.len() > 3000 && share(&tops_seen) > 0.75, "platforms");
        assert!(luma(p) > luma(t) + 10.0, "the platforms {p:?} against the tracks {t:?}");
        // and the rest of the ground as it was
        assert!(f64::from(elsewhere.1) < 0.03 * f64::from(elsewhere.0), "{elsewhere:?}");
    }

    /// Only the realistic map asks for the rail tiles: by deck's rule at zooms 7 to 12 on either
    /// projection, and on the flat map from zoom 15 the platforms' zoom 15 tiles within 4 km. Not
    /// the dark map, not a scene without the ground.
    #[test]
    fn only_the_realistic_map_asks_for_rails_and_the_flat_map_close_in_for_platforms() {
        let Some(gpu) = gpu() else { return };
        let asked = Arc::new(Mutex::new(Vec::new()));
        let noted = asked.clone();
        let dir = DirTiles::fixtures();
        let fetcher: Arc<dyn TileFetcher> = Arc::new(FnFetcher(move |source: &str, tile: TileIndex| -> FetchResult {
            if source == RAIL_SOURCE {
                noted.lock().unwrap().push(tile);
            }
            dir.read(source, tile)
        }));
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fetcher, None);
        let mut take = |scene: SceneInput| -> Vec<TileIndex> {
            render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            std::mem::take(&mut *asked.lock().unwrap())
        };
        let mut dark = realistic(STOCKHOLM_C);
        dark.basemap = Basemap::Dark;
        let mut bare = realistic(STOCKHOLM_C);
        bare.ground = ground::Input::default();
        assert_eq!(take(dark), []);
        assert_eq!(take(bare), []);
        // the globe: rail tiles alone, at deck's zoom for 512-px tiles (round(10) = 10)
        let globe = take(realistic(ViewState { zoom: 10.0, pitch: 0.0, ..STOCKHOLM_C }));
        assert!(!globe.is_empty() && globe.iter().all(|t| t.z == 10), "{globe:?}");
        // the flat map under zoom 15: the tracks at 12 (deck's 14, clamped), no platform yet
        let flat = take(realistic(ViewState { zoom: 14.9, ..STOCKHOLM_C }));
        assert!(!flat.is_empty() && flat.iter().all(|t| t.z == 12), "{flat:?}");
        // the station view: platform tiles within 4 km of the camera, those under the centre among them
        let close = take(realistic(STOCKHOLM_C));
        let platforms: Vec<TileIndex> = close.iter().copied().filter(|t| t.z == 15).collect();
        assert!(platforms.contains(&PLATFORMS_AT_C), "{close:?}");
        assert!(close.iter().all(|t| t.z == 12 || t.z == 15), "{close:?}");
        // (within 4 km of the camera: no tile's middle much further from the view's centre)
        let [x, y] = rustradar_scene::camera::lng_lat_to_world([STOCKHOLM_C.longitude, STOCKHOLM_C.latitude]);
        let metres_per_unit = rustradar_core::geo::EARTH_CIRCUMFERENCE_M * STOCKHOLM_C.latitude.to_radians().cos() / 512.0;
        for t in &platforms {
            let metres = (LocalMercator::tile(*t).to_world([0.5, 0.5]) - DVec2::new(x, y)).length() * metres_per_unit;
            assert!(metres < 5000.0, "{t:?} {metres} m out");
        }
    }

    /// On the globe (`ground-rails-globe`, zoom 11.6 over Stockholm C: deck's zoom 12 tiles, the
    /// fixtures'), the tracks are drawn from the same tiles the globe's way, far out a hairline:
    /// the pixels at the middles of the CPU's tracks change against the view without them.
    #[test]
    fn on_the_globe_the_tracks_are_hairlines() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { zoom: 11.6, pitch: 0.0, ..STOCKHOLM_C };
        let scene = realistic(view);
        let (with, without) = (shoot(&gpu, &scene, true), shoot(&gpu, &scene, false));
        with.save_png(&shots_dir().join("r9-globe.png")).expect("png");
        let rails = decode_rails(RAILS_AT_C, &fixture(RAIL_SOURCE, RAILS_AT_C), false, &Cancel::new()).unwrap().unwrap().0;
        let frame = ViewFrame::new(&view, SIZE, 1.0, 0.0);
        assert!(frame.viewport.is_globe());
        let tile = LocalMercator::tile(RAILS_AT_C);
        let (mut seen, mut changed) = (0, 0);
        for s in &rails.lines {
            let mid = [(f64::from(s.start[0]) + f64::from(s.end[0])) / 2.0, (f64::from(s.start[1]) + f64::from(s.end[1])) / 2.0];
            let [lon, lat] = rustradar_scene::camera::world_to_lng_lat(tile.to_world(mid).to_array());
            let [x, y, _] = frame.viewport.project([lon, lat, 0.0]);
            if !(2.0..1098.0).contains(&x) || !(2.0..718.0).contains(&y) {
                continue;
            }
            seen += 1;
            let (a, b) = (with.pixel(x as u32, y as u32), without.pixel(x as u32, y as u32));
            if (0..3).any(|c| a[c].abs_diff(b[c]) > 6) {
                changed += 1;
            }
        }
        eprintln!("{changed} of {seen} track middles changed");
        assert!(seen > 200 && f64::from(changed) > 0.7 * f64::from(seen), "{changed} of {seen}");
    }
}
