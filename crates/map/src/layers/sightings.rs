//! Dev builds: the reports each train is drawn from (item T9; doc 02 §13.1; `sightingLayers`,
//! G:2420-2518): row 16 of doc 03 §3 (row 13 of doc 02 §2), drawn by the trains layer
//! ([`super::trains::Trains`]) after the trains and before the selection ring.
//!
//! Today's page draws them only in a dev build (`import.meta.env.DEV`); the app's scene builder
//! fills [`Input`] only in a debug build (`cfg!(debug_assertions)`, as the backend's dev log
//! does), so a release build draws nothing here. What [`Input::of`] takes from the trains on the
//! map (`trains`), the selected train (`selectedTrain`) and where each is drawn (`trainData`, here
//! [`Drawn`]) at the local clock (`Date.now()`: the trains' frames are timed on this machine's
//! clock):
//! - the trains *sighted*: those with reports (`Train::reports`), and the selected one if it has
//!   reports and is not among them;
//! - `dev-predicted`: for each sighted train with predicted frames (`Train::predicted`), where the
//!   real train most likely is now (along those frames, as `positionAt`): a ring of
//!   [`PREDICTED_RADIUS_PX`] px, [`SIGHT_PREDICTED`] [`LINE_PX`] px wide, not filled;
//! - `dev-predicted-label`: "predicted" under the selected train's ring ([`PREDICTED_OFFSET`]);
//! - `dev-sightings`: each sighted train's latest report, a [`SIGHT_LATEST`] disc of
//!   [`LATEST_RADIUS_PX`] px in a white ring [`LINE_PX`] px wide;
//! - `dev-sighting-labels`: over the selected train's latest report ([`SIGHTING_OFFSET`]), how old
//!   it is and its speed, and how far the drawn train is ahead of (+) or behind (−) where the real
//!   one most likely is, and how much faster it is drawn going than reported ([`sighting_text`]).
//!
//! All four are deck's ScatterplotLayer and TextLayer defaults otherwise: circles lying on the
//! ground (`billboard: false`) with radii and lines in pixels; texts 11 px billboards, centred,
//! not SDF, in a [`SIGHT_LABEL_BG`] box padded `[4, 2]` px, the face Overpass Mono 400 where today's
//! is the system's `ui-monospace, monospace` (plan 1.4.10 and section 6, as the signal names').
//! Drawn in `top` (`{cullMode: "none", depthWriteEnabled: false, depthCompare: "always"}`,
//! [`DrawState::LABEL`]): over everything drawn before them.
//!
//! The texts' `updateTriggers` (`getText` once a second) never hold them back today: the layers'
//! data are new arrays at each render, so deck makes every attribute again; here [`Input`] is made
//! at each scene refresh and the texts with it.

use rustradar_core::model::{Sighting, Train, f32_as_js};
use rustradar_core::num::{js_round, js_string};
use rustradar_core::trains::{LatLon, heading_along, position_along};
use rustradar_scene::blocks::metres_between;
use rustradar_scene::geom::js_max;
use rustradar_scene::text::{Face, Family, FontSettings, GlyphAtlas, LaidText, TextLayout};
use rustradar_scene::trains::{Drawn, Way, ahead_of};

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::pipelines::circle::{self, Circle, CircleStyle};
use crate::pipelines::text::{AtlasTexture, Background, BackgroundStyle, Glyph, Label, TextBatch, TextPipeline, TextStyle};
use crate::state::DrawState;

