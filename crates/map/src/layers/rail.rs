//! The rail map (item T4; doc 02 §8.1–8.4; row 4 of doc 03 §3): `railLayers` (rail-layer.ts:518-542)
//! as today's app draws it over the map, under the weather, ships, trains and aircraft (G:2679), on
//! every basemap while the *rail map* setting is on. Its bridges on the flat map (doc 02 §9.2) are
//! item T7's part of this file (below).
//!
//! What today's app does, and so this layer, in drawing order ([`rustradar_scene::rail::rail_layers`]
//! decides which, from the frame's zoom and projection):
//! - **Overview below zoom 8** (`rail-overview-*`, doc 02 §8.3): Natural Earth's railroads shipped
//!   with the app (`static/rail/railroads.bin`, embedded), parsed and made into path segments on the
//!   worker pool the first time a view below zoom 8 asks for them (`railOverview()` loads lazily),
//!   drawn once they are in, never if the file cannot be read. Open paths, main lines `RAIL_MAIN`
//!   1.5 px, the rest `RAIL_MINOR` 1 px, `SURFACE_NO_CULL`, one mesh in the world's frame.
//! - **From zoom 8, the speed map or the network.** In the app the rail map is coloured by speed
//!   limit (`railSpeeds = isTauri() && !trainBlocks`, G:2074): the backend's OpenRailwayMap tracks
//!   ([`RAIL_TILES`], `backend::rail::tile` through the app's fetcher; tile zooms 7–12,
//!   over-zoomed beyond), each coloured by its limit, with the limits written on the lines from zoom
//!   12.5 (doc 02 §8.2). While the rail map shows the signal blocks, the network comes from
//!   OpenFreeMap's `transportation` layer instead (tile zooms 8–14; doc 02 §8.1). Lines are deck's
//!   GeoJsonLayer paths: widths in pixels times `railWidthScale(zoom)`, round caps, miter joints,
//!   tunnels dashed `[3, 2.5]` justified per segment (PathStyleExtension `{ dash: true }`),
//!   `SURFACE_NO_CULL` at true depth (no polygon offset anywhere in the port), so the buildings
//!   drawn later cover them.
//! - **Stations from zoom 12** (doc 02 §8.4): OpenFreeMap's `poi` stations and halts as rings
//!   (3.5 and 2.5 px, `[214,220,232]` in `[8,12,20,230]` 1.5 px), on the ground
//!   (`pointBillboard` false), `SURFACE_NO_CULL`.
//! - The OpenRailwayMap raster (doc 02 §8.5) is not ported: today's app never turns it on.
//!
//! Tiles as the vector map has them (`layers::vector_map`, items B1 and B2): deck's rule for 512-px
//! tiles, `maxRequests` 6, best-available refinement, one tile set per source for both
//! projections (tiles up to z12 tessellated the globe's way; see `rustradar_scene::rail`), decoded
//! on the worker pool, drawn coarse to fine in their tile frames. On the flat map MVTLayer clips
//! each tile to its square (`ClipExtension`): the lines through the stencil as the vector map does
//! (its [`CLIP_MARK`], [`CLIPPED`], [`CLIP_RESET`]), the stations and the speed labels by their
//! anchors. A tile's speed labels (a TextLayer of the tile's sublayers: not SDF, 10 px, Overpass
//! 650 for Inter (plan section 6, item 5), `[12,14,20]` on the limit's colour padded `[3, 1]`,
//! billboards, `LABEL`: depth always) draw right after the tile's lines, before the next tile.
//!
//! A sublayer that leaves `railLayers`' list (zoom 8 crossed, the rail map off, speeds swapped for
//! the network) drops its tiles, as deck finalizes a layer that leaves the list. Deviations: one
//! tile cache for both projections (deck keeps `rail-globe` and `rail-flat` apart, so crossing
//! zoom 12 refetches; plan section 6, item 2); the layers and the width scale are worked out each
//! frame from the frame's camera, not at each scene refresh (at most 50 ms apart at these zooms).
//!
//! The speed legend and the credit are the UI's (stage 7): `rustradar_scene::rail::speed_legend`,
//! `speed_legend_shown`, `next_shown_zoom`, `rail_attribution`.
//!
//! **Bridges** (item T7; doc 02 §8.1, §8.2, §9.2). On the flat map today's app draws each tile of
//! the network or the speed map that has bridges as two sublayers (`renderNetworkTile`,
//! `renderSpeedTile`, rail-layer.ts:134-143, 413-424): the tile's lines without its bridges, as
//! above, then `-bridges`, its bridges cut into pieces of at most 6 m that rise to their 6 m
//! decks over 40 m from each end, in the same style but `RAISED_NO_CULL` (no cull, no depth
//! written, depth test always), then the tile's speed labels; on the globe every line is drawn as
//! it is. **As high as today:** the TypeScript writes the heights in deck's common units where
//! deck reads a `CARTESIAN` tile's `z` as metres, so today's bridges stand
//! [`rustradar_scene::bridges::rail_map_lift`] of their decks up (0.15 mm), on the ground to the
//! eye (today's app at Söderström, pitched 55°: the main line's bridge where its ground is, no
//! ramp at the tunnel's mouth). What shows of the split is the order: a tile's bridges over its
//! other lines whatever the tile's order. The tiles from z12
//! ([`rustradar_scene::bridges::FLAT_MIN_Z`], the flat map's) are decoded the flat map's way once
//! more on the pool ([`rustradar_scene::bridges::network_bridges`], `speed_bridges`), next to the
//! lines both projections draw: a tile with bridges keeps both, and the flat map draws its
//! [`RaisedRail`] in place of its lines. The ground lines go through the tile's stencil square as
//! the other lines do; the bridges are cut at the tile's square by the scene, where they stand
//! (deck's `ClipExtension`, by fragment: a raised line would show off its stencil square), and
//! draw without the stencil. Tunnels are the network's and the speed map's own style (faint,
//! dashed) on the ground, as today. The trains and the signal blocks on the track take their
//! levels from the backend's structures (`rustradar_scene::bridges::level_at`, `on_structures`;
//! `layers::trains`, `layers::blocks`): those stand 6 m up on a bridge, today too.
//! Deviations: from doc 02 §9.2 and plan row T7 ("raised tile lines", "drawn over the water"
//! 6 m up), which describe what the TypeScript means rather than what it draws: the port draws
//! what today's app draws (post-parity, raising them for real is dropping `rail_map_lift`; the cut
//! at the square already holds for a raised line). None visible: a z12 tile with bridges draws
//! the flat map's tessellation on the flat map (as deck), one without the globe's (T4's, a
//! fraction of a pixel off).

use rustradar_scene::bridges::{self, RaisedRail};
use rustradar_scene::geom::path::PathSegment;
use rustradar_scene::rail::{
    self as rails, OVERVIEW_COLORS, RAIL_MIN_ZOOM, RailLayer, RailOptions, RailTile, SPEED_LABEL_PADDING, SPEED_LABEL_SIZE_PX, SPEED_LABEL_TEXT, SPEED_LABEL_WEIGHT,
    STATION_FILL, STATION_RING, STATION_RING_WIDTH, SpeedLabel, Station,
};
use rustradar_scene::rail_overview::parse_rail_overview;
use rustradar_scene::text::{Face, Family, FontSettings, GlyphAtlas, LaidText, TextLayout};

