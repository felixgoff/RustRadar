//! `cargo xtask fixtures`: download the real tiles the map's headless shots and tests draw from
//! (plan E16 and section 8) into `crates/map/tests/data/tiles`, with a `NOTICE` naming their
//! sources and licences. A shot never touches the network; it reads these files through the
//! file-backed `TileFetcher` of `crates/map/tests/common/fixture_tiles.rs`, and a tile the set
//! does not hold is simply not drawn.
//!
//! What is fetched ([`plan`]): for the named views V3 to V6 of plan section 8 ([`VIEWS`]), at the
//! shot size and the two reference sizes ([`SIZES`]), every tile the map's own selection asks of
//! each source; and OpenFreeMap's whole world at zooms 0 to 2 (V1's globe):
//! - `openfreemap/`: OpenFreeMap's vector tiles, deck's rule for `map-*` and `label-tiles`
//!   ([`VECTOR_TILES`]), plus on the flat map the distance LOD of the buildings
//!   ([`city_lod`]) and of the realistic ground ([`ground_lod`]) at their zooms up to 14, and the
//!   trees' z14 tiles from [`TREES_ZOOM`] to [`SGL_FROM_ZOOM`] (none of V3 to V6 needs them). The URL
//!   template is `tiles[0]` of the TileJSON at [`OPENFREEMAP_TILEJSON`], read at run time (it
//!   names the planet build), never hard-coded (doc 01 §5.6); the TileJSON is kept as
//!   `openfreemap/tilejson.json`.
//! - `esri/`: Esri World Imagery for the satellite basemap ([`IMAGERY_TILES`]).
//! - `terrarium/`: Mapzen's terrarium heights the hillshade shades ([`HILLSHADE_TILES`]).
//! - `rainviewer/`: one RainViewer radar frame, the latest past one of its frame list (the frame
//!   list is kept as `rainviewer/weather-maps.json`), at the radar's tiles ([`RADAR_TILES`]).
//! - `streets-gl/`: the Streets GL z16 tiles of the realistic map on the flat map: those the two
//!   distance LODs take close in (their deepest level is Streets GL's at any zoom; of V3 to V6
//!   only V5 is close enough to reach it), and from [`SGL_FROM_ZOOM`] the trees' tiles around
//!   the view's centre ([`tree_tiles`]; V5 again).
//!
//! Each tile is written as `<source>/<z>/<x>/<y>.<ext>` with exactly the bytes the server sent,
//! the HTTP compression undone (what `backend::tiles` hands the map). A tile the server has
//! nothing for (404 or 204, which the map settles as "nothing there") is written as an empty
//! `<y>.none`. The whole set must stay under [`BUDGET_BYTES`]; a download past it is refused.
//!
//! Downloads go through the `curl` command (Windows 10 and later, macOS and Linux ship it): it
//! honours `HTTPS_PROXY` and the system's certificates, and keeps an HTTP client out of this
//! crate. Everything is fetched into a staging folder first; the old set is replaced only when
//! every tile has arrived. The tiles change with each planet build, imagery update and radar
//! frame, so re-running gives a new set; the test `the_committed_fixtures_hold_the_plan` says when
//! the selection code asks for a tile the committed set lacks.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rustradar_core::geo::meters_per_pixel;
use rustradar_scene::camera::{CAMERA_HEIGHTS, ViewState, viewport_for};
use rustradar_scene::hillshade::{HILLSHADE_MAX_ZOOM, TERRAIN_ATTRIBUTION_FULL, TERRARIUM_URL};
use rustradar_scene::lod::{Lod, distance_tile_indices};
use rustradar_scene::streets_gl::{SGL_FROM_ZOOM, SGL_TILES, SGL_ZOOM};
use rustradar_scene::tiles::{IMAGERY_TILES, RADAR_TILES, TileIndex, TilesetOptions, VECTOR_TILES, tile_indices};
use rustradar_scene::trees::{TREE_TILE_ZOOM, tiles_around};