/// The latest reports' discs and their label (`SIGHT_LATEST`).
pub const SIGHT_LATEST: [u8; 4] = [240, 60, 60, 255];
/// The labels' boxes (`SIGHT_LABEL_BG`).
pub const SIGHT_LABEL_BG: [u8; 4] = [12, 14, 20, 225];
/// The predicted places' rings and their label (`SIGHT_PREDICTED`).
pub const SIGHT_PREDICTED: [u8; 4] = [80, 220, 120, 255];
/// The latest reports' discs' ring.
pub const SIGHT_RING: [u8; 4] = [255, 255, 255, 255];
/// `dev-predicted`'s radius, px.
pub const PREDICTED_RADIUS_PX: f64 = 7.0;
/// `dev-sightings`' radius, px.
pub const LATEST_RADIUS_PX: f64 = 4.0;
/// Both circles' line width, px.
pub const LINE_PX: f64 = 2.0;
/// The labels' size, px.
pub const TEXT_SIZE: f64 = 11.0;
/// `dev-predicted-label`'s text and offset (px, y down).
pub const PREDICTED_TEXT: &str = "predicted";
pub const PREDICTED_OFFSET: [f64; 2] = [0.0, 16.0];
/// `dev-sighting-labels`' offset (px, y down).
pub const SIGHTING_OFFSET: [f64; 2] = [0.0, -14.0];
/// The labels' boxes' padding, px (`backgroundPadding`).
pub const LABEL_PADDING: [f64; 2] = [4.0, 2.0];
/// The labels' face (today `ui-monospace, monospace`).
pub const TEXT_FACE: Face = Face::new(Family::OverpassMono, 400);
/// `top`: no cull, no depth write, depth always.
pub const STATE: DrawState = DrawState::LABEL;
/// `dev-predicted`'s props: on the ground, pixels, a ring.
pub const PREDICTED_STYLE: CircleStyle = CircleStyle { filled: false, stroked: true, ..CircleStyle::PIXELS };
/// `dev-sightings`' props: on the ground, pixels, a disc in a ring.
pub const LATEST_STYLE: CircleStyle = CircleStyle { filled: true, stroked: true, ..CircleStyle::PIXELS };

/// The labels' glyph atlas, texels: room for the few characters they use.
const ATLAS: [u32; 2] = [1024, 1024];

/// Where a sighted train most likely is now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Predicted {
    /// `[lon, lat]`.
    pub at: [f64; 2],
    /// It is the selected train's (labelled "predicted").
    pub selected: bool,
}

/// What the scene builder hands this layer (`trains::Input::sightings`). The default draws
/// nothing; so does a release build's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {
    /// `dev-predicted` (and, for the selected one, `dev-predicted-label`).
    pub predicted: Vec<Predicted>,
    /// `dev-sightings`: each sighted train's latest report, `[lon, lat]`.
    pub latest: Vec<[f64; 2]>,
    /// `dev-sighting-labels`: the selected train's latest report, `[lon, lat]`, and its text.
    pub labels: Vec<([f64; 2], String)>,
}

impl Input {
    /// The sightings of `list` (the trains on the map, in its order), `selected` (the selected
    /// train as now known), with the trains of `list` drawn as `drawn` says, at `now_ms` (local
    /// whole Unix ms).
    pub fn of(list: &[&Train], selected: Option<&Train>, drawn: &Drawn, now_ms: i64) -> Input {
        let mut sighted: Vec<&Train> = list.iter().copied().filter(|t| !t.reports.is_empty()).collect();
        let sel = selected.filter(|t| !t.reports.is_empty());
        if let Some(sel) = sel
            && !sighted.iter().any(|t| t.id == sel.id)
        {
            sighted.push(sel);
        }
        let latest = sighted.iter().flat_map(|t| &t.reports).filter(|s| s.latest).map(|s| [s.lon, s.lat]).collect();
        let labels = match sel {
            Some(sel) => {
                let versus = versus_text(sel, drawn, now_ms);
                sel.reports.iter().filter(|s| s.latest).map(|s| ([s.lon, s.lat], sighting_text(s, now_ms, &versus))).collect()
            }
            None => Vec::new(),
        };
        let predicted = sighted
            .iter()
            .filter(|t| !t.predicted.is_empty())
            .map(|t| {
                let LatLon { lat, lon } = position_along(&t.predicted, t.lat, t.lon, now_ms);
                Predicted { at: [lon, lat], selected: sel.is_some_and(|s| s.id == t.id) }
            })
            .collect();
        Input { predicted, latest, labels }
    }

