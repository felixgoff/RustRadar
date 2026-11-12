//! The realistic map's light on the GPU (item R1; doc 04 §1's per-frame driver, §3, §4).
//!
//! - **The driver** ([`frame_look`]; `Globe.svelte`'s `updateLook`, G:3647-3682, doc 03 §12): before
//!   every frame, for the camera of that frame and the local clock, the sun the map is lit by
//!   (`sceneSun(daylight, localSun(sunDirection(now), lat, lon))`), the weather under the clouds
//!   (`mixConditions(sky ?? sunny, sunny, smoothstep(3 km, 10 km, camera altitude))`) and the
//!   light, `lookFor(weather, sun, 1.5 · height · metresPerPixel)` (`scene::look::look_for`). It
//!   runs in the layers' `prepare`, not in the scene refresh, so the light keeps up with the camera
//!   between refreshes as today's `onBeforeRender` does. What it is made from is the scene's
//!   [`crate::layers::sky::Input`] (the chosen or live weather and the *daylight* setting).
//! - **The uniform** ([`RLight`]): doc 04 §4.1's `rlightUniforms` block, 192 bytes, `lib/rlight.wgsl`'s
//!   `RLight`, made by [`RLight::new`] from the look and the frame's view (`rlightProps`,
//!   `realistic.ts:493-554`); [`LightUniform`] holds it on the GPU as bind group 2 of every realistic
//!   pipeline ([`RLIGHT_GROUP`]) and writes it each frame ([`LightUniform::prepare`]).
//! - **The CPU twin** ([`twin`]): the vertex part and the fragment library of `lib/rlight.wgsl` in
//!   f64, deck's frames as they are (common space, not the port's render frame), for the tests and
//!   for colours the CPU must know (a pixel's reference).
//!
//! Deviations, each with its reason:
//! - `origin` and `cloud_origin` are the view centre's place within the texture and cloud periods
//!   for every draw, not each layer's common origin: `lib/rlight.wgsl` measures `rl_m` from the view
//!   centre on the flat map too (one uniform a frame serves every layer; precise near the centre,
//!   where it matters).
//! - `sun_globe` is the sun in the globe's render frame (`lib/project.wgsl`: common space turned by
//!   `Rx(φc)·Rz(−λc)`), which is what common space is on the GPU here. On the flat map it is the
//!   sun in common space, as deck's (unused there).
//! - `time` is the renderer's clock (seconds since its first frame), modulo 100 000 s as today's
//!   `performance.now()` is.
//! - `eye` and `cover` are for the terrain's cover passes (stage 9): 0 here.

use std::f64::consts::PI;

use bytemuck::{Pod, Zeroable};
use glam::{DMat3, DVec2, DVec3};
use rustradar_core::geo::meters_per_pixel;
use rustradar_core::num::clamp01;
use rustradar_core::sun::{local_sun, sun_direction};
use rustradar_core::weather::{Conditions, PRESETS, mix_conditions};
use rustradar_scene::camera::{ViewState, Viewport, lng_lat_to_world};
pub use rustradar_scene::look::{Look, display_rgb, from_local, look_for, scene_sun};

use crate::frame::{RENDER_FRAME_MAX_LATITUDE, ViewFrame};
use crate::layers::{PrepareCx, sky};
use crate::pipelines::fill::{uniform_group, uniform_layout};

/// The bind group of [`RLight`] in every realistic pipeline (after the globals and the frame).
pub const RLIGHT_GROUP: u32 = 2;

/// The camera sits this many viewport heights from its target (`CAMERA_HEIGHTS`, G:3072).
pub const CAMERA_HEIGHTS: f64 = 1.5;
/// The clouds' height, metres (`CLOUD_HEIGHT_M`, `clouds.ts:22`).
pub const CLOUD_HEIGHT_M: f64 = 10_000.0;
/// The weather's own look only under the clouds: from here up to them it gives way to the sun's
/// (`WEATHER_UP_TO_M = CLOUD_HEIGHT_M · 0.3`, G:3607).
pub const WEATHER_UP_TO_M: f64 = CLOUD_HEIGHT_M * 0.3;
/// The textures' period in common units (`PERIOD`, `realistic.ts`): 1/64 of a unit, about 1223 m ·
/// cos φ.
pub const PERIOD: f64 = 1.0 / 64.0;
/// The clouds' shadows' period in common units (`CLOUD_PERIOD`): a quarter of a unit.
pub const CLOUD_PERIOD: f64 = 0.25;
/// The Earth's circumference that `periodMetres` is worked out with (`realistic.ts:542`).
const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
/// `time` wraps here, seconds (`(performance.now() / 1000) % 100_000`).
pub const TIME_WRAP_S: f64 = 100_000.0;

