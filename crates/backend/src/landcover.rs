//! What covers the land, 10 m by 10 m, for the realistic map where the map's own data says
//! nothing (item R8; doc 04 §11; `landcover.ts`): woods with their kind of trees, wetland, heath,
//! fields, rock.
//!
//! - In Sweden, Naturvårdsverket's Nationella Marktäckedata (NMD2023, CC0), from its WMS
//!   ([`nmd_url`]): each class drawn in its legend's exact colour, read back here to the class
//!   ([`nmd_classes`]); clear where it has nothing (past the borders and out at sea).
//! - Elsewhere, and where NMD is clear, ESA WorldCover 2021 (CC BY 4.0) through Microsoft's
//!   Planetary Computer: a mosaic registered once ([`LandCover::mosaic`], the POST of
//!   [`WORLD_COVER_SEARCH`]), whose tiles from zoom [`WORLD_COVER_MIN_ZOOM`] carry the class codes
//!   in their grey ([`world_cover_url`], [`world_cover_classes`]).
//!
//! What today's `landCover(tile)` does, and so [`LandCover::tile`]: NMD's picture of the tile
//! when its middle is in Sweden's box ([`in_sweden`]); WorldCover's when that is not to be had or
//! is clear somewhere ([`wants_world_cover`]), filling NMD's clear pixels ([`combine`]); each
//! picture read as stored (no colour management, no premultiplying), drawn to [`SIZE`] × [`SIZE`]
//! without smoothing ([`pixels`]); a picture that can't be had or read is none, and a tile with
//! neither is none. The [`MAX_CACHED`] tiles asked last are kept, the most recently used last,
//! each ask while one is on its way waits for that one, and a tile that came to nothing is
//! forgotten once it has (asked again next time). The mosaic is registered once; an answer
//! without an id is kept as none, a registration that fails on the way is asked again later.
//!
//! The map asks through the app's tile fetcher for the source [`NAME`]: a tile's classes, the
//! answer `SIZE²` bytes, one class a pixel from the north-west corner, row by row (the land cover
//! ground, `map::layers::ground::landcover`, and the trees, `map::layers::trees`, the same tiles
//! kept once here as today's one cache serves both). The app's fetcher hands those asks to
//! [`tile`] (the network through [`global`]); the pictures are kept like every other tile in
//! `backend::tiles`' disk cache (folders [`NMD`] and [`WORLDCOVER`]: fresh for the server's
//! `max-age`, else 7 days; a stale copy beats none). A `--fixture` folder's pictures (its
//! `tiles/nmd` and `tiles/worldcover`) are put together by the map itself, `map::io::DirTiles`,
//! the same way.
//!
//! `scene::landcover` has the same tables and rules (ported with `landcover.ts` in stage 1); this
//! module cannot see it (the backend depends on `core` only), so the two are kept equal by hand
//! and the app's tests compare them (and both decodes over the fixture pictures).
//!
//! Deviations, invisible: the pictures go through the app's disk cache instead of the browser's;
//! a picture's pixels whose alpha is neither 0 nor 255 keep their colours exactly (a 2D canvas
//! holds them premultiplied and gives them back rounded; neither server sends any).

use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use indexmap::IndexMap;
use rustradar_core::num::to_fixed;

use crate::CmdResult;
use crate::tiles::{DEFAULT_FRESH, Tiles};

/// The source of a tile's classes, as the map's land cover ground asks the app's tile fetcher
/// for it (`map::layers::ground::landcover::LAND_COVER`).
pub const NAME: &str = "landcover";
/// NMD's pictures: their folder in the disk cache, as a fixture folder's `tiles/nmd`.
pub const NMD: &str = "nmd";
/// WorldCover's pictures: their folder in the disk cache, as a fixture folder's
/// `tiles/worldcover`.
pub const WORLDCOVER: &str = "worldcover";
/// Their credit (`LANDCOVER_ATTRIBUTION`).
pub const ATTRIBUTION: &str = "Land cover: NMD2023 Naturvårdsverket · ESA WorldCover 2021";
/// Pixels across a tile read, and classes across a tile's answer.
pub const SIZE: usize = 256;
/// Tiles kept, the most recently used last (`MAX_CACHED`).
pub const MAX_CACHED: usize = 96;
/// NMD's WMS (`NMD_WMS`).
pub const NMD_WMS: &str = "https://geodata.naturvardsverket.se/geoserver/wms";
/// Microsoft's Planetary Computer's data API (`PC`).
pub const PC: &str = "https://planetarycomputer.microsoft.com/api/data/v1";
/// The query registered for WorldCover 2021's mosaic, as `JSON.stringify` writes it.
pub const WORLD_COVER_SEARCH: &str = r#"{"collections":["esa-worldcover"],"filter-lang":"cql2-json","filter":{"op":"anyinteracts","args":[{"property":"datetime"},{"interval":["2021-01-01T23:59:59Z","2021-12-31T23:59:59Z"]}]}}"#;
/// WorldCover's mosaic is drawn from this zoom.
pub const WORLD_COVER_MIN_ZOOM: u32 = 6;
/// How long the mosaic's registration may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// Half the web mercator world, metres.
const HALF_WORLD: f64 = 20_037_508.342789244;

/// The classes, the map's own (`LC`; `scene::landcover::lc`'s twins).
pub mod lc {
    pub const NONE: u8 = 0;
    pub const CONIFERS: u8 = 1;
    pub const MIXED: u8 = 2;
    pub const BROADLEAVES: u8 = 3;
    /// Trees, kind not known (WorldCover).
    pub const FOREST: u8 = 4;
    /// Felled, growing back.
    pub const CLEARCUT: u8 = 5;
    pub const WETLAND: u8 = 6;
    pub const ARABLE: u8 = 7;
    pub const GRASS: u8 = 8;
    pub const SHRUB: u8 = 9;
    pub const HEATH: u8 = 10;
    pub const BARE: u8 = 11;
    pub const BUILT: u8 = 12;
    pub const WATER: u8 = 13;
    pub const SNOW: u8 = 14;
}

