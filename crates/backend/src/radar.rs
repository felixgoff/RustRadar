//! Shared ground for the 3D weather radar.
//!
//! Each source turns its native product into a [`Volume`]: a flat composite
//! for the ground, one set of filled contour polygons per height, and a mask
//! of where its radars actually see. The frontend stacks the layers so storms
//! stand up. NOAA MRMS over North America measures reflectivity aloft and
//! gives real heights; DWD over central Europe gives a finer composite whose
//! heights, like RainViewer's everywhere else, are estimated from intensity.
//!
//! Layers travel as smooth isobands rather than coloured pixels, so a storm
//! keeps clean edges however far the globe zooms in: [`Polygon`]s in longitude
//! and latitude, made by [`contour`].
//!
//! The native app takes them as they are ([`crate::Backend::radar_volumes`]:
//! each volume shared, nothing encoded); it triangulates them off the UI thread
//! (`map::layers::radar::measured`, item W4). Today's frontend gets each layer
//! as a blob instead ([`Volume::image`], `radar_image`; [`encode`] describes the
//! format), which is also how the test fixtures record a volume ([`read`]
//! reads one back; [`decode`] is the blob's inverse).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use contour::ContourBuilder;
use fr24::grpc::BoundingBox;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// MRMS publishes every two minutes and DWD every five, but a volume is tens
/// of megabytes and nothing on a flight tracker turns on eight-minute-old
/// rain, so trade freshness for bandwidth.
pub const MAX_AGE: Duration = Duration::from_secs(480);

/// Weakest echo drawn on the ground, and up in the air: aloft only moderate
/// rain and stronger, so widespread light rain stays a flat sheet and storm
/// cores stand out as columns.
pub const GROUND_MIN_DBZ: f32 = 5.0;
pub const LIFTED_MIN_DBZ: f32 = 25.0;

// A source's region, degrees, and what the globe is told of its volume: `rustradar_core::model`.
pub use rustradar_core::model::{RadarBounds as Bounds, RadarVolume as VolumeInfo};

/// One filled isoband polygon, in degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct Polygon {
    /// The isoband: [`PALETTE`] index, `[5 + 5·band, 10 + 5·band)` dBZ.
    pub band: u8,
    /// `[lon, lat]` rings. Ring 0 is the outer ring and runs counter-clockwise;
    /// the rest are holes and run clockwise (where the next stronger band sits,
    /// so the bands never overlap). Closed implicitly: the first point is not
    /// repeated.
    pub rings: Vec<Vec<[f32; 2]>>,
}

/// One source's current scan, reduced to what the globe draws.
pub struct Volume {
    /// Short name the frontend asks for layers by, e.g. `mrms`.
    pub source: &'static str,
    /// Unix seconds of the newest height's scan.
    pub time: i64,
    pub fetched: Instant,
    pub bounds: Bounds,
    /// Isobands ([`contour`]) of the strongest echo at any height, for the
    /// ground; empty when nothing reaches [`GROUND_MIN_DBZ`].
    pub ground: Vec<Polygon>,
    /// Isobands per height (km), lowest first; `None` where nothing reaches
    /// [`LIFTED_MIN_DBZ`].
    pub levels: Vec<(f32, Option<Vec<Polygon>>)>,
    /// One byte per coverage cell, north row first: 1 where a radar sees.
    pub coverage: Vec<u8>,
    pub coverage_size: (u32, u32),
}

impl Volume {
    pub fn info(&self) -> VolumeInfo {
        VolumeInfo {
            source: self.source.to_owned(),
            time: self.time,
            bounds: self.bounds,
            levels: self
                .levels
                .iter()
                .filter(|(_, shapes)| shapes.is_some())
                .map(|(km, _)| *km)
                .collect(),
            coverage_width: self.coverage_size.0,
            coverage_height: self.coverage_size.1,
        }
    }

    /// The isobands of each height [`Volume::info`] names, in that order.
    pub fn drawn_levels(&self) -> impl Iterator<Item = (f32, &[Polygon])> {
        self.levels
            .iter()
            .filter_map(|(km, shapes)| shapes.as_deref().map(|s| (*km, s)))
    }

    /// The isobands of the height `km`, if it has any.
    pub fn level(&self, km: f32) -> Option<&[Polygon]> {
        self.levels
            .iter()
            .find(|(k, _)| (*k - km).abs() < 0.01)
            .and_then(|(_, shapes)| shapes.as_deref())
    }

    /// `ground`, `coverage`, or a height in km such as `3`, as today's
    /// frontend reads it (`radar_image`): the isobands [`encode`]d, or the mask.
    pub fn image(&self, name: &str) -> Option<Vec<u8>> {
        match name {
            "ground" => Some(encode(&self.ground)),
            "coverage" => Some(self.coverage.clone()),
            km => self.level(km.parse().ok()?).map(encode),
        }
    }
}

#[cfg(test)]
impl Volume {
    /// Polygon counts and sizes of each layer, for checking the contouring.
    pub fn print_layers(&self) {
        let layers = std::iter::once(("ground".to_string(), Some(&self.ground))).chain(
            self.levels
                .iter()
                .map(|(km, s)| (format!("{km} km"), s.as_ref())),
        );
        for (name, shapes) in layers {
            match shapes {
                Some(s) => println!("{name:>8}: {} polygons, {} KB", s.len(), encode(s).len() / 1024),
                None => println!("{name:>8}: nothing"),
            }
        }
    }
}

