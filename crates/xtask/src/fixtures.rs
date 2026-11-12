//! `cargo xtask fixtures`: download the real data the map's headless shots and tests draw from
//! (plan E16, T3, X1 and section 8) into `crates/map/tests/data` ([`data_dir`]), with `NOTICE`
//! files naming their sources and licences. A shot never touches the network; it reads the tiles
//! through the map's file-backed `io::DirTiles` (`crates/map/tests/common/fixture_tiles.rs` for
//! the map's tests), and a tile the set does not hold is simply not drawn.
//!
//! The layout:
//! - `tiles/` ([`fixtures_dir`]): one folder per tile source, named as the map's layers name the
//!   source in a `FetchRequest`, each tile at `<source>/<z>/<x>/<y>.<ext>`, and `tiles/NOTICE`.
//! - beside it, what is not a map tile ([`DataPlan`], `NOTICE`): `clouds/` (R10's picture),
//!   `canopy/` (R7's recorded COG ranges), `overture/` (R6's PMTiles extract), `radar/` (W4's
//!   recorded measured volumes).
//!
//! What is fetched ([`plan`], [`data_plan`]): for each covered list of views ([`COVERS`]: the
//! views, what they hold ([`Cover`]) and the canvas sizes in CSS pixels they are covered at), every
//! tile the map's own selection asks of the view's sources ([`view_tiles`]); and OpenFreeMap's
//! whole world at zooms 0 to 2 (V1's globe, B5's night views):
//! - [`VIEWS`], the views V3 to V6 of plan section 8 ([`Cover::ALL`]).
//! - [`BASEMAP_VIEWS`], V1, V2, V7 and V8 (the dark and satellite shots of stages 3 and 4: B2, B4,
//!   B6, A4): OpenFreeMap and Esri imagery ([`Cover::BASEMAPS`]).
//! - [`DARK_VIEWS`], the dark map's views at the projection swap (B2: V3 at zooms 11.9 and 12.1;
//!   A5: V4 at the same zooms) and B5's night views: OpenFreeMap only ([`Cover::DARK`]).
//! - [`RADAR_VIEWS`] and [`PITCHED_RADAR_VIEWS`], V2 flat and pitched 45° (W1 to W3): RainViewer
//!   only ([`Cover::RADAR`]).
//! - [`REALISTIC_VIEWS`] (V4, V5) and [`CLOSE_UP_VIEWS`] (a park for R7, a station for R9) in
//!   Stockholm on the realistic map ([`Cover::REALISTIC`]): its ground and trees,
//!   OpenRailwayMap's tracks and platforms, land cover, and close to the centre
//!   ([`close_up_tiles`]) canopy heights and Overture's buildings.
//! - [`RELIEF_VIEWS`], the Alps from above (R12): the maps, imagery and hillshade
//!   ([`Cover::RELIEF`]).
//! - [`TERRAIN_VIEWS`], Zermatt pitched 60° (Z2 to Z4: a village with buildings on the slopes):
//!   the maps, the realistic ground, hillshade and the 3D surface ([`Cover::TERRAIN`]).
//! - [`SKYLINE_VIEWS`], Zermatt pitched 75° (Z1: distant ranges against the sky on the realistic
//!   map): hillshade and the 3D surface only ([`Cover::SKYLINE`]).
//!
//! A view on relief is covered with the terrain off and on: the camera's target raised to the
//! ground under it ([`View::ground`], doc 04 §14.4) and the tiles' bounding volumes up to its
//! highest ground ([`View::peak`], the terrain's growing `zRange`, doc 04 §14.3).
//!
//! The tile sources:
//! - `openfreemap/`: OpenFreeMap's vector tiles, deck's rule for `map-*`, `label-tiles` and the
//!   true-width runways (`aeroway-true-*`; all 512 px, zooms 0 to 14: [`VECTOR_TILES`]), plus on
//!   the flat map the distance LOD of the plain buildings ([`city_lod`]; dark and satellite
//!   draw its zooms 13 and 14, G:1047-1053) and of the realistic ground ([`ground_lod`]) at their
//!   zooms up to 14, and the trees' z14 tiles from [`TREES_ZOOM`] to [`SGL_FROM_ZOOM`]. The URL
//!   template is `tiles[0]` of the TileJSON at [`OPENFREEMAP_TILEJSON`], read at run time (it
//!   names the planet build), never hard-coded (doc 01 §5.6); the TileJSON is kept as
//!   `openfreemap/tilejson.json`.
//! - `esri/`: Esri World Imagery for the satellite basemap ([`IMAGERY_TILES`]).
//! - `terrarium/`: Mapzen's terrarium heights the hillshade shades ([`HILLSHADE_TILES`]), and
//!   the 3D surface's stand-ins where Mapterhorn has no tile ([`fallbacks`]).
//! - `mapterhorn/`: Mapterhorn's terrarium WebP heights of the 3D surface ([`surface_lod`]) and
//!   of the camera's ground ([`ground_tile`]).
//! - `rainviewer/`: one RainViewer radar frame, the latest past one of its frame list (the frame
//!   list is kept as `rainviewer/weather-maps.json`), at the radar's tiles ([`RADAR_TILES`]).
//! - `streets-gl/`: the Streets GL z16 tiles of the realistic map on the flat map: those the two
//!   distance LODs take close in (their deepest level is Streets GL's at any zoom), and from
//!   [`SGL_FROM_ZOOM`] the trees' tiles around the view's centre ([`tree_tiles`]).
//! - `openrailwaymap/`: OpenRailwayMap's tracks of the realistic ground (deck's rule, zooms 7 to
//!   12: `ground_rails::RAIL_TILES`) and its stations' platforms on the flat map (zoom 15 within
//!   4 km: `ground_rails::platform_lod`), fetched through the app's own backend
//!   (`backend::rail::tile`, with the identification OpenRailwayMap asks of the app).
//! - `nmd/` and `worldcover/`: the land cover's pictures of a tile (doc 04 §11): NMD's WMS for a
//!   tile in Sweden, WorldCover's mosaic for every tile from zoom 6 (today's app asks it where NMD
//!   has nothing or not all; the set holds it for every tile, so either answer is there). The
//!   mosaic's registration is kept as `worldcover/register.json`. Taken for the land cover
//!   layer's tiles (256 px, zooms 10 to 14, deck's rule) and the close-up tree tiles.
//!
//! The data beside the tiles ([`data_plan`]):
//! - `clouds/worldcloudmap_ir108.jpg`: EUMETSAT's world cloud picture as `CLOUD_URL` asks for it
//!   (R10), with `clouds/worldcloudmap_ir108.json` saying when it was fetched.
//! - `canopy/<quadkey>.json` and `canopy/<quadkey>/<start>-<end>.bin`: the byte ranges of a
//!   canopy-height Cloud-Optimised GeoTIFF that `canopy(tile)` reads for the close-up tiles (R7):
//!   the file's first bytes with all its IFDs ([`HEAD_BYTES`] at least) and the internal tiles
//!   under each tile's window ([`canopy_ranges`]). The JSON names the file's URL, its length and
//!   the ranges held, `[start, end)`; a reader serves a read that lies within one of them.
//! - `overture/buildings.pmtiles`: a PMTiles v3 archive holding Overture's z14 building tiles
//!   under the close-up tiles (R6), copied byte for byte from the release the catalogue names
//!   (kept as `overture/catalog.json`), with one root directory ([`write_extract`]).
//! - `radar/mrms/`: one measured volume as the backend served it to today's frontend (W4):
//!   `volume.json` (`radar_volumes`' `RadarVolume`) and the blobs `radar_image` answers,
//!   `ground.bin`, `coverage.bin` and `<km>.bin` for each height with shapes
//!   (`backend::radar::contour`'s encoding), recorded with the backend's own fetch.
//! - `radar/dwd/`: DWD's WN analysis as DWD publishes it now ([`record_dwd`]): the backend's
//!   `dwd::fetch` looks for `WN*.tar.bz2` runs of RADOLAN binaries, which DWD no longer
//!   publishes (its folder holds `composite_wn_<date>_<time>.tar` of ODIM HDF5 members), so no
//!   volume can be recorded through it; the newest run's analysis member (`…_000-hd5`) is kept
//!   as it came, with `source.json` naming the run.
//!
//! Not fetched: NVDB road widths (R9): today's app asks Trafikverket's API with the user's key,
//! which the set must never hold; the backend's tiles of other kinds (building LOD, signal
//! blocks) belong to their stages.
//!
//! Each tile is written as `<source>/<z>/<x>/<y>.<ext>` with exactly the bytes the server sent,
//! the HTTP compression undone (what `backend::tiles` hands the map). A tile the server has
//! nothing for (404 or 204, which the map settles as "nothing there") is written as an empty
//! `<y>.none`. The whole folder must stay under [`BUDGET_BYTES`]; a download past it is refused.
//!
//! Downloads go through the `curl` command (Windows 10 and later, macOS and Linux ship it): it
//! honours `HTTPS_PROXY` and the system's certificates; OpenRailwayMap's tiles and the radar
//! volumes go through the backend's code, as the app fetches them. A plain run tops the
//! committed set up ([`download`]): it fetches only the planned tiles and data the set lacks,
//! from the set's own OpenFreeMap build (read from the kept TileJSON, and checked to still serve
//! the bytes the set holds), so the tiles already committed do not change. A RainViewer frame
//! lives two hours: when the set's frame is gone, the radar's folder is fetched whole from the
//! day's frame. `--fresh` fetches everything anew from the day's build and frame. Either way the
//! files go to a staging folder first and reach the set only when every one has arrived. The
//! tests `the_committed_fixtures_hold_the_plan` and `the_committed_data_holds_the_plan` say when
//! the selection code asks for something the committed set lacks.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rustradar_core::geo::meters_per_pixel;
use rustradar_scene::camera::{CAMERA_HEIGHTS, ViewState, viewport_for};
use rustradar_scene::canopy::{is_mask, pick_image, place_in_file, window};
use rustradar_scene::clouds::CLOUD_URL;
use rustradar_scene::ground_rails::{RAIL_TILES, platform_lod};
use rustradar_scene::hillshade::{
    GROUND_ZOOM, HILLSHADE_MAX_ZOOM, MAPTERHORN_URL, TERRAIN_ATTRIBUTION, TERRAIN_ATTRIBUTION_FULL, TERRARIUM_MAX_ZOOM,
    TERRARIUM_URL, terrain_surface,
};
use rustradar_scene::landcover::{WORLD_COVER_MIN_ZOOM, WORLD_COVER_REGISTER, WORLD_COVER_SEARCH, in_sweden, mosaic_id, nmd_url, world_cover_url};
use rustradar_scene::landcover_ground;
use rustradar_scene::lod::{Lod, distance_tile_indices};
use rustradar_scene::overture;
use rustradar_scene::streets_gl::{SGL_FROM_ZOOM, SGL_TILES, SGL_ZOOM};
use rustradar_scene::tiles::{IMAGERY_TILES, RADAR_TILES, TileIndex, TilesetOptions, VECTOR_TILES, tile_indices};
use rustradar_scene::trees::{TREE_TILE_ZOOM, tiles_around};

/// OpenFreeMap's TileJSON (G:308); its `tiles[0]` is the URL template of the current planet build.
pub const OPENFREEMAP_TILEJSON: &str = "https://tiles.openfreemap.org/planet";
/// Esri World Imagery's map service (G:309, 483); its tiles are `{z}/{y}/{x}` below `/tile`.
pub const ESRI_IMAGERY: &str = "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer";
/// RainViewer's frame list (G:1117).
pub const RAINVIEWER_MAPS: &str = "https://api.rainviewer.com/public/weather-maps.json";

/// The fixtures' size cap, bytes, tiles and data together (plan E16 asked for under 40 MB for V3
/// to V6; with the views of stages 3 and 4, item T3: under 60 MB; with the tiles and data of
/// stages 8 to 10, item X1: under 80 MB).
pub const BUDGET_BYTES: u64 = 80_000_000;
/// How many downloads run at once.
const PARALLEL: usize = 6;

/// A source's folder name: what the map's layers call it in a `FetchRequest`.
pub const OPENFREEMAP: &str = "openfreemap";
pub const ESRI: &str = "esri";
pub const TERRARIUM: &str = "terrarium";
pub const MAPTERHORN: &str = "mapterhorn";
pub const RAINVIEWER: &str = "rainviewer";
pub const STREETS_GL: &str = "streets-gl";
/// OpenRailwayMap's tracks and platforms (`map::layers::rail::RAIL_TILES`, the app's `railtiles:`).
pub const OPENRAILWAYMAP: &str = "openrailwaymap";
/// Naturvårdsverket's NMD land cover pictures.
pub const NMD: &str = "nmd";
/// ESA WorldCover's land cover pictures.
pub const WORLDCOVER: &str = "worldcover";

/// The marker of a tile the server had nothing for.
pub const NOTHING_EXT: &str = "none";

/// The tiles' folder below the data folder.
pub const TILES: &str = "tiles";
/// The cloud picture and when it was fetched, below the data folder.
pub const CLOUDS_FILE: &str = "clouds/worldcloudmap_ir108.jpg";
pub const CLOUDS_RECORD: &str = "clouds/worldcloudmap_ir108.json";
/// The canopy heights' recorded ranges, below the data folder.
pub const CANOPY: &str = "canopy";
/// Overture's buildings extract and the catalogue that named its release, below the data folder.
pub const OVERTURE_FILE: &str = "overture/buildings.pmtiles";
pub const OVERTURE_CATALOG: &str = "overture/catalog.json";
/// The measured radar volumes, below the data folder, one folder per source.
pub const RADAR: &str = "radar";
/// The sources whose volumes are recorded through the backend.
pub const RADAR_SOURCES: [&str; 1] = ["mrms"];
/// DWD's raw WN analysis, below the data folder.
pub const DWD: &str = "radar/dwd";
/// DWD's WN composites (`backend::dwd`'s `DIRECTORY`).
pub const DWD_DIRECTORY: &str = "https://opendata.dwd.de/weather/radar/composite/wn/";
/// The start of an HDF5 file.
const HDF5_MAGIC: &[u8] = b"\x89HDF\r\n\x1a\n";
/// At least this many of a COG's first bytes are recorded (a reader's first read).
pub const HEAD_BYTES: u64 = 65_536;
/// The most of a COG's first bytes read to find its IFDs.
const MAX_HEAD_BYTES: u64 = 4 << 20;

/// One of the covered views: plan section 8's named ones and the stages' own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub name: &'static str,
    pub latitude: f64,
    pub longitude: f64,
    pub zoom: f64,
    pub pitch: f64,
    pub bearing: f64,
    /// Metres: the ground under the view's centre, where the terrain raises the camera's target
    /// on the flat map (doc 04 §14.4); 0 where there is no relief.
    pub ground: f64,
    /// Metres: the highest ground the view's tiles reach, the terrain's elevation range once its
    /// tiles have loaded (doc 04 §14.3); 0 where there is no relief.
    pub peak: f64,
}

impl View {
    /// A view on flat ground.
    pub const fn at(name: &'static str, latitude: f64, longitude: f64, zoom: f64, pitch: f64, bearing: f64) -> View {
        View { name, latitude, longitude, zoom, pitch, bearing, ground: 0.0, peak: 0.0 }
    }

    /// The view on relief: `ground` under its centre, up to `peak`, metres.
    pub const fn on_relief(self, ground: f64, peak: f64) -> View {
        View { ground, peak, ..self }
    }

    pub fn state(&self) -> ViewState {
        ViewState {
            longitude: self.longitude,
            latitude: self.latitude,
            zoom: self.zoom,
            pitch: self.pitch,
            bearing: self.bearing,
            position: None,
        }
    }

    /// The view as the tile selection meets it: as it is, and on relief on the flat map also
    /// with the terrain on (the camera's target raised to the ground, G:3204-3212).
    pub fn states(&self) -> Vec<ViewState> {
        let mut states = vec![self.state()];
        if self.ground > 0.0 && self.state().is_flat() {
            states.push(ViewState { position: Some([0.0, 0.0, self.ground]), ..self.state() });
        }
        states
    }

    /// The elevation range the 3D surface's tiles' bounding volumes span once its tiles have
    /// loaded (deck's growing `zRange`; before, sea level, a passing selection a settled shot
    /// does not draw): sea level to the peak on relief.
    pub fn z_range(&self) -> Option<[f64; 2]> {
        (self.peak > 0.0).then_some([0.0, self.peak])
    }
}

/// The views the fixtures cover for every source of stages 3 to 7 (plan section 8): Stockholm
/// region, Arlanda pitched, Stockholm city pitched, Gothenburg harbour.
pub const VIEWS: [View; 4] = [
    View::at("V3", 59.33, 18.07, 9.0, 0.0, 0.0),
    View::at("V4", 59.6519, 17.9186, 13.5, 45.0, 30.0),
    View::at("V5", 59.3293, 18.0686, 15.5, 60.0, 20.0),
    View::at("V6", 57.70, 11.93, 13.0, 0.0, 0.0),
];

/// The views the fixtures cover for the dark and satellite maps (plan section 8, stages 3 and
/// 4): the initial globe, Europe, Svalbard, the antimeridian.
pub const BASEMAP_VIEWS: [View; 4] = [
    View::at("V1", 35.0, 10.0, 1.9, 0.0, 0.0),
    View::at("V2", 52.0, 10.0, 4.5, 0.0, 0.0),
    View::at("V7", 78.2, 15.6, 6.0, 0.0, 0.0),
    View::at("V8", 0.0, 179.5, 3.0, 0.0, 0.0),
];

/// The views the fixtures cover for the dark map only: V3 and V4 just either side of the
/// projection swap at zoom 12 (B2: crossing it requests no new tile; A5: an aircraft keeps its
/// heading across it), and B5's night views at an equinox noon (the centre in full night, and
/// in the day), which OpenFreeMap's world at zooms 0 to 2 already holds.
pub const DARK_VIEWS: [View; 6] = [
    View::at("V3 at 11.9", 59.33, 18.07, 11.9, 0.0, 0.0),
    View::at("V3 at 12.1", 59.33, 18.07, 12.1, 0.0, 0.0),
    View::at("V4 at 11.9", 59.6519, 17.9186, 11.9, 45.0, 30.0),
    View::at("V4 at 12.1", 59.6519, 17.9186, 12.1, 45.0, 30.0),
    View::at("B5 night", 0.0, 180.0, 1.9, 0.0, 0.0),
    View::at("B5 day", 0.0, 0.0, 1.9, 0.0, 0.0),
];

/// The weather radar over Europe flat (W1, W2's shot; at pitch 10 W3's "none" holds here too).
pub const RADAR_VIEWS: [View; 1] = [View::at("V2", 52.0, 10.0, 4.5, 0.0, 0.0)];
/// The weather radar over Europe pitched 45° (W3's lifted layers), at the shot's size.
pub const PITCHED_RADAR_VIEWS: [View; 1] = [View::at("V2 at 45", 52.0, 10.0, 4.5, 45.0, 0.0)];

/// The realistic map of the reference pictures with the app's own sources (R2 to R9 against
/// `realistic-v4` and `realistic-v5`): Arlanda pitched, and Stockholm city with Stockholm
/// Central's tracks at its left edge.
pub const REALISTIC_VIEWS: [View; 2] =
    [View::at("V4", 59.6519, 17.9186, 13.5, 45.0, 30.0), View::at("V5", 59.3293, 18.0686, 15.5, 60.0, 20.0)];
/// The realistic map close in, at the shot's size: Humlegården at zoom 16 (R7's park) and
/// Stockholm Central at zoom 16 (R9's station).
pub const CLOSE_UP_VIEWS: [View; 2] = [
    View::at("Humlegarden", 59.3398, 18.0727, 16.0, 45.0, 20.0),
    View::at("Stockholm C", 59.3311, 18.0573, 16.0, 45.0, 0.0),
];

/// The Alps from above (R12's hillshade on the three basemaps): the Pennine Alps round the
/// Matterhorn and Monte Rosa.
pub const RELIEF_VIEWS: [View; 1] = [View::at("Alps", 46.0, 7.75, 9.45, 0.0, 0.0)];

