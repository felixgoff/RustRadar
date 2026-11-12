//! The realistic map: what it is lit by (item R1), then its ground, buildings, trees and clouds
//! (R2–R12, doc 04).
//!
//! The light (doc 04 §1's driver, `Globe.svelte`'s `updateLook`, G:3647-3682) is worked out by the
//! map before every frame for that frame's camera and the local clock (`map::look::frame_look`), as
//! today's `onBeforeRender` does; the scene hands it what it is made from ([`sky::Input`]): the
//! weather the *sky* setting picks ([`sky_conditions`], `+page.svelte:243`: the chosen preset, or for
//! *live* the live weather's conditions, none until they have come) and the *daylight* setting (off:
//! the fixed afternoon sun). The map's sky and ground-land layers draw from it on the realistic
//! basemap only; on the others it is handed over all the same and nothing reads it.
//!
//! The live weather is R11's (`state::weather`): until MET Norway has answered, *live* reads as
//! none (a fine day), as today's does. The *tilt-shift* setting goes to the map's post effects
//! (R11: drawn on the realistic basemap only); the rain follows the light the map works out.

use rustradar_core::weather::{Conditions, Sky};
use rustradar_map::SceneInput;
use rustradar_map::layers::{clouds, ground, sky};
use rustradar_map::post;

use crate::state::AppState;

/// The weather the realistic map is lit by (`skyConditions`, `+page.svelte:243`): for *live* the
/// live weather's conditions (`conditionsOf(liveWeather)`, `None` until it has come), else the
/// chosen preset.
pub fn sky_conditions(sky: Sky, live: Option<Conditions>) -> Option<Conditions> {
    match sky {
        Sky::Live => live,
        chosen => chosen.preset(),
    }
}

/// The realistic map: sky, ground, buildings, trees, clouds.
#[derive(Debug, Default)]
pub struct Realistic {}

impl Realistic {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, state: &AppState, scene: &mut SceneInput) {
        let settings = &state.persisted.settings;
        // (live weather: `state::weather`'s, none until it has come)
        scene.sky = sky::Input { shown: true, weather: sky_conditions(settings.sky, state.weather.conditions()), daylight: settings.daylight };
        // the tilt-shift (R11; `tiltShift && basemap === "realistic"`: the map checks the basemap)
        // and the rain overlay, which falls as the map's light says it rains
        scene.post = post::Input { tilt_shift: settings.tilt_shift, rain: true };
        // the ground's tiles and textures (R2; drawn on the realistic basemap only)
        scene.ground = ground::Input::SHOWN;
        // the latest cloud picture (R10; `cloudPicture`'s, the bytes shared): the map draws it on
        // the realistic basemap while the zoom shows the clouds
        scene.clouds = clouds::Input { picture: state.clouds.picture().map(|p| clouds::Picture(p.jpeg.clone())) };
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_core::weather::PRESETS;
    use rustradar_map::frame::ViewFrame;
    use rustradar_map::{Gpu, GpuError, look};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::Options;
    use crate::headless;
    use crate::scene_builder::SceneBuilder;
    use crate::state::{Clocks, Startup};

    /// V2 (plan section 8): Europe.
    const V2: ViewState = ViewState { latitude: 52.0, longitude: 10.0, zoom: 4.5, pitch: 0.0, bearing: 0.0, position: None };
    /// 2026-06-21 11:20 UTC: local noon at 10° E at midsummer (V2's centre).
    const V2_NOON: f64 = 1_782_040_800_000.0;
    /// 2026-12-21 22:00 UTC: night over Europe.
    const MIDWINTER_NIGHT: f64 = 1_797_890_400_000.0;
    const SIZE: [u32; 2] = [1100, 720];

    #[test]
    fn the_sky_setting_picks_the_weather() {
        assert_eq!(sky_conditions(Sky::Sunny, None), Some(PRESETS.sunny));
        assert_eq!(sky_conditions(Sky::Snow, Some(PRESETS.fog)), Some(PRESETS.snow));
        assert_eq!(sky_conditions(Sky::Live, None), None);
        assert_eq!(sky_conditions(Sky::Live, Some(PRESETS.fog)), Some(PRESETS.fog));
    }

