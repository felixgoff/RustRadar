//! `ground-land` (item R1; doc 04 §1 row 3, §6.2; `realistic-ground.ts:925-945`, G:2654): the land
//! under the realistic ground's tiles, one polygon from −180° to 180° and ±85.06° in the paint
//! [`LAND`] (unmapped land, surface 0, in Streets GL's temperate biome green), `SURFACE` (back
//! faces culled, depth-tested against the earth sphere, no depth written), lit by the realistic
//! light (`lib/rlight.wgsl`). The sea and everything else is drawn over it.
//!
//! Drawn on the realistic basemap when the scene asks for the realistic map (`SceneInput::sky`'s
//! `shown`, which the app's realistic scene builder sets) and for the earth and its land
//! (`SceneInput::earth`'s `shown`, as the dark map's land; so a test can draw the sky alone). The
//! rectangle is cut as deck cuts it on the globe (the earth layer's [`land_mesh`]: math.gl's 5°
//! grid, earcut per part), in one local Mercator frame for the world; on the flat map the world's
//! copies west and east are drawn too, as the dark map's land does.
//!
//! **Untextured until R2.** This is doc 04 §16.4's first step, "flat untextured ground": the
//! ground shader's surface-0 branch (§6.8) with its albedo the paint's tint alone, where R2's
//! `ground.wgsl` multiplies the terrain detail texture in and, far out, takes the Earth picture's
//! colour (`far`, the dry land's hue) and adds the night side's town lights (R10). What it keeps of
//! that branch: the tint through `rl_linear`, snow lying on it (`rlight.snow · GS_SNOW[0]`), the wet
//! darkening (`rlight.wet · 0.35`, unpaved), roughness `GS_ROUGH[0]`, the normal straight up, the
//! clouds' shadows, `rl_shade`, `rl_aerial` and `rl_display`: what [`lit_colour`] works out on the
//! CPU. R2 replaces this file's shader with its own and keeps the layer (or folds it in). Against
//! `realistic-v3-1440x900.png` (V3 at the reference clock, its land where today's has no water)
//! the untextured land averages (124, 150, 123) where today's textured land averages
//! (128, 145, 130): a little greener, the terrain texture's own grey missing.

use glam::DVec2;
use rustradar_core::geo::Basemap;
use rustradar_scene::camera::WORLD_SIZE;
use rustradar_scene::ground::LAND;

use crate::frame::{Frame, FrameSlot, ViewFrame};
use crate::layers::earth::land_mesh;
use crate::layers::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::look::{self, LightUniform, Look, RLight};
use crate::pipelines::fill::{self, Mesh};
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;

/// The land's shader: the projection, the light's vertex part, and the ground shader's branch for
/// surface 0 without its textures (module notes). Validated with the others by this file's test.
pub const SHADER: &str = r#"
//!include lib/rlight.wgsl

// surface 0 of the ground's table (`GS_ROUGH`, `GS_SNOW`, `GS_PAVED`)
const ROUGH: f32 = 0.9;
const SNOW: f32 = 1.0;
const PAVED: f32 = 0.0;

struct LandOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) eye: vec3<f32>,
    @location(1) sun: vec3<f32>,
    @location(2) t: vec2<f32>,
    @location(3) c: vec2<f32>,
    @location(4) tint: vec3<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> LandOut {
    let p = project_local(position);
    let rl = rl_vertex(p.rel, rl_mercator(local_offset(position.xy)));
    return LandOut(p.clip, rl.eye, rl.sun, rl.t, rl.c, color.rgb);
}

@fragment
fn fs(land: LandOut) -> @location(0) vec4<f32> {
    let rl = RlVary(land.eye, land.sun, land.t, land.c);
    var albedo = rl_linear(land.tint);
    // snow lies on the land; rain darkens it (not paved: a third as much); `far` is R2's
    let snow = rlight.snow * SNOW;
    albedo = mix(albedo, vec3<f32>(0.82, 0.84, 0.88) * mix(0.75, 1.0, SNOW), snow);
    let wet = rlight.wet * mix(0.35, 1.0, PAVED);
    albedo *= 1.0 - 0.45 * wet;
    let rough = mix(ROUGH, 0.12, wet * PAVED * (1.0 - rlight.snow));
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let lit = rl_shade(rl, albedo, n, normalize(rl.eye), rough, 0.0, 1.0, rl_sun_visible(rl), 1.0);
    return vec4<f32>(rl_display(rl_aerial(rl, lit)), 1.0);
}
"#;