/// A mountain village with buildings on its slopes (Z2's draping, Z3's buildings standing on the
/// surface, Z4's camera ground height, on any map): Zermatt (1608 m; Monte Rosa, 4634 m, the
/// highest ground its tiles reach), looking north down the Mattertal from above its southern end
/// at zoom 16, pitched 60°, the dark and satellite maps' limit.
pub const TERRAIN_VIEWS: [View; 1] = [View::at("Zermatt", 46.0207, 7.7491, 16.0, 60.0, 0.0).on_relief(1608.0, 4634.0)];

/// Z1's silhouette against the sky: the same place pitched 75°, the realistic map's limit (the
/// Bernese Alps against the sky beyond the valley). At 60° the top of the view looks 11.6° below
/// the horizontal (deck's 36.87° field of view), so no sky is ever in view there.
pub const SKYLINE_VIEWS: [View; 1] = [View::at("Zermatt skyline", 46.0207, 7.7491, 16.0, 75.0, 0.0).on_relief(1608.0, 4634.0)];

/// What a view's fixtures hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cover {
    /// OpenFreeMap by deck's rule (the map, its labels, the true-width runways) and on the flat
    /// map the plain buildings' [`city_lod`] tiles up to zoom 14.
    pub map: bool,
    /// On the flat map the realistic ground's [`ground_lod`] too, and both LODs' zoom 16 from
    /// Streets GL.
    pub ground: bool,
    /// On the flat map the trees' tiles around the centre ([`tree_tiles`]).
    pub trees: bool,
    /// Esri's imagery.
    pub imagery: bool,
    /// Mapzen's terrarium tiles the hillshade shades.
    pub hillshade: bool,
    /// RainViewer's radar frame.
    pub radar: bool,
    /// Mapterhorn's heights: the 3D surface ([`surface_lod`]) and the camera's ground
    /// ([`ground_tile`]), with Mapzen's where Mapterhorn has none ([`fallbacks`]).
    pub terrain: bool,
    /// OpenRailwayMap's tracks and, on the flat map, platforms.
    pub rails: bool,
    /// The land cover layer's pictures (NMD, WorldCover) on the flat map.
    pub land_cover: bool,
    /// For the tree tiles round the centre ([`close_up_tiles`]): their land cover, and as data
    /// their canopy heights and Overture's buildings.
    pub close_up: bool,
}

const NOTHING: Cover = Cover {
    map: false,
    ground: false,
    trees: false,
    imagery: false,
    hillshade: false,
    radar: false,
    terrain: false,
    rails: false,
    land_cover: false,
    close_up: false,
};

impl Cover {
    /// V3 to V6: every source of stages 3 and 4.
    pub const ALL: Cover = Cover { map: true, ground: true, trees: true, imagery: true, hillshade: true, radar: true, ..NOTHING };
    /// The dark and satellite maps.
    pub const BASEMAPS: Cover = Cover { map: true, imagery: true, ..NOTHING };
    /// The dark map.
    pub const DARK: Cover = Cover { map: true, ..NOTHING };
    /// The weather radar alone (the dark map's land under it is a colour).
    pub const RADAR: Cover = Cover { radar: true, ..NOTHING };
    /// The realistic map close in, with the app's own sources.
    pub const REALISTIC: Cover =
        Cover { map: true, ground: true, trees: true, rails: true, land_cover: true, close_up: true, ..NOTHING };
    /// Relief from above on the three basemaps (the hillshade over each one's ground).
    pub const RELIEF: Cover = Cover { map: true, imagery: true, hillshade: true, ..NOTHING };
    /// Relief close in on the three maps.
    pub const TERRAIN: Cover = Cover { map: true, ground: true, hillshade: true, terrain: true, ..NOTHING };
    /// The 3D surface alone, out to the horizon (draped with the hillshade; its own colour else).
    pub const SKYLINE: Cover = Cover { hillshade: true, terrain: true, ..NOTHING };

    /// The tile sources it holds, as the NOTICE names them.
    pub fn sources(self) -> Vec<&'static str> {
        let mut sources = Vec::new();
        let mut take = |on: bool, source: &'static str| {
            if on && !sources.contains(&source) {
                sources.push(source);
            }
        };
        take(self.map || self.trees, OPENFREEMAP);
        take(self.ground || self.trees, STREETS_GL);
        take(self.imagery, ESRI);
        take(self.hillshade || self.terrain, TERRARIUM);
        take(self.terrain, MAPTERHORN);
        take(self.radar, RAINVIEWER);
        take(self.rails, OPENRAILWAYMAP);
        take(self.land_cover || self.close_up, NMD);
        take(self.land_cover || self.close_up, WORLDCOVER);
        sources
    }
}

/// The canvas sizes in CSS pixels the reference views are covered at: the shot command's default
/// and the two sizes of `docs/port/reference`.
pub const SIZES: [[f64; 2]; 3] = [[1100.0, 720.0], [1440.0, 900.0], [390.0, 844.0]];
/// The shot command's default size alone: the views of stages 8 to 10 with no reference picture.
pub const SHOT_SIZE: [[f64; 2]; 1] = [[1100.0, 720.0]];
/// The terrain views' size (`--size 800x500` for their shots): Mapterhorn's Swiss tiles are
/// 150 to 170 KB each, and a smaller canvas takes a third fewer (76 against 112 at 1100x720).
pub const TERRAIN_SIZE: [[f64; 2]; 1] = [[800.0, 500.0]];

/// A list of views, what their fixtures hold, and the canvas sizes they are covered at.
pub type Covered = (&'static [View], Cover, &'static [[f64; 2]]);

/// Each list of views with what its fixtures hold and the sizes it is covered at.
pub const COVERS: [Covered; 10] = [
    (&VIEWS, Cover::ALL, &SIZES),
    (&BASEMAP_VIEWS, Cover::BASEMAPS, &SIZES),
    (&DARK_VIEWS, Cover::DARK, &SIZES),
    (&RADAR_VIEWS, Cover::RADAR, &SIZES),
    (&PITCHED_RADAR_VIEWS, Cover::RADAR, &SHOT_SIZE),
    (&REALISTIC_VIEWS, Cover::REALISTIC, &SIZES),
    (&CLOSE_UP_VIEWS, Cover::REALISTIC, &SHOT_SIZE),
    (&RELIEF_VIEWS, Cover::RELIEF, &SHOT_SIZE),
    (&TERRAIN_VIEWS, Cover::TERRAIN, &TERRAIN_SIZE),
    (&SKYLINE_VIEWS, Cover::SKYLINE, &TERRAIN_SIZE),
];

/// The zooms at which the fixtures hold OpenFreeMap's whole world.
pub const WORLD_ZOOMS: std::ops::RangeInclusive<u32> = 0..=2;

/// The hillshade's tiles (`hillshadeLayer`, terrain-layer.ts:322-329): 256 px, zooms 0 to 12.
pub const HILLSHADE_TILES: TilesetOptions = TilesetOptions::new(256.0, 0.0, HILLSHADE_MAX_ZOOM as f64);

/// The land cover layer's tiles (`landCoverGroundLayer`, landcover-ground.ts:101-103): 256 px,
/// zooms 10 to 14.
pub const LAND_COVER_TILES: TilesetOptions = TilesetOptions::new(
    landcover_ground::TILE_SIZE as f64,
    landcover_ground::MIN_ZOOM as f64,
    landcover_ground::MAX_ZOOM as f64,
);

/// The deepest OpenFreeMap zoom.
const OPENFREEMAP_MAX_ZOOM: u32 = 14;

/// `CITY_LOD` (G:1039): the buildings close in on the flat map, by their distance.
pub fn city_lod() -> Lod {
    Lod { levels: vec![13, 14, 16], bias: 0.5, max_distance: 25_000.0 }
}

/// `GROUND_LOD` (G:1041): the realistic ground on the flat map, by its distance.
pub fn ground_lod() -> Lod {
    Lod { levels: vec![9, 10, 11, 12, 13, 14, 16], bias: 0.5, max_distance: 80_000.0 }
}

/// `SURFACE_LOD` (terrain-layer.ts:375): the 3D surface's tiles by their distance, every zoom to
/// Mapterhorn's deepest, bias 0.5, no farthest distance.
pub fn surface_lod() -> Lod {
    let surface = terrain_surface(&[]);
    Lod { levels: (surface.min_zoom..=surface.max_zoom).collect(), bias: surface.lod_bias, max_distance: f64::INFINITY }
}

/// `TREES_ZOOM` (G:1003): the realistic map plants trees on the flat map from this zoom.
pub const TREES_ZOOM: f64 = 14.0;

/// The trees' tiles around the centre of `view` on a canvas `height` CSS pixels high (G:2695-2704,
/// tree-layer.ts:690-693): none below [`TREES_ZOOM`] or on the globe; from [`SGL_FROM_ZOOM`]
/// Streets GL's z16 tiles within `min(2400, max(900, 1.6·d))` metres, else OpenFreeMap's
/// [`TREE_TILE_ZOOM`] tiles within `min(4800, max(900, 1.6·d))`, where `d` is how far the camera
/// is from the point it looks at, `CAMERA_HEIGHTS · height · metresPerPixel(zoom, latitude)`.
pub fn tree_tiles(view: &View, height: f64) -> Vec<TileIndex> {
    if !view.state().is_flat() || view.zoom < TREES_ZOOM {
        return Vec::new();
    }
    let sgl = view.zoom >= SGL_FROM_ZOOM;
    let distance = CAMERA_HEIGHTS * height * meters_per_pixel(view.zoom, view.latitude);
    let radius = if sgl { 2400f64 } else { 4800.0 }.min(900f64.max(1.6 * distance));
    let zoom = if sgl { SGL_ZOOM } else { TREE_TILE_ZOOM };
    tiles_around(view.longitude, view.latitude, radius, zoom).into_iter().map(|t| t.tile).collect()
}

/// The tile at `(latitude, longitude)` at zoom `z` (latitude clamped to ±85, as `groundHeight`).
pub fn tile_at(latitude: f64, longitude: f64, z: u32) -> TileIndex {
    let n = f64::from(1u32 << z);
    let r = latitude.clamp(-85.0, 85.0).to_radians();
    let x = ((longitude + 180.0) / 360.0).rem_euclid(1.0) * n;
    let y = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    TileIndex::new(x.floor() as u32, y.floor() as u32, z)
}

/// The tree tiles close to a view's centre on the flat map from [`TREES_ZOOM`]: the one under it
/// and its eight neighbours, at the trees' zoom (Streets GL's from [`SGL_FROM_ZOOM`]). Their land
/// cover, canopy heights and Overture buildings are held, so one tile's planting can be checked
/// with all it stands on (R7).
pub fn close_up_tiles(view: &View) -> Vec<TileIndex> {
    if !view.state().is_flat() || view.zoom < TREES_ZOOM {
        return Vec::new();
    }
    let z = if view.zoom >= SGL_FROM_ZOOM { SGL_ZOOM } else { TREE_TILE_ZOOM };
    let centre = tile_at(view.latitude, view.longitude, z);
    let mut tiles = Vec::new();
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            tiles.push(TileIndex::new((i64::from(centre.x) + dx) as u32, (i64::from(centre.y) + dy) as u32, z));
        }
    }
    tiles
}

/// The Mapterhorn tile `groundHeight` reads for the camera's ground (terrain-layer.ts:224-236):
/// zoom [`GROUND_ZOOM`], under the view's centre.
pub fn ground_tile(view: &View) -> TileIndex {
    tile_at(view.latitude, view.longitude, GROUND_ZOOM)
}

/// A tile as `(z, x, y)`: sorted by zoom, then column, then row.
pub type Zxy = (u32, u32, u32);

fn zxy(tile: TileIndex) -> Zxy {
    (tile.z, tile.x, tile.y)
}

fn add(tiles: &mut BTreeMap<&'static str, BTreeSet<Zxy>>, source: &'static str, found: impl IntoIterator<Item = TileIndex>) {
    tiles.entry(source).or_default().extend(found.into_iter().map(zxy));
}

/// The land cover's pictures of these tiles: NMD's in Sweden, WorldCover's from its zoom 6.
fn land_cover(tiles: &mut BTreeMap<&'static str, BTreeSet<Zxy>>, found: impl IntoIterator<Item = TileIndex>) {
    for tile in found {
        if in_sweden(tile.x, tile.y, tile.z) {
            add(tiles, NMD, [tile]);
        }
        if tile.z >= WORLD_COVER_MIN_ZOOM {
            add(tiles, WORLDCOVER, [tile]);
        }
    }
}

/// Every tile the fixtures hold, by source (Mapzen's stand-ins for Mapterhorn's missing tiles
/// aside: [`fallbacks`]).
pub fn plan() -> BTreeMap<&'static str, BTreeSet<Zxy>> {
    let mut plan: BTreeMap<&'static str, BTreeSet<Zxy>> = BTreeMap::new();
    let world = plan.entry(OPENFREEMAP).or_default();
    for z in WORLD_ZOOMS {
        for x in 0..1 << z {
            for y in 0..1 << z {
                world.insert((z, x, y));
            }
        }
    }
    for (views, cover, sizes) in COVERS {
        for view in views {
            for &size in sizes {
                for (source, tiles) in view_tiles(view, size, cover) {
                    plan.entry(source).or_default().extend(tiles);
                }
            }
        }
    }
    plan
}

/// The tiles one view asks of each source at one canvas size, for what `cover` holds.
pub fn view_tiles(view: &View, size: [f64; 2], cover: Cover) -> BTreeMap<&'static str, BTreeSet<Zxy>> {
    let mut tiles: BTreeMap<&'static str, BTreeSet<Zxy>> = BTreeMap::new();
    // with the terrain on, on the flat map, the camera's target stands on the ground
    let raised_on_relief = view.states().len() > 1;
    for state in view.states() {
        let viewport = viewport_for(&state, size);
        // the 3D surface, drawn with the terrain on, its tiles' volumes over the heights they span
        if cover.terrain && (state.position.is_some() || !raised_on_relief) {
            add(&mut tiles, MAPTERHORN, distance_tile_indices(&viewport, &surface_lod(), view.z_range(), 0.0));
        }
        // the 2D layers, draped or not: deck's selection by the view (no elevation range)
        let deck = |opts: &TilesetOptions| tile_indices(&viewport, opts);
        if cover.map {
            add(&mut tiles, OPENFREEMAP, deck(&VECTOR_TILES));
        }
        if cover.imagery {
            add(&mut tiles, ESRI, deck(&IMAGERY_TILES));
        }
        if cover.hillshade {
            add(&mut tiles, TERRARIUM, deck(&HILLSHADE_TILES));
        }
        if cover.radar {
            add(&mut tiles, RAINVIEWER, deck(&RADAR_TILES));
        }
        if cover.rails {
            add(&mut tiles, OPENRAILWAYMAP, deck(&RAIL_TILES));
        }
        if !state.is_flat() {
            continue;
        }
        if cover.rails {
            add(&mut tiles, OPENRAILWAYMAP, distance_tile_indices(&viewport, &platform_lod(), None, 0.0));
        }
        if cover.land_cover {
            land_cover(&mut tiles, deck(&LAND_COVER_TILES));
        }
        if cover.map {
            // the buildings' and the realistic ground's tiles by distance (512 px tiles, no zoom
            // offset); their zooms up to 14 are OpenFreeMap's and 16 is Streets GL's, the
            // realistic map's source for it (G:1051, realistic-ground.ts:826): the dark and
            // satellite maps' buildings have no source there (G:1049), so a tile of 16 is empty
            let lods = if cover.ground { vec![city_lod(), ground_lod()] } else { vec![city_lod()] };
            for lod in lods {
                for tile in distance_tile_indices(&viewport, &lod, None, 0.0) {
                    if tile.z <= OPENFREEMAP_MAX_ZOOM {
                        add(&mut tiles, OPENFREEMAP, [tile]);
                    } else if tile.z == SGL_ZOOM && cover.ground {
                        add(&mut tiles, STREETS_GL, [tile]);
                    }
                }
            }
        }
    }
    if view.state().is_flat() {
        if cover.terrain {
            add(&mut tiles, MAPTERHORN, [ground_tile(view)]);
        }
        if cover.trees {
            for tile in tree_tiles(view, size[1]) {
                let source = if tile.z == SGL_ZOOM { STREETS_GL } else { OPENFREEMAP };
                add(&mut tiles, source, [tile]);
            }
        }
        if cover.close_up {
            land_cover(&mut tiles, close_up_tiles(view));
        }
    }
    tiles
}

/// What the fixtures hold beside the tiles, besides the cloud picture and the radar volumes
/// (always held).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DataPlan {
    /// The tiles whose canopy heights are recorded (their files' head and the ranges under
    /// their windows).
    pub canopy: BTreeSet<Zxy>,
    /// Overture's z14 tiles in the extract.
    pub overture: BTreeSet<Zxy>,
}

/// The data the fixtures hold: the close-up tiles' canopy heights and the Overture tiles under
/// them.
pub fn data_plan() -> DataPlan {
    let mut plan = DataPlan::default();
    for (views, cover, _) in COVERS {
        if !cover.close_up {
            continue;
        }
        for view in views {
            for tile in close_up_tiles(view) {
                plan.canopy.insert(zxy(tile));
                let k = tile.z.saturating_sub(overture::ZOOM);
                plan.overture.insert((overture::ZOOM, tile.x >> k, tile.y >> k));
            }
        }
    }
    plan
}

/// The canopy tiles by the COG file they are read from (its quadkey).
pub fn canopy_files(tiles: &BTreeSet<Zxy>) -> BTreeMap<String, Vec<Zxy>> {
    let mut files: BTreeMap<String, Vec<Zxy>> = BTreeMap::new();
    for &(z, x, y) in tiles {
        files.entry(place_in_file(x, y, z).key).or_default().push((z, x, y));
    }
    files
}

/// Mapzen's stand-ins for the 3D surface (`terrainFetch`, terrain-layer.ts:380-385): for each
/// Mapterhorn tile a set holds as nothing (404), Mapzen's terrarium tile of the same `z/x/y`,
/// where Mapzen has tiles (to zoom [`TERRARIUM_MAX_ZOOM`]).
pub fn fallbacks(nothing: &BTreeSet<Zxy>) -> BTreeSet<Zxy> {
    nothing.iter().copied().filter(|&(z, _, _)| z <= TERRARIUM_MAX_ZOOM).collect()
}

/// A tile's file name extension by source.
pub fn extension(source: &str) -> &'static str {
    match source {
        OPENFREEMAP | STREETS_GL | OPENRAILWAYMAP => "pbf",
        ESRI => "jpg",
        MAPTERHORN => "webp",
        _ => "png",
    }
}

/// A tile's URL from a template with `{z}`, `{x}` and `{y}` in any order.
pub fn tile_url(template: &str, (z, x, y): Zxy) -> String {
    template.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string())
}

/// Esri's tile template: rows before columns.
pub fn esri_template() -> String {
    format!("{ESRI_IMAGERY}/tile/{{z}}/{{y}}/{{x}}")
}

/// The URL template of a TileJSON: its first `tiles` entry.
pub fn tilejson_template(tilejson: &serde_json::Value) -> Result<String, String> {
    tilejson["tiles"][0].as_str().map(str::to_owned).ok_or_else(|| "the TileJSON has no tiles".to_owned())
}

/// The latest past RainViewer frame of a frame list: its time (Unix seconds) and its tile URL
/// template, `${host}${path}/256/{z}/{x}/{y}/2/1_1.png` (256 px, colour scheme 2 "Universal
/// Blue", smoothed, snow shown; G:1118-1119, doc 02 §3.2).
pub fn rainviewer_frame(maps: &serde_json::Value) -> Result<(i64, String), String> {
    let host = maps["host"].as_str().ok_or("the frame list has no host")?;
    let latest = maps["radar"]["past"].as_array().and_then(|past| past.last()).ok_or("the frame list has no past frame")?;
    let path = latest["path"].as_str().ok_or("the latest frame has no path")?;
    let time = latest["time"].as_i64().ok_or("the latest frame has no time")?;
    Ok((time, format!("{host}{path}/256/{{z}}/{{x}}/{{y}}/2/1_1.png")))
}

