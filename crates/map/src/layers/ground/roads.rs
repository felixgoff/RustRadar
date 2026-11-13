//! The survey's road widths (item R9; doc 04 §6.7; `road-widths.ts`, `withSurvey` in
//! `realistic-ground.ts:415-431`, G:2658-2661): close in on the realistic flat map, Sweden's
//! numbered roads as wide as Trafikverket's road database (NVDB) has them, and gravel where they
//! are gravel, on the city ground's tiles.
//!
//! What today's app does, and so this module:
//! - **Asking** (`wantRoadWidths`, G:2658: `if (flat && zoom >= ROAD_WIDTHS_ZOOM)` on the realistic
//!   map): each frame the city ground draws ([`super::city::GroundCity`]: the realistic flat map)
//!   at zoom [`ROAD_WIDTHS_ZOOM`] or more, the backend's 2 km squares round the view's centre are
//!   asked for once each (`scene::road_widths::RoadWidths::want`: the square there and those
//!   within a kilometre or so, in Sweden only). Here through the tile fetcher as the source
//!   [`ROAD_WIDTHS`] ([`request`]: the square's middle, which the app's fetcher hands to
//!   `backend::roads::widths`, with the user's Trafikverket key; none without one), the answer a
//!   JSON list of `RoadPiece`s ([`pieces_of`]). A square that answered nothing is not asked again,
//!   one that failed is asked again when the view comes back (`.catch(() => asked.delete)`).
//! - **Matching** (`withRoadWidths`, `stretchAt`): once stretches have come, each car road of a
//!   flat-map ground tile (OpenMapTiles' classes in `ROADS` but rail, transit, pier and track;
//!   Streets GL's kinds in `ROADS` but track: `scene::ground::is_car_road`) takes the width and
//!   surface of the nearest stretch within 10 m of its middle segment's middle running within 30°
//!   of its way, as the columns `nvdbWidth` and `nvdbGravel` ([`Survey::apply`], the hook
//!   `scene::tess::ground::tessellate_surveyed` calls before the tile's bridges are split off);
//!   `scene::ground::line_of`'s `asSurveyed` then draws such a road that wide, and as dirt where
//!   the stretch is not paved, unless the road's own tags say its width or surface.
//! - **Drawing again** (`roadWidthsVersion`, the layers' `roadsVersion` prop): each time
//!   stretches come, the version moves on and the ground's tiles take the new widths
//!   ([`Roads::prepare`] says so; the city ground decodes its selected tiles again).
//!
//! Deviations, invisible: deck re-derives a tile's line attributes from the tile it holds; here a
//! changed version makes the city ground ask for its selected tiles again (from the disk cache)
//! and decode them anew on the worker pool, their old content drawn until the new is in.
//!
//! The fixtures hold no stretches (today's app asks with the user's key, and no key may be in the
//! fixtures): the shots draw the roads without widths, as today's app does without a key; the
//! tests below make stretches over fixture roads in code.

use std::sync::{Arc, PoisonError, RwLock};

use rustradar_core::model::RoadPiece;
use rustradar_scene::bridges::{Lines, Properties};
use rustradar_scene::ground::is_car_road;
use rustradar_scene::road_widths::{self, Cell, RoadWidths};

use crate::io::{Fetch, FetchResult};
use crate::layers::{PrepareCx, TileIndex};

/// The source the stretches are asked for by (`backend::roads::widths` in the app's fetcher).
pub const ROAD_WIDTHS: &str = "road-widths";

/// From this zoom on the flat map, the stretches round the view's centre are asked for
/// (`ROAD_WIDTHS_ZOOM`, G:1005).
pub const ROAD_WIDTHS_ZOOM: f64 = 14.0;

/// A square's request as a tile index of [`ROAD_WIDTHS`]: zoom 0 (no tile of a map), its middle's
/// longitude and latitude in hundred-thousandths of a degree from −180° and −90° (a metre or so:
/// the backend finds the square from any point well inside it).
pub fn request(cell: &Cell) -> TileIndex {
    let [lon, lat] = cell.centre;
    TileIndex::new(((lon + 180.0) * 1e5).round() as u32, ((lat + 90.0) * 1e5).round() as u32, 0)
}

/// The point a [`request`] asks about, `[lon, lat]`.
pub fn centre_of(request: TileIndex) -> [f64; 2] {
    [f64::from(request.x) / 1e5 - 180.0, f64::from(request.y) / 1e5 - 90.0]
}

/// A square's answer: the backend's `RoadPiece`s as JSON (none when it had nothing).
pub fn pieces_of(answer: &[u8]) -> Result<Vec<RoadPiece>, String> {
    serde_json::from_slice(answer).map_err(|e| format!("road widths: {e}"))
}