/// A volume as the test fixtures record it (`cargo xtask fixtures`, item X1):
/// in `dir`, `volume.json` ([`VolumeInfo`]) and the blobs [`Volume::image`]
/// serves, `ground.bin`, `coverage.bin` and `<km>.bin` for each height it
/// names. Blocking file I/O. A source this backend does not know is refused.
pub fn read(dir: &Path) -> Result<Volume, Error> {
    let file = |name: &str| {
        std::fs::read(dir.join(name)).map_err(|e| -> Error { format!("{}: {e}", dir.join(name).display()).into() })
    };
    let info: VolumeInfo = serde_json::from_slice(&file("volume.json")?)?;
    let source = crate::RADAR_SOURCES
        .iter()
        .map(|(source, _)| *source)
        .find(|s| *s == info.source)
        .ok_or_else(|| format!("no radar source {:?}", info.source))?;
    let coverage = file("coverage.bin")?;
    if coverage.len() as u64 != u64::from(info.coverage_width) * u64::from(info.coverage_height) {
        return Err(format!("{source}'s coverage is not {}x{}", info.coverage_width, info.coverage_height).into());
    }
    let levels = info
        .levels
        .iter()
        .map(|&km| Ok((km, Some(decode(&file(&format!("{km}.bin"))?)?))))
        .collect::<Result<_, Error>>()?;
    Ok(Volume {
        source,
        time: info.time,
        fetched: Instant::now(),
        bounds: info.bounds,
        ground: decode(&file("ground.bin")?)?,
        levels,
        coverage,
        coverage_size: (info.coverage_width, info.coverage_height),
    })
}

impl crate::Backend {
    /// The newest 3D radar volumes for the sources visible in `area` (or
    /// everywhere, when it is `None`), whole: what [`crate::Backend::radar_frame`]
    /// describes, fetched as it fetches them, with each layer's isobands as
    /// [`Polygon`]s instead of `radar_image`'s blobs. The volumes are shared
    /// with the backend's cache, not copied.
    pub async fn radar_volumes(&self, area: Option<BoundingBox>) -> crate::CmdResult<Vec<Arc<Volume>>> {
        let frames = self.radar_frame(area).await?;
        let held = self.inner.radar.lock().await;
        // (a refresh that landed since is as good: it is newer)
        Ok(frames
            .iter()
            .filter_map(|frame| held.get(frame.source.as_str()).cloned())
            .collect())
    }
}

/// The usual NWS reflectivity colours, in 5 dBZ steps from 5 dBZ. Isoband
/// `i` covers `[5 + 5i, 10 + 5i)` dBZ and is drawn in `PALETTE[i]`; the
/// frontend keeps its own copy of these colours.
const PALETTE: [[u8; 3]; 14] = [
    [0x04, 0xe9, 0xe7],
    [0x01, 0x9f, 0xf4],
    [0x03, 0x00, 0xf4],
    [0x02, 0xfd, 0x02],
    [0x01, 0xc5, 0x01],
    [0x00, 0x8e, 0x00],
    [0xfd, 0xf8, 0x02],
    [0xe5, 0xbc, 0x00],
    [0xfd, 0x95, 0x00],
    [0xfd, 0x00, 0x00],
    [0xd4, 0x00, 0x00],
    [0xbc, 0x00, 0x00],
    [0xf8, 0x00, 0xfd],
    [0x98, 0x54, 0xc6],
];

/// The colour of a reflectivity, as the frontend draws its isoband.
#[cfg_attr(not(test), allow(dead_code))]
pub fn color(dbz: f32) -> [u8; 4] {
    let [r, g, b] = PALETTE[band_of(dbz)];
    // light echoes are translucent, so the map shows through drizzle
    let alpha = (150.0 + (dbz - 5.0) * 4.0).clamp(150.0, 235.0) as u8;
    [r, g, b, alpha]
}

/// Lower edge of the first isoband, and the width of each.
const FIRST_BAND_DBZ: f32 = 5.0;
const BAND_DBZ: f32 = 5.0;

/// The isoband (and [`PALETTE`] index) a reflectivity falls in.
fn band_of(dbz: f32) -> usize {
    (((dbz - FIRST_BAND_DBZ) / BAND_DBZ).floor().max(0.0) as usize).min(PALETTE.len() - 1)
}

/// How far below the weakest drawn band no-data and weaker echoes are
/// flattened before blurring. Close enough that the blur barely pulls storm
/// edges inward, far enough that the lowest contour still has a gradient.
const FLOOR_BELOW_DBZ: f32 = 10.0;
/// Douglas–Peucker tolerance, in grid cells.
const SIMPLIFY_CELLS: f32 = 0.4;
/// Rings enclosing less than this many cells are dropped as speckle.
const MIN_AREA_CELLS: f64 = 2.0;
/// Side of the buckets holes look their enclosing ring up in, in cells.
const BUCKET_CELLS: usize = 32;