/// The land's colour (0–255, unrounded) at the CSS pixel whose centre is `xy` in `view` lit by
/// `look`, as [`SHADER`] works it out, from deck's own frames ([`look::twin`]); `None` where the
/// pixel shows no ground (off the globe, above the flat map's horizon) or outside ±85.06°.
pub fn lit_colour(look: &Look, view: &ViewFrame, xy: [f64; 2]) -> Option<[f64; 3]> {
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
    let rl = look::twin::vary(&light, view, [lon, lat, 0.0]);
    let tint = glam::DVec3::new(f64::from(LAND[0]), f64::from(LAND[1]), f64::from(LAND[2])) / 255.0;
    let (snow, paved, rough) = (1.0, 0.0, 0.9);
    let mut albedo = look::twin::linear(tint);
    albedo = albedo.lerp(glam::DVec3::new(0.82, 0.84, 0.88) * (0.75 + 0.25 * snow), look.snow * snow);
    let wet = look.wet * (0.35 + 0.65 * paved);
    albedo *= 1.0 - 0.45 * wet;
    let n = glam::DVec3::Z;
    // (no clouds' shadows in the looks this twin is used with: `cloud_shadow` 0)
    let lit = look::twin::shade(look, &rl, albedo, n, rl.eye.normalize(), rough, 0.0, 1.0, 1.0, 1.0);
    Some(look::twin::display(look, look::twin::aerial(look, &rl, lit)))
}

/// Row 3 of doc 04 §1: `ground-land` (realistic basemap).
pub struct GroundLand {
    pipeline: wgpu::RenderPipeline,
    light: LightUniform,
    mesh: Mesh<fill::Vertex>,
    /// This frame's frames: the world, and on the flat map its copies west and east.
    slots: Vec<FrameSlot>,
}

impl GroundLand {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let light = LightUniform::new(device);
        let module = shaders::module(device, "ground-land", SHADER);
        let pipeline = build(
            init,
            &Desc {
                label: "ground-land",
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(fill::Vertex::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state: DrawState::SURFACE,
                groups: &[light.layout()],
            },
        );
        // the rectangle as the dark map's land has it, in the paint LAND
        let (mut vertices, indices) = land_mesh();
        for v in &mut vertices {
            v.color = LAND;
        }
        let mesh = Mesh::new(device, "ground-land", &vertices, &indices);
        GroundLand { pipeline, light, mesh, slots: Vec::new() }
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
        self.light.prepare(cx);
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
        pass.set_pipeline(&self.pipeline);
        self.light.bind(pass);
        for slot in &self.slots {
            cx.bind_frame(pass, *slot);
            self.mesh.draw(pass);
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
    /// 4).
    const TOLERANCE: f64 = 4.0;

    /// The shader parses and validates (lib/rlight.wgsl with it), and its constants are surface 0's.
    #[test]
    fn the_shader_validates_and_holds_surface_0() {
        crate::shaders::tests::validate("ground-land", &shaders::expand(SHADER).expect("libraries known"));
        let land = surface(0).expect("unmapped land");
        assert_eq!((land.rough, land.snow, land.paved), (0.9, 1.0, false));
        assert!(SHADER.contains("const ROUGH: f32 = 0.9;") && SHADER.contains("const SNOW: f32 = 1.0;") && SHADER.contains("const PAVED: f32 = 0.0;"));
        assert_eq!(LAND, [58, 81, 29, 255]);
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
                let Some(expected) = at(0.0, 0.0) else { continue };
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
                let off = (0..3).map(|i| (f64::from(got[i]) - expected[i]).abs()).fold(0.0, f64::max);
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
            lit_colour(look, view, [f64::from(x) + 0.5, f64::from(y) + 0.5])
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