use super::vector_map::{CLIP_MARK, CLIP_RESET, CLIPPED, MAX_REQUESTS};
use super::{ByteSize, DeckSelect, DrawCx, Init, Layer, Pass, PrepareCx, Source, TileIndex, TileSet, TileSetOptions};
use crate::frame::FrameSlot;
use crate::io;
use crate::jobs::{Job, JobPoll};
use crate::pipelines::circle::{self, Circle, CircleStyle};
use crate::pipelines::fill::{self, Fill};
use crate::pipelines::path::{self, Dash, Path, PathStyle};
use crate::pipelines::text::{AtlasTexture, Background, BackgroundStyle, Glyph, Label, TextBatch, TextPipeline, TextStyle};
use crate::state::DrawState;

/// The backend's rail tiles (OpenRailwayMap's `railway_line_high`, kept by `backend::rail`): the
/// source name the app's fetcher maps to `backend::rail::tile`, and a fixture folder's name.
pub const RAIL_TILES: &str = "openrailwaymap";

/// `static/rail/railroads.bin`, the overview (doc 02 §8.3).
pub static RAILROADS: &[u8] = include_bytes!("../../../../static/rail/railroads.bin");

/// The speed labels' face: Overpass at 650 for today's Inter (plan section 6, item 5).
pub const SPEED_LABEL_FACE: Face = Face::new(Family::Overpass, SPEED_LABEL_WEIGHT);

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// The rail map is on (`railMap`, G:2679), on every basemap.
    pub shown: bool,
    /// Coloured by speed limit from the backend's tracks (`railSpeeds`,
    /// `rustradar_scene::rail::rail_speeds`); else OpenFreeMap's network (the rail map showing the
    /// signal blocks).
    pub speeds: bool,
}

/// A tile of the network or of the speed map as the pool decodes it: its lines as the globe (and
/// the flat map, without bridges) draws them, and from z12 the flat map's lines with the tile's
/// bridges split off and raised as today's app raises them, when it has any (T7).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    pub tile: RailTile,
    pub bridges: Option<RaisedRail>,
}

impl Decoded {
    /// The bytes it holds (for the tile cache's budget).
    pub fn byte_size(&self) -> usize {
        self.tile.byte_size() + self.bridges.as_ref().map_or(0, RaisedRail::byte_size)
    }
}

impl ByteSize for Decoded {
    fn byte_size(&self) -> usize {
        Decoded::byte_size(self)
    }
}

/// A tile of lines on the GPU, and its speed labels (tile units).
pub struct TileLines {
    pub lines: path::Mesh,
    pub labels: Vec<SpeedLabel>,
    /// The flat map's lines of a tile with bridges (T7).
    pub bridges: Option<RaisedMeshes>,
}

/// [`RaisedRail`] on the GPU: the ground's lines and the bridges'.
pub struct RaisedMeshes {
    pub ground: path::Mesh,
    pub raised: path::Mesh,
}

/// A tile's decoded lines and its flat bridges, `None` for a tile that draws nothing.
fn decoded(
    tile: TileIndex,
    lines: Result<RailTile, rails::Cancelled>,
    flat: impl FnOnce() -> Result<Option<RaisedRail>, rails::Cancelled>,
) -> Result<Option<Decoded>, String> {
    let cancelled = |_| String::from("cancelled");
    let lines = lines.map_err(cancelled)?;
    // only the flat map's tiles are raised (z12 on: the globe draws up to z12, the flat map from it)
    let bridges = if tile.z >= bridges::FLAT_MIN_Z { flat().map_err(cancelled)? } else { None };
    Ok((!lines.is_empty() || bridges.is_some()).then_some(Decoded { tile: lines, bridges }))
}

/// The decode step of the network, on the worker pool (`None` for a tile with no rail).
pub fn decode_network(tile: TileIndex, bytes: &[u8], cancel: &crate::jobs::Cancel) -> Result<Option<Decoded>, String> {
    let cancelled = || cancel.is_cancelled();
    decoded(tile, rails::network_tile(tile, bytes, &cancelled), || bridges::network_bridges(tile, bytes, &cancelled))
}

/// The decode step of the speed map, on the worker pool.
pub fn decode_speeds(tile: TileIndex, bytes: &[u8], cancel: &crate::jobs::Cancel) -> Result<Option<Decoded>, String> {
    let cancelled = || cancel.is_cancelled();
    decoded(tile, rails::speed_tile(tile, bytes, &cancelled), || bridges::speed_bridges(tile, bytes, &cancelled))
}

/// The decode step of the stations, on the worker pool.
pub fn decode_stations(tile: TileIndex, bytes: &[u8], cancel: &crate::jobs::Cancel) -> Result<Option<Vec<Station>>, String> {
    match rails::station_tile(tile, bytes, &|| cancel.is_cancelled()) {
        Ok(s) if s.is_empty() => Ok(None),
        Ok(s) => Ok(Some(s)),
        Err(rails::Cancelled) => Err("cancelled".into()),
    }
}

/// The upload step of a tile of lines, on the main thread.
fn upload(device: &wgpu::Device, decoded: Decoded) -> TileLines {
    let Decoded { tile, bridges } = decoded;
    TileLines {
        lines: path::Mesh::new(device, "rail lines", &tile.lines),
        labels: tile.labels,
        bridges: bridges.map(|b| RaisedMeshes {
            ground: path::Mesh::new(device, "rail lines without bridges", &b.ground),
            raised: path::Mesh::new(device, "rail bridges", &b.raised),
        }),
    }
}

/// The overview, loaded on first need.
enum Overview {
    /// Not asked for yet.
    Idle,
    /// Parsed and turned into segments on the worker pool.
    Loading(Job<Option<Vec<PathSegment>>>),
    /// In, on the GPU.
    Ready(path::Mesh),
    /// The file could not be read: drawn without it, as today.
    Failed,
}

/// The tile's square and its two stencil passes (the flat map's clip, as the vector map's).
struct Clip {
    square: fill::Mesh<fill::Vertex>,
    mark: Fill,
    mark_style: fill::Style,
    reset: Fill,
    reset_style: fill::Style,
}

/// The line style of the network and the speed map at `zoom` (`lineWidthUnits: "pixels"`,
/// `lineWidthScale: railWidthScale(zoom)`, `lineCapRounded: true`, PathStyleExtension
/// `{ dash: true }` with `dashJustified: true`).
pub fn line_style(zoom: f64) -> PathStyle {
    PathStyle { width_scale: rails::rail_width_scale(zoom), cap_rounded: true, dash: Dash::Segment, dash_justified: true, ..PathStyle::PIXELS }
}

/// The overview's style: widths in pixels, PathLayer's defaults otherwise.
pub const OVERVIEW_STYLE: PathStyle = PathStyle::PIXELS;

/// The stations' style: GeoJsonLayer's circles, filled and stroked, radius and line width in
/// pixels, on the ground.
pub const STATION_STYLE: CircleStyle = CircleStyle { filled: true, stroked: true, ..CircleStyle::PIXELS };

/// Row 4: the rail map (`railMap`).
pub struct Rail {
    network: TileSet<Decoded, TileLines>,
    speed: TileSet<Decoded, TileLines>,
    stations: TileSet<Vec<Station>>,
    overview: Overview,
    /// The globe's lines (`SURFACE_NO_CULL`) and the overview.
    globe: Path,
    globe_lines: path::Style,
    overview_style: path::Style,
    /// The flat map's lines, inside each tile's square ([`CLIPPED`]).
    clipped: Path,
    clipped_lines: path::Style,
    clip: Clip,
    /// The flat map's bridges, raised (`RAISED_NO_CULL`, T7).
    raised: Path,
    raised_lines: path::Style,
    circles: Circle,
    circle_style: circle::Style,
    station_instances: circle::Instances,
    atlas: GlyphAtlas,
    texture: AtlasTexture,
    text: TextPipeline,
    /// One text batch per tile drawn with labels, reused from frame to frame.
    batches: Vec<TextBatch>,
    /// This frame: whether it draws, the projection, which sublayers.
    shown: bool,
    flat: bool,
    layers: Vec<RailLayer>,
    overview_slot: Option<FrameSlot>,
    /// The tiles of lines drawn this frame in draw order, with their frames; the speed map's or the
    /// network's ([`Self::speeds_drawn`]).
    drawn: Vec<(FrameSlot, TileIndex)>,
    speeds_drawn: bool,
    /// Whether this frame's tiles of lines draw their speed labels (batch `k` for `drawn[k]`).
    labels_drawn: bool,
}

