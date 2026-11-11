//! Ships: the selection halo, 3D models from zoom 12.5, icons, the selection ring (doc 02 §4): rows
//! 9–11 of doc 03 §3 (doc 02 §2, rows 6–8), drawn in that order.
//!
//! The icons (item H2; doc 02 §4.2–4.5, G:1813-1876): the scene builder hands over each refresh's
//! [`ShipsScene`] ([`rustradar_scene::ships::ShipBuilder`]) in [`Input::scene`]: every ship below
//! zoom 12.5, and close in the ships not drawn as models, each already placed where it is drawn,
//! sized, turned and coloured. This layer draws them as deck's IconLayer `ship-icons` with
//! [`crate::pipelines::icon`]:
//! - the atlas of [`rustradar_scene::atlas::ships`], drawn once on the CPU when the layer is made
//!   and uploaded as RGBA8 with a full mip chain (so it is in from the first frame; deck's
//!   `shipAtlas()` is drawn as synchronously);
//! - full-colour cells (`mask: false`): the atlas's type colours, faded by the instance's alpha
//!   (235, 105 for an old position);
//! - billboards facing the screen ([`STYLE`], deck's defaults: `billboard: true`,
//!   `sizeUnits: "pixels"`), the cell's height in pixels, turned anticlockwise by the angle the
//!   scene worked out on the screen;
//! - the [`DrawState::SHIP`] state (`SHIP_PARAMETERS`: no cull, no depth write, depth test on, so
//!   the globe hides the ships on its far side);
//! - the ship hovered in the sidebar's list highlighted [`ICON_HIGHLIGHT`] as deck's picking module
//!   blends it.
//!
//! Instances are rebuilt in each `prepare` from the scene and this frame's view (common offsets
//! from the frame's centre, plan 1.4.2) and written to the GPU only when they change.
//!
//! The other rows are other items': the halo under the ships and the ring over them (H4), the
//! models before the icons close in (H3, from [`ShipsScene::models`]).
//!
//! Stage 2 stub (E4): keep `new(&Init) -> Self` and the row in `layers::ordered`.

use rustradar_scene::atlas::ships as atlas;
use rustradar_scene::ships::{ICON_HIGHLIGHT, ShipIcon, ShipsScene};

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::frame::ViewFrame;
use crate::pipelines::bitmap::upload_rgba8;
use crate::pipelines::icon::{Batch, IconDef, IconPipeline, IconStyle, Instance};
use crate::state::DrawState;

/// What the scene builder hands this layer (`SceneInput`'s field of the same name).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// This scene refresh's ships (item H2): their icons, and close in the ships drawn as models;
    /// empty without ships.
    pub scene: ShipsScene,
}

/// `ship-icons`' props: deck's IconLayer defaults (`billboard: true`, `sizeUnits: "pixels"`).
pub const STYLE: IconStyle = IconStyle::DEFAULT;

/// `ship-icons`' draw state (`SHIP_PARAMETERS`).
pub const STATE: DrawState = DrawState::SHIP;

/// The atlas cell of the icon `name`, as the icon pipeline takes it (`mask: false`); none for a
/// name the atlas lacks (deck's `MISSING_ICON` draws nothing).
pub fn cell(name: &str) -> Option<IconDef> {
    let c = atlas::cell(name)?;
    Some(IconDef::cell(f64::from(c.x), f64::from(c.y), f64::from(c.width), f64::from(c.height), false))
}

/// The instances of `ship-icons`: every icon of `icons` with a cell, at its position, `size_px`
/// tall, turned by its angle, in its colour, the highlighted one with [`ICON_HIGHLIGHT`].
pub fn icon_instances(view: &ViewFrame, icons: &[ShipIcon]) -> Vec<Instance> {
    icons
        .iter()
        .filter_map(|icon| {
            let instance = Instance::at(view, icon.position, &cell(&icon.name)?, icon.size_px, icon.angle, icon.color);
            Some(if icon.highlighted { instance.with_highlight(ICON_HIGHLIGHT) } else { instance })
        })
        .collect()
}