const DEG: f64 = PI / 180.0;

/// `Globe.svelte`'s `smoothstep(a, b, x)`.
fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = clamp01((x - a) / (b - a));
    t * t * (3.0 - 2.0 * t)
}

/// How far the camera is from the middle of the view, metres: `CAMERA_HEIGHTS · height ·
/// metresPerPixel(zoom, latitude)` (`height` CSS pixels; G:3658).
pub fn camera_metres(view: &ViewState, height: f64) -> f64 {
    CAMERA_HEIGHTS * height * meters_per_pixel(view.zoom, view.latitude)
}

/// The camera's height above the ground, metres, from how far it is from the middle of the view
/// and its tilt (`cameraAltitude`, G:3604-3605).
pub fn camera_altitude(view: &ViewState, height: f64) -> f64 {
    camera_metres(view, height) * (view.pitch * DEG).cos()
}

/// The weather where the camera is: the sky's own (`None`: none yet, a fine day) under the
/// clouds, a fine day above them, mixed between 3 and 10 km up (G:3656).
pub fn weather_here(sky: Option<Conditions>, altitude: f64) -> Conditions {
    mix_conditions(sky.unwrap_or(PRESETS.sunny), PRESETS.sunny, smoothstep(WEATHER_UP_TO_M, CLOUD_HEIGHT_M, altitude))
}

/// The sun the realistic map is lit by at `now_ms` (local clock) over `view`'s centre, as east,
/// north, up: the real one with *daylight* on, else the fixed afternoon one (G:3654).
pub fn sun_at(daylight: bool, now_ms: f64, view: &ViewState) -> [f64; 3] {
    scene_sun(daylight, local_sun(sun_direction(now_ms), view.latitude, view.longitude))
}

/// The light of a frame (`updateLook`'s `setLook`, G:3654-3658): `input`'s weather and daylight,
/// the camera `view` on a canvas of `size` CSS pixels, the local clock `now_ms`.
pub fn frame_look(input: &sky::Input, view: &ViewState, size: [f64; 2], now_ms: f64) -> Look {
    let sun = sun_at(input.daylight, now_ms, view);
    let weather = weather_here(input.weather, camera_altitude(view, size[1]));
    look_for(&weather, sun, camera_metres(view, size[1]))
}

/// `((v % p) + p) % p`.
fn modulo(v: f64, p: f64) -> f64 {
    ((v % p) + p) % p
}

/// The world Mercator point `lib/rlight.wgsl` measures `rl_m` from: the view centre, as
/// `frame::ViewFrame` measures local offsets from it (on the globe the centre's longitude and
/// latitude within ±89.9°, on the flat map deck's centre).
pub fn light_origin(view: &ViewFrame) -> DVec2 {
    match &view.viewport {
        Viewport::Globe(globe) => {
            let latitude = globe.latitude.clamp(-RENDER_FRAME_MAX_LATITUDE, RENDER_FRAME_MAX_LATITUDE);
            DVec2::from_array(lng_lat_to_world([globe.longitude, latitude]))
        }
        Viewport::Mercator(flat) => DVec2::new(flat.center.x, flat.center.y),
    }
}

/// `lib/rlight.wgsl`'s `RLight`: doc 04 §4.1's block, member for member (std140-compatible; the
/// layout test compares the offsets).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct RLight {
    pub sun_dir: [f32; 3],
    pub sun_up: f32,
    pub sun_color: [f32; 3],
    pub ambient: f32,
    pub sky_zenith: [f32; 3],
    pub exposure: f32,
    pub sky_horizon: [f32; 3],
    pub haze_density: f32,
    pub haze_color: [f32; 3],
    pub wet: f32,
    pub ground_color: [f32; 3],
    pub snow: f32,
    pub sun_globe: [f32; 3],
    pub haze_start: f32,
    pub origin: [f32; 2],
    pub period: f32,
    pub period_metres: f32,
    pub cloud_origin: [f32; 2],
    pub cloud_period: f32,
    pub cloud_shadow: f32,
    pub anchor: [f32; 2],
    pub night: f32,
    pub time: f32,
    pub globe: f32,
    pub cover: f32,
    pub pad0: [f32; 2],
    pub eye: [f32; 3],
    pub pad1: f32,
}

