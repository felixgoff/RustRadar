//! `ground-land` (items R1, R2; doc 04 §1 row 3, §6.2; `realistic-ground.ts:925-945`, G:2654):
//! the land under the realistic ground's tiles, one polygon from −180° to 180° and ±85.06° in the
//! paint [`LAND`] (unmapped land, surface 0: Streets GL's terrain detail in its temperate biome
//! green), `SURFACE` (back faces culled, depth-tested against the earth sphere, no depth written),
//! drawn with the ground shader (`shaders/ground.wgsl`, `GroundExtension`). The sea and everything
//! else is drawn over it.
//!
//! Drawn on the realistic basemap when the scene asks for the realistic map (`SceneInput::sky`'s
//! `shown`, which the app's realistic scene builder sets) and for the earth and its land
//! (`SceneInput::earth`'s `shown`, as the dark map's land; so a test can draw the sky alone). The
//! rectangle is cut as deck cuts it on the globe (the earth layer's [`land_mesh`]: math.gl's 5°
//! grid, earcut per part), in one local Mercator frame for the world; on the flat map the world's
//! copies west and east are drawn too, as the dark map's land does.
//!
//! The shader's surface 0 (R2): the terrain texture (layer 22) tinted, a little unevenness over
//! tens of metres, the Earth picture's colour where that is dry and, far out (from 120 m a pixel),
//! the Earth picture's own colours, the night side's town lights, snow, wetness. The textures are
//! the renderer's ([`crate::layers::PrepareCx::textures`]); this layer does not start them (the
//! ground does, `ground::load_textures`), so a scene without the ground (the tests here) shows the
//! land with the stand-ins: white and flat, no Earth picture. [`lit_colour`] and [`lit_range`]
//! are its colour then, on the CPU (`ground::twin`).
//!
//! R1 drew it untextured, the tint alone (doc 04 §16.4's first step): against
//! `realistic-v3-1440x900.png` (V3 at the reference clock, its land where today's has no water)
//! that land averaged (124, 150, 123) where today's textured land averages (128, 145, 130).

use glam::DVec2;
use rustradar_core::geo::Basemap;
use rustradar_scene::camera::WORLD_SIZE;
use rustradar_scene::ground::LAND;
use rustradar_scene::tess::ground::explicit;

use super::twin::{self, Fragment};
use super::{GroundShader, GroundVertex, LineWidth};
use crate::frame::{Frame, FrameSlot, ViewFrame};
use crate::layers::earth::land_mesh;
use crate::layers::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::look::{self, Look, RLight};
use crate::pipelines::fill::Mesh;
use crate::state::DrawState;

/// The land's colour (0–255, unrounded) at the CSS pixel whose centre is `xy` in `view` lit by
/// `look`, as the ground shader works it out with the stand-in textures and no unevenness (exact
/// far out, where a pixel spans more than the grain), from deck's own frames ([`look::twin`]);
/// `None` where the pixel shows no ground (off the globe, above the flat map's horizon) or
/// outside ±85.06°.
pub fn lit_colour(look: &Look, view: &ViewFrame, xy: [f64; 2]) -> Option<[f64; 3]> {
    let rl = land_vary(look, view, xy)?;
    Some(twin::colour(look, &rl, &Fragment::stand_in(0, [LAND[0], LAND[1], LAND[2]])))
}

/// The range of the land's colour at `xy` as [`lit_colour`] has it, over the unevenness the GPU
/// may add there: `1 + 0.28 · grain · (noise − 0.5)` with the grain from how many of its cells a
/// pixel spans (`rl_cells(rl_t · 32)`, taken here from the neighbouring pixels' places, with a
/// margin of a quarter either way) and any noise: the low and the high colour.
pub fn lit_range(look: &Look, view: &ViewFrame, xy: [f64; 2]) -> Option<([f64; 3], [f64; 3])> {
    let rl = land_vary(look, view, xy)?;
    let right = land_vary(look, view, [xy[0] + 1.0, xy[1]])?;
    let below = land_vary(look, view, [xy[0], xy[1] + 1.0])?;
    let fwidth = (right.t - rl.t).abs() + (below.t - rl.t).abs();
    let cells = fwidth.x.max(fwidth.y);
    let smoothstep = |a: f64, b: f64, x: f64| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let grain = 1.0 - smoothstep(0.25, 1.0, 32.0 * cells * 0.75);
    let at = |factor: f64| twin::colour(look, &rl, &Fragment { grain_factor: factor, ..Fragment::stand_in(0, [LAND[0], LAND[1], LAND[2]]) });
    Some((at(1.0 - 0.14 * grain), at(1.0 + 0.14 * grain)))
}

