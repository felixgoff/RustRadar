//! The realistic map's buildings (item R4; doc 04 §7.1–7.3; row 9 of doc 04 §1; row 7 of doc 03
//! §3): `buildings-city-realistic` (`cityBuildingLayer` with `realistic`, G:1039-1092, through
//! `BuildingLayer`, `building-layer.ts`, with `BuildingExtension`, `realistic.ts`), each building
//! in its own façade, roof and colour, a window to each bay, lit by the realistic map's light and
//! its windows lit at night. The tessellation is `rustradar_scene::tess::buildings`'s realistic
//! half ([`tess::tessellate_realistic`]); the shader is `shaders/building.wgsl`.
//!
//! What today's app does, and so this layer:
//! - **When.** With the *buildings* setting on, on the flat map (zoom > 12), on the realistic
//!   basemap (G:2694; [`super::buildings::Buildings`] draws the other basemaps'). The scene builder
//!   sets `SceneInput::buildings` from the setting (`app::scene_builder::base`); this layer works
//!   out the projection and the basemap. When it stops drawing it drops its tiles, as deck
//!   finalizes a layer that leaves the list.
//! - **Tiles** by their distance from the camera (`LodMVTLayer`): [`LodSelect`] over
//!   `tess::city_lod()` (`CITY_LOD`: levels 13, 14, 16, bias 0.5, 25 km) with the look-ahead while
//!   zooming, OpenFreeMap's `building` layer at zooms 13 and 14 and Streets GL's `buildings` at its
//!   zoom 16 ([`sources`], `cityBuildingSources`; `io::STREETS_GL` asks for no other zoom, as
//!   `sglFetch` does), 8 requests at once (`maxRequests: 8`), refined by `keepCoarserUntilIn`
//!   ([`Refinement::KeepCoarserUntilIn`]: a coarser tile in place of finer ones until they are all
//!   in, never with them). Decoded on the worker pool, uploaded within the frame's budget, drawn
//!   coarse to fine. So at V5 (zoom 15.5, pitched 60°) every tile is Streets GL's.
//! - **Each tile**: a near one (zoom ≥ 14, `CITY_CLOSE_ZOOM`) extruded, its walls and flat roofs in
//!   the building shader ([`BuildingShader`]: Streets GL's texture arrays, `textures::
//!   BuildingTextures`, through the realistic light, `look::LightUniform`); a far one (zoom 13)
//!   flat on the ground in each building's colour and deck's alpha (`255 − code`), unlit: as
//!   today's app draws it (plan section 7: "as today"; the building extension needs `extruded`).
//!   Both with `NO_CULL` (depth tested and written), clipped to the tile's square by fragment
//!   (MVTLayer's `ClipExtension` on the flat map; the shader's `discard`), as the plain buildings.
//! - **Textures** (doc 04 §5.3): the realistic map's set, started by the first realistic layer that
//!   draws ([`super::ground::load_textures`]); the stand-ins (white, flat, no glow) are bound until
//!   every file is in, and the layer is pending meanwhile (the shots wait for them).
//!
//! For the items after this one: R5 (foot shade, shadows) draws each near tile's
//! [`TileMeshes::footprints`] (cast heights) before its buildings, R6 (pitched roofs,
//! [`super::roofs`]) each Streets GL tile's roof mesh after them, through `vs_roof` and `fs_roof` of
//! the same shader; both go in [`Layer::draw`]'s loop below, tile by tile, in that order
//! (`renderSubLayers`: `[...ground, buildings, ...over]`). R6 also decodes the tiles
//! ([`super::roofs::decode`]): Streets GL's with Overture's heights where they have none
//! (`sglBuildingFetch`) and their roofs.
//!
//! How it is checked (the tests below): V5 from the fixtures at noon, each of the nearest walls
//! facing the camera sampled along its top storey on the screen: its brightness repeats once a bay
//! (a window to each bay, `textures::BUILDING_TEXTURES`' window layers); at midwinter night the
//! windows' middles are brighter and warmer than the wall between them. Far tiles lie flat in each
//! building's colour and alpha. The layer asks for Streets GL's tiles only on the realistic flat
//! map with the setting on.
//!
//! Deviations, invisible: the walls are vertices of their own with their bays and storeys worked
//! out on the CPU (doc 04 §16.3), not deck's instanced sides; the textures' mip bias is a scale of
//! the gradients (`building.wgsl`'s header); the clip is per pixel, as deck's discard.

