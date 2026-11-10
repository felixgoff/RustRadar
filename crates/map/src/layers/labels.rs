//! Place names and airport codes (item B4; doc 01 §5.9; row 18 of doc 03 §3): today's
//! `label-tiles` and `labels` layers (G:610-752, 2743-2754). The rules (which features are labels,
//! their text, size, colour and priority, the horizon filter, the marked airports, deck's collision
//! filter) are `rustradar_scene::labels_map`'s; this file fetches the tiles, lays the text out,
//! runs the collision filter for each frame's camera and draws the text with the SDF pipeline.
//!
//! What today's app does, and so this layer:
//! - **When.** On every basemap, while the label setting is on (`tileUrl && labels && fontsReady`;
//!   the fonts are embedded here, so always ready): [`Input::shown`]. The URL is the app's
//!   fetcher's business, as for the vector map.
//! - **Tiles** (`label-tiles`): a TileLayer of its own over OpenFreeMap, `minZoom 0`, `maxZoom 14`,
//!   `tileSize 512` (`scene::tiles::VECTOR_TILES`, deck's rule), TileLayer's defaults otherwise
//!   (`maxRequests` 6, a cache of five times the selection). It draws nothing itself: each tile is
//!   decoded on the worker pool into its labels (`labels_map::decode`: the `place` and
//!   `aerodrome_label` layers, `isLabel`). One tile set serves both projections.
//! - **Which labels** (`onViewportLoad`, `labelsInView`, `withoutMarked`). Whenever every tile
//!   selected for the view has settled (loaded, empty or failed) and the selection or the loading
//!   state changed since the last frame, the labels become the selected tiles' labels in selection
//!   order (deck's `tileset.selectedTiles`), and until then the previous ones stay: there is no
//!   refinement, so labels never come from a parent or a child tile. Nothing is de-duplicated,
//!   although OpenMapTiles' `place` layer carries a buffer of about a tile round each tile, so a
//!   place sits in the labels of up to nine neighbouring tiles (doc 01 §5.9 says one): the copies
//!   share position and priority, so in the collision map the last drawn takes every texel and the
//!   others fade to 0, and one copy is drawn, as today. Of them, at zoom ≤ 6 only those
//!   facing the camera (`labels_map::InView`, with today's cache by the rounded centre) and never an
//!   airport with a marker of its own ([`Input::marked`]). Both filters read the scene refresh's
//!   view, as `buildLayers` does; the collision filter reads each frame's camera, as deck's effect
//!   does.
//! - **Collision** (`CollisionFilterExtension`, group `"labels"`). deck's collision map, done on the
//!   CPU as deck does it on the GPU (`labels_map::collision_fades`; see that module for the
//!   details): every label's glyph quads at `sizeScale` 1.3, whatever the label size setting, drawn
//!   in data order into a map at half the canvas's resolution with the depth test sorting by
//!   priority, then each label's fade from the 5 × 5 patch round its anchor. A label under a fade of
//!   0.0001 is not drawn and not picked; a fade below 1 multiplies its fill colour's alpha (not the
//!   halo's). It is recomputed whenever the camera, the canvas, the labels or the label size change,
//!   as deck redraws its map whenever the viewport changes.
//! - **Text** (`labels` TextLayer): Overpass at weight 600 from an SDF atlas (64 px, buffer 6),
//!   size `labelSize` px × `sizeScale = labelScale` ([`Input::label_scale`]), colour `labelColor`,
//!   halo 3 SDF px in `[6,12,19,220]`, anchored in the middle and centred vertically on the point
//!   at altitude 0, billboards, `LABEL_PARAMETERS` (no cull, depth `always`, no depth write).
//! - **Picking.** Every label drawn is pickable (a click on an airport's code focuses that airport,
//!   G:3732-3734; on a place it does nothing, but it is still what the pointer is over).
//!   [`Input::drawn`] is a handle the layer fills each frame with the labels it draws, each with its
//!   anchor and the rectangle round its glyph quads as drawn; [`Drawn::picks`] makes the pick list
//!   (rectangles, depth `always`, keyed by the airport's code or `None` for a place).
//!
//! Deviations from today's app, all invisible or nearly so:
//! - deck picks a label where its glyph quads are (each glyph's whole atlas frame); here the
//!   rectangle round them, which differs only at the ends of a row of letters of unequal height;
//! - the collision fade is applied to the colour's 8-bit alpha (deck multiplies the float);
//! - on the flat map a label is drawn on the world copy nearest the view (deck: the one its
//!   longitude names, which only matters across the antimeridian above zoom 12);
//! - the plan's whole-box greedy declutter (plan 1.4.10, section 6 item 17) is not used for these
//!   labels: deck's anchor-patch test is ported instead, so the density is today's (labels may
//!   overlap where today's do, "DENMARK" over "Copenhagen" at V2); the plan's 150 ms fade is not
//!   deck's and is not reproduced either.
//!
//! The scene builder fills [`Input`] at each scene refresh: `shown` from the label setting,
//! `label_scale` from the label size setting, `marked` from the focused airport and the selected
//! flight's route, and the same [`Drawn`] handle each time (kept by whoever picks).

use std::sync::{Arc, Mutex, MutexGuard};

