//! Signal blocks and signals (item T5; doc 02 §6.4; row 12 of doc 03 §3, row 9 of doc 02 §2), as
//! today's `blockLayers` (G:2264-2386) draws them with deck.gl 9.4's PathLayer, ScatterplotLayer
//! and TextLayer.
//!
//! The scene builder hands over each refresh's [`BlocksScene`] ([`rustradar_scene::blocks`]): the
//! block set's track as drawn (on the ground and bridges, in tunnels), its signals with their
//! blocks, the taken blocks, whether the signals' names show, and the taken blocks' labels. This
//! layer is the six deck layers, in this order:
//!
//! | Layer | Draws | Style | Draw state |
//! |---|---|---|---|
//! | `train-blocks-casing` | the `above` runs | [`BLOCK_CASING`], 3 px wider than the block, round caps | `SURFACE_NO_CULL` |
//! | `train-blocks` | the `above` runs | free [`BLOCK_FREE`] 3 px, taken [`BLOCK_TAKEN`] 5 px, round caps | `SURFACE_NO_CULL` |
//! | `train-blocks-tunnels` | the `below` runs | the block's colour at half its alpha, its width, butt caps, dashes `[2.5, 2]` along the whole path, justified | `LABEL` |
//! | `train-signals` | a disc per signal | billboard, radius 6 px, fill by aspect, 2 px [`BLOCK_CASING`] outline | `LABEL` |
//! | `train-signal-names` | the named signals, from zoom 15.5 | 10 px, `[235,238,245,255]`, anchored at its start 10 px right of the signal, box `[16,18,24,200]` padded `[3, 1]` | `LABEL` |
//! | `train-block-labels` | one per taken block | 11 px white, centred, box `[150,30,24,230]` padded `[4, 2]` | `LABEL` |
//!
//! `SURFACE_NO_CULL` is today's `ground` (`{cullMode: "none", depthWriteEnabled: false}`: on the
//! ground and bridges, hidden by buildings and road bridges in front, `getPolygonOffset: null`);
//! `LABEL` is its `top` (the same with `depthCompare: "always"`: tunnels faint and dashed, seen
//! through the ground; signals and texts over everything). Widths are in pixels, lying on the
//! ground (deck's PathLayer); the paths are 3D, so they rise over rail bridges. Dash arrays count
//! half line widths, as the path pipeline reads deck's `dashUnits: "widths"`.
//!
//! The texts are deck's TextLayer defaults but for the font: no outline, not SDF (deck's default
//! `fontSettings`: an alpha atlas at 64 px, buffer 4), billboards, middle and centre unless said.
//! Today's font is `ui-monospace, monospace` at the normal weight, which is whatever monospace the
//! system has; here Overpass Mono 400 (plan 1.4.10 and section 6).
//!
//! The lines are made again when the set or the taken blocks change, all in one local Mercator
//! frame round the set; the discs and texts are placed at every frame for that frame's camera.
//!
//! On the 3D terrain (item G3) deck drapes the three PathLayers (the tunnels' too: they then lie
//! on the surface) and raises the signals and texts by the ground under them: the lines are drawn
//! into the terrain's covers, and when they are made again only the covers round the runs whose
//! colour or width changed (or round both sets, for another set) are drawn again.

use std::sync::Arc;

use glam::DVec2;
use rustradar_scene::blocks::{
    BLOCK_CASING, BLOCK_LABEL_BACKGROUND, BLOCK_LABEL_COLOR, BLOCK_LABEL_PADDING, BLOCK_LABEL_SIZE, BLOCK_TUNNEL_DASHES, BlockRuns, BlocksScene,
    CASING_EXTRA_PX, SIGNAL_LINE_PX, SIGNAL_NAME_BACKGROUND, SIGNAL_NAME_COLOR, SIGNAL_NAME_OFFSET, SIGNAL_NAME_PADDING, SIGNAL_NAME_SIZE,
    SIGNAL_RADIUS_PX, TAKEN_WIDTH_PX,
};
use rustradar_scene::camera::lng_lat_to_world;
use rustradar_scene::geom::path::{LocalMercator, Paint, PathBuilder};
use rustradar_scene::text::{Face, Family, FontSettings, GlyphAtlas, LaidText, TextLayout};

pub use rustradar_scene::blocks::{BLOCK_FREE, BLOCK_TAKEN};

