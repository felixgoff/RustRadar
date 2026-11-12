//! The realistic globe's picture of the Earth and its towns' lights at night (item R10; doc 04
//! §4's globe terms, §5.1, §5.3, §6.8; `realistic-ground.ts:481-624`, `realistic-textures.ts`).
//!
//! Today's app has no layer of its own for them: they are terms of the ground shader, on all the
//! ground draws (`ground-land`'s rectangle, and the tiles' areas over it), and so they are here
//! (`shaders/ground.wgsl`, [`super::GroundShader`], item R2). This module is their account on the
//! CPU ([`earth_uv`], [`far`], [`dry`], [`town_lights`]) and the realistic globe's checks.
//!
//! - **The picture** (NASA's Blue Marble, `earth.color.ktx2`): read at `rg_e`, the place's
//!   longitude and latitude as the whole Earth's equirectangular coordinates ([`earth_uv`]; on the
//!   flat map the view's centre, near enough close in). Far out ([`far`]: as a pixel grows from
//!   120 m to 1200 m) every surface but ice takes the picture's colour for its own, before the
//!   snow and the wet (which fade with it), so the continents are the picture's; closer in,
//!   unmapped land (surface 0) keeps the picture's hue where it is dry ([`dry`]: deserts, rock) on
//!   the terrain texture's own unevenness. Water is never the picture's: the map's water is drawn
//!   as water at every zoom.
//! - **The lights** (NASA's Black Marble, `earth.night.ktx2`): on the globe, added to an area's
//!   lit colour where the sun has set *there* ([`town_lights`]: `0.55 · globe · dark`, the
//!   picture's colour in linear light), before the air; lines take none (`RL_LINE`).
//! - **Day and night per fragment** (doc 04 §4.2-4.3, R1's `lib/rlight.wgsl`): on the globe
//!   `rl_sun` is the sun turned into each vertex's own east, north and up (`sunGlobe · rl_frame`),
//!   so `rl_sun.z` is the sine of the sun's height at that place: the sun's light
//!   (`rl_sunLight`: reddening and fading through the evening from far out) and the sky's
//!   (`rl_dayHere`: down to 0.015 at night) follow it across the globe, and so do the lights.
//! - **The textures** (doc 04 §5.1, §5.3; plan 1.4.13; `textures::EARTH_COLOR`, `EARTH_NIGHT`):
//!   4096 × 2048, ETC1S transcoded to the device's best target (asked for explicitly, so an
//!   ASTC-only device does not fall to RGBA8), the top level left out on a device whose textures
//!   stop at 2048; sampled round the world east to west and clamped at the poles. Until they are
//!   in, the stand-in Earth has alpha 0, which turns [`far`] and [`dry`] off (the land keeps its
//!   own colours), and the stand-in night is black.
//!
//! Deviation from the plan's row (R10: "Earth picture and night lights below the vector ground"):
//! there is no layer under the ground; as today, the picture and the lights are the ground
//! shader's, on the ground's own areas, which is where R2 put them.

use rustradar_core::num::smoothstep;

/// `rg_e`: where `[lon, lat]` (degrees) lies on the pictures of the whole Earth, `(u, v)` with `v`
/// down from the north pole (`((lon + 180) / 360, (90 − lat) / 180)`).
pub fn earth_uv([lon, lat]: [f64; 2]) -> [f64; 2] {
    [(lon + 180.0) / 360.0, (90.0 - lat) / 180.0]
}

/// GLSL's `smoothstep(a, b, x)` (`a < b`).
fn step(a: f64, b: f64, x: f64) -> f64 {
    smoothstep((x - a) / (b - a))
}

/// How much of a surface's colour is the picture's (`far`): `earth_alpha` (the picture's alpha
/// there: 0 while it is not loaded) as a pixel spans from 120 m to 1200 m (`metres_per_pixel`,
/// `rl_cells(rl_t) · periodMetres`).
pub fn far(earth_alpha: f64, metres_per_pixel: f64) -> f64 {
    earth_alpha * step(120.0, 1200.0, metres_per_pixel)
}