// --- the rules (`scene::landcover`'s twins) ----------------------------------------------------

/// NMD2023 (v2) colours (`0xRRGGBB`), as its legend gives them, to the map's classes, in the
/// TypeScript's order (`landcover.ts:48-64`).
const NMD_COLOURS: &[(u8, &[u32])] = &[
    // forest on firm ground and on wetland: pine, spruce, mixed conifers; conifers with broadleaves; broadleaves; felled
    (lc::CONIFERS, &[0x6E8C05, 0x2D5F00, 0x4E7000, 0x598C55, 0x305E50, 0x23735A]),
    (lc::MIXED, &[0x38A800, 0x438870]),
    (lc::BROADLEAVES, &[0x4CE600, 0xAAFF00, 0x97E600, 0x89CD9B, 0xA5F578, 0xABCD78, 0x00E6A9, 0x55FF00]),
    (lc::CLEARCUT, &[0xCDCD66, 0x898944]),
    // open wetland, of every kind
    (lc::WETLAND, &[0xC29ED7, 0x894465, 0xCD6699, 0xF57AB6, 0xD69DBC, 0x73004C, 0xA80084, 0xE600A9, 0xFF00C5]),
    (lc::WETLAND, &[0x704489, 0xAA66CD, 0xCA7AF5, 0x4C0073, 0x8400A8, 0xA900E6, 0xC500FF]),
    (lc::ARABLE, &[0xFFFFBE]),
    (lc::BARE, &[0xE1E1E1, 0x800080]),
    (lc::SHRUB, &[0xCCD79E, 0xABC9A6, 0x9EB591]),
    (lc::HEATH, &[0xD7C29E, 0xCDAA66, 0x897044]),
    (lc::GRASS, &[0xFFEBAF, 0xFFD37F, 0xFFBF7F]),
    (lc::BUILT, &[0x5A1414, 0xE5464B, 0x191919]),
    (lc::WATER, &[0x6699CD, 0x8ACCFA]),
];

/// The class NMD draws in `colour` (`0xRRGGBB`), none for any other colour.
pub fn nmd_class(colour: u32) -> u8 {
    // (the later of two entries for a colour would win, as in the TypeScript's record; none is
    // named twice)
    NMD_COLOURS.iter().rev().find(|(_, colours)| colours.contains(&colour)).map_or(lc::NONE, |&(class, _)| class)
}

/// WorldCover's codes to the map's classes.
pub fn world_cover_class(code: u8) -> u8 {
    match code {
        10 => lc::FOREST,
        20 => lc::SHRUB,
        30 => lc::GRASS,
        40 => lc::ARABLE,
        50 => lc::BUILT,
        60 => lc::BARE,
        70 => lc::SNOW,
        80 => lc::WATER,
        90 => lc::WETLAND,
        95 => lc::BROADLEAVES,
        100 => lc::HEATH,
        _ => lc::NONE,
    }
}

/// An NMD picture's pixels (RGBA) to classes: none where it is clear (alpha under 128) or not one
/// of its colours.
pub fn nmd_classes(rgba: &[u8]) -> Vec<u8> {
    // (a table of every colour the legend has, looked up once a pixel)
    static TABLE: OnceLock<std::collections::HashMap<u32, u8>> = OnceLock::new();
    let table = TABLE.get_or_init(|| NMD_COLOURS.iter().flat_map(|&(class, colours)| colours.iter().map(move |&c| (c, class))).collect());
    rgba.chunks_exact(4)
        .map(|px| {
            if px[3] < 128 {
                return lc::NONE;
            }
            let colour = (u32::from(px[0]) << 16) | (u32::from(px[1]) << 8) | u32::from(px[2]);
            table.get(&colour).copied().unwrap_or(lc::NONE)
        })
        .collect()
}

/// A WorldCover picture's pixels (its grey, the red channel, the code) to classes; none where it
/// is clear.
pub fn world_cover_classes(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).map(|px| if px[3] < 128 { lc::NONE } else { world_cover_class(px[0]) }).collect()
}

/// A tile's corners in web mercator metres: west, south, east, north.
pub fn bbox(x: u32, y: u32, z: u32) -> [f64; 4] {
    let span = (2.0 * HALF_WORLD) / 2f64.powi(z as i32);
    let west = -HALF_WORLD + f64::from(x) * span;
    let north = HALF_WORLD - f64::from(y) * span;
    [west, north - span, west + span, north]
}

/// Whether a tile's middle is in Sweden's box (NMD's): longitude 10.5–24.5, latitude 55–69.2.
pub fn in_sweden(x: u32, y: u32, z: u32) -> bool {
    let n = 2f64.powi(z as i32);
    let lon = ((f64::from(x) + 0.5) / n) * 360.0 - 180.0;
    let lat = ((std::f64::consts::PI * (1.0 - (2.0 * (f64::from(y) + 0.5)) / n)).sinh().atan() * 180.0) / std::f64::consts::PI;
    lon > 10.5 && lon < 24.5 && lat > 55.0 && lat < 69.2
}

/// NMD's picture of a tile from the WMS at `wms` ([`NMD_WMS`]): [`SIZE`] pixels across, in web
/// mercator, clear where it has nothing; the box to the millimetre.
pub fn nmd_url_at(wms: &str, x: u32, y: u32, z: u32) -> String {
    let bbox = bbox(x, y, z).map(|v| to_fixed(v, 3)).join(",");
    format!(
        "{wms}?service=WMS&version=1.3.0&request=GetMap&layers=lc-nmd:LC.LandCoverRaster.Bas.2023.v2.x&styles=&crs=EPSG:3857\
         &bbox={bbox}&width={SIZE}&height={SIZE}&format=image/png&transparent=true"
    )
}