    /// Whether it draws nothing.
    pub fn is_empty(&self) -> bool {
        self.predicted.is_empty() && self.latest.is_empty() && self.labels.is_empty()
    }
}

/// `live {age} s · {speed} km/h{versus}` (or `· no speed`): a report's label.
pub fn sighting_text(s: &Sighting, now_ms: i64, versus: &str) -> String {
    let age = js_max(0.0, js_round((now_ms - s.unix_ms) as f64 / 1000.0));
    let speed = match s.speed_kmh {
        None => "no speed".to_owned(),
        Some(v) => format!("{} km/h", js_string(js_round(f32_as_js(v)))),
    };
    format!("live {} s · {speed}{versus}", js_string(age))
}

/// `+n unit` or `−n unit` (U+2212), `n` the magnitude of `n` rounded as JavaScript does.
pub fn signed(n: f64, unit: &str) -> String {
    format!("{}{} {unit}", if n >= 0.0 { "+" } else { "\u{2212}" }, js_string(js_round(n).abs()))
}

/// ` · drawn ±n m` (how far the drawn train is ahead of where the real one most likely is, along
/// its predicted way, or straight off without a heading) and, with a reported speed,
/// `, ±n km/h` (how much faster it is drawn going over the next second than reported); empty
/// when the train is not drawn or has no predicted frames.
pub fn versus_text(sel: &Train, drawn: &Drawn, now_ms: i64) -> String {
    let Some(i) = drawn.index_of(&sel.id) else { return String::new() };
    if sel.predicted.is_empty() {
        return String::new();
    }
    let [lon, lat, _] = drawn.positions[i];
    let way = Way::of_train(sel);
    let (a, b) = (way.position_at(now_ms), way.position_at(now_ms + 1000));
    let drawn_speed = metres_between([a.lon, a.lat], [b.lon, b.lat]);
    let real = position_along(&sel.predicted, sel.lat, sel.lon, now_ms);
    let reported_heading = sel.heading.map(f32_as_js);
    let heading = heading_along(&sel.predicted, reported_heading, now_ms).or(reported_heading);
    let off = match heading {
        None => metres_between([lon, lat], [real.lon, real.lat]),
        Some(h) => ahead_of(LatLon { lat, lon }, real, h),
    };
    let reported = sel.reports.iter().find(|s| s.latest).and_then(|s| s.speed_kmh).map(f32_as_js);
    let faster = match reported {
        None => String::new(),
        Some(r) => format!(", {}", signed(drawn_speed * 3.6 - r, "km/h")),
    };
    format!(" · drawn {}{faster}", signed(off, "m"))
}

/// The labels as laid out in the atlas.
#[derive(Default)]
struct Laid {
    /// What they were laid out for: the texts, the atlas's generation.
    texts: Vec<String>,
    generation: u64,
    predicted: Option<LaidText>,
    labels: Vec<LaidText>,
}

/// Row 16: `dev-predicted`, `dev-predicted-label`, `dev-sightings`, `dev-sighting-labels`.
pub struct Sightings {
    circles: Circle,
    predicted_style: circle::Style,
    latest_style: circle::Style,
    predicted: circle::Instances,
    latest: circle::Instances,
    atlas: GlyphAtlas,
    texture: AtlasTexture,
    text: TextPipeline,
    predicted_label: TextBatch,
    labels: TextBatch,
    laid: Laid,
}

