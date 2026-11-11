//! The base map: the vector map on the dark basemap (item B2; doc 01 §5.1, §5.6).
//!
//! Today's `buildLayers` (G:2668-2676) puts the land rectangle and then the vector map
//! (`map-globe` or `map-flat`) on the dark basemap, once the TileJSON has given the tile URL; the
//! satellite and realistic basemaps have no vector map. Here the builder only says whether it is
//! drawn ([`vector_map::Input::shown`]): the land is the earth layer's (the scene's `earth`), the
//! tiles and the projection are the map layer's, and the tile URL is the app's fetcher's
//! (`tile_fetcher::TileSources::app` reads OpenFreeMap's TileJSON once; until it has, or if it
//! cannot, no tile arrives and only the land shows, as today).
//!
//! The night (item B5; G:2692) follows the *daylight* setting, and the place and airport labels
//! (item B4; G:2743-2752) the *labels* and *label size* settings, on every basemap; the flight
//! builder then fills in the airports whose labels give way to markers (`labels.marked`).
//!
//! The plain buildings (item B7; G:2694) follow the *buildings* setting
//! ([`buildings::Input::shown`]; the layer works out the flat map and the basemap), and with them
//! the scene's key light is aimed at the sun (`scene.aim_lights`, G:2723-2724: from the real sun
//! over buildings on the flat map, else from its usual direction), once the clocks are set.

use rustradar_core::geo::Basemap;
use rustradar_map::SceneInput;
use rustradar_map::layers::{buildings, labels, night, vector_map};

use crate::state::AppState;

/// The base map: the vector map's tile source on the dark basemap, the night, the place and
/// airport labels, the plain buildings.
#[derive(Debug, Default)]
pub struct Base {
    /// Where the labels layer writes the labels it draws, kept across refreshes so that picking
    /// (A9) reads the labels on screen, not an empty list until the next frame.
    drawn: labels::Drawn,
}

impl Base {
    /// Fills its layers' inputs in `scene` from `state` (see [`super::SceneBuilder::build`]).
    pub fn build(&mut self, state: &AppState, scene: &mut SceneInput) {
        let settings = &state.persisted.settings;
        scene.vector_map = vector_map::Input { shown: state.basemap() == Basemap::Dark };
        // the night (B5; `if (daylight && !realistic)`, G:2692): the layer leaves realistic out
        scene.night = night::Input { shown: settings.daylight };
        // the place and airport labels (B4; G:2743-2752) on every basemap while the *labels*
        // setting is on, at the *label size*; the flight builder adds the airports it marks
        scene.labels = labels::Input { shown: settings.labels, label_scale: settings.label_size, marked: Vec::new(), drawn: self.drawn.clone() };
        let buildings = settings.buildings;
        scene.buildings = buildings::Input { shown: buildings };
        scene.aim_lights(buildings);
    }
}

#[cfg(test)]
mod tests {
    use rustradar_map::{Gpu, GpuError};
    use rustradar_map::headless::Image;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::Options;
    use crate::headless;
    use crate::scene_builder::SceneBuilder;
    use crate::state::{Clocks, Startup};

