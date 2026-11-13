//! Trains: the selected train's route, the selection halo, icons or car meshes (with tunnel layers),
//! dev sightings, the selection ring (doc 02 §5, §7, §13.1): rows 13–17 of doc 03 §3 (doc 02 §2,
//! rows 10–14), drawn in that order.
//!
//! The icons (item T2; doc 02 §5.2–5.5, G:2031-2071): the scene builder hands over each refresh's
//! [`TrainsScene`] ([`rustradar_scene::trains::TrainBuilder`]) in [`Input::scene`]: further out
//! every train as one icon, already placed where it is drawn, sized, turned and coloured. This layer
//! draws them as deck's IconLayer `train-icons` with [`crate::pipelines::icon`]:
//! - the atlas of [`rustradar_scene::atlas::trains`], drawn once on the CPU when the layer is made
//!   and uploaded as RGBA8 with a full mip chain (so it is in from the first frame; deck's
//!   `trainAtlas()` is drawn as synchronously);
//! - full-colour cells (`mask: false`): the atlas's colours, faded by the instance's alpha (240,
//!   90 in a tunnel);
//! - lying on the ground ([`STYLE`], `billboard: false`), sizes in pixels at the view centre's
//!   depth, turned anticlockwise by `−heading` in the true east/north frame (see the scene module
//!   for the half turn deck adds on the globe);
//! - the [`DrawState::TRAIN`] state (`TRAIN_PARAMETERS`: no cull, no depth write, depth test on,
//!   so the globe hides the trains on its far side);
//! - the train hovered in the sidebar's list highlighted [`ICON_HIGHLIGHT`] as deck's picking
//!   module blends it.
//!
//! Instances are rebuilt in each `prepare` from the scene and this frame's view (common offsets
//! from the frame's centre, plan 1.4.2) and written to the GPU only when they change.
//!
//! The other rows are other items': the route and halo (T6), the cars instead of the icons close
//! in (T3, [`TrainsScene::cars`]), the dev sightings (T9), the ring (T6).
//!
//! Stage 2 stub (E4): keep `new(&Init) -> Self` and the row in `layers::ordered`.

use rustradar_scene::atlas::trains as atlas;
use rustradar_scene::trains::{ICON_HIGHLIGHT, TrainIcon, TrainsScene};

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::frame::ViewFrame;
use crate::pipelines::bitmap::upload_rgba8;
use crate::pipelines::icon::{Batch, IconDef, IconPipeline, IconStyle, Instance};
use crate::state::DrawState;

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// This scene refresh's trains (item T2): their icons further out; empty without trains.
    pub scene: TrainsScene,
    /// Dev builds: the reports each train is drawn from (item T9, [`super::sightings`]); empty
    /// in a release build.
    pub sightings: super::sightings::Input,
    /// The selected train's route, halo and ring (item T6, [`super::train_route`]); the default
    /// draws nothing.
    pub route: super::train_route::Input,
}

/// `train-icons`' props: deck's IconLayer defaults lying on the ground (`billboard: false`,
/// `sizeUnits: "pixels"`).
pub const STYLE: IconStyle = IconStyle::GROUND;

/// `train-icons`' draw state (`TRAIN_PARAMETERS`).
pub const STATE: DrawState = DrawState::TRAIN;

/// The atlas cell of the icon `name`, as the icon pipeline takes it (`mask: false`); none for a
/// name the atlas lacks (deck's `MISSING_ICON` draws nothing).
pub fn cell(name: &str) -> Option<IconDef> {
    let c = atlas::cell(name)?;
    Some(IconDef::cell(f64::from(c.x), f64::from(c.y), f64::from(c.width), f64::from(c.height), false))
}

/// The instances of `train-icons`: every icon of `icons` with a cell, at its position, `size_px`
/// big, turned by its angle, in its colour, the highlighted one with [`ICON_HIGHLIGHT`].
pub fn icon_instances(view: &ViewFrame, icons: &[TrainIcon]) -> Vec<Instance> {
    icons
        .iter()
        .filter_map(|icon| {
            let instance = Instance::at(view, icon.position, &cell(&icon.name)?, icon.size_px, icon.angle, icon.color);
            Some(if icon.highlighted { instance.with_highlight(ICON_HIGHLIGHT) } else { instance })
        })
        .collect()
}