impl Sightings {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let circles = Circle::new(init, "dev sightings", STATE);
        let predicted_style = circles.style(device, &PREDICTED_STYLE);
        let latest_style = circles.style(device, &LATEST_STYLE);
        let mut atlas = GlyphAtlas::with_size(FontSettings::DEFAULT, ATLAS);
        let texture = AtlasTexture::new(init.gpu, "dev sighting glyphs", &mut atlas);
        let text = TextPipeline::new(init, "dev sighting texts", STATE);
        let predicted_label = text.batch(device, "dev-predicted-label", &texture);
        let labels = text.batch(device, "dev-sighting-labels", &texture);
        Sightings {
            circles,
            predicted_style,
            latest_style,
            predicted: circle::Instances::new(device, "dev-predicted"),
            latest: circle::Instances::new(device, "dev-sightings"),
            atlas,
            texture,
            text,
            predicted_label,
            labels,
            laid: Laid::default(),
        }
    }

    /// What each of the four draws this frame (for tests): predicted rings, the predicted label's
    /// glyphs and boxes, latest discs, the labels' glyphs and boxes.
    pub fn counts(&self) -> [u32; 6] {
        [
            self.predicted.count(),
            self.predicted_label.len(),
            self.predicted_label.background_len(),
            self.latest.count(),
            self.labels.len(),
            self.labels.background_len(),
        ]
    }

    /// Lays out the texts unless they already are, again from scratch whenever the atlas packs
    /// itself anew (which moves the glyphs).
    fn lay_out(&mut self, input: &Input) {
        let mut texts: Vec<String> = input.labels.iter().map(|(_, t)| t.clone()).collect();
        let predicted = input.predicted.iter().any(|p| p.selected);
        if predicted {
            texts.push(PREDICTED_TEXT.to_owned());
        }
        for _ in 0..4 {
            let generation = self.atlas.generation();
            if self.laid.texts == texts && self.laid.generation == generation {
                return;
            }
            let middle = TextLayout::DEFAULT;
            let labels = input.labels.iter().map(|(_, t)| self.atlas.layout(TEXT_FACE, t, &middle)).collect();
            let predicted = predicted.then(|| self.atlas.layout(TEXT_FACE, PREDICTED_TEXT, &middle));
            self.laid = Laid { texts: texts.clone(), generation: self.atlas.generation(), predicted, labels };
            if self.atlas.generation() == generation {
                return;
            }
        }
    }

    /// One frame of `input`: the circles and texts placed for this frame's camera.
    pub fn update(&mut self, cx: &PrepareCx<'_>, input: &Input) {
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        let rings: Vec<circle::Instance> = input
            .predicted
            .iter()
            .map(|p| circle::Instance::at(cx.view, p.at, cx.scene.stands_on.at(p.at)).radius(PREDICTED_RADIUS_PX).line(SIGHT_PREDICTED, LINE_PX))
            .collect();
        self.predicted.write(device, queue, &rings);
        let discs: Vec<circle::Instance> = input
            .latest
            .iter()
            .map(|&at| circle::Instance::at(cx.view, at, cx.scene.stands_on.at(at)).radius(LATEST_RADIUS_PX).fill(SIGHT_LATEST).line(SIGHT_RING, LINE_PX))
            .collect();
        self.latest.write(device, queue, &discs);

        self.lay_out(input);
        self.texture.sync(cx.gpu, &mut self.atlas);
        let boxes_style = BackgroundStyle::DEFAULT.padded(&LABEL_PADDING);
        let (mut glyphs, mut boxes): (Vec<Glyph>, Vec<Background>) = (Vec::new(), Vec::new());
        if let Some(laid) = &self.laid.predicted {
            for p in input.predicted.iter().filter(|p| p.selected) {
                let label = Label::at(cx.view, [p.at[0], p.at[1], cx.scene.stands_on.at(p.at)], TEXT_SIZE, SIGHT_PREDICTED).with_pixel_offset(PREDICTED_OFFSET);
                boxes.push(label.background(laid, SIGHT_LABEL_BG));
                glyphs.extend(label.glyphs(laid));
            }
        }
        self.predicted_label.set_style(queue, &TextStyle::DEFAULT, &boxes_style, &FontSettings::DEFAULT, cx.view);
        self.predicted_label.set_backgrounds(device, queue, &boxes);
        self.predicted_label.set_glyphs(device, queue, &glyphs);

        let (mut glyphs, mut boxes): (Vec<Glyph>, Vec<Background>) = (Vec::new(), Vec::new());
        for ((at, _), laid) in input.labels.iter().zip(&self.laid.labels) {
            let label = Label::at(cx.view, [at[0], at[1], cx.scene.stands_on.at(*at)], TEXT_SIZE, SIGHT_LATEST).with_pixel_offset(SIGHTING_OFFSET);
            boxes.push(label.background(laid, SIGHT_LABEL_BG));
            glyphs.extend(label.glyphs(laid));
        }
        self.labels.set_style(queue, &TextStyle::DEFAULT, &boxes_style, &FontSettings::DEFAULT, cx.view);
        self.labels.set_backgrounds(device, queue, &boxes);
        self.labels.set_glyphs(device, queue, &glyphs);
    }
}