/// NMD's picture of a tile (`nmdUrl`).
pub fn nmd_url(x: u32, y: u32, z: u32) -> String {
    nmd_url_at(NMD_WMS, x, y, z)
}

/// Where the mosaic is registered, under the data API `pc` ([`PC`]).
pub fn register_url(pc: &str) -> String {
    format!("{pc}/mosaic/register")
}

/// The mosaic's id in the register's answer (`j?.searchid ?? null`; an empty one is none, as
/// `worldCover`'s `if (!id)` has it).
pub fn mosaic_id(answer: &serde_json::Value) -> Option<&str> {
    answer.get("searchid").and_then(serde_json::Value::as_str).filter(|id| !id.is_empty())
}

/// WorldCover's picture of a tile in the mosaic `id` under the data API `pc`; none under
/// [`WORLD_COVER_MIN_ZOOM`].
pub fn world_cover_url_at(pc: &str, id: &str, x: u32, y: u32, z: u32) -> Option<String> {
    (z >= WORLD_COVER_MIN_ZOOM).then(|| format!("{pc}/mosaic/{id}/tiles/WebMercatorQuad/{z}/{x}/{y}@1x?assets=map&collection=esa-worldcover&format=png"))
}

/// WorldCover's picture of a tile in the mosaic `id`.
pub fn world_cover_url(id: &str, x: u32, y: u32, z: u32) -> Option<String> {
    world_cover_url_at(PC, id, x, y, z)
}

/// Whether WorldCover is wanted for a tile with these NMD classes: none (out of Sweden, or not to
/// be had), or clear somewhere.
pub fn wants_world_cover(nmd: Option<&[u8]>) -> bool {
    nmd.is_none_or(|classes| classes.contains(&lc::NONE))
}

/// A tile's classes from NMD's and WorldCover's, either missing: NMD's where it has one,
/// WorldCover's where it is clear; none when neither came.
pub fn combine(nmd: Option<Vec<u8>>, world: Option<Vec<u8>>) -> Option<Vec<u8>> {
    match (nmd, world) {
        (Some(mut classes), Some(world)) => {
            for (i, c) in classes.iter_mut().enumerate() {
                if *c == lc::NONE {
                    // (a store of `undefined` into a Uint8Array is 0)
                    *c = world.get(i).copied().unwrap_or(lc::NONE);
                }
            }
            Some(classes)
        }
        (None, world) => world,
        (nmd, None) => nmd,
    }
}

/// A picture's pixels (`pixels`): its RGBA exactly as stored (no colour management, no
/// premultiplying), drawn to [`SIZE`] × [`SIZE`] without smoothing (each pixel the picture's
/// under its middle); none for bytes that are no picture.
pub fn pixels(picture: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(picture).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    if (w as usize, h as usize) == (SIZE, SIZE) {
        return Some(image.into_raw());
    }
    if w == 0 || h == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        let sy = (((y as f64 + 0.5) * f64::from(h)) / SIZE as f64) as u32;
        for x in 0..SIZE {
            let sx = (((x as f64 + 0.5) * f64::from(w)) / SIZE as f64) as u32;
            out.extend_from_slice(&image.get_pixel(sx.min(w - 1), sy.min(h - 1)).0);
        }
    }
    Some(out)
}

// --- the network -------------------------------------------------------------------------------

/// Where the pictures are asked for: the live servers ([`Endpoints::live`]), or a test's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// NMD's WMS ([`NMD_WMS`]).
    pub nmd_wms: String,
    /// The Planetary Computer's data API ([`PC`]).
    pub pc: String,
}

impl Endpoints {
    pub fn live() -> Self {
        Endpoints { nmd_wms: NMD_WMS.to_owned(), pc: PC.to_owned() }
    }
}

/// A tile's answer on its way, shared by everyone who asks for it meanwhile.
type Pending = Shared<BoxFuture<'static, Option<Arc<[u8]>>>>;

/// Today's `landCover` and its caches (see the module docs). Cheap to clone; every clone is the
/// same.
#[derive(Clone)]
pub struct LandCover {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    endpoints: Endpoints,
    /// The mosaic's id once registered (`Some(None)`: the answer had none, kept so).
    mosaic: tokio::sync::Mutex<Option<Option<String>>>,
    /// The tiles asked last, the most recently used last.
    cache: Mutex<IndexMap<(u32, u32, u32), Pending>>,
}

impl std::fmt::Debug for LandCover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LandCover").field("endpoints", &self.inner.endpoints).field("cached", &self.cached().len()).finish_non_exhaustive()
    }
}

impl LandCover {
    /// Land cover from `endpoints`, the mosaic registered with `http` (the pictures go through the
    /// [`Tiles`] each ask brings).
    pub fn new(http: reqwest::Client, endpoints: Endpoints) -> Self {
        LandCover { inner: Arc::new(Inner { http, endpoints, mosaic: tokio::sync::Mutex::new(None), cache: Mutex::new(IndexMap::new()) }) }
    }

    /// The tiles kept, least recently used first (`z, x, y`).
    pub fn cached(&self) -> Vec<(u32, u32, u32)> {
        self.inner.cache.lock().unwrap_or_else(PoisonError::into_inner).keys().copied().collect()
    }

    /// WorldCover 2021's mosaic id (`worldCoverMosaic`), registered at the first ask: none when
    /// the registration answers without one (kept so) or fails on the way (asked again next time).
    /// Asks made while it is being registered wait for it.
    pub async fn mosaic(&self) -> Option<String> {
        let mut mosaic = self.inner.mosaic.lock().await;
        if let Some(id) = &*mosaic {
            return id.clone();
        }
        match self.register().await {
            Ok(id) => {
                *mosaic = Some(id.clone());
                id
            }
            // asked again later
            Err(e) => {
                log::warn!("land cover: WorldCover's mosaic: {e}");
                None
            }
        }
    }