use glam::DVec4;
use rustradar_scene::declutter::Rect;
use rustradar_scene::labels_map::{self, CollisionMap, InView, MapLabel, Placing};
use rustradar_scene::pick::{DepthTest, PickList};
use rustradar_scene::text::{Face, FontSettings, GlyphAtlas, LaidText, TextLayout};
use rustradar_scene::tiles::VECTOR_TILES;

use super::{DeckSelect, DrawCx, Drape, Init, Layer, Pass, PrepareCx, Source, TileSet, TileSetOptions};
use crate::frame::ViewFrame;
use crate::io;
use crate::pipelines::icon::focal_w;
use crate::pipelines::text::{AtlasTexture, BackgroundStyle, Glyph, Label, TextBatch, TextPipeline, TextStyle};
use crate::state::DrawState;

/// deck TileLayer's `maxRequests` default, which `label-tiles` keeps.
pub const MAX_REQUESTS: usize = 6;

/// The label size setting's default (`labelScale`, doc 06 §23 `labelSize: 1`).
pub const DEFAULT_LABEL_SCALE: f64 = 1.0;

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone)]
pub struct Input {
    /// Whether the labels are drawn: the label setting (G:2743). Off in a scene made with
    /// `SceneInput::new`.
    pub shown: bool,
    /// The label size setting (`labelScale`): the text's `sizeScale`. The collision filter tests
    /// every label at 1.3 whatever this is.
    pub label_scale: f64,
    /// The airport codes that have a marker of their own (the focused airport, the selected
    /// flight's origin and destination, G:2744-2746): their labels are left out.
    pub marked: Vec<String>,
    /// Where the layer writes what it draws each frame (for picking).
    pub drawn: Drawn,
}

impl Default for Input {
    fn default() -> Self {
        Input { shown: false, label_scale: DEFAULT_LABEL_SCALE, marked: Vec::new(), drawn: Drawn::default() }
    }
}

impl Input {
    /// The labels drawn at the default size, no airport marked.
    pub fn shown() -> Input {
        Input { shown: true, ..Input::default() }
    }
}

/// One label the layer draws this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub text: String,
    /// The airport's code, `None` for a place.
    pub iata: Option<String>,
    /// Where it is, degrees.
    pub lon: f64,
    pub lat: f64,
    /// Its anchor on the screen, CSS pixels from the canvas's top left.
    pub anchor: [f64; 2],
    /// NDC depth of the anchor (`Viewport::project`'s third component).
    pub depth: f64,
    /// The rectangle round its glyph quads as drawn (at the label size setting), CSS pixels.
    pub rect: Rect,
    /// deck's collision fade, `(0.0001, 1]`.
    pub fade: f64,
    pub priority: f64,
    /// `labelColor`, before the fade.
    pub color: [u8; 4],
    /// `labelSize`, CSS pixels before the label size setting.
    pub size: f64,
}

/// A handle to the labels the layer drew in its last frame, in draw order. Clones share it.
#[derive(Debug, Clone, Default)]
pub struct Drawn(Arc<Mutex<Vec<Shown>>>);

impl Drawn {
    fn lock(&self) -> MutexGuard<'_, Vec<Shown>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The labels drawn, in draw order (later on top).
    pub fn labels(&self) -> Vec<Shown> {
        self.lock().clone()
    }

    /// The labels drawn as a pick list in draw order: each its rectangle at its depth, under
    /// `LABEL_PARAMETERS` (depth `always`), keyed by its airport's code (`None` for a place).
    pub fn picks(&self) -> PickList<Option<String>> {
        let mut list = PickList::new();
        for shown in self.lock().iter() {
            list.rect(shown.iata.clone(), shown.rect, shown.depth, DepthTest::Always);
        }
        list
    }

    fn set(&self, labels: Vec<Shown>) {
        *self.lock() = labels;
    }
}

/// What the last placement was worked out for; the same again needs no new one.
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    view: rustradar_scene::camera::ViewState,
    size: [f64; 2],
    map_scale: f64,
    common_centre: glam::DVec3,
    version: u64,
    data: Vec<usize>,
    label_scale: f64,
    generation: u64,
}

/// Row 18: `label-tiles`, `labels`.
pub struct Labels {
    tiles: TileSet<Vec<MapLabel>>,
    /// The tile set's loading state and frame number at the last frame (`onViewportLoad`).
    was_loaded: bool,
    frame_number: Option<u64>,
    /// `labelFeatures`: the selected tiles' labels as of the last viewport load.
    features: Vec<MapLabel>,
    /// Moves on with each new `features`.
    version: u64,
    in_view: InView,
    /// Each feature's text laid out in [`Self::atlas`] (`None` until it is needed), valid for
    /// [`Self::laid_generation`].
    laid: Vec<Option<LaidText>>,
    laid_generation: u64,
    atlas: GlyphAtlas,
    texture: AtlasTexture,
    pipeline: TextPipeline,
    batch: TextBatch,
    collision: CollisionMap,
    placed: Option<Placed>,
    /// Whether this frame draws.
    shown: bool,
}

