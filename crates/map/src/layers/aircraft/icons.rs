//! The aircraft drawn as silhouettes (item A4; doc 03 §6, G:2957-3025): rows 27 and 28 of doc 03
//! §3, `aircraft-icon-outlines` then `aircraft-icons`.
//!
//! The scene builder (item A3, [`rustradar_scene::aircraft`]) has already worked out every icon of
//! the refresh ([`AircraftScene::icons`]): the published models drawn top-down while
//! `useIcons` (pitch ≤ 5°, zoom < 11, models loaded), each with its model's name (its cell in both
//! atlases), its size in pixels (`screenPx(size) · 1.06`, and that times `outlineScale` for the
//! outline), its colour (selection blue or white) and its angle on screen
//! (`screenAngle(viewport, position, heading, 20 px · mpp) ?? bearing − heading`: the heading
//! sampled one 20 px step ahead, doc 02 §1.1). This part turns them into two deck IconLayers
//! drawn with [`crate::pipelines::icon`], both instanced over the icons in feed order:
//!
//! | Layer | Atlas | Size, px | Colour | Highlight |
//! |---|---|---|---|---|
//! | outlines (first, underneath) | `icons-outline.ktx2`, drawn only when **both** atlases are in | `outline_size_px` | [`OUTLINE_COLOR`] `[8,12,20,215]` | none |
//! | icons | `icons.ktx2`, drawn when it is in | `size_px` | `color`: `[55,142,240]` selected, else white | the hovered icon, [`HIGHLIGHT_COLOR`] `[55,142,240,150]` |
//!
//! Both are deck's IconLayer defaults otherwise ([`STYLE`]): sizes in pixels, billboards (facing
//! the screen, a constant size in pixels), `alphaCutoff` 0.05, and the `NO_CULL` draw state (depth
//! test `less-equal` and depth write on). The cells are white masks, so a fragment is the instance
//! colour with the atlas's alpha. Each icon stands at its aircraft's position at its exaggerated
//! altitude (not raised onto its wheels), so the outline and the icon of one aircraft lie at the
//! same depth and the icon, drawn second, passes over its outline, while neighbours at different
//! heights hide one another by depth rather than by draw order (doc 03 §6).
//!
//! While an atlas is still transcoding, or if it failed, the layers that need it are not drawn
//! (doc 03 §4.4); the aircraft layer is pending meanwhile, and the pool's wake brings the frame
//! that uploads it. A name with no cell in an atlas draws nothing (deck's `MISSING_ICON`); every
//! published model has one in both.
//!
//! Instances are rebuilt in each `prepare` from the scene and this frame's view (common offsets
//! measured from the scene refresh's centre, as every dynamic instance is: plan 1.4.2), and
//! written to the GPU only when they change, so between refreshes nothing is uploaded.
//!
//! Deviations: none visible. deck draws the highlighted icon by its picking module
//! (`highlightedObjectIndex`); here the hovered icon's instance carries the highlight, which the
//! pipeline blends as deck's picking module does. Picking itself is on the CPU (item A9).

use rustradar_scene::aircraft::{AircraftScene, HIGHLIGHT_COLOR, Icon, OUTLINE_COLOR};

use super::assets::{AircraftAssets, Atlas};
use crate::frame::ViewFrame;
use crate::layers::{DrawCx, Init, Pass, PrepareCx};
use crate::pipelines::icon::{Batch, IconDef, IconPipeline, IconStyle, Instance};
use crate::state::DrawState;

/// Both layers' props: deck's IconLayer defaults (`sizeUnits: "pixels"`, `billboard: true`,
/// `alphaCutoff` 0.05, no size clamp, opaque).
pub const STYLE: IconStyle = IconStyle::DEFAULT;

/// Both layers' draw state (`parameters: NO_CULL`).
pub const STATE: DrawState = DrawState::NO_CULL;

/// The outlines' instances (`aircraft-icon-outlines`): every icon of `icons` whose name `cell`
/// finds, at its position, `outline_size_px` big, turned by its angle, in [`OUTLINE_COLOR`].
pub fn outline_instances(view: &ViewFrame, icons: &[Icon], cell: impl Fn(&str) -> Option<IconDef>) -> Vec<Instance> {
    icons
        .iter()
        .filter_map(|icon| Some(Instance::at(view, icon.position, &cell(&icon.name)?, icon.outline_size_px, icon.angle, OUTLINE_COLOR)))
        .collect()
}

