//! Aircraft labels (item A7; doc 03 §7, doc 01 §9.2; G:754-969, 3026-3035): row 29 of doc 03 §3,
//! `aircraft-label-names` then `aircraft-label-heights`, over every aircraft so that none is
//! drawn across a label.
//!
//! The scene builder hands over each refresh's [`Input`] (`rustradar_scene::aircraft_labels`): the
//! label set (texts, heights, priorities, laid out once per set), where each labelled aircraft is
//! drawn, each label's gap from it, the selected flight, the label size setting and the claimed
//! box's padding. This part, each frame:
//! - lays the set's texts out in its glyph atlas once per set (Overpass 600, SDF at 64 px with
//!   buffer 6, anchor `start`, baseline `center`), again only when the atlas packs itself anew;
//! - projects each label's aircraft with this frame's camera (deck's collision effect redraws its
//!   map whenever the viewport changes), and runs deck's collision filter for the group
//!   `"aircraft-labels"` on the CPU (`aircraft_labels::collision_fades`: the claimed boxes, then the
//!   callsigns, then the heights into the half-resolution map, each label's fade from the 5 × 5
//!   patch round its aircraft);
//! - makes the two TextLayers' glyphs: the callsign 11 px at `[gap, −line]` and the height 10 px
//!   at `[gap, +line]`, `sizeScale` the label size setting, billboards, the 3 px halo
//!   `[6,12,19,220]`, the fill colours' alpha times `smoothstep(0.3, 0.65, fade)`, nothing for a
//!   label under a fade of 0.0001; and draws them with the SDF text pipeline in the `LABEL` state
//!   (no cull, depth `always`, no depth write), the callsigns before the heights. Not pickable.
//!
//! The work is redone only when the camera, the canvas or the input changed since the last frame.
//!
//! Deviations, all invisible: the callsign layer's background boxes are transparent
//! (`getBackgroundColor [0,0,0,0]`) and drawn without depth writes, so they change no pixel and
//! are not drawn (they still claim their box in the collision pass); the fade multiplies the
//! colour's 8-bit alpha; on the flat map a label is drawn on the world copy nearest the view.

use std::sync::Arc;

use glam::DVec4;
use rustradar_scene::aircraft_labels::{self as rules, AIRCRAFT_LABEL_HEIGHT_SIZE, AIRCRAFT_LABEL_SIZE, LAYOUT, LabelSet, Placing};
use rustradar_scene::declutter::Rect;
use rustradar_scene::labels_map::{CollisionMap, bounds};
use rustradar_scene::text::{Face, FontSettings, GlyphAtlas, LaidText};

use crate::frame::ViewFrame;
use crate::layers::{DrawCx, Init, Pass, PrepareCx};
use crate::pipelines::icon::focal_w;
use crate::pipelines::text::{AtlasTexture, BackgroundStyle, Glyph, Label, TextBatch, TextPipeline, TextStyle};
use crate::state::DrawState;

/// What the scene builder hands this part (`layers::aircraft::Input::labels`).
pub type Input = rules::AircraftLabels;

/// Both layers' draw state (`LABEL_PARAMETERS`).
pub const STATE: DrawState = DrawState::LABEL;

/// One label drawn this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    /// The flight's id.
    pub id: u32,
    /// The callsign and the height line (`""` without it).
    pub text: String,
    pub height: String,
    /// Its aircraft on the screen, CSS px from the canvas's top left.
    pub anchor: [f64; 2],
    /// The focal depth's clip w over the anchor's: a billboard's pixel offsets and glyphs are
    /// drawn this many times their size (above 1 for an aircraft high up, nearer the camera).
    pub perspective: f64,
    /// deck's collision fade, `[0.0001, 1]`.
    pub fade: f64,
    /// The rectangles round the callsign's and the height's glyph quads as drawn, CSS px.
    pub name: Option<Rect>,
    pub height_rect: Option<Rect>,
    /// The claimed box (the callsign layer's background), CSS px.
    pub claimed: Rect,
    /// The colours drawn (after the fade).
    pub name_color: [u8; 4],
    pub height_color: [u8; 4],
}

