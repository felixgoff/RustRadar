//! The sky behind the realistic map (item R1; doc 04 §3.6, doc 03 §12): today the map container's
//! CSS background (`skyCss`, `realistic.ts:189-207`), set by `updateLook` before every frame
//! (G:3661-3681); here the first thing the map pass draws on the realistic basemap, one triangle
//! over the whole target (`shaders/sky.wgsl`), under everything else.
//!
//! Each frame, for that frame's camera: the light ([`look::frame_look`]), the sky's inputs
//! (`scene::look::sky_view`: the elevation of the top of the screen, where the horizon is, the sun on
//! the screen when it is in front of the camera on the flat map, how much of space shows) and its
//! gradient (`scene::look::sky_gradient`: the top's and the horizon's colours as whole bytes, the
//! horizon at a whole percent, the sun's glow), drawn as CSS draws it ([`twin`]).
//!
//! This layer's [`Input`] is also what the realistic map's light is made from (the chosen or live
//! weather, the *daylight* setting): every realistic layer reads `SceneInput::sky` through
//! [`look::LightUniform`].
//!
//! Deviations: the sky is drawn into the map target (today the browser composites the transparent
//! canvas over the container's background: the same picture, as the layers blend over it with
//! deck's blending either way). The dark and satellite basemaps keep the clear colour, the page's
//! own `--ink` today.