/// The stretches known, shared with the worker pool's decodes.
#[derive(Clone, Default)]
pub struct Survey(Arc<RwLock<RoadWidths>>);

impl Survey {
    /// `withSurvey`: a flat-map tile's lines (`tile`, in its own units, loaders.gl's binary lines)
    /// given the columns `nvdbWidth` and `nvdbGravel` where a car road lies on a stretch; nothing
    /// while no stretches have come.
    pub fn apply(&self, tile: TileIndex, lines: &mut Lines) {
        let widths = self.0.read().unwrap_or_else(PoisonError::into_inner);
        let view = road_widths::Lines::<Properties> {
            positions: &lines.positions,
            size: lines.size,
            path_indices: &lines.path_indices,
            feature_ids: &lines.feature_ids,
            properties: &lines.properties,
        };
        let Some(columns) = widths.with_road_widths(&view, (tile.x, tile.y, tile.z), is_car_road, &mut None) else { return };
        lines.numeric_props.insert("nvdbWidth".into(), columns.nvdb_width.iter().map(|&w| f64::from(w)).collect());
        lines.numeric_props.insert("nvdbGravel".into(), columns.nvdb_gravel.iter().map(|&g| f64::from(g)).collect());
    }

    /// The stretches' version (0 while none have come).
    pub fn version(&self) -> u64 {
        self.0.read().unwrap_or_else(PoisonError::into_inner).version()
    }