impl Labels {
    pub fn new(init: &Init<'_>) -> Self {
        let tiles = init.tile_set(
            vec![Source::new(io::OPENFREEMAP)],
            DeckSelect(VECTOR_TILES),
            TileSetOptions { max_requests: MAX_REQUESTS, ..Default::default() },
            |tile, bytes, _cancel| Ok(Some(labels_map::decode(tile, &bytes))),
        );
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let texture = AtlasTexture::new(init.gpu, "label glyphs", &mut atlas);
        let pipeline = TextPipeline::new(init, "labels", DrawState::LABEL);
        let batch = pipeline.batch(&init.gpu.device, "labels", &texture);
        Labels {
            tiles,
            was_loaded: false,
            frame_number: None,
            features: Vec::new(),
            version: 0,
            in_view: InView::default(),
            laid: Vec::new(),
            laid_generation: 0,
            atlas,
            texture,
            pipeline,
            batch,
            collision: CollisionMap::new(),
            placed: None,
            shown: false,
        }
    }

    /// The label tiles (for tests).
    pub fn tiles(&self) -> &TileSet<Vec<MapLabel>> {
        &self.tiles
    }

    /// `labelFeatures` (for tests).
    pub fn features(&self) -> &[MapLabel] {
        &self.features
    }

    /// `onLabelTiles`: the selected tiles' labels, in selection order.
    fn viewport_loaded(&mut self) {
        self.features = self.tiles.selected().iter().filter_map(|&t| self.tiles.get(t)).flatten().cloned().collect();
        self.version += 1;
        self.in_view.reset();
        self.laid = vec![None; self.features.len()];
        self.atlas.tick();
    }

    /// Lays out the text of `data`'s features not laid out yet, again from scratch whenever the
    /// atlas packs itself anew (which moves the glyphs).
    fn lay_out(&mut self, data: &[usize]) {
        for _ in 0..4 {
            let generation = self.atlas.generation();
            if generation != self.laid_generation {
                self.laid.iter_mut().for_each(|l| *l = None);
                self.laid_generation = generation;
            }
            for &i in data {
                if self.laid[i].is_none() {
                    self.laid[i] = Some(self.atlas.layout(Face::LABEL, &self.features[i].text, &TextLayout::DEFAULT));
                }
            }
            if self.atlas.generation() == generation {
                return;
            }
        }
    }
}

/// A world x difference moved into `[−256, 256)`.
fn wrap_world_x(dx: f64) -> f64 {
    const WORLD: f64 = 512.0;
    dx - WORLD * ((dx + WORLD / 2.0) / WORLD).floor()
}

/// Where a label at `[lon, lat]` (altitude 0) is drawn in `view`: its anchor in CSS pixels from
/// the top left, the focal depth's clip w over its own (a billboard's pixel offsets shrink by
/// that), and its NDC depth; `None` when it is not in front of the camera.
fn place_anchor(view: &ViewFrame, focal: f64, [lon, lat]: [f64; 2]) -> Option<([f64; 2], f64, f64)> {
    let mut p = view.viewport.project_position([lon, lat, 0.0]);
    let vp = &*view.viewport;
    if !view.is_globe() {
        p.x = vp.center.x + wrap_world_x(p.x - vp.center.x);
    }
    let clip = vp.view_projection_matrix * DVec4::new(p.x, p.y, p.z, 1.0);
    // (NaN is not in front either)
    if clip.w.is_nan() || clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(([(ndc.x + 1.0) / 2.0 * vp.width, (1.0 - ndc.y) / 2.0 * vp.height], focal / clip.w, ndc.z))
}

impl Layer for Labels {
    fn name(&self) -> &'static str {
        "labels"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let scene = cx.scene;
        let input = &scene.labels;
        self.shown = input.shown;
        if !self.shown {
            // deck finalizes `label-tiles` with its tiles; `labelFeatures` stays
            if !self.tiles.is_empty() {
                self.tiles.clear();
            }
            self.was_loaded = false;
            self.frame_number = None;
            self.placed = None;
            input.drawn.set(Vec::new());
            return;
        }
        cx.update_tiles(&mut self.tiles, |_, _, labels| labels);
        // `_updateTileset`: a viewport load when all selected tiles have settled and the loading
        // state or the selection changed
        let loaded = self.tiles.is_loaded();
        let frame_number = self.tiles.frame_number();
        if loaded && (loaded != self.was_loaded || Some(frame_number) != self.frame_number) {
            self.viewport_loaded();
        }
        self.was_loaded = loaded;
        self.frame_number = Some(frame_number);

        // `textLayer(withoutMarked(labelsInView(cLon, cLat, zoom), marked), labelScale)`
        let scene_view = cx.scene.view;
        let data = labels_map::without_marked(self.in_view.labels(&self.features, scene_view.longitude, scene_view.latitude, scene_view.zoom), &self.features, &input.marked);
        self.lay_out(&data);
        self.texture.sync(cx.gpu, &mut self.atlas);
        let label_scale = input.label_scale;
        let style = TextStyle::LABELS.scaled(label_scale);
        self.batch.set_style(&cx.gpu.queue, &style, &BackgroundStyle::DEFAULT, &FontSettings::LABELS, cx.view);