impl Layer for Sightings {
    fn name(&self) -> &'static str {
        "sightings"
    }

    /// The scene's sightings (`trains::Input::sightings`).
    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let scene = cx.scene;
        self.update(cx, &scene.trains.sightings);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map {
            return;
        }
        self.circles.draw(pass, &self.predicted_style, &self.predicted);
        self.text.draw(pass, &self.predicted_label);
        self.circles.draw(pass, &self.latest_style, &self.latest);
        self.text.draw(pass, &self.labels);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use rustradar_core::geo::Basemap;
    use rustradar_core::model::{TrainKind, TrainSource};
    use rustradar_scene::camera::{ViewState, viewport_for};
    use rustradar_scene::trains::TrainMotion;

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;
    const STOCKHOLM: [f64; 2] = [18.07, 59.33];
    const LAND: [u8; 3] = [50, 50, 50];
    const SIZE: [f64; 2] = [400.0, 300.0];

    /// `east` and `north` flat-earth metres (111 320 m a degree) from `[lon, lat]`.
    fn moved([lon, lat]: [f64; 2], east: f64, north: f64) -> [f64; 2] {
        [lon + east / (111_320.0 * lat.to_radians().cos()), lat + north / 111_320.0]
    }

    fn train(id: &str, at: [f64; 2]) -> Train {
        Train {
            id: id.into(),
            source: TrainSource::Trafikverket,
            name: format!("Train {id}"),
            kind: TrainKind::Regional,
            lat: at[1],
            lon: at[0],
            heading: Some(90.0),
            speed_kmh: None,
            destination: None,
            operator: None,
            delay_minutes: None,
            position_unix: NOW / 1000,
            frames: Vec::new(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: None,
            tail: Vec::new(),
            structures: Vec::new(),
            ended_unix: None,
            shape: None,
            model: None,
        }
    }

    fn frame(at: [f64; 2], unix_ms: i64) -> (f64, f64, i64) {
        (at[1], at[0], unix_ms)
    }

    fn report(at: [f64; 2], age_ms: i64, speed: Option<f32>, latest: bool) -> Sighting {
        Sighting { lat: at[1], lon: at[0], unix_ms: NOW - age_ms, speed_kmh: speed, latest }
    }

    /// Train `a` heading east at 20 m/s from Stockholm, its latest report 3.4 s old at 60 km/h 10 m
    /// behind, the one before 40 m behind, and the real train most likely 50 m on; `b` with a
    /// report and no speed, nothing predicted; `c` predicted but never reported.
    fn trains() -> Vec<Train> {
        let mut a = train("a", STOCKHOLM);
        a.frames = vec![frame(STOCKHOLM, NOW), frame(moved(STOCKHOLM, 200.0, 0.0), NOW + 10_000)];
        a.reports = vec![report(moved(STOCKHOLM, -40.0, 0.0), 33_000, Some(58.0), false), report(moved(STOCKHOLM, -10.0, 0.0), 3_400, Some(60.0), true)];
        a.predicted = vec![frame(moved(STOCKHOLM, -50.0, 0.0), NOW - 5_000), frame(moved(STOCKHOLM, 150.0, 0.0), NOW + 5_000)];
        let mut b = train("b", moved(STOCKHOLM, 0.0, 80.0));
        b.reports = vec![report(moved(STOCKHOLM, 0.0, 80.0), 500, None, true)];
        let mut c = train("c", moved(STOCKHOLM, 0.0, -80.0));
        c.predicted = vec![frame(moved(STOCKHOLM, 0.0, -80.0), NOW)];
        vec![a, b, c]
    }

    fn drawn(list: &[&Train]) -> Drawn {
        let mut motion = TrainMotion::new();
        motion.set_list(list, NOW);
        motion.step(list, NOW);
        motion.drawn().clone()
    }

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn each_sighted_train_shows_its_latest_report_and_where_it_most_likely_is() {
        let list = trains();
        let refs: Vec<&Train> = list.iter().collect();
        let input = Input::of(&refs, Some(&list[0]), &drawn(&refs), NOW);
        // the latest reports of a and b; c was never reported
        assert_eq!(input.latest.len(), 2);
        assert!(close(input.latest[0], moved(STOCKHOLM, -10.0, 0.0)) && close(input.latest[1], moved(STOCKHOLM, 0.0, 80.0)));
        // where a most likely is now: half way along its predicted frames, 50 m on; labelled
        assert_eq!(input.predicted.len(), 1);
        assert!(close(input.predicted[0].at, moved(STOCKHOLM, 50.0, 0.0)) && input.predicted[0].selected);
        // the selected train's latest report labelled
        assert_eq!(input.labels.len(), 1);
        assert!(close(input.labels[0].0, moved(STOCKHOLM, -10.0, 0.0)));
        // nothing selected: the same dots and rings, no labels
        let none = Input::of(&refs, None, &drawn(&refs), NOW);
        assert_eq!((none.latest.len(), none.predicted.len(), none.labels.len()), (2, 1, 0));
        assert!(!none.predicted[0].selected);
        // no reports at all: nothing
        assert!(Input::of(&refs[2..], None, &drawn(&refs[2..]), NOW).is_empty());
        assert!(Input::default().is_empty());
    }

    /// The label: 3.4 s is "3 s", 60 km/h reported; drawn at the start of its frames, 50 m behind
    /// the real train going east, and going 72 km/h along them, 12 faster than reported.
    #[test]
    fn the_selected_train_s_label_says_how_old_and_fast_its_report_is_and_how_far_off_the_drawn_train_is() {
        let list = trains();
        let refs: Vec<&Train> = list.iter().collect();
        let input = Input::of(&refs, Some(&list[0]), &drawn(&refs), NOW);
        assert_eq!(input.labels[0].1, "live 3 s · 60 km/h · drawn \u{2212}50 m, +12 km/h");
        // b selected: no speed reported, nothing predicted, so nothing to compare
        let input = Input::of(&refs, Some(&list[1]), &drawn(&refs), NOW);
        let b = &list[1].reports[0];
        assert_eq!(input.labels, [([b.lon, b.lat], "live 1 s · no speed".to_owned())]);
        // a without a heading anywhere: how far off, unsigned by direction (+)
        let mut still = list[0].clone();
        still.heading = None;
        still.predicted = vec![frame(moved(STOCKHOLM, 30.0, 40.0), NOW)];
        still.frames.clear();
        let refs = [&still];
        assert_eq!(versus_text(&still, &drawn(&refs), NOW), " · drawn +50 m, \u{2212}60 km/h");
        // not drawn (not on the list): no comparison
        assert_eq!(versus_text(&list[0], &Drawn::default(), NOW), "");
    }

    #[test]
    fn a_selected_train_off_the_list_is_sighted_too() {
        let list = trains();
        let refs: Vec<&Train> = list[1..].iter().collect();
        let input = Input::of(&refs, Some(&list[0]), &drawn(&refs), NOW);
        assert_eq!(input.latest.len(), 2);
        assert!(close(input.latest[1], moved(STOCKHOLM, -10.0, 0.0)), "after the list's");
        assert_eq!(input.predicted.len(), 1);
        // not drawn: its label says no more than its report
        assert_eq!(input.labels[0].1, "live 3 s · 60 km/h");
        // a selected train without reports is not sighted
        let mut quiet = list[0].clone();
        quiet.reports.clear();
        assert_eq!(Input::of(&refs, Some(&quiet), &drawn(&refs), NOW).latest.len(), 1);
    }

    #[test]
    fn numbers_are_signed_and_rounded_as_today_s() {
        assert_eq!(signed(12.4, "m"), "+12 m");
        assert_eq!(signed(0.0, "m"), "+0 m");
        assert_eq!(signed(-0.3, "m"), "\u{2212}0 m");
        // Math.round rounds half up: −2.5 is −2
        assert_eq!(signed(-2.5, "km/h"), "\u{2212}2 km/h");
        assert_eq!(signed(2.5, "km/h"), "+3 km/h");
        assert_eq!(signed(f64::NAN, "m"), "\u{2212}NaN m");
        // a report from the future is 0 s old
        let s = report(STOCKHOLM, -2_000, Some(87.5), true);
        assert_eq!(sighting_text(&s, NOW, ""), "live 0 s · 88 km/h");
    }

    /// A second sightings layer drawing `input` as the map's own would, and what it drew.
    struct Probe {
        sightings: Sightings,
        input: Input,
        counts: Rc<Cell<[u32; 6]>>,
    }

    impl Layer for Probe {
        fn name(&self) -> &'static str {
            "t9 probe"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.sightings.update(cx, &self.input);
            self.counts.set(self.sightings.counts());
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.sightings.draw(pass, cx);
        }
    }

    fn view() -> ViewState {
        ViewState { longitude: STOCKHOLM[0], latitude: STOCKHOLM[1], zoom: 15.0, bearing: 0.0, pitch: 0.0, position: None }
    }

    fn scene() -> SceneInput {
        let mut scene = SceneInput::new(view(), SIZE, 1.0, Basemap::Dark);
        scene.earth = crate::layers::earth::Input::SHOWN;
        scene.local_now_ms = NOW as f64;
        scene.feed_now_ms = NOW as f64;
        scene
    }

    fn input() -> Input {
        let list = trains();
        let refs: Vec<&Train> = list.iter().collect();
        Input::of(&refs, Some(&list[0]), &drawn(&refs), NOW)
    }

    fn screen([lon, lat]: [f64; 2]) -> [u32; 2] {
        let p = viewport_for(&view(), SIZE).project([lon, lat, 0.0]);
        [p[0].round() as u32, p[1].round() as u32]
    }

    fn save(image: &Image, name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots");
        std::fs::create_dir_all(dir).expect("shot folder");
        image.save_png(&std::path::Path::new(dir).join(name)).expect("png");
    }

    fn near(p: [u8; 4], want: [u8; 3], tolerance: u8) -> bool {
        (0..3).all(|c| p[c].abs_diff(want[c]) <= tolerance)
    }

    /// `top` at `alpha` over `under`.
    fn over(top: [u8; 4], under: [u8; 3]) -> [u8; 3] {
        let a = f64::from(top[3]) / 255.0;
        [0, 1, 2].map(|c| (f64::from(top[c]) * a + f64::from(under[c]) * (1.0 - a)).round() as u8)
    }

    /// How many pixels of the rectangle `[x0, y0, x1, y1)` are within `tolerance` of `rgb`.
    fn count(image: &Image, [x0, y0, x1, y1]: [u32; 4], rgb: [u8; 3], tolerance: u8) -> usize {
        count_where(image, [x0, y0, x1, y1], |p| near(p, rgb, tolerance))
    }

    fn count_where(image: &Image, [x0, y0, x1, y1]: [u32; 4], f: impl Fn([u8; 4]) -> bool) -> usize {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| f(image.pixel(x, y))).count()
    }

    /// Text ink in a channel: thin 11 px strokes blend with their box, so a hue, not the colour.
    fn ink(p: [u8; 4], channel: usize) -> bool {
        (0..3).all(|c| c == channel || p[channel] > p[c].saturating_add(50)) && p[channel] > 80
    }

    /// Straight down at zoom 15 over plain land: the red disc in its white ring at the latest
    /// report, its label above it in a dark box in the report's red, the green ring 50 m on where
    /// the train most likely is (hollow), "predicted" under it in green.
    #[test]
    fn red_discs_green_rings_and_their_labels_over_the_map() {
        let Some(gpu) = gpu() else { return };
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let counts = Rc::new(Cell::new([u32::MAX; 6]));
        let probe = Probe { sightings: Sightings::new(&renderer.init(&gpu)), input: input(), counts: counts.clone() };
        renderer.add_layer(Box::new(probe));
        let image = render_with(&gpu, &mut renderer, &scene(), 1.0).expect("rendered");
        save(&image, "t9-sightings.png");
        let counts = counts.get();
        assert_eq!([counts[0], counts[2], counts[3], counts[5]], [1, 1, 2, 1], "{counts:?}");
        assert_eq!(counts[1], PREDICTED_TEXT.chars().count() as u32);
        assert!(counts[4] > 30, "{counts:?}");

        let [x, y] = screen(moved(STOCKHOLM, -10.0, 0.0));
        assert!(near(image.pixel(x, y), [240, 60, 60], 3), "{:?}", image.pixel(x, y));
        // the ring: white from 3 to 5 px out, then the land
        for (dx, dy) in [(4, 0), (0, 4), (-4, 0)] {
            let p = image.pixel(x.wrapping_add_signed(dx), y.wrapping_add_signed(dy));
            assert!(near(p, [255, 255, 255], 30), "{dx},{dy}: {p:?}");
        }
        assert!(near(image.pixel(x, y + 7), LAND, 2));
        // its label: the box over the land 14 px up, the red text in it
        let boxed = over(SIGHT_LABEL_BG, LAND);
        let label = [x - 150, y - 24, x + 150, y - 5];
        assert!(count(&image, label, boxed, 2) > 1000, "{}", count(&image, label, boxed, 2));
        let red = count_where(&image, label, |p| ink(p, 0));
        assert!(red > 200, "{red} px of red text");
        // the box is centred on the report, 7 px tall plus its padding
        let rows: Vec<u32> = (y - 30..y).filter(|&r| near(image.pixel(x - 60, r), boxed, 2) || near(image.pixel(x + 60, r), boxed, 2)).collect();
        assert!(!rows.is_empty() && (y - 14).abs_diff((rows[0] + rows[rows.len() - 1]) / 2) <= 1, "{rows:?}");

        // the predicted place: a hollow green ring of 7 px
        let [gx, gy] = screen(moved(STOCKHOLM, 50.0, 0.0));
        assert!(near(image.pixel(gx + 7, gy), [80, 220, 120], 40), "{:?}", image.pixel(gx + 7, gy));
        assert!(near(image.pixel(gx - 7, gy), [80, 220, 120], 40));
        assert!(near(image.pixel(gx, gy), LAND, 2), "hollow");
        // "predicted" under it, in green on its box
        let label = [gx - 40, gy + 8, gx + 40, gy + 25];
        assert!(count(&image, label, boxed, 2) > 200);
        let green = count_where(&image, label, |p| ink(p, 1));
        assert!(green > 40, "{green} px of green text");
    }

    /// The map's own trains layer draws the scene's sightings (`trains::Input::sightings`).
    #[test]
    fn the_trains_layer_draws_the_scene_s_sightings() {
        let Some(gpu) = gpu() else { return };
        let mut scene = scene();
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let empty = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        scene.trains.sightings = input();
        let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        let [x, y] = screen(moved(STOCKHOLM, -10.0, 0.0));
        assert!(near(empty.pixel(x, y), LAND, 2));
        assert!(near(image.pixel(x, y), [240, 60, 60], 3), "{:?}", image.pixel(x, y));
        let [gx, gy] = screen(moved(STOCKHOLM, 50.0, 0.0));
        assert!(near(image.pixel(gx + 7, gy), [80, 220, 120], 40));
    }
}