/// How much of the picture's hue unmapped land takes (`dry`): where the picture's linear colour
/// `earth` (r, g, b, a) is redder than green, from 0.03 to 0.15 of `(r − g) / (r + g)`.
pub fn dry(earth: [f64; 4]) -> f64 {
    let [r, g, _, a] = earth;
    a * step(0.03, 0.15, (r - g) / (r + g + 1e-4))
}

/// How much of the night picture's light (linear) is added to an area: `0.55 · globe · dark`,
/// with `dark = 1 − smoothstep(−0.12, 0.03, sun_up)` for the sine of the sun's height there
/// (`rl_sun.z`); none off the globe (`globe` 0, the light's `look.globe`).
pub fn town_lights(globe: f64, sun_up: f64) -> f64 {
    if globe > 0.0 { 0.55 * globe * (1.0 - step(-0.12, 0.03, sun_up)) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;

    use glam::DVec4;

    use super::super::twin::{self, FarOut, Fragment, Picture};
    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{earth, ground, sky};
    use crate::look::{self, Look, RLight};
    use crate::renderer::{MapRenderer, SceneInput};

    /// V1 (plan section 8): the whole globe.
    const V1: ViewState = ViewState { latitude: 35.0, longitude: 10.0, zoom: 1.9, pitch: 0.0, bearing: 0.0, position: None };
    /// 2026-06-21 10:00 UTC: the reference pictures' summer morning.
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// 2026-06-21 22:00 UTC: night over Europe, Africa and the Middle East.
    const NIGHT: f64 = 1_782_079_200_000.0;
    const SIZE: [f64; 2] = [1100.0, 720.0];
    const SUNNY: sky::Input = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };

    /// Inland places of V1 with nothing mapped on them at the globe's zooms but the land: the
    /// Sahara, Arabia, the steppe, the Russian plain.
    const INLAND: [[f64; 2]; 12] = [
        [0.0, 24.0],
        [5.0, 28.0],
        [10.0, 20.0],
        [15.0, 25.0],
        [20.0, 18.0],
        [25.0, 22.0],
        [45.0, 22.0],
        [50.0, 26.0],
        [55.0, 47.0],
        [65.0, 48.0],
        [40.0, 52.0],
        [45.0, 57.0],
    ];

    /// The open sea of V1: the Atlantic, away from the sun's glint.
    const SEA: [[f64; 2]; 4] = [[-30.0, 30.0], [-35.0, 15.0], [-25.0, 42.0], [-20.0, 5.0]];

    /// Inland towns whose lights show from space: Cairo, Riyadh, Madrid, Rome, Ankara, Moscow,
    /// Milan, Tehran, Baghdad, Marrakesh.
    const TOWNS: [[f64; 2]; 10] = [
        [31.24, 30.04],
        [46.72, 24.71],
        [-3.70, 40.42],
        [12.50, 41.90],
        [32.86, 39.93],
        [37.62, 55.76],
        [9.19, 45.46],
        [51.39, 35.69],
        [44.36, 33.31],
        [-7.99, 31.63],
    ];

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// V1 on the realistic globe at `now_ms` as the app's scene builder has it (sky, earth, land,
    /// the ground's tiles from the committed fixtures; no clouds), and its light.
    fn shot(gpu: &crate::gpu::Gpu, now_ms: f64) -> (Image, ViewFrame, Look) {
        let mut scene = SceneInput::new(V1, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = now_ms;
        scene.feed_now_ms = now_ms;
        scene.earth = earth::Input::SHOWN;
        scene.sky = SUNNY;
        scene.ground = ground::Input::SHOWN;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(std::sync::Arc::new(crate::io::DirTiles::fixtures()), None);
        let image = render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
        (image, ViewFrame::new(&V1, SIZE, 1.0, 0.0), look::frame_look(&SUNNY, &V1, SIZE, now_ms))
    }

    /// The pixel `[lon, lat]` falls in (on the globe's side facing the camera), and the place its
    /// centre sees.
    fn pixel_of(view: &ViewFrame, [lon, lat]: [f64; 2]) -> ([u32; 2], [f64; 2]) {
        let [x, y, _] = view.viewport.project([lon, lat, 0.0]);
        assert!((0.0..SIZE[0]).contains(&x) && (0.0..SIZE[1]).contains(&y), "{lon},{lat} off the screen at {x},{y}");
        let (x, y) = (x.floor(), y.floor());
        ([x as u32, y as u32], view.viewport.unproject([x + 0.5, y + 0.5], 0.0))
    }

    /// The pixel's centre.
    fn centre([x, y]: [u32; 2]) -> [f64; 2] {
        [f64::from(x) + 0.5, f64::from(y) + 0.5]
    }

    /// The light's varyings at `[lon, lat]` on the ground.
    fn vary_at(look: &Look, view: &ViewFrame, [lon, lat]: [f64; 2]) -> look::twin::Vary {
        look::twin::vary(&RLight::new(look, view, 0.0), view, [lon, lat, 0.0])
    }

    /// `picture`'s first layer at `(u, v)` of `level`, filtered linearly in linear light, round
    /// the world east to west and clamped at the poles (the Earth's sampler on one level).
    fn sample(picture: &Picture, level: usize, [u, v]: [f64; 2]) -> DVec4 {
        let l = &picture.transcoded.levels[level];
        let (w, h) = (f64::from(l.width), f64::from(l.height));
        let (x, y) = (u * w - 0.5, (v * h - 0.5).clamp(0.0, h - 1.0));
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |dx: f64, dy: f64| picture.texel(level, 0, (x0 + dx).rem_euclid(w) as u32, (y0 + dy).clamp(0.0, h - 1.0) as u32);
        at(0.0, 0.0).lerp(at(1.0, 0.0), fx).lerp(at(0.0, 1.0).lerp(at(1.0, 1.0), fx), fy)
    }

    /// The levels of the Earth's pictures the GPU reads for the pixel at `xy` (its centre): from
    /// the one its shorter side spans a texel of to the one its longer side does (trilinear
    /// filtering, with anisotropy or without).
    fn levels_at(view: &ViewFrame, xy: [f64; 2], count: usize) -> std::ops::RangeInclusive<usize> {
        let uv = |dx: f64, dy: f64| earth_uv(view.viewport.unproject([xy[0] + dx, xy[1] + dy], 0.0));
        let (a, b, c) = (uv(0.0, 0.0), uv(1.0, 0.0), uv(0.0, 1.0));
        let texels = |p: [f64; 2]| ((p[0] - a[0]) * 4096.0).hypot((p[1] - a[1]) * 2048.0).max(1.0).log2();
        let (x, y) = (texels(b), texels(c));
        (x.min(y).floor() as usize).min(count - 1)..=(x.max(y).ceil() as usize).min(count - 1)
    }

    /// The land's colour far out (unmapped land, from 1200 m a pixel) where the pixel at `xy`
    /// looks, its light's varyings `rl`, under `look`: the lowest and the highest of each channel
    /// over the levels the GPU reads there and the places within a texel of each (the GPU
    /// interpolates the pictures' coordinates across the globe's flat 5° facets).
    fn land_range(pictures: &FarOut, look: &Look, view: &ViewFrame, rl: &look::twin::Vary, xy: [f64; 2]) -> ([f64; 3], [f64; 3]) {
        let uv = earth_uv(view.viewport.unproject(xy, 0.0));
        let land = rustradar_scene::ground::LAND;
        let (mut low, mut high) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for level in levels_at(view, xy, pictures.earth.transcoded.levels.len()) {
            let size = 4096.0 / f64::from(1u32 << level);
            for du in [-1.0, 0.0, 1.0] {
                for dv in [-1.0, 0.0, 1.0] {
                    let at = [uv[0] + du / size, uv[1] + dv / (size / 2.0)];
                    let fragment = Fragment {
                        normal_texel: pictures.terrain_normal,
                        earth: sample(&pictures.earth, level, at),
                        night: sample(&pictures.night, level, at).truncate(),
                        metres_per_pixel: 1e6,
                        ..Fragment::stand_in(0, [land[0], land[1], land[2]])
                    };
                    let c = twin::colour(look, rl, &fragment);
                    for i in 0..3 {
                        low[i] = low[i].min(c[i]);
                        high[i] = high[i].max(c[i]);
                    }
                }
            }
        }
        (low, high)
    }

    /// How far `got` lies outside `low..high`, the most of its channels.
    fn outside(got: [u8; 4], (low, high): ([f64; 3], [f64; 3])) -> f64 {
        (0..3).map(|i| (low[i] - f64::from(got[i])).max(f64::from(got[i]) - high[i]).max(0.0)).fold(0.0, f64::max)
    }

    fn luma(c: [u8; 4]) -> f64 {
        0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])
    }

    #[test]
    fn the_terms_are_the_shaders() {
        assert_eq!(earth_uv([-180.0, 90.0]), [0.0, 0.0]);
        assert_eq!(earth_uv([0.0, 0.0]), [0.5, 0.5]);
        assert_eq!(earth_uv([90.0, -45.0]), [0.75, 0.75]);
        // all the picture from 1200 m a pixel, none to 120 m, none while it is not in
        assert_eq!((far(1.0, 100.0), far(1.0, 120.0), far(1.0, 660.0), far(1.0, 1200.0), far(0.0, 5000.0)), (0.0, 0.0, 0.5, 1.0, 0.0));
        // a desert's hue (red over green) is the picture's, a forest's (green) is not
        assert_eq!(dry([0.6, 0.4, 0.2, 1.0]), 1.0);
        assert_eq!(dry([0.1, 0.2, 0.05, 1.0]), 0.0);
        assert_eq!(dry([0.6, 0.4, 0.2, 0.0]), 0.0);
        // the lights: from the sun 1.7° down to 7° down, only on the globe
        let up = |deg: f64| deg.to_radians().sin();
        assert_eq!(town_lights(1.0, up(5.0)), 0.0);
        assert_eq!(town_lights(1.0, up(1.72)), 0.0);
        assert!((town_lights(1.0, up(-6.9)) - 0.55).abs() < 1e-3);
        assert_eq!(town_lights(1.0, -1.0), 0.55);
        assert_eq!(town_lights(0.4, -1.0), 0.55 * 0.4);
        assert_eq!(town_lights(0.0, -1.0), 0.0);
        let half = town_lights(1.0, -0.045);
        assert!((half - 0.275).abs() < 1e-9, "{half}");
    }

    /// R10's acceptance, the continents: V1 realistic by day, its ground's tiles in, is the Earth
    /// picture's colours on the land, lit (each inland place within 4 levels of the CPU's twin of
    /// the ground shader from the pictures decoded on the CPU, on the levels the GPU reads there,
    /// within a texel of the place), and the map's own water on the sea, far from what the
    /// picture's ocean would give.
    #[test]
    fn v1_shows_the_continents_from_the_earth_picture() {
        let Some(gpu) = gpu() else { return };
        let pictures = FarOut::load().expect("the Earth's pictures");
        let (image, view, look) = shot(&gpu, MIDSUMMER);
        save(&image, "r10-v1-earth-day.png");
        for place in INLAND {
            let ([x, y], place) = pixel_of(&view, place);
            let rl = vary_at(&look, &view, place);
            let range = land_range(&pictures, &look, &view, &rl, centre([x, y]));
            let got = image.pixel(x, y);
            eprintln!("{place:?} at {x},{y}: {got:?}, the picture lit {:?} to {:?}", range.0, range.1);
            assert!(outside(got, range) <= 4.0, "{place:?}: {got:?} vs {range:?}");
        }
        for place in SEA {
            let ([x, y], place) = pixel_of(&view, place);
            let rl = vary_at(&look, &view, place);
            let picture = land_range(&pictures, &look, &view, &rl, centre([x, y]));
            let got = image.pixel(x, y);
            eprintln!("sea {place:?} at {x},{y}: {got:?}, the picture lit {:?} to {:?}", picture.0, picture.1);
            // the water's grey-blue, far from what the picture's ocean would be
            assert!(got[2] > got[0] + 15 && outside(got, picture) > 20.0, "{place:?}: {got:?}");
        }
    }

    /// R10's acceptance, the night side: V1 realistic at night over Europe, Africa and the Middle
    /// East shows the towns' lights (each town, where the sun is down there, brighter than the
    /// desert round it and within 8 levels of the CPU's twin with the Black Marble's light added),
    /// and the land without towns dark; by day the same places show no lights.
    #[test]
    fn the_night_side_shows_its_towns_lights() {
        let Some(gpu) = gpu() else { return };
        let pictures = FarOut::load().expect("the Earth's pictures");
        let (night, view, look) = shot(&gpu, NIGHT);
        save(&night, "r10-v1-earth-night.png");
        assert_eq!(look.globe, 1.0);
        // the brightest pixel within 2 of a town's
        let brightest = |image: &Image, [x, y]: [u32; 2]| {
            let mut best = image.pixel(x, y);
            for dy in 0..5 {
                for dx in 0..5 {
                    let p = image.pixel(x + dx - 2, y + dy - 2);
                    if luma(p) > luma(best) {
                        best = p;
                    }
                }
            }
            best
        };
        let mut desert = 0.0f64;
        for place in &INLAND[..6] {
            let ([x, y], place) = pixel_of(&view, *place);
            let rl = vary_at(&look, &view, place);
            let got = night.pixel(x, y);
            eprintln!("{place:?} at night: {got:?}, sun up {:.3}, CPU {:?}", rl.sun.normalize().z, land_range(&pictures, &look, &view, &rl, centre([x, y])));
            desert = desert.max(luma(got));
        }
        eprintln!("the Sahara at night: luma at most {desert:.1}");
        assert!(desert < 25.0, "{desert}");
        let mut lit = 0;
        for town in TOWNS {
            let (xy, town) = pixel_of(&view, town);
            let rl = vary_at(&look, &view, town);
            let lights = town_lights(look.globe, rl.sun.normalize().z);
            let got = night.pixel(xy[0], xy[1]);
            let range = land_range(&pictures, &look, &view, &rl, centre(xy));
            let best = brightest(&night, xy);
            eprintln!("{town:?} at {xy:?}: {got:?} (brightest near {best:?}), lights {lights:.3}, CPU {:?} to {:?}", range.0, range.1);
            assert!(outside(got, range) <= 8.0, "{town:?}: {got:?} vs {range:?}");
            if lights >= 0.5 {
                lit += 1;
                assert!(luma(best) > desert + 25.0, "{town:?}: {best:?}");
            }
        }
        assert!(lit >= 8, "{lit} towns in the dark");
        // by day, no lights: the towns are the picture's day colours
        let (day, view, look) = shot(&gpu, MIDSUMMER);
        for town in TOWNS {
            let (xy, town) = pixel_of(&view, town);
            let rl = vary_at(&look, &view, town);
            assert_eq!(town_lights(look.globe, rl.sun.normalize().z), 0.0, "{town:?}");
            let range = land_range(&pictures, &look, &view, &rl, centre(xy));
            assert!(outside(day.pixel(xy[0], xy[1]), range) <= 8.0, "{town:?}: {:?} vs {range:?}", day.pixel(xy[0], xy[1]));
        }
    }
}