/// OpenFreeMap's TileJSON (G:308); its `tiles[0]` is the URL template of the current planet build.
pub const OPENFREEMAP_TILEJSON: &str = "https://tiles.openfreemap.org/planet";
/// Esri World Imagery's map service (G:309, 483); its tiles are `{z}/{y}/{x}` below `/tile`.
pub const ESRI_IMAGERY: &str = "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer";
/// RainViewer's frame list (G:1117).
pub const RAINVIEWER_MAPS: &str = "https://api.rainviewer.com/public/weather-maps.json";

/// The fixtures' size cap, bytes (plan E16: under 40 MB).
pub const BUDGET_BYTES: u64 = 40_000_000;
/// How many downloads run at once.
const PARALLEL: usize = 6;

/// A source's folder name: what the map's layers call it in a `FetchRequest`.
pub const OPENFREEMAP: &str = "openfreemap";
pub const ESRI: &str = "esri";
pub const TERRARIUM: &str = "terrarium";
pub const RAINVIEWER: &str = "rainviewer";
pub const STREETS_GL: &str = "streets-gl";

/// The marker of a tile the server had nothing for.
pub const NOTHING_EXT: &str = "none";

/// One of plan section 8's named views.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub name: &'static str,
    pub latitude: f64,
    pub longitude: f64,
    pub zoom: f64,
    pub pitch: f64,
    pub bearing: f64,
}

impl View {
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
}

/// The views the fixtures cover (plan section 8): Stockholm region, Arlanda pitched, Stockholm
/// city pitched, Gothenburg harbour.
pub const VIEWS: [View; 4] = [
    View { name: "V3", latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0 },
    View { name: "V4", latitude: 59.6519, longitude: 17.9186, zoom: 13.5, pitch: 45.0, bearing: 30.0 },
    View { name: "V5", latitude: 59.3293, longitude: 18.0686, zoom: 15.5, pitch: 60.0, bearing: 20.0 },
    View { name: "V6", latitude: 57.70, longitude: 11.93, zoom: 13.0, pitch: 0.0, bearing: 0.0 },
];

/// The canvas sizes in CSS pixels the views are covered at: the shot command's default and the
/// two sizes of `docs/port/reference`.
pub const SIZES: [[f64; 2]; 3] = [[1100.0, 720.0], [1440.0, 900.0], [390.0, 844.0]];

/// The zooms at which the fixtures hold OpenFreeMap's whole world.
pub const WORLD_ZOOMS: std::ops::RangeInclusive<u32> = 0..=2;

/// The hillshade's tiles (`hillshadeLayer`, terrain-layer.ts:322-329): 256 px, zooms 0 to 12.
pub const HILLSHADE_TILES: TilesetOptions = TilesetOptions::new(256.0, 0.0, HILLSHADE_MAX_ZOOM as f64);

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

/// A tile as `(z, x, y)`: sorted by zoom, then column, then row.
pub type Zxy = (u32, u32, u32);

fn zxy(tile: TileIndex) -> Zxy {
    (tile.z, tile.x, tile.y)
}

/// Every tile the fixtures hold, by source.
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
    for view in &VIEWS {
        let state = view.state();
        for size in SIZES {
            let viewport = viewport_for(&state, size);
            let deck = |opts: &TilesetOptions| tile_indices(&viewport, opts).into_iter().map(zxy);
            plan.entry(OPENFREEMAP).or_default().extend(deck(&VECTOR_TILES));
            plan.entry(ESRI).or_default().extend(deck(&IMAGERY_TILES));
            plan.entry(TERRARIUM).or_default().extend(deck(&HILLSHADE_TILES));
            plan.entry(RAINVIEWER).or_default().extend(deck(&RADAR_TILES));
            if !state.is_flat() {
                continue;
            }
            // the buildings' and the realistic ground's tiles by distance (512 px tiles, no
            // zoom offset: `log2(512 / tileSize) + zoomOffset` is 0); their zooms up to 14 are
            // OpenFreeMap's and 16 is Streets GL's (the realistic map's source for it, G:1051,
            // realistic-ground.ts:826)
            for lod in [city_lod(), ground_lod()] {
                for tile in distance_tile_indices(&viewport, &lod, None, 0.0) {
                    if tile.z <= OPENFREEMAP_MAX_ZOOM {
                        plan.entry(OPENFREEMAP).or_default().insert(zxy(tile));
                    } else if tile.z == SGL_ZOOM {
                        plan.entry(STREETS_GL).or_default().insert(zxy(tile));
                    }
                }
            }
            for tile in tree_tiles(view, size[1]) {
                let source = if tile.z == SGL_ZOOM { STREETS_GL } else { OPENFREEMAP };
                plan.entry(source).or_default().insert(zxy(tile));
            }
        }
    }
    plan
}