/// Text of an HTML attribution: tags dropped, the few entities it uses decoded, spaces collapsed.
pub fn plain_text(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text.replace("&copy;", "©").replace("&amp;", "&").replace("&nbsp;", " ");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The UTC calendar date and time of Unix seconds: `YYYY-MM-DD HH:MM`.
pub fn utc(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds = unix_seconds.rem_euclid(86_400);
    // days since 1970-01-01 to a civil date (Howard Hinnant's `civil_from_days`)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

/// The repository root: `crates/xtask` is two levels below it.
fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}

/// Where the fixtures are committed: tiles in [`TILES`], the rest beside it.
pub fn data_dir() -> PathBuf {
    repo_root().join("crates/map/tests/data")
}

/// Where the tile fixtures are committed (the run takes [`TILES`] below its `--out`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn fixtures_dir() -> PathBuf {
    data_dir().join(TILES)
}

const USAGE: &str = "usage: cargo xtask fixtures [--fresh] [--list | --missing] [--out <dir>]

  (no option)    top the set up: fetch only the planned tiles and data it lacks, from the set's
                 own OpenFreeMap build (and RainViewer frame while it is served)
  --fresh        fetch everything planned anew, from the day's build and frame, and replace the set
  --list         print the planned tiles of each source and the planned data, download nothing
  --missing      print the planned tiles and data the set lacks, download nothing
  --out <dir>    the data folder instead of crates/map/tests/data (its tiles in <dir>/tiles)";

pub fn run(args: &[String]) -> Result<(), String> {
    let (mut list, mut missing, mut fresh) = (false, false, false);
    let mut out = data_dir();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => list = true,
            "--missing" => missing = true,
            "--fresh" => fresh = true,
            "--out" => out = PathBuf::from(args.next().ok_or(USAGE)?),
            _ => return Err(format!("unknown argument `{arg}`\n\n{USAGE}")),
        }
    }
    if list || missing {
        let plan = plan();
        let held = held_tiles(&out.join(TILES))?;
        let mut shown = if missing { lacking(&plan, &held) } else { plan };
        if missing {
            let lacked: BTreeSet<Zxy> = fallbacks(&nothing_held(&out.join(TILES), MAPTERHORN))
                .into_iter()
                .filter(|tile| !held.get(TERRARIUM).is_some_and(|h| h.contains(tile)))
                .collect();
            if !lacked.is_empty() {
                shown.entry(TERRARIUM).or_default().extend(lacked);
            }
        }
        for (source, tiles) in &shown {
            println!("{source}: {} tiles", tiles.len());
            for &(z, x, y) in tiles {
                println!("  {z}/{x}/{y}");
            }
        }
        let data = data_plan();
        for (key, tiles) in canopy_files(&data.canopy) {
            if !missing || !canopy_held(&out, &key, &tiles) {
                println!("canopy {key}: {} tiles", tiles.len());
            }
        }
        if !missing || !extract_holds(&out.join(OVERTURE_FILE), &data.overture) {
            println!("overture: {} tiles", data.overture.len());
            for &(z, x, y) in &data.overture {
                println!("  {z}/{x}/{y}");
            }
        }
        if !missing || !out.join(CLOUDS_FILE).is_file() {
            println!("{CLOUDS_FILE}");
        }
        for source in RADAR_SOURCES {
            if !missing || !radar_held(&out, source) {
                println!("{RADAR}/{source}");
            }
        }
        if !missing || !dwd_held(&out) {
            println!("{DWD}");
        }
        return Ok(());
    }
    download(&out, fresh)
}

/// What a run read from the services besides the tiles, for the NOTICE.
struct Found {
    openfreemap_template: String,
    openfreemap_attribution: String,
    esri_copyright: String,
    rainviewer_time: i64,
    rainviewer_template: String,
    /// WorldCover's mosaic, once registered.
    worldcover_id: Option<String>,
    /// When the set was downloaded whole, `YYYY-MM-DD HH:MM` UTC.
    downloaded: String,
    /// When it was last topped up, if it was.
    topped_up: Option<String>,
}

/// Unix seconds now.
fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The staging folder beside the set: `<out>.download`, emptied.
fn staging_for(out: &Path) -> Result<PathBuf, String> {
    let name = out.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("bad output folder {}", out.display()))?;
    let staging = out.with_file_name(format!("{name}.download"));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| format!("cannot clear {}: {e}", staging.display()))?;
    }
    fs::create_dir_all(&staging).map_err(|e| format!("cannot create {}: {e}", staging.display()))?;
    Ok(staging)
}

/// A tile's URL as the run found the sources; `None` for OpenRailwayMap's, which the backend
/// fetches.
fn source_url(found: &Found, source: &str, tile: Zxy) -> Result<Option<String>, String> {
    let (z, x, y) = tile;
    Ok(Some(match source {
        OPENFREEMAP => tile_url(&found.openfreemap_template, tile),
        ESRI => tile_url(&esri_template(), tile),
        TERRARIUM => tile_url(TERRARIUM_URL, tile),
        MAPTERHORN => tile_url(MAPTERHORN_URL, tile),
        RAINVIEWER => tile_url(&found.rainviewer_template, tile),
        STREETS_GL => tile_url(SGL_TILES, tile),
        NMD => nmd_url(x, y, z),
        WORLDCOVER => {
            let id = found.worldcover_id.as_deref().ok_or("WorldCover's mosaic is not registered")?;
            world_cover_url(id, x, y, z).ok_or_else(|| format!("WorldCover has no tile {z}/{x}/{y}"))?
        }
        OPENRAILWAYMAP => return Ok(None),
        _ => return Err(format!("no URL for the source {source}")),
    }))
}

/// One tile to fetch: its URL (none: through the backend), the tile and where it goes.
struct Job {
    url: Option<String>,
    tile: Zxy,
    path: PathBuf,
}

/// The download of each tile of `tiles` into `staging` (the tiles' folder of the staging).
fn jobs(tiles: &BTreeMap<&'static str, BTreeSet<Zxy>>, found: &Found, staging: &Path) -> Result<Vec<Job>, String> {
    let mut jobs = Vec::new();
    for (source, tiles) in tiles {
        for &tile in tiles {
            jobs.push(Job { url: source_url(found, source, tile)?, tile, path: staging.join(tile_path(source, tile)) });
        }
    }
    Ok(jobs)
}

/// Every job, [`PARALLEL`] at once: how many tiles the servers had nothing for, or every failure.
fn fetch_all(jobs: &[Job], staging: &Path) -> Result<usize, String> {
    if jobs.is_empty() {
        return Ok(0);
    }
    println!("downloading {} tiles", jobs.len());
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let nothing = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL {
            scope.spawn(|| {
                while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let fetched = match &job.url {
                        Some(url) => fetch_tile(url, &job.path),
                        None => fetch_rail(job.tile, &job.path),
                    };
                    match fetched {
                        Ok(true) => {}
                        Ok(false) => {
                            nothing.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(why) => failures.lock().unwrap_or_else(|e| e.into_inner()).push(why),
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap_or_else(|e| e.into_inner());
    if !failures.is_empty() {
        return Err(format!(
            "{} of {} tiles failed (the set is left as it was; {} holds what arrived):\n{}",
            failures.len(),
            jobs.len(),
            staging.display(),
            failures.join("\n")
        ));
    }
    Ok(nothing.load(Ordering::Relaxed))
}

/// The tokio runtime the backend's own fetches run on.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a tokio runtime"))
}

/// One OpenRailwayMap tile through the backend (`backend::rail::tile`, the app's `railtiles:`):
/// true when it was there, false when the server had nothing (an empty `.none` is left).
fn fetch_rail((z, x, y): Zxy, path: &Path) -> Result<bool, String> {
    match runtime().block_on(rustradar_backend::rail::tile(z, x, y)) {
        Ok(Some(bytes)) => {
            write(path, &bytes)?;
            Ok(true)
        }
        Ok(None) => {
            write(&path.with_extension(NOTHING_EXT), b"")?;
            Ok(false)
        }
        Err(e) => Err(format!("OpenRailwayMap {z}/{x}/{y}: {e}")),
    }
}

/// The planned tiles and data a set at `out` (the data folder) lacks, fetched into a staging
/// folder and moved in once all have arrived; with `fresh`, everything anew, replacing the set.
///
/// A plain run keeps the set's OpenFreeMap build (its kept TileJSON): if the build no longer
/// serves a tile the set holds byte for byte, it stops and names `--fresh`. Its RainViewer frame
/// is kept while it is served; once gone, the radar's folder is fetched whole from the day's
/// latest frame. The NOTICEs are written anew with the day of the run.
pub fn download(out: &Path, fresh: bool) -> Result<(), String> {
    let tiles_out = out.join(TILES);
    let plan = plan();
    let data = data_plan();
    let held = if fresh { BTreeMap::new() } else { held_tiles(&tiles_out)? };
    let staging = staging_for(out)?;
    let tiles_staging = staging.join(TILES);
    // what the run replaces as a whole once the staging is moved in
    let mut removed: Vec<PathBuf> = Vec::new();
    let mut missing = lacking(&plan, &held);
    let mut found = if fresh { found_today(&tiles_staging)? } else { found_held(&tiles_out)? };
    if fresh {
        removed.extend(plan.keys().map(|source| tiles_out.join(source)));
        removed.extend([tiles_out.join("NOTICE"), out.join("NOTICE")]);
        removed.extend(["clouds", CANOPY, "overture", RADAR].map(|name| out.join(name)));
    } else {
        if missing.contains_key(OPENFREEMAP) && !serves_held(OPENFREEMAP, &found.openfreemap_template, &tiles_out, &held, &staging)? {
            return Err(format!(
                "{} no longer serves the set's tiles: the set's build is gone; run with --fresh",
                found.openfreemap_template
            ));
        }
        if missing.contains_key(RAINVIEWER) && !serves_held(RAINVIEWER, &found.rainviewer_template, &tiles_out, &held, &staging)? {
            // the frame has gone: the radar's folder anew, from the day's latest frame
            let maps_bytes = get(RAINVIEWER_MAPS)?;
            let maps: serde_json::Value = serde_json::from_slice(&maps_bytes).map_err(|e| format!("{RAINVIEWER_MAPS}: {e}"))?;
            (found.rainviewer_time, found.rainviewer_template) = rainviewer_frame(&maps)?;
            write(&tiles_staging.join(RAINVIEWER).join("weather-maps.json"), &maps_bytes)?;
            missing.insert(RAINVIEWER, plan[RAINVIEWER].clone());
            removed.push(tiles_out.join(RAINVIEWER));
            println!("the set's radar frame is gone: the radar's tiles anew from {}", utc(found.rainviewer_time));
        }
        if missing.contains_key(ESRI) && found.esri_copyright.is_empty() {
            found.esri_copyright = esri_copyright()?;
        }
    }
    if missing.contains_key(WORLDCOVER) {
        found.worldcover_id = Some(register_world_cover(&tiles_staging)?);
    }

    // the tiles, then Mapzen's stand-ins where Mapterhorn has none
    let mut nothing = fetch_all(&jobs(&missing, &found, &tiles_staging)?, &staging)?;
    let mut mapterhorn_nothing = nothing_held(&tiles_staging, MAPTERHORN);
    if !fresh {
        mapterhorn_nothing.extend(nothing_held(&tiles_out, MAPTERHORN));
    }
    let staged = held_tiles(&tiles_staging)?;
    let stand_ins: BTreeSet<Zxy> = fallbacks(&mapterhorn_nothing)
        .into_iter()
        .filter(|tile| ![&held, &staged].iter().any(|set| set.get(TERRARIUM).is_some_and(|h| h.contains(tile))))
        .collect();
    if !stand_ins.is_empty() {
        let stand_ins: BTreeMap<&'static str, BTreeSet<Zxy>> = [(TERRARIUM, stand_ins)].into();
        nothing += fetch_all(&jobs(&stand_ins, &found, &tiles_staging)?, &staging)?;
        for (source, tiles) in stand_ins {
            missing.entry(source).or_default().extend(tiles);
        }
    }

    // the data
    let mut fetched_data = Vec::new();
    if fresh || !out.join(CLOUDS_FILE).is_file() {
        fetch_clouds(&staging)?;
        fetched_data.push("the cloud picture".to_owned());
    }
    for (key, tiles) in canopy_files(&data.canopy) {
        if fresh || !canopy_held(out, &key, &tiles) {
            record_canopy(&key, &tiles, &staging)?;
            removed.extend([out.join(CANOPY).join(&key), out.join(CANOPY).join(format!("{key}.json"))]);
            fetched_data.push(format!("canopy {key}"));
        }
    }
    if fresh || !extract_holds(&out.join(OVERTURE_FILE), &data.overture) {
        build_overture(&data.overture, &staging)?;
        fetched_data.push("the Overture extract".to_owned());
    }
    for source in RADAR_SOURCES {
        if fresh || !radar_held(out, source) {
            record_radar(source, &staging.join(RADAR).join(source))?;
            removed.push(out.join(RADAR).join(source));
            fetched_data.push(format!("the {source} volume"));
        }
    }
    if fresh || !dwd_held(out) {
        record_dwd(&staging.join(DWD))?;
        removed.push(out.join(DWD));
        fetched_data.push("DWD's WN analysis".to_owned());
    }

    // within the budget, then into the set
    let bytes = bytes_after(out, &staging, &removed)?;
    if bytes >= BUDGET_BYTES {
        return Err(format!(
            "the set would be {bytes} bytes, past the budget of {BUDGET_BYTES} (what arrived is left in {})",
            staging.display()
        ));
    }
    let changed = !missing.is_empty() || !fetched_data.is_empty();
    for path in &removed {
        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        } else if path.exists() {
            fs::remove_file(path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
        }
    }
    move_into(&staging, &staging, out)?;
    fs::remove_dir_all(&staging).map_err(|e| format!("cannot remove {}: {e}", staging.display()))?;
    if !fresh && changed {
        found.topped_up = Some(utc(now()));
    }
    if found.worldcover_id.is_none() {
        found.worldcover_id = read_json(&tiles_out.join(WORLDCOVER).join("register.json")).ok().and_then(|j| mosaic_id(&j).map(str::to_owned));
    }
    fs::create_dir_all(&tiles_out).map_err(|e| format!("cannot create {}: {e}", tiles_out.display()))?;
    fs::write(tiles_out.join("NOTICE"), notice(&found)).map_err(|e| format!("cannot write the NOTICE: {e}"))?;
    fs::write(out.join("NOTICE"), data_notice(out)?).map_err(|e| format!("cannot write the data NOTICE: {e}"))?;
    if !changed {
        println!("{} holds every planned tile and file", out.display());
        return Ok(());
    }
    for (source, tiles) in &missing {
        println!("{source}: {} tiles", tiles.len());
    }
    for item in &fetched_data {
        println!("{item}");
    }
    println!("wrote {} ({bytes} bytes in all; {nothing} tiles the servers had nothing for)", out.display());
    Ok(())
}

/// The day's OpenFreeMap build and RainViewer frame, and Esri's credit (`--fresh`); the TileJSON
/// and the frame list are kept in the staging's tiles.
fn found_today(tiles_staging: &Path) -> Result<Found, String> {
    let tilejson_bytes = get(OPENFREEMAP_TILEJSON)?;
    let tilejson: serde_json::Value =
        serde_json::from_slice(&tilejson_bytes).map_err(|e| format!("{OPENFREEMAP_TILEJSON}: {e}"))?;
    write(&tiles_staging.join(OPENFREEMAP).join("tilejson.json"), &tilejson_bytes)?;
    let maps_bytes = get(RAINVIEWER_MAPS)?;
    let maps: serde_json::Value = serde_json::from_slice(&maps_bytes).map_err(|e| format!("{RAINVIEWER_MAPS}: {e}"))?;
    write(&tiles_staging.join(RAINVIEWER).join("weather-maps.json"), &maps_bytes)?;
    let (rainviewer_time, rainviewer_template) = rainviewer_frame(&maps)?;
    Ok(Found {
        openfreemap_template: tilejson_template(&tilejson)?,
        openfreemap_attribution: plain_text(tilejson["attribution"].as_str().unwrap_or_default()),
        esri_copyright: esri_copyright()?,
        rainviewer_time,
        rainviewer_template,
        worldcover_id: None,
        downloaded: utc(now()),
        topped_up: None,
    })
}

/// The set's own build and frame (its kept TileJSON and frame list) and what its NOTICE says.
fn found_held(tiles_out: &Path) -> Result<Found, String> {
    let read = |name: &str| {
        fs::read(tiles_out.join(name))
            .map_err(|e| format!("cannot read {}: {e} (no set to top up: run with --fresh)", tiles_out.join(name).display()))
    };
    let tilejson: serde_json::Value =
        serde_json::from_slice(&read("openfreemap/tilejson.json")?).map_err(|e| format!("openfreemap/tilejson.json: {e}"))?;
    let maps: serde_json::Value =
        serde_json::from_slice(&read("rainviewer/weather-maps.json")?).map_err(|e| format!("rainviewer/weather-maps.json: {e}"))?;
    let old_notice = String::from_utf8_lossy(&read("NOTICE")?).into_owned();
    let (rainviewer_time, rainviewer_template) = rainviewer_frame(&maps)?;
    Ok(Found {
        openfreemap_template: tilejson_template(&tilejson)?,
        openfreemap_attribution: plain_text(tilejson["attribution"].as_str().unwrap_or_default()),
        esri_copyright: line_after(&old_notice, ESRI_CREDIT).map(str::to_owned).unwrap_or_default(),
        rainviewer_time,
        rainviewer_template,
        worldcover_id: read_json(&tiles_out.join(WORLDCOVER).join("register.json")).ok().and_then(|j| mosaic_id(&j).map(str::to_owned)),
        downloaded: line_after(&old_notice, DOWNLOADED_ON)
            .and_then(|rest| rest.split_once(" UTC"))
            .map_or_else(|| utc(now()), |(date, _)| date.to_owned()),
        topped_up: line_after(&old_notice, TOPPED_UP_ON).and_then(|rest| rest.split_once(" UTC")).map(|(date, _)| date.to_owned()),
    })
}

/// Registers WorldCover's 2021 mosaic (`worldCoverMosaic`, landcover.ts:143-161): its id, the
/// answer kept as `worldcover/register.json` in the staging's tiles.
fn register_world_cover(tiles_staging: &Path) -> Result<String, String> {
    let output = curl()
        .args(["--fail", "--request", "POST", "--header", "Content-Type: application/json", "--data", WORLD_COVER_SEARCH])
        .arg(WORLD_COVER_REGISTER)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!("{WORLD_COVER_REGISTER}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("{WORLD_COVER_REGISTER}: {e}"))?;
    let id = mosaic_id(&answer).ok_or_else(|| format!("{WORLD_COVER_REGISTER} named no mosaic"))?.to_owned();
    write(&tiles_staging.join(WORLDCOVER).join("register.json"), &output.stdout)?;
    Ok(id)
}

/// Whether a versioned source's template still serves what the set holds: one held tile (the
/// lowest) fetched again comes back byte for byte.
fn serves_held(source: &str, template: &str, tiles_out: &Path, held: &BTreeMap<String, BTreeSet<Zxy>>, staging: &Path) -> Result<bool, String> {
    let Some(tile) = held.get(source).and_then(|tiles| tiles.iter().find(|&&tile| tiles_out.join(tile_path(source, tile)).is_file())).copied() else {
        // nothing held to compare: a new build or frame serves the set as well as any
        return Ok(true);
    };
    let check = staging.join("check").join(tile_path(source, tile));
    let there = fetch_tile(&tile_url(template, tile), &check)?;
    let same = there && fs::read(&check).ok() == fs::read(tiles_out.join(tile_path(source, tile))).ok();
    fs::remove_dir_all(staging.join("check")).map_err(|e| format!("cannot remove {}: {e}", staging.join("check").display()))?;
    Ok(same)
}

/// Every file below `dir` moved to the same place below `out` (`root` is where `dir`'s walk
/// started).
fn move_into(dir: &Path, root: &Path, out: &Path) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?.path();
        if path.is_dir() {
            move_into(&path, root, out)?;
            continue;
        }
        let to = out.join(path.strip_prefix(root).map_err(|e| e.to_string())?);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        fs::rename(&path, &to).map_err(|e| format!("cannot move {} to {}: {e}", path.display(), to.display()))?;
    }
    Ok(())
}

/// The relative paths of every file below `dir`.
fn files_below(dir: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
            let path = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?.path();
            if path.is_dir() {
                walk(&path, root, out)?;
            } else {
                out.push(path.strip_prefix(root).map_err(|e| e.to_string())?.to_path_buf());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if dir.is_dir() {
        walk(dir, dir, &mut files)?;
    }
    Ok(files)
}

/// The bytes of the set at `out` once `removed` is gone and the staging moved in over it.
fn bytes_after(out: &Path, staging: &Path, removed: &[PathBuf]) -> Result<u64, String> {
    let size = |path: &Path| -> Result<u64, String> {
        if path.is_dir() {
            folder_bytes(path)
        } else {
            Ok(fs::metadata(path).map_or(0, |m| m.len()))
        }
    };
    let mut bytes = if out.is_dir() { folder_bytes(out)? } else { 0 };
    for path in removed {
        bytes -= size(path)?.min(bytes);
    }
    for file in files_below(staging)? {
        let old = out.join(&file);
        if !removed.iter().any(|r| old.starts_with(r)) {
            bytes -= size(&old)?.min(bytes);
        }
        bytes += size(&staging.join(&file))?;
    }
    Ok(bytes)
}

/// Esri's credit for its imagery, as the map service states it (`copyrightText`).
fn esri_copyright() -> Result<String, String> {
    let service_url = format!("{ESRI_IMAGERY}?f=json");
    let service: serde_json::Value = serde_json::from_slice(&get(&service_url)?).map_err(|e| format!("{service_url}: {e}"))?;
    Ok(service["copyrightText"].as_str().unwrap_or_default().to_owned())
}

/// The tiles a set holds, by source: every `<source>/<z>/<x>/<y>.<ext>`, a `.none` marker
/// included. No folder: none.
pub fn held_tiles(root: &Path) -> Result<BTreeMap<String, BTreeSet<Zxy>>, String> {
    let mut held: BTreeMap<String, BTreeSet<Zxy>> = BTreeMap::new();
    if !root.is_dir() {
        return Ok(held);
    }
    let names = |dir: &Path| -> Result<Vec<(String, PathBuf)>, String> {
        let mut names = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
            let path = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                names.push((name.to_owned(), path.clone()));
            }
        }
        Ok(names)
    };
    for (source, source_dir) in names(root)? {
        if !source_dir.is_dir() {
            continue;
        }
        for (z, z_dir) in names(&source_dir)? {
            let Ok(z) = z.parse::<u32>() else { continue };
            if !z_dir.is_dir() {
                continue;
            }
            for (x, x_dir) in names(&z_dir)? {
                let Ok(x) = x.parse::<u32>() else { continue };
                if !x_dir.is_dir() {
                    continue;
                }
                for (file, _) in names(&x_dir)? {
                    if let Some(y) = file.split_once('.').and_then(|(y, _)| y.parse::<u32>().ok()) {
                        held.entry(source.clone()).or_default().insert((z, x, y));
                    }
                }
            }
        }
    }
    Ok(held)
}

/// The tiles of `source` a set at `root` holds as nothing there (`.none`).
pub fn nothing_held(root: &Path, source: &str) -> BTreeSet<Zxy> {
    let mut nothing = BTreeSet::new();
    for file in files_below(&root.join(source)).unwrap_or_default() {
        let parts: Vec<&str> = file.iter().filter_map(|p| p.to_str()).collect();
        if let [z, x, name] = parts.as_slice()
            && let Some(y) = name.strip_suffix(&format!(".{NOTHING_EXT}"))
            && let (Ok(z), Ok(x), Ok(y)) = (z.parse(), x.parse(), y.parse())
        {
            nothing.insert((z, x, y));
        }
    }
    nothing
}

/// The planned tiles a set does not hold, by source; sources with none left out.
pub fn lacking(
    plan: &BTreeMap<&'static str, BTreeSet<Zxy>>,
    held: &BTreeMap<String, BTreeSet<Zxy>>,
) -> BTreeMap<&'static str, BTreeSet<Zxy>> {
    plan.iter()
        .map(|(&source, tiles)| {
            let lacked = tiles.iter().filter(|tile| !held.get(source).is_some_and(|h| h.contains(tile))).copied().collect();
            (source, lacked)
        })
        .filter(|(_, lacked): &(&str, BTreeSet<Zxy>)| !lacked.is_empty())
        .collect()
}

/// The rest of the first line of `text` after `prefix`.
fn line_after<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let start = text.find(prefix)? + prefix.len();
    text[start..].lines().next().map(str::trim_end)
}