/// Filled isobands of a dBZ grid in longitude and latitude, or `None` when
/// no band at or above `min_dbz` survives.
///
/// `values` is `w`×`h`, north row first, on plain latitude and longitude
/// spanning `bounds` (each value the centre of its cell). Anything not finite
/// counts as no echo. Bands start at the one holding `min_dbz` and are
/// disjoint: each has holes punched where the next stronger band sits, so
/// translucent fills never overlap. Rings run as [`Polygon`] says.
pub fn contour(
    w: usize,
    h: usize,
    values: &[f32],
    bounds: Bounds,
    min_dbz: f32,
) -> Option<Vec<Polygon>> {
    let shapes = isobands(w, h, values, min_dbz);
    if shapes.is_empty() {
        return None;
    }
    let sx = (bounds.east - bounds.west) / w as f64;
    let sy = (bounds.north - bounds.south) / h as f64;
    let to_lonlat = |[x, y]: [f32; 2]| {
        [
            (bounds.west + f64::from(x) * sx) as f32,
            (bounds.south + f64::from(y) * sy) as f32,
        ]
    };
    Some(
        shapes
            .into_iter()
            .map(|shape| Polygon {
                band: shape.band,
                rings: shape
                    .rings
                    .into_iter()
                    .map(|ring| ring.into_iter().map(to_lonlat).collect())
                    .collect(),
            })
            .collect(),
    )
}

/// A layer with nothing to draw.
pub fn no_shapes() -> Vec<Polygon> {
    Vec::new()
}

/// Isobands as the blob today's frontend reads (`radar_image`,
/// `parseVolumePolygons`), little-endian:
///
/// ```text
/// u32 polygon_count
/// per polygon: u8 band (PALETTE index), u8 reserved = 0, u16 reserved = 0,
///              u32 ring_count (8 bytes, so the f32s stay 4-byte aligned)
///   per ring:  u32 point_count, then point_count × (f32 lon, f32 lat)
/// ```
///
/// Rings as in [`Polygon`]. No polygons is the count 0 alone.
pub fn encode(polygons: &[Polygon]) -> Vec<u8> {
    let points: usize = polygons
        .iter()
        .flat_map(|s| &s.rings)
        .map(|r| 4 + r.len() * 8)
        .sum();
    let mut out = Vec::with_capacity(4 + polygons.len() * 8 + points);
    out.extend_from_slice(&(polygons.len() as u32).to_le_bytes());
    for polygon in polygons {
        out.push(polygon.band);
        out.push(0);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(polygon.rings.len() as u32).to_le_bytes());
        for ring in &polygon.rings {
            out.extend_from_slice(&(ring.len() as u32).to_le_bytes());
            for &[lon, lat] in ring {
                out.extend_from_slice(&lon.to_le_bytes());
                out.extend_from_slice(&lat.to_le_bytes());
            }
        }
    }
    out
}

/// The polygons of an [`encode`]d blob, exactly as they went in. An error
/// for a blob that ends before its counts say, or carries bytes past them.
pub fn decode(blob: &[u8]) -> Result<Vec<Polygon>, Error> {
    /// The next `n` bytes of `blob` from `at`, moving `at` past them.
    fn take<'a>(blob: &'a [u8], at: &mut usize, n: usize) -> Result<&'a [u8], Error> {
        let bytes = at
            .checked_add(n)
            .and_then(|end| blob.get(*at..end))
            .ok_or_else(|| format!("radar shapes end at byte {}, before {at} + {n}", blob.len()))?;
        *at += n;
        Ok(bytes)
    }
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut at = 0;
    let count = word(take(blob, &mut at, 4)?);
    // every polygon takes 8 bytes at least: a count past that is no blob of ours
    let mut polygons = Vec::with_capacity((count as usize).min(blob.len() / 8));
    for _ in 0..count {
        let head = take(blob, &mut at, 8)?;
        let (band, rings) = (head[0], word(&head[4..8]));
        let mut out = Vec::with_capacity((rings as usize).min(blob.len() / 4));
        for _ in 0..rings {
            let points = word(take(blob, &mut at, 4)?) as usize;
            let n = points.checked_mul(8).ok_or("radar shapes: too many points")?;
            let bytes = take(blob, &mut at, n)?;
            out.push(
                bytes
                    .chunks_exact(8)
                    .map(|p| {
                        [
                            f32::from_le_bytes([p[0], p[1], p[2], p[3]]),
                            f32::from_le_bytes([p[4], p[5], p[6], p[7]]),
                        ]
                    })
                    .collect(),
            );
        }
        polygons.push(Polygon { band, rings: out });
    }
    if at != blob.len() {
        return Err(format!("radar shapes: {} bytes past the last polygon", blob.len() - at).into());
    }
    Ok(polygons)
}

/// One isoband polygon in grid coordinates: x eastward and y northward, in
/// cells from the grid's south-west corner.
struct Shape {
    band: u8,
    /// Outer ring first (counter-clockwise), then holes (clockwise).
    rings: Vec<Vec<[f32; 2]>>,
}

/// A closed contour ring in grid coordinates (see [`Shape`]).
struct Ring {
    points: Vec<[f32; 2]>,
    /// Signed area in cells, positive counter-clockwise: the region at or
    /// above the threshold lies on the left, so positive rings are outer.
    area: f64,
    min: [f32; 2],
    max: [f32; 2],
}

