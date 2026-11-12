//! Shaded relief from terrarium tiles (item R12; doc 04 §12, §14.2; rows 3b, 3g, 3j of doc 03 §3):
//! today's `terrainLayers` / `hillshadeLayer` (`terrain-layer.ts:395-460`), a deck.gl TileLayer of
//! Mapzen's terrarium tiles each shaded on the CPU into a transparent image and drawn as a
//! BitmapLayer over the tile, under the map's water, borders, roads and runways.
//!
//! What today's app does, and so this layer:
//! - **When** (G:2649-2671). On the realistic basemap always (`terrainLayers({ …, opacity: terrain ?
//!   1.4 : 1, extensions: HAZE_FADED })`, after the ground and the rail ground, before the clouds);
//!   on the dark map (after the land, before the vector map) and on the satellite map (after the
//!   imagery) only with the *terrain* setting on. [`layer_for`] is that choice, made through
//!   `scene::hillshade::terrain_layers` itself; the scene builder hands over the setting
//!   ([`Input::terrain`]) and whether the scene has the relief at all ([`Input::shown`]: a bare
//!   `SceneInput::new` draws none, so other layers' tests are not drawn over).
//! - **Opacity** (`terrainLayers`): `HILLSHADE_OPACITY[basemap] · opacity · fade`, dark 1,
//!   satellite 0.45, realistic 0.6; `opacity` 1.4 on the realistic map with the terrain on;
//!   `fade = round(hillshadeFade(zoom) · 20) / 20`, `hillshadeFade = 1 − 0.4 · clamp01((zoom − 11) / 3)`
//!   (full out to zoom 11, 0.6 from 14 on). The zoom is this frame's camera's (today's
//!   `buildLayers` runs at each render; the steps of 1/20 keep the two the same but at a step).
//! - **Haze** (`HAZE_FADE`, realistic only): the fragment's alpha times the air's transmittance
//!   to the camera, `color.a *= rl_clear()` (`shaders/hillshade.wgsl`, with the realistic light of
//!   `lib/rlight.wgsl` as every realistic layer has it). On dark and satellite the tiles draw with
//!   the plain `bitmap` pipeline, deck's BitmapLayer as the imagery draws.
//! - **Tiles** (`hillshadeLayer`): [`io::TERRARIUM`] (the app's fetcher maps it to
//!   `…/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png`), `tileSize: 256`, zooms 0 to
//!   `HILLSHADE_MAX_ZOOM` 12 (deck's rule for 256-px tiles, `round(zoom + 1)` clamped, so a z12 tile
//!   is stretched from view zoom 11.5 on: most of the data is ~30 m, and deeper tiles would shade
//!   into terraces), `maxRequests: 8`, `maxCacheSize: 300` (refetching is the expensive part),
//!   deck's `best-available` refinement. One tile set serves both projections, as deck's one
//!   `terrain-hillshade` layer does.
//! - **Shading** ([`decode`], on the worker pool; `hillshadeTile`): the tile's PNG read to exact
//!   pixels, `decodeTerrarium`, `hillshade(heights, w, h, row → tileRowMetersPerPixel(z, y, row, h),
//!   { zFactor: reliefExaggeration(z) })` (`scene::hillshade`, transliterated from the TypeScript):
//!   Horn's 3 × 3 gradient lit from 315° at 45°, the sea floor raised to sea level, a shadow colour
//!   `(4, 8, 16)` up to alpha 135 and a highlight `(255, 244, 225)` up to alpha 40, flat ground fully
//!   transparent; RGBA not premultiplied, as `createImageBitmap(…, { premultiplyAlpha: "none" })`
//!   hands it over.
//! - **Each tile a BitmapLayer** over its bounding box with `_imageCoordinateSystem: CARTESIAN` and
//!   `parameters: SURFACE` (culled, depth-tested against the globe, no depth written): the imagery's
//!   [`tile_mesh`] in the tile's frame, Mercator-correct on the globe, one quad on the flat map; the
//!   texture with deck's mip chain and sampler ([`upload_rgba8`]); BitmapLayer's fragment
//!   (`tintColor` white, no desaturation, no transparent colour) with the layer's opacity; tiles
//!   drawn coarse to fine.
//!
//! When the layer leaves today's list (the terrain turned off on dark or satellite) it drops its
//! tiles, as deck finalizes a layer that leaves the list; across basemaps where it stays it keeps
//! them (one layer id, `terrain-hillshade`). It is [`Layer::pending`] only while it draws.
//!
//! Deviations:
//! - A tile the server has nothing for (404), or whose image cannot be read, leaves its coarser
//!   ancestor drawn there (the tile runtime's rule, as the imagery's); deck counts it loaded and
//!   leaves a hole. Flat ground is transparent either way, so only a tile in the hills that fails
//!   shows the difference.
//! - The PNG is read by the `image` crate, not the browser; both give the exact pixels, as
//!   `pixels()`'s `colorSpaceConversion: "none"` asks.
//! - Terrain (stage 9): deck drapes the relief over the 3D surface; this layer is
//!   [`Drape::Cover`](super::Drape::Cover) but draws only in the map pass until the terrain items
//!   give it a cover pass (with `cover = 1` the haze is then 1, as `rl_clear` has it).