    const V3: ViewState = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };

    #[test]
    fn the_vector_map_is_drawn_on_the_dark_basemap_only() {
        for (basemap, shown) in [(Basemap::Dark, true), (Basemap::Satellite, false), (Basemap::Realistic, false)] {
            let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, V3, [1100.0, 720.0]);
            let clocks = Clocks { local_ms: 1.7e12, feed_ms: 1.7e12 };
            let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
            assert_eq!(scene.vector_map.shown, shown, "{basemap:?}");
        }
    }

    /// The plain buildings follow the setting (B7), and the key light is aimed at the sun over
    /// them on the flat map (G:2723-2724), not with the setting off.
    #[test]
    fn the_buildings_follow_the_setting_and_aim_the_key_light() {
        let v4 = ViewState { latitude: 59.6519, longitude: 17.9186, zoom: 13.5, pitch: 45.0, bearing: 30.0, position: None };
        let clocks = Clocks { local_ms: 1_782_036_000_000.0, feed_ms: 1_782_036_000_000.0 };
        for on in [true, false] {
            let mut state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, v4, [1440.0, 900.0]);
            state.persisted.settings.buildings = on;
            let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
            assert_eq!(scene.buildings.shown, on);
            assert_eq!(scene.lights, rustradar_map::renderer::scene_lights(&v4, clocks.feed_ms, on));
            assert_eq!(scene.lights == rustradar_map::pipelines::mesh::LightRig::new(), !on, "aimed only over buildings");
        }
    }

    /// The night follows *daylight* (B5) and the labels *labels* and *label size* (B4), on every
    /// basemap (the night layer itself leaves realistic out).
    #[test]
    fn the_night_and_the_labels_follow_their_settings() {
        let clocks = Clocks { local_ms: 1.7e12, feed_ms: 1.7e12 };
        for basemap in [Basemap::Dark, Basemap::Satellite, Basemap::Realistic] {
            for on in [true, false] {
                let mut state = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, V3, [1100.0, 720.0]);
                state.persisted.settings.daylight = on;
                state.persisted.settings.labels = !on;
                state.persisted.settings.label_size = if on { 1.25 } else { 0.85 };
                let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
                assert_eq!(scene.night.shown, on, "{basemap:?}");
                assert_eq!(scene.labels.shown, !on, "{basemap:?}");
                assert_eq!(scene.labels.label_scale, state.persisted.settings.label_size);
                assert!(scene.labels.marked.is_empty(), "no flight, no airport marked");
            }
        }
        // today's defaults: both on, at size 1
        let mut state = AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, V3, [1100.0, 720.0]);
        let scene = SceneBuilder::default().build(&mut state, clocks, 1.0);
        assert!(scene.night.shown && scene.labels.shown);
        assert_eq!(scene.labels.label_scale, 1.0);
    }

    /// B5's acceptance through the whole app (`--screenshot --view 0,180,1.9 --now <equinox noon
    /// UTC>`, default settings): the antipode of the sun is ocean in full night, the dark sea
    /// `(30,30,30)` four times toward `(2,6,14)` at 46/255, `(15,17,21)` ± 2; at `--view 0,0,1.9`
    /// the sea under the sun keeps `(30,30,30)`.
    #[test]
    fn an_equinox_noon_shot_has_the_night_opposite_the_sun() {
        let gpu = match Gpu::headless() {
            Ok(gpu) => gpu,
            Err(GpuError::NoAdapter(e)) => {
                eprintln!("skipped: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        for (lon, expected) in [(180.0, [15, 17, 21]), (0.0, [30, 30, 30])] {
            let options = Options {
                view: Some(ViewState { latitude: 0.0, longitude: lon, zoom: 1.9, pitch: 0.0, bearing: 0.0, position: None }),
                basemap: Some(Basemap::Dark),
                size: Some([1100, 720]),
                scale: Some(1.0),
                now_ms: Some(1_774_008_000_000.0),
                ..Default::default()
            };
            let shot = headless::render_on(&gpu, &options).expect("the shot");
            let centre = shot.image.pixel(550, 360);
            eprintln!("0,{lon},1.9 at equinox noon: centre {centre:?}");
            assert!((0..3).all(|i| i32::from(centre[i]).abs_diff(expected[i]) <= 2), "0,{lon}: {centre:?}, expected {expected:?}");
        }
    }

    /// The share of `image`'s pixels in the rectangle `[x0, x1) × [y0, y1)` that are `rgb`.
    fn share(image: &Image, [x0, y0, x1, y1]: [u32; 4], rgb: [u8; 3]) -> f64 {
        let mut hits = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                hits += usize::from(image.pixel(x, y)[..3] == rgb);
            }
        }
        hits as f64 / f64::from((x1 - x0) * (y1 - y0))
    }

    /// `--screenshot --view 59.33,18.07,9 --basemap dark` from the committed fixtures, through the
    /// whole app (state, scene builder, map, UI pass): the Stockholm region's sea and lakes in the
    /// vector map's water `(30,30,30)` over the land `(50,50,50)`, with its roads.
    #[test]
    fn a_dark_shot_draws_the_vector_map_from_the_fixtures() {
        let gpu = match Gpu::headless() {
            Ok(gpu) => gpu,
            Err(GpuError::NoAdapter(e)) => {
                eprintln!("skipped: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let options = Options {
            view: Some(V3),
            basemap: Some(Basemap::Dark),
            size: Some([1100, 720]),
            scale: Some(1.0),
            now_ms: Some(1_782_036_000_000.0),
            ..Default::default()
        };
        let shot = headless::render_on(&gpu, &options).expect("the shot");
        assert!(shot.missing_tiles.is_empty(), "the fixtures lack {:?}", shot.missing_tiles);
        // the map away from the UI's corners
        let map = [300, 100, 1000, 650];
        let (sea, land) = (share(&shot.image, map, [30, 30, 30]), share(&shot.image, map, [50, 50, 50]));
        let roads = share(&shot.image, map, [88, 88, 88]) + share(&shot.image, map, [66, 66, 66]);
        eprintln!("sea {sea:.3}, land {land:.3}, roads {roads:.4}");
        assert!(sea > 0.2 && land > 0.3, "sea {sea}, land {land}");
        assert!(roads > 0.001, "roads {roads}");
    }

    /// A picture of `docs/port/reference/`, as an RGBA image.
    fn reference(name: &str) -> Image {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().expect("a PNG");
        let mut buf = vec![0; reader.output_buffer_size().expect("its size")];
        let info = reader.next_frame(&mut buf).expect("its pixels");
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
            png::ColorType::Rgb => buf[..info.buffer_size()].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            other => panic!("{name}: {other:?}"),
        };
        Image { width: info.width, height: info.height, rgba }
    }

    /// `--screenshot --view 59.6519,17.9186,13.5,45,30 --basemap dark --size 1440x900` (V4) through
    /// the whole app with the default settings (*buildings* on): the plain buildings (B7) stand
    /// where today's picture has them, and the hangar roof is lit as today's, by the key light this
    /// builder aims at the sun (G:2723-2724).
    #[test]
    fn a_dark_shot_at_v4_draws_the_buildings_lit_as_today() {
        let gpu = match Gpu::headless() {
            Ok(gpu) => gpu,
            Err(GpuError::NoAdapter(e)) => {
                eprintln!("skipped: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let v4 = ViewState { latitude: 59.6519, longitude: 17.9186, zoom: 13.5, pitch: 45.0, bearing: 30.0, position: None };
        let options = Options {
            view: Some(v4),
            basemap: Some(Basemap::Dark),
            size: Some([1440, 900]),
            scale: Some(1.0),
            now_ms: Some(1_782_036_000_000.0),
            ..Default::default()
        };
        let shot = headless::render_on(&gpu, &options).expect("the shot");
        assert!(shot.missing_tiles.is_empty(), "the fixtures lack {:?}", shot.missing_tiles);
        let today = reference("dark-v4-1440x900.png");
        // a building's pixel is bluish, as the building colour lit any way is; no ground of the
        // dark map nor a shadow on it is
        let is_building = |p: [u8; 4]| i32::from(p[2]) - i32::from(p[0]) >= 9;
        // the map in today's picture, away from its sidebar, map controls and mode bar
        let (mut theirs, mut ours, mut both) = (0usize, 0usize, 0usize);
        for y in 80..880 {
            for x in 360..1370 {
                let (t, o) = (is_building(today.pixel(x, y)), is_building(shot.image.pixel(x, y)));
                theirs += usize::from(t);
                ours += usize::from(o);
                both += usize::from(t && o);
            }
        }
        let (kept, back) = (both as f64 / theirs.max(1) as f64, both as f64 / ours.max(1) as f64);
        eprintln!("V4 through the app: today's buildings {theirs} px, ours {ours} px, {both} in both ({kept:.3}, {back:.3})");
        assert!(theirs > 10_000 && kept > 0.95 && back > 0.95, "{theirs}, {kept}, {back}");
        let (theirs, mine) = (today.pixel(1060, 760), shot.image.pixel(1060, 760));
        assert!((0..3).all(|i| theirs[i].abs_diff(mine[i]) <= 1), "the hangar roof: {theirs:?} today, {mine:?} here");
    }
}