impl Ring {
    /// Cleans up one contour ring, given in the contour crate's coordinates
    /// (y counts rows down from the north edge), or drops it as speckle.
    fn new(coords: &[contour::Pt], h: usize) -> Option<Ring> {
        let mut points: Vec<[f32; 2]> = coords.iter().map(|c| [c.x, h as f32 - c.y]).collect();
        points.dedup();
        if points.len() > 1 && points.first() == points.last() {
            points.pop();
        }
        if points.len() < 3 {
            return None;
        }
        let area = signed_area(&points);
        if area.abs() < MIN_AREA_CELLS {
            return None;
        }
        let points = simplify(&points, SIMPLIFY_CELLS);
        // a sliver can collapse or turn inside out; it was too thin to see
        if points.len() < 3 || signed_area(&points).signum() != area.signum() {
            return None;
        }
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for p in &points {
            for k in 0..2 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        Some(Ring {
            points,
            area,
            min,
            max,
        })
    }

    fn holds(&self, p: [f32; 2]) -> bool {
        p[0] >= self.min[0]
            && p[0] <= self.max[0]
            && p[1] >= self.min[1]
            && p[1] <= self.max[1]
            && point_in_ring(&self.points, p)
    }
}

/// A ring as one band sees it: the stronger band's rings bound this one from
/// the other side, so they are walked backwards.
#[derive(Clone, Copy)]
struct Side<'a> {
    ring: &'a Ring,
    flip: bool,
}

impl Side<'_> {
    fn area(&self) -> f64 {
        if self.flip {
            -self.ring.area
        } else {
            self.ring.area
        }
    }

    fn points(&self) -> Vec<[f32; 2]> {
        let mut points = self.ring.points.clone();
        if self.flip {
            points.reverse();
        }
        points
    }
}

fn isobands(w: usize, h: usize, values: &[f32], min_dbz: f32) -> Vec<Shape> {
    if w < 2 || h < 2 || values.len() != w * h || !values.iter().any(|&v| v >= min_dbz) {
        return Vec::new();
    }
    let first = band_of(min_dbz);
    let thresholds: Vec<f32> = (first..PALETTE.len())
        .map(|k| FIRST_BAND_DBZ + k as f32 * BAND_DBZ)
        .collect();
    let field = smooth_field(w, h, values, min_dbz - FLOOR_BELOW_DBZ);
    // linear interpolation along cell edges is what makes the curves smooth
    let Ok(lines) = ContourBuilder::new(w, h, true).lines(&field, &thresholds) else {
        return Vec::new();
    };
    drop(field);
    // each threshold's rings are cleaned once and shared by the band below
    // and the band above, so neighbouring bands meet exactly
    let rings: Vec<Vec<Ring>> = lines
        .into_iter()
        .map(|line| {
            line.into_inner()
                .0
                .0
                .iter()
                .filter_map(|ls| Ring::new(&ls.0, h))
                .collect()
        })
        .collect();
    let mut shapes = Vec::new();
    for (i, band) in (first..PALETTE.len()).enumerate() {
        let upper = rings.get(i + 1).map_or(&[][..], |r| &r[..]);
        assemble(band as u8, &rings[i], upper, w, h, &mut shapes);
    }
    shapes
}

/// Builds one band's polygons: the region at or above its own threshold
/// (`lower`) minus the region at or above the next (`upper`).
fn assemble(band: u8, lower: &[Ring], upper: &[Ring], w: usize, h: usize, out: &mut Vec<Shape>) {
    let sides = lower
        .iter()
        .map(|ring| Side { ring, flip: false })
        .chain(upper.iter().map(|ring| Side { ring, flip: true }));
    let (mut outers, holes): (Vec<Side>, Vec<Side>) = sides.partition(|s| s.area() > 0.0);
    if outers.is_empty() {
        return;
    }
    // rings never cross, so a hole belongs to the smallest outer ring around it
    outers.sort_by(|a, b| a.area().total_cmp(&b.area()));
    let (bw, bh) = (w.div_ceil(BUCKET_CELLS) + 1, h.div_ceil(BUCKET_CELLS) + 1);
    let bucket = |x: f32, y: f32| {
        let bx = ((x.max(0.0) as usize) / BUCKET_CELLS).min(bw - 1);
        let by = ((y.max(0.0) as usize) / BUCKET_CELLS).min(bh - 1);
        (bx, by)
    };
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); bw * bh];
    for (i, outer) in outers.iter().enumerate() {
        let (x0, y0) = bucket(outer.ring.min[0], outer.ring.min[1]);
        let (x1, y1) = bucket(outer.ring.max[0], outer.ring.max[1]);
        for by in y0..=y1 {
            for bx in x0..=x1 {
                buckets[by * bw + bx].push(i as u32);
            }
        }
    }
    let mut owned: Vec<Vec<Side>> = vec![Vec::new(); outers.len()];
    for hole in holes {
        let p = hole.ring.points[0];
        let (bx, by) = bucket(p[0], p[1]);
        if let Some(&i) = buckets[by * bw + bx]
            .iter()
            .find(|&&i| outers[i as usize].ring.holds(p))
        {
            owned[i as usize].push(hole);
        }
    }
    for (outer, holes) in outers.into_iter().zip(owned) {
        let mut rings = Vec::with_capacity(1 + holes.len());
        rings.push(outer.points());
        rings.extend(holes.iter().map(Side::points));
        out.push(Shape { band, rings });
    }
}