impl Rail {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let options = TileSetOptions { max_requests: MAX_REQUESTS, ..Default::default() };
        let network = init.tile_set(vec![Source::new(io::OPENFREEMAP)], DeckSelect(rails::NETWORK_TILES), options, |tile, bytes, cancel| {
            decode_network(tile, &bytes, cancel)
        });
        let speed =
            init.tile_set(vec![Source::new(RAIL_TILES)], DeckSelect(rails::SPEED_TILES), options, |tile, bytes, cancel| decode_speeds(tile, &bytes, cancel));
        let stations = init.tile_set(vec![Source::new(io::OPENFREEMAP)], DeckSelect(rails::STATION_TILES), options, |tile, bytes, cancel| {
            decode_stations(tile, &bytes, cancel)
        });
        let globe = Path::new(init, "rail map", DrawState::SURFACE_NO_CULL);
        let clipped = Path::new(init, "rail map clipped", CLIPPED);
        let globe_lines = globe.style(device, &line_style(8.0));
        let overview_style = globe.style(device, &OVERVIEW_STYLE);
        let clipped_lines = clipped.style(device, &line_style(8.0));
        let raised = Path::new(init, "rail map bridges", DrawState::RAISED_NO_CULL);
        let raised_lines = raised.style(device, &line_style(8.0));
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|[u, v]| fill::Vertex { position: [u, v, 0.0], color: [0; 4] });
        let mark = Fill::new(init, "rail map clip", CLIP_MARK);
        let reset = Fill::new(init, "rail map clip reset", CLIP_RESET);
        let clip = Clip {
            square: fill::Mesh::new(device, "rail map clip", &corners, &[0, 1, 2, 0, 2, 3]),
            mark_style: mark.style(device, fill::OPAQUE),
            reset_style: reset.style(device, fill::OPAQUE),
            mark,
            reset,
        };
        let circles = Circle::new(init, "rail stations", DrawState::SURFACE_NO_CULL);
        let circle_style = circles.style(device, &STATION_STYLE);
        let mut atlas = GlyphAtlas::new(FontSettings::DEFAULT);
        let texture = AtlasTexture::new(init.gpu, "speed label glyphs", &mut atlas);
        let text = TextPipeline::new(init, "speed labels", DrawState::LABEL);
        Rail {
            network,
            speed,
            stations,
            overview: Overview::Idle,
            globe,
            globe_lines,
            overview_style,
            clipped,
            clipped_lines,
            clip,
            raised,
            raised_lines,
            circles,
            circle_style,
            station_instances: circle::Instances::new(device, "rail stations"),
            atlas,
            texture,
            text,
            batches: Vec::new(),
            shown: false,
            flat: false,
            layers: Vec::new(),
            overview_slot: None,
            drawn: Vec::new(),
            speeds_drawn: false,
            labels_drawn: false,
        }
    }

    /// The sublayers drawn this frame (for tests).
    pub fn layers(&self) -> &[RailLayer] {
        &self.layers
    }

    /// The network's tiles (for tests).
    pub fn network(&self) -> &TileSet<Decoded, TileLines> {
        &self.network
    }

    /// The speed map's tiles (for tests).
    pub fn speeds(&self) -> &TileSet<Decoded, TileLines> {
        &self.speed
    }

    /// The stations' tiles (for tests).
    pub fn stations(&self) -> &TileSet<Vec<Station>> {
        &self.stations
    }

    /// How many stations and speed-label glyphs this frame draws (for tests).
    pub fn counts(&self) -> (u32, u32) {
        let glyphs = if self.labels_drawn { self.batches.iter().take(self.drawn.len()).map(TextBatch::len).sum() } else { 0 };
        (self.station_instances.count(), glyphs)
    }

    /// Whether the overview is in (for tests).
    pub fn overview_ready(&self) -> bool {
        matches!(self.overview, Overview::Ready(_))
    }

    /// Starts loading the overview on the worker pool the first time it is asked for, and takes
    /// it in once it is ready.
    fn poll_overview(&mut self, cx: &PrepareCx<'_>, wanted: bool) {
        if wanted && matches!(self.overview, Overview::Idle) {
            let job = cx.tiles.pool.spawn(0.0, |_| parse_rail_overview(RAILROADS).ok().map(|o| rails::overview_segments(&o, &OVERVIEW_COLORS)));
            self.overview = Overview::Loading(job);
        }
        if let Overview::Loading(job) = &mut self.overview {
            self.overview = match job.poll() {
                JobPoll::Pending => return,
                JobPoll::Done(Some(segments)) => Overview::Ready(path::Mesh::new(&cx.gpu.device, "rail overview", &segments)),
                // silently absent (`.catch(() => {})`)
                JobPoll::Done(None) | JobPoll::Cancelled | JobPoll::Lost => Overview::Failed,
            };
        }
    }

    /// This frame's speed labels: each drawn tile's labels inside its square, laid out and placed
    /// in batch `k` for tile `k`.
    fn place_labels(&mut self, cx: &PrepareCx<'_>) {
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        // every drawn tile's labels, laid out together (again if the atlas packs itself anew)
        let labels: Vec<Vec<&SpeedLabel>> =
            self.drawn.iter().map(|&(_, tile)| self.speed.get(tile).map_or(Vec::new(), |t| t.labels.iter().filter(|l| l.inside()).collect())).collect();
        let mut laid: Vec<LaidText> = Vec::new();
        for _ in 0..4 {
            let generation = self.atlas.generation();
            laid = labels.iter().flatten().map(|l| self.atlas.layout(SPEED_LABEL_FACE, &l.text, &TextLayout::DEFAULT)).collect();
            if self.atlas.generation() == generation {
                break;
            }
        }
        self.texture.sync(cx.gpu, &mut self.atlas);
        while self.batches.len() < self.drawn.len() {
            self.batches.push(self.text.batch(device, "speed labels", &self.texture));
        }
        let background = BackgroundStyle::DEFAULT.padded(&SPEED_LABEL_PADDING);
        let mut texts = laid.iter();
        for (k, &(_, tile)) in self.drawn.iter().enumerate() {
            let mut glyphs: Vec<Glyph> = Vec::new();
            let mut boxes: Vec<Background> = Vec::new();
            for l in &labels[k] {
                let Some(text) = texts.next() else { break };
                let [lon, lat] = rails::tile_lng_lat(tile, l.position);
                let label = Label::at(cx.view, [lon, lat, 0.0], SPEED_LABEL_SIZE_PX, SPEED_LABEL_TEXT);
                boxes.push(label.background(text, l.color));
                glyphs.extend(label.glyphs(text));
            }
            let batch = &mut self.batches[k];
            batch.set_style(queue, &TextStyle::DEFAULT, &background, &FontSettings::DEFAULT, cx.view);
            batch.set_glyphs(device, queue, &glyphs);
            batch.set_backgrounds(device, queue, &boxes);
        }
    }

    /// One frame's work for `input` (what [`Layer::prepare`] does with the scene's).
    pub fn prepare_with(&mut self, cx: &mut PrepareCx<'_>, input: &Input) {
        self.drawn.clear();
        self.overview_slot = None;
        self.labels_drawn = false;
        self.speeds_drawn = false;
        self.shown = input.shown;
        let zoom = cx.view.view.zoom;
        self.flat = !cx.view.is_globe();
        let speeds = input.speeds;
        // `railOverview()` is asked for only below zoom 8, and starts loading then
        self.poll_overview(cx, self.shown && zoom < RAIL_MIN_ZOOM);
        self.layers = if self.shown {
            rails::rail_layers(&RailOptions {
                // the tile URL is the fetcher's business (OpenFreeMap's TileJSON): always "known"
                tile_url: Some(io::OPENFREEMAP),
                flat: self.flat,
                zoom,
                overview: matches!(self.overview, Overview::Ready(_)),
                stations: true,
                open_railway_map: false,
                speeds,
            })
        } else {
            Vec::new()
        };
        let (mut network, mut speed, mut labels, mut stations) = (false, false, false, false);
        for layer in &self.layers {
            match *layer {
                RailLayer::Overview { .. } => self.overview_slot = Some(cx.tile_frame(TileIndex::ROOT, 0.0)),
                RailLayer::Network { .. } => network = true,
                RailLayer::Speed { labels: l, .. } => (speed, labels) = (true, l),
                RailLayer::Stations { .. } => stations = true,
                RailLayer::OpenRailwayMap => {}
            }
        }
        // a sublayer that leaves the list drops its tiles, as deck finalizes it
        for (on, set) in [(network, &mut self.network), (speed, &mut self.speed)] {
            if !on && !set.is_empty() {
                set.clear();
            }
        }
        if !stations && !self.stations.is_empty() {
            self.stations.clear();
        }
        let style = line_style(zoom);
        self.globe_lines.set(&cx.gpu.queue, &style);
        self.clipped_lines.set(&cx.gpu.queue, &style);
        self.raised_lines.set(&cx.gpu.queue, &style);
        if network || speed {
            let set = if speed { &mut self.speed } else { &mut self.network };
            cx.update_tiles(set, |gpu, _tile, decoded| upload(&gpu.device, decoded));
            let tiles: Vec<TileIndex> = set.visible().map(|(tile, _)| tile).collect();
            for tile in tiles {
                let slot = cx.tile_frame(tile, 0.0);
                self.drawn.push((slot, tile));
            }
            self.speeds_drawn = speed;
            if speed && labels {
                self.labels_drawn = true;
                self.place_labels(cx);
            }
        }
        let mut rings: Vec<circle::Instance> = Vec::new();
        if stations {
            cx.update_tiles(&mut self.stations, |_, _, decoded| decoded);
            for (_, tile) in self.stations.visible() {
                rings.extend(tile.iter().filter(|s| s.drawn(self.flat)).map(|s| {
                    circle::Instance::at(cx.view, s.lng_lat, 0.0).radius(s.radius).fill(STATION_FILL).line(STATION_RING, STATION_RING_WIDTH)
                }));
            }
        }
        self.station_instances.write(&cx.gpu.device, &cx.gpu.queue, &rings);
    }
}