/// The silhouettes' instances (`aircraft-icons`): every icon of `icons` whose name `cell` finds, at
/// its position, `size_px` big, turned by its angle, in its colour; the one at `hovered` (an index
/// into `icons`, `iconIndex.get(hoveredId)`) highlighted [`HIGHLIGHT_COLOR`].
pub fn icon_instances(view: &ViewFrame, icons: &[Icon], hovered: Option<usize>, cell: impl Fn(&str) -> Option<IconDef>) -> Vec<Instance> {
    icons
        .iter()
        .enumerate()
        .filter_map(|(i, icon)| {
            let instance = Instance::at(view, icon.position, &cell(&icon.name)?, icon.size_px, icon.angle, icon.color);
            Some(if hovered == Some(i) { instance.with_highlight(HIGHLIGHT_COLOR) } else { instance })
        })
        .collect()
}

/// One of the two IconLayers: its batch once its atlas is in, what it last wrote, whether it draws
/// this frame.
struct IconLayer {
    label: &'static str,
    batch: Option<Batch>,
    written: Vec<Instance>,
    drawn: bool,
}

impl IconLayer {
    fn new(label: &'static str) -> IconLayer {
        IconLayer { label, batch: None, written: Vec::new(), drawn: false }
    }

    /// Draws `instances` from `atlas` this frame, or nothing when `shown` is false or the atlas is
    /// not in.
    fn prepare(&mut self, cx: &PrepareCx<'_>, pipeline: &IconPipeline, atlas: &Atlas, shown: bool, instances: impl FnOnce() -> Vec<Instance>) {
        self.drawn = false;
        let Some(texture) = atlas.texture().filter(|_| shown) else { return };
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        let batch = self.batch.get_or_insert_with(|| pipeline.batch(device, self.label, &texture.view, texture.size));
        batch.set_style(queue, &STYLE, cx.view);
        let instances = instances();
        if instances != self.written {
            batch.set_instances(device, queue, &instances);
            self.written = instances;
        }
        self.drawn = !batch.is_empty();
    }

    fn batch(&self) -> Option<&Batch> {
        self.batch.as_ref().filter(|_| self.drawn)
    }
}

/// Rows 27 and 28: the icon outlines, then the icons.
pub struct Icons {
    pipeline: IconPipeline,
    outlines: IconLayer,
    icons: IconLayer,
}

impl Icons {
    pub fn new(init: &Init<'_>) -> Self {
        Icons {
            pipeline: IconPipeline::new(init, "aircraft icons", STATE),
            outlines: IconLayer::new("aircraft-icon-outlines"),
            icons: IconLayer::new("aircraft-icons"),
        }
    }