/// A tile's path below the fixtures folder.
pub fn tile_path(source: &str, (z, x, y): Zxy) -> PathBuf {
    Path::new(source).join(z.to_string()).join(x.to_string()).join(format!("{y}.{}", extension(source)))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// `curl` with the options every request uses: errors shown, HTTP compression asked for and
/// undone, redirects followed, transient failures (timeouts, 408, 429, 5xx) retried.
fn curl() -> Command {
    let mut curl = Command::new("curl");
    curl.args(["--silent", "--show-error", "--compressed", "--location", "--retry", "4", "--retry-delay", "2"]);
    curl.args(["--connect-timeout", "20", "--max-time", "300"]);
    curl
}

/// A small document (a TileJSON, a frame list), failing on any HTTP error.
fn get(url: &str) -> Result<Vec<u8>, String> {
    let output = curl().arg("--fail").arg(url).output().map_err(|e| format!("cannot run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!("{url}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(output.stdout)
}

/// Bytes `[start, end)` of a file (fewer when the file ends first), and the file's length, by an
/// HTTP range request into `scratch`.
fn get_range(url: &str, start: u64, end: u64, scratch: &Path) -> Result<(Vec<u8>, u64), String> {
    if let Some(dir) = scratch.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let output = Command::new("curl")
        .args(["--silent", "--show-error", "--location", "--retry", "4", "--retry-delay", "2", "--connect-timeout", "20"])
        .args(["--max-time", "300", "--range", &format!("{start}-{}", end - 1), "--output"])
        .arg(scratch)
        .args(["--write-out", "%{http_code} %header{content-range}"])
        .arg(url)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    let answer = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() {
        return Err(format!("{url} [{start}, {end}): {} ({answer})", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let bytes = fs::read(scratch).map_err(|e| format!("cannot read {}: {e}", scratch.display()))?;
    let _ = fs::remove_file(scratch);
    let (status, range) = answer.split_once(' ').unwrap_or((&answer, ""));
    // `bytes 0-65535/123456789`
    let length = range.rsplit_once('/').and_then(|(_, total)| total.trim().parse::<u64>().ok());
    match (status, length) {
        ("206", Some(length)) if bytes.len() as u64 == end.min(length) - start => Ok((bytes, length)),
        _ => Err(format!("{url} [{start}, {end}): HTTP {status} `{range}` with {} bytes", bytes.len())),
    }
}

/// One tile into `path`: true when it was there, false when the server had nothing (404 or 204;
/// an empty `.none` file is left in its place).
fn fetch_tile(url: &str, path: &Path) -> Result<bool, String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let output = curl()
        .arg("--output")
        .arg(path)
        .args(["--write-out", "%{http_code}"])
        .arg(url)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    let status = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() {
        return Err(format!("{url}: {} (HTTP {status})", String::from_utf8_lossy(&output.stderr).trim()));
    }
    match status.as_str() {
        "200" => Ok(true),
        "404" | "204" => {
            let _ = fs::remove_file(path);
            write(&path.with_extension(NOTHING_EXT), b"")?;
            Ok(false)
        }
        _ => {
            let _ = fs::remove_file(path);
            Err(format!("{url}: HTTP {status}"))
        }
    }
}

/// The bytes of every file below a folder.
pub fn folder_bytes(dir: &Path) -> Result<u64, String> {
    let mut total = 0;
    for entry in fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let meta = entry.metadata().map_err(|e| format!("cannot read {}: {e}", entry.path().display()))?;
        total += if meta.is_dir() { folder_bytes(&entry.path())? } else { meta.len() };
    }
    Ok(total)
}

// --- the cloud picture -----------------------------------------------------------------------------

/// EUMETSAT's cloud picture (`cloudPicture`, clouds.ts:10-12) into the staging, and when it came.
fn fetch_clouds(staging: &Path) -> Result<(), String> {
    let picture = get(CLOUD_URL)?;
    let (width, height) = jpeg_size(&picture).ok_or("EUMETSAT's answer is not a JPEG")?;
    if (width, height) != (4096, 1934) {
        return Err(format!("EUMETSAT's picture is {width}x{height}, not 4096x1934"));
    }
    write(&staging.join(CLOUDS_FILE), &picture)?;
    let record = serde_json::json!({ "url": CLOUD_URL, "fetched": now() });
    write(&staging.join(CLOUDS_RECORD), &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?)
}

/// A JPEG's width and height, from its first start-of-frame marker.
pub fn jpeg_size(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut at = 2;
    while at + 9 < bytes.len() {
        if bytes[at] != 0xff {
            return None;
        }
        let marker = bytes[at + 1];
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        // SOF0 to SOF15 but DHT (c4), JPG (c8) and DAC (cc)
        if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
            let height = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]);
            let width = u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]);
            return Some((width, height));
        }
        at += 2 + length;
    }
    None
}

// --- canopy heights: a COG's ranges ----------------------------------------------------------------

/// One image of a TIFF as a COG reader meets it (the tags `canopy` reads, through geotiff.js).
#[derive(Debug, Clone, PartialEq)]
pub struct TiffImage {
    /// `NewSubfileType` (254); a mask has the 4 bit set.
    pub new_subfile_type: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    /// Where each internal tile's bytes start and how many there are, row by row.
    pub offsets: Vec<u64>,
    pub byte_counts: Vec<u64>,
}

/// Why a TIFF's IFDs could not be read from the bytes at hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TiffError {
    /// The IFDs go on past the bytes given: at least this many are needed.
    Short(u64),
    Bad(String),
}

/// The images of a TIFF or BigTIFF from its first bytes, and where the last byte of its IFDs and
/// their values ends (a COG keeps them all at its start).
pub fn parse_tiff(bytes: &[u8]) -> Result<(Vec<TiffImage>, u64), TiffError> {
    let need = |end: u64| -> Result<(), TiffError> { if end > bytes.len() as u64 { Err(TiffError::Short(end)) } else { Ok(()) } };
    need(8)?;
    let little = match &bytes[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(TiffError::Bad("not a TIFF".into())),
    };
    let uint = |at: u64, size: u64| -> Result<u64, TiffError> {
        need(at + size)?;
        let b = &bytes[at as usize..(at + size) as usize];
        Ok(b.iter().enumerate().fold(0u64, |v, (i, &byte)| {
            let shift = if little { 8 * i } else { 8 * (size as usize - 1 - i) };
            v | (u64::from(byte) << shift)
        }))
    };
    let big = match uint(2, 2)? {
        42 => false,
        43 => true,
        v => return Err(TiffError::Bad(format!("TIFF version {v}"))),
    };
    let (count_size, entry_size, offset_size) = if big { (8, 20, 8) } else { (2, 12, 4) };
    let mut next = if big { uint(8, 8)? } else { uint(4, 4)? };
    let mut end = if big { 16 } else { 8 };
    let mut images = Vec::new();
    let mut seen = BTreeSet::new();
    while next != 0 {
        if !seen.insert(next) || images.len() > 64 {
            return Err(TiffError::Bad("the IFDs loop".into()));
        }
        let count = uint(next, count_size)?;
        let entries = next + count_size;
        let ifd_end = entries + count * entry_size + offset_size;
        need(ifd_end)?;
        end = end.max(ifd_end);
        let mut tags: HashMap<u16, Vec<u64>> = HashMap::new();
        for i in 0..count {
            let at = entries + i * entry_size;
            let tag = uint(at, 2)? as u16;
            let kind = uint(at + 2, 2)?;
            let n = uint(at + 4, if big { 8 } else { 4 })?;
            let value_at = at + if big { 12 } else { 8 };
            let item = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 | 16 | 17 | 18 => 8,
                _ => continue,
            };
            let size = n * item;
            let start = if size <= offset_size { value_at } else { uint(value_at, offset_size)? };
            if size > offset_size {
                end = end.max(start + size);
            }
            // only the integer tags a reader needs to find the tiles
            if [254, 256, 257, 322, 323, 324, 325].contains(&tag) && matches!(kind, 3 | 4 | 16) {
                need(start + size)?;
                let values = (0..n).map(|k| uint(start + k * item, item)).collect::<Result<Vec<_>, _>>()?;
                tags.insert(tag, values);
            }
        }
        next = uint(ifd_end - offset_size, offset_size)?;
        let one = |tag: u16| tags.get(&tag).and_then(|v| v.first()).map(|&v| v as u32);
        let image = TiffImage {
            new_subfile_type: one(254),
            width: one(256).ok_or_else(|| TiffError::Bad("an image without ImageWidth".into()))?,
            height: one(257).ok_or_else(|| TiffError::Bad("an image without ImageLength".into()))?,
            tile_width: one(322).ok_or_else(|| TiffError::Bad("an image without tiles".into()))?,
            tile_height: one(323).ok_or_else(|| TiffError::Bad("an image without tiles".into()))?,
            offsets: tags.remove(&324).unwrap_or_default(),
            byte_counts: tags.remove(&325).unwrap_or_default(),
        };
        if image.tile_width == 0 || image.tile_height == 0 || image.offsets.len() != image.byte_counts.len() {
            return Err(TiffError::Bad("an image's tiles do not add up".into()));
        }
        images.push(image);
    }
    Ok((images, end))
}

/// Ranges sorted, with those that overlap or touch joined.
pub fn merge_ranges(mut ranges: Vec<[u64; 2]>) -> Vec<[u64; 2]> {
    ranges.sort_unstable();
    let mut merged: Vec<[u64; 2]> = Vec::new();
    for [start, end] in ranges {
        match merged.last_mut() {
            Some(last) if start <= last[1] => last[1] = last[1].max(end),
            _ => merged.push([start, end]),
        }
    }
    merged
}

/// The byte ranges of a COG's internal tiles that `canopy(tile)` reads for each tile
/// (canopy.ts:114-131): the images that are not masks, largest first; the smallest with at least
/// `WANT` pixels across the tile ([`pick_image`]); the tile's [`window`] of it; every internal
/// tile the window touches (a tile without bytes reads as nothing).
pub fn canopy_ranges(images: &[TiffImage], tiles: &[Zxy]) -> Vec<[u64; 2]> {
    let mut readable: Vec<&TiffImage> = images.iter().filter(|im| !is_mask(im.new_subfile_type)).collect();
    // (JavaScript's sort is stable)
    readable.sort_by_key(|im| std::cmp::Reverse(im.width));
    let widths: Vec<u32> = readable.iter().map(|im| im.width).collect();
    let mut ranges = Vec::new();
    for &(z, x, y) in tiles {
        let place = place_in_file(x, y, z);
        let Some(i) = pick_image(&widths, place.span) else { continue };
        let image = readable[i];
        let win = window(image.width, &place);
        let across = image.width.div_ceil(image.tile_width) as usize;
        let down = image.height.div_ceil(image.tile_height) as usize;
        let (tw, th) = (image.tile_width as usize, image.tile_height as usize);
        let cols = win.x0 / tw..=((win.x0 + win.size - 1) / tw).min(across - 1);
        let rows = win.y0 / th..=((win.y0 + win.size - 1) / th).min(down - 1);
        for row in rows {
            for col in cols.clone() {
                let k = row * across + col;
                if let (Some(&offset), Some(&count)) = (image.offsets.get(k), image.byte_counts.get(k))
                    && count > 0
                {
                    ranges.push([offset, offset + count]);
                }
            }
        }
    }
    merge_ranges(ranges)
}

/// What a canopy file's recording holds: its URL, length and ranges.
fn canopy_manifest(out: &Path, key: &str) -> Option<(u64, Vec<[u64; 2]>)> {
    let manifest = read_json(&out.join(CANOPY).join(format!("{key}.json"))).ok()?;
    let length = manifest["length"].as_u64()?;
    let ranges = manifest["ranges"].as_array()?.iter().map(|r| Some([r[0].as_u64()?, r[1].as_u64()?])).collect::<Option<Vec<_>>>()?;
    Some((length, ranges))
}

/// The recorded bytes `[start, end)` of a canopy file.
fn canopy_range_path(out: &Path, key: &str, [start, end]: [u64; 2]) -> PathBuf {
    out.join(CANOPY).join(key).join(format!("{start}-{end}.bin"))
}

/// Whether a set's recording of the canopy file `key` holds everything `canopy` reads for
/// `tiles`: its head with every IFD, and the ranges under the tiles' windows.
pub fn canopy_held(out: &Path, key: &str, tiles: &[Zxy]) -> bool {
    canopy_missing(out, key, tiles).is_ok_and(|missing| missing.is_empty())
}

/// The ranges `canopy` reads for `tiles` that the recording of `key` does not hold.
pub fn canopy_missing(out: &Path, key: &str, tiles: &[Zxy]) -> Result<Vec<[u64; 2]>, String> {
    let (length, ranges) = canopy_manifest(out, key).ok_or_else(|| format!("no recording of canopy {key}"))?;
    for &range in &ranges {
        let bytes = fs::metadata(canopy_range_path(out, key, range)).map_err(|e| format!("canopy {key} {range:?}: {e}"))?.len();
        if bytes != range[1] - range[0] || range[1] > length {
            return Err(format!("canopy {key} {range:?}: {bytes} bytes"));
        }
    }
    let head = ranges.iter().find(|r| r[0] == 0).ok_or_else(|| format!("canopy {key} holds no head"))?;
    let head = fs::read(canopy_range_path(out, key, *head)).map_err(|e| e.to_string())?;
    let (images, end) = parse_tiff(&head).map_err(|e| format!("canopy {key}: {e:?}"))?;
    let mut wanted = canopy_ranges(&images, tiles);
    wanted.push([0, end.max(HEAD_BYTES).min(length)]);
    Ok(wanted.into_iter().filter(|w| !ranges.iter().any(|r| r[0] <= w[0] && w[1] <= r[1])).collect())
}

/// Records what `canopy` reads of the file `key` for `tiles` into the staging: its head (all its
/// IFDs, [`HEAD_BYTES`] at least) and the internal tiles under the tiles' windows.
fn record_canopy(key: &str, tiles: &[Zxy], staging: &Path) -> Result<(), String> {
    let url = rustradar_scene::canopy::file_url(key);
    let scratch = staging.join("scratch.bin");
    let mut want = HEAD_BYTES;
    let (head, images, length) = loop {
        let (bytes, length) = get_range(&url, 0, want, &scratch)?;
        match parse_tiff(&bytes) {
            Ok((images, end)) => {
                let keep = end.max(HEAD_BYTES).min(length) as usize;
                break (bytes[..keep.min(bytes.len())].to_vec(), images, length);
            }
            Err(TiffError::Short(need)) if need > want && need <= MAX_HEAD_BYTES && want < length => {
                want = need.next_multiple_of(HEAD_BYTES);
            }
            Err(e) => return Err(format!("{url}: {e:?}")),
        }
    };
    let mut ranges = canopy_ranges(&images, tiles);
    ranges.push([0, head.len() as u64]);
    let ranges = merge_ranges(ranges);
    for &[start, end] in &ranges {
        let bytes = if end <= head.len() as u64 { head[start as usize..end as usize].to_vec() } else { get_range(&url, start, end, &scratch)?.0 };
        write(&canopy_range_path(staging, key, [start, end]), &bytes)?;
    }
    let manifest = serde_json::json!({ "url": url, "length": length, "ranges": ranges, "tiles": tiles });
    write(&staging.join(CANOPY).join(format!("{key}.json")), &serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)?;
    println!("canopy {key}: {} ranges, {} bytes", ranges.len(), ranges.iter().map(|r| r[1] - r[0]).sum::<u64>());
    Ok(())
}