/// Row 15 further out: `train-icons`.
struct Icons {
    pipeline: IconPipeline,
    /// Kept alive for the batch's bind group.
    _atlas: wgpu::Texture,
    batch: Batch,
    written: Vec<Instance>,
}

impl Icons {
    fn new(init: &Init<'_>) -> Self {
        let drawn = atlas::draw();
        let size = [drawn.width, drawn.height];
        let texture = upload_rgba8(init.gpu, "train icon atlas", size, &drawn.rgba, true);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let pipeline = IconPipeline::new(init, "train icons", STATE);
        let batch = pipeline.batch(&init.gpu.device, "train-icons", &view, size);
        Icons { pipeline, _atlas: texture, batch, written: Vec::new() }
    }

    fn prepare(&mut self, cx: &PrepareCx<'_>, icons: &[TrainIcon]) {
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        self.batch.set_style(queue, &STYLE, cx.view);
        let instances = icon_instances(cx.view, icons);
        if instances != self.written {
            self.batch.set_instances(device, queue, &instances);
            self.written = instances;
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.pipeline.draw(pass, &self.batch);
    }

    fn len(&self) -> usize {
        self.batch.len() as usize
    }
}

/// Rows 13–17: train route, halo, trains, sightings, ring.
pub struct Trains {
    icons: Icons,
    /// Close in, the cars instead of the icons (item T3).
    cars: super::train_cars::TrainCars,
    sightings: super::sightings::Sightings,
    /// The selected train's route, halo and ring (item T6).
    route: super::train_route::SelectedTrain,
}

impl Trains {
    pub fn new(init: &Init<'_>) -> Self {
        Trains {
            icons: Icons::new(init),
            cars: super::train_cars::TrainCars::new(init),
            sightings: super::sightings::Sightings::new(init),
            route: super::train_route::SelectedTrain::new(init),
        }
    }

    /// The selected train's route, halo and ring as drawn this frame (item T6).
    pub fn route(&self) -> &super::train_route::SelectedTrain {
        &self.route
    }

    /// How many train icons this frame draws.
    pub fn icons_drawn(&self) -> usize {
        self.icons.len()
    }

    /// This frame's car draws: deck's layer ids and how many cars each draws.
    pub fn cars_drawn(&self) -> Vec<(String, u32)> {
        self.cars.drawn()
    }
}

impl Layer for Trains {
    fn name(&self) -> &'static str {
        "trains"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let scene = &cx.scene.trains.scene;
        self.icons.prepare(cx, &scene.icons);
        self.cars.prepare(cx, &scene.car_layers);
        self.sightings.prepare(cx);
        self.route.prepare(cx);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        // on the 3D terrain the route's lines (deck's PathLayers) go into the covers, its stops and
        // everything else of the trains stand in the map pass (item G3)
        if cx.draws_draped() {
            self.route.draw_route_lines(pass, cx);
        }
        if cx.pass != Pass::Map {
            return;
        }
        // the selected train's route and halo (T6), under the trains
        self.route.draw_route_stops(pass);
        self.route.draw_halo(pass);
        self.icons.draw(pass);
        // close in, the cars instead (T3)
        self.cars.draw(pass, cx);
        // the dev sightings (T9), then the selected train's ring (T6), over them
        self.sightings.draw(pass, cx);
        self.route.draw_ring(pass);
    }

    fn drape(&self) -> super::Drape {
        // the route's dashes and its way to the stop draped over the 3D terrain (item G3)
        super::Drape::Part
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_core::model::{Train, TrainKind, TrainSource};
    use rustradar_scene::camera::{ViewState, viewport_for};
    use rustradar_scene::trains::TrainBuilder;

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;
    const SIZE: [f64; 2] = [400.0, 200.0];
    const SCALE: f64 = 2.0;
    /// Stockholm at zoom 11: icons at their full size, on the globe.
    const VIEW: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 11.0, bearing: 0.0, pitch: 0.0, position: None };