/// The light's varyings where the pixel `xy` meets the land.
fn land_vary(look: &Look, view: &ViewFrame, xy: [f64; 2]) -> Option<look::twin::Vary> {
    if let Some(globe) = view.viewport.globe()
        && globe.ray_distance_to_globe_center_ratio(xy) >= 1.0
    {
        return None;
    }
    let [lon, lat] = view.viewport.unproject(xy, 0.0);
    if !(lon.is_finite() && lat.is_finite()) || lat.abs() > 85.06 {
        return None;
    }
    let light = RLight::new(look, view, 0.0);
    Some(look::twin::vary(&light, view, [lon, lat, 0.0]))
}

/// Row 3 of doc 04 §1: `ground-land` (realistic basemap).
pub struct GroundLand {
    shader: GroundShader,
    pipeline: wgpu::RenderPipeline,
    mesh: Mesh<GroundVertex>,
    /// This frame's frames: the world, and on the flat map its copies west and east.
    slots: Vec<FrameSlot>,
}

impl GroundLand {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let shader = GroundShader::new(init, "ground-land");
        let pipeline = shader.areas(init, "ground-land", DrawState::SURFACE);
        // the rectangle as the dark map's land has it, in the paint LAND
        let (vertices, indices) = land_mesh();
        let [r, g, b, surface] = explicit(LAND);
        let vertices: Vec<GroundVertex> = vertices.iter().map(|v| GroundVertex { position: v.position, tint: [r, g, b], surface }).collect();
        let mesh = Mesh::new(device, "ground-land", &vertices, &indices);
        GroundLand { shader, pipeline, mesh, slots: Vec::new() }
    }
}

/// `frame` moved `dx` world units east, its offset split again into hi + lo.
fn shifted(frame: &Frame, dx: f64) -> Frame {
    let x = f64::from(frame.offset_hi[0]) + f64::from(frame.offset_lo[0]) + dx;
    let hi = x as f32;
    Frame { offset_hi: [hi, frame.offset_hi[1]], offset_lo: [(x - f64::from(hi)) as f32, frame.offset_lo[1]], ..*frame }
}