/// Row 10's icons: `ship-icons`.
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
        let texture = upload_rgba8(init.gpu, "ship icon atlas", size, &drawn.rgba, true);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let pipeline = IconPipeline::new(init, "ship icons", STATE);
        let batch = pipeline.batch(&init.gpu.device, "ship-icons", &view, size);
        Icons { pipeline, _atlas: texture, batch, written: Vec::new() }
    }

    fn prepare(&mut self, cx: &PrepareCx<'_>, icons: &[ShipIcon]) {
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

/// Rows 9–11: ship halo, ships, ship ring.
pub struct Ships {
    icons: Icons,
}

impl Ships {
    pub fn new(init: &Init<'_>) -> Self {
        Ships { icons: Icons::new(init) }
    }

    /// How many ship icons this frame draws.
    pub fn icons_drawn(&self) -> usize {
        self.icons.len()
    }
}

impl Layer for Ships {
    fn name(&self) -> &'static str {
        "ships"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let scene = &cx.scene.ships.scene;
        self.icons.prepare(cx, &scene.icons);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map {
            return;
        }
        // the selection halo (H4), then the models (H3) go here, under the icons
        self.icons.draw(pass);
        // the selection ring (H4) goes here, over them
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_core::model::Ship;
    use rustradar_scene::camera::{ViewState, viewport_for};
    use rustradar_scene::ships::ShipBuilder;

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;
    const SIZE: [f64; 2] = [400.0, 200.0];
    const SCALE: f64 = 2.0;
    /// Gothenburg harbour at zoom 11: icons at their full size, on the globe.
    const VIEW: ViewState = ViewState { longitude: 11.93, latitude: 57.70, zoom: 11.0, bearing: 0.0, pitch: 0.0, position: None };

    /// A ship of `kind` and `size` heading `heading`, standing at `xy` on the canvas of `view`.
    fn ship(mmsi: u32, kind: u8, size: u8, heading: Option<f32>, xy: [f64; 2], view: &ViewState) -> Ship {
        let [lon, lat] = viewport_for(view, SIZE).unproject(xy, 0.0);
        Ship { mmsi, lat, lon, name: format!("SHIP {mmsi}"), kind, size, heading, ..Ship::default() }
    }

    /// A row of ships across the canvas: a large cargo ship north, a medium tanker east, a small
    /// passenger ship south-west, a moored fishing boat, a hovered tug north.
    fn row(view: &ViewState) -> Vec<Ship> {
        vec![
            ship(1, 4, 2, Some(0.0), [60.0, 100.0], view),
            ship(2, 6, 1, Some(90.0), [130.0, 100.0], view),
            ship(3, 3, 0, Some(225.0), [200.0, 100.0], view),
            ship(4, 5, 1, None, [270.0, 100.0], view),
            ship(5, 2, 1, Some(0.0), [340.0, 100.0], view),
        ]
    }

    fn ships_scene(view: &ViewState, ships: &[Ship], hovered: Option<u32>) -> ShipsScene {
        ShipBuilder::new().build(ships, true, view, SIZE, hovered, NOW)
    }

    fn shoot(gpu: &Gpu, view: ViewState, ships: ShipsScene) -> Image {
        let mut scene = SceneInput::new(view, SIZE, SCALE, Basemap::Dark);
        scene.local_now_ms = NOW as f64;
        scene.feed_now_ms = NOW as f64;
        scene.ships.scene = ships;
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

    /// `rgb` at `alpha` over the map pixel `ground`.
    fn over(rgb: [u8; 3], alpha: u8, ground: [u8; 4]) -> [u8; 3] {
        let a = f64::from(alpha) / 255.0;
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

    /// The bounding box of everything drawn within `r` device px of `[x, y]` (pixels off the
    /// ground colour `ground`).
    fn drawn_box(image: &Image, [x, y]: [f64; 2], r: f64, ground: [u8; 4]) -> [u32; 4] {
        let mut b: Option<[u32; 4]> = None;
        for py in (y - r).max(0.0) as u32..((y + r) as u32).min(image.height) {
            for px in (x - r).max(0.0) as u32..((x + r) as u32).min(image.width) {
                if !close(image.pixel(px, py), [ground[0], ground[1], ground[2]], 8) {
                    let e = b.get_or_insert([px, py, px, py]);
                    *e = [e[0].min(px), e[1].min(py), e[2].max(px), e[3].max(py)];
                }
            }
        }
        b.expect("drawn")
    }

    #[test]
    fn instances_take_the_icons_cells_sizes_angles_and_colours() {
        let view = ViewFrame::new(&VIEW, SIZE, SCALE, 0.0);
        let ships = row(&VIEW);
        let scene = ships_scene(&VIEW, &ships, Some(5));
        let instances = icon_instances(&view, &scene.icons);
        assert_eq!(instances.len(), 5);
        let names: Vec<&str> = scene.icons.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["hull-4", "hull-6", "hull-3", "dot-5", "hull-2"]);
        for (icon, drawn) in scene.icons.iter().zip(&instances) {
            assert_eq!(drawn.position, <[f32; 3]>::from(view.common_offset([icon.position[0], icon.position[1]], 0.0)));
            assert_eq!((drawn.size, drawn.angle), (icon.size_px as f32, icon.angle as f32));
            assert_eq!(drawn.color_mode, 0.0, "full colour, not a mask");
            assert_eq!(drawn.offset, [0.0, 0.0], "anchored at the middle");
            let c = atlas::cell(&icon.name).unwrap();
            assert_eq!(drawn.frame, [c.x, c.y, c.width, c.height].map(|v| v as f32));
        }
        // the heading anticlockwise, up to the globe's lean 100 px off the centre; 0 for the dot
        let angles: Vec<f32> = instances.iter().map(|i| i.angle).collect();
        for (angle, want) in angles.iter().zip([0.0, -90.0, -225.0 + 360.0, 0.0, 0.0]) {
            let d = (f64::from(*angle) - want + 540.0).rem_euclid(360.0) - 180.0;
            assert!(d.abs() < 0.5, "{angles:?}");
        }
        assert_eq!(instances.iter().map(|i| i.size).collect::<Vec<_>>(), [22.0, 18.0, 15.0, 18.0 * 0.62, 18.0]);
        assert_eq!(instances[1].color, [218, 92, 92, 235]);
        assert_eq!(instances.iter().map(|i| i.highlight).collect::<Vec<_>>()[3..], [[0; 4], ICON_HIGHLIGHT]);
        // a name without a cell draws nothing
        let mut icons = scene.icons.clone();
        icons[0].name = "hull-1".into();
        assert_eq!(icon_instances(&view, &icons).len(), 4);
    }

    /// The row as the map draws it: each icon in its type colour, as tall as its size, its hull
    /// pointing the way it goes, the dot without a heading, the hovered one blended toward the
    /// selection blue.
    #[test]
    fn icons_stand_in_their_type_colours_turned_to_their_headings() {
        let Some(gpu) = gpu() else { return };
        let ships = row(&VIEW);
        let image = shoot(&gpu, VIEW, ships_scene(&VIEW, &ships, Some(5)));
        save(&image, "h2-ship-icons-row.png");
        let at = |xy: [f64; 2]| [xy[0] * SCALE, xy[1] * SCALE];
        let ground = image.pixel(4, 4);
        let (green, red, blue, pink) =
            (over([116, 188, 112], 235, ground), over([218, 92, 92], 235, ground), over([98, 152, 240], 235, ground), over([236, 128, 156], 235, ground));
        // the large cargo ship north: green, its cell 22 px tall: the hull and its border (55 + 5
        // of the 64 texels long, 30 + 5 wide) stand out from the map that size
        let (_, n) = coloured(&image, at([60.0, 100.0]), 40.0, green, 6).expect("the green hull");
        assert!(n > 200, "{n}");
        let [x, y] = at([60.0, 100.0]);
        let b = drawn_box(&image, [x, y], 40.0, ground);
        let (w, h) = (f64::from(b[2] - b[0] + 1), f64::from(b[3] - b[1] + 1));
        let px = 22.0 * SCALE / 64.0;
        assert!((h - 60.0 * px).abs() <= 2.0, "{w} x {h}");
        assert!((w - 35.0 * px).abs() <= 2.0, "{w} x {h}");
        // the bow up: the hull is narrower at its top than at its stern
        let wide_at = |y: u32| (b[0]..=b[2]).filter(|&x| close(image.pixel(x, y), green, 6)).count();
        assert!(wide_at(b[1] + 4) < wide_at(b[3] - 4), "bow north: {} vs {}", wide_at(b[1] + 4), wide_at(b[3] - 4));
        // the cell's middle is the ship's place: the stern is further from it than the bow tip
        // (the hull spans texels 4 to 59 of 64, centred 0.5 texel low)
        assert!((f64::from(b[0] + b[2] + 1) / 2.0 - x).abs() <= 1.0, "{b:?}");
        assert!((f64::from(b[1] + b[3] + 1) / 2.0 - y).abs() <= 1.5, "{b:?}");
        // the tanker heading east: red, lying across, its bow to the right
        let (b, _) = coloured(&image, at([130.0, 100.0]), 40.0, red, 6).expect("the red hull");
        assert!(b[2] - b[0] > b[3] - b[1] + 6, "east: wider than tall {b:?}");
        let tall_at = |x: u32| (b[1]..=b[3]).filter(|&y| close(image.pixel(x, y), red, 6)).count();
        assert!(tall_at(b[2] - 2) < tall_at(b[0] + 2), "bow east: {} vs {}", tall_at(b[2] - 2), tall_at(b[0] + 2));
        // the small passenger ship south-west: its bow at the bottom left
        let (b, n) = coloured(&image, at([200.0, 100.0]), 40.0, blue, 6).expect("the blue hull");
        let (cx, cy) = (f64::from(b[0] + b[2]) / 2.0, f64::from(b[1] + b[3]) / 2.0);
        let [sx, sy] = at([200.0, 100.0]);
        let mass = (b[0]..=b[2])
            .flat_map(|x| (b[1]..=b[3]).map(move |y| (x, y)))
            .filter(|&(x, y)| close(image.pixel(x, y), blue, 6))
            .fold((0.0, 0.0), |(mx, my), (x, y)| (mx + f64::from(x) - sx, my + f64::from(y) - sy));
        // the hull's stern is broad, its bow pointed: its colour leans toward the stern (north-east)
        assert!(mass.0 / n as f64 > 0.3 && mass.1 / (n as f64) < -0.3, "{mass:?} over {n} at {cx},{cy}");
        // the moored fishing boat: a pink dot of 40/64 of 0.62 · 18 px across, round
        let (b, _) = coloured(&image, at([270.0, 100.0]), 40.0, pink, 6).expect("the dot");
        let side = 40.0 * 18.0 * 0.62 * SCALE / 64.0;
        assert!((f64::from(b[2] - b[0] + 1) - side).abs() <= 2.0 && (f64::from(b[3] - b[1] + 1) - side).abs() <= 2.0, "{b:?} vs {side}");
        // the hovered tug: its teal blended toward the blue by 200/255, 235/255 opaque
        let (a, ah) = (235.0 / 255.0, 200.0 / 255.0);
        let t = ah / (ah + a * (1.0 - ah));
        let mix = |c: f64, h: f64| c + (h - c) * t;
        let want = [mix(72.0, 55.0), mix(198.0, 142.0), mix(194.0, 240.0)];
        let centre = image.pixel((340.0 * SCALE) as u32, (100.0 * SCALE) as u32);
        let alpha = ah + a * (1.0 - ah);
        for c in 0..3 {
            let expected = want[c] * alpha + f64::from(ground[c]) * (1.0 - alpha);
            assert!((f64::from(centre[c]) - expected).abs() <= 6.0, "{centre:?} vs {expected} on {c}");
        }
        assert!(coloured(&image, at([340.0, 100.0]), 40.0, over([72, 198, 194], 235, ground), 6).is_none(), "no plain teal left");
    }

    /// An old position is drawn faded: alpha 105, not 235.
    #[test]
    fn an_old_position_is_faded() {
        let Some(gpu) = gpu() else { return };
        let mut ships = vec![ship(1, 6, 2, Some(0.0), [100.0, 100.0], &VIEW), ship(2, 6, 2, Some(0.0), [300.0, 100.0], &VIEW)];
        ships[1].old = true;
        let image = shoot(&gpu, VIEW, ships_scene(&VIEW, &ships, None));
        save(&image, "h2-ship-icons-old.png");
        let ground = image.pixel(4, 4);
        let fresh = image.pixel(200, 200);
        let old = image.pixel(600, 200);
        for c in 0..3 {
            let want = over([218, 92, 92], 235, ground)[c];
            assert!(fresh[c].abs_diff(want) <= 3, "{fresh:?}");
            let want = over([218, 92, 92], 105, ground)[c];
            assert!(old[c].abs_diff(want) <= 3, "{old:?} vs {want}");
        }
    }

    /// Billboards: tilted and turned, a hull keeps its size on the screen and points along its
    /// heading as the camera sees it (east with the map turned 90° is up the screen).
    #[test]
    fn icons_face_the_screen_when_tilted() {
        let Some(gpu) = gpu() else { return };
        let tilted = ViewState { bearing: 90.0, pitch: 60.0, ..VIEW };
        let ships = vec![ship(1, 4, 2, Some(90.0), [200.0, 100.0], &tilted)];
        let scene = ships_scene(&tilted, &ships, None);
        assert!(scene.icons[0].angle.abs() < 0.5, "{}", scene.icons[0].angle);
        let image = shoot(&gpu, tilted, scene);
        save(&image, "h2-ship-icon-tilted.png");
        let ground = image.pixel(4, 4);
        let b = drawn_box(&image, [400.0, 200.0], 60.0, ground);
        let (w, h) = (f64::from(b[2] - b[0] + 1), f64::from(b[3] - b[1] + 1));
        // upright and not foreshortened: as tall and wide as at no tilt
        let px = 22.0 * SCALE / 64.0;
        assert!((h - 60.0 * px).abs() <= 2.0 && (w - 35.0 * px).abs() <= 2.0, "{w} x {h}");
        let green = over([116, 188, 112], 235, ground);
        let wide_at = |y: u32| (b[0]..=b[2]).filter(|&x| close(image.pixel(x, y), green, 6)).count();
        assert!(wide_at(b[1] + 4) < wide_at(b[3] - 4), "bow up the screen");
    }

    /// Draws the atlas itself into `target/shots` to look at beside the canvas's, as a check of the
    /// mapping the layer reads (every cell non-empty is the atlas module's test).
    #[test]
    fn the_atlas_saved_for_a_look() {
        let drawn = atlas::draw();
        let image = Image { width: drawn.width, height: drawn.height, rgba: drawn.rgba.clone() };
        save(&image, "h2-ship-atlas.png");
        for name in ["hull-0", "dot-8"] {
            assert!(cell(name).is_some());
        }
    }
}