/// Every label of the set in this frame's collision pass, drawn or not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub id: u32,
    /// Its aircraft on the screen, CSS px; `None` when it is not in front of the camera.
    pub anchor: Option<[f64; 2]>,
    /// Its claimed box, CSS px (a hidden label's still covers others).
    pub claimed: Option<Rect>,
    /// `getCollisionPriority`.
    pub priority: f64,
    /// deck's collision fade: 0 when hidden or off the screen.
    pub fade: f64,
}

/// The texts of one set laid out in the atlas.
#[derive(Default)]
struct Laid {
    /// The set they were laid out for, and the atlas's generation.
    set: Option<Arc<LabelSet>>,
    generation: u64,
    names: Vec<LaidText>,
    heights: Vec<LaidText>,
}

impl Laid {
    fn is_for(&self, set: &Arc<LabelSet>, generation: u64) -> bool {
        self.generation == generation && self.set.as_ref().is_some_and(|s| Arc::ptr_eq(s, set))
    }
}

/// What the last frame's work was done for; the same again needs none.
#[derive(Debug, Clone)]
struct Key {
    view: rustradar_scene::camera::ViewState,
    size: [f64; 2],
    map_scale: f64,
    common_centre: glam::DVec3,
    input: Input,
    generation: u64,
}

impl Key {
    fn same(&self, other: &Key) -> bool {
        let (a, b) = (&self.input, &other.input);
        self.view == other.view
            && self.size == other.size
            && self.map_scale == other.map_scale
            && self.common_centre == other.common_centre
            && self.generation == other.generation
            && Arc::ptr_eq(&a.set, &b.set)
            && a.positions == b.positions
            && a.gaps == b.gaps
            && a.selected == b.selected
            && a.scale == b.scale
            && a.layout == b.layout
    }
}

/// Row 29: the aircraft labels.
pub struct Labels {
    atlas: GlyphAtlas,
    texture: AtlasTexture,
    pipeline: TextPipeline,
    /// `aircraft-label-names`, `aircraft-label-heights`.
    names: TextBatch,
    heights: TextBatch,
    collision: CollisionMap,
    laid: Laid,
    placed: Option<Key>,
    placements: Vec<Placement>,
    shown: Vec<Shown>,
    /// Whether this frame draws, and the heights with it.
    draws: bool,
    with_height: bool,
}

/// A world x difference moved into `[−256, 256)`.
fn wrap_world_x(dx: f64) -> f64 {
    const WORLD: f64 = 512.0;
    dx - WORLD * ((dx + WORLD / 2.0) / WORLD).floor()
}

/// Where an aircraft at `[lon, lat, metres]` is on the screen in `view`: CSS px from the top left,
/// and the focal depth's clip w over its own; `None` when it is not in front of the camera.
pub fn project(view: &ViewFrame, focal: f64, [lon, lat, h]: [f64; 3]) -> Option<([f64; 2], f64)> {
    let vp = &*view.viewport;
    let mut p = view.viewport.project_position([lon, lat, h]);
    if !view.is_globe() {
        p.x = vp.center.x + wrap_world_x(p.x - vp.center.x);
    }
    let clip = vp.view_projection_matrix * DVec4::new(p.x, p.y, p.z, 1.0);
    // (NaN is not in front either)
    if clip.w.is_nan() || clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(([(ndc.x + 1.0) / 2.0 * vp.width, (1.0 - ndc.y) / 2.0 * vp.height], focal / clip.w))
}