use rustradar_core::geo::Basemap;
use rustradar_scene::lod::TILE_SIZE;
use rustradar_scene::streets_gl::SGL_ZOOM;
use rustradar_scene::tess::buildings::{self as tess, Footprints, RealisticTile, RealisticVertex};

use super::building_shadows::BuildingShadows;
use super::roofs::{self, Overture, Roofs};
use super::{ByteSize, DrawCx, Init, Layer, LodSelect, Pass, PrepareCx, Refinement, Source, TileIndex, TileSet, TileSetOptions};
use crate::frame::FrameSlot;
use crate::io;
use crate::jobs::Cancel;
use crate::layers::ground::load_textures;
use crate::look::{LightUniform, Look};
use crate::pipelines::fill;
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;
use crate::textures::Sampling;

/// `shaders/building.wgsl`.
pub const SHADER: &str = include_str!("../shaders/building.wgsl");

/// The bind group of the buildings' textures (after the light's, group 2).
pub const BUILDING_GROUP: u32 = 3;

/// [`RealisticVertex`]'s buffer: position, normal, uv (`Float32x3`, `Float32x3`, `Float32x2`), the
/// tint (`Unorm8x4`) and the style (`Uint8x4`), locations 0–4.
pub const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<RealisticVertex>() as u64,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Unorm8x4, 4 => Uint8x4],
};

/// The realistic buildings' sources by zoom (`cityBuildingSources` with `realistic`): OpenFreeMap
/// at 13 and 14, Streets GL at its zoom.
pub fn sources() -> Vec<Source> {
    vec![Source::at(io::OPENFREEMAP, tess::SOURCE_ZOOMS.to_vec()), Source::at(io::STREETS_GL, vec![SGL_ZOOM])]
}

impl ByteSize for RealisticTile {
    fn byte_size(&self) -> usize {
        RealisticTile::byte_size(self)
    }
}

/// The decode step, on the worker pool (`None` for a tile with no buildings), without Overture's
/// heights (the layer's own decode has them: [`roofs::decode`]).
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<RealisticTile>, String> {
    roofs::decode(tile, bytes, cancel, None)
}

/// A tile on the GPU.
pub struct TileMeshes {
    /// Standing in the building shader (zoom ≥ 14), or flat and unlit.
    pub extruded: bool,
    /// Streets GL's (zoom 16).
    pub sgl: bool,
    pub mesh: fill::Mesh<RealisticVertex>,
    /// What the tile's shadows and foot shade are cast from (R5).
    pub footprints: Footprints,
    /// Its pitched roofs (R6), drawn after its buildings.
    pub roofs: Option<fill::Mesh<RealisticVertex>>,
}

/// The upload step, on the main thread.
pub fn upload(device: &wgpu::Device, decoded: RealisticTile) -> TileMeshes {
    TileMeshes {
        extruded: decoded.extruded,
        sgl: decoded.sgl,
        mesh: fill::Mesh::new(device, "realistic buildings", &decoded.vertices, &decoded.indices),
        roofs: roofs::upload(device, &decoded.roofs),
        footprints: decoded.footprints,
    }
}

/// What every draw of the building shader shares: the module, the light (group 2) and the
/// textures with their sampler (group 3, made again when the textures swap in). A layer keeps one,
/// makes its pipelines with it and calls [`Self::prepare`] each frame it draws.
pub struct BuildingShader {
    module: wgpu::ShaderModule,
    light: LightUniform,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Group 3, made from the textures as they were (`bound`: ready or not).
    group: Option<wgpu::BindGroup>,
    bound: Option<bool>,
}