use super::terrain::cover::{DrawnArea, line_reach};
use super::{DrawCx, Init, Layer, Pass, PrepareCx, note_drawn_area};
use crate::frame::{FrameSlot, power_of_two_scale};
use crate::pipelines::circle::{self, Circle, CircleStyle};
use crate::pipelines::path::{Dash, Mesh, Path, PathStyle, Segment, Style};
use crate::pipelines::text::{AtlasTexture, Background, BackgroundStyle, Glyph, Label, TextBatch, TextPipeline, TextStyle};
use crate::state::DrawState;

/// What the scene builder hands this layer (`SceneInput`'s field of the same name). The default
/// draws nothing.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// This scene refresh's blocks: empty while they are off, none are in or the camera is out
    /// beyond zoom 13.
    pub scene: BlocksScene,
}

/// `ground`: the casing and the blocks.
pub const GROUND: DrawState = DrawState::SURFACE_NO_CULL;
/// `top`: the tunnels, the signals and the texts.
pub const TOP: DrawState = DrawState::LABEL;
/// The signal names' and block labels' face (today `ui-monospace, monospace`).
pub const TEXT_FACE: Face = Face::new(Family::OverpassMono, 400);
/// `train-blocks-casing`'s and `train-blocks`' props: widths in pixels, round caps.
pub const LINE_STYLE: PathStyle = PathStyle { cap_rounded: true, ..PathStyle::PIXELS };
/// `train-blocks-tunnels`' props: widths in pixels, dashes along the whole path, justified.
pub const TUNNEL_STYLE: PathStyle = PathStyle { dash: Dash::Path, dash_justified: true, ..PathStyle::PIXELS };
/// `train-signals`' props.
pub const SIGNAL_STYLE: CircleStyle = CircleStyle { billboard: true, filled: true, stroked: true, ..CircleStyle::PIXELS };

/// The smallest frame's scale (a set of one place still needs a scale `f32` holds exactly).
const MIN_SCALE: f64 = 1.0 / 1_048_576.0;