    /// The scene carries the *sky* and *daylight* settings to the map's light on every basemap.
    #[test]
    fn the_scene_hands_the_light_its_settings() {
        for basemap in [Basemap::Realistic, Basemap::Dark] {
            let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, V2, [1100.0, 720.0]);
            let clocks = Clocks { local_ms: V2_NOON, feed_ms: V2_NOON };
            let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
            // the defaults: live weather (none yet), daylight on
            assert_eq!(scene.sky, sky::Input { shown: true, weather: None, daylight: true });
            state.persisted.settings.sky = Sky::Rain;
            state.persisted.settings.daylight = false;
            let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
            assert_eq!(scene.sky, sky::Input { shown: true, weather: Some(PRESETS.rain), daylight: false });
        }
    }

    /// The scene shows the realistic ground (R2), which the map draws on the realistic basemap
    /// alone.
    #[test]
    fn the_scene_shows_the_ground() {
        let mut state = AppState::new(Startup { basemap: Some(Basemap::Realistic), ..Default::default() }, None, None, None, V2, [1100.0, 720.0]);
        let clocks = Clocks { local_ms: V2_NOON, feed_ms: V2_NOON };
        let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
        assert_eq!(scene.ground, ground::Input::SHOWN);
    }

    /// The committed fixture folder, which holds a cloud picture (`clouds/`, item X1).
    fn fixture_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../map/tests/data")
    }

    /// The scene hands the map the state's cloud picture (R10): none until one has come, then the
    /// feed's own bytes (shared, not copied).
    #[test]
    fn the_scene_hands_the_map_the_cloud_picture() {
        let mut state = AppState::new(Startup { basemap: Some(Basemap::Realistic), ..Default::default() }, None, None, None, V1, [1100.0, 720.0]);
        let clocks = Clocks { local_ms: MIDSUMMER, feed_ms: MIDSUMMER };
        let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
        assert!(scene.clouds.picture.is_none());
        state.clouds = rustradar_backend::clouds::Feed::fixture(fixture_dir());
        assert!(state.clouds.poll(MIDSUMMER, true), "the fixture's picture is in at the ask");
        let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
        let picture = scene.clouds.picture.expect("the picture handed over");
        assert!(std::sync::Arc::ptr_eq(&picture.0, &state.clouds.picture().expect("the feed's").jpeg));
    }

    /// V1 (plan section 8): the whole globe.
    const V1: ViewState = ViewState { latitude: 35.0, longitude: 10.0, zoom: 1.9, pitch: 0.0, bearing: 0.0, position: None };
    /// 2026-06-21 10:00 UTC: the reference pictures' summer morning.
    const MIDSUMMER: f64 = 1_782_036_000_000.0;

    /// R10's acceptance through the whole app (`--screenshot --view 35,10,1.9 --basemap realistic
    /// --fixture crates/map/tests/data`): the --fixture folder's cloud picture reaches the map and
    /// is drawn over the globe. Where the picture is clear all round the place a pixel sees at the
    /// clouds' height, the globe is as without the folder; where it is thick all round and the sun
    /// is up there, the cloud is white; and many pixels change.
    #[test]
    fn v1_shows_the_fixture_clouds_through_the_app() {
        let Some(gpu) = gpu() else { return };
        let options = Options {
            view: Some(V1),
            basemap: Some(Basemap::Realistic),
            size: Some(SIZE),
            scale: Some(1.0),
            now_ms: Some(MIDSUMMER),
            ..Default::default()
        };
        let labels_off = |state: &mut AppState| state.persisted.settings.labels = false;
        let bare = headless::render_on_with(&gpu, &options, labels_off).expect("the shot without clouds").image;
        let with_folder = Options { fixture: Some(fixture_dir()), ..options.clone() };
        let clouded = headless::render_on_with(&gpu, &with_folder, labels_off).expect("the shot with clouds").image;
        let jpeg = std::fs::read(fixture_dir().join(rustradar_backend::clouds::FIXTURE)).expect("the fixture's picture");
        let picture = clouds::decode(&jpeg, 8192, &rustradar_map::jobs::Cancel::new()).expect("decoded");
        let (w, h) = (i64::from(picture.width), i64::from(picture.height));
        // the picture's least and most grey (0–1) within 3 texels of `[lon, lat]`
        let grey = |[lon, lat]: [f64; 2]| {
            let (x, y) = (((lon + 180.0) / 360.0 * w as f64) as i64, ((85.0 - lat) / 170.0 * h as f64) as i64);
            let (mut low, mut high) = (255u8, 0u8);
            for dy in -3..=3 {
                for dx in -3..=3 {
                    let g = picture.levels[0][((y + dy).clamp(0, h - 1) * w + (x + dx).rem_euclid(w)) as usize];
                    (low, high) = (low.min(g), high.max(g));
                }
            }
            (f64::from(low) / 255.0, f64::from(high) / 255.0)
        };
        let size = [f64::from(SIZE[0]), f64::from(SIZE[1])];
        let view = ViewFrame::new(&V1, size, 1.0, 0.0);
        let look = look::frame_look(&sky::Input { shown: true, weather: None, daylight: true }, &V1, size, MIDSUMMER);
        let globe = view.viewport.globe().expect("V1 is the globe");
        let (mut clear, mut thick, mut changed, mut on_globe) = (0, 0, 0, 0);
        for y in (4..SIZE[1]).step_by(8) {
            for x in (4..SIZE[0]).step_by(8) {
                let xy = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                if globe.ray_distance_to_globe_center_ratio(xy) >= 0.75 {
                    continue;
                }
                let [lon, lat] = view.viewport.unproject(xy, clouds::CLOUD_HEIGHT_M);
                if lat.abs() > 84.0 {
                    continue;
                }
                on_globe += 1;
                let (under, over) = (bare.pixel(x, y), clouded.pixel(x, y));
                if under != over {
                    changed += 1;
                }
                let (low, high) = grey([lon, lat]);
                let sun = look::twin::vary(&look::RLight::new(&look, &view, 0.0), &view, [lon, lat, clouds::CLOUD_HEIGHT_M]).sun.normalize().z;
                if high <= 0.33 {
                    clear += 1;
                    assert_eq!(over, under, "clear sky at {x},{y}");
                } else if low >= 0.62 && sun > 0.3 {
                    thick += 1;
                    assert!(over[..3].iter().all(|&c| c >= 190), "thick cloud at {x},{y}: {over:?} over {under:?}");
                }
            }
        }
        eprintln!("V1 through the app: {on_globe} pixels on the globe, {changed} changed, {clear} clear, {thick} thick");
        assert!(clear > 100 && thick >= 20, "{clear} clear, {thick} thick");
        assert!(changed * 5 > on_globe, "{changed} of {on_globe} changed");
    }

    fn gpu() -> Option<Gpu> {
        match Gpu::headless() {
            Ok(gpu) => Some(gpu),
            Err(GpuError::NoAdapter(e)) => {
                eprintln!("skipped: {e}");
                None
            }
            Err(e) => panic!("{e}"),
        }
    }

    /// A realistic shot through the whole app (state, scene builder, map) with the labels off, so
    /// the land shows everywhere: the image and the frame's light.
    fn shot(gpu: &Gpu, view: ViewState, now_ms: f64, sky: Sky) -> (rustradar_map::headless::Image, ViewFrame, look::Look) {
        let options = Options {
            view: Some(view),
            basemap: Some(Basemap::Realistic),
            size: Some(SIZE),
            scale: Some(1.0),
            now_ms: Some(now_ms),
            ..Default::default()
        };
        let shot = headless::render_on_with(gpu, &options, |state| {
            state.persisted.settings.sky = sky;
            state.persisted.settings.labels = false;
        })
        .expect("the shot");
        let size = [f64::from(SIZE[0]), f64::from(SIZE[1])];
        let input = sky::Input { shown: true, weather: sky_conditions(sky, None), daylight: true };
        let look = look::frame_look(&input, &view, size, now_ms);
        (shot.image, ViewFrame::new(&view, size, 1.0, 0.0), look)
    }

    /// The land's colour at `xy` far out as the CPU has it (the light of `scene::look` through the
    /// ground shader's twin, `ground::twin`: from 1200 m a pixel the Earth picture's own colours,
    /// lit, and its towns' lights at night), within a texel of the place: the lowest and highest.
    fn far_land(far: &ground::twin::FarOut, look: &look::Look, view: &ViewFrame, xy: [f64; 2]) -> ([f64; 3], [f64; 3]) {
        let [lon, lat] = view.viewport.unproject(xy, 0.0);
        let rl = look::twin::vary(&look::RLight::new(look, view, 0.0), view, [lon, lat, 0.0]);
        far.land_range(look, &rl, [lon, lat])
    }

    /// R1's acceptance (`--screenshot --view 52,10,4.5 --basemap realistic`, the sunny preset at
    /// local noon) as R2 has it: a land pixel within 4 levels of the CPU's reference (doc 04 §3.5's
    /// display transform of the light of `scene::look` on the ground far out, the Earth picture's
    /// colours: [`far_land`]); the space round the globe (V2 is far out: space's colours, the
    /// horizon above the screen) as the gradient's stops say; at night the land dark. (R1 drew the
    /// land untextured, its tint alone, `land::lit_colour`; its points were (550, 360), (300, 200)
    /// and (800, 550), the second now in the North Sea's water, so a point on the land replaces it.)
    #[test]
    fn v2_realistic_at_noon_is_lit_as_the_cpu_says() {
        let Some(gpu) = gpu() else { return };
        let far = ground::twin::FarOut::load().expect("the Earth's pictures");
        let (image, view, look) = shot(&gpu, V2, V2_NOON, Sky::Sunny);
        for xy in [[550.0, 360.0], [420.0, 470.0], [800.0, 550.0]] {
            let (low, high) = far_land(&far, &look, &view, xy);
            let got = image.pixel(xy[0] as u32, xy[1] as u32);
            eprintln!("V2 noon at {xy:?}: {got:?}, CPU {low:?} to {high:?}");
            assert!((0..3).all(|i| f64::from(got[i]) >= low[i] - 4.0 && f64::from(got[i]) <= high[i] + 4.0), "{xy:?}: {got:?} vs {low:?} to {high:?}");
        }
        let sky = rustradar_map::layers::sky::gradient(&look, &V2, [1100.0, 720.0]);
        assert_eq!((sky.top, sky.horizon), ([4, 7, 16], [12, 18, 34]));
        let (night, view, look) = shot(&gpu, V2, MIDWINTER_NIGHT, Sky::Sunny);
        let centre = night.pixel(550, 360);
        let (low, high) = far_land(&far, &look, &view, [550.0, 360.0]);
        eprintln!("V2 night: {centre:?}, CPU {low:?} to {high:?}");
        assert!((0..3).all(|i| f64::from(centre[i]) >= low[i] - 4.0 && f64::from(centre[i]) <= high[i] + 4.0), "{centre:?} vs {low:?} to {high:?}");
        assert!(centre[..3].iter().all(|&c| c < 30), "{centre:?}");
    }

    /// Close in and tilted to the realistic map's 75°, the sky shows over the horizon: by day the
    /// gradient's stops at the top of the screen, at night a dark sky.
    #[test]
    fn a_tilted_view_shows_the_sky_by_day_and_by_night() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 75.0, bearing: 20.0, position: None };
        let mut tops = Vec::new();
        for now in [1_782_036_000_000.0, MIDWINTER_NIGHT] {
            let (image, _, look) = shot(&gpu, view, now, Sky::Sunny);
            let sky = rustradar_map::layers::sky::gradient(&look, &view, [1100.0, 720.0]);
            let top = image.pixel(550, 0);
            eprintln!("{now}: top {top:?}, gradient {sky:?}");
            assert!((0..3).all(|i| top[i].abs_diff(sky.top[i]) <= 4), "{top:?} vs {:?}", sky.top);
            tops.push(top);
        }
        let [day, night] = [tops[0], tops[1]];
        assert!((0..3).all(|i| night[i] + 60 < day[i]), "night {night:?}, day {day:?}");
    }
}