/// Flattens no-data and weak echoes to `floor`, then blurs with a 3×3
/// Gaussian so block-max staircases contour into curves.
fn smooth_field(w: usize, h: usize, values: &[f32], floor: f32) -> Vec<f32> {
    // `>` is false for NaN as well as for -inf
    let clean = |v: f32| if v > floor { v } else { floor };
    let mut across = vec![0.0f32; w * h];
    for (src, dst) in values.chunks_exact(w).zip(across.chunks_exact_mut(w)) {
        for x in 0..w {
            let l = clean(src[x.saturating_sub(1)]);
            let r = clean(src[(x + 1).min(w - 1)]);
            dst[x] = (l + 2.0 * clean(src[x]) + r) * 0.25;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let up = &across[y.saturating_sub(1) * w..][..w];
        let mid = &across[y * w..][..w];
        let down = &across[(y + 1).min(h - 1) * w..][..w];
        let dst = &mut out[y * w..][..w];
        for x in 0..w {
            dst[x] = (up[x] + 2.0 * mid[x] + down[x]) * 0.25;
        }
    }
    out
}

/// Shoelace area, positive counter-clockwise.
fn signed_area(points: &[[f32; 2]]) -> f64 {
    let mut sum = 0.0;
    let mut prev = points[points.len() - 1];
    for &p in points {
        sum += f64::from(prev[0]) * f64::from(p[1]) - f64::from(p[0]) * f64::from(prev[1]);
        prev = p;
    }
    sum / 2.0
}

/// Even-odd ray cast.
fn point_in_ring(ring: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Squared distance from `p` to the segment `a`–`b`.
fn segment_distance2(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ey) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    ex * ex + ey * ey
}

/// Douglas–Peucker for a closed ring: split at the point farthest from the
/// first, then simplify both halves as open paths.
fn simplify(points: &[[f32; 2]], epsilon: f32) -> Vec<[f32; 2]> {
    let n = points.len();
    if n <= 4 {
        return points.to_vec();
    }
    let at = |i: usize| points[i % n];
    let far = (1..n)
        .max_by(|&a, &b| {
            segment_distance2(at(a), at(0), at(0)).total_cmp(&segment_distance2(
                at(b),
                at(0),
                at(0),
            ))
        })
        .unwrap_or(n / 2);
    let mut keep = vec![false; n + 1];
    keep[0] = true;
    keep[far] = true;
    let mut stack = vec![(0, far), (far, n)];
    let limit = epsilon * epsilon;
    while let Some((a, b)) = stack.pop() {
        let (mut worst, mut at_worst) = (limit, None);
        for i in a + 1..b {
            let d = segment_distance2(at(i), at(a), at(b));
            if d > worst {
                worst = d;
                at_worst = Some(i);
            }
        }
        if let Some(i) = at_worst {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    (0..n).filter(|&i| keep[i]).map(|i| points[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An isoband as the blob today's frontend reads carries it: band and rings.
    type Shape = (u8, Vec<Vec<[f32; 2]>>);

    /// Reads a blob back, checking it is consumed exactly.
    fn parse(blob: &[u8]) -> Vec<Shape> {
        let mut at = 0;
        let mut take = |n: usize| {
            let s = &blob[at..at + n];
            at += n;
            s
        };
        let u32_at = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap());
        let count = u32_at(take(4));
        let mut out = Vec::new();
        for _ in 0..count {
            let head = take(8);
            let band = head[0];
            assert_eq!(&head[1..4], &[0, 0, 0], "reserved bytes");
            let ring_count = u32_at(&head[4..8]);
            let mut rings = Vec::new();
            for _ in 0..ring_count {
                let n = u32_at(take(4)) as usize;
                let ring = (0..n)
                    .map(|_| {
                        let p = take(8);
                        [
                            f32::from_le_bytes(p[0..4].try_into().unwrap()),
                            f32::from_le_bytes(p[4..8].try_into().unwrap()),
                        ]
                    })
                    .collect();
                rings.push(ring);
            }
            out.push((band, rings));
        }
        assert_eq!(at, blob.len(), "trailing bytes");
        out
    }

    /// The isobands as the frontend would read them: through the blob.
    fn wire(polygons: &[Polygon]) -> Vec<Shape> {
        parse(&encode(polygons))
    }

    const BOX: Bounds = Bounds {
        west: 0.0,
        south: 50.0,
        east: 8.0,
        north: 58.0,
    };

    /// An 80×80 grid at 0.1° with a cone-shaped storm: 62 dBZ at the centre,
    /// falling 1.5 dBZ per cell, so the 5 dBZ edge sits 38 cells out.
    fn storm() -> Vec<f32> {
        let mut grid = vec![f32::NEG_INFINITY; 80 * 80];
        for row in 0..80 {
            for col in 0..80 {
                let (dx, dy) = (col as f32 + 0.5 - 40.0, row as f32 + 0.5 - 40.0);
                let v = 62.0 - 1.5 * (dx * dx + dy * dy).sqrt();
                if v > 0.0 {
                    grid[row * 80 + col] = v;
                }
            }
        }
        grid
    }

    fn inside(polygon: &Shape, p: [f32; 2]) -> bool {
        point_in_ring(&polygon.1[0], p) && !polygon.1[1..].iter().any(|h| point_in_ring(h, p))
    }

    #[test]
    fn palette_steps() {
        assert_eq!(color(5.0)[..3], PALETTE[0]);
        assert_eq!(color(52.0)[..3], PALETTE[9]);
        assert_eq!(color(90.0)[..3], PALETTE[13]);
    }

    #[test]
    fn empty_levels_contour_to_nothing() {
        let quiet = vec![-99.0f32; 16];
        assert!(contour(4, 4, &quiet, BOX, LIFTED_MIN_DBZ).is_none());
        let unseen = vec![f32::NEG_INFINITY; 16];
        assert!(contour(4, 4, &unseen, BOX, GROUND_MIN_DBZ).is_none());
        assert_eq!(parse(&encode(&no_shapes())).len(), 0);
        assert_eq!(encode(&no_shapes()), 0u32.to_le_bytes());
    }

    #[test]
    fn a_round_storm_makes_nested_disjoint_bands() {
        let polygons = wire(&contour(80, 80, &storm(), BOX, GROUND_MIN_DBZ).unwrap());
        let bands: Vec<u8> = polygons.iter().map(|p| p.0).collect();
        // one ring of each band from 5 dBZ up to the core
        let top = *bands.iter().max().unwrap();
        assert!(top >= 9, "core band {top}");
        for band in 0..=top {
            assert_eq!(bands.iter().filter(|&&b| b == band).count(), 1, "{bands:?}");
        }
        let centre = [4.0, 54.0];
        for (band, rings) in &polygons {
            // orientation: outer counter-clockwise, holes clockwise
            assert!(signed_area(&rings[0]) > 0.0, "band {band} outer");
            for hole in &rings[1..] {
                assert!(signed_area(hole) < 0.0, "band {band} hole");
            }
            // every band but the core has the next one punched out of it
            assert_eq!(rings.len(), if *band == top { 1 } else { 2 }, "band {band}");
            // and its outline sits where the cone crosses its threshold
            let r = rings[0]
                .iter()
                .map(|p| ((p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2)).sqrt())
                .sum::<f32>()
                / rings[0].len() as f32;
            let expected = (62.0 - (5.0 + 5.0 * f32::from(*band))) / 1.5 * 0.1;
            assert!(
                (r - expected).abs() < 0.12,
                "band {band}: radius {r} vs {expected}"
            );
            // implicitly closed
            assert_ne!(rings[0].first(), rings[0].last());
        }
        // a band's hole is exactly the next band's outline, walked backwards
        for pair in polygons.windows(2) {
            let (lo, hi) = if pair[0].0 < pair[1].0 {
                (&pair[0], &pair[1])
            } else {
                (&pair[1], &pair[0])
            };
            if hi.0 == lo.0 + 1 {
                let mut hole = lo.1[1].clone();
                hole.reverse();
                assert_eq!(hole, hi.1[0]);
            }
        }
        // no point is covered twice
        for i in 0..400 {
            let p = [0.1 + (i % 20) as f32 * 0.4, 50.1 + (i / 20) as f32 * 0.4];
            let hits = polygons.iter().filter(|poly| inside(poly, p)).count();
            assert!(hits <= 1, "{p:?} is in {hits} bands");
        }
        assert_eq!(
            polygons.iter().filter(|poly| inside(poly, centre)).count(),
            1
        );
    }

    #[test]
    fn lifted_layers_start_at_their_minimum() {
        let polygons = wire(&contour(80, 80, &storm(), BOX, LIFTED_MIN_DBZ).unwrap());
        assert!(
            polygons
                .iter()
                .all(|p| usize::from(p.0) >= band_of(LIFTED_MIN_DBZ))
        );
        assert!(
            polygons
                .iter()
                .any(|p| usize::from(p.0) == band_of(LIFTED_MIN_DBZ))
        );
    }

    /// Single-cell speckle is too small to keep.
    #[test]
    fn speckle_is_dropped() {
        let mut grid = vec![-99.0f32; 20 * 20];
        grid[10 * 20 + 10] = 40.0;
        grid[3 * 20 + 3] = f32::NAN;
        assert!(contour(20, 20, &grid, BOX, GROUND_MIN_DBZ).is_none());
    }

    /// Light rain dotted with hundreds of showers keeps every hole.
    #[test]
    fn hundreds_of_holes_survive() {
        let (w, h) = (200, 200);
        let mut grid = vec![7.0f32; w * h];
        for row in (5..h - 5).step_by(10) {
            for col in (5..w - 5).step_by(10) {
                for (dr, dc) in [
                    (0, 0),
                    (0, 1),
                    (1, 0),
                    (1, 1),
                    (0, 2),
                    (2, 0),
                    (2, 2),
                    (1, 2),
                    (2, 1),
                ] {
                    grid[(row + dr) * w + col + dc] = 25.0;
                }
            }
        }
        let polygons = wire(&contour(w, h, &grid, BOX, GROUND_MIN_DBZ).unwrap());
        let light: Vec<_> = polygons.iter().filter(|p| p.0 == 0).collect();
        assert_eq!(light.len(), 1);
        // 19 × 19 showers, each punched out of the light rain
        assert_eq!(light[0].1.len(), 1 + 19 * 19);
    }

    /// The structs are what the blob carries, value for value, and come back
    /// out of it unchanged.
    #[test]
    fn polygons_are_the_blob_s_shapes() {
        let polygons = contour(80, 80, &storm(), BOX, GROUND_MIN_DBZ).unwrap();
        let shapes: Vec<Shape> = polygons.iter().map(|p| (p.band, p.rings.clone())).collect();
        assert_eq!(wire(&polygons), shapes);
        assert_eq!(decode(&encode(&polygons)).unwrap(), polygons);
        assert_eq!(decode(&encode(&no_shapes())).unwrap(), no_shapes());
    }

    /// A blob that ends early, or runs on past its last polygon, is refused.
    #[test]
    fn broken_blobs_are_refused() {
        let blob = encode(&contour(80, 80, &storm(), BOX, GROUND_MIN_DBZ).unwrap());
        for end in [0, 3, 4 + 7, blob.len() / 2, blob.len() - 1] {
            assert!(decode(&blob[..end]).is_err(), "cut at {end}");
        }
        let mut longer = blob.clone();
        longer.push(0);
        assert!(decode(&longer).is_err());
        // a count no blob of this size could hold
        let mut huge = u32::MAX.to_le_bytes().to_vec();
        huge.extend_from_slice(&[1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        assert!(decode(&huge).is_err());
    }

    fn recorded() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../map/tests/data/radar/mrms")
    }

    /// The committed MRMS volume (X1) reads back into the volume the backend
    /// had, and serves its blobs byte for byte as they were recorded.
    #[test]
    fn a_recorded_volume_reads_back() {
        let dir = recorded();
        let volume = read(&dir).unwrap();
        let info: VolumeInfo =
            serde_json::from_slice(&std::fs::read(dir.join("volume.json")).unwrap()).unwrap();
        assert_eq!(volume.info(), info);
        assert_eq!((volume.source, volume.bounds), ("mrms", crate::mrms::BOUNDS));
        assert_eq!(volume.coverage_size, (700, 350));
        assert!(volume.coverage.contains(&1) && volume.coverage.contains(&0));
        assert_eq!(volume.drawn_levels().map(|(km, _)| km).collect::<Vec<_>>(), crate::mrms::LEVELS_KM);
        for name in ["ground".to_owned(), "coverage".to_owned()]
            .into_iter()
            .chain(info.levels.iter().map(|km| km.to_string()))
        {
            let file = std::fs::read(dir.join(format!("{name}.bin"))).unwrap();
            assert_eq!(volume.image(&name).unwrap(), file, "{name}");
        }
        assert!(volume.image("13").is_none() && volume.image("radar").is_none());
        // the shapes are the contouring's: every band in the palette, rings of
        // three points at least, in the source's bounds
        let b = volume.bounds;
        for polygon in volume.ground.iter().chain(volume.drawn_levels().flat_map(|(_, s)| s)) {
            assert!(usize::from(polygon.band) < PALETTE.len());
            for ring in &polygon.rings {
                assert!(ring.len() >= 3);
                for &[lon, lat] in ring {
                    let (lon, lat) = (f64::from(lon), f64::from(lat));
                    assert!(lon >= b.west - 1e-3 && lon <= b.east + 1e-3 && lat >= b.south - 1e-3 && lat <= b.north + 1e-3);
                }
            }
        }
        // aloft only moderate rain and stronger
        let lifted = band_of(LIFTED_MIN_DBZ) as u8;
        assert!(volume.drawn_levels().all(|(_, s)| s.iter().all(|p| p.band >= lifted)));
        assert_eq!(volume.level(2.0), volume.drawn_levels().nth(1).map(|(_, s)| s));
    }

    #[test]
    fn a_recorded_volume_needs_its_files() {
        let temp = std::env::temp_dir().join(format!("rustradar-radar-recorded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        for file in ["volume.json", "ground.bin", "coverage.bin"] {
            std::fs::copy(recorded().join(file), temp.join(file)).unwrap();
        }
        // a height it names is missing
        assert!(read(&temp).is_err());
        let json = std::fs::read_to_string(recorded().join("volume.json")).unwrap();
        let no_heights = json.replace(
            &json[json.find("\"levels\"").unwrap()..json.find("\"coverageWidth\"").unwrap()],
            "\"levels\": [],\n  ",
        );
        std::fs::write(temp.join("volume.json"), &no_heights).unwrap();
        let volume = read(&temp).unwrap();
        assert!(volume.levels.is_empty() && !volume.ground.is_empty());
        // nor a source the backend does not know, or a mask of the wrong size
        std::fs::write(temp.join("volume.json"), no_heights.replace("\"mrms\"", "\"nexrad\"")).unwrap();
        assert!(read(&temp).is_err());
        std::fs::write(temp.join("volume.json"), no_heights.replace("700", "701")).unwrap();
        assert!(read(&temp).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// The native app's ask hands over the volumes held, shared and whole,
    /// for the sources in view; a fresh one is not fetched again.
    #[test]
    fn volumes_are_handed_over_as_held() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let root = std::env::temp_dir().join(format!("rustradar-radar-volumes-{}", std::process::id()));
        let dirs = crate::Dirs { cache: root.join("cache"), config: root.join("config"), downloads: None };
        let (tx, _rx) = std::sync::mpsc::channel();
        let backend = crate::Backend::with(fr24::Fr24::new().unwrap(), dirs, rt.handle().clone(), crate::Notifier::new(tx, Arc::new(|| {})));
        let volume = Arc::new(read(&recorded()).unwrap());
        rt.block_on(async {
            backend.inner.radar.lock().await.insert("mrms", Arc::clone(&volume));
            // the plains: MRMS, held and fresh, so nothing goes out
            let plains = BoundingBox { north: 45.0, south: 35.0, west: -105.0, east: -95.0 };
            let given = backend.radar_volumes(Some(plains)).await.unwrap();
            assert_eq!(given.len(), 1);
            assert!(Arc::ptr_eq(&given[0], &volume));
            assert_eq!(backend.radar_frame(Some(plains)).await.unwrap(), vec![volume.info()]);
            assert_eq!(backend.radar_image("mrms", "ground").await.unwrap(), encode(&volume.ground));
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// DWD's composite can no longer be fetched (its folder holds ODIM HDF5
    /// now, X1), so its volume is built here the way `dwd::fetch` builds one
    /// from a decoded grid: the ground, then each height keeping the echoes
    /// whose estimated top reaches it. It is handed over for a view of
    /// central Europe.
    #[test]
    fn a_dwd_volume_built_from_a_grid_is_handed_over_for_europe() {
        let bounds = crate::RADAR_SOURCES[1].1;
        let (w, h) = (80, 80);
        let grid = storm();
        let echo_top = |dbz: f32| (2.0 + (dbz - 10.0) * 0.22).clamp(1.0, 12.0);
        let levels: Vec<(f32, Option<Vec<Polygon>>)> = [1.5f32, 3.0, 4.5, 6.0, 8.0, 10.0, 12.0]
            .into_iter()
            .map(|km| {
                let aloft: Vec<f32> = grid
                    .iter()
                    .map(|&v| if v.is_finite() && echo_top(v) >= km { v } else { f32::NEG_INFINITY })
                    .collect();
                (km, contour(w, h, &aloft, bounds, LIFTED_MIN_DBZ))
            })
            .collect();
        let volume = Arc::new(Volume {
            source: "dwd",
            time: 1_794_446_400,
            fetched: Instant::now(),
            bounds,
            ground: contour(w, h, &grid, bounds, GROUND_MIN_DBZ).unwrap_or_else(no_shapes),
            levels,
            coverage: grid.iter().step_by(8).map(|v| u8::from(v.is_finite())).collect(),
            coverage_size: (10, 80),
        });
        // the storm's 62 dBZ core reaches 13 km by the fit (12 at most), its
        // 25 dBZ edge only 5.3: the stack narrows with height and every level
        // has something
        let info = volume.info();
        assert_eq!(info.levels, [1.5, 3.0, 4.5, 6.0, 8.0, 10.0, 12.0]);
        let widths: Vec<f32> = volume
            .drawn_levels()
            .map(|(_, shapes)| shapes.iter().flat_map(|p| &p.rings[0]).map(|p| p[0]).fold(f32::MIN, f32::max))
            .collect();
        assert!(widths.windows(2).all(|w| w[1] <= w[0]), "{widths:?}");
        assert!(volume.ground.len() > volume.level(12.0).unwrap().len());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let root = std::env::temp_dir().join(format!("rustradar-radar-dwd-{}", std::process::id()));
        let dirs = crate::Dirs { cache: root.join("cache"), config: root.join("config"), downloads: None };
        let (tx, _rx) = std::sync::mpsc::channel();
        let backend = crate::Backend::with(fr24::Fr24::new().unwrap(), dirs, rt.handle().clone(), crate::Notifier::new(tx, Arc::new(|| {})));
        rt.block_on(async {
            backend.inner.radar.lock().await.insert("dwd", Arc::clone(&volume));
            let europe = BoundingBox { north: 54.0, south: 47.0, west: 5.0, east: 15.0 };
            let given = backend.radar_volumes(Some(europe)).await.unwrap();
            assert_eq!(given.len(), 1);
            assert!(Arc::ptr_eq(&given[0], &volume));
            assert_eq!(decode(&backend.radar_image("dwd", "4.5").await.unwrap()).unwrap(), volume.level(4.5).unwrap());
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn simplification_keeps_corners_and_drops_noise() {
        let mut ring = Vec::new();
        for i in 0..40 {
            ring.push([i as f32 * 0.25, if i % 2 == 0 { 0.0 } else { 0.05 }]);
        }
        ring.push([10.0, 10.0]);
        ring.push([0.0, 10.0]);
        let simple = simplify(&ring, 0.4);
        assert!(simple.len() <= 5, "{simple:?}");
        assert!(simple.contains(&[10.0, 10.0]) && simple.contains(&[0.0, 10.0]));
    }
}