/// One local Mercator frame round every run of `runs`: anchored at the north-west corner of their
/// extent, a power of two across.
pub fn frame_of(runs: &BlockRuns) -> LocalMercator {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in runs.above.iter().chain(&runs.below).flat_map(|r| &r.path) {
        let w = DVec2::from(lng_lat_to_world([p[0], p[1]]));
        if w.is_finite() {
            min = min.min(w);
            max = max.max(w);
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return LocalMercator::new(DVec2::ZERO, 1.0);
    }
    LocalMercator::new(DVec2::new(min.x, max.y), power_of_two_scale((max - min).max_element()).max(MIN_SCALE))
}

/// `path` (`[lon, lat, metres]`) in `local`'s `[u, v, metres]`.
fn in_frame(local: &LocalMercator, path: &[[f64; 3]]) -> Vec<[f64; 3]> {
    path.iter()
        .map(|&[lon, lat, h]| {
            let [u, v] = local.from_world(DVec2::from(lng_lat_to_world([lon, lat])));
            [u, v, h]
        })
        .collect()
}

/// The segments of the three line layers of `scene`, in one frame: casing, blocks, tunnels.
pub fn lines(scene: &BlocksScene) -> (LocalMercator, [Vec<Segment>; 3]) {
    let local = frame_of(&scene.runs);
    let mut casing = PathBuilder::new(local);
    let mut blocks = PathBuilder::new(local);
    for run in &scene.runs.above {
        let points = in_frame(&local, &run.path);
        casing.push(&points, &Paint::solid(BLOCK_CASING, scene.casing_width(run)));
        blocks.push(&points, &Paint::solid(scene.run_color(run), scene.run_width(run)));
    }
    let mut tunnels = PathBuilder::measuring(local);
    for run in &scene.runs.below {
        let points = in_frame(&local, &run.path);
        tunnels.push(&points, &Paint::solid(scene.tunnel_color(run), scene.run_width(run)).dashed(BLOCK_TUNNEL_DASHES));
    }
    (local, [casing.finish(), blocks.finish(), tunnels.finish()])
}

/// The trains' names by the id of the block they are in ([`BlocksScene::taken`]).
type TakenBy = Arc<std::collections::HashMap<String, String>>;

/// The texts as laid out in the atlas: each named signal's index and name, each label's.
#[derive(Default)]
struct Laid {
    /// What they were laid out for: the set's runs, the labels' texts, the atlas's generation.
    runs: Option<Arc<BlockRuns>>,
    labels: Vec<String>,
    generation: u64,
    names: Vec<(usize, LaidText)>,
    label_texts: Vec<LaidText>,
}

/// Row 12: `train-blocks-casing`, `train-blocks`, `train-blocks-tunnels`, `train-signals`,
/// `train-signal-names`, `train-block-labels`.
pub struct Blocks {
    ground: Path,
    top: Path,
    /// Casing, blocks, tunnels.
    styles: [Style; 3],
    meshes: [Mesh; 3],
    /// The set and taken blocks the meshes were made for.
    built: Option<(Arc<BlockRuns>, TakenBy)>,
    /// The meshes' frame.
    local: LocalMercator,
    signals: Circle,
    signal_style: circle::Style,
    signal_instances: circle::Instances,
    atlas: GlyphAtlas,
    texture: AtlasTexture,
    text: TextPipeline,
    names: TextBatch,
    labels: TextBatch,
    laid: Laid,
    /// This frame's frame of the lines, when there are any.
    slot: Option<FrameSlot>,
    /// What the lines were last drawn as, for the 3D terrain's covers (item G3).
    draped: Draped,
}

/// How far the lines reach past their points, texels: the widest, a taken block's casing.
pub fn lines_reach() -> f64 {
    line_reach(TAKEN_WIDTH_PX + CASING_EXTRA_PX)
}

/// What the three line layers drew, as the 3D terrain's covers see it (item G3): the set's runs,
/// each run's colour and width (the ground's, then the tunnels'), and the box round them all.
#[derive(Default)]
struct Draped {
    runs: Option<Arc<BlockRuns>>,
    styles: Vec<([u8; 4], f64)>,
    all: Option<DrawnArea>,
}

/// Each run's colour and width as `scene` draws it: the runs on the ground and bridges (whose
/// casing follows the width), then those in tunnels.
fn run_styles(scene: &BlocksScene) -> Vec<([u8; 4], f64)> {
    let above = scene.runs.above.iter().map(|r| (scene.run_color(r), scene.run_width(r)));
    let below = scene.runs.below.iter().map(|r| (scene.tunnel_color(r), scene.run_width(r)));
    above.chain(below).collect()
}

/// The box round a run's track.
fn run_area(path: &[[f64; 3]]) -> Option<DrawnArea> {
    DrawnArea::round(path.iter().map(|p| [p[0], p[1]]), lines_reach())
}

/// The box round every run of `runs`.
fn runs_area(runs: &BlockRuns) -> Option<DrawnArea> {
    runs.above.iter().chain(&runs.below).fold(None, |a, r| DrawnArea::either(a, run_area(&r.path)))
}

/// Where `scene`'s lines draw differently from `was` (item G3): everywhere either set reaches for
/// another set, else round each run whose colour or width changed.
fn lines_changed(was: &Draped, scene: &BlocksScene) -> Option<DrawnArea> {
    let same_runs = was.runs.as_ref().is_some_and(|r| Arc::ptr_eq(r, &scene.runs) || **r == *scene.runs);
    if !same_runs {
        return DrawnArea::either(was.all, runs_area(&scene.runs));
    }
    let styles = run_styles(scene);
    let runs = scene.runs.above.iter().chain(&scene.runs.below);
    runs.zip(styles.iter().zip(&was.styles)).filter(|(_, (now, before))| now != before).fold(None, |a, (r, _)| DrawnArea::either(a, run_area(&r.path)))
}

impl Blocks {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let ground = Path::new(init, "train blocks", GROUND);
        let top = Path::new(init, "train block tunnels", TOP);
        let styles = [ground.style(device, &LINE_STYLE), ground.style(device, &LINE_STYLE), top.style(device, &TUNNEL_STYLE)];
        let meshes = ["train-blocks-casing", "train-blocks", "train-blocks-tunnels"].map(|label| Mesh::new(device, label, &[]));
        let signals = Circle::new(init, "train signals", TOP);
        let signal_style = signals.style(device, &SIGNAL_STYLE);
        let mut atlas = GlyphAtlas::new(FontSettings::DEFAULT);
        let texture = AtlasTexture::new(init.gpu, "block glyphs", &mut atlas);
        let text = TextPipeline::new(init, "block texts", TOP);
        let names = text.batch(device, "train-signal-names", &texture);
        let labels = text.batch(device, "train-block-labels", &texture);
        Blocks {
            ground,
            top,
            styles,
            meshes,
            built: None,
            local: LocalMercator::new(DVec2::ZERO, 1.0),
            signals,
            signal_style,
            signal_instances: circle::Instances::new(device, "train-signals"),
            atlas,
            texture,
            text,
            names,
            labels,
            laid: Laid::default(),
            slot: None,
            draped: Draped::default(),
        }
    }

    /// What each row draws this frame (for tests): segments of the casing, the blocks and the
    /// tunnels; signals; glyphs and boxes of the names; glyphs and boxes of the labels.
    pub fn counts(&self) -> [u32; 8] {
        [
            self.meshes[0].len(),
            self.meshes[1].len(),
            self.meshes[2].len(),
            self.signal_instances.count(),
            self.names.len(),
            self.names.background_len(),
            self.labels.len(),
            self.labels.background_len(),
        ]
    }

    /// Lays out the signals' names and the labels unless they already are, again from scratch
    /// whenever the atlas packs itself anew (which moves the glyphs).
    fn lay_out(&mut self, scene: &BlocksScene) {
        let texts: Vec<String> = scene.labels.iter().map(|l| l.text.clone()).collect();
        for _ in 0..4 {
            let generation = self.atlas.generation();
            let same_runs = self.laid.runs.as_ref().is_some_and(|r| Arc::ptr_eq(r, &scene.runs));
            if same_runs && self.laid.labels == texts && self.laid.generation == generation {
                return;
            }
            let start = TextLayout::START;
            let middle = TextLayout::DEFAULT;
            let names = scene
                .runs
                .signals
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.name.as_deref().filter(|n| !n.is_empty()).map(|n| (i, self.atlas.layout(TEXT_FACE, n, &start))))
                .collect();
            let label_texts = texts.iter().map(|t| self.atlas.layout(TEXT_FACE, t, &middle)).collect();
            self.laid = Laid { runs: Some(scene.runs.clone()), labels: texts.clone(), generation: self.atlas.generation(), names, label_texts };
            if self.atlas.generation() == generation {
                return;
            }
        }
    }
}