    /// One registration: the id (none for an answer that is not ok or holds none), or why it
    /// failed on the way (no answer, or an answer that is no JSON).
    async fn register(&self) -> CmdResult<Option<String>> {
        let response = self
            .inner
            .http
            .post(register_url(&self.inner.endpoints.pc))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(WORLD_COVER_SEARCH)
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("can't reach the Planetary Computer: {e}"))?;
        if !response.status().is_success() {
            return Ok(None);
        }
        let body = response.bytes().await.map_err(|e| e.to_string())?;
        let answer: serde_json::Value = serde_json::from_slice(&body).map_err(|e| format!("the register's answer is no JSON: {e}"))?;
        Ok(answer.get("searchid").and_then(serde_json::Value::as_str).map(str::to_owned))
    }

    /// NMD's picture of tile `z/x/y` as the WMS sends it, through `tiles` (folder [`NMD`]): none
    /// where the server has nothing (404).
    pub async fn nmd_picture(&self, tiles: &Tiles, z: u32, x: u32, y: u32) -> CmdResult<Option<Vec<u8>>> {
        tiles.get(&format!("{NMD}/{z}/{x}/{y}"), &nmd_url_at(&self.inner.endpoints.nmd_wms, x, y, z), DEFAULT_FRESH).await
    }

    /// WorldCover's picture of tile `z/x/y` as the mosaic sends it, through `tiles` (folder
    /// [`WORLDCOVER`]): none under [`WORLD_COVER_MIN_ZOOM`] (nothing is asked), without a mosaic,
    /// or where the server has nothing.
    pub async fn world_cover_picture(&self, tiles: &Tiles, z: u32, x: u32, y: u32) -> CmdResult<Option<Vec<u8>>> {
        if z < WORLD_COVER_MIN_ZOOM {
            return Ok(None);
        }
        let Some(id) = self.mosaic().await.filter(|id| !id.is_empty()) else { return Ok(None) };
        let Some(url) = world_cover_url_at(&self.inner.endpoints.pc, &id, x, y, z) else { return Ok(None) };
        tiles.get(&format!("{WORLDCOVER}/{z}/{x}/{y}"), &url, DEFAULT_FRESH).await
    }

    /// Tile `z/x/y`'s classes (`landCover`, see the module docs): [`SIZE`]² of them from its
    /// north-west corner, row by row, or none where there are none to be had. Kept (the
    /// [`MAX_CACHED`] asked last); an ask while the tile is on its way waits for the same answer,
    /// which comes to its end whoever waits. Spawns on the current tokio runtime.
    pub fn tile(&self, tiles: &Tiles, z: u32, x: u32, y: u32) -> impl Future<Output = Option<Arc<[u8]>>> + Send + 'static {
        let key = (z, x, y);
        let mut cache = self.inner.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(hit) = cache.shift_remove(&key) {
            // the most recently used last
            cache.insert(key, hit.clone());
            return hit;
        }
        let (this, tiles) = (self.clone(), tiles.clone());
        let work = tokio::spawn(async move {
            let got = this.classes(&tiles, z, x, y).await;
            if got.is_none() {
                // a failure is asked again once forgotten
                this.inner.cache.lock().unwrap_or_else(PoisonError::into_inner).shift_remove(&key);
            }
            got
        });
        let pending: Pending = async move { work.await.ok().flatten() }.boxed().shared();
        cache.insert(key, pending.clone());
        while cache.len() > MAX_CACHED {
            cache.shift_remove_index(0);
        }
        pending
    }

    /// One tile's classes, asked for now.
    async fn classes(&self, tiles: &Tiles, z: u32, x: u32, y: u32) -> Option<Arc<[u8]>> {
        let mut classes = None;
        if in_sweden(x, y, z) {
            classes = match self.nmd_picture(tiles, z, x, y).await {
                Ok(Some(png)) => pixels(&png).map(|px| nmd_classes(&px)),
                _ => None,
            };
        }
        // past NMD's edge, WorldCover's
        if wants_world_cover(classes.as_deref()) {
            let world = match self.world_cover_picture(tiles, z, x, y).await {
                Ok(Some(png)) => pixels(&png).map(|px| world_cover_classes(&px)),
                _ => None,
            };
            classes = combine(classes, world);
        }
        classes.map(Arc::from)
    }
}

/// The app's land cover: the live servers, the mosaic registered through the app's HTTP client
/// (`fr24::client_builder()`, doc 07 §2.3), made at the first ask.
pub fn global() -> Option<&'static LandCover> {
    static LAND_COVER: OnceLock<Option<LandCover>> = OnceLock::new();
    LAND_COVER
        .get_or_init(|| {
            let http = fr24::client_builder()
                .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
                .timeout(TIMEOUT)
                .build()
                .ok()?;
            Some(LandCover::new(http, Endpoints::live()))
        })
        .as_ref()
}

/// Tile `z/x/y`'s classes from the app's land cover ([`global`]), its pictures through `tiles`:
/// the answer for the source [`NAME`] (`Ok(None)`: none to be had).
pub async fn tile(tiles: &Tiles, z: u32, x: u32, y: u32) -> CmdResult<Option<Vec<u8>>> {
    let land = global().ok_or("no HTTP client for the land cover")?;
    Ok(land.tile(tiles, z, x, y).await.map(|classes| classes.to_vec()))
}