impl BuildingShader {
    pub fn new(init: &Init<'_>, label: &str) -> Self {
        let device = &init.gpu.device;
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("buildings"),
            entries: &[
                texture(0),
                texture(1),
                texture(2),
                texture(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        BuildingShader {
            module: shaders::module(device, label, SHADER),
            light: LightUniform::new(device),
            layout,
            sampler: Sampling::Repeat.sampler(device),
            group: None,
            bound: None,
        }
    }

    /// A pipeline of `vertex_entry` (`vs_building`, `vs_roof`) and `fragment_entry`
    /// (`fs_building`, `fs_flat`) with `state`, for [`RealisticVertex`]'s buffer.
    pub fn pipeline(&self, init: &Init<'_>, label: &str, vertex_entry: &str, fragment_entry: &str, state: DrawState) -> wgpu::RenderPipeline {
        build(
            init,
            &Desc {
                label,
                module: &self.module,
                vertex_entry,
                fragment_entry,
                buffers: &[Some(VERTEX_LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state,
                groups: &[self.light.layout(), &self.layout],
            },
        )
    }

    /// This frame's light (written, and returned), and group 3 made again if the textures have
    /// swapped since.
    pub fn prepare(&mut self, cx: &PrepareCx<'_>) -> Look {
        let ready = cx.textures.ready;
        if self.bound != Some(ready) {
            let t = &cx.textures.buildings;
            self.group = Some(cx.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("buildings"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&t.color.view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&t.normal.view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&t.mask.view) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&t.glow.view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            }));
            self.bound = Some(ready);
        }
        self.light.prepare(cx)
    }

    /// Sets `pipeline` and binds groups 2 and 3. After [`Self::prepare`].
    pub fn begin(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline) {
        pass.set_pipeline(pipeline);
        self.light.bind(pass);
        if let Some(group) = &self.group {
            pass.set_bind_group(BUILDING_GROUP, group, &[]);
        }
    }
}

/// Row 9 of doc 04 §1: `buildings-city-realistic`.
pub struct BuildingsRealistic {
    tiles: TileSet<RealisticTile, TileMeshes>,
    shader: BuildingShader,
    /// Near tiles: walls and roofs in the building shader.
    walls: wgpu::RenderPipeline,
    /// Far tiles: footprints flat, unlit.
    flat: wgpu::RenderPipeline,
    /// Whether this frame draws, and whether the textures were still loading.
    shown: bool,
    textures_loading: bool,
    /// This frame's light.
    look: Option<Look>,
    /// This frame's tiles in draw order, each with its frame.
    drawn: Vec<(FrameSlot, TileIndex)>,
    /// The near tiles' foot shade and shadows (R5).
    shadows: BuildingShadows,
    /// Streets GL's tiles' pitched roofs (R6).
    roofs: Roofs,
}

impl BuildingsRealistic {
    pub fn new(init: &Init<'_>) -> Self {
        // (Streets GL's tiles with Overture's heights where they have none: R6)
        let overture = Overture::new(init.tiles);
        let tiles = init.tile_set(
            sources(),
            LodSelect::new(tess::city_lod(), TILE_SIZE),
            TileSetOptions { max_requests: tess::MAX_REQUESTS, refinement: Refinement::KeepCoarserUntilIn, ..Default::default() },
            move |tile, bytes, cancel| roofs::decode(tile, &bytes, cancel, Some(&overture)),
        );
        let shader = BuildingShader::new(init, "realistic buildings");
        let walls = shader.pipeline(init, "realistic buildings", "vs_building", "fs_building", DrawState::NO_CULL);
        let flat = shader.pipeline(init, "realistic buildings, far", "vs_building", "fs_flat", DrawState::NO_CULL);
        let shadows = BuildingShadows::new(init);
        let roofs = Roofs::new(init, &shader);
        BuildingsRealistic { tiles, shader, walls, flat, shown: false, textures_loading: false, look: None, drawn: Vec::new(), shadows, roofs }
    }

    /// The tile set (tests).
    pub fn tiles(&self) -> &TileSet<RealisticTile, TileMeshes> {
        &self.tiles
    }

    /// This frame's tiles in draw order (coarse to fine).
    pub fn drawn(&self) -> impl Iterator<Item = TileIndex> + '_ {
        self.drawn.iter().map(|&(_, tile)| tile)
    }

    /// The light of the last frame drawn.
    pub fn look(&self) -> Option<&Look> {
        self.look.as_ref()
    }
}

impl Layer for BuildingsRealistic {
    fn name(&self) -> &'static str {
        "buildings-realistic"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.drawn.clear();
        self.shown = cx.scene.buildings.shown && !cx.view.is_globe() && cx.scene.basemap == Basemap::Realistic;
        if !self.shown {
            // deck drops a layer that leaves the list, its tiles with it
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            self.textures_loading = false;
            self.look = None;
            self.shadows.clear();
            return;
        }
        cx.update_tiles(&mut self.tiles, |gpu, _tile, decoded| upload(&gpu.device, decoded));
        load_textures(cx);
        self.textures_loading = cx.textures.pending();
        self.look = Some(self.shader.prepare(cx));
        self.shadows.prepare(cx, &self.tiles);
        for (tile, _) in self.tiles.visible() {
            self.drawn.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !self.shown || cx.pass != Pass::Map {
            return;
        }
        for &(slot, tile) in &self.drawn {
            let Some(meshes) = self.tiles.get(tile) else { continue };
            if meshes.mesh.index_count() == 0 {
                continue;
            }
            // a near tile's foot shade, then its shadows, on the ground under its buildings
            self.shadows.draw_tile(pass, cx, slot, tile);
            // far off, flat: only the roofs show from there
            self.shader.begin(pass, if meshes.extruded { &self.walls } else { &self.flat });
            cx.bind_frame(pass, slot);
            meshes.mesh.draw(pass);
            // a Streets GL tile's pitched roofs over its buildings (R6)
            if let Some(roofs) = &meshes.roofs {
                self.roofs.draw(pass, cx, &self.shader, slot, roofs);
            }
        }
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Offset
    }