impl Layer for Blocks {
    fn name(&self) -> &'static str {
        "blocks"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let scene = &cx.scene.blocks.scene;
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        if scene.is_empty() {
            if self.built.take().is_some() {
                for mesh in &mut self.meshes {
                    mesh.write(device, queue, "train blocks", &[]);
                }
            }
            // the lines gone from the 3D terrain's covers (item G3)
            if let Some(area) = std::mem::take(&mut self.draped).all {
                note_drawn_area(area);
            }
            self.slot = None;
            self.signal_instances.write(device, queue, &[]);
            self.names.set_glyphs(device, queue, &[]);
            self.names.set_backgrounds(device, queue, &[]);
            self.labels.set_glyphs(device, queue, &[]);
            self.labels.set_backgrounds(device, queue, &[]);
            return;
        }
        // the lines, made again for a new set or new taken blocks
        let fresh = self.built.as_ref().is_some_and(|(runs, taken)| Arc::ptr_eq(runs, &scene.runs) && Arc::ptr_eq(taken, &scene.taken));
        if !fresh {
            let (local, made) = lines(scene);
            for (mesh, segments) in self.meshes.iter_mut().zip(made) {
                mesh.write(device, queue, "train blocks", &segments);
            }
            self.local = local;
            self.built = Some((scene.runs.clone(), scene.taken.clone()));
            // on the 3D terrain the lines are draped: the covers where they drew differently are
            // drawn again (item G3; deck redraws them all whenever the layer's data or colours change)
            if let Some(area) = lines_changed(&self.draped, scene) {
                note_drawn_area(area);
            }
            self.draped = Draped { runs: Some(scene.runs.clone()), styles: run_styles(scene), all: runs_area(&scene.runs) };
        }
        self.slot = Some(cx.frames.push(&cx.view.local(self.local.anchor, self.local.scale, 0.0)));

        // each block's signal at its start, lit as the blocks say (the signals and the texts on the
        // 3D terrain raised by the ground under them, Z3; the lines draped, G3)
        let discs: Vec<circle::Instance> = scene
            .runs
            .signals
            .iter()
            .map(|s| circle::Instance::at(cx.view, s.at, cx.scene.stands_on.at(s.at)).radius(SIGNAL_RADIUS_PX).fill(scene.aspect(s).color()).line(BLOCK_CASING, SIGNAL_LINE_PX))
            .collect();
        self.signal_instances.write(device, queue, &discs);