impl Layer for Rail {
    fn name(&self) -> &'static str {
        "rail"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let input = cx.scene.rail.clone();
        self.prepare_with(cx, &input);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !cx.pass.draws(self.drape()) || !self.shown {
            return;
        }
        // stage 9: the lines (deck's PathLayers) draped over the 3D terrain, the labels and the
        // stations (a TextLayer, a ScatterplotLayer) standing in the map pass
        let (draped, standing) = (cx.draws_draped(), cx.pass == Pass::Map);
        if let (true, Some(slot), Overview::Ready(mesh)) = (draped, self.overview_slot, &self.overview) {
            self.globe.draw(pass, cx, slot, &self.overview_style, mesh);
        }
        let set = if self.speeds_drawn { &self.speed } else { &self.network };
        for (k, &(slot, tile)) in self.drawn.iter().enumerate() {
            let Some(t) = set.get(tile) else { continue };
            if draped && self.flat {
                // a tile with bridges: its lines on the ground, then `-bridges` raised (T7)
                let ground = t.bridges.as_ref().map_or(&t.lines, |b| &b.ground);
                if !ground.is_empty() {
                    let c = &self.clip;
                    c.mark.draw(pass, cx, slot, &c.mark_style, &c.square);
                    self.clipped.draw(pass, cx, slot, &self.clipped_lines, ground);
                    c.reset.draw(pass, cx, slot, &c.reset_style, &c.square);
                }
                if let Some(b) = &t.bridges {
                    self.raised.draw(pass, cx, slot, &self.raised_lines, &b.raised);
                }
            } else if draped && !t.lines.is_empty() {
                self.globe.draw(pass, cx, slot, &self.globe_lines, &t.lines);
            }
            // the tile's speed labels, after its lines and before the next tile's
            if standing
                && self.labels_drawn
                && let Some(batch) = self.batches.get(k)
                && !batch.is_empty()
            {
                self.text.draw(pass, batch);
            }
        }
        if standing {
            self.circles.draw(pass, &self.circle_style, &self.station_instances);
        }
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Part
    }

    fn pending(&self) -> bool {
        if !self.shown {
            return false;
        }
        let drawn = |wanted: fn(&RailLayer) -> bool| self.layers.iter().any(wanted);
        matches!(self.overview, Overview::Loading(_))
            || (drawn(|l| matches!(l, RailLayer::Network { .. })) && self.network.loading())
            || (drawn(|l| matches!(l, RailLayer::Speed { .. })) && self.speed.loading())
            || (drawn(|l| matches!(l, RailLayer::Stations { .. })) && self.stations.loading())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path as FilePath;
    use std::rc::Rc;
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, viewport_for};
    use rustradar_scene::rail::{RAIL_MAIN, RAIL_MINOR, RAIL_TRANSIT, Rgba, SPEED_BANDS, tunnel_of};
    use rustradar_scene::bridges::RAIL_DECK_M;
    use rustradar_scene::tess::vector::{LAND, WATER};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FetchRequest, FetchResult, TileFetcher, answered};
    use crate::layers::earth;
    use crate::renderer::{MapRenderer, SceneInput};

    fn view(latitude: f64, longitude: f64, zoom: f64) -> ViewState {
        ViewState { latitude, longitude, zoom, pitch: 0.0, bearing: 0.0, position: None }
    }

    /// The committed fixtures with every request noted, and the backend's rail tiles from `rail`.
    struct Tiles {
        dir: DirTiles,
        rail: Box<dyn Fn(TileIndex) -> Option<Vec<u8>> + Send + Sync>,
        asked: Mutex<Vec<(String, TileIndex)>>,
    }

    impl Tiles {
        fn new(rail: impl Fn(TileIndex) -> Option<Vec<u8>> + Send + Sync + 'static) -> Arc<Self> {
            Arc::new(Tiles { dir: DirTiles::fixtures(), rail: Box::new(rail), asked: Mutex::new(Vec::new()) })
        }

        fn asked(&self) -> Vec<(String, TileIndex)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl TileFetcher for Tiles {
        fn fetch(&self, request: FetchRequest) -> Receiver<FetchResult> {
            self.asked.lock().unwrap().push((request.source.to_string(), request.tile));
            if &*request.source == RAIL_TILES { answered(Ok((self.rail)(request.tile))) } else { self.dir.fetch(request) }
        }
    }

    fn renderer(gpu: &crate::gpu::Gpu, tiles: Arc<Tiles>) -> MapRenderer {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(tiles, None);
        renderer
    }

    /// The dark map's scene: the earth and the land, the vector map if `map`, the rail map as
    /// `rail` says.
    fn dark(view: ViewState, size: [f64; 2], map: bool, rail: Input) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = crate::layers::vector_map::Input { shown: map };
        scene.rail = rail;
        scene
    }

    const NETWORK: Input = Input { shown: true, speeds: false };
    const SPEEDS: Input = Input { shown: true, speeds: true };
    const OFF: Input = Input { shown: false, speeds: true };

    fn save(image: &Image, name: &str) {
        let dir = FilePath::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        image.save_png(&dir.join(name)).expect("the shot written");
    }

    /// `c` drawn over `under` with deck's blending (alpha 0–255).
    fn over(c: Rgba, under: [u8; 3]) -> [f64; 3] {
        let a = f64::from(c[3]) / 255.0;
        [0, 1, 2].map(|i| f64::from(c[i]) * a + f64::from(under[i]) * (1.0 - a))
    }

    fn near(p: [u8; 4], rgb: [f64; 3], tolerance: f64) -> bool {
        (0..3).all(|i| (f64::from(p[i]) - rgb[i]).abs() <= tolerance)
    }

    /// Pixels in `[x0, x1) × [y0, y1)` within `tolerance` of `rgb`.
    fn count(image: &Image, [x0, y0, x1, y1]: [u32; 4], rgb: [f64; 3], tolerance: f64) -> usize {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| near(image.pixel(x, y), rgb, tolerance)).count()
    }

    fn land() -> [u8; 3] {
        [LAND[0], LAND[1], LAND[2]]
    }

    /// Pixels of a coral hue (red well above green and blue): the main lines at any coverage.
    fn coralish(image: &Image) -> usize {
        image.rgba.chunks_exact(4).filter(|p| p[0] > 120 && i32::from(p[0]) - i32::from(p[1]) > 60 && i32::from(p[0]) - i32::from(p[2]) > 70).count()
    }

    /// Plan section 8's V3: the Stockholm region.
    fn v3() -> ViewState {
        view(59.33, 18.07, 9.0)
    }

    /// T4's acceptance, first half: V3 with the rail map on (the network, as while it shows the
    /// signal blocks; the speed map's tiles are not among the fixtures): the main lines are drawn
    /// in `RAIL_MAIN` over the dark map, from the committed tiles, and nothing like it without.
    #[test]
    fn v3_draws_the_main_lines_in_their_coral() {
        let Some(gpu) = gpu() else { return };
        let tiles = Tiles::new(|_| None);
        let mut renderer = renderer(&gpu, tiles.clone());
        let on = render_with(&gpu, &mut renderer, &dark(v3(), [1440.0, 900.0], true, NETWORK), 1.0).expect("rendered");
        save(&on, "t4-rail-v3-1440x900.png");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        assert!(tiles.dir.misses().is_empty(), "the fixtures lack {:?}", tiles.dir.misses());
        assert!(tiles.asked().iter().all(|(source, t)| source == io::OPENFREEMAP && t.z == 9), "{:?}", tiles.asked());
        let off = render_with(&gpu, &mut renderer, &dark(v3(), [1440.0, 900.0], true, OFF), 1.0).expect("rendered");
        // coral: `RAIL_MAIN` over the grey map, at any coverage (today's picture with the rail map
        // on has 4537 such pixels right of its sidebar; this shot 4866 there)
        let coral = coralish(&on);
        let without = coralish(&off);
        eprintln!("coral pixels: {coral} with the rail map, {without} without");
        assert!(coral > 4_000, "{coral}");
        assert_eq!(without, 0);
        // a main line's middle is `RAIL_MAIN` at alpha 245, or the colour itself where two tracks
        // overlap (today's commonest coral pixel is (240, 112, 96) too)
        assert!(count(&on, [0, 0, 1440, 900], [240.0, 112.0, 96.0], 1.0) > 300);
        // the lines run through Stockholm Central: coral within 12 px of it
        let [x, y, _] = viewport_for(&v3(), [1440.0, 900.0]).project([18.0575, 59.3299, 0.0]);
        let (x, y) = (x.round() as u32, y.round() as u32);
        assert!(count(&on, [x - 12, y - 12, x + 12, y + 12], over(RAIL_MAIN, land()), 12.0) > 0);
    }

    /// T4's acceptance, second half: at zoom 6 the overview draws, main lines `RAIL_MAIN` and the
    /// rest `RAIL_MINOR`, and asks for no tile; at zoom 8 it hands over to the tiles (the
    /// fixtures have none at z8 here: nothing of the overview is left).
    #[test]
    fn at_zoom_6_the_overview_draws_and_at_8_it_hands_over() {
        let Some(gpu) = gpu() else { return };
        let tiles = Tiles::new(|_| None);
        let mut renderer = renderer(&gpu, tiles.clone());
        let z6 = view(59.33, 18.07, 6.0);
        let image = render_with(&gpu, &mut renderer, &dark(z6, [1440.0, 900.0], false, SPEEDS), 1.0).expect("rendered");
        save(&image, "t4-rail-overview-z6-1440x900.png");
        assert!(!renderer.pending());
        assert!(tiles.asked().is_empty(), "{:?}", tiles.asked());
        let all = [0, 0, 1440, 900];
        let main = count(&image, all, over(RAIL_MAIN, land()), 3.0);
        let minor = count(&image, all, over(RAIL_MINOR, land()), 3.0);
        eprintln!("overview pixels: {main} main, {minor} minor");
        assert!(main > 1_000 && minor > 300, "{main} {minor}");
        let z8 = view(59.33, 18.07, 8.0);
        let image = render_with(&gpu, &mut renderer, &dark(z8, [1440.0, 900.0], false, SPEEDS), 1.0).expect("rendered");
        assert_eq!(count(&image, all, over(RAIL_MAIN, land()), 6.0) + count(&image, all, over(RAIL_MINOR, land()), 6.0), 0);
        assert!(tiles.asked().iter().all(|(source, t)| source == RAIL_TILES && t.z == 8), "{:?}", tiles.asked());
    }

    // --- a small vector tile encoder for the speed map's tiles ----------------------------------

    #[derive(Clone, Copy)]
    enum V {
        S(&'static str),
        U(u64),
        B(bool),
    }

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push((v as u8) | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn field_bytes(out: &mut Vec<u8>, field: u64, b: &[u8]) {
        varint(out, (field << 3) | 2);
        varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }

    fn field_uint(out: &mut Vec<u8>, field: u64, v: u64) {
        varint(out, field << 3);
        varint(out, v);
    }

    fn field_packed(out: &mut Vec<u8>, field: u64, values: &[u64]) {
        let mut body = Vec::new();
        for &v in values {
            varint(&mut body, v);
        }
        field_bytes(out, field, &body);
    }

    /// A line of a test tile: its tags and points (units of 4096).
    type TestLine<'a> = (&'a [(&'static str, V)], &'a [(i64, i64)]);

    /// A tile of one layer `name` of lines.
    fn lines_tile(name: &str, lines: &[TestLine<'_>]) -> Vec<u8> {
        let zigzag = |v: i64| ((v << 1) ^ (v >> 63)) as u64;
        let (mut keys, mut values): (Vec<&str>, Vec<Vec<u8>>) = (Vec::new(), Vec::new());
        let mut layer = Vec::new();
        field_uint(&mut layer, 15, 2);
        field_bytes(&mut layer, 1, name.as_bytes());
        for (tags, points) in lines {
            let mut t = Vec::new();
            for &(k, v) in *tags {
                let ki = keys.iter().position(|&x| x == k).unwrap_or_else(|| {
                    keys.push(k);
                    keys.len() - 1
                });
                let mut value = Vec::new();
                match v {
                    V::S(s) => field_bytes(&mut value, 1, s.as_bytes()),
                    V::U(n) => field_uint(&mut value, 5, n),
                    V::B(b) => field_uint(&mut value, 7, u64::from(b)),
                }
                values.push(value);
                t.extend([ki as u64, values.len() as u64 - 1]);
            }
            let mut geometry = vec![(1 << 3) | 1];
            let (mut cx, mut cy) = (0, 0);
            for (k, &(x, y)) in points.iter().enumerate() {
                if k == 1 {
                    geometry.push(((points.len() as u64 - 1) << 3) | 2);
                }
                geometry.extend([zigzag(x - cx), zigzag(y - cy)]);
                (cx, cy) = (x, y);
            }
            let mut feature = Vec::new();
            field_packed(&mut feature, 2, &t);
            field_uint(&mut feature, 3, 2);
            field_packed(&mut feature, 4, &geometry);
            field_bytes(&mut layer, 2, &feature);
        }
        for k in &keys {
            field_bytes(&mut layer, 3, k.as_bytes());
        }
        for v in &values {
            field_bytes(&mut layer, 4, v);
        }
        field_uint(&mut layer, 5, 4096);
        let mut tile = Vec::new();
        field_bytes(&mut tile, 3, &layer);
        tile
    }

    /// The z12 tile over Stockholm Central, and the view on its middle.
    const SPEED_TILE: TileIndex = TileIndex::new(2253, 1204, 12);

    fn tile_middle(zoom: f64) -> ViewState {
        let [lon, lat] = rustradar_scene::rail::tile_lng_lat(SPEED_TILE, [0.5, 0.5]);
        view(lat, lon, zoom)
    }

    /// Three tracks across the tile: 160 km/h through its middle, a 40 km/h tunnel a tenth of
    /// the tile below, a tram a tenth above.
    fn speed_tracks() -> Vec<u8> {
        let (s, u) = (V::S, V::U);
        lines_tile(
            SPEED_TILE_LAYER_NAME,
            &[
                (&[("feature", s("rail")), ("maxspeed", u(160))], &[(-64, 2048), (4160, 2048)]),
                (&[("feature", s("rail")), ("maxspeed", u(40)), ("tunnel", V::B(true))], &[(-64, 2458), (4160, 2458)]),
                (&[("feature", s("tram")), ("maxspeed", u(50))], &[(-64, 1638), (4160, 1638)]),
            ],
        )
    }

    const SPEED_TILE_LAYER_NAME: &str = rustradar_scene::rail::SPEED_TILE_LAYER;

    /// The speed map close in: tracks by their limits, the tunnel dashed and faint, the tram
    /// violet, and from zoom 12.5 the limits written on the lines on their colours.
    #[test]
    fn the_speed_map_colours_tracks_by_limit_and_writes_the_limits_from_12_5() {
        let Some(gpu) = gpu() else { return };
        let tracks = speed_tracks();
        let tiles = Tiles::new(move |t| (t == SPEED_TILE).then(|| tracks.clone()));
        let mut renderer = renderer(&gpu, tiles.clone());
        let size = [800.0, 600.0];
        let image = render_with(&gpu, &mut renderer, &dark(tile_middle(13.0), size, false, SPEEDS), 1.0).expect("rendered");
        save(&image, "t4-rail-speeds-z13-800x600.png");
        assert!(tiles.asked().iter().any(|(source, t)| source == RAIL_TILES && *t == SPEED_TILE), "{:?}", tiles.asked());
        // at zoom 13 the z12 tile is 1024 px across: the middle line on row 300, the tunnel 102 px
        // below, the tram 102 px above
        let row = |y: u32, rgb: [f64; 3]| count(&image, [40, y - 3, 340, y + 4], rgb, 4.0);
        assert!(row(300, over(SPEED_BANDS[3].color, land())) > 200, "the 160 km/h line");
        assert!(row(198, over(RAIL_TRANSIT, land())) > 200, "the tram");
        let tunnel = over(tunnel_of(SPEED_BANDS[0].color), land());
        assert!(row(402, tunnel) > 50, "the tunnel");
        // dashed: along the tunnel's middle the colour comes and goes
        let on: Vec<bool> = (40..760).map(|x| (399..406).any(|y| near(image.pixel(x, y), tunnel, 6.0))).collect();
        let dashes = on.windows(2).filter(|w| w[0] && !w[1]).count();
        assert!(dashes >= 10, "{dashes} dashes");
        // the limit on its line, halfway along (the canvas's middle): a box of the band's colour
        // with dark digits (10 px glyphs without an SDF: their strokes mix with the box)
        let box_colour = over(SPEED_BANDS[3].color, land());
        let in_box = count(&image, [385, 294, 415, 306], box_colour, 6.0);
        let digits = |image: &Image| {
            (389..411).flat_map(|x| (295..303).map(move |y| (x, y))).filter(|&(x, y)| {
                let p = image.pixel(x, y);
                p[0] < 120 && i32::from(p[0]) - i32::from(p[2]) > 10
            })
            .count()
        };
        eprintln!("label: {in_box} box pixels, {} digit pixels", digits(&image));
        assert!(in_box > 40 && digits(&image) > 8, "{in_box} {}", digits(&image));
        // and below 12.5 no label: no digits on the line
        let image = render_with(&gpu, &mut renderer, &dark(tile_middle(12.4), size, false, SPEEDS), 1.0).expect("rendered");
        assert_eq!(digits(&image), 0);
    }

    /// Stockholm Central close in (z14 fixture): a station ring in `STATION_FILL` with a dark rim.
    #[test]
    fn stockholm_central_is_a_ring_close_in() {
        let Some(gpu) = gpu() else { return };
        let tiles = Tiles::new(|_| None);
        let mut renderer = renderer(&gpu, tiles.clone());
        // the station's point in the z14 tile 9013/4818
        let centre = view(59.32987585608794, 18.05750012397766, 14.0);
        let image = render_with(&gpu, &mut renderer, &dark(centre, [400.0, 300.0], false, SPEEDS), 1.0).expect("rendered");
        save(&image, "t4-rail-station-z14-400x300.png");
        assert!(tiles.dir.misses().iter().all(|(source, _)| source != io::OPENFREEMAP), "{:?}", tiles.dir.misses());
        // the canvas's middle is the corner between pixels 199 and 200; deck's circle: the rim's
        // middle at the radius, 3.5 px, a 1.5 px rim, the fill inside 2.75 px
        let fill = [214.0, 220.0, 232.0];
        for x in [198, 199, 200, 201] {
            assert!(near(image.pixel(x, 150), fill, 2.0), "{x}: {:?}", image.pixel(x, 150));
        }
        let rim = over(STATION_RING, land());
        for x in [196, 203] {
            assert!(near(image.pixel(x, 150), rim, 3.0), "{x}: {:?}", image.pixel(x, 150));
        }
        assert!(near(image.pixel(206, 150), [50.0; 3], 0.0), "{:?}", image.pixel(206, 150));
    }

    /// A probe drawing a rail layer of its own with an input of its own (the scene's rail map off),
    /// to look at what it decided.
    struct Probe(Rc<RefCell<Rail>>, Rc<RefCell<Input>>);

    impl Layer for Probe {
        fn name(&self) -> &'static str {
            "rail probe"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            let input = self.1.borrow().clone();
            self.0.borrow_mut().prepare_with(cx, &input);
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.0.borrow().draw(pass, cx);
        }

        fn pending(&self) -> bool {
            self.0.borrow().pending()
        }
    }

    /// Which sublayers draw at which zoom, and a sublayer that leaves the list drops its tiles.
    #[test]
    fn sublayers_by_zoom_and_tiles_dropped_when_their_layer_goes() {
        let Some(gpu) = gpu() else { return };
        let tracks = speed_tracks();
        let tiles = Tiles::new(move |t| (t == SPEED_TILE).then(|| tracks.clone()));
        let mut renderer = renderer(&gpu, tiles.clone());
        let rail = Rc::new(RefCell::new(Rail::new(&renderer.init(&gpu))));
        let input = Rc::new(RefCell::new(SPEEDS));
        renderer.add_layer(Box::new(Probe(rail.clone(), input.clone())));
        let size = [800.0, 600.0];
        let ids = |rail: &Rail| rail.layers().iter().map(RailLayer::id).collect::<Vec<_>>();
        render_with(&gpu, &mut renderer, &dark(tile_middle(13.0), size, false, OFF), 1.0).expect("rendered");
        assert_eq!(ids(&rail.borrow()), ["rail-speed-flat", "rail-stations-flat"]);
        let (_, glyphs) = rail.borrow().counts();
        assert_eq!(glyphs, 5, "160 and 40");
        assert!(!rail.borrow().speeds().is_empty());
        // the network instead (the rail map showing the blocks): the speed map's tiles go
        *input.borrow_mut() = NETWORK;
        render_with(&gpu, &mut renderer, &dark(tile_middle(13.0), size, false, OFF), 1.0).expect("rendered");
        assert_eq!(ids(&rail.borrow()), ["rail-flat", "rail-stations-flat"]);
        assert!(rail.borrow().speeds().is_empty() && !rail.borrow().network().is_empty());
        // zoomed out: the overview, once in; the tiles go
        render_with(&gpu, &mut renderer, &dark(view(59.33, 18.07, 6.0), size, false, OFF), 1.0).expect("rendered");
        assert_eq!(ids(&rail.borrow()), ["rail-overview-globe"]);
        assert!(rail.borrow().overview_ready());
        assert!(rail.borrow().network().is_empty() && rail.borrow().stations().is_empty());
        // the rail map off: nothing, nothing pending
        *input.borrow_mut() = OFF;
        render_with(&gpu, &mut renderer, &dark(v3(), size, false, OFF), 1.0).expect("rendered");
        assert!(ids(&rail.borrow()).is_empty() && !rail.borrow().pending());
    }

    #[test]
    fn a_fixture_tile_decodes_on_the_pool_s_terms() {
        let tile = TileIndex::new(281, 150, 9);
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, tile).expect("read").expect("a V3 tile");
        let decoded = decode_network(tile, &bytes, &crate::jobs::Cancel::new()).expect("decoded").expect("rail in it");
        assert_eq!(ByteSize::byte_size(&decoded), decoded.byte_size());
        assert_eq!(decode_network(tile, &[], &crate::jobs::Cancel::new()), Ok(None));
        assert_eq!(decode_speeds(SPEED_TILE, &[], &crate::jobs::Cancel::new()), Ok(None));
        let cancel = crate::jobs::Cancel::new();
        cancel.cancel();
        assert!(decode_network(tile, &bytes, &cancel).is_err());
        let central = TileIndex::new(9013, 4818, 14);
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, central).expect("read").expect("a z14 tile");
        assert!(decode_stations(central, &bytes, &crate::jobs::Cancel::new()).expect("decoded").is_some_and(|s| !s.is_empty()));
        assert!(decode_stations(central, &bytes, &cancel).is_err());
        // the overview file is the scene's parser's
        assert!(parse_rail_overview(RAILROADS).is_ok());
        // T7: a globe tile is not raised; the flat map's tile of Söderström's bridges is, and
        // keeps the lines both projections draw
        assert!(decoded.bridges.is_none());
        let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, SODERSTROM).expect("read").expect("a z14 tile");
        let decoded = decode_network(SODERSTROM, &bytes, &crate::jobs::Cancel::new()).expect("decoded").expect("rail in it");
        let bridges = decoded.bridges.as_ref().expect("bridges");
        assert!(!decoded.tile.lines.is_empty() && !bridges.ground.is_empty() && !bridges.raised.is_empty());
        assert!(bridges.ground.len() < decoded.tile.lines.len());
        assert_eq!(decoded.byte_size(), decoded.tile.byte_size() + bridges.byte_size());
        assert!(decode_network(SODERSTROM, &bytes, &cancel).is_err());
    }

    /// The z14 tile of Söderström, between Gamla stan and Södermalm: the main line's bridge
    /// (Järnvägsbron) and the subway's over the water, the rail and subway tunnels into Söder.
    const SODERSTROM: TileIndex = TileIndex::new(9014, 4818, 14);

    /// The centre of the rows of `colour` (within `tolerance`) in column `x` between `y0` and
    /// `y1`, or `None`.
    fn line_centre(image: &Image, x: f64, [y0, y1]: [f64; 2], colour: [f64; 3], tolerance: f64) -> Option<f64> {
        let x = x.floor() as u32;
        let rows: Vec<u32> = (y0.floor() as u32..y1.ceil() as u32).filter(|&y| near(image.pixel(x, y), colour, tolerance)).collect();
        (!rows.is_empty()).then(|| rows.iter().map(|&y| f64::from(y) + 0.5).sum::<f64>() / rows.len() as f64)
    }

    /// T7, today's look: on the flat map a tile's bridges are drawn after its other lines (over a
    /// track the tile has after them), where their ground is (6 m decks 0.15 mm up), not 6 m up.
    /// A made-up speed tile: a 160 km/h track east-west through the tile's middle, a 40 km/h track
    /// north-south after it, crossing it there; seen pitched 60° close in, where 6 m are 17 px up
    /// the screen.
    #[test]
    fn a_tiles_bridges_draw_over_its_other_lines_where_their_ground_is() {
        let Some(gpu) = gpu() else { return };
        let tile = |bridge: bool| {
            let mut east = vec![("feature", V::S("rail")), ("maxspeed", V::U(160))];
            if bridge {
                east.push(("bridge", V::B(true)));
            }
            let north: &[(&str, V)] = &[("feature", V::S("rail")), ("maxspeed", V::U(40))];
            // (their limits are written halfway along, well off the canvas)
            lines_tile(SPEED_TILE_LAYER_NAME, &[(&east, &[(1024, 2048), (4096, 2048)]), (north, &[(2048, 4096), (2048, 1024)])])
        };
        let size = [800.0, 600.0];
        let view = ViewState { pitch: 60.0, ..tile_middle(17.0) };
        let viewport = viewport_for(&view, size);
        let lat = rustradar_scene::rail::tile_lng_lat(SPEED_TILE, [0.5, 0.5])[1];
        let tile_metres = rustradar_core::geo::EARTH_CIRCUMFERENCE_M * lat.to_radians().cos() / 4096.0;
        // `d` metres east of the crossing, `h` up, on the screen
        let at = |d: f64, h: f64| viewport.project([rustradar_scene::rail::tile_lng_lat(SPEED_TILE, [0.5 + d / tile_metres, 0.5])[0], lat, h]);
        assert!(at(100.0, 0.0)[1] - at(100.0, RAIL_DECK_M)[1] > 15.0, "6 m show well clear of the ground here");
        let yellow = over(SPEED_BANDS[3].color, land());
        let blue = |p: [u8; 4]| p[2] > 200 && p[0] < 150;
        let mut seen = Vec::new();
        for bridge in [true, false] {
            let bytes = tile(bridge);
            let tiles = Tiles::new(move |t| (t == SPEED_TILE).then(|| bytes.clone()));
            let mut renderer = renderer(&gpu, tiles);
            let image = render_with(&gpu, &mut renderer, &dark(view, size, false, SPEEDS), 1.0).expect("rendered");
            if bridge {
                save(&image, "t7-rail-bridge-pitched-800x600.png");
            }
            // the east-west track's rows 100 m east and 100 m west of the crossing
            let rows = [100.0, -100.0].map(|d| {
                let (ground, deck) = (at(d, 0.0), at(d, RAIL_DECK_M));
                (line_centre(&image, ground[0], [deck[1] - 12.0, ground[1] + 12.0], yellow, 10.0), ground[1])
            });
            // the crossing, at the canvas's middle: whose colour is on top
            let crossing = [image.pixel(399, 299), image.pixel(400, 300)];
            seen.push((rows, crossing));
        }
        eprintln!("(rows, crossing) with the bridge, without: {seen:?}");
        for (k, (rows, crossing)) in seen.iter().enumerate() {
            // where its ground is, bridge or not
            for (centre, ground) in rows {
                let y = centre.expect("the track");
                assert!((y - ground).abs() <= 1.0, "{y} for its ground {ground}");
            }
            // the bridge over the track that comes after it in the tile; without it, in the
            // tile's order: the later track on top
            let on_top = crossing.iter().all(|&p| if k == 0 { near(p, yellow, 12.0) } else { blue(p) });
            assert!(on_top, "{}: {crossing:?}", if k == 0 { "bridge" } else { "no bridge" });
        }
    }

    /// T7's acceptance shot: Söderström looking east-north-east, pitched 55°, the network (as
    /// while the rail map shows the blocks: the speed map's tiles are not among the fixtures). The
    /// main line crosses the water on Järnvägsbron as today's app draws it: over the water where
    /// its ground is (its 6 m decks 0.15 mm up: the TypeScript's common units read as metres), not
    /// 6 m up, and straight on into the tunnel on Söder (no ramp down to it); the tunnel draws
    /// faint and dashed. Today's app at this view (a scratch capture with the rail map on): the
    /// main line's two tracks at the same rows over the water and into the tunnel's mouth.
    #[test]
    fn the_main_line_crosses_soderstrom_on_its_bridge_as_today() {
        let Some(gpu) = gpu() else { return };
        let tiles = Tiles::new(|_| None);
        let mut renderer = renderer(&gpu, tiles.clone());
        let size = [1100.0, 720.0];
        let view = ViewState { latitude: 59.3214, longitude: 18.0680, zoom: 17.0, pitch: 55.0, bearing: 73.0, position: None };
        let image = render_with(&gpu, &mut renderer, &dark(view, size, true, NETWORK), 1.0).expect("rendered");
        save(&image, "t7-soderstrom-z17-1100x720.png");
        assert!(tiles.dir.misses().iter().all(|(source, _)| source != io::OPENFREEMAP), "{:?}", tiles.dir.misses());
        let off = render_with(&gpu, &mut renderer, &dark(view, size, true, OFF), 1.0).expect("rendered");
        // where the main line's bridges are on their decks and its tunnels run, from the two
        // tiles the bridge spans (as the layer decoded them)
        let viewport = viewport_for(&view, size);
        let on_screen = |p: [f64; 3]| (2.0..size[0] - 2.0).contains(&p[0]) && (2.0..size[1] - 2.0).contains(&p[1]);
        let (mut decks, mut tunnels) = (Vec::new(), Vec::new());
        for tile in [SODERSTROM, TileIndex::new(9014, 4819, 14)] {
            let bytes = DirTiles::fixtures().read(io::OPENFREEMAP, tile).expect("read").expect("a z14 tile");
            let parts = bridges::network_bridges(tile, &bytes, &|| false).expect("decoded").expect("bridges");
            let top = (RAIL_DECK_M * bridges::rail_map_lift(tile)) as f32;
            let lng_lat = |p: [f32; 3]| rustradar_scene::rail::tile_lng_lat(tile, [f64::from(p[0]), f64::from(p[1])]);
            decks.extend(parts.raised.iter().filter(|s| s.color == RAIL_MAIN && s.start[2] == top).map(|s| lng_lat(s.start)));
            for s in parts.ground.iter().filter(|s| s.color == tunnel_of(RAIL_MAIN)) {
                tunnels.extend((0..200).map(|k| {
                    let t = f64::from(k) / 200.0;
                    lng_lat([0, 1, 2].map(|i| s.start[i] + (s.end[i] - s.start[i]) * t as f32))
                }));
            }
        }
        let pixel = |image: &Image, p: [f64; 3]| image.pixel(p[0] as u32, p[1] as u32);
        let coral = |p: [u8; 4]| p[0] > 120 && i32::from(p[0]) - i32::from(p[1]) > 60 && i32::from(p[0]) - i32::from(p[2]) > 70;
        let water = |p: [u8; 4]| near(p, [f64::from(WATER[0]); 3], 3.0);
        let decks: Vec<([f64; 3], [f64; 3])> = decks
            .iter()
            .map(|&[lon, lat]| (viewport.project([lon, lat, 0.0]), viewport.project([lon, lat, RAIL_DECK_M])))
            .filter(|&(ground, up)| on_screen(ground) && on_screen(up))
            .collect();
        let share = |hits: usize| hits as f64 / decks.len() as f64;
        let ground = share(decks.iter().filter(|(p, _)| coral(pixel(&image, *p))).count());
        let up = share(decks.iter().filter(|(_, p)| coral(pixel(&image, *p))).count());
        let over_water = share(decks.iter().filter(|(p, _)| water(pixel(&off, *p))).count());
        let lift = decks.iter().map(|(p, q)| p[1] - q[1]).fold(f64::INFINITY, f64::min);
        eprintln!("{} deck samples: {ground:.3} drawn on their ground, {up:.3} 6 m up, {over_water:.3} over water; 6 m would be {lift:.1} px up", decks.len());
        assert!(decks.len() >= 40, "{}", decks.len());
        assert!(lift > 10.0, "{lift}");
        assert!(ground >= 0.95, "{ground}");
        assert!(up <= 0.05, "{up}");
        assert!(over_water >= 0.6, "{over_water}");
        // the tunnel: dashed, in the tunnel's faint colour, hardly ever the line's own (the
        // bridge's line meets it at the tunnel's mouth)
        let tunnel: Vec<[u8; 4]> = tunnels.iter().map(|&[lon, lat]| viewport.project([lon, lat, 0.0])).filter(|&p| on_screen(p)).map(|p| pixel(&image, p)).collect();
        let faint = |p: [u8; 4]| (110..190).contains(&p[0]) && i32::from(p[0]) - i32::from(p[1]) > 45;
        let dashed: Vec<bool> = tunnel.iter().map(|&p| faint(p)).collect();
        let dashes = dashed.windows(2).filter(|w| w[0] && !w[1]).count();
        let lit = dashed.iter().filter(|&&on| on).count() as f64 / dashed.len().max(1) as f64;
        let full = tunnel.iter().filter(|&&p| p[0] >= 200).count() as f64 / tunnel.len().max(1) as f64;
        eprintln!("{} tunnel samples: {lit:.3} in its colour, {full:.3} in the line's own, {dashes} dashes", tunnel.len());
        assert!(tunnel.len() >= 200, "{}", tunnel.len());
        assert!(full <= 0.05, "{full}");
        assert!((0.25..0.9).contains(&lit), "{lit}");
        assert!(dashes >= 10, "{dashes}");
    }

    #[test]
    fn the_line_style_follows_the_zoom() {
        assert_eq!(line_style(8.0).width_scale, 0.8);
        assert_eq!(line_style(13.0).width_scale, 1.5);
        let s = line_style(20.0);
        assert_eq!((s.width_scale, s.cap_rounded, s.joint_rounded, s.dash, s.dash_justified), (1.8, true, false, Dash::Segment, true));
        assert_eq!(s.width_units, crate::pipelines::path::Units::Pixels);
    }
}