    fn pending(&self) -> bool {
        self.shown && (self.tiles.loading() || self.textures_loading || self.shadows.pending())
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path as FilePath, PathBuf};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::lod::distance_tile_indices;
    use rustradar_scene::tess::buildings::{GLASS, STYLE_BAYS, STYLE_WALL};
    use rustradar_scene::tiles::osm_tile_to_lng_lat;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchRequest, FetchResult, TileFetcher};
    use crate::layers::{buildings, earth, ground, sky};
    use crate::renderer::{MapRenderer, SceneInput};
    use crate::target::CLEAR_RGB;

    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// 2026-12-21 22:00 UTC: night over Stockholm.
    const MIDWINTER_NIGHT: f64 = 1_797_890_400_000.0;
    /// V5 (plan section 8): Stockholm city, pitched, at today's pictures' size.
    const V5: ViewState = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
    const V5_SIZE: [f64; 2] = [1440.0, 900.0];
    /// V5's place close in (zoom 18): a bay of 4 m some 13 px on the screen, where the windows show
    /// (at V5 itself a pixel is 1.7 m and the shader evens the windows out over the walls, as
    /// today's picture has them).
    const CLOSE: ViewState = ViewState { zoom: 18.0, ..V5 };
    const CLOSE_SIZE: [f64; 2] = [1100.0, 720.0];