        // the texts, placed for this frame's camera
        self.lay_out(scene);
        self.texture.sync(cx.gpu, &mut self.atlas);
        let (mut glyphs, mut boxes): (Vec<Glyph>, Vec<Background>) = (Vec::new(), Vec::new());
        if scene.names {
            for (i, laid) in &self.laid.names {
                let [lon, lat] = scene.runs.signals[*i].at;
                let label = Label::at(cx.view, [lon, lat, cx.scene.stands_on.at([lon, lat])], SIGNAL_NAME_SIZE, SIGNAL_NAME_COLOR).with_pixel_offset(SIGNAL_NAME_OFFSET);
                boxes.push(label.background(laid, SIGNAL_NAME_BACKGROUND));
                glyphs.extend(label.glyphs(laid));
            }
        }
        let name_boxes = BackgroundStyle::DEFAULT.padded(&SIGNAL_NAME_PADDING);
        self.names.set_style(queue, &TextStyle::DEFAULT, &name_boxes, &FontSettings::DEFAULT, cx.view);
        self.names.set_backgrounds(device, queue, &boxes);
        self.names.set_glyphs(device, queue, &glyphs);

        let (mut glyphs, mut boxes): (Vec<Glyph>, Vec<Background>) = (Vec::new(), Vec::new());
        for (block, laid) in scene.labels.iter().zip(&self.laid.label_texts) {
            let [lon, lat] = block.at;
            let label = Label::at(cx.view, [lon, lat, cx.scene.stands_on.at([lon, lat])], BLOCK_LABEL_SIZE, BLOCK_LABEL_COLOR);
            boxes.push(label.background(laid, BLOCK_LABEL_BACKGROUND));
            glyphs.extend(label.glyphs(laid));
        }
        let label_boxes = BackgroundStyle::DEFAULT.padded(&BLOCK_LABEL_PADDING);
        self.labels.set_style(queue, &TextStyle::DEFAULT, &label_boxes, &FontSettings::DEFAULT, cx.view);
        self.labels.set_backgrounds(device, queue, &boxes);
        self.labels.set_glyphs(device, queue, &glyphs);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        // on the 3D terrain the three PathLayers go into the covers (deck drapes them, the tunnels'
        // too), the signals and the texts stand in the map pass (item G3)
        if let (true, Some(slot)) = (cx.draws_draped(), self.slot) {
            self.ground.begin(pass);
            self.ground.draw_mesh(pass, cx, slot, &self.styles[0], &self.meshes[0]);
            self.ground.draw_mesh(pass, cx, slot, &self.styles[1], &self.meshes[1]);
            self.top.draw(pass, cx, slot, &self.styles[2], &self.meshes[2]);
        }
        if cx.pass != Pass::Map {
            return;
        }
        self.signals.draw(pass, &self.signal_style, &self.signal_instances);
        self.text.draw(pass, &self.names);
        self.text.draw(pass, &self.labels);
    }

    fn drape(&self) -> super::Drape {
        // the lines (deck's PathLayers) draped over the 3D terrain, the discs and texts raised
        super::Drape::Part
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::f64::consts::PI;
    use std::rc::Rc;

    use rustradar_core::geo::Basemap;
    use rustradar_core::model::{Structure, TrainBlock};
    use rustradar_scene::blocks::{Aspect, BlockBuilder, DrawnTrain, SetRef};
    use rustradar_scene::camera::{ViewState, viewport_for};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::renderer::{MapRenderer, SceneInput};

    const LAND: [u8; 3] = [50, 50, 50];
    /// Arlanda's northern portal: 59.672° N, 17.91° E.
    const LAT: f64 = 59.6735;
    const LON: f64 = 17.91;
    const SIZE: [f64; 2] = [1000.0, 400.0];

    /// The place `east` metres east and `north` metres north of [`LAT`], [`LON`].
    fn spot(east: f64, north: f64) -> [f64; 2] {
        [LON + east / (111_320.0 * (LAT * PI / 180.0).cos()), LAT + north / 111_320.0]
    }

    fn view(zoom: f64) -> ViewState {
        ViewState { longitude: LON, latitude: LAT, zoom, pitch: 0.0, bearing: 0.0, position: None }
    }

    /// A line east from `from` to `to` metres at `north`, a point every 20 m.
    fn east_line(from: f64, to: f64, north: f64) -> Vec<[f64; 2]> {
        let n = ((to - from) / 20.0).round() as usize;
        (0..=n).map(|k| spot(from + (to - from) * k as f64 / n as f64, north)).collect()
    }

    /// Three blocks one after another on a track running east 60 m north of the centre, the last
    /// one into a tunnel from 150 m east; one train in the middle block.
    fn blocks() -> (Vec<TrainBlock>, Vec<Structure>) {
        let list = vec![
            TrainBlock { id: "w".into(), signal: Some("121".into()), lines: vec![east_line(-250.0, -100.0, 60.0)] },
            TrainBlock { id: "m".into(), signal: Some("123".into()), lines: vec![east_line(-100.0, 50.0, 60.0)] },
            TrainBlock { id: "e".into(), signal: None, lines: vec![east_line(50.0, 250.0, 60.0)] },
        ];
        let tunnel = Structure { tunnel: true, line: vec![spot(150.0, 60.0), spot(400.0, 60.0)], joined: [false, true] };
        (list, vec![tunnel])
    }

    fn scene_at(zoom: f64) -> SceneInput {
        let (list, structures) = blocks();
        let v = view(zoom);
        let trains = [DrawnTrain { id: "t", name: "SJ 523", at: spot(-25.0, 60.5) }];
        let built = BlockBuilder::new().build(Some(SetRef { generation: 1, list: &list, structures: &structures }), &trains, &v, SIZE, 1.782e12);
        let mut scene = SceneInput::new(v, SIZE, 1.0, Basemap::Dark);
        scene.earth = crate::layers::earth::Input::SHOWN;
        scene.blocks.scene = built;
        scene
    }

    /// What the lines' change puts out of date on the 3D terrain (item G3): nothing for the same
    /// set and taken blocks (made again or not), round the runs of the block whose train left,
    /// everything either set reaches for another set.
    #[test]
    fn the_covers_change_round_the_runs_that_change() {
        let (list, structures) = blocks();
        let v = view(16.0);
        let build = |trains: &[DrawnTrain<'_>], generation: u64, list: &[TrainBlock]| {
            BlockBuilder::new().build(Some(SetRef { generation, list, structures: &structures }), trains, &v, SIZE, 1.782e12)
        };
        let in_m = [DrawnTrain { id: "t", name: "SJ 523", at: spot(-25.0, 60.5) }];
        let taken = build(&in_m, 1, &list);
        let drawn = Draped { runs: Some(taken.runs.clone()), styles: run_styles(&taken), all: runs_area(&taken.runs) };
        assert_eq!(lines_changed(&drawn, &taken), None);
        assert_eq!(lines_changed(&drawn, &build(&in_m, 1, &list)), None, "the same lines made again");
        // the train gone: block m's run, from 100 m west to 50 m east
        let free = build(&[], 1, &list);
        let area = lines_changed(&drawn, &free).expect("a change");
        let m = DrawnArea::round(east_line(-100.0, 50.0, 60.0), lines_reach()).expect("m's run");
        assert_eq!(area, m);
        // another set: both sets' runs
        let mut moved = list.clone();
        moved[2].lines = vec![east_line(50.0, 600.0, 60.0)];
        let other = build(&in_m, 2, &moved);
        let area = lines_changed(&drawn, &other).expect("a change");
        assert_eq!(area, DrawnArea::either(runs_area(&taken.runs), runs_area(&other.runs)).expect("both"));
        assert!(area.max[0] > drawn.all.expect("drawn").max[0], "as far east as the new set reaches");
        assert_eq!(lines_reach(), 17.0);
    }

    /// A second blocks layer that prepares as the map's own does and draws nothing: what each row
    /// of the map's draws.
    struct Probe {
        blocks: Blocks,
        counts: Rc<Cell<[u32; 8]>>,
    }

    impl Layer for Probe {
        fn name(&self) -> &'static str {
            "t5 probe"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.blocks.prepare(cx);
            self.counts.set(self.blocks.counts());
        }

        fn draw(&self, _pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {}
    }

    /// `scene` drawn by the map, and what the blocks layer drew.
    fn render(gpu: &crate::gpu::Gpu, scene: &SceneInput) -> (Image, [u32; 8]) {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let counts = Rc::new(Cell::new([u32::MAX; 8]));
        let probe = Probe { blocks: Blocks::new(&renderer.init(gpu)), counts: counts.clone() };
        renderer.add_layer(Box::new(probe));
        let image = render_with(gpu, &mut renderer, scene, 1.0).expect("rendered");
        (image, counts.get())
    }

    fn screen(v: ViewState, [lon, lat]: [f64; 2]) -> [f64; 2] {
        let p = viewport_for(&v, SIZE).project([lon, lat, 0.0]);
        [p[0], p[1]]
    }

    fn save(image: &Image, name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots");
        std::fs::create_dir_all(dir).expect("shot folder");
        image.save_png(&std::path::Path::new(dir).join(name)).expect("png");
    }

    /// `top` at `alpha` over `under`, rounded as the blend does.
    fn over(top: [u8; 3], alpha: u8, under: [u8; 3]) -> [u8; 3] {
        let a = f64::from(alpha) / 255.0;
        [0, 1, 2].map(|c| (f64::from(top[c]) * a + f64::from(under[c]) * (1.0 - a)).round() as u8)
    }

    fn near(p: [u8; 4], want: [u8; 3], tolerance: u8) -> bool {
        (0..3).all(|c| p[c].abs_diff(want[c]) <= tolerance)
    }

    #[test]
    fn one_frame_holds_every_run() {
        let (list, structures) = blocks();
        let runs = rustradar_scene::blocks::runs_of(&list, &structures);
        let local = frame_of(&runs);
        for run in runs.above.iter().chain(&runs.below) {
            for p in in_frame(&local, &run.path) {
                assert!((0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]), "{p:?}");
            }
        }
        assert_eq!(local.scale, power_of_two_scale(local.scale));
        assert_eq!(frame_of(&BlockRuns::default()).scale, 1.0, "nothing: a frame all the same");
    }

    #[test]
    fn the_lines_are_the_casing_the_blocks_and_the_dashed_tunnels() {
        let scene = scene_at(16.0).blocks.scene;
        let (_, [casing, blocks, tunnels]) = lines(&scene);
        assert_eq!(casing.len(), blocks.len());
        assert!(!tunnels.is_empty());
        // the middle block is taken: 5 px in red over 8 px of casing; the others free, 3 over 6
        let widths: Vec<(f32, [u8; 4])> = blocks.iter().map(|s| (s.width, s.color)).collect();
        assert!(widths.contains(&(5.0, BLOCK_TAKEN)) && widths.contains(&(3.0, BLOCK_FREE)));
        assert!(casing.iter().all(|s| s.color == BLOCK_CASING && (s.width == 6.0 || s.width == 8.0)));
        assert!(tunnels.iter().all(|s| s.color == [60, 200, 110, 115] && s.width == 3.0 && s.dash_array == [2.5, 2.0]));
        assert!(tunnels.last().unwrap().dash_flat[1] > 0.0, "dashes measured along the whole path");
        let aspects: Vec<Aspect> = scene.runs.signals.iter().map(|s| scene.aspect(s)).collect();
        assert_eq!(aspects, [Aspect::Caution, Aspect::Stop, Aspect::Clear]);
    }

    /// At zoom 16 straight down: across the track, the casing under each block (1.5 px each side of
    /// a free block, the same of a taken one), the taken block in its red 5 px wide, the free ones
    /// in their green 3 px; the tunnel faint and dashed; the signals' discs by aspect; the names
    /// right of their signals in dark boxes and the label of the taken block in its red box.
    #[test]
    fn the_blocks_draw_over_their_casing_in_their_colours() {
        let Some(gpu) = gpu() else { return };
        let scene = scene_at(16.0);
        let v = scene.view;
        let (image, counts) = render(&gpu, &scene);
        save(&image, "t5-blocks-z16.png");
        assert!(counts[..4].iter().all(|&c| c > 0), "{counts:?}");
        assert_eq!(counts[3], 3, "three signals");
        assert_eq!(counts[5], 2, "two named signals, each in its box");
        assert_eq!(counts[7], 1, "one taken block's label in its box");

        // a column across the free west block and one across the taken middle one, clear of the
        // signals and the label
        let casing_over_land = over([16, 18, 24], 200, LAND);
        for (east, color, width) in [(-200.0, BLOCK_FREE, 3.0), (-70.0, BLOCK_TAKEN, 5.0)] {
            let [x, y] = screen(v, spot(east, 60.0));
            let column: Vec<[u8; 4]> = ((y - 10.0) as u32..(y + 10.0) as u32).map(|py| image.pixel(x as u32, py)).collect();
            let line_rgb = [color[0], color[1], color[2]];
            let line = if color[3] == 255 { line_rgb } else { over(line_rgb, color[3], casing_over_land) };
            let full = column.iter().filter(|p| near(**p, line, 3)).count();
            let casing = column.iter().filter(|p| near(**p, casing_over_land, 3)).count();
            let touched = column.iter().filter(|p| p[..3] != LAND).count();
            assert!((width as usize - 1..=width as usize + 1).contains(&full), "{east} m: {full} px of {color:?}: {column:?}");
            assert!((2..=4).contains(&casing), "{east} m: {casing} px of casing: {column:?}");
            assert!(touched as f64 <= width + 3.0 + 2.0, "{east} m: {touched} px touched");
        }

        // the tunnel east of 150 m: half the green over the land, in dashes
        let [x0, y] = screen(v, spot(160.0, 60.0));
        let [x1, _] = screen(v, spot(240.0, 60.0));
        let faint = over([60, 200, 110], 115, LAND);
        let row: Vec<bool> = (x0 as u32..x1 as u32).map(|px| near(image.pixel(px, y as u32), faint, 4)).collect();
        let on = row.iter().filter(|&&d| d).count();
        let runs_on = row.windows(2).filter(|w| !w[0] && w[1]).count();
        assert!(on > row.len() / 4 && on < row.len() * 3 / 4, "dashed: {on} of {} px", row.len());
        assert!(runs_on >= 5, "{runs_on} dashes");

        // the signals: the west block's leads into the taken one (caution), the taken one's stop,
        // the last clear
        for (east, aspect) in [(-250.0, Aspect::Caution), (-100.0, Aspect::Stop), (50.0, Aspect::Clear)] {
            let [x, y] = screen(v, spot(east, 60.0));
            let c = aspect.color();
            assert!(near(image.pixel(x as u32, y as u32), [c[0], c[1], c[2]], 3), "{aspect:?} at {east} m: {:?}", image.pixel(x as u32, y as u32));
            // the outline 7 px out
            assert!(near(image.pixel((x + 6.5) as u32, y as u32), casing_over_land, 40), "outline: {:?}", image.pixel((x + 6.5) as u32, y as u32));
        }

        // the names' boxes start 10 − 3 px right of their signals
        let name_box = over([16, 18, 24], 200, LAND);
        let [x, y] = screen(v, spot(-250.0, 60.0));
        let boxed = (0..30).filter(|k| near(image.pixel((x + 8.0) as u32 + k, y as u32 - 4), name_box, 3)).count();
        assert!(boxed > 3, "the name's box right of the signal: {boxed}");

        // the taken block's label: its red box and white glyphs halfway along it
        let [x, y] = screen(v, scene.blocks.scene.labels[0].at);
        let label_box = over([150, 30, 24], 230, LAND);
        let around = |pick: &dyn Fn([u8; 4]) -> bool| -> usize {
            let mut n = 0;
            for dy in -6..=6 {
                for dx in -30..=30 {
                    if pick(image.pixel((x + f64::from(dx)) as u32, (y + f64::from(dy)) as u32)) {
                        n += 1;
                    }
                }
            }
            n
        };
        let red = around(&|p| near(p, label_box, 6));
        // the glyphs: grey to white (11 px strokes from the 64 px alpha atlas cover 0.7 of a pixel
        // at most), far above the box's green and the red line's
        let white = around(&|p| p[1] > 120 && p[1].abs_diff(p[2]) < 25);
        assert!(red > 150, "the label's box: {red} px");
        assert!(white > 20, "the label's text: {white} px");
    }

    /// Further out than zoom 15.5 the names go; beyond zoom 13 everything does.
    #[test]
    fn names_from_zoom_15_5_and_nothing_further_out_than_13() {
        let Some(gpu) = gpu() else { return };
        let (_, counts) = render(&gpu, &scene_at(15.0));
        assert_eq!((counts[3], counts[4], counts[5], counts[7]), (3, 0, 0, 1), "{counts:?}");
        let (image, counts) = render(&gpu, &scene_at(12.9));
        assert_eq!(counts, [0; 8]);
        let [x, y] = [500, 200];
        assert_eq!(image.pixel(x, y)[..3], LAND);
    }
}