impl Layer for GroundLand {
    fn name(&self) -> &'static str {
        "ground-land"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots.clear();
        if !cx.scene.sky.shown || !cx.scene.earth.shown || cx.scene.basemap != Basemap::Realistic {
            return;
        }
        self.shader.prepare(cx);
        let world = cx.view.local(DVec2::new(0.0, WORLD_SIZE), WORLD_SIZE, 0.0);
        if cx.view.is_globe() {
            self.slots.push(cx.frames.push(&world));
        } else {
            for copy in [-1.0, 0.0, 1.0] {
                self.slots.push(cx.frames.push(&shifted(&world, copy * WORLD_SIZE)));
            }
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if self.slots.is_empty() || cx.pass != Pass::Map {
            return;
        }
        self.shader.begin(pass, &self.pipeline, LineWidth::Hairline);
        for slot in &self.slots {
            self.shader.draw_areas(pass, cx, *slot, &self.mesh);
        }
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::ground::surface;

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{earth, sky};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 11:20 UTC: local noon at 10° E at midsummer (V2's centre).
    const V2_NOON: f64 = 1_782_040_800_000.0;
    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// 2026-06-21 22:00 UTC: night over Europe, day over the Pacific.
    const MIDSUMMER_NIGHT: f64 = 1_782_079_200_000.0;
    const SIZE: [f64; 2] = [1100.0, 720.0];

    /// Keeps a shot in `target/shots/` for review.
    fn save(image: &Image, name: impl AsRef<std::path::Path>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }
    /// V2 (plan section 8): Europe.
    const V2: ViewState = ViewState { latitude: 52.0, longitude: 10.0, zoom: 4.5, pitch: 0.0, bearing: 0.0, position: None };
    /// How far (levels, each channel) the GPU's land may be from [`lit_colour`] (R1's acceptance:
    /// 4), or outside [`lit_range`].
    const TOLERANCE: f64 = 4.0;

    /// The land is surface 0 of the ground's table in its paint, with the stand-ins until the
    /// ground loads the textures.
    #[test]
    fn the_land_is_surface_0_in_its_paint() {
        let land = surface(0).expect("unmapped land");
        assert_eq!((land.rough, land.snow, land.paved, land.layer()), (0.9, 1.0, false, 22.0));
        assert_eq!(LAND, [58, 81, 29, 255]);
        assert_eq!(explicit(LAND), [58, 81, 29, 0]);
        let rows = super::super::surface_rows();
        assert_eq!((rows[0], rows[1]), ([22.0, 76.0, 0.9, 1.0], [0.0, -1.0, -1.0, -1.0]));
    }

    /// The scene with the earth and the land on it (as the app's scene builder has it).
    fn land_shot(view: ViewState, now_ms: f64, input: sky::Input) -> Option<(Image, ViewFrame, Look)> {
        let gpu = gpu()?;
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = now_ms;
        scene.earth = earth::Input::SHOWN;
        scene.sky = input;
        let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        let look = look::frame_look(&input, &view, SIZE, now_ms);
        Some((image, ViewFrame::new(&view, SIZE, 1.0, 0.0), look))
    }

    /// Every sampled pixel of `image` whose whole neighbourhood is land against [`lit_colour`];
    /// how many were compared.
    fn compare(image: &Image, view: &ViewFrame, look: &Look, step: u32) -> usize {
        let mut worst = (0.0, [0, 0]);
        let mut compared = 0;
        for y in (step / 2..image.height).step_by(step as usize) {
            for x in (step / 2..image.width).step_by(step as usize) {
                let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                let at = |dx: f64, dy: f64| lit_colour(look, view, [xy[0] + dx, xy[1] + dy]);
                let Some((low, high)) = lit_range(look, view, xy) else { continue };
                // the GPU draws the globe in flat 5° facets, as deck does: towards the limb they
                // sag visibly below the sphere the twin finds the pixel on
                if view.viewport.globe().is_some_and(|g| g.ray_distance_to_globe_center_ratio(xy) > 0.9) {
                    continue;
                }
                // away from the globe's limb and the land's edges
                if [(-3.0, 0.0), (3.0, 0.0), (0.0, -3.0), (0.0, 3.0)].iter().any(|&(dx, dy)| at(dx, dy).is_none()) {
                    continue;
                }
                compared += 1;
                let got = image.pixel(x, y);
                // outside the range the unevenness may take it over
                let off = (0..3).map(|i| (low[i] - f64::from(got[i])).max(f64::from(got[i]) - high[i]).max(0.0)).fold(0.0, f64::max);
                if off > worst.0 {
                    worst = (off, [x, y]);
                }
            }
        }
        let [x, y] = worst.1;
        eprintln!("{compared} pixels, worst {:.2} levels at {x},{y}", worst.0);
        assert!(
            worst.0 <= TOLERANCE,
            "{} levels off at {x},{y}: {:?} for {:?}",
            worst.0,
            image.pixel(x, y),
            lit_range(look, view, [f64::from(x) + 0.5, f64::from(y) + 0.5])
        );
        compared
    }

    /// R1's acceptance, in the map: V2 on the realistic basemap with the sunny preset at local
    /// noon, the land within 4 levels of the CPU's light (`scene::look` through `look::twin`) at
    /// the centre and over the whole disc; the sky round the globe in space's colour.
    #[test]
    fn v2_land_at_noon_is_the_cpus_light() {
        let Some((image, view, look)) = land_shot(V2, V2_NOON, sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true }) else {
            return;
        };
        save(&image, "r1-land-v2-noon.png");
        let centre = lit_colour(&look, &view, [550.0, 360.0]).expect("land at the centre");
        let got = image.pixel(550, 360);
        eprintln!("V2 noon centre {got:?}, CPU {centre:?}");
        assert!((0..3).all(|i| (f64::from(got[i]) - centre[i]).abs() <= TOLERANCE), "{got:?} vs {centre:?}");
        // green land, lit
        assert!(got[1] > got[0] && got[1] > got[2] && got[1] > 100, "{got:?}");
        assert!(compare(&image, &view, &look, 23) > 600);
    }

    /// Far out each place has its own day: at V1 when it is night over Europe the land there is
    /// dark and the day side lit, each within 4 levels of the CPU's light; with *daylight* off the
    /// fixed afternoon sun lights it all.
    #[test]
    fn far_out_the_night_side_is_dark() {
        let v1 = ViewState { latitude: 35.0, longitude: 10.0, zoom: 1.9, pitch: 0.0, bearing: 0.0, position: None };
        let Some((image, view, look)) = land_shot(v1, MIDSUMMER_NIGHT, sky::Input { shown: true, weather: None, daylight: true }) else {
            return;
        };
        save(&image, "r1-land-v1-night.png");
        compare(&image, &view, &look, 19);
        let europe = image.pixel(550, 300);
        assert!(europe[..3].iter().all(|&c| c < 30), "night over Europe: {europe:?}");
        let Some((image, view, look)) = land_shot(v1, MIDSUMMER_NIGHT, sky::Input { shown: true, weather: None, daylight: false }) else {
            return;
        };
        compare(&image, &view, &look, 19);
        assert!(image.pixel(550, 300)[1] > 100, "{:?}", image.pixel(550, 300));
    }

    /// Close in on the flat map, tilted, in each weather: the land within 4 levels of the CPU's
    /// light from the near edge to far off, the snow white, the rain darker.
    #[test]
    fn close_in_the_land_takes_the_weather() {
        let v5 = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
        let mut greens = Vec::new();
        for (name, weather) in [("sunny", PRESETS.sunny), ("rain", PRESETS.rain), ("snow", PRESETS.snow), ("fog", PRESETS.fog)] {
            let Some((image, view, look)) = land_shot(v5, MIDSUMMER, sky::Input { shown: true, weather: Some(weather), daylight: true }) else {
                return;
            };
            save(&image, format!("r1-land-v5-{name}.png"));
            eprintln!("{name}: centre {:?}", image.pixel(550, 360));
            compare(&image, &view, &look, 29);
            greens.push(image.pixel(550, 500));
        }
        let [sunny, rain, snow, _] = greens[..] else { unreachable!() };
        assert!(rain[1] < sunny[1], "rain {rain:?} sunny {sunny:?}");
        assert!(snow[..3].iter().all(|&c| c > 150), "snow {snow:?}");
    }

    /// Close in at night (the sun 7° below the horizon at midwinter) the land is dim but there to
    /// see, as `lookFor` means it (the sky's little light taken brighter by the night's exposure),
    /// within 4 levels of the CPU's light, well below the day's.
    #[test]
    fn close_in_at_night_the_land_is_dim() {
        let v5 = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
        let input = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        let Some((image, view, look)) = land_shot(v5, 1_797_890_400_000.0, input) else {
            return;
        };
        save(&image, "r1-land-v5-night.png");
        assert!(look.night > 0.99 && look.sun_up < 0.0, "{look:?}");
        compare(&image, &view, &look, 29);
        let Some((day, _, _)) = land_shot(v5, MIDSUMMER, input) else {
            return;
        };
        let [night, day] = [image.pixel(550, 500), day.pixel(550, 500)];
        eprintln!("night {night:?}, day {day:?}");
        assert!(night[1] > 30 && night[1] + 30 < day[1], "night {night:?}, day {day:?}");
    }
}