// --- Overture: a PMTiles extract -------------------------------------------------------------------

/// A PMTiles v3 header (https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PmHeader {
    pub root_offset: u64,
    pub root_length: u64,
    pub metadata_offset: u64,
    pub metadata_length: u64,
    pub leaf_offset: u64,
    pub leaf_length: u64,
    pub data_offset: u64,
    pub data_length: u64,
    pub addressed_tiles: u64,
    pub tile_entries: u64,
    pub tile_contents: u64,
    pub clustered: u8,
    pub internal_compression: u8,
    pub tile_compression: u8,
    pub tile_type: u8,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub min_lon_e7: i32,
    pub min_lat_e7: i32,
    pub max_lon_e7: i32,
    pub max_lat_e7: i32,
    pub center_zoom: u8,
    pub center_lon_e7: i32,
    pub center_lat_e7: i32,
}

/// The header's length.
pub const PM_HEADER: usize = 127;
/// Gzip, the only compression of directories handled here (Overture's).
const GZIP: u8 = 2;

impl PmHeader {
    pub fn parse(bytes: &[u8]) -> Result<PmHeader, String> {
        if bytes.len() < PM_HEADER || &bytes[..7] != b"PMTiles" || bytes[7] != 3 {
            return Err("not a PMTiles v3 archive".into());
        }
        let u = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or_default());
        let i = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or_default());
        Ok(PmHeader {
            root_offset: u(8),
            root_length: u(16),
            metadata_offset: u(24),
            metadata_length: u(32),
            leaf_offset: u(40),
            leaf_length: u(48),
            data_offset: u(56),
            data_length: u(64),
            addressed_tiles: u(72),
            tile_entries: u(80),
            tile_contents: u(88),
            clustered: bytes[96],
            internal_compression: bytes[97],
            tile_compression: bytes[98],
            tile_type: bytes[99],
            min_zoom: bytes[100],
            max_zoom: bytes[101],
            min_lon_e7: i(102),
            min_lat_e7: i(106),
            max_lon_e7: i(110),
            max_lat_e7: i(114),
            center_zoom: bytes[118],
            center_lon_e7: i(119),
            center_lat_e7: i(123),
        })
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut out = b"PMTiles\x03".to_vec();
        for v in [
            self.root_offset,
            self.root_length,
            self.metadata_offset,
            self.metadata_length,
            self.leaf_offset,
            self.leaf_length,
            self.data_offset,
            self.data_length,
            self.addressed_tiles,
            self.tile_entries,
            self.tile_contents,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&[self.clustered, self.internal_compression, self.tile_compression, self.tile_type, self.min_zoom, self.max_zoom]);
        for v in [self.min_lon_e7, self.min_lat_e7, self.max_lon_e7, self.max_lat_e7] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(self.center_zoom);
        out.extend_from_slice(&self.center_lon_e7.to_le_bytes());
        out.extend_from_slice(&self.center_lat_e7.to_le_bytes());
        out
    }
}

/// A PMTiles directory entry: a run of tiles from `tile_id` (`run_length` 0: a leaf directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmEntry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u32,
    pub run_length: u32,
}

/// A tile's id on PMTiles' Hilbert curve (`zxyToTileId` of the reference implementation).
pub fn tile_id(z: u32, x: u32, y: u32) -> u64 {
    let mut acc = 0u64;
    for tz in 0..z {
        acc += 1u64 << (2 * tz);
    }
    let (mut x, mut y) = (u64::from(x), u64::from(y));
    let mut d = 0u64;
    let mut s = (1u64 << z) / 2;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        // the reference's `rotate(s, xy, rx, ry)`: only the bits under `s` are read again, so
        // `s − 1 − x` is taken on them (the reference's negative numbers agree there)
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - (x & (s - 1));
                y = s - 1 - (y & (s - 1));
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    acc + d
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at).ok_or("a directory ends in a number")?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("a directory's number is too long".into())
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// A directory's entries from its (uncompressed) bytes.
pub fn parse_directory(bytes: &[u8]) -> Result<Vec<PmEntry>, String> {
    let mut at = 0;
    let n = varint(bytes, &mut at)? as usize;
    if n > bytes.len() {
        return Err("a directory with more entries than bytes".into());
    }
    let mut entries = vec![PmEntry { tile_id: 0, offset: 0, length: 0, run_length: 0 }; n];
    let mut last = 0u64;
    for e in &mut entries {
        last += varint(bytes, &mut at)?;
        e.tile_id = last;
    }
    for e in &mut entries {
        e.run_length = varint(bytes, &mut at)? as u32;
    }
    for e in &mut entries {
        e.length = varint(bytes, &mut at)? as u32;
    }
    // 0: right after the entry before; else the offset plus one
    let mut previous: Option<(u64, u32)> = None;
    for e in &mut entries {
        let v = varint(bytes, &mut at)?;
        e.offset = match previous {
            Some((offset, length)) if v == 0 => offset + u64::from(length),
            _ => v.saturating_sub(1),
        };
        previous = Some((e.offset, e.length));
    }
    Ok(entries)
}

/// A directory's (uncompressed) bytes.
pub fn directory_bytes(entries: &[PmEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, entries.len() as u64);
    let mut last = 0;
    for e in entries {
        put_varint(&mut out, e.tile_id - last);
        last = e.tile_id;
    }
    for e in entries {
        put_varint(&mut out, u64::from(e.run_length));
    }
    for e in entries {
        put_varint(&mut out, u64::from(e.length));
    }
    for (i, e) in entries.iter().enumerate() {
        let follows = i > 0 && e.offset == entries[i - 1].offset + u64::from(entries[i - 1].length);
        put_varint(&mut out, if follows { 0 } else { e.offset + 1 });
    }
    out
}

/// The entry of a directory that holds `id`: the last starting at or before it (a leaf, or a run
/// that reaches it).
pub fn find_entry(entries: &[PmEntry], id: u64) -> Option<PmEntry> {
    let i = entries.partition_point(|e| e.tile_id <= id).checked_sub(1)?;
    let e = entries[i];
    (e.run_length == 0 || id < e.tile_id + u64::from(e.run_length)).then_some(e)
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).map_err(|e| format!("gzip: {e}"))?;
    Ok(out)
}

fn gzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(bytes).map_err(|e| format!("gzip: {e}"))?;
    encoder.finish().map_err(|e| format!("gzip: {e}"))
}

/// A tile's corner `(x, y)` at zoom `z` as longitude and latitude ×10⁷.
fn corner_e7(z: u32, x: u32, y: u32) -> (i32, i32) {
    let n = f64::from(1u32 << z);
    let lon = f64::from(x) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / n)).sinh().atan().to_degrees();
    ((lon * 1e7).round() as i32, (lat * 1e7).round() as i32)
}

/// A PMTiles v3 archive of `tiles` (`(z, x, y)` and the bytes as stored, in the source's tile
/// compression), with the source's `metadata` (as stored) and one root directory, gzip'd.
pub fn write_extract(source: &PmHeader, metadata: &[u8], tiles: &BTreeMap<Zxy, Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut by_id: Vec<(u64, &Vec<u8>)> = tiles.iter().map(|(&(z, x, y), bytes)| (tile_id(z, x, y), bytes)).collect();
    by_id.sort_by_key(|(id, _)| *id);
    let mut entries = Vec::new();
    let mut data = Vec::new();
    for (id, bytes) in &by_id {
        entries.push(PmEntry { tile_id: *id, offset: data.len() as u64, length: bytes.len() as u32, run_length: 1 });
        data.extend_from_slice(bytes);
    }
    let root = gzip(&directory_bytes(&entries))?;
    let zooms = tiles.keys().map(|t| t.0);
    let (min_zoom, max_zoom) = (zooms.clone().min().unwrap_or(0) as u8, zooms.max().unwrap_or(0) as u8);
    let (mut west, mut south, mut east, mut north) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &(z, x, y) in tiles.keys() {
        let (w, n) = corner_e7(z, x, y);
        let (e, s) = corner_e7(z, x + 1, y + 1);
        (west, south, east, north) = (west.min(w), south.min(s), east.max(e), north.max(n));
    }
    let header = PmHeader {
        root_offset: PM_HEADER as u64,
        root_length: root.len() as u64,
        metadata_offset: (PM_HEADER + root.len()) as u64,
        metadata_length: metadata.len() as u64,
        leaf_offset: (PM_HEADER + root.len() + metadata.len()) as u64,
        leaf_length: 0,
        data_offset: (PM_HEADER + root.len() + metadata.len()) as u64,
        data_length: data.len() as u64,
        addressed_tiles: entries.len() as u64,
        tile_entries: entries.len() as u64,
        tile_contents: entries.len() as u64,
        clustered: 1,
        internal_compression: GZIP,
        tile_compression: source.tile_compression,
        tile_type: source.tile_type,
        min_zoom,
        max_zoom,
        min_lon_e7: west,
        min_lat_e7: south,
        max_lon_e7: east,
        max_lat_e7: north,
        center_zoom: max_zoom,
        center_lon_e7: ((i64::from(west) + i64::from(east)) / 2) as i32,
        center_lat_e7: ((i64::from(south) + i64::from(north)) / 2) as i32,
    };
    let mut out = header.bytes();
    out.extend_from_slice(&root);
    out.extend_from_slice(metadata);
    out.extend_from_slice(&data);
    Ok(out)
}

/// The tiles of an extract written by [`write_extract`] (one gzip'd root directory): each tile's
/// id and stored bytes.
pub fn read_extract(bytes: &[u8]) -> Result<(PmHeader, BTreeMap<u64, Vec<u8>>), String> {
    let header = PmHeader::parse(bytes)?;
    if header.internal_compression != GZIP || header.leaf_length != 0 {
        return Err("not an extract of this tool".into());
    }
    let slice = |offset: u64, length: u64| -> Result<&[u8], String> {
        bytes.get(offset as usize..(offset + length) as usize).ok_or_else(|| "the archive ends early".to_owned())
    };
    let entries = parse_directory(&gunzip(slice(header.root_offset, header.root_length)?)?)?;
    let mut tiles = BTreeMap::new();
    for e in entries {
        if e.run_length != 1 {
            return Err(format!("entry {} runs {}", e.tile_id, e.run_length));
        }
        tiles.insert(e.tile_id, slice(header.data_offset + e.offset, u64::from(e.length))?.to_vec());
    }
    Ok((header, tiles))
}

/// Whether an extract on disk holds every one of `tiles`.
pub fn extract_holds(path: &Path, tiles: &BTreeSet<Zxy>) -> bool {
    let Ok(bytes) = fs::read(path) else { return false };
    let Ok((_, held)) = read_extract(&bytes) else { return false };
    tiles.iter().all(|&(z, x, y)| held.contains_key(&tile_id(z, x, y)))
}

/// Builds the Overture extract of `tiles` (z14) into the staging: the catalogue's latest release
/// (`overture.ts`: `c?.latest ?? KNOWN_RELEASE`), its buildings archive read with range requests
/// as the app reads it (header, root directory, the leaf directories on the way, the tiles), and
/// each tile's bytes copied as stored.
fn build_overture(tiles: &BTreeSet<Zxy>, staging: &Path) -> Result<(), String> {
    let catalog_bytes = get(overture::CATALOG)?;
    let catalog: serde_json::Value = serde_json::from_slice(&catalog_bytes).map_err(|e| format!("{}: {e}", overture::CATALOG))?;
    let release = overture::release_of(Some(&catalog));
    let url = overture::archive_url(&release);
    let scratch = staging.join("scratch.bin");
    let (head, _) = get_range(&url, 0, 16_384, &scratch)?;
    let header = PmHeader::parse(&head)?;
    if header.internal_compression != GZIP {
        return Err(format!("{url}: directories compressed with {}, not gzip", header.internal_compression));
    }
    let range = |offset: u64, length: u64| -> Result<Vec<u8>, String> {
        if offset + length <= head.len() as u64 {
            Ok(head[offset as usize..(offset + length) as usize].to_vec())
        } else {
            Ok(get_range(&url, offset, offset + length, &scratch)?.0)
        }
    };
    let root = parse_directory(&gunzip(&range(header.root_offset, header.root_length)?)?)?;
    let mut leaves: HashMap<u64, Vec<PmEntry>> = HashMap::new();
    let mut found = BTreeMap::new();
    for &(z, x, y) in tiles {
        let id = tile_id(z, x, y);
        let mut entries = root.clone();
        let mut tile = None;
        for _depth in 0..4 {
            let Some(entry) = find_entry(&entries, id) else { break };
            if entry.run_length > 0 {
                tile = Some(entry);
                break;
            }
            let leaf = match leaves.get(&entry.offset) {
                Some(leaf) => leaf.clone(),
                None => {
                    let leaf = parse_directory(&gunzip(&range(header.leaf_offset + entry.offset, u64::from(entry.length))?)?)?;
                    leaves.insert(entry.offset, leaf.clone());
                    leaf
                }
            };
            entries = leaf;
        }
        let tile = tile.ok_or_else(|| format!("{url} has no tile {z}/{x}/{y}"))?;
        found.insert((z, x, y), range(header.data_offset + tile.offset, u64::from(tile.length))?);
    }
    let metadata = range(header.metadata_offset, header.metadata_length)?;
    let extract = write_extract(&header, &metadata, &found)?;
    write(&staging.join(OVERTURE_CATALOG), &catalog_bytes)?;
    write(&staging.join(OVERTURE_FILE), &extract)?;
    println!("overture {release}: {} tiles, {} bytes", found.len(), extract.len());
    Ok(())
}

// --- measured radar volumes ------------------------------------------------------------------------

/// Whether a set holds a source's recorded volume: its `volume.json` and every blob it names.
pub fn radar_held(out: &Path, source: &str) -> bool {
    radar_files(out, source).is_ok_and(|files| files.iter().all(|f| out.join(RADAR).join(source).join(f).is_file()))
}

/// The files a recorded volume consists of, by its `volume.json`.
pub fn radar_files(out: &Path, source: &str) -> Result<Vec<String>, String> {
    let info: rustradar_core::model::RadarVolume = serde_json::from_value(read_json(&out.join(RADAR).join(source).join("volume.json"))?)
        .map_err(|e| format!("{source}'s volume.json: {e}"))?;
    let mut files = vec!["volume.json".to_owned(), "ground.bin".to_owned(), "coverage.bin".to_owned()];
    files.extend(info.levels.iter().map(|km| format!("{km}.bin")));
    Ok(files)
}

/// One measured volume through the backend's own fetch (`backend::mrms::fetch`), as
/// `radar_volumes` and `radar_image` serve it, into `dir`.
fn record_radar(source: &str, dir: &Path) -> Result<(), String> {
    let http = reqwest::Client::builder().build().map_err(|e| format!("an HTTP client: {e}"))?;
    println!("recording the {source} volume (the backend's fetch and contours)");
    let volume = runtime()
        .block_on(async {
            match source {
                "mrms" => rustradar_backend::mrms::fetch(&http).await,
                _ => Err(format!("no backend fetch records {source}").into()),
            }
        })
        .map_err(|e| format!("the {source} volume: {e}"))?;
    let info = volume.info();
    write(&dir.join("volume.json"), &serde_json::to_vec_pretty(&info).map_err(|e| e.to_string())?)?;
    let names = ["ground".to_owned(), "coverage".to_owned()].into_iter().chain(info.levels.iter().map(|km| km.to_string()));
    for name in names {
        let bytes = volume.image(&name).ok_or_else(|| format!("the {source} volume has no {name}"))?;
        write(&dir.join(format!("{name}.bin")), &bytes)?;
    }
    println!("{source}: {} heights, {}", info.levels.len(), utc(info.time));
    Ok(())
}

/// The regular files of a tar archive (`backend::dwd`'s `untar`): name and bytes.
pub fn untar(data: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 512 <= data.len() {
        let header = &data[at..at + 512];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let name = String::from_utf8_lossy(&header[..100]).trim_end_matches('\0').to_string();
        let size = String::from_utf8_lossy(&header[124..136]);
        let size = usize::from_str_radix(size.trim_end_matches(['\0', ' ']).trim(), 8).unwrap_or(0);
        at += 512;
        let Some(body) = data.get(at..at + size) else { break };
        // regular files only ('0' or NUL)
        if matches!(header[156], b'0' | 0) {
            out.push((name, body));
        }
        at += size.div_ceil(512) * 512;
    }
    out
}

/// Whether a set holds DWD's raw analysis: `source.json` and the HDF5 member it names.
pub fn dwd_held(out: &Path) -> bool {
    let Ok(source) = read_json(&out.join(DWD).join("source.json")) else { return false };
    let Some(member) = source["member"].as_str() else { return false };
    fs::read(out.join(DWD).join(member)).is_ok_and(|bytes| bytes.starts_with(HDF5_MAGIC))
}

/// DWD's newest WN run from its folder (`composite_wn_<date>_<time>.tar`, the newest by name),
/// its analysis member (`…_000-hd5`, lead time 0) kept as it came into `dir`, with `source.json`.
fn record_dwd(dir: &Path) -> Result<(), String> {
    let listing = String::from_utf8_lossy(&get(DWD_DIRECTORY)?).into_owned();
    let newest = listing
        .split("href=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .filter(|name| name.starts_with("composite_wn_2") && name.ends_with(".tar"))
        .max()
        .ok_or("no composite_wn_<date>_<time>.tar in DWD's folder")?
        .to_owned();
    let url = format!("{DWD_DIRECTORY}{newest}");
    let tar = get(&url)?;
    let (member, body) = untar(&tar)
        .into_iter()
        .find(|(name, _)| name.ends_with("_000-hd5"))
        .ok_or_else(|| format!("{url} holds no analysis (…_000-hd5)"))?;
    // (a plain file name: the archive is not trusted)
    if member.starts_with('.') || !member.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) {
        return Err(format!("{url}: a member named {member:?}"));
    }
    if !body.starts_with(HDF5_MAGIC) {
        return Err(format!("{url}: {member} is not HDF5"));
    }
    write(&dir.join(&member), body)?;
    let source = serde_json::json!({ "url": url, "member": member, "fetched": now() });
    write(&dir.join("source.json"), &serde_json::to_vec_pretty(&source).map_err(|e| e.to_string())?)?;
    println!("dwd: {member}, {} bytes", body.len());
    Ok(())
}

// --- the NOTICEs ---------------------------------------------------------------------------------

/// The NOTICE line that says when the set was downloaded whole (a top-up reads it back).
const DOWNLOADED_ON: &str = "Downloaded by `cargo xtask fixtures` (crates/xtask/src/fixtures.rs) on ";
/// The NOTICE line that says when the set was last topped up.
const TOPPED_UP_ON: &str = "Topped up on ";
/// The NOTICE line with Esri's credit.
const ESRI_CREDIT: &str = "  Credit as the service asks: ";

/// What the NOTICE says the set covers: each view, `lat, lon, zoom, pitch, bearing` (and its
/// relief), with the sizes and the sources it holds tiles of.
fn coverage() -> String {
    let mut lines = Vec::new();
    for (views, cover, sizes) in COVERS {
        let sources = cover.sources().join(", ");
        let sizes = sizes.iter().map(|[w, h]| format!("{w}x{h}")).collect::<Vec<_>>().join(", ");
        for view in views {
            let View { name, latitude, longitude, zoom, pitch, bearing, ground, peak } = *view;
            let relief = if peak > 0.0 { format!("; ground {ground} m, peak {peak} m") } else { String::new() };
            lines.push(format!("  {name} ({latitude}, {longitude}, {zoom}, {pitch}, {bearing}{relief}) at {sizes}: {sources}"));
        }
    }
    lines.join("\n")
}