#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::tiles::{CAP_BYTES, Config, PER_HOST};

    /// The committed fixture pictures (plan, "Fixtures for stages 8-10"): `tiles/nmd` and
    /// `tiles/worldcover`.
    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../map/tests/data/tiles")
    }

    fn fixture(source: &str, z: u32, x: u32, y: u32) -> Vec<u8> {
        let path = fixtures().join(format!("{source}/{z}/{x}/{y}.png"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Every fixture picture of `source`, as `(z, x, y, path)`.
    fn every(source: &str) -> Vec<(u32, u32, u32, PathBuf)> {
        let mut out = Vec::new();
        let numbered = |dir: &Path| -> Vec<(u32, PathBuf)> {
            let mut v: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| Some((e.path().file_stem()?.to_str()?.parse().ok()?, e.path())))
                .collect();
            v.sort();
            v
        };
        for (z, zdir) in numbered(&fixtures().join(source)) {
            for (x, xdir) in numbered(&zdir) {
                for (y, path) in numbered(&xdir) {
                    out.push((z, x, y, path));
                }
            }
        }
        out
    }

    /// Classes counted: `counts[c]` cells of class `c`.
    fn histogram(classes: &[u8]) -> Vec<(u8, usize)> {
        let mut counts = [0usize; 256];
        for &c in classes {
            counts[usize::from(c)] += 1;
        }
        counts.iter().enumerate().filter(|(_, n)| **n > 0).map(|(c, &n)| (c as u8, n)).collect()
    }

    // --- the rules, as `landcover.test.ts` and the scene's twins have them ----------------------

    #[test]
    fn nmds_colours_back_to_their_classes_pine_birch_a_mire_a_field_clear_and_unknown_are_none() {
        let rgba = [[0x6e, 0x8c, 0x05, 255], [0x4c, 0xe6, 0x00, 255], [0xc2, 0x9e, 0xd7, 255], [0xff, 0xff, 0xbe, 255], [0x6e, 0x8c, 0x05, 0], [1, 2, 3, 255]].concat();
        assert_eq!(nmd_classes(&rgba), [lc::CONIFERS, lc::BROADLEAVES, lc::WETLAND, lc::ARABLE, lc::NONE, lc::NONE]);
    }

    #[test]
    fn worldcovers_codes_its_grey() {
        let rgba = [[10, 10, 10, 255], [40, 40, 40, 255], [80, 80, 80, 255], [50, 50, 50, 0]].concat();
        assert_eq!(world_cover_classes(&rgba), [lc::FOREST, lc::ARABLE, lc::WATER, lc::NONE]);
    }

    #[test]
    fn every_legend_colour_is_read_back_and_no_colour_is_named_twice() {
        let count: usize = NMD_COLOURS.iter().map(|(_, colours)| colours.len()).sum();
        assert_eq!(count, 51);
        let mut seen = std::collections::HashSet::new();
        for &(class, colours) in NMD_COLOURS {
            for &colour in colours {
                assert!(seen.insert(colour), "#{colour:06X} twice");
                let [_, r, g, b] = colour.to_be_bytes();
                assert_eq!(nmd_classes(&[r, g, b, 128]), [class], "#{colour:06X}");
                assert_eq!(nmd_class(colour), class);
                // (alpha under 128 is clear)
                assert_eq!(nmd_classes(&[r, g, b, 127]), [lc::NONE]);
            }
        }
        // the wetland's two lines, and nothing else is a class
        assert_eq!(NMD_COLOURS.iter().filter(|(c, _)| *c == lc::WETLAND).map(|(_, v)| v.len()).sum::<usize>(), 16);
        assert_eq!(nmd_class(0x000000), lc::NONE);
        assert_eq!(nmd_class(0xFFFFFF), lc::NONE);
    }

    #[test]
    fn worldcovers_eleven_codes_and_nothing_else() {
        let known: Vec<(u8, u8)> = (0..=255).map(|code| (code, world_cover_class(code))).filter(|&(_, c)| c != lc::NONE).collect();
        assert_eq!(
            known,
            [
                (10, lc::FOREST),
                (20, lc::SHRUB),
                (30, lc::GRASS),
                (40, lc::ARABLE),
                (50, lc::BUILT),
                (60, lc::BARE),
                (70, lc::SNOW),
                (80, lc::WATER),
                (90, lc::WETLAND),
                (95, lc::BROADLEAVES),
                (100, lc::HEATH)
            ]
        );
        // only the red channel is the code
        assert_eq!(world_cover_classes(&[10, 0, 0, 200, 10, 0, 0, 127]), [lc::FOREST, lc::NONE]);
    }

    #[test]
    fn nmds_url_names_the_tiles_box_to_the_millimetre() {
        // Lund's zoom-14 tile and quarters of the world, as the TypeScript writes them
        assert_eq!(
            nmd_url(8792, 5125, 14),
            "https://geodata.naturvardsverket.se/geoserver/wms?service=WMS&version=1.3.0&request=GetMap\
             &layers=lc-nmd:LC.LandCoverRaster.Bas.2023.v2.x&styles=&crs=EPSG:3857\
             &bbox=1467590.943,7499389.719,1470036.928,7501835.704&width=256&height=256&format=image/png&transparent=true"
        );
        assert!(nmd_url(0, 1, 1).contains("&bbox=-20037508.343,-20037508.343,0.000,0.000&"));
        assert!(nmd_url(3, 1, 2).contains("&bbox=10018754.171,0.000,20037508.343,10018754.171&"));
        assert_eq!(bbox(0, 0, 0), [-HALF_WORLD, -HALF_WORLD, HALF_WORLD, HALF_WORLD]);
    }

    #[test]
    fn sweden_by_its_tiles_middles() {
        // Lund and Kiruna in; Berlin south of the box, Bergen west of it
        assert!(in_sweden(8792, 5125, 14));
        assert!(in_sweden(569, 246, 10));
        assert!(!in_sweden(8801, 5374, 14));
        assert!(!in_sweden(527, 295, 10));
        // the fixtures' tiles round Arlanda and Stockholm
        for (z, x, y, _) in every(NMD) {
            assert!(in_sweden(x, y, z), "{z}/{x}/{y}");
        }
    }

    #[test]
    fn worldcovers_mosaic_its_id_and_tiles_from_zoom_6() {
        let search: serde_json::Value = serde_json::from_str(WORLD_COVER_SEARCH).unwrap();
        assert_eq!(search["collections"][0], "esa-worldcover");
        assert_eq!(search["filter-lang"], "cql2-json");
        assert_eq!(search["filter"]["args"][1]["interval"][0], "2021-01-01T23:59:59Z");
        assert_eq!(search["filter"]["args"][1]["interval"][1], "2021-12-31T23:59:59Z");
        assert_eq!(register_url(PC), "https://planetarycomputer.microsoft.com/api/data/v1/mosaic/register");
        assert_eq!(mosaic_id(&serde_json::json!({"searchid": "abc"})), Some("abc"));
        assert_eq!(mosaic_id(&serde_json::json!({"searchid": ""})), None);
        assert_eq!(mosaic_id(&serde_json::json!(null)), None);
        assert_eq!(world_cover_url("abc", 1, 2, 5), None);
        assert_eq!(
            world_cover_url("abc", 35, 20, 6).as_deref(),
            Some("https://planetarycomputer.microsoft.com/api/data/v1/mosaic/abc/tiles/WebMercatorQuad/6/35/20@1x?assets=map&collection=esa-worldcover&format=png")
        );
        // the fixtures' registration
        let register: serde_json::Value = serde_json::from_slice(&std::fs::read(fixtures().join("worldcover/register.json")).unwrap()).unwrap();
        assert!(mosaic_id(&register).is_some_and(|id| id.starts_with("pc1.")));
    }

    #[test]
    fn nmd_first_worldcover_where_it_is_clear() {
        assert!(wants_world_cover(None));
        assert!(wants_world_cover(Some(&[1, 0, 2])));
        assert!(!wants_world_cover(Some(&[1, 3, 2])));
        assert_eq!(combine(None, None), None);
        assert_eq!(combine(None, Some(vec![4, 4])), Some(vec![4, 4]));
        assert_eq!(combine(Some(vec![0, 1]), None), Some(vec![0, 1]));
        assert_eq!(combine(Some(vec![0, 1, 0]), Some(vec![13, 4])), Some(vec![13, 1, 0]));
    }

    // --- the fixture pictures --------------------------------------------------------------------

    /// The fixtures' pictures decode as the servers drew them: 256 × 256, NMD's opaque pixels
    /// every one a legend colour and its clear ones fully clear, WorldCover's greys every one a
    /// code; two tiles' classes counted as an independent reading (PIL) counts them.
    #[test]
    fn the_fixture_pictures_decode_to_their_classes() {
        let (nmd, world) = (every(NMD), every(WORLDCOVER));
        assert_eq!((nmd.len(), world.len()), (66, 66));
        for (z, x, y, path) in &nmd {
            let px = pixels(&std::fs::read(path).unwrap()).unwrap_or_else(|| panic!("{z}/{x}/{y} is no picture"));
            assert_eq!(px.len(), SIZE * SIZE * 4);
            let classes = nmd_classes(&px);
            for (p, &c) in px.chunks_exact(4).zip(&classes) {
                assert!(p[3] == 0 || p[3] == 255, "{z}/{x}/{y}: alpha {}", p[3]);
                assert!(p[3] == 0 || c != lc::NONE, "{z}/{x}/{y}: #{:02X}{:02X}{:02X} is no legend colour", p[0], p[1], p[2]);
            }
        }
        for (z, x, y, path) in &world {
            let px = pixels(&std::fs::read(path).unwrap()).unwrap_or_else(|| panic!("{z}/{x}/{y} is no picture"));
            for (p, c) in px.chunks_exact(4).zip(world_cover_classes(&px)) {
                assert!(p[3] == 0 || (c != lc::NONE && p[0] == p[1] && p[1] == p[2]), "{z}/{x}/{y}: {p:?}");
            }
        }
        // Arlanda's west (V4) and the middle of Stockholm (V5's close-up), counted by PIL
        let at = |source: &str, z, x, y| histogram(&if source == NMD { nmd_classes } else { world_cover_classes }(&pixels(&fixture(source, z, x, y)).unwrap()));
        assert_eq!(
            at(NMD, 14, 9007, 4787),
            [(1, 10873), (2, 2183), (3, 13804), (5, 13577), (6, 4032), (7, 580), (8, 7491), (9, 1887), (10, 5894), (11, 748), (12, 4417), (13, 50)]
        );
        assert_eq!(at(WORLDCOVER, 14, 9007, 4787), [(4, 41419), (6, 335), (7, 130), (8, 21976), (11, 220), (12, 1456)]);
        assert_eq!(at(NMD, 16, 36057, 19273), [(1, 961), (2, 1405), (3, 2750), (8, 2536), (9, 871), (10, 1521), (11, 264), (12, 38314), (13, 16914)]);
        assert_eq!(at(WORLDCOVER, 16, 36057, 19273), [(4, 5823), (8, 2540), (11, 458), (12, 37827), (13, 18888)]);
    }

    #[test]
    fn a_picture_of_another_size_is_drawn_to_256_without_smoothing_and_no_picture_is_none() {
        // 2 × 2: each quarter of the tile one pixel's colour
        let mut small = image::RgbaImage::new(2, 2);
        for (x, y, p) in small.enumerate_pixels_mut() {
            *p = image::Rgba([x as u8 * 100, y as u8 * 100, 7, 255]);
        }
        let mut png = Vec::new();
        small.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let px = pixels(&png).unwrap();
        let at = |x: usize, y: usize| &px[(y * SIZE + x) * 4..][..4];
        assert_eq!(at(0, 0), [0, 0, 7, 255]);
        assert_eq!(at(127, 127), [0, 0, 7, 255]);
        assert_eq!(at(128, 0), [100, 0, 7, 255]);
        assert_eq!(at(0, 128), [0, 100, 7, 255]);
        assert_eq!(at(255, 255), [100, 100, 7, 255]);
        assert_eq!(pixels(b"no picture"), None);
        assert_eq!(pixels(&[]), None);
    }

    // --- over the network --------------------------------------------------------------------------

    /// What the test server answers.
    struct Answers {
        /// The register's status and body.
        register: (u16, &'static str),
        /// NMD's picture by its box's west edge (`bbox=<west>,…`), else 404.
        nmd: Vec<(String, Vec<u8>)>,
        /// WorldCover's picture by its tile path (`z/x/y`), else 404.
        world: Vec<(String, Vec<u8>)>,
    }

    /// A server on 127.0.0.1 standing in for both, noting each request (`METHOD path`).
    async fn server(answers: Answers) -> (Endpoints, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (noted, answers) = (asked.clone(), Arc::new(answers));
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (noted, answers) = (noted.clone(), answers.clone());
                tokio::spawn(async move {
                    let (mut head, mut buf) = (Vec::new(), [0u8; 4096]);
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let text = String::from_utf8_lossy(&head).into_owned();
                    let mut words = text.split_whitespace();
                    let (method, path) = (words.next().unwrap_or("").to_owned(), words.next().unwrap_or("/").to_owned());
                    noted.lock().unwrap().push(format!("{method} {path}"));
                    let (status, body): (u16, Vec<u8>) = if path == "/pc/mosaic/register" {
                        // (the body is read and checked to be the query)
                        let length: usize = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                        let start = text.find("\r\n\r\n").unwrap() + 4;
                        let mut body = head[start..].to_vec();
                        while body.len() < length {
                            let n = socket.read(&mut buf).await.unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            body.extend_from_slice(&buf[..n]);
                        }
                        assert_eq!(body, WORLD_COVER_SEARCH.as_bytes());
                        assert!(text.to_ascii_lowercase().contains("content-type: application/json"));
                        (answers.register.0, answers.register.1.as_bytes().to_vec())
                    } else if path.starts_with("/wms?") {
                        let found = answers.nmd.iter().find(|(west, _)| path.contains(&format!("&bbox={west},")));
                        found.map_or((404, Vec::new()), |(_, png)| (200, png.clone()))
                    } else if let Some(rest) = path.strip_prefix("/pc/mosaic/") {
                        let found = answers.world.iter().find(|(zxy, _)| rest.contains(&format!("/tiles/WebMercatorQuad/{zxy}@1x?")));
                        found.map_or((404, Vec::new()), |(_, png)| (200, png.clone()))
                    } else {
                        (404, Vec::new())
                    };
                    let head = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (Endpoints { nmd_wms: format!("{base}/wms"), pc: format!("{base}/pc") }, asked)
    }

    /// A cache folder of its own for one test, removed when it ends.
    struct TempRoot(PathBuf);

    static ROOTS: AtomicUsize = AtomicUsize::new(0);

    impl TempRoot {
        fn new() -> Self {
            let n = ROOTS.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!("rustradar-landcover-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            Self(root)
        }

        fn tiles(&self) -> Tiles {
            Tiles::with_config(Config { root: self.0.clone(), cap_bytes: CAP_BYTES, per_host: PER_HOST, client: Some(client()) }).unwrap()
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn client() -> reqwest::Client {
        fr24::client_builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap()
    }

    /// The west edge of a tile's box as NMD's URL writes it.
    fn west(x: u32, y: u32, z: u32) -> String {
        to_fixed(bbox(x, y, z)[0], 3)
    }

    /// A picture `SIZE` across of one colour, its left `clear` columns clear.
    fn solid_png(rgb: [u8; 3], clear: usize) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(SIZE as u32, SIZE as u32, |x, _| if (x as usize) < clear { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([rgb[0], rgb[1], rgb[2], 255]) });
        let mut png = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        png
    }

    /// A fixture tile through the network path: NMD's picture alone (it is clear nowhere), its
    /// classes as the picture's, kept on disk under `nmd/`; asked again it comes from the cache in
    /// memory, nothing asked of the servers; WorldCover never asked for.
    #[tokio::test]
    async fn a_swedish_tile_is_nmd_s_and_is_kept() {
        let root = TempRoot::new();
        let (endpoints, asked) = server(Answers { register: (200, r#"{"searchid":"abc"}"#), nmd: vec![(west(9007, 4787, 14), fixture(NMD, 14, 9007, 4787))], world: vec![] }).await;
        let land = LandCover::new(client(), endpoints);
        let tiles = root.tiles();
        let classes = land.tile(&tiles, 14, 9007, 4787).await.expect("classes");
        assert_eq!(&*classes, nmd_classes(&pixels(&fixture(NMD, 14, 9007, 4787)).unwrap()).as_slice());
        assert!(root.0.join("nmd/14/9007/4787").is_file());
        assert_eq!(asked.lock().unwrap().len(), 1, "{:?}", asked.lock().unwrap());
        assert!(asked.lock().unwrap()[0].starts_with("GET /wms?service=WMS&version=1.3.0&request=GetMap&layers=lc-nmd:"));
        let again = land.tile(&tiles, 14, 9007, 4787).await.expect("classes");
        assert!(Arc::ptr_eq(&classes, &again));
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert_eq!(land.cached(), [(14, 9007, 4787)]);
    }

    /// Past NMD's edge: where its picture is clear WorldCover's classes fill it (the mosaic
    /// registered once, for every tile); out of Sweden WorldCover's alone; under zoom 6 nothing is
    /// to be had, and nothing is asked.
    #[tokio::test]
    async fn worldcover_fills_what_nmd_leaves_clear_and_stands_alone_abroad() {
        let root = TempRoot::new();
        // a tile on Sweden's edge, NMD's left half clear (pine), WorldCover farmland; Berlin
        let (edge, berlin) = ((8792, 5125, 14), (8801, 5374, 14));
        let (endpoints, asked) = server(Answers {
            register: (200, r#"{"searchid":"m1"}"#),
            nmd: vec![(west(edge.0, edge.1, edge.2), solid_png([0x6E, 0x8C, 0x05], 100))],
            world: vec![("14/8792/5125".into(), solid_png([40, 40, 40], 0)), ("14/8801/5374".into(), solid_png([10, 10, 10], 0))],
        })
        .await;
        let land = LandCover::new(client(), endpoints);
        let tiles = root.tiles();
        let classes = land.tile(&tiles, edge.2, edge.0, edge.1).await.expect("classes");
        assert_eq!(classes.len(), SIZE * SIZE);
        for y in [0, 128, 255] {
            for x in [0, 99, 100, 255] {
                assert_eq!(classes[y * SIZE + x], if x < 100 { lc::ARABLE } else { lc::CONIFERS }, "{x}, {y}");
            }
        }
        let abroad = land.tile(&tiles, berlin.2, berlin.0, berlin.1).await.expect("classes");
        assert!(abroad.iter().all(|&c| c == lc::FOREST));
        assert_eq!(land.tile(&tiles, 5, 16, 10).await, None);
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.iter().filter(|a| a.starts_with("POST /pc/mosaic/register")).count(), 1, "{asked:?}");
        assert_eq!(asked.iter().filter(|a| a.starts_with("GET /wms?")).count(), 1, "{asked:?}");
        assert!(asked.iter().any(|a| a == "GET /pc/mosaic/m1/tiles/WebMercatorQuad/14/8792/5125@1x?assets=map&collection=esa-worldcover&format=png"));
        assert!(asked.iter().any(|a| a.starts_with("GET /pc/mosaic/m1/tiles/WebMercatorQuad/14/8801/5374@1x?")));
        assert_eq!(asked.len(), 4, "{asked:?}");
        assert!(root.0.join("worldcover/14/8801/5374").is_file());
        // the pictures themselves, as kept on disk
        assert_eq!(land.world_cover_picture(&tiles, 14, 8801, 5374).await, Ok(Some(solid_png([10, 10, 10], 0))));
        assert_eq!(land.world_cover_picture(&tiles, 5, 17, 9).await, Ok(None));
        assert_eq!(land.nmd_picture(&tiles, 14, 8792, 5125).await, Ok(Some(solid_png([0x6E, 0x8C, 0x05], 100))));
    }

    /// A tile with nothing to be had is forgotten once it has come to nothing, so it is asked
    /// again; the register's answer without an id is kept (WorldCover never asked for), and a
    /// register that can't be reached is asked again.
    #[tokio::test]
    async fn nothing_is_forgotten_and_asked_again_and_a_mosaic_without_an_id_is_kept() {
        let root = TempRoot::new();
        let (endpoints, asked) = server(Answers { register: (500, "busy"), nmd: vec![], world: vec![] }).await;
        let land = LandCover::new(client(), endpoints);
        let tiles = root.tiles();
        assert_eq!(land.tile(&tiles, 14, 9007, 4787).await, None);
        assert!(land.cached().is_empty());
        assert_eq!(land.tile(&tiles, 14, 9007, 4787).await, None);
        let asked = asked.lock().unwrap().clone();
        // the register once (its answer, without an id, kept)
        assert_eq!(asked.iter().filter(|a| a.starts_with("POST ")).count(), 1, "{asked:?}");
        assert!(asked.iter().all(|a| !a.contains("/tiles/WebMercatorQuad/")), "{asked:?}");
        // a register nobody answers: none now, asked again next time
        let nowhere = LandCover::new(client(), Endpoints { nmd_wms: "http://127.0.0.1:9/wms".into(), pc: "http://127.0.0.1:9/pc".into() });
        assert_eq!(nowhere.mosaic().await, None);
        assert_eq!(*nowhere.inner.mosaic.lock().await, None);
        // an answer without an id is none, kept
        let (endpoints, asked) = server(Answers { register: (200, r#"{"links":[]}"#), nmd: vec![], world: vec![] }).await;
        let land = LandCover::new(client(), endpoints);
        assert_eq!(land.mosaic().await, None);
        assert_eq!(land.mosaic().await, None);
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    /// Asks for one tile while it is on its way wait for the same answer (one request); past
    /// [`MAX_CACHED`] tiles the least recently used are forgotten, a hit moving to the back.
    #[tokio::test]
    async fn asks_share_a_tile_on_its_way_and_the_least_recently_used_go_first() {
        let root = TempRoot::new();
        let png = solid_png([0x38, 0xA8, 0x00], 0);
        let nmd: Vec<(String, Vec<u8>)> = (0..MAX_CACHED as u32 + 2).map(|i| (west(9000 + i, 4787, 14), png.clone())).collect();
        let (endpoints, asked) = server(Answers { register: (200, "{}"), nmd, world: vec![] }).await;
        let land = LandCover::new(client(), endpoints);
        let tiles = root.tiles();
        let (a, b) = tokio::join!(land.tile(&tiles, 14, 9000, 4787), land.tile(&tiles, 14, 9000, 4787));
        assert!(Arc::ptr_eq(&a.unwrap(), &b.unwrap()));
        assert_eq!(asked.lock().unwrap().len(), 1);
        for i in 1..MAX_CACHED as u32 {
            assert!(land.tile(&tiles, 14, 9000 + i, 4787).await.is_some_and(|c| c.iter().all(|&c| c == lc::MIXED)));
        }
        assert_eq!(land.cached().len(), MAX_CACHED);
        // the first, used again, outlives the second
        land.tile(&tiles, 14, 9000, 4787).await;
        land.tile(&tiles, 14, 9000 + MAX_CACHED as u32, 4787).await;
        let cached = land.cached();
        assert_eq!(cached.len(), MAX_CACHED);
        assert!(cached.contains(&(14, 9000, 4787)) && !cached.contains(&(14, 9001, 4787)));
        assert_eq!(cached.last(), Some(&(14, 9000 + MAX_CACHED as u32, 4787)));
        assert_eq!(asked.lock().unwrap().len(), MAX_CACHED + 1);
    }
}