fn f3(v: [f64; 3]) -> [f32; 3] {
    v.map(|c| c as f32)
}

impl RLight {
    /// The light's props for a frame (`rlightProps`): `look`, and where the frame `view` is, for
    /// what lies fixed to the world; `time_s` the renderer's clock in seconds.
    pub fn new(look: &Look, view: &ViewFrame, time_s: f64) -> Self {
        let base = &*view.viewport;
        // `Math.fround(viewport.longitude)`, `Math.fround(viewport.latitude)`
        let (lng, lat) = (f64::from(base.longitude as f32), f64::from(base.latitude as f32));
        let at = light_origin(view);
        // towards the sun in the globe's common space (x, y and z are the Earth's y, −x and z)
        let sun = from_local(look.sun_dir, lat, lng);
        let common = DVec3::new(sun[1], -sun[0], sun[2]);
        let sun_globe = match &view.viewport {
            // turned as lib/project.wgsl's render frame is (frame::ViewFrame)
            Viewport::Globe(globe) => {
                let latitude = globe.latitude.clamp(-RENDER_FRAME_MAX_LATITUDE, RENDER_FRAME_MAX_LATITUDE);
                DMat3::from_rotation_x(latitude * DEG) * DMat3::from_rotation_z(-globe.longitude * DEG) * common
            }
            Viewport::Mercator(_) => common,
        };
        RLight {
            sun_dir: f3(look.sun_dir),
            sun_up: look.sun_up as f32,
            sun_color: f3(look.sun_color),
            ambient: look.ambient as f32,
            sky_zenith: f3(look.sky_zenith),
            exposure: look.exposure as f32,
            sky_horizon: f3(look.sky_horizon),
            haze_density: look.haze_density as f32,
            haze_color: f3(look.haze_color),
            wet: look.wet as f32,
            ground_color: f3(look.ground_color),
            snow: look.snow as f32,
            sun_globe: f3(sun_globe.to_array()),
            haze_start: look.haze_start as f32,
            origin: [modulo(at.x, PERIOD) as f32, modulo(at.y, PERIOD) as f32],
            period: PERIOD as f32,
            period_metres: (PERIOD * EARTH_CIRCUMFERENCE_M * (lat * DEG).cos() / 512.0) as f32,
            cloud_origin: [modulo(at.x, CLOUD_PERIOD) as f32, modulo(at.y, CLOUD_PERIOD) as f32],
            cloud_period: CLOUD_PERIOD as f32,
            cloud_shadow: look.cloud_shadow as f32,
            anchor: [lng as f32, lat as f32],
            night: look.night as f32,
            time: modulo(time_s, TIME_WRAP_S) as f32,
            globe: look.globe as f32,
            cover: 0.0,
            pad0: [0.0; 2],
            eye: [0.0; 3],
            pad1: 0.0,
        }
    }
}

/// [`RLight`] on the GPU: a realistic layer's bind group 2, written each frame. Each layer keeps
/// its own (its pipeline is made with [`Self::layout`]); every one holds the same values.
pub struct LightUniform {
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl LightUniform {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = uniform_layout(device, "rlight", size_of::<RLight>());
        let (buffer, group) = uniform_group(device, "rlight", &layout, &RLight::zeroed());
        LightUniform { layout, buffer, group }
    }

    /// The layout of group 2 for the layer's pipelines.
    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// This frame's light ([`frame_look`] for the frame's camera and the local clock), written; the
    /// look is returned for what the layer works out on the CPU.
    pub fn prepare(&self, cx: &PrepareCx<'_>) -> Look {
        let look = frame_look(&cx.scene.sky, &cx.view.view, cx.view.size, cx.local_now_ms);
        let light = RLight::new(&look, cx.view, f64::from(cx.view.globals.viewport[3]));
        cx.gpu.queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&light));
        look
    }

    /// Binds it as group 2.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(RLIGHT_GROUP, &self.group, &[]);
    }
}

/// `lib/rlight.wgsl` in f64, as the GLSL has it: the varyings at a point worked out in deck's own
/// frames ([`vary`]), and the fragment library over them. Colours are linear, 0–1, until
/// [`display`].
pub mod twin {
    use glam::{DVec2, DVec3};