use rustradar_core::geo::Basemap;
use rustradar_core::weather::Conditions;
use rustradar_scene::look::{SkyGradient, sky_gradient, sky_view};

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::look;
use crate::pipelines::fill::{uniform_group, uniform_layout};
use crate::pipelines::{Desc, build};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/sky.wgsl`.
pub const SHADER: &str = include_str!("../shaders/sky.wgsl");

/// What the realistic map is lit by (`SceneInput::sky`): the weather (`sky` in `Globe.svelte`:
/// the chosen preset, or the live weather's conditions; `None` until it has come, a fine day) and
/// the *daylight* setting (off: the fixed afternoon sun).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Input {
    /// Whether the sky and the realistic ground are drawn at all (on the realistic basemap). Off
    /// in a scene made with `SceneInput::new`, which has nothing on it; the app's scene builder
    /// always turns it on.
    pub shown: bool,
    pub weather: Option<Conditions>,
    pub daylight: bool,
}

/// `sky.wgsl`'s `Sky`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkyUniform {
    /// The top's colour (0–1); `[3]` unused.
    pub top: [f32; 4],
    /// The horizon's colour (0–1); `[3]`: where the horizon is, 0 (top) to 1 (bottom).
    pub horizon: [f32; 4],
    /// The glow's centre (CSS pixels), its alpha at 0 and 14 px, its alpha at 40 px.
    pub glow_at: [f32; 4],
    /// The glow's colour at 40 and 320 px (0–1); `[3]`: 1 when there is a glow.
    pub glow: [f32; 4],
}

impl From<&SkyGradient> for SkyUniform {
    fn from(g: &SkyGradient) -> Self {
        let c = |rgb: [u8; 3], a: f64| [f32::from(rgb[0]) / 255.0, f32::from(rgb[1]) / 255.0, f32::from(rgb[2]) / 255.0, a as f32];
        let (glow_at, glow) = match &g.glow {
            Some(glow) => ([glow.at[0] as f32, glow.at[1] as f32, glow.stops[0].alpha as f32, glow.stops[2].alpha as f32], c(glow.stops[2].rgb, 1.0)),
            None => ([0.0; 4], [0.0; 4]),
        };
        SkyUniform { top: c(g.top, 0.0), horizon: c(g.horizon, g.horizon_percent / 100.0), glow_at, glow }
    }
}

/// The sky of a frame: `look` (that frame's light) for the camera `view` on a canvas of `size` CSS
/// pixels (`updateLook`'s `skyCss` call, G:3662-3680).
pub fn gradient(look: &look::Look, view: &rustradar_scene::camera::ViewState, size: [f64; 2]) -> SkyGradient {
    let at = sky_view(view.zoom, view.pitch, view.bearing, look.sun_dir, size[0], size[1]);
    sky_gradient(look, at.top, at.horizon, at.sun_at, at.space)
}

/// `sky.wgsl` on the CPU: the colour (0–255, unrounded) of the CSS pixel whose centre is `xy` on a
/// canvas `height` CSS pixels high.
pub fn twin(g: &SkyGradient, height: f64, xy: [f64; 2]) -> [f64; 3] {
    let at = g.horizon_percent / 100.0 * height;
    let t = if at > 0.0 { (xy[1] / at).clamp(0.0, 1.0) } else { 1.0 };
    let mut rgb: [f64; 3] = std::array::from_fn(|i| f64::from(g.top[i]) + (f64::from(g.horizon[i]) - f64::from(g.top[i])) * t);
    if let Some(glow) = &g.glow {
        let r = (xy[0] - glow.at[0]).hypot(xy[1] - glow.at[1]);
        let stops = glow.stops;
        let premultiplied = |s: usize| (stops[s].rgb.map(|c| f64::from(c) * stops[s].alpha), stops[s].alpha);
        let between = stops.windows(2).position(|w| r >= w[0].radius && r < w[1].radius);
        let (premul, alpha) = match between {
            Some(i) => {
                let t = (r - stops[i].radius) / (stops[i + 1].radius - stops[i].radius);
                let ((a, aa), (b, ba)) = (premultiplied(i), premultiplied(i + 1));
                (std::array::from_fn::<f64, 3, _>(|k| a[k] + (b[k] - a[k]) * t), aa + (ba - aa) * t)
            }
            None => ([0.0; 3], 0.0),
        };
        rgb = std::array::from_fn(|k| premul[k] + rgb[k] * (1.0 - alpha));
    }
    rgb
}

/// The realistic sky.
pub struct Sky {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// Whether this frame draws it.
    visible: bool,
}

impl Sky {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let layout = uniform_layout(device, "sky", size_of::<SkyUniform>());
        let (buffer, group) = uniform_group(device, "sky", &layout, &SkyUniform::from(&SkyGradient {
            top: [0; 3],
            horizon: [0; 3],
            horizon_percent: 0.0,
            glow: None,
        }));
        let module = shaders::module(device, "sky", SHADER);
        let pipeline = build(
            init,
            &Desc {
                label: "sky",
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[],
                topology: wgpu::PrimitiveTopology::TriangleList,
                // over the whole target whatever its depth, writing none
                state: DrawState::LABEL,
                groups: &[&layout],
            },
        );
        Sky { pipeline, buffer, group, visible: false }
    }
}

impl Layer for Sky {
    fn name(&self) -> &'static str {
        "sky"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.visible = cx.scene.sky.shown && cx.scene.basemap == Basemap::Realistic;
        if !self.visible {
            return;
        }
        let look = look::frame_look(&cx.scene.sky, &cx.view.view, cx.view.size, cx.local_now_ms);
        let sky = gradient(&look, &cx.view.view, cx.view.size);
        cx.gpu.queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&SkyUniform::from(&sky)));
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if !self.visible || cx.pass != Pass::Map {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(2, &self.group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC: noon in Stockholm at midsummer (the reference pictures' clock).
    const MIDSUMMER_NOON: f64 = 1_782_036_000_000.0;
    /// 2026-12-21 22:00 UTC: night in Stockholm.
    const MIDWINTER_NIGHT: f64 = 1_797_890_400_000.0;
    /// 2026-06-21 18:30 UTC: the sun 8° up in the north-west over Stockholm (20:30 local).
    const MIDSUMMER_EVENING: f64 = 1_782_066_600_000.0;
    const SIZE: [f64; 2] = [1100.0, 720.0];

    /// Keeps a shot in `target/shots/` for review.
    fn save(image: &Image, name: impl AsRef<std::path::Path>) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// Stockholm close in at `pitch` and `bearing`.
    fn stockholm(pitch: f64, bearing: f64) -> ViewState {
        ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch, bearing, position: None }
    }

    #[test]
    fn the_uniform_is_the_shaders() {
        let names = ["top", "horizon", "glow_at", "glow"];
        let wgsl: Vec<(String, u32)> = names.iter().enumerate().map(|(i, n)| (n.to_string(), 16 * i as u32)).collect();
        assert_eq!(wgsl_members(SHADER, "Sky"), (64, wgsl));
        assert_eq!(size_of::<SkyUniform>(), 64);
    }

    /// The sky alone (the scene has no earth or ground on it), on the realistic basemap only.
    fn sky_shot(view: ViewState, now_ms: f64, input: Input, basemap: Basemap) -> Option<(Image, SkyGradient)> {
        let gpu = gpu()?;
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let mut scene = SceneInput::new(view, SIZE, 1.0, basemap);
        scene.local_now_ms = now_ms;
        scene.sky = input;
        let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
        let look = look::frame_look(&input, &view, SIZE, now_ms);
        Some((image, gradient(&look, &view, SIZE)))
    }

    /// Every pixel of `image` against [`twin`]: at most `tolerance` levels off (each channel).
    fn assert_twin(image: &Image, sky: &SkyGradient, tolerance: f64) {
        let mut worst = (0.0, [0, 0]);
        for y in (0..image.height).step_by(7) {
            for x in (0..image.width).step_by(13) {
                let expected = twin(sky, SIZE[1], [f64::from(x) + 0.5, f64::from(y) + 0.5]);
                let got = image.pixel(x, y);
                let off = (0..3).map(|i| (f64::from(got[i]) - expected[i]).abs()).fold(0.0, f64::max);
                if off > worst.0 {
                    worst = (off, [x, y]);
                }
            }
        }
        let [x, y] = worst.1;
        assert!(worst.0 <= tolerance, "{} levels off at {x},{y}: {:?} for {:?}", worst.0, image.pixel(x, y), twin(sky, SIZE[1], [f64::from(x) + 0.5, f64::from(y) + 0.5]));
    }

    /// R1's acceptance for the sky: tilted to 75° close in at noon, the top of the screen and the
    /// horizon (10 % down) in the colours of the gradient's stops within 4 levels, the gradient
    /// between, the horizon's colour below it.
    #[test]
    fn the_sky_is_the_gradient_of_its_stops() {
        let Some((image, sky)) = sky_shot(stockholm(75.0, 20.0), MIDSUMMER_NOON, Input { shown: true, weather: None, daylight: true }, Basemap::Realistic) else {
            return;
        };
        save(&image, "r1-sky-noon.png");
        assert_eq!(sky.horizon_percent, 10.0);
        assert!(sky.glow.is_none(), "the sun is behind the camera at noon looking north-north-east");
        let near = |got: [u8; 4], rgb: [u8; 3]| (0..3).all(|i| got[i].abs_diff(rgb[i]) <= 4);
        assert!(near(image.pixel(550, 0), sky.top), "top {:?} vs {:?}", image.pixel(550, 0), sky.top);
        assert!(near(image.pixel(550, 72), sky.horizon), "horizon {:?} vs {:?}", image.pixel(550, 72), sky.horizon);
        assert!(near(image.pixel(10, 700), sky.horizon), "below {:?} vs {:?}", image.pixel(10, 700), sky.horizon);
        // a clear midsummer noon: blue overhead, paler at the horizon
        assert!(sky.top[2] > sky.top[0] + 40 && sky.horizon[0] > sky.top[0], "{sky:?}");
        assert_twin(&image, &sky, 1.0);
    }

    /// In the evening, the camera tilted up as far as following a vehicle lets it (85°) and turned
    /// to the sun: its glow round where it is on the screen, over the gradient, as CSS composites
    /// the radial gradient (premultiplied stops) over the linear one.
    #[test]
    fn the_sun_glows_where_it_is_on_the_screen() {
        let Some((image, sky)) = sky_shot(stockholm(85.0, 305.0), MIDSUMMER_EVENING, Input { shown: true, weather: None, daylight: true }, Basemap::Realistic) else {
            return;
        };
        save(&image, "r1-sky-evening.png");
        let glow = sky.glow.expect("the sun is ahead");
        eprintln!("{sky:?}");
        assert!(glow.at[1] > 40.0 && glow.at[1] < 300.0 && glow.at[0] > 300.0 && glow.at[0] < 800.0, "{glow:?}");
        assert!(glow.stops[0].alpha > 0.3, "{glow:?}");
        assert_twin(&image, &sky, 1.0);
        // brighter than the gradient alone at its centre, the same past 320 px
        let plain = SkyGradient { glow: None, ..sky };
        let [x, y] = glow.at;
        let centre = image.pixel(x as u32, y as u32);
        let under = twin(&plain, SIZE[1], [x + 0.5, y + 0.5]);
        // (a warm white over a pale blue sky: redder, a little less blue)
        assert!(f64::from(centre[0]) - under[0] > 10.0, "{centre:?} over {under:?}");
        let far = [if x > 550.0 { 10.0 } else { 1090.0 }, 710.0];
        assert!(((x - far[0]).hypot(y - far[1])) > 320.0);
        let got = image.pixel(far[0] as u32, far[1] as u32);
        let expected = twin(&plain, SIZE[1], [far[0] + 0.5, far[1] + 0.5]);
        assert!((0..3).all(|i| (f64::from(got[i]) - expected[i]).abs() <= 1.0), "{got:?} vs {expected:?}");
    }

    /// A night `--now` gives a dark sky (taken brighter by the night's exposure, as today: the
    /// night's own colours times `adapt · 3.2`, a dark slate blue), well below the day's; far out
    /// space's colours, whatever the hour.
    #[test]
    fn the_night_sky_is_dark() {
        let Some((image, sky)) = sky_shot(stockholm(75.0, 20.0), MIDWINTER_NIGHT, Input { shown: true, weather: None, daylight: true }, Basemap::Realistic) else {
            return;
        };
        save(&image, "r1-sky-night.png");
        let day = gradient(&look::frame_look(&Input { shown: true, weather: None, daylight: true }, &stockholm(75.0, 20.0), SIZE, MIDSUMMER_NOON), &stockholm(75.0, 20.0), SIZE);
        eprintln!("night {sky:?}, day {day:?}");
        for (night, day) in [(sky.top, day.top), (sky.horizon, day.horizon)] {
            assert!((0..3).all(|i| night[i] < 120 && night[i] + 60 < day[i]), "night {night:?}, day {day:?}");
            assert!(night[2] > night[0], "blue: {night:?}");
        }
        let top = image.pixel(550, 0);
        assert!((0..3).all(|i| top[i].abs_diff(sky.top[i]) <= 4), "{top:?} vs {:?}", sky.top);
        assert_twin(&image, &sky, 1.0);
        // with daylight off the fixed afternoon sun lights the same night
        let Some((_, afternoon)) = sky_shot(stockholm(75.0, 20.0), MIDWINTER_NIGHT, Input { shown: true, weather: None, daylight: false }, Basemap::Realistic) else {
            return;
        };
        assert!(afternoon.top[2] > 100, "{afternoon:?}");
        // V2 is far out: space's colours, the horizon above the screen
        let v2 = ViewState { latitude: 52.0, longitude: 10.0, zoom: 4.5, pitch: 0.0, bearing: 0.0, position: None };
        let Some((image, sky)) = sky_shot(v2, MIDSUMMER_NOON, Input { shown: true, weather: Some(PRESETS.rain), daylight: true }, Basemap::Realistic) else {
            return;
        };
        assert_eq!((sky.top, sky.horizon, sky.horizon_percent), ([4, 7, 16], [12, 18, 34], 0.0));
        assert_eq!(image.pixel(0, 0)[..3], [12, 18, 34]);
        assert_eq!(image.pixel(1099, 719)[..3], [12, 18, 34]);
    }

    /// Off the realistic basemap nothing is drawn, nor where the scene has not asked for it: the
    /// clear colour stays.
    #[test]
    fn only_the_realistic_map_has_a_sky() {
        let shown = Input { shown: true, ..Input::default() };
        let Some((image, _)) = sky_shot(stockholm(75.0, 20.0), MIDSUMMER_NOON, shown, Basemap::Dark) else {
            return;
        };
        assert_eq!(image.pixel(550, 0)[..3], crate::target::CLEAR_RGB);
        // and a scene that has not asked for it has none (`SceneInput::new` has nothing on it)
        let Some((image, _)) = sky_shot(stockholm(75.0, 20.0), MIDSUMMER_NOON, Input::default(), Basemap::Realistic) else {
            return;
        };
        assert_eq!(image.pixel(550, 0)[..3], crate::target::CLEAR_RGB);
    }
}