        let placed = Placed {
            view: cx.view.view,
            size: cx.view.size,
            map_scale: cx.view.map_scale,
            common_centre: cx.view.common_centre,
            version: self.version,
            data,
            label_scale,
            generation: self.laid_generation,
        };
        if self.placed.as_ref() == Some(&placed) {
            return;
        }
        // the collision pass for this camera, then the labels it leaves
        let focal = focal_w(cx.view);
        let anchors: Vec<_> = placed.data.iter().map(|&i| place_anchor(cx.view, focal, [self.features[i].lon, self.features[i].lat])).collect();
        let empty = LaidText { glyphs: Vec::new(), rect: [0.0; 4], font_size: FontSettings::LABELS.font_size, generation: 0 };
        let placings: Vec<Placing<'_>> = placed
            .data
            .iter()
            .zip(&anchors)
            .map(|(&i, at)| {
                let f = &self.features[i];
                Placing {
                    anchor: at.map(|(anchor, _, _)| anchor),
                    perspective: at.map_or(1.0, |(_, k, _)| k),
                    priority: f.priority,
                    size: f.size,
                    text: self.laid[i].as_ref().unwrap_or(&empty),
                }
            })
            .collect();
        let fades = labels_map::collision_fades(&mut self.collision, cx.view.size, cx.view.map_scale, &placings);
        let mut glyphs: Vec<Glyph> = Vec::new();
        let mut shown = Vec::new();
        for ((placing, &fade), (&i, at)) in placings.iter().zip(&fades).zip(placed.data.iter().zip(&anchors)) {
            let Some((anchor, perspective, depth)) = *at else { continue };
            if !labels_map::drawn(fade) {
                continue;
            }
            let f = &self.features[i];
            let mut color = f.color;
            color[3] = (f64::from(color[3]) * fade).round() as u8;
            let label = Label::at(cx.view, [f.lon, f.lat, 0.0], f.size, color);
            glyphs.extend(label.glyphs(placing.text));
            let Some(rect) = labels_map::bounds(labels_map::glyph_quads(placing.text, anchor, f.size * label_scale, perspective)) else { continue };
            shown.push(Shown { text: f.text.clone(), iata: f.iata.clone(), lon: f.lon, lat: f.lat, anchor, depth, rect, fade, priority: f.priority, color: f.color, size: f.size });
        }
        self.batch.set_glyphs(&cx.gpu.device, &cx.gpu.queue, &glyphs);
        input.drawn.set(shown);
        self.placed = Some(placed);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map || !self.shown || self.batch.is_empty() {
            return;
        }
        self.pipeline.draw(pass, &self.batch);
    }

    fn drape(&self) -> Drape {
        Drape::Offset
    }

    fn pending(&self) -> bool {
        self.shown && self.tiles.loading()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path as FilePath, PathBuf};
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::time::{Duration, Instant};

    use rustradar_core::geo::{Basemap, cos_angle};
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::labels_map::{AIRPORT_COLOR, CITY_COLOR, COLLISION_SIZE_SCALE};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::layers::TileIndex;
    use crate::headless::{FORMAT, Image, Offscreen, render_with};
    use crate::io::{DirTiles, FetchRequest, FetchResult, FnFetcher, TileFetcher};
    use crate::layers::{earth, vector_map};
    use crate::renderer::{MapRenderer, SceneInput};

    fn view(latitude: f64, longitude: f64, zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { latitude, longitude, zoom, pitch, bearing, position: None }
    }

    /// The plan's named views (section 8).
    fn v1() -> ViewState {
        view(35.0, 10.0, 1.9, 0.0, 0.0)
    }
    fn v2() -> ViewState {
        view(52.0, 10.0, 4.5, 0.0, 0.0)
    }
    fn v3() -> ViewState {
        view(59.33, 18.07, 9.0, 0.0, 0.0)
    }

    const SIZE: [f64; 2] = [1440.0, 900.0];

    /// The scene of `view` on `basemap` with the labels drawn by `labels`: the earth and, on the
    /// dark basemap, the vector map under them.
    fn scene(view: ViewState, basemap: Basemap, labels: Input) -> SceneInput {
        let mut scene = SceneInput::new(view, SIZE, 1.0, basemap);
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = vector_map::Input { shown: basemap == Basemap::Dark };
        scene.labels = labels;
        scene
    }

    fn renderer(gpu: &crate::gpu::Gpu, fetcher: Arc<dyn TileFetcher>) -> MapRenderer {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(fetcher, None);
        renderer
    }

    fn shots_dir() -> PathBuf {
        let dir = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    fn save(image: &Image, name: &str) {
        image.save_png(&shots_dir().join(name)).expect("the shot written");
    }

    fn reference(name: &str) -> Image {
        let path = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        let rgba = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).into_rgba8();
        Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
    }

    /// `scene` rendered once nothing is loading any more: [`render_with`] again on the same
    /// renderer (its tiles carry over) while a layer is still pending after its wait, as a debug
    /// build on a busy machine can take longer than one wait.
    fn settled(gpu: &crate::gpu::Gpu, renderer: &mut MapRenderer, scene: &SceneInput) -> Image {
        let mut image = render_with(gpu, renderer, scene, 1.0).expect("rendered");
        for _ in 0..4 {
            if !renderer.pending() {
                break;
            }
            image = render_with(gpu, renderer, scene, 1.0).expect("rendered");
        }
        image
    }

    /// A dark shot of `view` with `labels` from the fixtures at 1440 × 900, saved as `name`, with
    /// no tile missing; the picture and the labels drawn.
    fn shot(view: ViewState, labels: Input, name: &str) -> Option<(Image, Vec<Shown>)> {
        let gpu = gpu()?;
        let fixtures = Arc::new(DirTiles::fixtures());
        let mut renderer = renderer(&gpu, fixtures.clone());
        let drawn = labels.drawn.clone();
        let image = settled(&gpu, &mut renderer, &scene(view, Basemap::Dark, labels));
        save(&image, name);
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        assert!(fixtures.misses().is_empty(), "the fixtures lack {:?}", fixtures.misses());
        Some((image, drawn.labels()))
    }

    /// The map in today's pictures at 1440 × 900: right of the sidebar, left of the map controls,
    /// under the mode bar and the dev readout, over the attribution.
    const MAP: [u32; 4] = [360, 80, 1370, 880];

    /// A pixel of a label's fill: light grey to white (the place colours), or the airports' blue.
    /// Roads, borders and runways are darker or not grey; the halo is darker still.
    fn is_text(p: [u8; 4]) -> bool {
        let [r, g, b, _] = p;
        let (lo, hi) = (r.min(g).min(b), r.max(g).max(b));
        (lo >= 110 && hi >= 140) || (b >= 170 && u16::from(b) > u16::from(r) + 80)
    }

    /// Whether `image` has a text pixel within `reach` pixels of `(x, y)`.
    fn text_near(image: &Image, x: i32, y: i32, reach: i32) -> bool {
        (-reach..=reach).any(|dy| (-reach..=reach).any(|dx| {
            let (px, py) = (x + dx, y + dy);
            px >= 0 && py >= 0 && (px as u32) < image.width && (py as u32) < image.height && is_text(image.pixel(px as u32, py as u32))
        }))
    }

    /// How alike the labels of `ours` and `today` are inside [`MAP`], at the best shift within 6 px
    /// (today's pictures stand about 4.5 px higher than deck's viewport; T1): the share of today's
    /// text pixels with one of ours within 1 px, the share of ours with one of today's within 1 px
    /// (glyphs sit up to 1 px apart, plan section 6 item 16), today's and our text pixel counts,
    /// and the shift.
    fn text_agreement(ours: &Image, today: &Image) -> (f64, f64, usize, usize, [i32; 2]) {
        let [x0, y0, x1, y1] = MAP;
        let pixels = |image: &Image| -> Vec<(i32, i32)> {
            (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| is_text(image.pixel(x, y))).map(|(x, y)| (x as i32, y as i32)).collect()
        };
        let (theirs, mine) = (pixels(today), pixels(ours));
        let mut best = (0.0, 0.0, [0, 0]);
        for dy in -6..=6 {
            for dx in -6..=6 {
                let recall = theirs.iter().filter(|&&(x, y)| text_near(ours, x + dx, y + dy, 1)).count() as f64 / theirs.len().max(1) as f64;
                let precision = mine.iter().filter(|&&(x, y)| text_near(today, x - dx, y - dy, 1)).count() as f64 / mine.len().max(1) as f64;
                if recall + precision > best.0 + best.1 {
                    best = (recall, precision, [dx, dy]);
                }
            }
        }
        (best.0, best.1, theirs.len(), mine.len(), best.2)
    }

    fn named<'a>(drawn: &'a [Shown], text: &str) -> &'a Shown {
        drawn.iter().find(|s| s.text == text).unwrap_or_else(|| panic!("{text:?} is not drawn"))
    }

    /// Pixels in [`MAP`] within `tolerance` levels of `rgb` on each channel.
    fn near(image: &Image, rgb: [u8; 3], tolerance: u8) -> usize {
        let [x0, y0, x1, y1] = MAP;
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| image.pixel(x, y)[..3].iter().zip(rgb).all(|(&a, b)| a.abs_diff(b) <= tolerance)).count()
    }

    /// deck's collision filter as drawn: no label is drawn whose anchor lies inside the middle row
    /// of a more important drawn label's footprint at 1.3 (its rectangle grown 1.3 times about its
    /// anchor, kept clear of its ends by 4 px and of its anchor's row by 1 px: the 5 × 5 patch
    /// round such an anchor is the other label's, so its fade is 0). Their boxes may overlap, as
    /// today's do; how many pairs do is returned.
    fn assert_collision_filtered(drawn: &[Shown]) -> usize {
        let mut overlapping = 0;
        for a in drawn {
            let k = COLLISION_SIZE_SCALE;
            let [ax, ay] = a.anchor;
            let (x0, x1) = (ax + (a.rect.x0 - ax) * k + 4.0, ax + (a.rect.x1 - ax) * k - 4.0);
            for b in drawn {
                if std::ptr::eq(a, b) {
                    continue;
                }
                let [bx, by] = b.anchor;
                assert!(
                    !(a.priority > b.priority && (x0..=x1).contains(&bx) && (by - ay).abs() <= 1.0),
                    "{:?} (priority {}) is drawn under {:?} (priority {})",
                    b.text,
                    b.priority,
                    a.text,
                    a.priority
                );
                overlapping += usize::from(a.rect.overlaps(&b.rect));
            }
        }
        overlapping / 2
    }

    /// The decode step on a V3 fixture tile: Stockholm's name and Bromma's code with today's text,
    /// size, colour and priority.
    #[test]
    fn a_fixture_tile_gives_place_names_and_airport_codes() {
        let tile = TileIndex::new(281, 150, 9);
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, tile).expect("read").expect("a V3 tile");
        let labels = labels_map::decode(tile, &bytes);
        let find = |text: &str| labels.iter().find(|l| l.text == text).unwrap_or_else(|| panic!("no {text:?} in {}", labels.len()));
        let stockholm = find("Stockholm");
        assert_eq!((stockholm.size, stockholm.color, stockholm.priority, stockholm.iata.as_deref()), (14.0, CITY_COLOR, 780.0, None));
        assert!((stockholm.lon - 18.07).abs() < 0.05 && (stockholm.lat - 59.33).abs() < 0.05, "{stockholm:?}");
        let bromma = find("BMA");
        assert_eq!((bromma.size, bromma.color, bromma.priority, bromma.iata.as_deref()), (12.0, AIRPORT_COLOR, 600.0, Some("BMA")));
        // places first, then airports, as loaders.gl walks `layers`
        let first_airport = labels.iter().position(|l| l.iata.is_some()).expect("an airport");
        assert!(labels[first_airport..].iter().all(|l| l.iata.is_some()));
    }

    /// Plan B4, V2 (Europe, zoom 4.5): country and city names in today's colours, and the same
    /// labels as today's picture: of today's text pixels 99.6 % have one of ours within 1 px, of
    /// ours 99.4 % one of today's, as many pixels within 2 % (plan section 6 item 17: deck's own
    /// collision test, not the whole-box greedy pass, keeps today's density). As today, two labels'
    /// boxes may overlap ("DENMARK" under "Copenhagen"), and a label half under another fades
    /// ("Amsterdam").
    #[test]
    fn v2_draws_today_s_country_and_city_names() {
        let Some((image, drawn)) = shot(v2(), Input::shown(), "b4-dark-v2-1440x900.png") else { return };
        let (recall, precision, theirs, mine, shift) = text_agreement(&image, &reference("dark-v2-1440x900.png"));
        let cities = near(&image, [241, 238, 231], 2);
        let countries = near(&image, [162, 172, 183], 2);
        let overlapping = assert_collision_filtered(&drawn);
        eprintln!(
            "V2: {} labels, {overlapping} pairs of boxes overlapping; city pixels {cities}, country {countries}; today's text {theirs} px, ours {mine}: {recall:.3} of today's and {precision:.3} of ours matched at {shift:?}",
            drawn.len()
        );
        assert!(cities > 1_000 && countries > 200, "city {cities}, country {countries}");
        for text in ["Berlin", "Copenhagen", "DENMARK", "Stockholm", "Paris", "London", "GERMANY"] {
            assert_eq!(named(&drawn, text).fade, 1.0, "{text}");
        }
        let amsterdam = named(&drawn, "Amsterdam");
        assert!(amsterdam.fade < 0.5, "Amsterdam is half hidden today: {amsterdam:?}");
        assert!(named(&drawn, "DENMARK").rect.overlaps(&named(&drawn, "Copenhagen").rect));
        assert!(recall > 0.97 && precision > 0.97, "{recall} of today's, {precision} of ours");
        assert!((mine as f64 / theirs as f64 - 1.0).abs() < 0.05, "{mine} text pixels, today {theirs}");
    }

    /// Plan B4, V3 (Stockholm, zoom 9): the city's name in `(241,238,231)`, Bromma's code in
    /// `(55,142,240)`, and today's labels: 98.6 % of today's text pixels and 97.6 % of ours
    /// matched at today's 5 px offset. The pick list gives Bromma's code under its label.
    #[test]
    fn v3_draws_today_s_names_and_bromma_s_code() {
        let Some((image, drawn)) = shot(v3(), Input::shown(), "b4-dark-v3-1440x900.png") else { return };
        let (recall, precision, theirs, mine, shift) = text_agreement(&image, &reference("dark-v3-1440x900.png"));
        let cities = near(&image, [241, 238, 231], 2);
        let airports = near(&image, [55, 142, 240], 0);
        let overlapping = assert_collision_filtered(&drawn);
        eprintln!(
            "V3: {} labels, {overlapping} pairs of boxes overlapping; city pixels {cities}, airport {airports}; today's text {theirs} px, ours {mine}: {recall:.3} of today's and {precision:.3} of ours matched at {shift:?}",
            drawn.len()
        );
        assert!(cities > 100 && airports > 15, "city {cities}, airport {airports}");
        let stockholm = named(&drawn, "Stockholm");
        let bromma = named(&drawn, "BMA");
        assert_eq!((stockholm.fade, stockholm.color, bromma.fade, bromma.iata.as_deref()), (1.0, CITY_COLOR, 1.0, Some("BMA")));
        // today: "Sundbyberg", "Solna" and "BMA" crowd each other, all drawn
        assert!(named(&drawn, "Sundbyberg").rect.overlaps(&named(&drawn, "Solna").rect));
        assert!(recall > 0.96 && precision > 0.96, "{recall} of today's, {precision} of ours");
        assert!((mine as f64 / theirs as f64 - 1.0).abs() < 0.05, "{mine} text pixels, today {theirs}");
        // picking: Bromma's code at its anchor, Stockholm's name (a place: no code) at its own
        let frame = ViewFrame::new(&v3(), SIZE, 1.0, 0.0);
        let picks = Input::shown().drawn.picks();
        assert!(picks.is_empty(), "a new handle holds nothing");
        let handle = Input::shown();
        handle.drawn.set(drawn.clone());
        let picks = handle.drawn.picks();
        assert_eq!(picks.len(), drawn.len());
        let hit = picks.pick(&frame.viewport, bromma.anchor, 1.0).expect("a label under Bromma's anchor");
        assert_eq!((hit.key.as_deref(), hit.distance_px), (Some("BMA"), 0.0));
        let hit = picks.pick(&frame.viewport, stockholm.anchor, 1.0).expect("a label under Stockholm's anchor");
        assert_eq!((drawn[hit.index].text.as_str(), hit.key), ("Stockholm", &None));
    }

    /// The label size setting scales the text drawn, not the collision test (`sizeScale: 1.3`
    /// whatever `labelScale` is): the same labels with the same fades, each 1.5 times as wide. A
    /// marked airport loses its label.
    #[test]
    fn the_size_setting_scales_the_text_and_marked_airports_lose_their_labels() {
        let Some(gpu) = gpu() else { return };
        let mut renderer = renderer(&gpu, Arc::new(DirTiles::fixtures()));
        let mut run = |labels: Input| {
            let drawn = labels.drawn.clone();
            settled(&gpu, &mut renderer, &scene(v3(), Basemap::Dark, labels));
            drawn.labels()
        };
        let normal = run(Input::shown());
        let large = run(Input { label_scale: 1.5, ..Input::shown() });
        let marked = run(Input { marked: vec!["BMA".into(), "ARN".into()], ..Input::shown() });
        let key = |d: &[Shown]| d.iter().map(|s| (s.text.clone(), s.fade)).collect::<Vec<_>>();
        assert_eq!(key(&large), key(&normal), "the setting moved the collision test");
        for (a, b) in normal.iter().zip(&large) {
            let (wa, wb) = (a.rect.width(), b.rect.width());
            assert!((wb - 1.5 * wa).abs() < 1e-6, "{}: {wa} → {wb}", a.text);
            assert_eq!(a.anchor, b.anchor);
        }
        assert!(normal.iter().any(|s| s.iata.as_deref() == Some("BMA")));
        assert!(marked.iter().all(|s| s.iata.as_deref() != Some("BMA")), "BMA is marked");
        assert_eq!(named(&marked, "Stockholm").fade, 1.0);
    }

    /// V1 (the whole globe, zoom 1.9): only labels facing the camera (`cosAngle > 0.42` from the
    /// view's centre), so nothing on the far side shows through the globe; country names and big
    /// cities.
    #[test]
    fn far_out_only_labels_facing_the_camera_are_drawn() {
        let Some((_, drawn)) = shot(v1(), Input::shown(), "b4-dark-v1-1440x900.png") else { return };
        let v = v1();
        eprintln!("V1: {} labels: {:?}", drawn.len(), drawn.iter().map(|s| s.text.as_str()).collect::<Vec<_>>());
        assert!(drawn.len() > 10 && drawn.iter().any(|s| s.color == labels_map::COUNTRY_COLOR), "{drawn:?}");
        for s in &drawn {
            assert!(cos_angle(s.lon, s.lat, v.longitude, v.latitude) > labels_map::HORIZON_MIN_COS, "{s:?}");
        }
    }

    /// A fetcher that keeps back the tiles of `held` until it is opened, and answers every other
    /// tile from the fixtures at once.
    struct Gate {
        tiles: DirTiles,
        held: Mutex<Vec<TileIndex>>,
        waiting: Mutex<Vec<(TileIndex, Sender<FetchResult>)>>,
    }

    impl Gate {
        fn new(held: Vec<TileIndex>) -> Arc<Self> {
            Arc::new(Gate { tiles: DirTiles::fixtures(), held: Mutex::new(held), waiting: Mutex::new(Vec::new()) })
        }

        /// Answers the tiles held back (with the bytes of `instead`, when given), and from now on
        /// every tile at once.
        fn open(&self, instead: Option<TileIndex>) {
            self.held.lock().unwrap().clear();
            for (tile, tx) in self.waiting.lock().unwrap().drain(..) {
                let _ = tx.send(self.tiles.read(io::OPENFREEMAP, instead.unwrap_or(tile)));
            }
        }

        fn hold(&self, tiles: Vec<TileIndex>) {
            *self.held.lock().unwrap() = tiles;
        }

        fn waiting(&self) -> usize {
            self.waiting.lock().unwrap().len()
        }
    }

    impl TileFetcher for Gate {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            if self.held.lock().unwrap().contains(&request.tile) {
                let (tx, rx) = mpsc::channel();
                self.waiting.lock().unwrap().push((request.tile, tx));
                rx
            } else {
                self.tiles.fetch(request)
            }
        }
    }

    /// The z9 tile under `[lon, lat]`.
    fn z9([lon, lat]: [f64; 2]) -> TileIndex {
        let n = 512.0;
        let phi = lat.to_radians();
        let y = (1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
        TileIndex::new(((lon + 180.0) / 360.0 * n).floor() as u32, y.floor() as u32, 9)
    }

    /// `onViewportLoad`: the labels change only once every tile selected for the view has come in.
    /// While one is held back nothing is drawn at first; after a pan the old labels stay drawn
    /// (moved with the camera) until the new tile is in. Every label tile also carries its
    /// neighbours' places (OpenMapTiles' place buffer), so the tile held back after the pan is
    /// answered with the bytes of 279/150 to tell its labels apart: Eskilstuna's lands 4 columns
    /// east, in the sea off the archipelago, where no other tile has a label.
    #[test]
    fn labels_change_only_once_every_selected_tile_is_in() {
        let Some(gpu) = gpu() else { return };
        // 1000 × 400 px at V3 spans about 17.38° to 18.76° east (z9 columns 280 to 282, row 150);
        // 0.6° further east it reaches into column 283
        let size = [1000.0, 400.0];
        let start = v3();
        let east = ViewState { longitude: start.longitude + 0.6, ..start };
        let new_tile = z9([19.1, start.latitude]);
        assert_eq!((new_tile.x, new_tile.y), (283, 150));
        let gate = Gate::new(vec![z9([start.longitude, start.latitude])]);
        let mut renderer = renderer(&gpu, gate.clone());
        let target = Offscreen::new(&gpu, [1000, 400]);
        let input = Input::shown();
        let drawn = input.drawn.clone();
        let frames = |renderer: &mut MapRenderer, view: ViewState, until: &dyn Fn(&MapRenderer) -> bool| {
            let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
            scene.labels = input.clone();
            renderer.update(scene);
            let started = Instant::now();
            while !until(renderer) && started.elapsed() < Duration::from_secs(120) {
                renderer.render(&gpu, target.view(), target.size(), 0.0);
                gpu.device.poll(wgpu::PollType::wait_indefinitely()).expect("polled");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        // every tile but the centre's: still nothing
        frames(&mut renderer, start, &|_| gate.waiting() > 0);
        let asked = Instant::now();
        frames(&mut renderer, start, &|_| asked.elapsed() > Duration::from_secs(2));
        assert!(renderer.pending(), "the held tile keeps the layer loading");
        assert!(drawn.labels().is_empty(), "drawn before the view loaded: {:?}", drawn.labels());
        // the centre's tile comes in: the labels of every selected tile
        gate.open(None);
        frames(&mut renderer, start, &|r| !r.pending());
        let before = drawn.labels();
        assert!(before.iter().any(|s| s.text == "Stockholm"), "{before:?}");
        // a pan that selects one more tile, held back: the old labels, moved
        gate.hold(vec![new_tile]);
        frames(&mut renderer, east, &|_| gate.waiting() > 0);
        let asked = Instant::now();
        frames(&mut renderer, east, &|_| asked.elapsed() > Duration::from_secs(2));
        let during = drawn.labels();
        assert!(!during.is_empty() && !during.iter().any(|s| s.text == "Eskilstuna"), "{during:?}");
        let mut common = 0;
        for s in &during {
            let Some(old) = before.iter().find(|b| b.text == s.text && b.lon == s.lon && b.lat == s.lat) else { continue };
            assert!(s.anchor[0] < old.anchor[0] - 300.0, "{:?} did not move with the pan: {:?} → {:?}", s.text, old.anchor, s.anchor);
            common += 1;
        }
        assert!(common > 10, "{common} labels drawn before and after the pan");
        // the tile comes in: its labels too
        gate.open(Some(TileIndex::new(279, 150, 9)));
        frames(&mut renderer, east, &|r| !r.pending());
        let after = drawn.labels();
        let eskilstuna = named(&after, "Eskilstuna");
        assert!(z9([eskilstuna.lon, eskilstuna.lat]) == new_tile, "{eskilstuna:?}");
    }

    /// The labels show on every basemap while the setting is on; off, nothing is asked for and
    /// nothing is drawn or pickable.
    #[test]
    fn labels_show_on_every_basemap_and_nothing_is_asked_while_off() {
        let Some(gpu) = gpu() else { return };
        let size = [600.0, 400.0];
        let asked: Arc<Mutex<Vec<TileIndex>>> = Arc::default();
        let noted = asked.clone();
        let tiles = DirTiles::fixtures();
        let fetcher = Arc::new(FnFetcher(move |source: &str, tile| {
            if source == io::OPENFREEMAP {
                noted.lock().unwrap().push(tile);
            }
            tiles.read(source, tile)
        }));
        let mut renderer = renderer(&gpu, fetcher);
        let off = Input::default();
        let drawn = off.drawn.clone();
        let mut scene = SceneInput::new(v3(), size, 1.0, Basemap::Dark);
        scene.labels = off;
        render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        assert!(asked.lock().unwrap().is_empty() && drawn.labels().is_empty());
        for basemap in [Basemap::Satellite, Basemap::Dark] {
            let input = Input::shown();
            let drawn = input.drawn.clone();
            let mut scene = SceneInput::new(v3(), size, 1.0, basemap);
            scene.labels = input.clone();
            settled(&gpu, &mut renderer, &scene);
            assert!(drawn.labels().iter().any(|s| s.text == "Stockholm"), "{basemap:?}");
            // turned off: nothing drawn, nothing to pick
            scene.labels = Input { shown: false, ..input };
            render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            assert!(drawn.labels().is_empty() && drawn.picks().is_empty(), "{basemap:?}");
        }
    }
}