/// A tile's file name extension by source.
pub fn extension(source: &str) -> &'static str {
    match source {
        OPENFREEMAP | STREETS_GL => "pbf",
        ESRI => "jpg",
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

/// Where the fixtures are committed.
pub fn fixtures_dir() -> PathBuf {
    repo_root().join("crates/map/tests/data/tiles")
}

const USAGE: &str = "usage: cargo xtask fixtures [--list] [--out <dir>]

  --list         print the tiles of each source and download nothing
  --out <dir>    write the set there instead of crates/map/tests/data/tiles";

pub fn run(args: &[String]) -> Result<(), String> {
    let mut list = false;
    let mut out = fixtures_dir();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => list = true,
            "--out" => out = PathBuf::from(args.next().ok_or(USAGE)?),
            _ => return Err(format!("unknown argument `{arg}`\n\n{USAGE}")),
        }
    }
    let plan = plan();
    if list {
        for (source, tiles) in &plan {
            println!("{source}: {} tiles", tiles.len());
            for &(z, x, y) in tiles {
                println!("  {z}/{x}/{y}");
            }
        }
        return Ok(());
    }
    download(&plan, &out)
}

/// What a run read from the services besides the tiles, for the NOTICE.
struct Found {
    openfreemap_template: String,
    openfreemap_attribution: String,
    esri_copyright: String,
    rainviewer_time: i64,
    rainviewer_template: String,
    downloaded: i64,
}

