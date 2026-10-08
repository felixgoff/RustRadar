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
//! keeps clean edges however far the globe zooms in. See [`contour`] for the
//! binary format.

use std::time::{Duration, Instant};

use contour::ContourBuilder;
use serde::Serialize;

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

#[derive(Clone, Copy, Serialize)]
pub struct Bounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// One source's current scan, reduced to what the globe draws.
pub struct Volume {
    /// Short name the frontend asks for layers by, e.g. `mrms`.
    pub source: &'static str,
    /// Unix seconds of the newest height's scan.
    pub time: i64,
    pub fetched: Instant,
    pub bounds: Bounds,
    /// Isoband shapes ([`contour`]) of the strongest echo at any height, for
    /// the ground; holds no polygons when nothing reaches [`GROUND_MIN_DBZ`].
    pub ground: Vec<u8>,
    /// Isoband shapes per height (km), lowest first; `None` where nothing
    /// reaches [`LIFTED_MIN_DBZ`].
    pub levels: Vec<(f32, Option<Vec<u8>>)>,
    /// One byte per coverage cell, north row first: 1 where a radar sees.
    pub coverage: Vec<u8>,
    pub coverage_size: (u32, u32),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub source: &'static str,
    pub time: i64,
    pub bounds: Bounds,
    /// Heights that have shapes, km.
    pub levels: Vec<f32>,
    pub coverage_width: u32,
    pub coverage_height: u32,
}

impl Volume {
    pub fn info(&self) -> VolumeInfo {
        VolumeInfo {
            source: self.source,
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

    /// `ground`, `coverage`, or a height in km such as `3`.
    pub fn image(&self, name: &str) -> Option<Vec<u8>> {
        match name {
            "ground" => Some(self.ground.clone()),
            "coverage" => Some(self.coverage.clone()),
            km => {
                let km: f32 = km.parse().ok()?;
                self.levels
                    .iter()
                    .find(|(k, _)| (*k - km).abs() < 0.01)
                    .and_then(|(_, shapes)| shapes.clone())
            }
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
        for (name, blob) in layers {
            match blob {
                Some(b) => {
                    let polygons = u32::from_le_bytes(b[..4].try_into().unwrap());
                    println!("{name:>8}: {polygons} polygons, {} KB", b.len() / 1024);
                }
                None => println!("{name:>8}: nothing"),
            }
        }
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

/// Filled isobands of a dBZ grid, encoded for the frontend, or `None` when
/// no band at or above `min_dbz` survives.
///
/// `values` is `w`×`h`, north row first, on plain latitude and longitude
/// spanning `bounds` (each value the centre of its cell). Anything not finite
/// counts as no echo. Bands start at the one holding `min_dbz` and are
/// disjoint: each has holes punched where the next stronger band sits, so
/// translucent fills never overlap.
///
/// The blob is little-endian:
///
/// ```text
/// u32 polygon_count
/// per polygon: u8 band (PALETTE index), u8 reserved = 0, u16 reserved = 0,
///              u32 ring_count (8 bytes, so the f32s stay 4-byte aligned)
///   per ring:  u32 point_count, then point_count × (f32 lon, f32 lat)
/// ```
///
/// Ring 0 is the outer ring and runs counter-clockwise in lon/lat; the rest
/// are holes and run clockwise. Rings are closed implicitly: the first point
/// is not repeated.
pub fn contour(
    w: usize,
    h: usize,
    values: &[f32],
    bounds: Bounds,
    min_dbz: f32,
) -> Option<Vec<u8>> {
    let shapes = isobands(w, h, values, min_dbz);
    if shapes.is_empty() {
        return None;
    }
    let sx = (bounds.east - bounds.west) / w as f64;
    let sy = (bounds.north - bounds.south) / h as f64;
    Some(encode_shapes(&shapes, |[x, y]| {
        [
            (bounds.west + f64::from(x) * sx) as f32,
            (bounds.south + f64::from(y) * sy) as f32,
        ]
    }))
}

/// A layer with nothing to draw.
pub fn no_shapes() -> Vec<u8> {
    0u32.to_le_bytes().to_vec()
}

/// One isoband polygon in grid coordinates: x eastward and y northward, in
/// cells from the grid's south-west corner.
struct Shape {
    band: u8,
    /// Outer ring first (counter-clockwise), then holes (clockwise).
    rings: Vec<Vec<[f32; 2]>>,
}

fn encode_shapes(shapes: &[Shape], to_lonlat: impl Fn([f32; 2]) -> [f32; 2]) -> Vec<u8> {
    let points: usize = shapes
        .iter()
        .flat_map(|s| &s.rings)
        .map(|r| 4 + r.len() * 8)
        .sum();
    let mut out = Vec::with_capacity(4 + shapes.len() * 4 + points);
    out.extend_from_slice(&(shapes.len() as u32).to_le_bytes());
    for shape in shapes {
        out.push(shape.band);
        out.push(0);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(shape.rings.len() as u32).to_le_bytes());
        for ring in &shape.rings {
            out.extend_from_slice(&(ring.len() as u32).to_le_bytes());
            for &p in ring {
                let [lon, lat] = to_lonlat(p);
                out.extend_from_slice(&lon.to_le_bytes());
                out.extend_from_slice(&lat.to_le_bytes());
            }
        }
    }
    out
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

    type Polygon = (u8, Vec<Vec<[f32; 2]>>);

    /// Reads a blob back, checking it is consumed exactly.
    fn parse(blob: &[u8]) -> Vec<Polygon> {
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

    fn inside(polygon: &Polygon, p: [f32; 2]) -> bool {
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
        assert_eq!(parse(&no_shapes()).len(), 0);
    }

    #[test]
    fn a_round_storm_makes_nested_disjoint_bands() {
        let blob = contour(80, 80, &storm(), BOX, GROUND_MIN_DBZ).unwrap();
        let polygons = parse(&blob);
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
        let blob = contour(80, 80, &storm(), BOX, LIFTED_MIN_DBZ).unwrap();
        let polygons = parse(&blob);
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
        let polygons = parse(&contour(w, h, &grid, BOX, GROUND_MIN_DBZ).unwrap());
        let light: Vec<_> = polygons.iter().filter(|p| p.0 == 0).collect();
        assert_eq!(light.len(), 1);
        // 19 × 19 showers, each punched out of the light rain
        assert_eq!(light[0].1.len(), 1 + 19 * 19);
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