    /// The stretches known (tests).
    pub fn pieces(&self) -> Vec<RoadPiece> {
        self.0.read().unwrap_or_else(PoisonError::into_inner).pieces().to_vec()
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, RoadWidths> {
        self.0.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The squares asked for and the stretches come: the city ground's (one per layer, as today's
/// module state is one per page).
#[derive(Default)]
pub struct Roads {
    survey: Survey,
    /// Squares on their way.
    asking: Vec<(Cell, Fetch)>,
}

impl Roads {
    pub fn new() -> Self {
        Self::default()
    }

    /// The stretches, for the city ground's decode step.
    pub fn survey(&self) -> Survey {
        self.survey.clone()
    }

    /// Whether squares are on their way.
    pub fn pending(&self) -> bool {
        !self.asking.is_empty()
    }

    /// This frame (the city ground drawing): asks for the squares round the view's centre from
    /// zoom [`ROAD_WIDTHS_ZOOM`] on, takes the answers come. Whether new stretches came (the
    /// ground's tiles to take them).
    pub fn prepare(&mut self, cx: &PrepareCx<'_>) -> bool {
        let view = &cx.view.view;
        if !cx.view.is_globe() && view.zoom >= ROAD_WIDTHS_ZOOM {
            let cells = self.survey.write().want(view.longitude, view.latitude);
            if !cells.is_empty() {
                let fetcher = cx.tiles.fetcher();
                for cell in cells {
                    let fetch = Fetch::start(&*fetcher, ROAD_WIDTHS.into(), request(&cell));
                    self.asking.push((cell, fetch));
                }
            }
        }
        self.collect()
    }

    /// The answers come: stretches added (whether any were), a square that failed forgotten.
    fn collect(&mut self) -> bool {
        let mut came = false;
        let mut i = 0;
        while i < self.asking.len() {
            let Some(answer) = self.asking[i].1.poll() else {
                i += 1;
                continue;
            };
            let (cell, _) = self.asking.swap_remove(i);
            came |= self.take(&cell, answer);
        }
        came
    }

    /// One square's answer.
    fn take(&mut self, cell: &Cell, answer: FetchResult) -> bool {
        let pieces = match answer {
            Ok(Some(bytes)) => pieces_of(&bytes),
            Ok(None) => Ok(Vec::new()),
            Err(e) => Err(format!("{e:?}")),
        };
        match pieces {
            Ok(pieces) => self.survey.write().arrived(pieces),
            Err(e) => {
                log::debug!("road widths for {:?}: {e}", cell.centre);
                self.survey.write().failed(cell.key);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use glam::DVec2;
    use rustradar_core::geo::Basemap;
    use rustradar_core::mvt::{self, GeomType};
    use rustradar_core::weather::PRESETS;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::ground::{line_of, SGL_GROUND_LAYERS};
    use rustradar_scene::tess::ground::{tessellate, tessellate_surveyed};
    use serde_json::Value;

    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{DirTiles, FnFetcher, TileFetcher};
    use crate::layers::{earth, ground, hillshade, sky};
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:00 UTC: noon in Stockholm (the reference pictures' clock).
    const MIDSUMMER: f64 = 1_782_036_000_000.0;
    /// V5 (plan section 8): Stockholm city, pitched, at today's pictures' size.
    const V5: ViewState = ViewState { latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0, position: None };
    const SIZE: [f64; 2] = [1440.0, 900.0];
    /// Streets GL's zoom 16 tile under V5's centre.
    const V5_TILE: TileIndex = TileIndex::new(36057, 19273, 16);
    /// The test stretch's width, metres: no road of the tile's is as wide.
    const WIDTH: f32 = 13.75;

    fn shots_dir() -> PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// A point of `tile` (its own units) as longitude and latitude, as `withRoadWidths` turns it.
    fn lon_lat(tile: TileIndex, [u, v]: [f64; 2]) -> [f64; 2] {
        let n = 2f64.powi(tile.z as i32);
        [((f64::from(tile.x) + u) / n) * 360.0 - 180.0, ((PI * (1.0 - (2.0 * (f64::from(tile.y) + v)) / n)).sinh().atan() * 180.0) / PI]
    }

    /// A car road of V5's Streets GL tile, its own width and surface not mapped, nearest to the
    /// view's centre: its path (tile units) and a stretch lying on its middle segment (the one
    /// `withRoadWidths` matches by), unpaved and [`WIDTH`] wide.
    fn a_road_and_its_stretch() -> (Vec<[f64; 2]>, RoadPiece) {
        let bytes = DirTiles::fixtures().read(crate::io::STREETS_GL, V5_TILE).unwrap().expect("V5's tile");
        let centre = {
            let n = 2f64.powi(16);
            let [x, y] = rustradar_scene::camera::lng_lat_to_world([V5.longitude, V5.latitude]);
            [x / 512.0 * n - f64::from(V5_TILE.x), (512.0 - y) / 512.0 * n - f64::from(V5_TILE.y)]
        };
        let metres = rustradar_core::geo::EARTH_CIRCUMFERENCE_M * V5.latitude.to_radians().cos() / 2f64.powi(16);
        let mut best: Option<(f64, Vec<[f64; 2]>)> = None;
        for layer in mvt::layers_named(&bytes, &["highways"]) {
            for feature in layer.features() {
                if feature.kind != GeomType::LineString {
                    continue;
                }
                let mut p = feature.json_properties();
                p.insert("layerName".into(), Value::from(layer.name));
                let mapped = p.get("width").and_then(Value::as_f64).is_some_and(|w| w > 0.0) || matches!(p.get("surface"), Some(Value::String(_)));
                if !is_car_road(&p) || mapped {
                    continue;
                }
                for line in feature.lines() {
                    let path: Vec<[f64; 2]> = line.iter().map(|&(x, y)| [x / layer.extent, y / layer.extent]).collect();
                    if path.len() < 2 {
                        continue;
                    }
                    let mid = (path.len() - 1) / 2;
                    let (a, b) = (DVec2::from(path[mid]), DVec2::from(path[mid + 1]));
                    if (b - a).length() * metres < 20.0 {
                        continue;
                    }
                    let d = ((a + b) / 2.0 - DVec2::from(centre)).length();
                    if best.as_ref().is_none_or(|(e, _)| d < *e) {
                        best = Some((d, path));
                    }
                }
            }
        }
        let (_, path) = best.expect("a car road near V5's centre");
        let mid = (path.len() - 1) / 2;
        let piece = RoadPiece { line: vec![lon_lat(V5_TILE, path[mid]), lon_lat(V5_TILE, path[mid + 1])], width: WIDTH, paved: false };
        (path, piece)
    }

    fn survey_of(pieces: Vec<RoadPiece>) -> Survey {
        let survey = Survey::default();
        assert!(survey.write().arrived(pieces));
        survey
    }

    #[test]
    fn a_request_names_its_square_s_middle() {
        for (lon, lat) in [(18.0686, 59.3293), (13.19, 55.7), (24.4, 69.1), (10.6, 55.01)] {
            let cell = road_widths::cell_of(lon, lat);
            let back = centre_of(request(&cell));
            assert!((back[0] - cell.centre[0]).abs() <= 0.6e-5 && (back[1] - cell.centre[1]).abs() <= 0.6e-5, "{back:?} {:?}", cell.centre);
            assert_eq!(road_widths::cell_of(back[0], back[1]).key, cell.key);
            assert_eq!(request(&cell).z, 0);
        }
        assert_eq!(pieces_of(b"[]"), Ok(Vec::new()));
        let json = br#"[{"line":[[18.0,59.3],[18.01,59.31]],"width":7.5,"paved":false}]"#;
        assert_eq!(pieces_of(json), Ok(vec![RoadPiece { line: vec![[18.0, 59.3], [18.01, 59.31]], width: 7.5, paved: false }]));
        assert!(pieces_of(b"<html>").is_err());
        assert_eq!(ROAD_WIDTHS_ZOOM, 14.0);
    }

    /// `withSurvey` on V5's Streets GL tile: with no stretch the tile is as before; with one over a
    /// road's middle segment, that road (every segment of its path) is the stretch's width and,
    /// unpaved, dirt (`paint(34)`), its pavement as before; nothing else changes.
    #[test]
    fn a_stretch_over_a_fixture_road_makes_it_as_wide_as_surveyed() {
        let bytes = DirTiles::fixtures().read(crate::io::STREETS_GL, V5_TILE).unwrap().unwrap();
        let (path, piece) = a_road_and_its_stretch();
        let never = || false;
        let own = tessellate(V5_TILE, &bytes, &SGL_GROUND_LAYERS, true, &never).unwrap();
        let none = Survey::default();
        let unsurveyed = tessellate_surveyed(V5_TILE, &bytes, &SGL_GROUND_LAYERS, true, &|t, l| none.apply(t, l), &never).unwrap();
        assert_eq!(unsurveyed, own, "no stretches: the tile as it was");
        let survey = survey_of(vec![piece]);
        let surveyed = tessellate_surveyed(V5_TILE, &bytes, &SGL_GROUND_LAYERS, true, &|t, l| survey.apply(t, l), &never).unwrap();
        // the road's segments: as wide as the stretch, dirt
        let wide: Vec<_> = surveyed.lines.iter().filter(|s| s.width == WIDTH).collect();
        assert!(own.lines.iter().all(|s| s.width != WIDTH));
        assert!(wide.len() >= path.len() - 1, "{} segments for a path of {}", wide.len(), path.len());
        assert!(wide.iter().all(|s| s.color == [255, 255, 255, 34]), "{:?}", wide.iter().map(|s| s.color).collect::<Vec<_>>());
        let on_path = |s: &&rustradar_scene::geom::path::PathSegment| {
            path.iter().any(|q| (f64::from(s.start[0]) - q[0]).abs() < 1e-6 && (f64::from(s.start[1]) - q[1]).abs() < 1e-6)
        };
        assert!(wide.iter().all(on_path), "only the matched road");
        // the rest as it was: as many segments, the others' widths and paints the same
        assert_eq!(surveyed.lines.len(), own.lines.len());
        assert_eq!(surveyed.edges.len(), own.edges.len());
        let changed = surveyed.lines.iter().zip(&own.lines).filter(|(a, b)| a != b).count();
        assert_eq!(changed, wide.len());
        // (its own line as Streets GL has it, unsurveyed, for the record)
        let _ = line_of;
    }

    /// The realistic map of `view` (sky, earth, ground, relief), sunny at midsummer noon.
    fn realistic(view: ViewState) -> SceneInput {
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Realistic);
        scene.local_now_ms = MIDSUMMER;
        scene.feed_now_ms = MIDSUMMER;
        scene.earth = earth::Input::SHOWN;
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.sunny), daylight: true };
        scene.ground = ground::Input::SHOWN;
        scene.hillshade = hillshade::Input { shown: true, terrain: false };
        scene
    }

    /// The fixtures, the squares' answers from `pieces` (each square its own, as JSON), every
    /// square asked noted in `asked`.
    fn fetcher(pieces: Vec<RoadPiece>, asked: Arc<Mutex<Vec<TileIndex>>>) -> Arc<dyn TileFetcher> {
        let dir = DirTiles::fixtures();
        Arc::new(FnFetcher(move |source: &str, tile: TileIndex| -> FetchResult {
            if source != ROAD_WIDTHS {
                return dir.read(source, tile);
            }
            asked.lock().unwrap().push(tile);
            let [lon, lat] = centre_of(tile);
            let key = road_widths::cell_of(lon, lat).key;
            let own: Vec<&RoadPiece> = pieces.iter().filter(|p| road_widths::cell_of(p.line[0][0], p.line[0][1]).key == key).collect();
            Ok(Some(serde_json::to_vec(&own).unwrap()))
        }))
    }

    /// From zoom 14 on the realistic flat map, the squares round the view's centre are asked for,
    /// once each: not under 14, not on the globe, not on the dark map.
    #[test]
    fn the_squares_round_the_centre_are_asked_for_once_from_zoom_14() {
        let Some(gpu) = gpu() else { return };
        let asked = Arc::new(Mutex::new(Vec::new()));
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        renderer.set_tiles(fetcher(Vec::new(), asked.clone()), None);
        let mut take = |scene: SceneInput| -> Vec<TileIndex> {
            render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            std::mem::take(&mut *asked.lock().unwrap())
        };
        let mut dark = realistic(V5);
        dark.basemap = Basemap::Dark;
        assert_eq!(take(dark), []);
        assert_eq!(take(realistic(ViewState { zoom: 13.9, ..V5 })), []);
        assert_eq!(take(realistic(ViewState { zoom: 11.0, pitch: 0.0, ..V5 })), []);
        let first = take(realistic(V5));
        let here = road_widths::cell_of(V5.longitude, V5.latitude);
        assert!(first.contains(&request(&here)), "{first:?}");
        assert!((1..=5).contains(&first.len()), "{first:?}");
        let keys: std::collections::HashSet<_> = first.iter().map(|&t| road_widths::cell_of(centre_of(t)[0], centre_of(t)[1]).key).collect();
        assert_eq!(keys.len(), first.len(), "each square once");
        // the same view again: nothing more (none of the squares had anything, so none is asked again)
        assert_eq!(take(realistic(V5)), []);
    }

    /// The survey through the whole map at V5: a stretch over a Streets GL road near the centre
    /// comes, the city ground decodes its tiles again with it, and that road is drawn as wide and
    /// as gravelled as the stretch: pixels change along it, none far from it.
    #[test]
    fn at_v5_a_surveyed_road_is_drawn_as_wide_as_the_survey_has_it() {
        let Some(gpu) = gpu() else { return };
        let (path, piece) = a_road_and_its_stretch();
        let shoot = |pieces: Vec<RoadPiece>| -> Image {
            let asked = Arc::new(Mutex::new(Vec::new()));
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(fetcher(pieces, asked.clone()), None);
            let image = render_with(&gpu, &mut renderer, &realistic(V5), 1.0).expect("rendered");
            assert!(!renderer.pending(), "{:?}", renderer.pending_layers());
            assert!(!asked.lock().unwrap().is_empty());
            image
        };
        let (with, without) = (shoot(vec![piece.clone()]), shoot(Vec::new()));
        with.save_png(&shots_dir().join("r9-v5-road-widths.png")).expect("png");
        without.save_png(&shots_dir().join("r9-v5-without-road-widths.png")).expect("png");
        let view = ViewFrame::new(&V5, SIZE, 1.0, 0.0);
        let frame = rustradar_scene::geom::path::LocalMercator::tile(V5_TILE);
        let metres = rustradar_core::geo::EARTH_CIRCUMFERENCE_M * V5.latitude.to_radians().cos() / 2f64.powi(16);
        let distance = |p: DVec2| {
            path.windows(2)
                .map(|w| {
                    let (a, b) = (DVec2::from(w[0]), DVec2::from(w[1]));
                    let t = ((p - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0);
                    (a + (b - a) * t - p).length() * metres
                })
                .fold(f64::INFINITY, f64::min)
        };
        let (mut near, mut far, mut checked) = (0, 0, 0);
        for y in (300..900).step_by(2) {
            for x in (0..1440).step_by(2) {
                let [lon, lat] = view.viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
                let [wx, wy] = rustradar_scene::camera::lng_lat_to_world([lon, lat]);
                let d = distance(DVec2::from(frame.from_world(DVec2::new(wx, wy))));
                let (a, b) = (with.pixel(x, y), without.pixel(x, y));
                let changed = (0..3).any(|c| a[c].abs_diff(b[c]) > 6);
                checked += 1;
                if changed && d <= f64::from(WIDTH) {
                    near += 1;
                } else if changed && d > 30.0 {
                    far += 1;
                }
            }
        }
        // where the stretch lies: dirt where asphalt was, browner
        let [lon, lat] = [(piece.line[0][0] + piece.line[1][0]) / 2.0, (piece.line[0][1] + piece.line[1][1]) / 2.0];
        let [x, y, _] = view.viewport.project([lon, lat, 0.0]);
        let (a, b) = (with.pixel(x as u32, y as u32), without.pixel(x as u32, y as u32));
        eprintln!("{near} pixels changed along the road, {far} far from it, of {checked}; at its middle {a:?} where it was {b:?}");
        assert!(near > 200, "{near}");
        assert!(far * 50 < near, "{far} far, {near} near");
        let warm = |p: [u8; 4]| i32::from(p[0]) - i32::from(p[2]);
        assert!(warm(a) > warm(b) + 8, "{a:?} against {b:?}");
    }
}