    /// A train of `kind` run by `operator` heading `heading`, standing at `xy` on the canvas of
    /// [`VIEW`].
    fn train(id: &str, kind: TrainKind, operator: Option<&str>, heading: Option<f32>, xy: [f64; 2], view: &ViewState) -> Train {
        let [lon, lat] = viewport_for(view, SIZE).unproject(xy, 0.0);
        Train {
            id: id.into(),
            source: TrainSource::Trafikverket,
            name: format!("Train {id}"),
            kind,
            lat,
            lon,
            heading,
            speed_kmh: None,
            destination: None,
            operator: operator.map(str::to_owned),
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

    /// A row of trains across the canvas: north, east, south, no heading, and a highlighted one.
    fn row(view: &ViewState) -> Vec<Train> {
        vec![
            train("n", TrainKind::HighSpeed, None, Some(0.0), [60.0, 100.0], view),
            train("e", TrainKind::Regional, Some("SJ"), Some(90.0), [130.0, 100.0], view),
            train("s", TrainKind::Intercity, Some("Mälartåg"), Some(180.0), [200.0, 100.0], view),
            train("box", TrainKind::Suburban, None, None, [270.0, 100.0], view),
            train("hover", TrainKind::Other, Some("Arlanda Express"), Some(0.0), [340.0, 100.0], view),
        ]
    }

    fn trains_scene(view: &ViewState, trains: &[Train], hovered: Option<&str>) -> TrainsScene {
        let list: Vec<&Train> = trains.iter().collect();
        TrainBuilder::new().build(&list, true, view, hovered, NOW)
    }

    fn shoot(gpu: &Gpu, view: ViewState, trains: TrainsScene) -> Image {
        let mut scene = SceneInput::new(view, SIZE, SCALE, Basemap::Dark);
        scene.local_now_ms = NOW as f64;
        scene.feed_now_ms = NOW as f64;
        scene.trains.scene = trains;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let image = render_with(gpu, &mut renderer, &scene, SCALE).expect("rendered");
        assert!(!renderer.pending(), "nothing waits: the atlas is in from the start");
        image
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// `rgb` at the trains' alpha (240) over the map pixel `ground`.
    fn over(rgb: [u8; 3], ground: [u8; 4]) -> [u8; 3] {
        let a = 240.0 / 255.0;
        [0, 1, 2].map(|c| (f64::from(rgb[c]) * a + f64::from(ground[c]) * (1.0 - a)).round() as u8)
    }

    fn close(a: [u8; 4], b: [u8; 3], tolerance: u8) -> bool {
        (0..3).all(|c| a[c].abs_diff(b[c]) <= tolerance)
    }

    /// The pixels within `r` device px of `[x, y]` within `tolerance` of `rgb`: their bounding box
    /// `[x0, y0, x1, y1]` (inclusive) and how many.
    fn coloured(image: &Image, [x, y]: [f64; 2], r: f64, rgb: [u8; 3], tolerance: u8) -> Option<([u32; 4], usize)> {
        let (mut b, mut n): (Option<[u32; 4]>, usize) = (None, 0);
        for py in (y - r).max(0.0) as u32..((y + r) as u32).min(image.height) {
            for px in (x - r).max(0.0) as u32..((x + r) as u32).min(image.width) {
                if close(image.pixel(px, py), rgb, tolerance) {
                    let e = b.get_or_insert([px, py, px, py]);
                    *e = [e[0].min(px), e[1].min(py), e[2].max(px), e[3].max(py)];
                    n += 1;
                }
            }
        }
        b.map(|b| (b, n))
    }

    #[test]
    fn instances_take_the_icons_cells_sizes_angles_and_colours() {
        let view = ViewFrame::new(&VIEW, SIZE, SCALE, 0.0);
        let trains = row(&VIEW);
        let scene = trains_scene(&VIEW, &trains, Some("hover"));
        let instances = icon_instances(&view, &scene.icons);
        assert_eq!(instances.len(), 5);
        let names: Vec<&str> = scene.icons.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["car-highSpeed", "car-op0", "car-op2", "box-suburban", "car-op6"]);
        for (icon, drawn) in scene.icons.iter().zip(&instances) {
            assert_eq!(drawn.position, <[f32; 3]>::from(view.common_offset([icon.position[0], icon.position[1]], 0.0)));
            assert_eq!((drawn.size, drawn.angle), (icon.size_px as f32, icon.angle as f32));
            assert_eq!(drawn.color_mode, 0.0, "full colour, not a mask");
            assert_eq!(drawn.offset, [0.0, 0.0], "anchored at the middle");
            let c = atlas::cell(&icon.name).unwrap();
            assert_eq!(drawn.frame, [c.x, c.y, c.width, c.height].map(|v| v as f32));
        }
        assert_eq!(instances.iter().map(|i| i.angle).collect::<Vec<_>>(), [0.0, -90.0, -180.0, 0.0, 0.0]);
        assert_eq!(instances.iter().map(|i| i.size).collect::<Vec<_>>(), [24.0, 21.0, 23.0, 16.0, 19.0]);
        assert_eq!(instances[1].color, [232, 232, 236, 240]);
        assert_eq!(instances.iter().map(|i| i.highlight).collect::<Vec<_>>()[3..], [[0; 4], ICON_HIGHLIGHT]);
        // a name without a cell draws nothing
        let mut icons = scene.icons.clone();
        icons[0].name = "car-nobody".into();
        assert_eq!(icon_instances(&view, &icons).len(), 4);
    }

    /// The row as the map draws it: each icon in its operator's or kind's colour, as long as its
    /// size, its carriage pointing the way it goes, the square without a heading, the hovered one
    /// blended toward the selection blue.
    #[test]
    fn icons_lie_in_their_colours_turned_to_their_headings() {
        let Some(gpu) = gpu() else { return };
        let trains = row(&VIEW);
        let image = shoot(&gpu, VIEW, trains_scene(&VIEW, &trains, Some("hover")));
        save(&image, "t2-train-icons-row.png");
        let at = |xy: [f64; 2]| [xy[0] * SCALE, xy[1] * SCALE];
        let ground = image.pixel(4, 4);
        let (magenta, silver, red, cerulean) =
            (over([226, 92, 196], ground), over([232, 232, 236], ground), over([236, 84, 84], ground), over([112, 190, 232], ground));
        // the high-speed train north: magenta, its cell 24 px tall: the carriage and its border
        // (56 + 5 of the 64 texels long, 26 + 5 wide) stand out from the map that size
        let (_, n) = coloured(&image, at([60.0, 100.0]), 40.0, magenta, 6).expect("the magenta carriage");
        assert!(n > 200, "{n}");
        let [x, y] = at([60.0, 100.0]);
        let mut b: Option<[u32; 4]> = None;
        for py in (y - 40.0) as u32..(y + 40.0) as u32 {
            for px in (x - 40.0) as u32..(x + 40.0) as u32 {
                if !close(image.pixel(px, py), [ground[0], ground[1], ground[2]], 8) {
                    let e = b.get_or_insert([px, py, px, py]);
                    *e = [e[0].min(px), e[1].min(py), e[2].max(px), e[3].max(py)];
                }
            }
        }
        let b = b.expect("drawn");
        let (w, h) = (f64::from(b[2] - b[0] + 1), f64::from(b[3] - b[1] + 1));
        let px = 24.0 * SCALE / 64.0;
        assert!((h - 61.0 * px).abs() <= 2.0, "{w} x {h}");
        assert!((w - 31.0 * px).abs() <= 2.0, "{w} x {h}");
        // centred on the train
        assert!((f64::from(b[0] + b[2] + 1) / 2.0 - x).abs() <= 1.0 && (f64::from(b[1] + b[3] + 1) / 2.0 - y).abs() <= 1.0, "{b:?}");
        // SJ's silver heading east: lying across
        let (b, _) = coloured(&image, at([130.0, 100.0]), 40.0, silver, 6).expect("the silver carriage");
        assert!(b[2] - b[0] > b[3] - b[1] + 10, "east: wider than tall {b:?}");
        // the nose leads: the carriage's half toward its heading is narrower
        let wide_at = |x: u32| (b[1]..=b[3]).filter(|&y| close(image.pixel(x, y), silver, 6)).count();
        assert!(wide_at(b[2] - 2) < wide_at(b[0] + 2), "nose east: {} vs {}", wide_at(b[2] - 2), wide_at(b[0] + 2));
        // Mälartåg's red heading south: nose down
        let (b, _) = coloured(&image, at([200.0, 100.0]), 40.0, red, 6).expect("the red carriage");
        let wide_at = |y: u32| (b[0]..=b[2]).filter(|&x| close(image.pixel(x, y), red, 6)).count();
        assert!(b[3] - b[1] > b[2] - b[0] + 10 && wide_at(b[3] - 2) < wide_at(b[1] + 2), "south {b:?}");
        // no heading: a cerulean square of 36/64 of 0.8 · 20 px
        let (b, _) = coloured(&image, at([270.0, 100.0]), 40.0, cerulean, 6).expect("the square");
        let side = 36.0 * 16.0 * SCALE / 64.0;
        assert!((f64::from(b[2] - b[0] + 1) - side).abs() <= 2.5 && (f64::from(b[3] - b[1] + 1) - side).abs() <= 2.5, "{b:?}");
        // the hovered one: Arlanda Express's yellow blended toward the blue by 200/255, 240/255 opaque
        let (a, ah) = (240.0 / 255.0, 200.0 / 255.0);
        let t = ah / (ah + a * (1.0 - ah));
        let mix = |c: f64, h: f64| c + (h - c) * t;
        let want = [mix(250.0, 55.0), mix(212.0, 142.0), mix(64.0, 240.0)];
        let centre = image.pixel((340.0 * SCALE) as u32, (100.0 * SCALE) as u32);
        // over the dark map at the blended alpha
        let alpha = ah + a * (1.0 - ah);
        for c in 0..3 {
            let expected = want[c] * alpha + f64::from(ground[c]) * (1.0 - alpha);
            assert!((f64::from(centre[c]) - expected).abs() <= 6.0, "{centre:?} vs {expected} on {c}");
        }
        assert!(coloured(&image, at([340.0, 100.0]), 40.0, [250, 212, 64], 30).is_none(), "no plain yellow left");
    }

    /// Lying on the ground: the carriage turns with the map's bearing and shortens with its pitch.
    #[test]
    fn icons_turn_with_the_map_and_lie_flat_when_tilted() {
        let Some(gpu) = gpu() else { return };
        let tilted = ViewState { bearing: 90.0, pitch: 60.0, ..VIEW };
        let trains = vec![train("n", TrainKind::HighSpeed, None, Some(0.0), [200.0, 100.0], &tilted)];
        let image = shoot(&gpu, tilted, trains_scene(&tilted, &trains, None));
        save(&image, "t2-train-icon-tilted.png");
        let (b, _) = coloured(&image, [400.0, 200.0], 60.0, over([226, 92, 196], image.pixel(4, 4)), 6).expect("the carriage");
        let (w, h) = (f64::from(b[2] - b[0] + 1), f64::from(b[3] - b[1] + 1));
        // north is to the left with the map turned 90°: the carriage lies across the screen, its
        // length kept (it runs across the view), its width foreshortened by cos 60°
        let px = 24.0 * SCALE / 64.0;
        assert!((w - 56.0 * px).abs() <= 4.0, "{w} x {h}");
        assert!(h < 26.0 * px * 0.5 + 4.0 && h > 26.0 * px * 0.5 - 4.0, "{w} x {h}");
    }

    /// Draws the atlas itself into `target/shots` to look at beside the canvas's, as a check of the
    /// mapping the layer reads (every cell non-empty is the atlas module's test).
    #[test]
    fn the_atlas_saved_for_a_look() {
        let drawn = atlas::draw();
        let image = Image { width: drawn.width, height: drawn.height, rgba: drawn.rgba.clone() };
        save(&image, "t2-train-atlas.png");
        for name in ["car-highSpeed", "box-op15"] {
            assert!(cell(name).is_some());
        }
    }
}