    /// Once a frame, after [`AircraftAssets::poll`]: this frame's outlines and icons of `scene`.
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, assets: &AircraftAssets, scene: &AircraftScene) {
        let any = !scene.icons.is_empty();
        let (icons, outline) = (assets.icons(), assets.outline());
        let view = cx.view;
        // the border needs both atlases, the silhouettes only their own
        let both = any && icons.texture().is_some();
        self.outlines.prepare(cx, &self.pipeline, outline, both, || outline_instances(view, &scene.icons, |name| outline.icon(name)));
        self.icons.prepare(cx, &self.pipeline, icons, any, || icon_instances(view, &scene.icons, scene.hovered_icon, |name| icons.icon(name)));
    }

    /// The outlines, then the icons (map pass only).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map {
            return;
        }
        let batches: Vec<&Batch> = [self.outlines.batch(), self.icons.batch()].into_iter().flatten().collect();
        if batches.is_empty() {
            return;
        }
        self.pipeline.begin(pass);
        for batch in batches {
            self.pipeline.draw_batch(pass, batch);
        }
    }

    /// How many outlines and icons this frame draws.
    pub fn drawn(&self) -> [usize; 2] {
        [&self.outlines, &self.icons].map(|layer| layer.batch().map_or(0, |b| b.len() as usize))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use rustradar_core::geo::Basemap;
    use rustradar_core::liveries::DEFAULT_LIVERY;
    use rustradar_core::model::{LiveFlight, f32_as_js};
    use rustradar_core::motion::aircraft::{Phase, Pose};
    use rustradar_scene::aircraft::{AircraftBuilder, ICON_FILL, Inputs};
    use rustradar_scene::aircraft_assets::{AircraftModels, IconCell};
    use rustradar_scene::camera::{ViewState, viewport_for};

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::Layer;
    use crate::layers::aircraft::assets::published;
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 12:00 UTC: daylight over Stockholm.
    const NOON_MS: f64 = 1_782_043_200_000.0;
    /// The canvas, CSS px, drawn at twice its size.
    const SIZE: [f64; 2] = [520.0, 260.0];
    const SCALE: f64 = 2.0;
    /// Stockholm just short of zoom 11: the largest icons drawn (the size floor is 24 px).
    const VIEW: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 10.5, bearing: 0.0, pitch: 0.0, position: None };
    /// The aircraft of [`row`]: id, ICAO type and icon class, heading, and where on the canvas.
    const ROW: [(u32, &str, &str, i32, [f64; 2]); 5] = [
        (1, "A388", "A380", 0, [60.0, 130.0]),
        (2, "A388", "A380", 90, [160.0, 130.0]),
        (3, "A388", "A380", 270, [260.0, 130.0]),
        (4, "A20N", "A320", 0, [360.0, 130.0]),
        (5, "A20N", "A320", 0, [460.0, 130.0]),
    ];
    const SELECTED: u32 = 4;
    const HOVERED: u32 = 5;

    /// Five aircraft on the ground in a row across the canvas, each where [`ROW`] puts it.
    fn row() -> Vec<LiveFlight> {
        let viewport = viewport_for(&VIEW, SIZE);
        ROW.iter()
            .map(|&(id, typecode, icon, track, xy)| {
                let [lon, lat] = viewport.unproject(xy, 0.0);
                LiveFlight {
                    id,
                    lat: lat as f32,
                    lon: lon as f32,
                    track,
                    on_ground: true,
                    callsign: format!("TST{id}"),
                    typecode: typecode.into(),
                    icon: icon.into(),
                    ..Default::default()
                }
            })
            .collect()
    }

    /// The reported position as the pose, wheels down.
    fn pose(f: &LiveFlight) -> Pose {
        Pose {
            lon: f32_as_js(f.lon),
            lat: f32_as_js(f.lat),
            alt_ft: f64::from(f.alt),
            heading: f64::from(f.track),
            track: f64::from(f.track),
            pitch: 0.0,
            bank: 0.0,
            gear: 1.0,
            phase: Phase::Parked,
        }
    }

    /// The row's aircraft as the scene builder makes them: all five as icons, the fourth selected
    /// and the fifth hovered.
    fn scene() -> AircraftScene {
        let inputs = Inputs {
            view: VIEW,
            size: SIZE,
            exaggeration: 1.0,
            show_aircraft: true,
            labelling: false,
            selected: Some(SELECTED),
            follow: false,
            hovered: Some(HOVERED),
        };
        let scene = AircraftBuilder::new().build(&inputs, published(), &row(), pose, |_| DEFAULT_LIVERY);
        assert_eq!(scene.icons.len(), ROW.len(), "all drawn as icons");
        assert_eq!(scene.mesh_count(), 0);
        scene
    }

    fn cell(c: &IconCell) -> IconDef {
        IconDef::cell(f64::from(c.x), f64::from(c.y), f64::from(c.width), f64::from(c.height), c.mask)
    }

    #[test]
    fn the_outlines_lie_under_the_icons_at_their_size_angle_and_colour() {
        let models = published().unwrap();
        let view = ViewFrame::new(&VIEW, SIZE, SCALE, 0.0);
        let mut scene = scene();
        let outlines = outline_instances(&view, &scene.icons, |name| models.outline_mapping.get(name).map(cell));
        let icons = icon_instances(&view, &scene.icons, scene.hovered_icon, |name| models.icon_mapping.get(name).map(cell));
        assert_eq!((outlines.len(), icons.len()), (ROW.len(), ROW.len()));
        for ((icon, outline), drawn) in scene.icons.iter().zip(&outlines).zip(&icons) {
            let at = view.common_offset([icon.position[0], icon.position[1]], icon.position[2]);
            assert_eq!(outline.position, drawn.position, "{}: the border under its icon", icon.id);
            assert_eq!(drawn.position, <[f32; 3]>::from(at));
            assert_eq!((outline.angle, drawn.angle), (icon.angle as f32, icon.angle as f32));
            assert_eq!(drawn.size, icon.size_px as f32);
            assert_eq!(outline.size, icon.outline_size_px as f32);
            // `screenPx(size) · 1.06` and that times `outlineScale` (152 / 128)
            assert!((f64::from(outline.size) / f64::from(drawn.size) - 1.1875).abs() < 1e-6);
            assert!((icon.size_px / ICON_FILL - scene.frame.screen_px(icon.size_m)).abs() < 1e-9);
            assert_eq!(outline.color, OUTLINE_COLOR);
            assert_eq!(outline.highlight, Instance::NO_HIGHLIGHT, "the border is never highlighted");
            assert_eq!(drawn.color, if icon.id == SELECTED { [55, 142, 240, 255] } else { [255; 4] }, "{}", icon.id);
            assert_eq!(drawn.highlight, if icon.id == HOVERED { [55, 142, 240, 150] } else { [0; 4] }, "{}", icon.id);
            assert_eq!((outline.color_mode, drawn.color_mode), (1.0, 1.0), "white masks tinted by the colour");
            let ([x, y, w, h], c) = (drawn.frame, models.icon_mapping[&icon.name]);
            assert_eq!([x, y, w, h], [c.x, c.y, c.width, c.height].map(|v| v as f32));
            assert_eq!(drawn.offset, [0.0, 0.0], "anchored at the middle");
        }
        // headings 0, 90 and 270 at the middle of an untilted, unturned view: `bearing − heading`
        let angles: Vec<f64> = scene.icons.iter().map(|i| i.angle).collect();
        for (angle, expected) in angles.iter().zip([0.0, -90.0, -270.0, 0.0, 0.0]) {
            // the same turn; meridians lean by a tenth of a degree 200 px out
            assert!(((angle - expected).rem_euclid(360.0) + 180.0).rem_euclid(360.0) - 180.0 < 0.15, "{angles:?}");
        }
        // a name with no cell draws nothing (deck's MISSING_ICON), the others stay as they were
        scene.icons[1].name = "no such model".into();
        scene.hovered_icon = Some(4);
        let outlines = outline_instances(&view, &scene.icons, |name| models.outline_mapping.get(name).map(cell));
        let icons = icon_instances(&view, &scene.icons, scene.hovered_icon, |name| models.icon_mapping.get(name).map(cell));
        assert_eq!((outlines.len(), icons.len()), (4, 4));
        assert_eq!(icons[3].highlight, [55, 142, 240, 150], "the hovered one is still the fifth icon");
        assert_eq!(icons.iter().filter(|i| i.highlight[3] > 0).count(), 1);
    }

    /// The row drawn by the map's own aircraft layer.
    fn shoot(gpu: &Gpu, aircraft: AircraftScene) -> Image {
        let mut scene = SceneInput::new(VIEW, SIZE, SCALE, Basemap::Dark);
        scene.feed_now_ms = NOON_MS;
        scene.local_now_ms = NOON_MS;
        scene.aircraft.scene = aircraft;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let image = render_with(gpu, &mut renderer, &scene, SCALE).expect("rendered");
        assert!(!renderer.pending(), "the atlases are in");
        image
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// A lit pixel: an icon's, not the land's or a border's.
    fn lit(p: [u8; 4]) -> bool {
        p[0].max(p[1]).max(p[2]) >= 150
    }

    /// The lit pixels within `r` device px of `[x, y]`: their bounding box `[x0, y0, x1, y1]`
    /// (inclusive), their centroid (pixel centres), how many.
    fn blob(image: &Image, [x, y]: [f64; 2], r: f64) -> Option<([u32; 4], [f64; 2], usize)> {
        let (mut b, mut sum, mut n): (Option<[u32; 4]>, [f64; 2], usize) = (None, [0.0; 2], 0);
        for py in (y - r).max(0.0) as u32..((y + r) as u32).min(image.height) {
            for px in (x - r).max(0.0) as u32..((x + r) as u32).min(image.width) {
                if lit(image.pixel(px, py)) {
                    let e = b.get_or_insert([px, py, px, py]);
                    *e = [e[0].min(px), e[1].min(py), e[2].max(px), e[3].max(py)];
                    sum = [sum[0] + f64::from(px) + 0.5, sum[1] + f64::from(py) + 0.5];
                    n += 1;
                }
            }
        }
        b.map(|b| (b, [sum[0] / n as f64, sum[1] / n as f64], n))
    }

    /// The 4-connected groups of lit pixels in the whole image.
    fn lit_groups(image: &Image) -> usize {
        let (w, h) = (image.width as usize, image.height as usize);
        let mut seen = vec![false; w * h];
        let mut groups = 0;
        for start in 0..w * h {
            if seen[start] || !lit(image.pixel((start % w) as u32, (start / w) as u32)) {
                continue;
            }
            groups += 1;
            seen[start] = true;
            let mut stack = vec![start];
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                let near = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
                for j in near.into_iter().flatten() {
                    if !seen[j] && lit(image.pixel((j % w) as u32, (j / w) as u32)) {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        groups
    }

    fn close(a: [u8; 4], b: [u8; 3], tolerance: u8) -> bool {
        (0..3).all(|c| a[c].abs_diff(b[c]) <= tolerance)
    }

    /// A published model's span and length, metres (the index's: the silhouettes are drawn from the
    /// detailed mesh).
    fn span_and_length(name: &str) -> [f64; 2] {
        let model = published().unwrap().get(name).unwrap();
        [model.span, model.length]
    }

    /// The row as the map draws it: white silhouettes the size `screenPx` gives them, turned to their
    /// headings (nose up at 0°, right at 90°, left at 270°), each inside its navy border, the
    /// selected one in selection blue, the hovered one highlighted as deck highlights it.
    #[test]
    fn silhouettes_are_white_turned_to_their_headings_and_bordered_in_navy() {
        let Some(gpu) = gpu() else { return };
        let scene = scene();
        let image = shoot(&gpu, scene.clone());
        save(&image, "a4-icons-row.png");
        let land = image.pixel(4, 4);
        assert!(!lit(land) && land[0] >= 20, "the map behind them: {land:?}");
        assert_eq!(lit_groups(&image), ROW.len(), "one silhouette each, nothing else lit");

        let viewport = viewport_for(&VIEW, SIZE);
        let blobs: Vec<_> = scene
            .icons
            .iter()
            .map(|icon| {
                let [x, y, _] = viewport.project(icon.position);
                let centre = [x * SCALE, y * SCALE];
                let found = blob(&image, centre, 90.0).unwrap_or_else(|| panic!("{}: drawn", icon.id));
                (icon, centre, found)
            })
            .collect();
        let mut aft = [0.0; 3];
        for (k, (icon, centre, ([x0, y0, x1, y1], centroid, _))) in blobs.iter().enumerate() {
            let middle = [f64::from(x0 + x1 + 1) / 2.0, f64::from(y0 + y1 + 1) / 2.0];
            assert!((middle[0] - centre[0]).abs() < 3.0 && (middle[1] - centre[1]).abs() < 3.0, "{}: centred on its aircraft ({middle:?} for {centre:?})", icon.id);
            // the silhouette spans `screenPx` along the model's largest dimension (94 % of the cell)
            let px = icon.size_px / ICON_FILL * SCALE;
            let [span, length] = span_and_length(&icon.name).map(|m| m / icon.size_m * px);
            let (across, along) = (f64::from(x1 - x0 + 1), f64::from(y1 - y0 + 1));
            let heading = ROW[k].3;
            let (wide, tall) = if heading % 180 == 0 { (span, length) } else { (length, span) };
            eprintln!("{}: {across} × {along} px drawn, {wide:.1} × {tall:.1} expected; centroid {centroid:?}, middle {middle:?}", icon.id);
            assert!((across - wide).abs() <= 3.0 && (along - tall).abs() <= 3.0, "{}: {across} × {along} px for {wide:.1} × {tall:.1}", icon.id);
            if k < 3 {
                // where the silhouette's weight lies along its length, from the middle toward the tail
                aft[k] = match heading {
                    0 => centroid[1] - middle[1],
                    90 => middle[0] - centroid[0],
                    _ => centroid[0] - middle[0],
                };
            }
        }
        // the A380's wings and tailplane put its weight behind the middle: with the nose up that is
        // down the screen, with the nose right it is left, with the nose left it is right
        eprintln!("A380 weight aft of the middle at headings 0, 90, 270: {aft:?} px");
        assert!(aft[0] > 1.0, "{aft:?}");
        assert!(aft.iter().all(|a| (a - aft[0]).abs() < 1.0), "turned the way its heading says: {aft:?}");

        // colours: white, selection blue, and white highlighted toward blue at alpha 150/255
        let at = |k: usize| {
            let (_, _, (_, [x, y], _)) = blobs[k];
            image.pixel(x as u32, y as u32)
        };
        for k in 0..3 {
            assert!(close(at(k), [255, 255, 255], 2), "{:?}", at(k));
        }
        assert!(close(at(3), [55, 142, 240], 2), "selected: {:?}", at(3));
        let t = 150.0 / 255.0;
        let highlighted = [55.0, 142.0, 240.0].map(|h: f64| (255.0 + (h - 255.0) * t).round() as u8);
        assert!(close(at(4), highlighted, 3), "hovered: {:?} for {highlighted:?}", at(4));

        // the border: [8,12,20] at 215/255 over the land, a few pixels beyond each silhouette's nose
        let border = [8.0, 12.0, 20.0].map(|c: f64| (f64::from(land[0]) * (1.0 - 215.0 / 255.0) + c * 215.0 / 255.0).round() as u8);
        for (icon, _, ([x0, y0, x1, y1], _, _)) in &blobs {
            let heading = ROW.iter().find(|r| r.0 == icon.id).unwrap().3;
            // the nose's pixel: the lit pixel furthest along the heading
            let lit_at = |x: u32, y: u32| lit(image.pixel(x, y));
            let (nose, step): ((u32, u32), (i32, i32)) = match heading {
                0 => ((*x0..=*x1).map(|x| (x, *y0)).find(|&(x, y)| lit_at(x, y)).unwrap(), (0, -1)),
                90 => ((*y0..=*y1).map(|y| (*x1, y)).find(|&(x, y)| lit_at(x, y)).unwrap(), (1, 0)),
                _ => ((*y0..=*y1).map(|y| (*x0, y)).find(|&(x, y)| lit_at(x, y)).unwrap(), (-1, 0)),
            };
            // 9 cell texels wide, its last one antialiased
            let ring = 9.0 * icon.size_px * SCALE / 128.0;
            for d in 2..=(ring.floor() as i32 - 1).max(2) {
                let (x, y) = ((nose.0 as i32 + step.0 * d) as u32, (nose.1 as i32 + step.1 * d) as u32);
                let p = image.pixel(x, y);
                assert!(close(p, border, 4), "{}: {d} px past the nose {p:?}, the border is {border:?}", icon.id);
            }
        }
    }

    /// A test layer: [`Icons`] with assets of its own, drawing `scene` (the map's own aircraft
    /// layer has nothing to draw).
    struct Only {
        assets: AircraftAssets,
        icons: Icons,
        scene: AircraftScene,
        drawn: Rc<Cell<[usize; 2]>>,
    }

    impl Layer for Only {
        fn name(&self) -> &'static str {
            "a4 icons only"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.assets.poll(cx.gpu);
            self.icons.prepare(cx, &self.assets, &self.scene);
            self.drawn.set(self.icons.drawn());
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.icons.draw(pass, cx);
        }

        fn pending(&self) -> bool {
            self.assets.pending()
        }
    }

    /// The row drawn by an [`Only`] layer whose assets are `models` (`None`: they did not load).
    fn shoot_only(gpu: &Gpu, models: Option<&AircraftModels>) -> (Image, [usize; 2]) {
        let mut input = SceneInput::new(VIEW, SIZE, SCALE, Basemap::Dark);
        input.feed_now_ms = NOON_MS;
        input.local_now_ms = NOON_MS;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let drawn = Rc::new(Cell::new([usize::MAX; 2]));
        let init = renderer.init(gpu);
        let layer = Only { assets: AircraftAssets::with_models(&init, models), icons: Icons::new(&init), scene: scene(), drawn: drawn.clone() };
        renderer.add_layer(Box::new(layer));
        let image = render_with(gpu, &mut renderer, &input, SCALE).expect("rendered");
        assert!(!renderer.pending());
        (image, drawn.get())
    }

    /// The layers need their atlases: with none (the models did not load) nothing is drawn and
    /// nothing waits; without the border's atlas the silhouettes are drawn alone (doc 03 §4.4: the
    /// outline layer needs both).
    #[test]
    fn each_layer_waits_for_its_atlases() {
        let Some(gpu) = gpu() else { return };
        let (image, drawn) = shoot_only(&gpu, None);
        assert_eq!(drawn, [0, 0]);
        assert_eq!(lit_groups(&image), 0);

        let mut no_border = published().unwrap().clone();
        no_border.outline_atlas = None;
        no_border.outline_mapping.clear();
        let (image, drawn) = shoot_only(&gpu, Some(&no_border));
        assert_eq!(drawn, [0, ROW.len()]);
        assert_eq!(lit_groups(&image), ROW.len());
        let land = image.pixel(4, 4);
        let dark = (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y))).filter(|&(x, y)| u16::from(image.pixel(x, y)[0]) + 8 < u16::from(land[0])).count();
        assert_eq!(dark, 0, "no border under them");

        let (image, drawn) = shoot_only(&gpu, published());
        assert_eq!(drawn, [ROW.len(), ROW.len()]);
        assert_eq!(lit_groups(&image), ROW.len());
    }
}