    use super::{DEG, Look, RLight, modulo};
    use crate::frame::ViewFrame;
    use rustradar_scene::camera::Viewport;

    /// deck's varyings `rl_eye`, `rl_sun`, `rl_t`, `rl_c`.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Vary {
        pub eye: DVec3,
        pub sun: DVec3,
        pub t: DVec2,
        pub c: DVec2,
    }

    /// `rl_frame`: east, north and up at `p` (deck's globe common space).
    fn enu(p: DVec3) -> [DVec3; 3] {
        let up = p.normalize();
        let east = (DVec3::new(-up.y, up.x, 0.0) + DVec3::new(1e-9, 0.0, 0.0)).normalize();
        [east, up.cross(east), up]
    }

    /// The varyings at `[lon, lat, h]` (degrees, metres) in `view` with the light `light`, as deck's
    /// vertex injection makes them (`realistic.ts:417-442`), from the viewport's own common space.
    pub fn vary(light: &RLight, view: &ViewFrame, [lon, lat, h]: [f64; 3]) -> Vary {
        let vp = &view.viewport;
        let p = vp.project_position([lon, lat, h]);
        let units_per_metre = vp.distance_scales.units_per_meter[0];
        let to_camera = (vp.camera_position - p) / units_per_metre;
        let sun_dir = DVec3::from_array(light.sun_dir.map(f64::from));
        let anchor = DVec2::new(f64::from(light.anchor[0]), f64::from(light.anchor[1]));
        let (eye, sun, m) = match vp {
            Viewport::Globe(_) => {
                let [e, n, u] = enu(p);
                // the sun as deck has it: common space (the uniform holds it in the render frame)
                let s = rustradar_scene::look::from_local(sun_dir.to_array(), anchor.y, anchor.x);
                let sun_globe = DVec3::new(s[1], -s[0], s[2]);
                let local = |v: DVec3| DVec3::new(v.dot(e), v.dot(n), v.dot(u));
                let deg = DVec2::new(lon, lat) - anchor;
                let cos = ((anchor.y + 0.5 * deg.y) * DEG).cos().max(0.01);
                (local(to_camera), local(sun_globe), DVec2::new(deg.x, deg.y / cos) * (512.0 / 360.0))
            }
            // deck's common origin is the view centre: what is fixed to the world is measured from
            // the same place as `origin`
            Viewport::Mercator(flat) => (to_camera, sun_dir, DVec2::new(p.x - flat.center.x, p.y - flat.center.y)),
        };
        let origin = super::light_origin(view);
        let period = f64::from(light.period);
        let cloud = f64::from(light.cloud_period);
        let t = (DVec2::new(modulo(origin.x, period), modulo(origin.y, period)) + m) / period;
        let c = (DVec2::new(modulo(origin.x, cloud), modulo(origin.y, cloud)) + m) / cloud;
        Vary { eye, sun, t, c }
    }

    fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    fn mix(a: f64, b: f64, t: f64) -> f64 {
        a + (b - a) * t
    }

    fn v3(c: [f64; 3]) -> DVec3 {
        DVec3::from_array(c)
    }

    /// `rl_linear`: an sRGB colour (0–1) to linear light.
    pub fn linear(c: DVec3) -> DVec3 {
        c.max(DVec3::ZERO).powf(2.2)
    }

    /// `rl_display`, 0–255 unrounded.
    pub fn display(look: &Look, c: DVec3) -> [f64; 3] {
        ((c * look.exposure).clamp(DVec3::ZERO, DVec3::ONE).powf(1.0 / 2.2) * 255.0).to_array()
    }

    /// `rl_sunLight`.
    pub fn sun_light(look: &Look, rl: &Vary) -> DVec3 {
        let up = rl.sun.z;
        let far = look.globe;
        let dusk = DVec3::ONE.lerp(DVec3::new(1.0, 0.62, 0.38), far * (1.0 - smoothstep(0.0, 0.3, up)));
        v3(look.sun_color) * dusk * smoothstep(mix(-0.02, -0.03, far), mix(0.02, 0.22, far), up)
    }

    /// `rl_dayHere`.
    pub fn day_here(look: &Look, rl: &Vary) -> f64 {
        mix(1.0, mix(0.015, 1.0, smoothstep(-0.12, 0.12, rl.sun.z)), look.globe)
    }

    /// `rl_sky`.
    pub fn sky(look: &Look, rl: &Vary, dir: DVec3) -> DVec3 {
        let up = dir.z.clamp(-1.0, 1.0);
        let mut sky = v3(look.sky_horizon).lerp(v3(look.sky_zenith), up.max(0.0).sqrt());
        let to_sun = dir.dot(rl.sun.normalize()).max(0.0);
        sky += sun_light(look, rl) * (0.04 * to_sun.powf(6.0) + 0.5 * to_sun.powf(900.0));
        sky.lerp(v3(look.ground_color), 1.0 - smoothstep(-0.2, 0.0, up)) * day_here(look, rl)
    }

    /// `rl_irradiance`.
    pub fn irradiance(look: &Look, rl: &Vary, n: DVec3) -> DVec3 {
        DVec3::splat(look.ambient * day_here(look, rl)) * mix(0.6, 1.0, (0.5 + 0.5 * n.z).clamp(0.0, 1.0))
    }

    /// `rl_envBRDF`.
    pub fn env_brdf(n_dot_v: f64, roughness: f64) -> DVec2 {
        let c0 = [-1.0, -0.0275, -0.572, 0.022];
        let c1 = [1.0, 0.0425, 1.04, -0.04];
        let r: [f64; 4] = std::array::from_fn(|i| roughness * c0[i] + c1[i]);
        let a004 = (r[0] * r[0]).min((-9.28 * n_dot_v).exp2()) * r[0] + r[1];
        DVec2::new(-1.04, 1.04) * a004 + DVec2::new(r[2], r[3])
    }

    /// `rl_shade` (`glint`: the GLSL's `rl_glint`).
    #[allow(clippy::too_many_arguments)]
    pub fn shade(look: &Look, rl: &Vary, albedo: DVec3, n: DVec3, v: DVec3, roughness: f64, metalness: f64, ao: f64, sun_visible: f64, glint: f64) -> DVec3 {
        let l = rl.sun.normalize();
        let roughness = roughness.clamp(0.04, 1.0);
        let n_dot_l = n.dot(l).max(0.0);
        let n_dot_v = n.dot(v).max(1e-3);
        let h = (l + v).normalize();
        let n_dot_h = n.dot(h).max(0.0);
        let v_dot_h = v.dot(h).max(0.0);
        let f0 = DVec3::splat(0.04).lerp(albedo, metalness);
        let fresnel = f0 + (DVec3::ONE - f0) * (1.0 - v_dot_h).max(0.0).powf(5.0);
        let a = roughness * roughness;
        let a2 = a * a;
        let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
        let distribution = a2 / (std::f64::consts::PI * d * d);
        let k = a * 0.5;
        let vis = 0.25 / ((n_dot_l * (1.0 - k) + k) * (n_dot_v * (1.0 - k) + k)).max(1e-4);
        let diffuse = albedo * (1.0 - metalness);
        let specular_sun = fresnel * (distribution * vis * std::f64::consts::PI).min(60.0) * glint;
        let direct = ((DVec3::ONE - fresnel) * diffuse + specular_sun) * sun_light(look, rl) * n_dot_l * sun_visible;
        let brdf = env_brdf(n_dot_v, roughness);
        let r = -v - 2.0 * n.dot(-v) * n;
        let env = sky(look, rl, r).lerp(irradiance(look, rl, r) / look.ambient.max(1e-3), roughness);
        let specular = env * (f0 * brdf.x + DVec3::splat(brdf.y)) * (1.0 - 0.5 * roughness);
        direct + (diffuse * irradiance(look, rl, n) + specular) * ao
    }

    /// `rl_clear` (never draped here: `cover` is 0).
    pub fn clear(look: &Look, rl: &Vary) -> f64 {
        (-(rl.eye.length() - look.haze_start).max(0.0) * look.haze_density).exp()
    }

    /// `rl_aerial`.
    pub fn aerial(look: &Look, rl: &Vary, c: DVec3) -> DVec3 {
        let t = clear(look, rl);
        let to_here = -rl.eye / rl.eye.length().max(1e-3);
        let forward = to_here.dot(rl.sun.normalize()).max(0.0).powf(8.0);
        let air = v3(look.haze_color) * day_here(look, rl) + sun_light(look, rl) * 0.06 * forward;
        c * t + air * (1.0 - t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::shaders;

    /// 2026-06-21 10:00 UTC: noon in Stockholm at midsummer (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;

    fn view(lat: f64, lon: f64, zoom: f64, pitch: f64) -> ViewState {
        ViewState { latitude: lat, longitude: lon, zoom, pitch, bearing: 0.0, position: None }
    }

    /// doc 04 §4.1: the block's members at their std140 offsets (`globe` 160, `cover` 164, `eye`
    /// 176, 192 bytes), the Rust struct's the same.
    #[test]
    fn the_uniform_is_the_block_of_doc_04() {
        let (size, members) = wgsl_members(shaders::RLIGHT, "RLight");
        assert_eq!(size, 192);
        assert_eq!(size_of::<RLight>(), 192);
        let offset = |name: &str| members.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no {name}")).1;
        assert_eq!((offset("globe"), offset("cover"), offset("eye")), (160, 164, 176));
        macro_rules! same {
            ($($f:ident),*) => {
                let rust = vec![$((stringify!($f).to_string(), std::mem::offset_of!(RLight, $f) as u32)),*];
                assert_eq!(members, rust);
            };
        }
        same!(sun_dir, sun_up, sun_color, ambient, sky_zenith, exposure, sky_horizon, haze_density, haze_color, wet,
            ground_color, snow, sun_globe, haze_start, origin, period, period_metres, cloud_origin, cloud_period,
            cloud_shadow, anchor, night, time, globe, cover, eye);
    }

    /// The weather gives way to a fine day between 3 and 10 km up (G:3656), and `None` (no sky yet,
    /// or live weather not come) is a fine day.
    #[test]
    fn the_weather_holds_under_the_clouds_only() {
        let rain = PRESETS.rain;
        assert_eq!(weather_here(Some(rain), 0.0), rain);
        assert_eq!(weather_here(Some(rain), 3000.0), rain);
        // (mixed all the way: `a + (b − a) · 1`, as the TypeScript's, not quite `b`)
        let close = |a: Conditions, b: Conditions| {
            [(a.sun, b.sun), (a.cloud, b.cloud), (a.haze, b.haze), (a.wet, b.wet), (a.snow, b.snow), (a.rain, b.rain)]
                .iter()
                .all(|(x, y)| (x - y).abs() < 1e-12)
        };
        assert!(close(weather_here(Some(rain), 10_000.0), PRESETS.sunny));
        assert!(close(weather_here(Some(rain), f64::INFINITY), PRESETS.sunny));
        assert_eq!(weather_here(None, 0.0), PRESETS.sunny);
        let half = weather_here(Some(rain), 6500.0);
        assert!((half.wet - 0.5).abs() < 1e-12, "{half:?}");
        // the camera's distance: 1.5 viewport heights; its height: that times cos(pitch)
        let v = view(59.33, 18.07, 15.0, 60.0);
        let metres = camera_metres(&v, 900.0);
        assert!((metres - 1.5 * 900.0 * meters_per_pixel(15.0, 59.33)).abs() < 1e-9);
        assert!((camera_altitude(&v, 900.0) - metres * 0.5).abs() < 1e-6);
    }

    /// With *daylight* off the light comes from the fixed afternoon sun (`sceneSun`); on, from the
    /// real one over the view's centre at the local clock; the look is `lookFor` of the weather
    /// there with the camera's distance.
    #[test]
    fn a_frame_is_lit_by_the_scene_sun_in_the_weather_there() {
        let v = view(59.33, 18.07, 15.5, 60.0);
        let size = [1440.0, 900.0];
        let off = sky::Input { shown: true, weather: Some(PRESETS.cloudy), daylight: false };
        let look = frame_look(&off, &v, size, MIDSUMMER);
        let fixed = scene_sun(false, [0.0, 0.0, 1.0]);
        assert_eq!(look.sun_dir, fixed);
        let expected = look_for(&weather_here(Some(PRESETS.cloudy), camera_altitude(&v, 900.0)), fixed, camera_metres(&v, 900.0));
        assert_eq!(look, expected);
        // close in, under the clouds: the chosen weather's own grey sky
        assert_eq!(look.sun_color, [0.0; 3]);
        let on = sky::Input { shown: true, weather: None, daylight: true };
        let look = frame_look(&on, &v, size, MIDSUMMER);
        let real = local_sun(sun_direction(MIDSUMMER), 59.33, 18.07);
        assert_eq!(look.sun_dir[..2], real[..2]);
        // noon at midsummer in Stockholm: the sun 54° up, to the south
        assert!((real[2].asin() / DEG - 54.0).abs() < 1.0 && real[1] < -0.5, "{real:?}");
        // and at night it is down: the windows' night is full
        let night = frame_look(&on, &v, size, 1_797_890_400_000.0);
        assert!(night.sun_up < -0.1 && night.night > 0.99, "{night:?}");
    }

    /// `rlightProps`: the look as it is, the periods and the view centre's place within them, the
    /// sun turned into the globe's render frame (the view centre's up is −y there), the clock
    /// wrapped.
    #[test]
    fn the_uniform_holds_the_look_and_the_places() {
        let look = frame_look(&sky::Input { shown: true, weather: None, daylight: true }, &view(52.0, 10.0, 4.5, 0.0), [1100.0, 720.0], MIDSUMMER);
        let globe = ViewFrame::new(&view(52.0, 10.0, 4.5, 0.0), [1100.0, 720.0], 1.0, 0.0);
        let light = RLight::new(&look, &globe, 123_456.5);
        assert_eq!(light.exposure, look.exposure as f32);
        assert_eq!(light.sky_horizon, f3(look.sky_horizon));
        assert_eq!(light.globe, 1.0);
        assert_eq!((light.period, light.cloud_period), (1.0 / 64.0, 0.25));
        assert!((f64::from(light.time) - 23_456.5).abs() < 1e-3);
        assert_eq!(light.anchor, [10.0, 52.0]);
        let [x, y] = lng_lat_to_world([10.0, 52.0]);
        assert!((f64::from(light.origin[0]) - modulo(x, PERIOD)).abs() < 1e-9 && f64::from(light.origin[1]) < PERIOD);
        assert!((f64::from(light.cloud_origin[1]) - modulo(y, CLOUD_PERIOD)).abs() < 1e-7);
        // about 600 m at 60° north (doc 04 §4.1): 1223 m · cos 52°
        assert!((f64::from(light.period_metres) - 1223.0 * (52.0 * DEG).cos()).abs() < 1.0, "{}", light.period_metres);
        // the render frame: east +x, north +z, up −y at the view centre
        let [e, n, u] = look.sun_dir;
        let s = light.sun_globe.map(f64::from);
        assert!((s[0] - e).abs() < 1e-6 && (s[1] + u).abs() < 1e-6 && (s[2] - n).abs() < 1e-6, "{s:?} for {:?}", look.sun_dir);
        // the flat map: deck's common space
        let flat = ViewFrame::new(&view(59.33, 18.07, 15.5, 60.0), [1100.0, 720.0], 1.0, 0.0);
        let light = RLight::new(&look, &flat, 1.0);
        let sun = from_local(look.sun_dir, f64::from(59.33f32), f64::from(18.07f32));
        assert_eq!(light.sun_globe, f3([sun[1], -sun[0], sun[2]]));
        assert_eq!(light.origin.map(f64::from), [modulo(flat.viewport.center.x, PERIOD), modulo(flat.viewport.center.y, PERIOD)].map(|v| v as f32 as f64));
    }

    /// The twin's varyings at the view centre seen from straight above: the camera straight up at
    /// 1.5 viewport heights of the view's metres per pixel, the sun as the look has it there.
    #[test]
    fn the_twin_sees_the_centre_from_straight_above() {
        for v in [view(52.0, 10.0, 4.5, 0.0), view(59.33, 18.07, 15.5, 0.0)] {
            let size = [1100.0, 720.0];
            let look = frame_look(&sky::Input { shown: true, weather: None, daylight: true }, &v, size, MIDSUMMER);
            let vf = ViewFrame::new(&v, size, 1.0, 0.0);
            let light = RLight::new(&look, &vf, 0.0);
            let rl = twin::vary(&light, &vf, [v.longitude, v.latitude, 0.0]);
            let distance = 1.5 * 720.0 * vf.viewport.meters_per_pixel();
            assert!(rl.eye.x.abs() < 1e-3 * distance && rl.eye.y.abs() < 1e-3 * distance, "{v:?}: {:?}", rl.eye);
            assert!((rl.eye.z - distance).abs() < 1e-3 * distance, "{v:?}: {} vs {distance}", rl.eye.z);
            let sun = DVec3::from_array(look.sun_dir);
            assert!((rl.sun - sun).length() < 1e-6, "{v:?}: {:?} vs {sun:?}", rl.sun);
        }
    }
}