/// The tiles' NOTICE: where each folder's tiles come from, under which terms, and the credit
/// each asks for.
fn notice(found: &Found) -> String {
    let terrain = TERRAIN_ATTRIBUTION_FULL.iter().map(|line| format!("    - {line}\n")).collect::<String>();
    let topped_up = found.topped_up.as_ref().map_or_else(String::new, |date| {
        format!(
            "{TOPPED_UP_ON}{date} UTC with the planned tiles the set lacked, from the same\n\
             OpenFreeMap build (the tiles held before were left as they were).\n"
        )
    });
    let mosaic = found.worldcover_id.as_deref().unwrap_or("<id>");
    format!(
        "Tile fixtures of crates/map (docs/port/00-architecture-and-plan.md, items E16, T3 and X1, section 8)

{DOWNLOADED_ON}{downloaded} UTC.
{topped_up}Each folder holds one source's tiles as <z>/<x>/<y>.<ext>, the bytes the server sent with the
HTTP compression undone; an empty <y>.none marks a tile the server had nothing for (404/204).
They hold every tile the map's tile selection asks for at these views (the plan's section 8 and
the shots of its stages 3 to 10; lat, lon, zoom, pitch, bearing) at the canvas sizes in CSS
pixels named, of the sources named, and OpenFreeMap's world at zooms 0 to 2:
{coverage}
The data beside this folder (clouds, canopy heights, Overture buildings, radar volumes) has its
own NOTICE, in the folder above.

These files are third-party data, kept here only so that the tests and headless screenshots run
without the network. They are not covered by the licence of this repository's code; each stays
under its owner's terms, below. Do not use them for anything else.

openfreemap/
  Vector tiles (OpenMapTiles schema) from OpenFreeMap, https://openfreemap.org
  Template: {ofm_template}
  (read from the TileJSON at {ofm_tilejson}, kept as openfreemap/tilejson.json)
  Credit as the TileJSON asks: {ofm_attribution}
  Map data © OpenStreetMap contributors, available under the Open Database License (ODbL) 1.0,
  https://www.openstreetmap.org/copyright. Schema © OpenMapTiles, https://openmaptiles.org
  (CC-BY 4.0). OpenFreeMap's terms: https://openfreemap.org/tos/

esri/
  Esri World Imagery, {esri_service}
  Template: {esri_template}
  Credit as the service asks: {esri_copyright}
  Under Esri's terms of use for the service: https://goto.arcgisonline.com/maps/World_Imagery

terrarium/
  Terrain Tiles (Mapzen / Tilezen Joerd, terrarium encoding) from the AWS Open Data registry,
  https://registry.opendata.aws/terrain-tiles/
  Template: {terrarium}
  Attribution as https://github.com/tilezen/joerd/blob/master/docs/attribution.md asks:
{terrain}
mapterhorn/
  Terrain tiles (terrarium encoding, 512 px lossless WebP) from Mapterhorn, https://mapterhorn.com
  Template: {mapterhorn}
  Where Mapterhorn has no tile (an empty .none), terrarium/ holds Mapzen's of the same z/x/y.
  Credit as the app gives it: {terrain_line}
  Mapterhorn's sources and their licences: https://mapterhorn.com/attribution/

rainviewer/
  One weather radar frame from RainViewer, https://www.rainviewer.com, frame of {frame} UTC
  ({frame_unix}), the latest past frame of {rainviewer_maps} (kept as rainviewer/weather-maps.json).
  Template: {rainviewer_template}
  Weather data by RainViewer. The RainViewer API is free for personal and educational use, with
  this credit and a link to rainviewer.com: https://www.rainviewer.com/api.html,
  https://www.rainviewer.com/terms.html

streets-gl/
  Vector tiles from Streets GL (https://github.com/StrandedKitty/streets-gl, by StrandedKitty)
  Template: {sgl}
  Map data © OpenStreetMap contributors, available under the Open Database License (ODbL) 1.0,
  https://www.openstreetmap.org/copyright

openrailwaymap/
  Vector tiles from OpenRailwayMap, https://openrailwaymap.app: the tracks (railway_line_high,
  railway_text_km) at zooms 7 to 12 and stations' platforms (standard_railway_platforms,
  standard_railway_platform_edges) at zoom 15, fetched through the app's own backend
  (backend::rail::tile, identified as the app is to OpenRailwayMap).
  Credit as the app gives it: Rail: OpenRailwayMap, © OpenStreetMap contributors
  Map data © OpenStreetMap contributors, available under the Open Database License (ODbL) 1.0,
  https://www.openstreetmap.org/copyright

nmd/
  Land cover pictures (256 px PNG) of Naturvårdsverket's Nationella Marktäckedata 2023 (NMD2023,
  v2) from its WMS, https://geodata.naturvardsverket.se/geoserver/wms (layer
  lc-nmd:LC.LandCoverRaster.Bas.2023.v2.x, EPSG:3857, each tile's box), tiles in Sweden only.
  Naturvårdsverket publishes NMD under CC0 1.0. Credit as the app gives it: Land cover: NMD2023
  Naturvårdsverket

worldcover/
  Land cover pictures (256 px PNG, the class code in the red channel) of ESA WorldCover 2021 from
  Microsoft's Planetary Computer, mosaic {mosaic} (its registration kept as
  worldcover/register.json).
  Template: https://planetarycomputer.microsoft.com/api/data/v1/mosaic/{mosaic}/tiles/WebMercatorQuad/{{z}}/{{x}}/{{y}}@1x?assets=map&collection=esa-worldcover&format=png
  © ESA WorldCover project 2021 / Contains modified Copernicus Sentinel data (2021) processed by
  ESA WorldCover consortium, under CC BY 4.0, https://esa-worldcover.org/en/data-access
",
        downloaded = found.downloaded,
        coverage = coverage(),
        ofm_template = found.openfreemap_template,
        ofm_tilejson = OPENFREEMAP_TILEJSON,
        ofm_attribution = found.openfreemap_attribution,
        esri_service = ESRI_IMAGERY,
        esri_template = esri_template(),
        esri_copyright = found.esri_copyright,
        terrarium = TERRARIUM_URL,
        mapterhorn = MAPTERHORN_URL,
        terrain_line = TERRAIN_ATTRIBUTION,
        frame = utc(found.rainviewer_time),
        frame_unix = found.rainviewer_time,
        rainviewer_maps = RAINVIEWER_MAPS,
        rainviewer_template = found.rainviewer_template,
        sgl = SGL_TILES,
    )
}

/// The data's NOTICE, from what the folder holds.
fn data_notice(out: &Path) -> Result<String, String> {
    let fetched = read_json(&out.join(CLOUDS_RECORD)).ok().and_then(|r| r["fetched"].as_i64()).map_or_else(|| "-".to_owned(), utc);
    let mut canopy = String::new();
    let mut keys: Vec<String> = files_below(&out.join(CANOPY))?
        .iter()
        .filter_map(|f| f.to_str()?.strip_suffix(".json").map(str::to_owned))
        .collect();
    keys.sort();
    for key in keys {
        let url = read_json(&out.join(CANOPY).join(format!("{key}.json"))).ok().and_then(|m| m["url"].as_str().map(str::to_owned)).unwrap_or_default();
        canopy.push_str(&format!("  {key}: {url}\n"));
    }
    let release = read_json(&out.join(OVERTURE_CATALOG)).ok().map(|c| overture::release_of(Some(&c))).unwrap_or_default();
    let tiles = fs::read(out.join(OVERTURE_FILE))
        .ok()
        .and_then(|b| read_extract(&b).ok())
        .map(|(_, tiles)| tiles.len())
        .unwrap_or_default();
    let mut radar = String::new();
    for source in RADAR_SOURCES {
        if let Ok(info) = read_json(&out.join(RADAR).join(source).join("volume.json"))
            .and_then(|j| serde_json::from_value::<rustradar_core::model::RadarVolume>(j).map_err(|e| e.to_string()))
        {
            radar.push_str(&format!("  {source}: scan of {} UTC ({}), heights {:?} km\n", utc(info.time), info.time, info.levels));
        }
    }
    let dwd = read_json(&out.join(DWD).join("source.json"))
        .ok()
        .map(|s| format!("{} from {}", s["member"].as_str().unwrap_or_default(), s["url"].as_str().unwrap_or_default()))
        .unwrap_or_default();
    Ok(format!(
        "Data fixtures of crates/map beside the tiles (docs/port/00-architecture-and-plan.md, item X1)

Written by `cargo xtask fixtures` (crates/xtask/src/fixtures.rs). The tiles are in tiles/, with
their own NOTICE. These files are third-party data, kept here only so that the tests and headless
screenshots run without the network. They are not covered by the licence of this repository's
code; each stays under its owner's terms, below. Do not use them for anything else.

clouds/
  EUMETSAT's world cloud map (infrared 10.8 µm, mumi:worldcloudmap_ir108) from EUMETView's WMS,
  4096 x 1934 JPEG over longitude -180 to 180 and latitude -85 to 85, fetched {fetched} UTC:
  {cloud_url}
  Credit as the app gives it: Clouds: © EUMETSAT. Under EUMETSAT's data policy,
  https://www.eumetsat.int/data-policy

canopy/
  Byte ranges of Meta and the World Resources Institute's canopy height map v2 (Cloud-Optimised
  GeoTIFFs, one per zoom-10 web mercator tile named by its quadkey) as Taylor Geospatial Engine
  Labs mirror it on Source Cooperative: each <quadkey>.json names its file, length and the ranges
  held ([start, end), in <quadkey>/<start>-<end>.bin): the file's head with all its IFDs and the
  internal tiles under the close-up tiles' windows.
{canopy}  Under CC BY 4.0. Credit as the app gives it: Canopy height: Meta & WRI (CC BY 4.0), via TGE Labs

overture/
  buildings.pmtiles: {tiles} zoom-14 tiles of Overture Maps' buildings, release {release} (the latest the
  catalogue at {catalog} named, kept as catalog.json), copied as stored from
  {archive} into a PMTiles v3 archive with one root directory.
  Credit as the app gives it: Building heights: © Overture Maps Foundation, © OpenStreetMap (ODbL).
  Overture's buildings are under the Open Database License (ODbL) 1.0,
  https://docs.overturemaps.org/attribution/

radar/
  mrms/: one measured reflectivity volume as the app's backend served it (radar_volumes'
  volume.json; radar_image's ground.bin, coverage.bin and <km>.bin, the isobands encoded as
  backend::radar::contour describes), made by the backend's own fetch:
{radar}  from NOAA's Multi-Radar Multi-Sensor MergedReflectivityQC in the NOAA Open Data
  Dissemination bucket noaa-mrms-pds, https://registry.opendata.aws/noaa-mrms-pds/ (NOAA data,
  open to all).
  dwd/: Deutscher Wetterdienst's WN composite analysis (ODIM HDF5) as DWD publishes it, kept as it
  came: {dwd} (named in source.json). Source: Deutscher Wetterdienst, under DWD's terms of use,
  https://www.dwd.de/EN/service/copyright/copyright_node.html