    fn shots_dir() -> PathBuf {
        let dir = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// The realistic map at `view` on a `size` canvas at `now`, a fine day's weather: the sky, the
    /// earth, the ground, and the buildings when `buildings`.
    fn realistic(view: ViewState, size: [f64; 2], now: f64, buildings: bool) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Realistic);
        scene.local_now_ms = now;
        scene.feed_now_ms = now;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        scene.buildings = buildings::Input { shown: buildings };
        scene
    }

    fn luma(p: [u8; 4]) -> f64 {
        0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
    }

    #[test]
    fn the_vertex_layout_is_the_scene_s_and_the_shader_validates() {
        shaders::tests::validate("building.wgsl", &shaders::expand(SHADER).expect("libraries known"));
        assert_eq!(VERTEX_LAYOUT.array_stride, 40);
        let offsets: Vec<u64> = VERTEX_LAYOUT.attributes.iter().map(|a| a.offset).collect();
        assert_eq!(
            offsets,
            [
                std::mem::offset_of!(RealisticVertex, position),
                std::mem::offset_of!(RealisticVertex, normal),
                std::mem::offset_of!(RealisticVertex, uv),
                std::mem::offset_of!(RealisticVertex, tint),
                std::mem::offset_of!(RealisticVertex, style),
            ]
            .map(|o| o as u64)
        );
        assert_eq!(sources().len(), 2);
    }

    #[test]
    fn fixture_tiles_decode_streets_gl_s_and_openfreemap_s() {
        let fixtures = DirTiles::fixtures();
        for (source, tile) in [(io::STREETS_GL, TileIndex::new(36057, 19267, 16)), (io::OPENFREEMAP, TileIndex::new(9014, 4818, 14)), (io::OPENFREEMAP, TileIndex::new(4507, 2409, 13))] {
            let bytes = fixtures.read(source, tile).expect("read").expect("a fixture tile");
            let decoded = decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("buildings");
            assert_eq!(decoded.sgl, tile.z == SGL_ZOOM);
            assert_eq!(decoded.extruded, tile.z >= tess::CITY_CLOSE_ZOOM);
            assert_eq!(ByteSize::byte_size(&decoded), decoded.byte_size());
            let cancel = Cancel::new();
            cancel.cancel();
            assert!(decode(tile, &bytes, &cancel).is_err());
        }
        assert_eq!(decode(TileIndex::new(36057, 19267, 16), &[], &Cancel::new()), Ok(None));
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

    /// The buildings of the realistic map ask for Streets GL's tiles (G:2694) only on the realistic
    /// basemap, on the flat map, with the setting on: at V5 every one the distance LOD picks (all
    /// zoom 16, none coarser); on the dark map, with the setting off or on the globe, none.
    #[test]
    fn streets_gl_s_buildings_are_asked_for_only_on_the_realistic_flat_map() {
        let Some(gpu) = gpu() else { return };
        let asked = |basemap: Basemap, view: ViewState, shown: bool| {
            let noted = Arc::new(Noted { tiles: DirTiles::fixtures(), asked: Mutex::new(Vec::new()) });
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(noted.clone(), None);
            let mut scene = SceneInput::new(view, V5_SIZE, 1.0, basemap);
            scene.local_now_ms = MIDSUMMER;
            scene.buildings = buildings::Input { shown };
            render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            let asked = noted.asked.lock().unwrap_or_else(|e| e.into_inner()).clone();
            asked.into_iter().filter(|(source, _)| source == io::STREETS_GL).map(|(_, tile)| tile).collect::<Vec<_>>()
        };
        let wanted = distance_tile_indices(&ViewFrame::new(&V5, V5_SIZE, 1.0, 0.0).viewport, &tess::city_lod(), None, 0.0);
        assert!(!wanted.is_empty() && wanted.iter().all(|t| t.z == SGL_ZOOM), "{wanted:?}");
        let got = asked(Basemap::Realistic, V5, true);
        for tile in &wanted {
            assert!(got.contains(tile), "{tile:?} not asked");
        }
        assert!(got.iter().all(|t| t.z == SGL_ZOOM));
        assert!(asked(Basemap::Dark, V5, true).is_empty());
        assert!(asked(Basemap::Realistic, V5, false).is_empty());
        assert!(asked(Basemap::Realistic, ViewState { zoom: 12.0, ..V5 }, true).is_empty());
    }

    /// V5 from the fixtures by day and at night, beside the same shots without the buildings (the
    /// pixels that change are the buildings'): they stand over much of the view; at night their
    /// walls carry their windows' light evened out (a pixel spans 1.7 m: a bay is too small to
    /// tell its windows apart), warm and brighter than the ground beside them.
    #[test]
    fn v5_stands_full_of_buildings_whose_walls_glow_at_night() {
        let Some(gpu) = gpu() else { return };
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let mut shot = |now: f64, buildings: bool, name: &str| {
            let image = render_with(&gpu, &mut renderer, &realistic(V5, V5_SIZE, now, buildings), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            image.save_png(&shots_dir().join(name)).expect("png");
            image
        };
        let day = shot(MIDSUMMER, true, "r4-v5-day.png");
        let bare_day = shot(MIDSUMMER, false, "r4-v5-day-bare.png");
        let night = shot(MIDWINTER_NIGHT, true, "r4-v5-night.png");
        let bare_night = shot(MIDWINTER_NIGHT, false, "r4-v5-night-bare.png");
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
        let differs = |a: [u8; 4], b: [u8; 4]| (0..3).map(|i| a[i].abs_diff(b[i]) as u32).sum::<u32>() > 6;
        let pixels = (V5_SIZE[0] * V5_SIZE[1]) as usize;
        let (mut covered, mut lit, mut ground) = (0usize, [0.0f64; 4], [0.0f64; 3]);
        for y in 0..900 {
            for x in 0..1440 {
                if differs(day.pixel(x, y), bare_day.pixel(x, y)) {
                    covered += 1;
                }
                let (n, b) = (night.pixel(x, y), bare_night.pixel(x, y));
                if differs(n, b) {
                    lit[0] += 1.0;
                    lit[1] += luma(n);
                    lit[2] += f64::from(n[0]) - f64::from(n[2]);
                    lit[3] += luma(b);
                } else {
                    ground[0] += 1.0;
                    ground[1] += luma(n);
                    ground[2] += f64::from(n[0]) - f64::from(n[2]);
                }
            }
        }
        let (walls, warmth, under) = (lit[1] / lit[0], lit[2] / lit[0], lit[3] / lit[0]);
        let (open, cold) = (ground[1] / ground[0], ground[2] / ground[0]);
        eprintln!(
            "V5: buildings over {:.1} % of the view by day; at night {:.0} px of buildings, luma {walls:.1} (r − b {warmth:.1}) over ground of {under:.1} where they stand and {open:.1} (r − b {cold:.1}) between them",
            100.0 * covered as f64 / pixels as f64,
            lit[0]
        );
        assert!(covered * 3 > pixels, "{covered} of {pixels}");
        assert!(warmth > 10.0 && warmth > cold + 15.0, "warm: {warmth} against the ground's {cold}");
        assert!(walls > under + 40.0 && walls > open + 40.0, "{walls} against {under} and {open}");
    }

    /// One wall of a decoded tile: its ends (tile units), height, bays, storeys and façade.
    #[derive(Debug, Clone, Copy)]
    struct Wall {
        tile: TileIndex,
        a: [f64; 2],
        b: [f64; 2],
        height: f64,
        bays: f64,
        levels: f64,
        normal: [f64; 2],
        facade: u8,
    }

    impl Wall {
        /// `[lon, lat]` of the point `s` (0 to 1) along it.
        fn lng_lat(&self, s: f64) -> [f64; 2] {
            let [u, v] = [self.a[0] + (self.b[0] - self.a[0]) * s, self.a[1] + (self.b[1] - self.a[1]) * s];
            osm_tile_to_lng_lat(f64::from(self.tile.x) + u, f64::from(self.tile.y) + v, self.tile.z)
        }

        /// The pixel of the point `bays` along it and `storeys` up it.
        fn pixel(&self, view: &ViewFrame, bays: f64, storeys: f64) -> Option<[u32; 2]> {
            let [lon, lat] = self.lng_lat(bays / self.bays);
            let [x, y, _] = view.viewport.project([lon, lat, self.height * storeys / self.levels]);
            let (w, h) = (f64::from(view.globals.viewport[0]), f64::from(view.globals.viewport[1]));
            (x >= 2.0 && y >= 2.0 && x < w - 2.0 && y < h - 2.0).then_some([x as u32, y as u32])
        }
    }

    /// The walls with a window to each bay (no glass) of `tile`'s decoded buildings, each quad of
    /// `tess::tessellate_realistic` (four vertices, its wall style).
    fn walls_of(tile: TileIndex, decoded: &RealisticTile) -> Vec<Wall> {
        let v = &decoded.vertices;
        let mut out = Vec::new();
        let mut i = 0;
        while i < v.len() {
            if v[i].style[3] & STYLE_WALL == 0 {
                i += 1;
                continue;
            }
            let q = &v[i..i + 4];
            if q[0].style[3] & STYLE_BAYS != 0 && q[0].style[0] != GLASS {
                out.push(Wall {
                    tile,
                    a: [f64::from(q[0].position[0]), f64::from(q[0].position[1])],
                    b: [f64::from(q[1].position[0]), f64::from(q[1].position[1])],
                    height: f64::from(q[2].position[2]),
                    bays: f64::from(q[1].uv[0]),
                    levels: f64::from(q[2].uv[1]),
                    normal: [f64::from(q[0].normal[0]), f64::from(q[0].normal[1])],
                    facade: q[0].style[0],
                });
            }
            i += 4;
        }
        out
    }

    /// The walls of `view`'s Streets GL tiles facing its camera, at least `min_bays` bays and three
    /// storeys, a bay and a storey at least 12 px on the screen and every one of their top storey's
    /// bays on it, nearest the camera first.
    fn near_walls(view: &ViewFrame, min_bays: f64) -> Vec<Wall> {
        let fixtures = DirTiles::fixtures();
        let camera = glam::Vec3::from(view.globals.camera_render).as_dvec3();
        let mut walls: Vec<(f64, Wall)> = Vec::new();
        for tile in distance_tile_indices(&view.viewport, &tess::city_lod(), None, 0.0) {
            let Ok(Some(bytes)) = fixtures.read(io::STREETS_GL, tile) else { continue };
            let Ok(Some(decoded)) = decode(tile, &bytes, &Cancel::new()) else { continue };
            for wall in walls_of(tile, &decoded) {
                if wall.bays < min_bays || wall.levels < 3.0 {
                    continue;
                }
                let middle = view.common_offset(wall.lng_lat(0.5), wall.height / 2.0).as_dvec3();
                let to_camera = (camera - middle).truncate().normalize();
                let normal = glam::DVec2::from_array(wall.normal).normalize();
                if normal.dot(to_camera) < 0.4 {
                    continue;
                }
                let top = wall.levels - 0.5;
                let at = |b: f64, s: f64| wall.pixel(view, b, s).map(|[x, y]| glam::DVec2::new(f64::from(x), f64::from(y)));
                let (Some(p0), Some(p1), Some(p2)) = (at(0.0, top - 0.5), at(1.0, top - 0.5), at(0.0, top + 0.5)) else { continue };
                if p0.distance(p1) < 12.0 || p0.distance(p2) < 12.0 {
                    continue;
                }
                if (0..=(wall.bays as u32)).any(|k| at(f64::from(k), top).is_none()) {
                    continue;
                }
                walls.push(((camera - middle).length(), wall));
            }
        }
        walls.sort_by(|a, b| a.0.total_cmp(&b.0));
        walls.into_iter().map(|(_, w)| w).collect()
    }

    /// Close in at V5's place (zoom 18), each of the nearest walls facing the camera sampled along
    /// its top storey: by day, on each of them that faces the sun, the middle of each bay (its
    /// window: dark glass in the façade's texture) differs from the bay's edges (the wall) by much
    /// more than the wall varies (on a wall in the shade the two are close, as the texture has
    /// them); at midwinter night about half of the windows are lit (`step(0.48, pick)`), warm and
    /// well above the wall between them.
    #[test]
    fn close_in_each_bay_has_a_window_and_at_night_half_of_them_are_lit() {
        let Some(gpu) = gpu() else { return };
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let day = render_with(&gpu, &mut renderer, &realistic(CLOSE, CLOSE_SIZE, MIDSUMMER, true), 1.0).expect("rendered");
        day.save_png(&shots_dir().join("r4-close-day.png")).expect("png");
        let night = render_with(&gpu, &mut renderer, &realistic(CLOSE, CLOSE_SIZE, MIDWINTER_NIGHT, true), 1.0).expect("rendered");
        night.save_png(&shots_dir().join("r4-close-night.png")).expect("png");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        let view = ViewFrame::new(&CLOSE, CLOSE_SIZE, 1.0, 0.0);
        let walls = near_walls(&view, 4.0);
        eprintln!("{} walls to look at, the nearest: {:?}", walls.len(), walls.iter().take(6).collect::<Vec<_>>());
        assert!(walls.len() >= 4, "{walls:?}");
        // a bay's window (its middle) and its wall (its edges), on the top storey
        let sample = |image: &Image, wall: &Wall, k: f64, at: &[f64]| -> Option<[f64; 4]> {
            let mut sum = [0.0; 4];
            for &u in at {
                let [x, y] = wall.pixel(&view, k + u, wall.levels - 0.5)?;
                let p = image.pixel(x, y);
                for i in 0..3 {
                    sum[i] += f64::from(p[i]) / at.len() as f64;
                }
                sum[3] += luma(p) / at.len() as f64;
            }
            Some(sum)
        };
        const WINDOW: [f64; 2] = [0.42, 0.58];
        const EDGES: [f64; 2] = [0.08, 0.92];
        // towards the sun at noon, east and north, at the view's centre
        let to_sun = rustradar_core::sun::local_sun(rustradar_core::sun::sun_direction(MIDSUMMER), CLOSE.latitude, CLOSE.longitude);
        let sun = glam::DVec2::new(to_sun[0], to_sun[1]).normalize();
        let (mut sunny, mut windowed) = (0, 0);
        let (mut lit, mut bays, mut lit_luma, mut lit_warmth, mut wall_luma) = (0usize, 0usize, Vec::new(), Vec::new(), Vec::new());
        for wall in walls.iter().take(6) {
            let mut differences = Vec::new();
            let mut spread = Vec::new();
            for k in 0..wall.bays as u32 {
                let k = f64::from(k);
                let (Some(window), Some(edge)) = (sample(&day, wall, k, &WINDOW), sample(&day, wall, k, &EDGES)) else { continue };
                differences.push(window[3] - edge[3]);
                spread.push(edge[3]);
                let (Some(window), Some(edge)) = (sample(&night, wall, k, &WINDOW), sample(&night, wall, k, &EDGES)) else { continue };
                bays += 1;
                wall_luma.push(edge[3]);
                if window[3] > edge[3] + 30.0 {
                    lit += 1;
                    lit_luma.push(window[3]);
                    lit_warmth.push(window[0] - window[2]);
                }
            }
            let n = differences.len() as f64;
            let mean = differences.iter().sum::<f64>() / n;
            let edge_mean = spread.iter().sum::<f64>() / n;
            let edge_sd = (spread.iter().map(|e| (e - edge_mean).powi(2)).sum::<f64>() / n).sqrt();
            let facing = glam::DVec2::from_array(wall.normal).normalize().dot(sun);
            eprintln!("wall of façade {} ({} bays, {} storeys, {facing:.2} to the sun): window − wall {mean:.1} by day, the wall's own spread {edge_sd:.1}", wall.facade, wall.bays, wall.levels);
            if facing > 0.6 {
                sunny += 1;
                if mean.abs() > 12.0 && mean.abs() > 3.0 * edge_sd {
                    windowed += 1;
                }
            }
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let share = lit as f64 / bays as f64;
        eprintln!(
            "by day {windowed} of the {sunny} nearest walls in the sun show a window to each bay; at night {lit} of {bays} windows lit ({share:.2}), median luma {:.1} (r − b {:.1}) against the wall's {:.1}",
            median(&mut lit_luma.clone()),
            median(&mut lit_warmth.clone()),
            median(&mut wall_luma.clone())
        );
        assert!(sunny >= 2 && windowed == sunny, "{windowed} of {sunny} walls in the sun with windows");
        assert!((0.3..=0.75).contains(&share), "{lit} of {bays} lit");
        assert!(median(&mut lit_luma) > median(&mut wall_luma) + 40.0);
        assert!(median(&mut lit_warmth) > 20.0, "warm light");
    }

    /// V4's place just onto the flat map (zoom 12.1): every building tile is a far one of zoom 13,
    /// so every building lies flat on the ground, unlit, in its own colour at deck's alpha
    /// (`255 − code`): over the map's clear colour (nothing else drawn) a pixel inside a footprint
    /// is that colour blended once.
    #[test]
    fn far_tiles_lie_flat_in_each_building_s_colour() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { latitude: 59.6519, longitude: 17.9186, zoom: 12.1, pitch: 45.0, bearing: 30.0, position: None };
        let size = [1440.0, 900.0];
        let frame = ViewFrame::new(&view, size, 1.0, 0.0);
        let wanted = distance_tile_indices(&frame.viewport, &tess::city_lod(), None, 0.0);
        assert!(!wanted.is_empty() && wanted.iter().all(|t| t.z == 13), "{wanted:?}");
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        scene.buildings = buildings::Input::SHOWN;
        let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        image.save_png(&shots_dir().join("r4-far-flat.png")).expect("png");
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
        let mut blended = std::collections::HashSet::new();
        for tile in &wanted {
            let bytes = fixtures.read(io::OPENFREEMAP, *tile).expect("read").expect("a fixture tile");
            // (a tile of fields and woods has none)
            let Some(decoded) = decode(*tile, &bytes, &Cancel::new()).expect("decoded") else { continue };
            assert!(!decoded.extruded);
            for v in &decoded.vertices {
                let a = f64::from(v.tint[3]) / 255.0;
                blended.insert(std::array::from_fn::<u8, 3, _>(|i| (f64::from(v.tint[i]) * a + f64::from(CLEAR_RGB[i]) * (1.0 - a)).round() as u8));
            }
        }
        let near = |p: [u8; 4]| blended.iter().any(|c| (0..3).all(|i| c[i].abs_diff(p[i]) <= 1));
        let drawn: Vec<[u8; 4]> = image.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).filter(|p| p[..3] != CLEAR_RGB).collect();
        let exact = drawn.iter().filter(|&&p| near(p)).count();
        eprintln!("V4 at 12.1: {} px drawn, {exact} in a building's colour blended once ({} colours)", drawn.len(), blended.len());
        // (the rest: the edges of small buildings, blended with the clear colour, and the parts of
        // buildings mapped twice, blended twice)
        assert!(exact > 2_000 && exact * 5 > drawn.len() * 2, "{exact} of {}", drawn.len());
    }
}