use bytemuck::Zeroable;
use rustradar_core::geo::Basemap;
use rustradar_scene::hillshade::{
    self as relief, Extension, HILLSHADE_MAX_ZOOM, HILLSHADE_STYLE, HillshadeLayer, HillshadeStyle, TerrainOptions,
    decode_terrarium, relief_exaggeration, terrain_layers, tile_row_meters_per_pixel,
};
use rustradar_scene::tiles::TilesetOptions;

use super::{ByteSize, DeckSelect, DrawCx, Init, Layer, Pass, PrepareCx, Source, TileIndex, TileSet, TileSetOptions};
use crate::frame::FrameSlot;
use crate::gpu::Gpu;
use crate::io;
use crate::jobs::Cancel;
use crate::look::{LightUniform, Look};
use crate::pipelines::bitmap::{self, Bitmap, Params, StyleUniform, Vertex, tile_cells, tile_mesh, upload_rgba8};
use crate::pipelines::fill::Mesh;
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/hillshade.wgsl`: the realistic map's relief, faded by the haze.
pub const SHADER: &str = include_str!("../shaders/hillshade.wgsl");

/// The bind group of the image, its sampler and its style in the realistic pipeline (after the
/// light's, group 2).
pub const HAZE_IMAGE_GROUP: u32 = 3;

/// The hillshade's tiles for deck's rule (`hillshadeLayer`): 256 px, zooms 0 to 12.
pub const HILLSHADE_TILES: TilesetOptions = TilesetOptions::new(relief::TILE_SIZE as f64, 0.0, HILLSHADE_MAX_ZOOM as f64);

/// The hillshade's strength on the realistic map with the 3D terrain on (G:2665: `terrain ? 1.4 : 1`).
pub const TERRAIN_OPACITY: f64 = 1.4;

/// What the scene builder hands this layer (`SceneInput::hillshade`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Input {
    /// Whether the scene has the relief at all: the app's scene builder always sets it (the layer
    /// works out the basemap's rule, [`layer_for`]); off in a scene made with `SceneInput::new`.
    pub shown: bool,
    /// The *terrain* setting: the relief on dark and satellite, and stronger on realistic.
    pub terrain: bool,
}

/// The hillshade today's `buildLayers` puts in the list for `basemap` with the *terrain* setting
/// at a view `zoom` (`flat`: the flat projection), or none (G:2649-2671): `terrainLayers`' one
/// layer, its opacity and extensions as each branch passes them.
pub fn layer_for(basemap: Basemap, terrain: bool, flat: bool, zoom: f64) -> Option<HillshadeLayer> {
    let options = match basemap {
        // "and always in relief: the lie of the land"
        Basemap::Realistic => TerrainOptions {
            flat,
            zoom,
            basemap,
            opacity: if terrain { TERRAIN_OPACITY } else { 1.0 },
            extensions: vec![Extension::HazeFade],
        },
        Basemap::Dark | Basemap::Satellite if terrain => TerrainOptions { flat, zoom, basemap, ..TerrainOptions::default() },
        Basemap::Dark | Basemap::Satellite => return None,
    };
    terrain_layers(&options).into_iter().next()
}

// ------------------------------------------------------------------ the tiles

/// A tile shaded on the worker pool: RGBA8, not premultiplied, rows top (north) first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shaded {
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
}

impl ByteSize for Shaded {
    fn byte_size(&self) -> usize {
        self.rgba.len()
    }
}

/// The decode step, on the worker pool (`hillshadeTile`): a terrarium tile's image to its exact
/// pixels, heights (`decodeTerrarium`) and the hillshade of them at the tile's zoom and rows.
/// `None` for no bytes; an error for bytes that are no image, or once cancelled.
pub fn decode(tile: TileIndex, bytes: &[u8], cancel: &Cancel) -> Result<Option<Shaded>, String> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    let image = image::load_from_memory(bytes).map_err(|e| format!("terrarium tile: {e}"))?.into_rgba8();
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    let (width, height) = image.dimensions();
    let heights = decode_terrarium(image.as_raw(), None);
    let style = HillshadeStyle { z_factor: relief_exaggeration(f64::from(tile.z)), ..HILLSHADE_STYLE };
    let rows = height as usize;
    let rgba = relief::hillshade(&heights, width as usize, rows, |row| tile_row_meters_per_pixel(tile.z, tile.y, row, rows), &style, None);
    Ok(Some(Shaded { size: [width, height], rgba }))
}

/// A shaded tile on the GPU: the texture with its mip chain, bound for each pipeline.
pub struct TileTexture {
    /// For the `bitmap` pipeline (dark and satellite).
    plain: bitmap::Texture,
    /// For the realistic pipeline: the image, the sampler and the layer's style (group 3).
    haze: wgpu::BindGroup,
    /// Kept with the bind groups that sample it.
    _texture: wgpu::Texture,
}

// ------------------------------------------------------------------ the realistic pipeline

/// `shaders/hillshade.wgsl`'s pipeline: BitmapLayer with `HAZE_FADE`.
struct HazePipeline {
    light: LightUniform,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// `StyleUniform`, shared by every tile's group.
    style: wgpu::Buffer,
}

impl HazePipeline {
    fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let light = LightUniform::new(device);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hillshade"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(size_of::<StyleUniform>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let module = shaders::module(device, "hillshade", SHADER);
        let pipeline = build(
            init,
            &Desc {
                label: "hillshade",
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(Vertex::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state: DrawState::SURFACE,
                groups: &[light.layout(), &layout],
            },
        );
        // deck's DEFAULT_TEXTURE_PARAMETERS, as the bitmap pipeline's
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hillshade"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let style = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("hillshade style"),
                contents: bytemuck::bytes_of(&StyleUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        HazePipeline { light, pipeline, layout, sampler, style }
    }

    /// `view` bound with the sampler and the style (group 3).
    fn group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hillshade tile"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.style.as_entire_binding() },
            ],
        })
    }
}

/// The upload step, on the main thread: the shaded image as a texture with deck's mip chain,
/// bound for both pipelines.
fn upload(gpu: &Gpu, bitmap: &Bitmap, haze: &HazePipeline, shaded: &Shaded) -> TileTexture {
    let texture = upload_rgba8(gpu, "hillshade tile", shaded.size, &shaded.rgba, true);
    let view = texture.create_view(&Default::default());
    TileTexture { plain: bitmap.texture(&gpu.device, &view), haze: haze.group(&gpu.device, &view), _texture: texture }
}

// ------------------------------------------------------------------ the layer

/// What this frame draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drawn {
    /// The layer's opacity (`HILLSHADE_OPACITY[basemap] · opacity · fade`).
    pub opacity: f64,
    /// Faded by the haze (`HAZE_FADE`, the realistic map).
    pub haze: bool,
}

/// Rows 3b, 3g, 3j: `terrain-hillshade`.
pub struct Hillshade {
    tiles: TileSet<Shaded, TileTexture>,
    /// `SURFACE`: dark and satellite.
    bitmap: Bitmap,
    style: bitmap::Style,
    haze: HazePipeline,
    /// A tile's grid for each tile zoom 0–12 (`tile_mesh(tile_cells(z, 256))`), in tile units.
    meshes: Vec<Mesh<Vertex>>,
    /// This frame's layer, `None`: not drawn.
    drawn: Option<Drawn>,
    /// This frame's tiles in draw order, each with its frame.
    slots: Vec<(FrameSlot, TileIndex)>,
    /// The realistic light of the last frame drawn with the haze.
    look: Option<Look>,
}

impl Hillshade {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let layer = relief::hillshade_layer(1.0, &[]);
        let tiles = init.tile_set(
            vec![Source::new(io::TERRARIUM)],
            DeckSelect(HILLSHADE_TILES),
            TileSetOptions { max_requests: layer.max_requests, max_cache_size: Some(layer.max_cache_size), ..Default::default() },
            |tile, bytes, cancel| decode(tile, &bytes, cancel),
        );
        let bitmap = Bitmap::new(init, "hillshade", DrawState::SURFACE);
        let style = bitmap.style(device, &Params::default());
        let meshes = (0..=HILLSHADE_MAX_ZOOM)
            .map(|z| {
                let (vertices, indices) = tile_mesh(tile_cells(z, relief::TILE_SIZE as f64));
                Mesh::new(device, "hillshade tile", &vertices, &indices)
            })
            .collect();
        Hillshade { tiles, bitmap, style, haze: HazePipeline::new(init), meshes, drawn: None, slots: Vec::new(), look: None }
    }

    /// The tile set (for tests).
    pub fn tiles(&self) -> &TileSet<Shaded, TileTexture> {
        &self.tiles
    }

    /// What the last frame drew, `None`: nothing.
    pub fn drawn(&self) -> Option<Drawn> {
        self.drawn
    }

    /// The realistic light of the last frame drawn with the haze.
    pub fn look(&self) -> Option<&Look> {
        self.look.as_ref()
    }
}

impl Layer for Hillshade {
    fn name(&self) -> &'static str {
        "hillshade"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots.clear();
        self.look = None;
        let input = cx.scene.hillshade;
        let view = &cx.view.view;
        let layer = if input.shown { layer_for(cx.scene.basemap, input.terrain, !cx.view.is_globe(), view.zoom) } else { None };
        let Some(layer) = layer else {
            self.drawn = None;
            // deck drops a layer that leaves the list, and its tiles with it
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            return;
        };
        let drawn = Drawn { opacity: layer.opacity, haze: layer.extensions.contains(&Extension::HazeFade) };
        let params = Params { opacity: drawn.opacity as f32, ..Params::default() };
        if drawn.haze {
            cx.gpu.queue.write_buffer(&self.haze.style, 0, bytemuck::bytes_of(&StyleUniform::from(&params)));
            self.look = Some(self.haze.light.prepare(cx));
        } else {
            self.style.set(&cx.gpu.queue, &params);
        }
        self.drawn = Some(drawn);
        let (bitmap, haze) = (&self.bitmap, &self.haze);
        cx.update_tiles(&mut self.tiles, |gpu, _tile, shaded| upload(gpu, bitmap, haze, &shaded));
        for (tile, _) in self.tiles.visible() {
            self.slots.push((cx.tile_frame(tile, 0.0), tile));
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        let Some(drawn) = self.drawn else { return };
        if cx.pass != Pass::Map || self.slots.is_empty() {
            return;
        }
        let mesh = |tile: TileIndex| &self.meshes[tile.z.min(HILLSHADE_MAX_ZOOM) as usize];
        if drawn.haze {
            pass.set_pipeline(&self.haze.pipeline);
            self.haze.light.bind(pass);
            for &(slot, tile) in &self.slots {
                let Some(texture) = self.tiles.get(tile) else { continue };
                cx.bind_frame(pass, slot);
                pass.set_bind_group(HAZE_IMAGE_GROUP, &texture.haze, &[]);
                mesh(tile).draw(pass);
            }
        } else {
            for &(slot, tile) in &self.slots {
                let Some(texture) = self.tiles.get(tile) else { continue };
                self.bitmap.draw(pass, cx, slot, &texture.plain, &self.style, mesh(tile));
            }
        }
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Cover
    }

    fn pending(&self) -> bool {
        self.drawn.is_some() && self.tiles.loading()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::{ViewState, viewport_for};
    use rustradar_scene::hillshade::{hillshade, hillshade_fade, hillshade_opacity};
    use rustradar_scene::tiles::tile_indices;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchRequest, FetchResult, TileFetcher};
    use crate::layers::{earth, ground, sky};
    use crate::look::{self, RLight, twin};
    use crate::renderer::{MapRenderer, SceneInput};

    /// `RELIEF_VIEWS`' Alps (the fixtures' view for this item; X1): Zermatt and the Valais, the
    /// globe at zoom 9.45, its terrarium tiles z10.
    const ALPS: ViewState = ViewState { latitude: 46.0, longitude: 7.75, zoom: 9.45, pitch: 0.0, bearing: 0.0, position: None };
    /// The canvas the Alps' fixtures were fetched for.
    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// 2026-06-21 10:00 UTC (the reference pictures' summer morning).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// The hillshade's shadow colour.
    const SHADOW: [u8; 3] = HILLSHADE_STYLE.shadow;

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// The scene of `view` on `basemap`, the relief `shown` or not, with the *terrain* setting.
    fn scene(view: ViewState, size: [f64; 2], basemap: Basemap, shown: bool, terrain: bool) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, basemap);
        scene.local_now_ms = MIDSUMMER;
        scene.feed_now_ms = MIDSUMMER;
        scene.earth = earth::Input::SHOWN;
        if basemap == Basemap::Realistic {
            scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
            scene.ground = ground::Input::SHOWN;
        }
        scene.vector_map = crate::layers::vector_map::Input { shown: basemap == Basemap::Dark };
        scene.hillshade = Input { shown, terrain };
        scene
    }

    /// A fetcher with every request noted.
    struct Noted<F> {
        tiles: F,
        asked: Mutex<Vec<(String, TileIndex)>>,
    }

    impl<F: TileFetcher> TileFetcher for Noted<F> {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            self.asked.lock().unwrap().push((request.source.to_string(), request.tile));
            self.tiles.fetch(request)
        }
    }

    impl<F> Noted<F> {
        fn terrarium(&self) -> Vec<TileIndex> {
            self.asked.lock().unwrap().iter().filter(|(s, _)| s == io::TERRARIUM).map(|(_, t)| *t).collect()
        }
    }

    /// Terrarium pixels for heights in whole metres (`height + 32768 = R·256 + G`, B 0).
    fn terrarium_png(width: u32, height: u32, metres: impl Fn(u32, u32) -> i32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            let v = (metres(x, y) + 32768) as u32;
            image::Rgb([(v / 256) as u8, (v % 256) as u8, 0])
        });
        let mut png = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("a PNG");
        png
    }

    /// Every terrarium tile one plane falling 40 m a pixel to the east and to the south (a slope
    /// facing away from the light in the north-west), every Esri tile one plain colour, nothing
    /// else.
    struct Slope;

    const PLAIN: [u8; 3] = [200, 186, 160];

    fn slope_png() -> Vec<u8> {
        terrarium_png(256, 256, |x, y| 21_000 - 40 * x as i32 - 40 * y as i32)
    }

    impl TileFetcher for Slope {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            match &*request.source {
                io::TERRARIUM => io::answered(Ok(Some(slope_png()))),
                io::ESRI => {
                    let image = image::RgbImage::from_pixel(4, 4, image::Rgb(PLAIN));
                    let mut png = Vec::new();
                    image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("a PNG");
                    io::answered(Ok(Some(png)))
                }
                _ => io::answered(Ok(None)),
            }
        }
    }

    // ------------------------------------------------------------------ the CPU side

    /// G:2649-2671 through `terrainLayers`: realistic always (0.6, 0.84 with the terrain, the
    /// haze), dark (1) and satellite (0.45) with the terrain only, each times the zoom's fade in
    /// steps of 1/20; the tile layer's own settings.
    #[test]
    fn the_layer_is_todays_on_each_basemap() {
        assert_eq!(layer_for(Basemap::Dark, false, false, 9.45), None);
        assert_eq!(layer_for(Basemap::Satellite, false, false, 9.45), None);
        let at = |basemap, terrain, zoom: f64| layer_for(basemap, terrain, zoom > 12.0, zoom).expect("drawn");
        let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
        for zoom in [1.9, 4.5, 9.45, 11.0] {
            assert!(close(at(Basemap::Dark, true, zoom).opacity, 1.0), "{zoom}");
            assert!(close(at(Basemap::Satellite, true, zoom).opacity, 0.45), "{zoom}");
            assert!(close(at(Basemap::Realistic, false, zoom).opacity, 0.6), "{zoom}");
            assert!(close(at(Basemap::Realistic, true, zoom).opacity, 0.6 * 1.4), "{zoom}");
        }
        // the fade: 1 − 0.4·clamp01((zoom − 11)/3), in twentieths
        assert!(close(hillshade_fade(12.5), 0.8) && close(at(Basemap::Dark, true, 12.5).opacity, 0.8));
        assert!(close(at(Basemap::Dark, true, 13.0).opacity, 0.75), "0.7333 to the twentieth");
        assert!(close(at(Basemap::Satellite, true, 13.5).opacity, 0.45 * 0.65));
        assert!(close(at(Basemap::Realistic, false, 15.5).opacity, 0.6 * 0.6));
        assert!(close(at(Basemap::Realistic, true, 20.0).opacity, 0.6 * 1.4 * 0.6));
        for basemap in [Basemap::Dark, Basemap::Satellite, Basemap::Realistic] {
            assert!(close(at(basemap, true, 9.0).opacity, hillshade_opacity(basemap) * if basemap == Basemap::Realistic { 1.4 } else { 1.0 }));
        }
        // only the realistic map's is faded by the haze
        assert_eq!(at(Basemap::Realistic, false, 9.0).extensions, vec![Extension::HazeFade]);
        assert!(at(Basemap::Dark, true, 9.0).extensions.is_empty() && at(Basemap::Satellite, true, 9.0).extensions.is_empty());
        let layer = at(Basemap::Dark, true, 9.0);
        assert_eq!((layer.id, layer.min_zoom, layer.max_zoom, layer.tile_size, layer.max_requests, layer.max_cache_size), ("terrain-hillshade", 0, 12, 256, 8, 300));
        assert_eq!(layer.url, "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png");
    }

    /// deck's rule for 256-px tiles, `round(zoom + 1)` clamped to 0–12: the Alps' twelve z10 tiles
    /// (the fixtures'), and z12 stretched on the flat map.
    #[test]
    fn the_tiles_are_decks_z0_to_12() {
        let mut alps = tile_indices(&viewport_for(&ALPS, SIZE), &HILLSHADE_TILES);
        alps.sort_by_key(|t| (t.x, t.y));
        let expected: Vec<TileIndex> = (532..=535).flat_map(|x| (363..=365).map(move |y| TileIndex { x, y, z: 10 })).collect();
        assert_eq!(alps, expected);
        let close_in = ViewState { zoom: 15.5, pitch: 0.0, ..ALPS };
        let tiles = tile_indices(&viewport_for(&close_in, SIZE), &HILLSHADE_TILES);
        assert!(!tiles.is_empty() && tiles.iter().all(|t| t.z == 12), "{tiles:?}");
        let far = ViewState { zoom: 1.9, ..ALPS };
        assert!(tile_indices(&viewport_for(&far, SIZE), &HILLSHADE_TILES).iter().all(|t| t.z == 3));
    }

    /// A fixture tile shaded on the pool is `hillshadeTile`'s image: its pixels decoded as
    /// terrarium, shaded with the zoom's exaggeration and each row's ground size; the Alps have
    /// deep shadow and faint light, the sea none, and bytes that are no image fail.
    #[test]
    fn a_tile_is_shaded_as_terrain_layer_ts_shades_it() {
        let tile = TileIndex { x: 533, y: 364, z: 10 };
        let bytes = std::fs::read(io::fixture_tiles_dir().join("terrarium/10/533/364.png")).expect("the fixture tile");
        let shaded = decode(tile, &bytes, &Cancel::new()).expect("decoded").expect("a picture");
        assert_eq!(shaded.size, [256, 256]);
        // the same steps by hand
        let image = image::load_from_memory(&bytes).expect("a PNG").into_rgba8();
        let heights: Vec<f32> = image.pixels().map(|p| (f64::from(p[0]) * 256.0 + f64::from(p[1]) + f64::from(p[2]) / 256.0 - 32768.0) as f32).collect();
        let z_factor = 2f64.powf((11.0 - 10.0) / 3.0);
        let style = HillshadeStyle { z_factor, ..HILLSHADE_STYLE };
        let rgba = hillshade(&heights, 256, 256, |row| tile_row_meters_per_pixel(10, 364, row, 256), &style, None);
        assert_eq!(shaded.rgba, rgba);
        // Zermatt's valleys: shadow and light, the shadow up to its 135
        let alphas = |colour: [u8; 3]| shaded.rgba.chunks_exact(4).filter(|p| p[..3] == colour).map(|p| p[3]).collect::<Vec<u8>>();
        let (shadow, light) = (alphas(SHADOW), alphas(HILLSHADE_STYLE.highlight));
        assert_eq!(shadow.len() + light.len(), 256 * 256);
        // (a slope facing the light shades `(1 − sin 45°) / 0.55` at most: 21 of the highlight's 40)
        assert_eq!((shadow.iter().max(), light.iter().max()), (Some(&135), Some(&21)));
        assert!(shadow.iter().filter(|&&a| a > 30).count() > 256 * 256 / 5, "deep shadow on a fifth of the tile at least");
        // the sea (and the plains) leave the map as it is
        let flat = terrarium_png(16, 16, |_, _| -40);
        let sea = decode(tile, &flat, &Cancel::new()).expect("decoded").expect("a picture");
        assert!(sea.rgba.chunks_exact(4).all(|p| p[3] == 0));
        assert_eq!(decode(tile, &[], &Cancel::new()), Ok(None));
        assert!(decode(tile, b"not a picture", &Cancel::new()).is_err());
    }

    // ------------------------------------------------------------------ on the GPU

    /// How much of the shadow colour a picture took over its base, per pixel: `(B − O)·(B − S)`
    /// and `(B − S)²` summed over the channels (a least-squares alpha when summed over pixels).
    fn shadow_terms(base: [u8; 4], over: [u8; 4]) -> (f64, f64) {
        (0..3).fold((0.0, 0.0), |(n, d), c| {
            let (b, o, s) = (f64::from(base[c]), f64::from(over[c]), f64::from(SHADOW[c]));
            (n + (b - o) * (b - s), d + (b - s) * (b - s))
        })
    }

    /// `rl_clear` at the ground under `xy` (the CPU's light, `look::twin`).
    fn clear_at(look: &Look, light: &RLight, view: &ViewFrame, xy: [f64; 2]) -> f64 {
        let [lon, lat] = view.viewport.unproject(xy, 0.0);
        twin::clear(look, &twin::vary(light, view, [lon, lat, 0.0]))
    }

    /// The opacity table, measured: one plane facing away from the light covers the Alps' view, so
    /// the relief is one shadow alpha everywhere (the CPU's, `a0`); over each basemap's own ground
    /// (the dark land, a plain picture tinted as the imagery is, the realistic land lit) the
    /// shadow colour takes `a0 · opacity` of each pixel: dark 1, satellite 0.45, realistic 0.6
    /// times the haze's transmittance there (and 0.84 with the terrain on), within 0.015 of it.
    #[test]
    fn a_slope_darkens_each_basemap_by_the_table() {
        let Some(gpu) = gpu() else { return };
        let size = [480.0, 320.0];
        let a0 = {
            let shaded = decode(TileIndex { x: 533, y: 364, z: 10 }, &slope_png(), &Cancel::new()).expect("decoded").expect("a picture");
            let centre = (128 * 256 + 128) * 4;
            assert_eq!(shaded.rgba[centre..centre + 3], SHADOW);
            // the plane is one shade but for the rows' ground size
            assert!(shaded.rgba.chunks_exact(4).all(|p| p[..3] == SHADOW && p[3].abs_diff(shaded.rgba[centre + 3]) <= 1));
            f64::from(shaded.rgba[centre + 3]) / 255.0
        };
        assert!((0.4..0.53).contains(&a0), "a0 {a0}");
        let view = ViewFrame::new(&ALPS, size, 1.0, 0.0);
        for (basemap, terrain, opacity) in [
            (Basemap::Dark, true, 1.0),
            (Basemap::Satellite, true, 0.45),
            (Basemap::Realistic, false, 0.6),
            (Basemap::Realistic, true, 0.84),
        ] {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(Arc::new(Slope), None);
            let base = render_with(&gpu, &mut renderer, &scene(ALPS, size, basemap, false, terrain), 1.0).expect("rendered");
            let over = render_with(&gpu, &mut renderer, &scene(ALPS, size, basemap, true, terrain), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            let name = format!("r12-slope-{basemap:?}{}.png", if terrain { "-terrain" } else { "" }).to_lowercase();
            over.save_png(&shots_dir().join(name)).expect("png");
            let sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
            let look = look::frame_look(&sky, &ALPS, size, MIDSUMMER);
            let light = RLight::new(&look, &view, 0.0);
            // the middle of the picture, clear of the tiles' edges' filtering
            let (mut num, mut den) = (0.0, 0.0);
            for y in (40..280).step_by(3) {
                for x in (60..420).step_by(3) {
                    let (n, d) = shadow_terms(base.pixel(x, y), over.pixel(x, y));
                    let haze = if basemap == Basemap::Realistic { clear_at(&look, &light, &view, [f64::from(x) + 0.5, f64::from(y) + 0.5]) } else { 1.0 };
                    num += n;
                    den += d * a0 * opacity * haze;
                }
            }
            let ratio = num / den;
            eprintln!(
                "{basemap:?} terrain {terrain}: base {:?} over {:?}, alpha {:.4} expected {:.4} (a0 {a0:.4}, haze at the middle {:.4})",
                base.pixel(240, 160),
                over.pixel(240, 160),
                ratio * a0 * opacity,
                a0 * opacity,
                clear_at(&look, &light, &view, [240.5, 160.5])
            );
            assert!((ratio - 1.0).abs() < 0.015 / (a0 * opacity), "{basemap:?}: {ratio}");
        }
    }

    /// Today's list (G:2649-2671): on dark and satellite the relief only with the terrain on (no
    /// terrarium tile asked for without), on realistic always; a layer that leaves the list drops
    /// its tiles (asked for again when it comes back), and a scene made with `SceneInput::new`
    /// has none.
    #[test]
    fn the_relief_is_asked_for_only_where_today_draws_it() {
        let Some(gpu) = gpu() else { return };
        let size = [400.0, 300.0];
        let fixtures = Arc::new(Noted { tiles: DirTiles::fixtures(), asked: Mutex::new(Vec::new()) });
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fixtures.clone(), None);
        // (the same scenes without the relief, on a renderer of their own)
        let mut plain = MapRenderer::new(&gpu, FORMAT);
        plain.set_tiles(Arc::new(DirTiles::fixtures()), None);
        let wanted = tile_indices(&viewport_for(&ALPS, size), &HILLSHADE_TILES).len();
        assert!(wanted >= 4, "{wanted}");
        // no earth and no map under it (but the satellite map's imagery): how many terrarium
        // tiles were asked for so far, and whether the relief changed the picture
        let mut asked = |basemap, shown, terrain| {
            let scene = |hillshade| {
                let mut scene = SceneInput::new(ALPS, size, 1.0, basemap);
                scene.local_now_ms = MIDSUMMER;
                scene.hillshade = hillshade;
                scene
            };
            let image = render_with(&gpu, &mut renderer, &scene(Input { shown, terrain }), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            let drawn = image != render_with(&gpu, &mut plain, &scene(Input::default()), 1.0).expect("rendered");
            (fixtures.terrarium().len(), drawn)
        };
        assert_eq!(asked(Basemap::Dark, true, false), (0, false));
        assert_eq!(asked(Basemap::Satellite, true, false), (0, false));
        assert_eq!(asked(Basemap::Realistic, false, true), (0, false));
        assert_eq!(asked(Basemap::Dark, true, true), (wanted, true));
        // the same layer on another basemap keeps its tiles
        assert_eq!(asked(Basemap::Satellite, true, true), (wanted, true));
        assert_eq!(asked(Basemap::Realistic, true, false), (wanted, true));
        // the terrain off on dark: gone, and its tiles with it
        assert_eq!(asked(Basemap::Dark, true, false), (wanted, false));
        assert_eq!(asked(Basemap::Dark, true, true), (2 * wanted, true));
    }

    /// `RELIEF_VIEWS`' Alps from the committed fixtures (`RELIEF_VIEWS`: OpenFreeMap, Esri,
    /// terrarium z10, all there) on each basemap as the app shows it, the terrain on for dark and
    /// satellite (realistic draws its relief without): the relief shows on each (a share of the
    /// map darkened, lit slopes lightened), and over the same pixels the share of the shadow
    /// colour each basemap takes follows the opacity table (satellite 0.45 of dark's, realistic 0.6
    /// of dark's in clear air: measured over strongly shaded bare land, divided by the haze's
    /// transmittance there). The shots are `target/shots/r12-alps-*.png`, each beside the same
    /// view without the relief.
    #[test]
    fn the_alps_in_relief_on_every_basemap() {
        let Some(gpu) = gpu() else { return };
        let view = ViewFrame::new(&ALPS, SIZE, 1.0, 0.0);
        let sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        let look = look::frame_look(&sky, &ALPS, SIZE, MIDSUMMER);
        let light = RLight::new(&look, &view, 0.0);
        let mut shots: Vec<(Basemap, Image, Image)> = Vec::new();
        for (basemap, terrain) in [(Basemap::Dark, true), (Basemap::Satellite, true), (Basemap::Realistic, false)] {
            let fixtures = Arc::new(Noted { tiles: DirTiles::fixtures(), asked: Mutex::new(Vec::new()) });
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(fixtures.clone(), None);
            let base = render_with(&gpu, &mut renderer, &scene(ALPS, SIZE, basemap, false, terrain), 1.0).expect("rendered");
            let over = render_with(&gpu, &mut renderer, &scene(ALPS, SIZE, basemap, true, terrain), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            assert!(fixtures.tiles.misses().is_empty(), "the fixtures lack {:?}", fixtures.tiles.misses());
            let mut asked = fixtures.terrarium();
            asked.sort_by_key(|t| (t.z, t.x, t.y));
            asked.dedup();
            assert!(asked.len() == 12 && asked.iter().all(|t| t.z == 10), "{asked:?}");
            let name = format!("{basemap:?}").to_lowercase();
            over.save_png(&shots_dir().join(format!("r12-alps-{name}.png"))).expect("png");
            base.save_png(&shots_dir().join(format!("r12-alps-{name}-without.png"))).expect("png");
            // the relief shows: darker on the slopes away from the light, lighter (dark map) on
            // those facing it
            let (mut darker, mut lighter) = (0usize, 0usize);
            for (b, o) in base.rgba.chunks_exact(4).zip(over.rgba.chunks_exact(4)) {
                let change: i32 = (0..3).map(|c| i32::from(o[c]) - i32::from(b[c])).sum();
                darker += usize::from(change <= -6);
                lighter += usize::from(change >= 3);
            }
            let total = base.rgba.len() / 4;
            eprintln!("{basemap:?}: {:.1} % darker, {:.1} % lighter", 100.0 * darker as f64 / total as f64, 100.0 * lighter as f64 / total as f64);
            assert!(darker * 6 > total, "{basemap:?}: {darker} of {total} darker");
            if basemap == Basemap::Dark {
                assert!(lighter * 20 > total, "{lighter} of {total} lighter");
            }
            shots.push((basemap, base, over));
        }
        // the share of the shadow colour a pixel took (least squares over its channels)
        let alpha = |base: &Image, over: &Image, x: u32, y: u32| {
            let (n, d) = shadow_terms(base.pixel(x, y), over.pixel(x, y));
            n / d
        };
        let [(_, dark_base, dark), (_, sat_base, sat), (_, real_base, real)] = &shots[..] else { unreachable!() };
        let (mut satellite, mut realistic) = (Vec::new(), Vec::new());
        for y in 0..SIZE[1] as u32 {
            for x in 0..SIZE[0] as u32 {
                // bare land on the dark map (nothing of the vector map over the relief)
                if dark_base.pixel(x, y)[..3] != earth::LAND[..3] {
                    continue;
                }
                let a = alpha(dark_base, dark, x, y);
                if a < 0.25 {
                    continue;
                }
                satellite.push(alpha(sat_base, sat, x, y) / a);
                let haze = clear_at(&look, &light, &view, [f64::from(x) + 0.5, f64::from(y) + 0.5]);
                realistic.push(alpha(real_base, real, x, y) / (a * haze));
            }
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        assert!(satellite.len() > 5000, "{} strongly shaded bare pixels", satellite.len());
        let (sat_ratio, real_ratio) = (median(&mut satellite), median(&mut realistic));
        eprintln!("{} pixels: satellite / dark {sat_ratio:.3}, realistic / dark / haze {real_ratio:.3}", satellite.len());
        assert!((sat_ratio - 0.45).abs() < 0.04, "satellite / dark {sat_ratio}");
        assert!((real_ratio - 0.6).abs() < 0.05, "realistic / dark {real_ratio}");
    }

    /// Today's picture of V3 on the realistic map (`docs/port/reference/realistic-v3-1440x900.png`)
    /// has the hillshade over Stockholm's low hills (z10 tiles, as here): where ours darkens or
    /// lightens the ground by so much, today's picture is that much darker or lighter than our
    /// ground without it. Over the map clear of today's panels, by how much our relief changes a
    /// pixel's luma (bins of 20 000 pixels or more), today's picture less our ground without the
    /// relief changes by the same within 0.7 levels (both against the pixels the relief leaves as
    /// they are; today's sampled half way between its pixels at T1's offset, 4.5 px up and 0.5 px
    /// left; the names, roads and water ours draws apart left out).
    #[test]
    fn v3_has_todays_relief() {
        let Some(gpu) = gpu() else { return };
        const V3: ViewState = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };
        let size = [1440.0, 900.0];
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(Arc::new(DirTiles::fixtures()), None);
        let without = render_with(&gpu, &mut renderer, &scene(V3, size, Basemap::Realistic, false, false), 1.0).expect("rendered");
        let with = render_with(&gpu, &mut renderer, &scene(V3, size, Basemap::Realistic, true, false), 1.0).expect("rendered");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        with.save_png(&shots_dir().join("r12-v3-realistic.png")).expect("png");
        without.save_png(&shots_dir().join("r12-v3-realistic-without.png")).expect("png");
        let today = {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference/realistic-v3-1440x900.png");
            let rgba = image::open(&path).expect("today's picture").into_rgba8();
            Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
        };
        let luma = |p: [u8; 4]| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]);
        let theirs = |x: u32, y: u32| (luma(today.pixel(x, y - 4)) + luma(today.pixel(x + 1, y - 4)) + luma(today.pixel(x, y - 5)) + luma(today.pixel(x + 1, y - 5))) / 4.0;
        // (our relief, today's less our ground) per pixel of the map
        let mut pairs = Vec::new();
        for y in 80..880 {
            for x in 360..1370 {
                let ours = luma(without.pixel(x, y));
                let beyond = theirs(x, y) - ours;
                // names, roads and water ours lacks or draws apart: not the relief's
                if beyond.abs() <= 25.0 {
                    pairs.push((luma(with.pixel(x, y)) - ours, beyond));
                }
            }
        }
        let mean_in = |lo: f64, hi: f64| {
            let bin: Vec<&(f64, f64)> = pairs.iter().filter(|(relief, _)| (lo..hi).contains(relief)).collect();
            let n = bin.len() as f64;
            (bin.len(), bin.iter().map(|p| p.0).sum::<f64>() / n, bin.iter().map(|p| p.1).sum::<f64>() / n)
        };
        let (_, _, level) = mean_in(-0.5, 0.5);
        let mut checked = 0;
        for (lo, hi) in [(-4.0, -2.5), (-2.5, -1.5), (-1.5, -0.5), (0.5, 1.5)] {
            let (n, ours, theirs) = mean_in(lo, hi);
            eprintln!("relief {lo} to {hi}: {n} pixels, ours {ours:.2}, today's {:.2}", theirs - level);
            if n >= 20_000 {
                assert!((theirs - level - ours).abs() <= 0.7, "relief {lo} to {hi}: ours {ours}, today's {}", theirs - level);
                checked += 1;
            }
        }
        assert!(checked >= 3, "{checked} bins");
    }
}