impl Labels {
    pub fn new(init: &Init<'_>) -> Self {
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let texture = AtlasTexture::new(init.gpu, "aircraft label glyphs", &mut atlas);
        let pipeline = TextPipeline::new(init, "aircraft labels", STATE);
        let names = pipeline.batch(&init.gpu.device, "aircraft-label-names", &texture);
        let heights = pipeline.batch(&init.gpu.device, "aircraft-label-heights", &texture);
        Labels {
            atlas,
            texture,
            pipeline,
            names,
            heights,
            collision: CollisionMap::new(),
            laid: Laid::default(),
            placed: None,
            placements: Vec::new(),
            shown: Vec::new(),
            draws: false,
            with_height: false,
        }
    }

    /// The labels drawn this frame, in the set's order.
    pub fn shown(&self) -> &[Shown] {
        &self.shown
    }

    /// Every label of the set as the last collision pass placed it, in the set's order.
    pub fn placements(&self) -> &[Placement] {
        &self.placements
    }

    /// The glyphs written: callsigns, heights.
    pub fn glyphs(&self) -> [u32; 2] {
        [self.names.len(), if self.with_height { self.heights.len() } else { 0 }]
    }

    /// Lays out `set`'s texts unless they already are, again from scratch whenever the atlas
    /// packs itself anew (which moves the glyphs).
    fn lay_out(&mut self, set: &Arc<LabelSet>) {
        for _ in 0..4 {
            let generation = self.atlas.generation();
            if self.laid.is_for(set, generation) {
                return;
            }
            let names = set.texts.iter().map(|t| self.atlas.layout(Face::LABEL, t, &LAYOUT)).collect();
            let heights = if set.with_height { set.heights.iter().map(|t| self.atlas.layout(Face::LABEL, t, &LAYOUT)).collect() } else { Vec::new() };
            self.laid = Laid { set: Some(set.clone()), generation: self.atlas.generation(), names, heights };
            if self.atlas.generation() == generation {
                return;
            }
        }
    }

    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, input: &Input) {
        self.draws = !input.is_empty();
        if !self.draws {
            if self.placed.take().is_some() {
                self.names.set_glyphs(&cx.gpu.device, &cx.gpu.queue, &[]);
                self.heights.set_glyphs(&cx.gpu.device, &cx.gpu.queue, &[]);
            }
            self.shown.clear();
            self.placements.clear();
            return;
        }
        self.with_height = input.with_height();
        self.lay_out(&input.set);
        self.texture.sync(cx.gpu, &mut self.atlas);
        let style = TextStyle::LABELS.scaled(input.scale);
        let boxes = BackgroundStyle::DEFAULT.padded(&input.layout.padding);
        self.names.set_style(&cx.gpu.queue, &style, &boxes, &FontSettings::LABELS, cx.view);
        self.heights.set_style(&cx.gpu.queue, &style, &BackgroundStyle::DEFAULT, &FontSettings::LABELS, cx.view);

        let placed = Key {
            view: cx.view.view,
            size: cx.view.size,
            map_scale: cx.view.map_scale,
            common_centre: cx.view.common_centre,
            input: input.clone(),
            generation: self.laid.generation,
        };
        if self.placed.as_ref().is_some_and(|p| p.same(&placed)) {
            return;
        }
        // the collision pass for this camera, then the labels it leaves
        let focal = focal_w(cx.view);
        let at: Vec<Option<([f64; 2], f64)>> = input.positions.iter().map(|p| p.and_then(|p| project(cx.view, focal, p))).collect();
        let placings: Vec<Placing<'_>> = (0..input.len())
            .map(|i| Placing {
                anchor: at[i].map(|(anchor, _)| anchor),
                perspective: at[i].map_or(1.0, |(_, k)| k),
                name: &self.laid.names[i],
                height: self.laid.heights.get(i),
            })
            .collect();
        let fades = rules::collision_fades(&mut self.collision, cx.view.size, cx.view.map_scale, input, &placings);
        self.placements = placings
            .iter()
            .zip(&fades)
            .enumerate()
            .map(|(i, (p, &fade))| Placement {
                id: input.set.ids[i],
                anchor: p.anchor,
                claimed: p.anchor.map(|a| {
                    let [x0, y0, x1, y1] = input.background(i, a, p.perspective, p.name);
                    Rect::new(x0, y0, x1, y1)
                }),
                priority: input.priority(i),
                fade,
            })
            .collect();

        let (mut names, mut heights): (Vec<Glyph>, Vec<Glyph>) = (Vec::new(), Vec::new());
        self.shown.clear();
        for (i, (placing, &fade)) in placings.iter().zip(&fades).enumerate() {
            let (Some(position), Some((anchor, perspective))) = (input.positions[i], at[i]) else { continue };
            if !rules::drawn(fade) {
                continue;
            }
            let name_color = rules::faded(input.name_color(i), fade);
            let label = Label::at(cx.view, position, AIRCRAFT_LABEL_SIZE, name_color).with_pixel_offset(input.name_offset(i));
            names.extend(label.glyphs(placing.name));
            let mut height_color = [0; 4];
            let mut height_rect = None;
            if let (true, Some(height)) = (self.with_height, placing.height) {
                height_color = rules::faded(input.height_color(i), fade);
                let label = Label::at(cx.view, position, AIRCRAFT_LABEL_HEIGHT_SIZE, height_color).with_pixel_offset(input.height_offset(i));
                heights.extend(label.glyphs(height));
                height_rect = bounds(input.height_quads(i, anchor, perspective, height));
            }
            let [x0, y0, x1, y1] = input.background(i, anchor, perspective, placing.name);
            self.shown.push(Shown {
                id: input.set.ids[i],
                text: input.set.texts[i].clone(),
                height: input.set.heights[i].clone(),
                anchor,
                perspective,
                fade,
                name: bounds(input.name_quads(i, anchor, perspective, placing.name)),
                height_rect,
                claimed: Rect::new(x0, y0, x1, y1),
                name_color,
                height_color,
            });
        }
        self.names.set_glyphs(&cx.gpu.device, &cx.gpu.queue, &names);
        self.heights.set_glyphs(&cx.gpu.device, &cx.gpu.queue, &heights);
        self.placed = Some(placed);
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map || !self.draws {
            return;
        }
        if !self.names.is_empty() {
            self.pipeline.draw(pass, &self.names);
        }
        if self.with_height && !self.heights.is_empty() {
            self.pipeline.draw(pass, &self.heights);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use rustradar_core::geo::Basemap;
    use rustradar_core::liveries::DEFAULT_LIVERY;
    use rustradar_core::model::{LiveFlight, f32_as_js};
    use rustradar_core::motion::aircraft::{Phase, Pose};
    use rustradar_scene::aircraft::{AircraftBuilder, AircraftScene, Inputs};
    use rustradar_scene::aircraft_labels::{AIRCRAFT_LABEL_SELECTED, Labeller};
    use rustradar_scene::camera::{ViewState, viewport_for};

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::Layer;
    use crate::layers::aircraft::assets::published;
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 12:00 UTC.
    const NOON_MS: f64 = 1_782_043_200_000.0;
    const SIZE: [f64; 2] = [600.0, 300.0];
    /// V3: Stockholm at zoom 9, straight down (the height line shows).
    const VIEW: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };
    /// id, callsign, flight number, registration, altitude (ft), on the ground, where on the canvas.
    type Row = (u32, &'static str, &'static str, &'static str, i32, bool, [f64; 2]);
    const ROW: [Row; 5] = [
        // high up: drawn well out from where its report is on the ground (the altitude raises it
        // toward the camera)
        (1, "SAS1", "", "", 37_000, false, [200.0, 110.0]),
        // selected
        (2, "DLH4AB", "", "", 3_000, false, [330.0, 90.0]),
        // 30 px left of the selected one: inside its claimed box
        (3, "BAW7", "", "", 3_000, false, [300.0, 90.0]),
        (4, "SAS22", "", "", 0, true, [90.0, 220.0]),
        // no callsign, no flight number: its registration
        (5, "", "", "SE-ROA", 2_500, false, [400.0, 220.0]),
    ];
    const SELECTED: u32 = 2;

    fn flights() -> Vec<LiveFlight> {
        let viewport = viewport_for(&VIEW, SIZE);
        ROW.iter()
            .map(|&(id, callsign, flight, reg, alt, on_ground, xy)| {
                let [lon, lat] = viewport.unproject(xy, 0.0);
                LiveFlight {
                    id,
                    lat: lat as f32,
                    lon: lon as f32,
                    alt,
                    on_ground,
                    callsign: callsign.into(),
                    flight: flight.into(),
                    reg: reg.into(),
                    typecode: "A20N".into(),
                    icon: "A320".into(),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn pose(f: &LiveFlight) -> Pose {
        Pose {
            lon: f32_as_js(f.lon),
            lat: f32_as_js(f.lat),
            alt_ft: f64::from(f.alt),
            heading: 90.0,
            track: 90.0,
            pitch: 0.0,
            bank: 0.0,
            gear: if f.on_ground { 1.0 } else { 0.0 },
            phase: if f.on_ground { Phase::Parked } else { Phase::Cruise },
        }
    }

    /// The row's aircraft as the scene builder makes them, labels on, and their labels.
    fn scene() -> (AircraftScene, Input) {
        let inputs = Inputs { view: VIEW, size: SIZE, exaggeration: 1.0, show_aircraft: true, labelling: true, selected: Some(SELECTED), follow: false, hovered: None };
        let flights = flights();
        let scene = AircraftBuilder::new().build(&inputs, published(), &flights, pose, |_| DEFAULT_LIVERY);
        assert_eq!(scene.labelled.len(), ROW.len(), "every aircraft labelled");
        let labels = Labeller::new().build(&scene, true, 1, |row| flights.get(row), 1.0);
        (scene, labels)
    }

    /// A test layer drawing only [`Labels`], from `input`, and keeping what it drew.
    struct Only {
        labels: Labels,
        input: Input,
        shown: Rc<RefCell<Vec<Shown>>>,
    }

    impl Layer for Only {
        fn name(&self) -> &'static str {
            "a7 labels only"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.labels.prepare(cx, &self.input);
            *self.shown.borrow_mut() = self.labels.shown().to_vec();
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.labels.draw(pass, cx);
        }
    }

    fn base(scale: f64) -> SceneInput {
        let mut input = SceneInput::new(VIEW, SIZE, scale, Basemap::Dark);
        input.feed_now_ms = NOON_MS;
        input.local_now_ms = NOON_MS;
        input
    }

    /// `labels` drawn alone on the empty dark map at `scale`, and what was drawn.
    fn shoot_only(gpu: &Gpu, labels: Input, scale: f64) -> (Image, Vec<Shown>) {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let shown = Rc::new(RefCell::new(Vec::new()));
        let init = renderer.init(gpu);
        renderer.add_layer(Box::new(Only { labels: Labels::new(&init), input: labels, shown: shown.clone() }));
        let image = render_with(gpu, &mut renderer, &base(scale), scale).expect("rendered");
        (image, shown.take())
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// A text fill pixel: light grey to white (the callsigns and heights), or the selection blue.
    fn is_text(p: [u8; 4]) -> bool {
        let [r, g, b, _] = p;
        (r.min(g).min(b) >= 110) || (u16::from(b) > u16::from(r) + 80 && b >= 150)
    }

    fn is_blue(p: [u8; 4]) -> bool {
        let [r, g, b, _] = p;
        u16::from(b) > u16::from(r) + 100 && g > r
    }

    /// The text pixels in `[x0, y0, x1, y1]` (CSS px at `scale`), clipped to the image.
    fn count(image: &Image, scale: f64, [x0, y0, x1, y1]: [f64; 4], pick: impl Fn([u8; 4]) -> bool) -> usize {
        let px = |v: f64, hi: u32| ((v * scale).round().max(0.0) as u32).min(hi);
        let (x0, x1, y0, y1) = (px(x0, image.width), px(x1, image.width), px(y0, image.height), px(y1, image.height));
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| pick(image.pixel(x, y))).count()
    }

    /// Plan A7's shot at V3: the callsigns right of their aircraft and the heights under them,
    /// none overlapping; the aircraft inside the selected one's box loses its label; the selected
    /// one's label in blue.
    #[test]
    fn callsigns_right_of_their_aircraft_heights_below_none_overlapping() {
        let Some(gpu) = gpu() else { return };
        let (_, labels) = scene();
        assert!(labels.with_height(), "zoom 9: the height line shows");
        for scale in [1.0, 2.0] {
            let (image, shown) = shoot_only(&gpu, labels.clone(), scale);
            save(&image, &format!("a7-labels-x{scale}.png"));
            let ids: Vec<u32> = shown.iter().map(|s| s.id).collect();
            assert_eq!(ids, [1, 2, 4, 5], "the third, 30 px left of the selected aircraft, loses its label");
            let texts: Vec<(&str, &str)> = shown.iter().map(|s| (s.text.as_str(), s.height.as_str())).collect();
            assert_eq!(texts, [("SAS1", "FL370"), ("DLH4AB", "3,000 ft"), ("SAS22", "GND"), ("SE-ROA", "2,500 ft")]);
            for s in &shown {
                assert_eq!(s.fade, 1.0, "{}: nothing covers it", s.text);
                let name = s.name.expect("glyphs");
                let height = s.height_rect.expect("a height line");
                // the gap and the glyphs grow with the perspective (an aircraft high up is nearer)
                let gap = labels.gaps[labels.set.index[&s.id]] * s.perspective;
                assert!((0.99..1.6).contains(&s.perspective), "{}: {}", s.text, s.perspective);
                // the callsign starts a gap right of the aircraft, above its middle; the height below
                assert!((name.x0 - (s.anchor[0] + gap)).abs() < 1.5, "{}: {name:?} from {:?}, gap {gap}", s.text, s.anchor);
                assert!(name.y0 < s.anchor[1] - 6.0 && name.y1 > s.anchor[1] - 6.0 && name.y1 < s.anchor[1] + 3.0, "{}: {name:?}", s.text);
                assert!(height.y0 > s.anchor[1] - 3.0 && height.y1 > s.anchor[1] + 6.0, "{}: {height:?}", s.text);
                // and the pixels say so: the callsign's fill in its band right of the aircraft,
                // nothing of it left of the gap
                let ink = count(&image, scale, [name.x0, s.anchor[1] - 12.0, name.x1, s.anchor[1] - 1.0], is_text);
                assert!(ink as f64 > 20.0 * scale * scale, "{}: {ink} text pixels", s.text);
                let left = count(&image, scale, [s.anchor[0] - 40.0, s.anchor[1] - 14.0, s.anchor[0] + gap - 2.0, s.anchor[1] + 14.0], is_text);
                assert_eq!(left, 0, "{}: nothing left of the gap", s.text);
                let below = count(&image, scale, [height.x0, s.anchor[1] + 1.0, height.x1, s.anchor[1] + 12.0], is_text);
                assert!(below as f64 > 15.0 * scale * scale, "{}: {below} height pixels", s.text);
                let blue = count(&image, scale, [name.x0, name.y0, name.x1, height.y1], is_blue);
                if s.id == SELECTED {
                    assert_eq!(s.name_color, AIRCRAFT_LABEL_SELECTED);
                    assert!(blue as f64 > 20.0 * scale * scale, "the selected label in blue: {blue}");
                } else {
                    assert_eq!(blue, 0, "{}", s.text);
                }
            }
            // no two labels' text overlap, and no aircraft lies under another's text
            for (i, a) in shown.iter().enumerate() {
                let a_box = a.name.unwrap().union(&a.height_rect.unwrap());
                for b in &shown[i + 1..] {
                    let b_box = b.name.unwrap().union(&b.height_rect.unwrap());
                    assert!(!a_box.overlaps(&b_box), "{} and {} overlap", a.text, b.text);
                    assert!(!a.claimed.overlaps(&Rect::new(b.anchor[0] - 2.0, b.anchor[1] - 2.0, b.anchor[0] + 2.0, b.anchor[1] + 2.0)) || a.id == SELECTED, "{} claims {}", a.text, b.text);
                }
            }
            // where the hidden label would be, nothing is written (its aircraft at x 300, the
            // selected one's at 330: in between and above the middle)
            let hidden = labels.set.index[&3];
            let gap = labels.gaps[hidden];
            let stray = count(&image, scale, [300.0 + gap - 1.0, 75.0, 330.0 - 2.0, 100.0], is_text);
            assert_eq!(stray, 0, "BAW7 is not drawn");
        }
    }

    /// The labels go through the aircraft layer from `SceneInput::aircraft`, drawn over its icons,
    /// as the test layer draws them alone; with no labels there, none.
    #[test]
    fn the_aircraft_layer_draws_the_scenes_labels() {
        let Some(gpu) = gpu() else { return };
        let (_, labels) = scene();
        let (alone, _) = shoot_only(&gpu, labels.clone(), 1.0);
        let mut input = base(1.0);
        input.aircraft.labels = labels.clone();
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let image = render_with(&gpu, &mut renderer, &input, 1.0).expect("rendered");
        assert!(alone.rgba == image.rgba, "the same picture");
        // with the label setting off the set is let go of: nothing drawn
        input.aircraft.labels = Input::default();
        let image = render_with(&gpu, &mut renderer, &input, 1.0).expect("rendered");
        let text = count(&image, 1.0, [0.0, 0.0, SIZE[0], SIZE[1]], is_text);
        assert_eq!(text, 0);
        // icons and labels together: the labels on top
        let (scene, labels) = scene();
        input.aircraft.scene = scene;
        input.aircraft.labels = labels;
        let image = render_with(&gpu, &mut renderer, &input, 1.0).expect("rendered");
        assert!(!renderer.pending());
        save(&image, "a7-icons-and-labels.png");
        let text = count(&image, 1.0, [0.0, 0.0, SIZE[0], SIZE[1]], is_text);
        assert!(text > count(&alone, 1.0, [0.0, 0.0, SIZE[0], SIZE[1]], is_text), "the icons and the labels");
    }

    /// The label size setting scales the text and the line spacing, not the gap; the work is
    /// redone only when the camera or the input changes.
    #[test]
    fn the_label_size_setting_scales_the_text() {
        let Some(gpu) = gpu() else { return };
        let (scene, _) = scene();
        let flights = flights();
        let big = Labeller::new().build(&scene, true, 1, |row| flights.get(row), 1.5);
        let (_, normal) = shoot_only(&gpu, self::scene().1, 1.0);
        let (_, larger) = shoot_only(&gpu, big, 1.0);
        let by_id = |shown: &[Shown], id: u32| shown.iter().find(|s| s.id == id).cloned().unwrap();
        let (a, b) = (by_id(&normal, 4), by_id(&larger, 4));
        let (na, nb) = (a.name.unwrap(), b.name.unwrap());
        assert!((nb.width() / na.width() - 1.5).abs() < 0.01, "{na:?} {nb:?}");
        // the same gap: from it the glyphs' frames reach 1.5 times as far
        let gap = self::scene().1.gaps[3];
        let from_gap = |s: &Shown, r: Rect| (r.x0 - s.anchor[0]) / s.perspective - gap;
        assert!((from_gap(&b, nb) - 1.5 * from_gap(&a, na)).abs() < 1e-6, "{na:?} {nb:?} gap {gap}");
        // the lines 6 · 1.5 px from the middle
        let mid = |r: Rect| (r.y0 + r.y1) / 2.0;
        assert!((mid(b.height_rect.unwrap()) - mid(nb) - 1.5 * (mid(a.height_rect.unwrap()) - mid(na))).abs() < 0.5);
    }
}