fn download(plan: &BTreeMap<&'static str, BTreeSet<Zxy>>, out: &Path) -> Result<(), String> {
    let staging = out.with_file_name(format!(
        "{}.download",
        out.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("bad output folder {}", out.display()))?
    ));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| format!("cannot clear {}: {e}", staging.display()))?;
    }
    for source in plan.keys() {
        let dir = staging.join(source);
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }

    // the templates of the day
    let tilejson_bytes = get(OPENFREEMAP_TILEJSON)?;
    let tilejson: serde_json::Value =
        serde_json::from_slice(&tilejson_bytes).map_err(|e| format!("{OPENFREEMAP_TILEJSON}: {e}"))?;
    let openfreemap_template = tilejson_template(&tilejson)?;
    write(&staging.join(OPENFREEMAP).join("tilejson.json"), &tilejson_bytes)?;
    let maps_bytes = get(RAINVIEWER_MAPS)?;
    let maps: serde_json::Value = serde_json::from_slice(&maps_bytes).map_err(|e| format!("{RAINVIEWER_MAPS}: {e}"))?;
    let (rainviewer_time, rainviewer_template) = rainviewer_frame(&maps)?;
    write(&staging.join(RAINVIEWER).join("weather-maps.json"), &maps_bytes)?;
    let service_url = format!("{ESRI_IMAGERY}?f=json");
    let service: serde_json::Value =
        serde_json::from_slice(&get(&service_url)?).map_err(|e| format!("{service_url}: {e}"))?;
    let found = Found {
        openfreemap_attribution: plain_text(tilejson["attribution"].as_str().unwrap_or_default()),
        openfreemap_template,
        esri_copyright: service["copyrightText"].as_str().unwrap_or_default().to_owned(),
        rainviewer_time,
        rainviewer_template,
        downloaded: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64),
    };
    let template = |source: &str| match source {
        OPENFREEMAP => found.openfreemap_template.clone(),
        ESRI => esri_template(),
        TERRARIUM => TERRARIUM_URL.to_owned(),
        RAINVIEWER => found.rainviewer_template.clone(),
        _ => SGL_TILES.to_owned(),
    };

    // the tiles
    let jobs: Vec<(String, PathBuf)> = plan
        .iter()
        .flat_map(|(source, tiles)| {
            let template = template(source);
            let staging = &staging;
            tiles.iter().map(move |&tile| (tile_url(&template, tile), staging.join(tile_path(source, tile))))
        })
        .collect();
    println!("downloading {} tiles", jobs.len());
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let nothing = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL {
            scope.spawn(|| {
                while let Some((url, path)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    match fetch_tile(url, path) {
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
            "{} of {} tiles failed (the old set is kept; {} holds what arrived):\n{}",
            failures.len(),
            jobs.len(),
            staging.display(),
            failures.join("\n")
        ));
    }

    fs::write(staging.join("NOTICE"), notice(&found)).map_err(|e| format!("cannot write the NOTICE: {e}"))?;
    let bytes = folder_bytes(&staging)?;
    if bytes >= BUDGET_BYTES {
        return Err(format!(
            "the set is {bytes} bytes, past the budget of {BUDGET_BYTES} (left in {})",
            staging.display()
        ));
    }

    // replace the old set
    fs::create_dir_all(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    for name in plan.keys().copied().chain(["NOTICE"]) {
        let old = out.join(name);
        if old.is_dir() {
            fs::remove_dir_all(&old).map_err(|e| format!("cannot remove {}: {e}", old.display()))?;
        } else if old.exists() {
            fs::remove_file(&old).map_err(|e| format!("cannot remove {}: {e}", old.display()))?;
        }
        fs::rename(staging.join(name), &old).map_err(|e| format!("cannot move {name} into {}: {e}", out.display()))?;
    }
    fs::remove_dir_all(&staging).map_err(|e| format!("cannot remove {}: {e}", staging.display()))?;
    for (source, tiles) in plan {
        println!("{source}: {} tiles", tiles.len());
    }
    println!(
        "wrote {} ({bytes} bytes; {} tiles the servers had nothing for)",
        out.display(),
        nothing.load(Ordering::Relaxed)
    );
    Ok(())
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

/// `curl` with the options every request uses: errors shown, HTTP compression asked for and
/// undone, redirects followed, transient failures (timeouts, 408, 429, 5xx) retried.
fn curl() -> Command {
    let mut curl = Command::new("curl");
    curl.args(["--silent", "--show-error", "--compressed", "--location", "--retry", "4", "--retry-delay", "2"]);
    curl.args(["--connect-timeout", "20", "--max-time", "120"]);
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

/// The NOTICE: where each folder's tiles come from, under which terms, and the credit each asks
/// for.
fn notice(found: &Found) -> String {
    let terrain = TERRAIN_ATTRIBUTION_FULL.iter().map(|line| format!("    - {line}\n")).collect::<String>();
    format!(
        "Tile fixtures of crates/map (docs/port/00-architecture-and-plan.md, item E16 and section 8)

Downloaded by `cargo xtask fixtures` (crates/xtask/src/fixtures.rs) on {downloaded} UTC.
Each folder holds one source's tiles as <z>/<x>/<y>.<ext>, the bytes the server sent with the
HTTP compression undone; an empty <y>.none marks a tile the server had nothing for (404/204).
They cover the views V3 to V6 of the plan's section 8 at 1100x720, 1440x900 and 390x844 CSS
pixels, as the map's tile selection asks for them, and OpenFreeMap's world at zooms 0 to 2.

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
",
        downloaded = utc(found.downloaded),
        ofm_template = found.openfreemap_template,
        ofm_tilejson = OPENFREEMAP_TILEJSON,
        ofm_attribution = found.openfreemap_attribution,
        esri_service = ESRI_IMAGERY,
        esri_template = esri_template(),
        esri_copyright = found.esri_copyright,
        terrarium = TERRARIUM_URL,
        frame = utc(found.rainviewer_time),
        frame_unix = found.rainviewer_time,
        rainviewer_maps = RAINVIEWER_MAPS,
        rainviewer_template = found.rainviewer_template,
        sgl = SGL_TILES,
    )
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
                let n = f64::from(1u32 << z);
                let x = ((view.longitude + 180.0) / 360.0 * n).floor() as u32;
                let lat = view.latitude.to_radians();
                let y = ((1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
                assert!(plan[source].contains(&(z, x, y)), "{} {source} {z}/{x}/{y}", view.name);
            }
        }
    }

    #[test]
    fn streets_gl_holds_z16_tiles_around_v5_s_centre_only() {
        let plan = plan();
        let sgl = &plan[STREETS_GL];
        assert!(sgl.iter().all(|t| t.0 == SGL_ZOOM));
        // V5's centre (59.3293, 18.0686) at z16
        assert!(sgl.contains(&(16, 36057, 19273)));
        // only V5 is close enough for the LODs' deepest level and for Streets GL's trees; both
        // take z16 within a few kilometres
        assert!(sgl.iter().all(|&(_, x, y)| x.abs_diff(36057) < 40 && y.abs_diff(19273) < 40), "{sgl:?}");
        assert!(sgl.len() < 250, "{}", sgl.len());
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
        let at = |zoom: f64| View { name: "", latitude: 59.3293, longitude: 18.0686, zoom, pitch: 0.0, bearing: 0.0 };
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
    }

    #[test]
    fn tile_paths_carry_the_source_s_extension() {
        assert_eq!(tile_path(OPENFREEMAP, (2, 1, 3)), Path::new("openfreemap/2/1/3.pbf"));
        assert_eq!(tile_path(ESRI, (17, 72115, 38546)), Path::new("esri/17/72115/38546.jpg"));
        assert_eq!(tile_path(TERRARIUM, (12, 2253, 1204)), Path::new("terrarium/12/2253/1204.png"));
        assert_eq!(tile_path(RAINVIEWER, (7, 70, 37)), Path::new("rainviewer/7/70/37.png"));
        assert_eq!(tile_path(STREETS_GL, (16, 36057, 19273)), Path::new("streets-gl/16/36057/19273.pbf"));
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

    /// Every file of the committed set: tiles as `(source, (z, x, y), extension)`, other files by
    /// their path.
    fn committed() -> (BTreeSet<(String, Zxy, String)>, BTreeSet<String>) {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    out.push(path.strip_prefix(root).unwrap().to_path_buf());
                }
            }
        }
        let root = fixtures_dir();
        let mut files = Vec::new();
        walk(&root, &root, &mut files);
        let (mut tiles, mut others) = (BTreeSet::new(), BTreeSet::new());
        for file in files {
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

    /// The set holds every planned tile. Tiles the plan no longer asks for are let be: they cost
    /// space, not correctness, and a re-run (which fetches the day's planet build and radar frame
    /// for everything) is needed only when a planned tile is missing.
    #[test]
    fn the_committed_fixtures_hold_the_plan() {
        let (tiles, others) = committed();
        let held: BTreeSet<(String, Zxy)> = tiles.iter().map(|(s, t, _)| (s.clone(), *t)).collect();
        let planned: BTreeSet<(String, Zxy)> =
            plan().into_iter().flat_map(|(s, tiles)| tiles.into_iter().map(move |t| (s.to_owned(), t))).collect();
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
        let expected: BTreeSet<String> =
            ["NOTICE", "openfreemap/tilejson.json", "rainviewer/weather-maps.json"].map(str::to_owned).into();
        assert_eq!(others, expected);
    }

    #[test]
    fn the_committed_fixtures_are_within_budget() {
        let bytes = folder_bytes(&fixtures_dir()).unwrap();
        assert!(bytes < BUDGET_BYTES, "{bytes}");
    }

    #[test]
    fn the_notice_names_every_source_and_its_terms() {
        let notice = fs::read_to_string(fixtures_dir().join("NOTICE")).unwrap();
        for source in plan().keys() {
            assert!(notice.contains(&format!("\n{source}/\n")), "{source}");
        }
        for needle in [
            "Open Database License",
            "OpenMapTiles",
            "Esri",
            "https://goto.arcgisonline.com/maps/World_Imagery",
            "joerd",
            "Weather data by RainViewer",
            "Streets GL",
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
    }
}