",
        cloud_url = CLOUD_URL,
        catalog = overture::CATALOG,
        archive = overture::archive_url(&release),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_world_is_there_at_zooms_0_to_2() {
        let plan = plan();
        let ofm = &plan[OPENFREEMAP];
        assert_eq!(ofm.iter().filter(|t| t.0 <= 2).count(), 1 + 4 + 16);
        for z in WORLD_ZOOMS {
            for x in 0..1 << z {
                for y in 0..1 << z {
                    assert!(ofm.contains(&(z, x, y)), "{z}/{x}/{y}");
                }
            }
        }
    }

    #[test]
    fn each_view_takes_tiles_at_deck_s_zoom_for_each_source() {
        let plan = plan();
        // round(zoom + log2(512 / size)), clamped: vector 512 px to 14, imagery 256 px to 19,
        // hillshade 256 px to 12, radar 256 px to 7
        let expected = [
            ("V3", [9, 10, 10, 7]),
            ("V4", [14, 15, 12, 7]),
            ("V5", [14, 17, 12, 7]),
            ("V6", [13, 14, 12, 7]),
        ];
        for (view, zooms) in expected {
            let view = VIEWS.iter().find(|v| v.name == view).unwrap();
            for (source, z) in [OPENFREEMAP, ESRI, TERRARIUM, RAINVIEWER].into_iter().zip(zooms) {
                // the tile under the view's centre, at that zoom
                let t = tile_at(view.latitude, view.longitude, z);
                assert!(plan[source].contains(&zxy(t)), "{} {source} {}", view.name, t.id());
            }
        }
        // the dark and satellite views (JavaScript's round: 4.5 + 1 goes to 6), the dark map's
        // either side of the swap (11.9 and 12.1 both round to 12), the radar's V2 (6), the
        // Alps' hillshade and imagery (9.45 + 1 to 10) and map (9)
        let expected: [(&str, &[(&str, u32)]); 13] = [
            ("V1", &[(OPENFREEMAP, 2), (ESRI, 3)]),
            ("V2", &[(OPENFREEMAP, 5), (ESRI, 6), (RAINVIEWER, 6)]),
            ("V7", &[(OPENFREEMAP, 6), (ESRI, 7)]),
            ("V8", &[(OPENFREEMAP, 3), (ESRI, 4)]),
            ("V3 at 11.9", &[(OPENFREEMAP, 12)]),
            ("V3 at 12.1", &[(OPENFREEMAP, 12)]),
            ("V4 at 11.9", &[(OPENFREEMAP, 12)]),
            ("V4 at 12.1", &[(OPENFREEMAP, 12)]),
            ("B5 night", &[(OPENFREEMAP, 2)]),
            ("B5 day", &[(OPENFREEMAP, 2)]),
            ("V2 at 45", &[(RAINVIEWER, 6)]),
            ("Alps", &[(OPENFREEMAP, 9), (ESRI, 10), (TERRARIUM, 10)]),
            ("Zermatt", &[(TERRARIUM, 12), (OPENFREEMAP, 14), (STREETS_GL, 16), (MAPTERHORN, 16)]),
        ];
        let all: Vec<View> = COVERS.iter().flat_map(|(views, ..)| views.iter().copied()).collect();
        for (name, zooms) in expected {
            let view = all.iter().find(|v| v.name == name).unwrap();
            for &(source, z) in zooms {
                let t = tile_at(view.latitude, view.longitude, z);
                assert!(plan[source].contains(&zxy(t)), "{name} {source} {}", t.id());
            }
        }
    }

    #[test]
    fn each_cover_holds_its_sources_only() {
        for (views, cover, sizes) in COVERS {
            let taken: BTreeSet<&'static str> =
                views.iter().flat_map(|v| sizes.iter().map(|&size| view_tiles(v, size, cover))).flat_map(|t| t.into_keys()).collect();
            let named: BTreeSet<&'static str> = cover.sources().into_iter().collect();
            assert_eq!(taken, named, "{:?}", views.iter().map(|v| v.name).collect::<Vec<_>>());
        }
        assert_eq!(Cover::ALL.sources(), [OPENFREEMAP, STREETS_GL, ESRI, TERRARIUM, RAINVIEWER]);
        assert_eq!(Cover::BASEMAPS.sources(), [OPENFREEMAP, ESRI]);
        assert_eq!(Cover::DARK.sources(), [OPENFREEMAP]);
        assert_eq!(Cover::RADAR.sources(), [RAINVIEWER]);
        assert_eq!(Cover::REALISTIC.sources(), [OPENFREEMAP, STREETS_GL, OPENRAILWAYMAP, NMD, WORLDCOVER]);
        assert_eq!(Cover::RELIEF.sources(), [OPENFREEMAP, ESRI, TERRARIUM]);
        assert_eq!(Cover::TERRAIN.sources(), [OPENFREEMAP, STREETS_GL, TERRARIUM, MAPTERHORN]);
        assert_eq!(Cover::SKYLINE.sources(), [TERRARIUM, MAPTERHORN]);
        // what V3 to V6 hold is what they held before the other covers came (E16's plan)
        for view in &VIEWS {
            for size in SIZES {
                let all = view_tiles(view, size, Cover::ALL);
                let basemaps = view_tiles(view, size, Cover::BASEMAPS);
                for (source, tiles) in &basemaps {
                    assert!(tiles.is_subset(&all[source]), "{} {source}", view.name);
                }
            }
        }
    }

    #[test]
    fn the_dark_map_s_flat_views_take_the_plain_buildings_far_tiles() {
        // at 12.1 on the flat map the buildings' LOD takes OpenFreeMap's z13 tiles (flat roofs out
        // to 25 km, G:1033-1042); its z16 level has no source on the dark map; on the globe at
        // 11.9 there are no buildings
        let at = |name: &str| *DARK_VIEWS.iter().find(|v| v.name == name).unwrap();
        for size in SIZES {
            let flat = view_tiles(&at("V3 at 12.1"), size, Cover::DARK);
            let z13 = flat[OPENFREEMAP].iter().filter(|t| t.0 == 13).count();
            assert!(z13 > 4, "{size:?}: {z13}");
            assert!(flat[OPENFREEMAP].iter().all(|t| t.0 <= 13), "{size:?}");
            let globe = view_tiles(&at("V3 at 11.9"), size, Cover::DARK);
            assert!(globe[OPENFREEMAP].iter().all(|t| t.0 <= 12), "{size:?}");
        }
        // and the same city LOD tiles for V5 on the dark map are held by its every-source cover
        let v5 = VIEWS.iter().find(|v| v.name == "V5").unwrap();
        for size in SIZES {
            let dark = view_tiles(v5, size, Cover::DARK);
            assert!(dark.keys().eq([OPENFREEMAP].iter()), "{size:?}");
            assert!(dark[OPENFREEMAP].is_subset(&view_tiles(v5, size, Cover::ALL)[OPENFREEMAP]));
        }
    }

    #[test]
    fn the_vector_tiles_of_12_1_are_among_those_of_11_9() {
        // B2: crossing zoom 12 at V3 asks for no new tile of the map, at the shot's size and the
        // references'. (Not so under V4's pitch: at 1100x720 the flat map at 12.1 takes one z12
        // tile, 2252/1195, that the globe at 11.9 does not.)
        let view = |name: &str| *DARK_VIEWS.iter().find(|v| v.name == name).unwrap();
        for size in SIZES {
            let deck = |view: View| -> BTreeSet<Zxy> {
                tile_indices(&viewport_for(&view.state(), size), &VECTOR_TILES).into_iter().map(zxy).collect()
            };
            let (globe, flat) = (deck(view("V3 at 11.9")), deck(view("V3 at 12.1")));
            assert!(flat.is_subset(&globe), "{size:?}: {:?}", flat.difference(&globe).collect::<Vec<_>>());
        }
        let v4 = |name: &str| tile_indices(&viewport_for(&view(name).state(), SIZES[0]), &VECTOR_TILES);
        assert!(!v4("V4 at 12.1").iter().all(|tile| v4("V4 at 11.9").contains(tile)));
    }

    #[test]
    fn a_view_on_relief_is_taken_with_the_terrain_off_and_on() {
        let zermatt = TERRAIN_VIEWS[0];
        let states = zermatt.states();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].position, None);
        assert_eq!(states[1].position, Some([0.0, 0.0, 1608.0]));
        assert_eq!(zermatt.z_range(), Some([0.0, 4634.0]));
        // on the globe the target stays (the app leaves `position` unset there)
        assert_eq!(View { zoom: 11.0, ..zermatt }.states().len(), 1);
        assert_eq!(VIEWS[2].states().len(), 1);
        assert_eq!(VIEWS[2].z_range(), None);
        // the 2D layers' tiles from both cameras; the raised one takes tiles the plain one does not
        let size = TERRAIN_SIZE[0];
        let plain = View { ground: 0.0, peak: 0.0, ..zermatt };
        let on = view_tiles(&zermatt, size, Cover::TERRAIN);
        let off = view_tiles(&plain, size, Cover::TERRAIN);
        for (source, tiles) in off.iter().filter(|(source, _)| **source != MAPTERHORN) {
            assert!(tiles.is_subset(&on[source]), "{source}");
        }
        assert!(on[OPENFREEMAP].len() > off[OPENFREEMAP].len(), "{} {}", on[OPENFREEMAP].len(), off[OPENFREEMAP].len());
        // the 3D surface as drawn with the terrain on alone: from the raised camera, its volumes up
        // to the peak (and the camera's ground tile)
        let raised = viewport_for(&states[1], size);
        let surface: BTreeSet<Zxy> =
            distance_tile_indices(&raised, &surface_lod(), Some([0.0, 4634.0]), 0.0).into_iter().map(zxy).collect();
        let mut expected = surface.clone();
        expected.insert(zxy(ground_tile(&zermatt)));
        assert_eq!(on[MAPTERHORN], expected);
        assert_ne!(on[MAPTERHORN], off[MAPTERHORN]);
    }

    #[test]
    fn the_surface_takes_mapterhorn_tiles_by_distance_and_the_ground_tile() {
        assert_eq!(surface_lod(), Lod { levels: (0..=16).collect(), bias: 0.5, max_distance: f64::INFINITY });
        let zermatt = TERRAIN_VIEWS[0];
        let tiles = view_tiles(&zermatt, TERRAIN_SIZE[0], Cover::TERRAIN);
        let surface = &tiles[MAPTERHORN];
        // the camera's ground: zoom 12 under the centre (terrain-layer.ts:224-236)
        assert_eq!(ground_tile(&zermatt), TileIndex::new(2136, 1456, 12));
        assert!(surface.contains(&(12, 2136, 1456)));
        // fine close in, coarse further off
        let zooms: BTreeSet<u32> = surface.iter().map(|t| t.0).collect();
        assert!(zooms.contains(&16) && zooms.iter().any(|&z| z <= 12), "{zooms:?}");
        // at pitch 60 the far edge is a few kilometres off; to the horizon at pitch 75, coarse
        // tiles too
        assert!(surface.iter().all(|t| t.0 >= 11), "{surface:?}");
        let skyline = view_tiles(&SKYLINE_VIEWS[0], TERRAIN_SIZE[0], Cover::SKYLINE);
        assert!(skyline[MAPTERHORN].iter().any(|t| t.0 <= 10), "{:?}", skyline[MAPTERHORN]);
        assert!(skyline[MAPTERHORN].contains(&(12, 2136, 1456)));
        // on the globe, at sea level: the surface at the globe's zooms
        let globe = View::at("", 46.0, 7.75, 9.45, 0.0, 0.0).on_relief(0.0, 4634.0);
        let globe = view_tiles(&globe, SHOT_SIZE[0], Cover::SKYLINE);
        assert!(!globe[MAPTERHORN].is_empty() && globe[MAPTERHORN].iter().all(|t| t.0 <= 12), "{:?}", globe[MAPTERHORN]);
    }

    #[test]
    fn the_realistic_views_take_rails_platforms_and_land_cover() {
        let station = CLOSE_UP_VIEWS.iter().find(|v| v.name == "Stockholm C").unwrap();
        let tiles = view_tiles(station, SHOT_SIZE[0], Cover::REALISTIC);
        let rails = &tiles[OPENRAILWAYMAP];
        // the tracks at deck's zoom, clamped to 12; platforms at 15 within 4 km of the camera
        assert!(rails.contains(&zxy(tile_at(station.latitude, station.longitude, 12))));
        assert!(rails.contains(&zxy(tile_at(station.latitude, station.longitude, 15))));
        assert!(rails.iter().all(|t| (7..=12).contains(&t.0) || t.0 == 15), "{rails:?}");
        // the land cover at 14 (16 + 1 clamped) and the close-up tiles' at 16: NMD in Sweden, and
        // WorldCover for every one
        let nmd = &tiles[NMD];
        assert!(nmd.contains(&zxy(tile_at(station.latitude, station.longitude, 14))));
        assert!(nmd.contains(&zxy(tile_at(station.latitude, station.longitude, 16))));
        assert_eq!(nmd, &tiles[WORLDCOVER]);
        // outside Sweden WorldCover alone
        let mut alps = BTreeMap::new();
        land_cover(&mut alps, [tile_at(46.02, 7.75, 14)]);
        assert_eq!(alps.keys().copied().collect::<Vec<_>>(), [WORLDCOVER]);
    }

    #[test]
    fn the_close_up_is_the_tree_tile_under_the_centre_and_its_neighbours() {
        let park = CLOSE_UP_VIEWS[0];
        let tiles = close_up_tiles(&park);
        assert_eq!(tiles.len(), 9);
        let centre = tile_at(park.latitude, park.longitude, 16);
        assert_eq!(tiles[4], centre);
        assert!(tiles.iter().all(|t| t.z == 16 && t.x.abs_diff(centre.x) <= 1 && t.y.abs_diff(centre.y) <= 1));
        // each is a tree tile of the view
        let trees = tree_tiles(&park, SHOT_SIZE[0][1]);
        assert!(tiles.iter().all(|t| trees.contains(t)));
        // between zooms 14 and 15.5 the trees' tiles are OpenFreeMap's z14; on the globe none
        assert!(close_up_tiles(&View::at("", 59.33, 18.07, 14.5, 0.0, 0.0)).iter().all(|t| t.z == 14));
        assert!(close_up_tiles(&VIEWS[0]).is_empty());
        // the data: their canopy, and Overture's z14 tiles under them
        let data = data_plan();
        assert!(tiles.iter().all(|t| data.canopy.contains(&zxy(*t))));
        assert!(data.overture.contains(&(14, centre.x >> 2, centre.y >> 2)));
        assert!(data.overture.iter().all(|t| t.0 == 14));
        assert!((9..=27).contains(&data.canopy.len()), "three views, nine tiles each: {}", data.canopy.len());
        // (V4, at 13.5, plants no trees)
        assert!(close_up_tiles(&REALISTIC_VIEWS[0]).is_empty());
        assert!(data.overture.len() <= 8, "{:?}", data.overture);
        // one canopy file holds them all (a zoom-10 tile is 39 km across)
        assert_eq!(canopy_files(&data.canopy).len(), 1);
    }

    #[test]
    fn mapzen_stands_in_where_mapterhorn_has_nothing_to_zoom_15() {
        let nothing: BTreeSet<Zxy> = [(14, 1, 2), (15, 3, 4), (16, 5, 6)].into();
        assert_eq!(fallbacks(&nothing), [(14, 1, 2), (15, 3, 4)].into());
        let root = std::env::temp_dir().join(format!("xtask-fixtures-nothing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write(&root.join("mapterhorn/14/1/2.none"), b"").unwrap();
        write(&root.join("mapterhorn/14/1/3.webp"), b"RIFF").unwrap();
        assert_eq!(nothing_held(&root, MAPTERHORN), [(14, 1, 2)].into());
        assert!(nothing_held(&root, TERRARIUM).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_plan_stays_small_enough() {
        let plan = plan();
        let count = |source: &str| plan.get(source).map_or(0, BTreeSet::len);
        // the views of stages 8 to 10 cost tiles of a hundred kilobytes and more: keep an eye on them
        assert!(count(MAPTERHORN) < 160, "{}", count(MAPTERHORN));
        assert!(count(OPENRAILWAYMAP) < 80, "{}", count(OPENRAILWAYMAP));
        assert!(count(NMD) < 150, "{}", count(NMD));
    }

    #[test]
    fn a_set_s_tiles_are_read_back_and_what_it_lacks_is_listed() {
        let root = std::env::temp_dir().join(format!("xtask-fixtures-held-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, bytes) in [
            ("openfreemap/tilejson.json", &b"{}"[..]),
            ("openfreemap/0/0/0.pbf", b"\x1a\x00"),
            ("openfreemap/5/16/10.pbf", b"\x1a\x00"),
            ("esri/6/33/20.none", b""),
            ("NOTICE", b"text"),
            ("esri/not-a-zoom/1/2.jpg", b""),
        ] {
            write(&root.join(path), bytes).unwrap();
        }
        let held = held_tiles(&root).unwrap();
        assert_eq!(held.len(), 2);
        assert_eq!(held[OPENFREEMAP], [(0, 0, 0), (5, 16, 10)].into());
        assert_eq!(held[ESRI], [(6, 33, 20)].into(), "a tile the server had nothing for is held");
        let plan: BTreeMap<&str, BTreeSet<Zxy>> = [
            (OPENFREEMAP, [(0, 0, 0), (5, 16, 10), (5, 17, 10)].into()),
            (ESRI, [(6, 33, 20)].into()),
            (TERRARIUM, [(12, 2253, 1204)].into()),
        ]
        .into();
        let lacked = lacking(&plan, &held);
        assert_eq!(lacked, [(OPENFREEMAP, [(5, 17, 10)].into()), (TERRARIUM, [(12, 2253, 1204)].into())].into());
        assert!(held_tiles(&root.join("nowhere")).unwrap().is_empty());
        // the staged tiles move into the set beside those held
        let staging = root.with_file_name(format!("xtask-fixtures-held-{}.download", std::process::id()));
        let _ = fs::remove_dir_all(&staging);
        write(&staging.join("openfreemap/5/17/10.pbf"), b"\x1a\x01").unwrap();
        write(&staging.join("terrarium/12/2253/1204.none"), b"").unwrap();
        // what the set would weigh: the old file of a path written again does not count twice,
        // a folder replaced counts its new files only
        write(&staging.join("openfreemap/0/0/0.pbf"), b"\x1a\x00\x00").unwrap();
        let before = folder_bytes(&root).unwrap();
        assert_eq!(bytes_after(&root, &staging, &[]).unwrap(), before + 2 + 1);
        assert_eq!(bytes_after(&root, &staging, &[root.join("esri"), root.join("openfreemap")]).unwrap(), 4 + 2 + 3);
        fs::remove_file(staging.join("openfreemap/0/0/0.pbf")).unwrap();
        move_into(&staging, &staging, &root).unwrap();
        assert!(lacking(&plan, &held_tiles(&root).unwrap()).is_empty());
        assert_eq!(fs::read(root.join("openfreemap/5/17/10.pbf")).unwrap(), b"\x1a\x01");
        assert_eq!(fs::read(root.join("openfreemap/0/0/0.pbf")).unwrap(), b"\x1a\x00");
        fs::remove_dir_all(&staging).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_top_up_reads_the_dates_and_esri_s_credit_back_from_the_notice() {
        let mut found = Found {
            openfreemap_template: "https://tiles.openfreemap.org/planet/20261101_080013_pt/{z}/{x}/{y}.pbf".into(),
            openfreemap_attribution: "OpenFreeMap © OpenMapTiles Data from OpenStreetMap".into(),
            esri_copyright: "Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community".into(),
            rainviewer_time: 1_794_328_200,
            rainviewer_template: "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png".into(),
            worldcover_id: Some("pc1.1.abc".into()),
            downloaded: "2026-11-10 16:31".into(),
            topped_up: None,
        };
        let date = |text: &str, prefix: &str| line_after(text, prefix).and_then(|r| r.split_once(" UTC")).map(|(d, _)| d.to_owned());
        let first = notice(&found);
        assert_eq!(date(&first, DOWNLOADED_ON).as_deref(), Some("2026-11-10 16:31"));
        assert_eq!(date(&first, TOPPED_UP_ON), None);
        assert_eq!(line_after(&first, ESRI_CREDIT), Some(found.esri_copyright.as_str()));
        assert!(first.contains("mosaic/pc1.1.abc/tiles/WebMercatorQuad/{z}/{x}/{y}@1x"));
        found.topped_up = Some("2026-11-10 18:49".into());
        let again = notice(&found);
        assert_eq!(date(&again, DOWNLOADED_ON).as_deref(), Some("2026-11-10 16:31"));
        assert_eq!(date(&again, TOPPED_UP_ON).as_deref(), Some("2026-11-10 18:49"));
        // every covered view is named, with its place
        for (views, ..) in COVERS {
            for view in views {
                assert!(again.contains(&format!("  {} ({}, {}, ", view.name, view.latitude, view.longitude)), "{}", view.name);
            }
        }
        // E16's NOTICE, as committed before T3, reads back the same way
        let e16 = "Downloaded by `cargo xtask fixtures` (crates/xtask/src/fixtures.rs) on 2026-11-10 16:31 UTC.\n\
                   esri/\n  Esri World Imagery, x\n  Credit as the service asks: Source: Esri, Vantor\n";
        assert_eq!(date(e16, DOWNLOADED_ON).as_deref(), Some("2026-11-10 16:31"));
        assert_eq!(line_after(e16, ESRI_CREDIT), Some("Source: Esri, Vantor"));
    }

    #[test]
    fn streets_gl_holds_z16_tiles_close_to_the_flat_views_only() {
        let plan = plan();
        let sgl = &plan[STREETS_GL];
        assert!(sgl.iter().all(|t| t.0 == SGL_ZOOM));
        // V5's centre (59.3293, 18.0686) at z16
        assert!(sgl.contains(&(16, 36057, 19273)));
        // in Stockholm only V5 and its realistic neighbours are close enough for the LODs'
        // deepest level and for Streets GL's trees; both take z16 within a few kilometres; and
        // in Zermatt the terrain view
        let near = |(_, x, y): Zxy, (cx, cy): (u32, u32)| x.abs_diff(cx) < 40 && y.abs_diff(cy) < 40;
        let zermatt = tile_at(46.0207, 7.7491, 16);
        assert!(sgl.iter().all(|&t| near(t, (36057, 19273)) || near(t, (zermatt.x, zermatt.y))), "{sgl:?}");
        for view in &VIEWS {
            let state = view.state();
            for size in SIZES {
                let viewport = viewport_for(&state, size);
                let deepest = [city_lod(), ground_lod()]
                    .iter()
                    .flat_map(|lod| distance_tile_indices(&viewport, lod, None, 0.0))
                    .any(|t| t.z == SGL_ZOOM);
                assert_eq!(deepest, view.name == "V5", "{} {size:?}", view.name);
            }
        }
    }

    #[test]
    fn trees_take_tiles_around_the_centre_from_zoom_14_on_the_flat_map() {
        let at = |zoom: f64| View::at("", 59.3293, 18.0686, zoom, 0.0, 0.0);
        // on the globe and below TREES_ZOOM, none
        assert!(tree_tiles(&at(12.0), 900.0).is_empty());
        assert!(tree_tiles(&at(13.9), 900.0).is_empty());
        // from 14, OpenFreeMap's z14 tiles: at 14.2 and 900 px the camera is 1.5·900·mpp off,
        // 1.6 times that is under 4800 m
        let mpp = meters_per_pixel(14.2, 59.3293);
        let radius = 1.6 * 1.5 * 900.0 * mpp;
        assert!(radius > 900.0 && radius < 4800.0, "{radius}");
        let tiles = tree_tiles(&at(14.2), 900.0);
        assert!(!tiles.is_empty() && tiles.iter().all(|t| t.z == TREE_TILE_ZOOM), "{tiles:?}");
        assert_eq!(tiles, tiles_around(18.0686, 59.3293, radius, 14).into_iter().map(|t| t.tile).collect::<Vec<_>>());
        // a short canvas still reaches 900 m; a tall one stops at 4800 m
        assert_eq!(tree_tiles(&at(14.2), 10.0), tiles_around(18.0686, 59.3293, 900.0, 14).into_iter().map(|t| t.tile).collect::<Vec<_>>());
        assert_eq!(tree_tiles(&at(14.2), 1e5), tiles_around(18.0686, 59.3293, 4800.0, 14).into_iter().map(|t| t.tile).collect::<Vec<_>>());
        // from SGL_FROM_ZOOM, Streets GL's z16 tiles within at most 2400 m
        let v5 = VIEWS.iter().find(|v| v.name == "V5").unwrap();
        let tiles = tree_tiles(v5, 900.0);
        assert!(tiles.iter().all(|t| t.z == SGL_ZOOM));
        assert!(tiles.contains(&TileIndex::new(36057, 19273, 16)));
        assert_eq!(tree_tiles(v5, 1e5), tiles_around(18.0686, 59.3293, 2400.0, 16).into_iter().map(|t| t.tile).collect::<Vec<_>>());
        // and every one of them is in the plan
        let plan = plan();
        for size in SIZES {
            for tile in tree_tiles(v5, size[1]) {
                assert!(plan[STREETS_GL].contains(&zxy(tile)), "{}", tile.id());
            }
        }
        // V3, V4 and V6 plant none
        for view in VIEWS.iter().filter(|v| v.name != "V5") {
            assert!(tree_tiles(view, 900.0).is_empty(), "{}", view.name);
        }
    }

    #[test]
    fn the_lods_add_openfreemap_tiles_at_their_levels() {
        let plan = plan();
        let ofm = &plan[OPENFREEMAP];
        assert!(ofm.iter().all(|t| t.0 <= OPENFREEMAP_MAX_ZOOM));
        // deck's rule gives V4 and V5 only z14 tiles; looking out under their pitch, the LODs
        // take z13 further off (V5's centre is in z13 column 4507, row 2409, V4's in 4503, 2394;
        // V6, the only view deck gives z13, is Gothenburg, 400 km away)
        let far = ofm.iter().filter(|&&(z, x, y)| z == 13 && x.abs_diff(4507) < 20 && y.abs_diff(2409) < 20).count();
        assert!(far > 0);
    }

    #[test]
    fn plan_is_deterministic() {
        assert_eq!(plan(), plan());
        assert_eq!(data_plan(), data_plan());
    }

    #[test]
    fn urls_fill_the_template_in_its_own_order() {
        assert_eq!(
            tile_url(&esri_template(), (10, 562, 301)),
            "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/10/301/562"
        );
        assert_eq!(
            tile_url(TERRARIUM_URL, (10, 562, 301)),
            "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/10/562/301.png"
        );
        assert_eq!(tile_url(SGL_TILES, (16, 36057, 19273)), "https://tiles.streets.gl/vector/16/36057/19273");
        assert_eq!(tile_url(MAPTERHORN_URL, (12, 2136, 1456)), "https://tiles.mapterhorn.com/12/2136/1456.webp");
        let found = Found {
            openfreemap_template: String::new(),
            openfreemap_attribution: String::new(),
            esri_copyright: String::new(),
            rainviewer_time: 0,
            rainviewer_template: String::new(),
            worldcover_id: Some("pc1.2.f00".into()),
            downloaded: String::new(),
            topped_up: None,
        };
        assert_eq!(
            source_url(&found, WORLDCOVER, (14, 9014, 4818)).unwrap().unwrap(),
            "https://planetarycomputer.microsoft.com/api/data/v1/mosaic/pc1.2.f00/tiles/WebMercatorQuad/14/9014/4818@1x?assets=map&collection=esa-worldcover&format=png"
        );
        assert!(source_url(&found, NMD, (14, 9014, 4818)).unwrap().unwrap().starts_with("https://geodata.naturvardsverket.se/geoserver/wms?"));
        assert_eq!(source_url(&found, OPENRAILWAYMAP, (12, 2253, 1204)), Ok(None));
    }

    #[test]
    fn tile_paths_carry_the_source_s_extension() {
        assert_eq!(tile_path(OPENFREEMAP, (2, 1, 3)), Path::new("openfreemap/2/1/3.pbf"));
        assert_eq!(tile_path(ESRI, (17, 72115, 38546)), Path::new("esri/17/72115/38546.jpg"));
        assert_eq!(tile_path(TERRARIUM, (12, 2253, 1204)), Path::new("terrarium/12/2253/1204.png"));
        assert_eq!(tile_path(RAINVIEWER, (7, 70, 37)), Path::new("rainviewer/7/70/37.png"));
        assert_eq!(tile_path(STREETS_GL, (16, 36057, 19273)), Path::new("streets-gl/16/36057/19273.pbf"));
        assert_eq!(tile_path(MAPTERHORN, (12, 2136, 1456)), Path::new("mapterhorn/12/2136/1456.webp"));
        assert_eq!(tile_path(OPENRAILWAYMAP, (15, 18028, 9636)), Path::new("openrailwaymap/15/18028/9636.pbf"));
        assert_eq!(tile_path(NMD, (14, 9014, 4818)), Path::new("nmd/14/9014/4818.png"));
        assert_eq!(tile_path(WORLDCOVER, (14, 9014, 4818)), Path::new("worldcover/14/9014/4818.png"));
    }

    #[test]
    fn the_tilejson_s_first_template_is_taken() {
        let tilejson = serde_json::json!({
            "tilejson": "3.0.0",
            "tiles": ["https://tiles.openfreemap.org/planet/20261101_080013_pt/{z}/{x}/{y}.pbf", "https://elsewhere/{z}/{x}/{y}"],
        });
        assert_eq!(
            tilejson_template(&tilejson).unwrap(),
            "https://tiles.openfreemap.org/planet/20261101_080013_pt/{z}/{x}/{y}.pbf"
        );
        assert!(tilejson_template(&serde_json::json!({"tiles": []})).is_err());
    }

    #[test]
    fn rainviewer_s_latest_past_frame_is_taken() {
        let maps = serde_json::json!({
            "version": "2.0",
            "generated": 1794324925,
            "host": "https://tilecache.rainviewer.com",
            "radar": {
                "past": [
                    {"time": 1794324000, "path": "/v2/radar/c6c6eea59214"},
                    {"time": 1794324600, "path": "/v2/radar/6a4ff13e7ba8"}
                ],
                "nowcast": []
            }
        });
        let (time, template) = rainviewer_frame(&maps).unwrap();
        assert_eq!(time, 1794324600);
        assert_eq!(template, "https://tilecache.rainviewer.com/v2/radar/6a4ff13e7ba8/256/{z}/{x}/{y}/2/1_1.png");
        let none = serde_json::json!({"host": "https://tilecache.rainviewer.com", "radar": {"past": []}});
        assert!(rainviewer_frame(&none).is_err());
    }

    #[test]
    fn html_attributions_read_as_text() {
        let html = "<a href=\"https://openfreemap.org\" target=\"_blank\">OpenFreeMap</a> <a href=\"https://www.openmaptiles.org/\" target=\"_blank\">&copy; OpenMapTiles</a> Data from <a href=\"https://www.openstreetmap.org/copyright\" target=\"_blank\">OpenStreetMap</a>";
        assert_eq!(plain_text(html), "OpenFreeMap © OpenMapTiles Data from OpenStreetMap");
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01 00:00");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00");
        assert_eq!(utc(1_794_324_600), "2026-11-10 15:30");
        assert_eq!(utc(-86_400), "1969-12-31 00:00");
    }

    /// A little-endian TIFF (or BigTIFF) with `images` of `(subfile type, width, height, tile
    /// size, tile offsets, tile byte counts)`, the IFDs' arrays after them.
    type Image = (u32, u32, u32, u32, Vec<u64>, Vec<u64>);

    fn tiff(big: bool, images: &[Image]) -> Vec<u8> {
        let mut out = if big { b"II\x2b\x00\x08\x00\x00\x00".to_vec() } else { b"II\x2a\x00".to_vec() };
        let (count_size, entry_size, offset_size) = if big { (8, 20, 8) } else { (2, 12, 4) };
        let put = |out: &mut Vec<u8>, v: u64, size: usize| out.extend_from_slice(&v.to_le_bytes()[..size]);
        let first = out.len() + offset_size;
        put(&mut out, first as u64, offset_size);
        let ifd_size = count_size + 7 * entry_size + offset_size;
        // the IFDs one after another, then each image's two arrays
        let arrays_at = first + images.len() * ifd_size;
        let mut arrays = Vec::new();
        for (i, (kind, width, height, tile, offsets, counts)) in images.iter().enumerate() {
            put(&mut out, 7, count_size);
            let long = if big { 16 } else { 4 };
            let entry = |out: &mut Vec<u8>, tag: u64, kind: u64, n: u64, value: u64| {
                put(out, tag, 2);
                put(out, kind, 2);
                put(out, n, if big { 8 } else { 4 });
                put(out, value, offset_size);
            };
            entry(&mut out, 254, 4, 1, u64::from(*kind));
            entry(&mut out, 256, 4, 1, u64::from(*width));
            entry(&mut out, 257, 4, 1, u64::from(*height));
            entry(&mut out, 322, 3, 1, u64::from(*tile));
            entry(&mut out, 323, 3, 1, u64::from(*tile));
            let item = if big { 8 } else { 4 };
            let at = (arrays_at + arrays.len()) as u64;
            entry(&mut out, 324, long, offsets.len() as u64, at);
            entry(&mut out, 325, long, counts.len() as u64, at + (offsets.len() * item) as u64);
            for &v in offsets.iter().chain(counts) {
                put(&mut arrays, v, item);
            }
            let next = if i + 1 < images.len() { (first + (i + 1) * ifd_size) as u64 } else { 0 };
            put(&mut out, next, offset_size);
        }
        out.extend_from_slice(&arrays);
        out
    }

    #[test]
    fn a_cog_s_images_are_read_from_its_head() {
        for big in [false, true] {
            // a 4096 px image in 1024 px tiles (16 of them), its 2048 px copy (4) and a mask
            let full: Vec<u64> = (0..16).map(|k| 100_000 + 1000 * k).collect();
            let half: Vec<u64> = (0..4).map(|k| 50_000 + 1000 * k).collect();
            let bytes = tiff(
                big,
                &[
                    (0, 4096, 4096, 1024, full.clone(), vec![900; 16]),
                    (1, 2048, 2048, 1024, half.clone(), vec![0, 900, 900, 900]),
                    (5, 2048, 2048, 1024, vec![1; 4], vec![1; 4]),
                ],
            );
            let (images, end) = parse_tiff(&bytes).unwrap();
            assert_eq!(end, bytes.len() as u64, "big {big}");
            assert_eq!(images.len(), 3);
            assert_eq!((images[0].width, images[0].height, images[0].tile_width), (4096, 4096, 1024));
            assert_eq!(images[1].offsets, half);
            assert_eq!(images[2].new_subfile_type, Some(5));
            // short of its arrays: how far to read
            assert_eq!(parse_tiff(&bytes[..bytes.len() - 1]), Err(TiffError::Short(bytes.len() as u64)));
            assert!(matches!(parse_tiff(b"PK\x03\x04xxxx"), Err(TiffError::Bad(_))));
        }
    }

    #[test]
    fn canopy_reads_the_internal_tiles_under_a_tile_s_window() {
        // a zoom-10 file of 4096 px in 1024 px tiles, a 2048 px copy, and a mask (never read)
        let full: Vec<u64> = (0..16).map(|k| 100_000 + 1000 * k).collect();
        let half: Vec<u64> = (0..4).map(|k| 50_000 + 1000 * k).collect();
        let bytes = tiff(
            false,
            &[(0, 4096, 4096, 1024, full, vec![900; 16]), (1, 2048, 2048, 1024, half, vec![0, 900, 900, 900]), (4, 8192, 8192, 1024, vec![7; 64], vec![7; 64])],
        );
        let (images, _) = parse_tiff(&bytes).unwrap();
        // the zoom-10 tile itself (span 1): the 2048 px copy has 512 px and more across it, so the
        // smallest is it; its whole window, all four tiles but the empty one
        let file = (10, 563, 297);
        assert_eq!(canopy_ranges(&images, &[file]), [[51_000, 51_900], [52_000, 52_900], [53_000, 53_900]]);
        // a zoom-12 tile (span 1/4): the 2048 px copy still has 512 px across it; its window,
        // from 1024, 512, is in that copy's top right tile
        assert_eq!(canopy_ranges(&images, &[(12, 563 * 4 + 2, 297 * 4 + 1)]), [[51_000, 51_900]]);
        // a zoom-13 tile (span 1/8): only the 4096 px image has 512 across; its window, 512 px
        // from 2560, 1024, is one internal tile (row 1, column 2 of the 4 x 4)
        let (x, y) = (563 * 8 + 5, 297 * 8 + 2);
        assert_eq!(canopy_ranges(&images, &[(13, x, y)]), [[106_000, 106_900]]);
        // its neighbour reads the next one; the same twice is read once
        assert_eq!(canopy_ranges(&images, &[(13, x, y), (13, x + 1, y), (13, x, y)]), [[106_000, 106_900], [107_000, 107_900]]);
        assert_eq!(merge_ranges(vec![[5, 9], [0, 2], [2, 4], [8, 12]]), [[0, 4], [5, 12]]);
    }

    #[test]
    fn pmtiles_ids_follow_the_hilbert_curve() {
        // the reference implementation's values
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!(tile_id(1, 0, 0), 1);
        assert_eq!(tile_id(1, 0, 1), 2);
        assert_eq!(tile_id(1, 1, 1), 3);
        assert_eq!(tile_id(1, 1, 0), 4);
        assert_eq!(tile_id(2, 0, 0), 5);
        assert_eq!(tile_id(3, 7, 0), 84);
        assert_eq!(tile_id(20, 0, 0), 366_503_875_925);
        // every id of a zoom once, after those of the zooms above
        for z in 0..4u32 {
            let ids: BTreeSet<u64> = (0..1 << z).flat_map(|x| (0..1 << z).map(move |y| tile_id(z, x, y))).collect();
            let first = (0..z).map(|k| 1u64 << (2 * k)).sum::<u64>();
            assert_eq!(ids, (first..first + (1 << (2 * z))).collect(), "zoom {z}");
        }
    }

    #[test]
    fn an_extract_reads_back_with_its_tiles() {
        let entries = [
            PmEntry { tile_id: 5, offset: 0, length: 10, run_length: 1 },
            PmEntry { tile_id: 9, offset: 10, length: 4, run_length: 3 },
            PmEntry { tile_id: 20, offset: 100, length: 7, run_length: 0 },
        ];
        assert_eq!(parse_directory(&directory_bytes(&entries)).unwrap(), entries);
        assert_eq!(find_entry(&entries, 5), Some(entries[0]));
        assert_eq!(find_entry(&entries, 6), None);
        assert_eq!(find_entry(&entries, 11), Some(entries[1]));
        assert_eq!(find_entry(&entries, 12), None);
        assert_eq!(find_entry(&entries, 99), Some(entries[2]), "a leaf covers what follows");
        assert_eq!(find_entry(&entries, 4), None);
        let source = PmHeader { tile_compression: 2, tile_type: 1, ..PmHeader::default() };
        let tiles: BTreeMap<Zxy, Vec<u8>> = [((14, 9014, 4818), vec![1, 2, 3]), ((14, 9015, 4818), vec![4, 5])].into();
        let bytes = write_extract(&source, b"{}", &tiles).unwrap();
        let (header, read) = read_extract(&bytes).unwrap();
        assert_eq!(PmHeader::parse(&header.bytes()).unwrap(), header);
        assert_eq!(header.bytes().len(), PM_HEADER);
        assert_eq!((header.min_zoom, header.max_zoom, header.tile_compression, header.tile_type), (14, 14, 2, 1));
        assert_eq!(&bytes[header.metadata_offset as usize..][..2], b"{}");
        assert_eq!(read.len(), 2);
        assert_eq!(read[&tile_id(14, 9014, 4818)], [1, 2, 3]);
        assert_eq!(read[&tile_id(14, 9015, 4818)], [4, 5]);
        // the box of the two tiles: from 9014/4818's north-west corner to 9015/4818's south-east
        assert_eq!((header.min_lon_e7, header.max_lat_e7), corner_e7(14, 9014, 4818));
        assert_eq!((header.max_lon_e7, header.min_lat_e7), corner_e7(14, 9016, 4819));
        assert_eq!(corner_e7(1, 1, 1), (0, 0));
        assert_eq!(corner_e7(0, 0, 0).0, -1_800_000_000);
        let path = std::env::temp_dir().join(format!("xtask-extract-{}.pmtiles", std::process::id()));
        fs::write(&path, &bytes).unwrap();
        assert!(extract_holds(&path, &[(14, 9014, 4818)].into()));
        assert!(!extract_holds(&path, &[(14, 9016, 4818)].into()));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_jpeg_s_size_is_read_from_its_frame() {
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08, 0x07, 0x8e, 0x10, 0x00, 0x03, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(jpeg_size(&jpeg), Some((4096, 1934)));
        assert_eq!(jpeg_size(b"\x89PNG\r\n\x1a\n"), None);
    }

    /// Every file of the committed tiles: tiles as `(source, (z, x, y), extension)`, other files
    /// by their path.
    fn committed() -> (BTreeSet<(String, Zxy, String)>, BTreeSet<String>) {
        let root = fixtures_dir();
        let (mut tiles, mut others) = (BTreeSet::new(), BTreeSet::new());
        for file in files_below(&root).unwrap() {
            let parts: Vec<&str> = file.iter().filter_map(|p| p.to_str()).collect();
            let tile = match parts.as_slice() {
                [source, z, x, name] => name.split_once('.').and_then(|(y, ext)| {
                    Some((source.to_string(), (z.parse().ok()?, x.parse().ok()?, y.parse().ok()?), ext.to_owned()))
                }),
                _ => None,
            };
            match tile {
                Some(tile) => {
                    tiles.insert(tile);
                }
                None => {
                    others.insert(parts.join("/"));
                }
            }
        }
        (tiles, others)
    }

    /// The set holds every planned tile, and Mapzen's stand-in for each Mapterhorn tile it holds
    /// as nothing. Tiles the plan no longer asks for are let be: they cost space, not
    /// correctness, and a re-run is needed only when a planned tile is missing.
    #[test]
    fn the_committed_fixtures_hold_the_plan() {
        let (tiles, others) = committed();
        let held: BTreeSet<(String, Zxy)> = tiles.iter().map(|(s, t, _)| (s.clone(), *t)).collect();
        let mut planned: BTreeSet<(String, Zxy)> =
            plan().into_iter().flat_map(|(s, tiles)| tiles.into_iter().map(move |t| (s.to_owned(), t))).collect();
        planned.extend(fallbacks(&nothing_held(&fixtures_dir(), MAPTERHORN)).into_iter().map(|t| (TERRARIUM.to_owned(), t)));
        let missing: Vec<_> = planned.difference(&held).collect();
        assert!(
            missing.is_empty(),
            "the fixtures lack tiles the selection asks for: run `cargo xtask fixtures`\nmissing: {missing:?}"
        );
        assert!(held.iter().all(|(source, _)| plan().contains_key(source.as_str())), "a source the plan has not");
        assert_eq!(held.len(), tiles.len(), "a tile held twice");
        for (source, tile, ext) in &tiles {
            assert!(ext == extension(source) || ext == NOTHING_EXT, "{source} {tile:?}.{ext}");
        }
        let expected: BTreeSet<String> = ["NOTICE", "openfreemap/tilejson.json", "rainviewer/weather-maps.json", "worldcover/register.json"]
            .map(str::to_owned)
            .into();
        assert_eq!(others, expected);
    }

    /// The data beside the tiles: the cloud picture, every planned canopy range, the Overture
    /// extract with every planned tile (buildings in each), and both radar volumes whole.
    #[test]
    fn the_committed_data_holds_the_plan() {
        let out = data_dir();
        let picture = fs::read(out.join(CLOUDS_FILE)).unwrap();
        assert_eq!(jpeg_size(&picture), Some((4096, 1934)));
        let record = read_json(&out.join(CLOUDS_RECORD)).unwrap();
        assert_eq!(record["url"], CLOUD_URL);
        let data = data_plan();
        for (key, tiles) in canopy_files(&data.canopy) {
            assert_eq!(canopy_missing(&out, &key, &tiles), Ok(Vec::new()), "canopy {key}: run `cargo xtask fixtures`");
            let manifest = read_json(&out.join(CANOPY).join(format!("{key}.json"))).unwrap();
            assert_eq!(manifest["url"], rustradar_scene::canopy::file_url(&key));
        }
        let bytes = fs::read(out.join(OVERTURE_FILE)).unwrap();
        let (header, held) = read_extract(&bytes).unwrap();
        assert_eq!((header.min_zoom, header.max_zoom, header.tile_type), (14, 14, 1));
        for &(z, x, y) in &data.overture {
            let tile = held.get(&tile_id(z, x, y)).unwrap_or_else(|| panic!("overture {z}/{x}/{y}: run `cargo xtask fixtures`"));
            let pbf = if header.tile_compression == GZIP { gunzip(tile).unwrap() } else { tile.clone() };
            let layers: Vec<String> = rustradar_core::mvt::layers(&pbf).map(|l| l.name.to_owned()).collect();
            assert!(layers.iter().any(|l| l == "building"), "overture {z}/{x}/{y}: {layers:?}");
        }
        let catalog = read_json(&out.join(OVERTURE_CATALOG)).unwrap();
        assert!(!overture::release_of(Some(&catalog)).is_empty());
        for source in RADAR_SOURCES {
            assert!(radar_held(&out, source), "radar {source}: run `cargo xtask fixtures`");
            let files = radar_files(&out, source).unwrap();
            // each blob as `radar_image` serves it: a polygon count and the shapes, or the mask
            for file in files.iter().filter(|f| f.ends_with(".bin") && *f != "coverage.bin") {
                let blob = fs::read(out.join(RADAR).join(source).join(file)).unwrap();
                assert!(blob.len() >= 4, "{source} {file}");
            }
            let info: rustradar_core::model::RadarVolume =
                serde_json::from_value(read_json(&out.join(RADAR).join(source).join("volume.json")).unwrap()).unwrap();
            assert_eq!(info.source, source);
            let coverage = fs::read(out.join(RADAR).join(source).join("coverage.bin")).unwrap();
            assert_eq!(coverage.len() as u64, u64::from(info.coverage_width) * u64::from(info.coverage_height), "{source}");
        }
        assert!(dwd_held(&out), "radar/dwd: run `cargo xtask fixtures`");
    }

    #[test]
    fn a_tar_s_files_are_read() {
        let mut tar = Vec::new();
        for (name, body, kind) in [("a_000-hd5", &b"\x89HDF\r\n\x1a\nxyz"[..], b'0'), ("dir/", &b""[..], b'5'), ("b", &b"12"[..], 0)] {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            header[124..135].copy_from_slice(format!("{:011o}", body.len()).as_bytes());
            header[156] = kind;
            tar.extend_from_slice(&header);
            tar.extend_from_slice(body);
            tar.resize(tar.len().div_ceil(512) * 512, 0);
        }
        tar.extend_from_slice(&[0; 1024]);
        let files = untar(&tar);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], ("a_000-hd5".to_owned(), &b"\x89HDF\r\n\x1a\nxyz"[..]));
        assert_eq!(files[1], ("b".to_owned(), &b"12"[..]));
        // a truncated archive ends the walk
        assert_eq!(untar(&tar[..515]).len(), 0);
    }

    #[test]
    fn the_committed_fixtures_are_within_budget() {
        let bytes = folder_bytes(&data_dir()).unwrap();
        assert!(bytes < BUDGET_BYTES, "{bytes}");
    }

    #[test]
    fn the_notice_names_every_source_and_its_terms() {
        let notice = fs::read_to_string(fixtures_dir().join("NOTICE")).unwrap();
        for source in plan().keys() {
            assert!(notice.contains(&format!("\n{source}/\n")), "{source}");
        }
        // the views it covers, as the plan has them now
        assert!(notice.contains(&coverage()), "the NOTICE's views are not the plan's: run `cargo xtask fixtures`");
        for needle in [
            "Open Database License",
            "OpenMapTiles",
            "Esri",
            "https://goto.arcgisonline.com/maps/World_Imagery",
            "joerd",
            "Mapterhorn",
            "Weather data by RainViewer",
            "Streets GL",
            "OpenRailwayMap",
            "CC0",
            "ESA WorldCover",
        ] {
            assert!(notice.contains(needle), "{needle}");
        }
        // the OpenFreeMap template the set came from, as its TileJSON gave it
        let tilejson: serde_json::Value =
            serde_json::from_slice(&fs::read(fixtures_dir().join("openfreemap/tilejson.json")).unwrap()).unwrap();
        assert!(notice.contains(&tilejson_template(&tilejson).unwrap()));
        let maps: serde_json::Value =
            serde_json::from_slice(&fs::read(fixtures_dir().join("rainviewer/weather-maps.json")).unwrap()).unwrap();
        assert!(notice.contains(&rainviewer_frame(&maps).unwrap().1));
        let register = read_json(&fixtures_dir().join("worldcover/register.json")).unwrap();
        assert!(notice.contains(mosaic_id(&register).unwrap()));
        // and the data's
        let data = fs::read_to_string(data_dir().join("NOTICE")).unwrap();
        for needle in ["\nclouds/\n", "\ncanopy/\n", "\noverture/\n", "\nradar/\n", "EUMETSAT", "CC BY 4.0", "Overture", "NOAA", "Deutscher Wetterdienst", "_000-hd5"] {
            assert!(data.contains(needle), "{needle}");
        }
    }
}
