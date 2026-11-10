//! Port of `landcover-ground.ts` (S1.29; doc 04 §11): the realistic map's ground where the map's
//! data says nothing: what the land cover (landcover.rs) has there, woods, fields, mires, heath,
//! in the ground's own surfaces (ground.rs's [`land_cover_paint`]). Drawn over the grass that
//! lies everywhere and under every mapped area, so it shows only where the map leaves the land
//! unmapped.
//!
//! Each class of a tile's 10 m grid is traced into outlines ([`cover_outlines`]: d3-contour's
//! `contours()` at 0.5, its rings from [`IsoRings`], its holes matched to their polygons as d3
//! matches them), the grid softened first so they come out rounded rather than in steps.
//!
//! The tile layer (`landCoverGroundLayer`: tiles of zoom [`MIN_ZOOM`] to [`MAX_ZOOM`],
//! [`TILE_SIZE`] pixels, at most [`MAX_REQUESTS`] loading, each tile's areas filled with the
//! ground shader) is the map's; [`placed`] gives it a tile's areas in longitude and latitude.

use std::f64::consts::PI;

use crate::bridges::TileIndex;
use crate::ground::{Rgba, land_cover_paint};
use crate::landcover::{LandClass, LandGrid};
use crate::radar::iso::IsoRings;

/// A ring of an outline: closed (its last point repeats its first).
pub type Ring = Vec<[f64; 2]>;

/// An outline of one kind of ground, with its holes, and its paint (`CoverArea`).
#[derive(Debug, Clone, PartialEq)]
pub struct CoverArea {
    /// The outer ring, then its holes.
    pub polygon: Vec<Ring>,
    pub paint: Rgba,
}

/// Areas smaller than this many of the grid's cells are left out.
const MIN_CELLS: f64 = 6.0;

/// The layer's tiles: this zoom to [`MAX_ZOOM`] (finer than 14 is finer than the land cover
/// itself: those tiles stretched).
pub const MIN_ZOOM: u32 = 10;
pub const MAX_ZOOM: u32 = 14;
pub const TILE_SIZE: u32 = 256;
/// At most this many tiles loading at once.
pub const MAX_REQUESTS: usize = 4;

/// A mask's values averaged with their neighbours', `passes` times over (as a `Float32Array`
/// holds them; each sum is a double).
fn soften(mask: Vec<f32>, size: usize, passes: usize) -> Vec<f32> {
    let mut a = mask;
    let mut b = vec![0.0f32; a.len()];
    for _ in 0..passes {
        for y in 0..size {
            for x in 0..size {
                let mut sum = 0.0f64;
                let mut n = 0.0f64;
                for j in y.saturating_sub(1)..=(y + 1).min(size - 1) {
                    for i in x.saturating_sub(1)..=(x + 1).min(size - 1) {
                        sum += f64::from(a[j * size + i]);
                        n += 1.0;
                    }
                }
                b[y * size + x] = (sum / n) as f32;
            }
        }
        std::mem::swap(&mut a, &mut b);
    }
    a
}

/// A ring's area, unsigned (`ringArea`).
fn ring_area(r: &[[f64; 2]]) -> f64 {
    let mut s = 0.0;
    for (i, &[x, y]) in r.iter().enumerate() {
        let [nx, ny] = r[(i + 1) % r.len()];
        s = s + x * ny - nx * y;
    }
    s.abs() / 2.0
}

/// d3-contour's `area`: twice a ring's signed area, positive for its outer rings.
fn d3_area(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    let mut area = ring[n - 1][1] * ring[0][0] - ring[n - 1][0] * ring[0][1];
    for i in 1..n {
        area += ring[i - 1][1] * ring[i][0] - ring[i - 1][0] * ring[i][1];
    }
    area
}

/// d3-contour's `contains`: 1 if the hole is inside the ring, -1 if outside, 0 if all of it is on
/// the ring's edge (by its first point not on the edge).
fn d3_contains(ring: &[[f64; 2]], hole: &[[f64; 2]]) -> i32 {
    for &point in hole {
        let c = ring_contains(ring, point);
        if c != 0 {
            return c;
        }
    }
    0
}

fn ring_contains(ring: &[[f64; 2]], point: [f64; 2]) -> i32 {
    let [x, y] = point;
    let mut contains = -1;
    let mut j = ring.len().wrapping_sub(1);
    for i in 0..ring.len() {
        let (pi, pj) = (ring[i], ring[j]);
        let ([xi, yi], [xj, yj]) = (pi, pj);
        if segment_contains(pi, pj, point) {
            return 0;
        }
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            contains = -contains;
        }
        j = i;
    }
    contains
}

fn segment_contains(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    let collinear = (b[0] - a[0]) * (c[1] - a[1]) == (c[0] - a[0]) * (b[1] - a[1]);
    // along y if the segment is upright, else along x
    let i = usize::from(a[0] == b[0]);
    let (p, q, r) = (a[i], c[i], b[i]);
    collinear && ((p <= q && q <= r) || (r <= q && q <= p))
}

/// d3-contour's `contours().size([dx, dy]).thresholds([value])`'s one contour: polygons (outer
/// ring, then holes) in the grid's cells. Rings are outer by d3's sign of their area; each hole
/// goes to the first polygon whose outer ring holds it.
fn contour(values: &[f32], dx: usize, dy: usize, value: f64) -> Vec<Vec<Ring>> {
    let mut polygons: Vec<Vec<Ring>> = Vec::new();
    let mut holes: Vec<Ring> = Vec::new();
    IsoRings::new(dx, dy).trace(values, value, |flat| {
        let ring: Ring = flat.chunks_exact(2).map(|p| [p[0], p[1]]).collect();
        if d3_area(&ring) > 0.0 {
            polygons.push(vec![ring]);
        } else {
            holes.push(ring);
        }
    });
    for hole in holes {
        if let Some(polygon) = polygons.iter_mut().find(|polygon| d3_contains(&polygon[0], &hole) != -1) {
            polygon.push(hole);
        }
    }
    polygons
}

/// A tile's land cover as outlines of each kind of ground it paints, in the grid's own cells (0
/// to `size`, y down). Classes sharing a paint are traced as one, in the order their first
/// class first appears in the grid; towns and water (no paint) are left out.
pub fn cover_outlines(grid: &LandGrid) -> Vec<CoverArea> {
    let LandGrid { size, ref classes } = *grid;
    let mut out = Vec::new();
    // (a set in the order first met)
    let mut present: Vec<LandClass> = Vec::new();
    for &c in classes {
        if !present.contains(&c) {
            present.push(c);
        }
    }
    // classes sharing a paint traced as one (the paints are distinct values, as the TypeScript's
    // are distinct objects)
    let mut by_paint: Vec<(Rgba, Vec<LandClass>)> = Vec::new();
    for c in present {
        let Some(paint) = land_cover_paint(c) else { continue };
        match by_paint.iter_mut().find(|(p, _)| *p == paint) {
            Some((_, group)) => group.push(c),
            None => by_paint.push((paint, vec![c])),
        }
    }
    for (paint, group) in by_paint {
        let mut mask = vec![0.0f32; size * size];
        let mut count = 0.0;
        for (i, m) in mask.iter_mut().enumerate() {
            if classes.get(i).is_some_and(|c| group.contains(c)) {
                *m = 1.0;
                count += 1.0;
            }
        }
        if count < MIN_CELLS {
            continue;
        }
        for polygon in contour(&soften(mask, size, 2), size, size, 0.5) {
            if ring_area(&polygon[0]) < MIN_CELLS {
                continue;
            }
            out.push(CoverArea { polygon, paint });
        }
    }
    out
}

/// A tile's outlines ([`cover_outlines`] of its grid) in longitude and latitude.
pub fn placed(t: TileIndex, grid: &LandGrid) -> Vec<CoverArea> {
    let n = 2f64.powi(t.z as i32);
    let s = grid.size as f64;
    let lon_of = |x: f64| ((f64::from(t.x) + x / s) / n) * 360.0 - 180.0;
    let lat_of = |y: f64| ((PI * (1.0 - (2.0 * (f64::from(t.y) + y / s)) / n)).sinh().atan() * 180.0) / PI;
    cover_outlines(grid)
        .into_iter()
        .map(|CoverArea { polygon, paint }| CoverArea {
            polygon: polygon.into_iter().map(|ring| ring.into_iter().map(|[x, y]| [lon_of(x), lat_of(y)]).collect()).collect(),
            paint,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ground::area;
    use crate::landcover::lc;

    fn js_max_of(values: impl Iterator<Item = f64>) -> f64 {
        values.fold(f64::NEG_INFINITY, f64::max)
    }

    fn js_min_of(values: impl Iterator<Item = f64>) -> f64 {
        values.fold(f64::INFINITY, f64::min)
    }

    /// `landcover.test.ts`'s grid: a wood, a field and a town in three bands of columns, and a
    /// scrap of heath too small to keep.
    fn three_bands() -> LandGrid {
        let size = 32;
        let mut classes = vec![lc::NONE; size * size];
        for y in 0..size {
            for x in 0..size {
                classes[y * size + x] = if x < 12 {
                    lc::CONIFERS
                } else if x < 24 {
                    lc::ARABLE
                } else {
                    lc::BUILT
                };
            }
        }
        classes[5 * size + 5] = lc::HEATH;
        LandGrid { size, classes }
    }

    #[test]
    fn land_cover_traced_into_rounded_outlines_a_wood_and_a_field_towns_and_water_left_out() {
        let areas = cover_outlines(&three_bands());
        assert_eq!(areas.len(), 2);
        let xs: Vec<Vec<f64>> = areas.iter().map(|a| a.polygon[0].iter().map(|p| p[0]).collect()).collect();
        // the wood's outline about its 12 columns, the field's from there to 24
        assert!(js_max_of(xs[0].iter().copied()) > 10.0);
        assert!(js_max_of(xs[0].iter().copied()) < 14.0);
        assert!(js_min_of(xs[1].iter().copied()) > 10.0);
        assert!(js_max_of(xs[1].iter().copied()) < 26.0);
    }

    // New tests (no TypeScript twin).

    /// What the bun script printed of a grid's outlines: how many, the sum of `3x + y` over every
    /// point, the paints, each polygon's rings' lengths and their first points.
    type Digest = (usize, f64, Vec<Rgba>, Vec<Vec<usize>>, Vec<Vec<[f64; 2]>>);

    fn digest(areas: &[CoverArea]) -> Digest {
        let mut sum = 0.0;
        for a in areas {
            for r in &a.polygon {
                for &[x, y] in r {
                    sum += x * 3.0 + y;
                }
            }
        }
        (
            areas.len(),
            sum,
            areas.iter().map(|a| a.paint).collect(),
            areas.iter().map(|a| a.polygon.iter().map(Vec::len).collect()).collect(),
            areas.iter().map(|a| a.polygon.iter().map(|r| r[0]).collect()).collect(),
        )
    }

    /// A ring of mixed wood round a field round a broadleaved island, in water; felled wood in a
    /// corner and heath in another (the same paint), and a mire.
    fn nested() -> LandGrid {
        let size = 24;
        let mut classes = vec![lc::NONE; size * size];
        for y in 0..size {
            for x in 0..size {
                let r = (x as f64 - 11.5).abs().max((y as f64 - 11.5).abs());
                classes[y * size + x] = if r > 9.0 {
                    lc::WATER
                } else if r > 6.0 {
                    lc::MIXED
                } else if r > 2.0 {
                    lc::ARABLE
                } else {
                    lc::BROADLEAVES
                };
            }
        }
        for y in 1..5 {
            for x in 1..4 {
                classes[y * size + x] = lc::CLEARCUT;
            }
        }
        for y in 19..23 {
            for x in 19..23 {
                classes[y * size + x] = lc::HEATH;
            }
        }
        for y in 0..3 {
            for x in 20..24 {
                classes[y * size + x] = lc::WETLAND;
            }
        }
        LandGrid { size, classes }
    }

    /// Every class at random (seeded), with blobs of grass, bare rock speckled with snow, and
    /// scrub in it.
    fn speckled() -> LandGrid {
        let size = 40;
        let mut classes = vec![lc::NONE; size * size];
        let mut seed = 11u32;
        for c in classes.iter_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *c = ((seed >> 16) % 15) as u8;
        }
        for y in 4..16 {
            for x in 6..20 {
                classes[y * size + x] = lc::GRASS;
            }
        }
        for y in 22..36 {
            for x in 20..38 {
                classes[y * size + x] = if (x + y) % 7 == 0 { lc::SNOW } else { lc::BARE };
            }
        }
        for y in 25..31 {
            for x in 25..31 {
                classes[y * size + x] = lc::SHRUB;
            }
        }
        LandGrid { size, classes }
    }

    #[test]
    fn outlines_match_the_typescript_point_for_point() {
        // (from `landcover-ground.ts` and d3-contour 4.0.2, run in bun)
        let (wood, field, heath, wetland) = ([255, 255, 255, 253], [255, 255, 255, 252], [236, 220, 196, 251], [196, 214, 200, 251]);
        assert_eq!(
            digest(&cover_outlines(&three_bands())),
            (2, 9323.000004559755, vec![wood, field], vec![vec![89], vec![89]], vec![vec![[12.000000044703482, 31.5]], vec![[24.000000044703484, 31.5]]])
        );
        // d3's holes: the field's ring holds the island, the wood's ring the field
        assert_eq!(
            digest(&cover_outlines(&nested())),
            (
                6,
                12888.364850457298,
                vec![wetland, heath, heath, wood, wood, field],
                vec![vec![15], vec![17], vec![21], vec![17], vec![73, 49], vec![49, 17]],
                vec![
                    vec![[24.0, 2.5]],
                    vec![[2.7499998994171424, 4.5]],
                    vec![[21.690476102893843, 23.5]],
                    vec![[13.12500002514571, 13.5]],
                    vec![[18.35000007241965, 20.5], [16.5, 17.81249994342215]],
                    vec![[17.12500002514571, 17.5], [12.5, 13.812499943422148]],
                ]
            )
        );
        let (scrub, meadow, rock) = ([230, 236, 214, 253], [255, 246, 206, 251], [255, 255, 255, 248]);
        assert_eq!(
            digest(&cover_outlines(&speckled())),
            (
                5,
                18900.592156614257,
                vec![scrub, wood, wood, meadow, rock],
                vec![vec![25], vec![15], vec![17], vec![53], vec![73, 25]],
                vec![
                    vec![[30.12500002514571, 30.5]],
                    vec![[25.562499962281436, 13.5]],
                    vec![[9.91666649902857, 23.5]],
                    vec![[19.12500002514571, 15.5]],
                    vec![[35.416666733721875, 35.5], [29.5, 30.904761855496847]],
                ]
            )
        );
        // every ring closed
        for a in cover_outlines(&nested()) {
            for r in &a.polygon {
                assert_eq!(r[0], r[r.len() - 1]);
            }
        }
    }

    #[test]
    fn the_paints_group_the_classes_as_the_typescripts_objects_do() {
        // (the TypeScript groups by the paint's object; here by its value, which is the same
        // only while no two of the paints are equal)
        let mut groups: Vec<(Rgba, Vec<LandClass>)> = Vec::new();
        for c in 0..=lc::SNOW {
            if let Some(paint) = land_cover_paint(c) {
                match groups.iter_mut().find(|(p, _)| *p == paint) {
                    Some((_, g)) => g.push(c),
                    None => groups.push((paint, vec![c])),
                }
            }
        }
        let classes: Vec<Vec<LandClass>> = groups.into_iter().map(|(_, g)| g).collect();
        assert_eq!(
            classes,
            [
                vec![lc::CONIFERS, lc::MIXED, lc::BROADLEAVES, lc::FOREST],
                vec![lc::CLEARCUT, lc::HEATH],
                vec![lc::WETLAND],
                vec![lc::ARABLE],
                vec![lc::GRASS],
                vec![lc::SHRUB],
                vec![lc::BARE],
                vec![lc::SNOW],
            ]
        );
        assert_eq!(land_cover_paint(lc::CONIFERS), Some(area::WOOD));
    }

    #[test]
    fn too_small_or_unpainted_ground_has_no_outline() {
        assert!(cover_outlines(&LandGrid::default()).is_empty());
        // five cells of heath: under six
        let mut classes = vec![lc::BUILT; 16 * 16];
        for i in 0..5 {
            classes[40 + i] = lc::HEATH;
        }
        assert!(cover_outlines(&LandGrid { size: 16, classes: classes.clone() }).is_empty());
        // all water
        assert!(cover_outlines(&LandGrid { size: 16, classes: vec![lc::WATER; 256] }).is_empty());
        // a grid shorter than its size reads none past its end
        classes.truncate(100);
        assert!(cover_outlines(&LandGrid { size: 16, classes }).is_empty());
    }

    #[test]
    fn outlines_placed_in_longitude_and_latitude() {
        let t = TileIndex { x: 549, y: 320, z: 10 };
        let grid = three_bands();
        let cells = cover_outlines(&grid);
        let placed = placed(t, &grid);
        assert_eq!(placed.len(), cells.len());
        let n = 1024.0;
        for (a, b) in cells.iter().zip(&placed) {
            assert_eq!(a.paint, b.paint);
            for (ra, rb) in a.polygon.iter().zip(&b.polygon) {
                assert_eq!(ra.len(), rb.len());
                for (&[x, y], &[lon, lat]) in ra.iter().zip(rb) {
                    assert_eq!(lon, ((549.0 + x / 32.0) / n) * 360.0 - 180.0);
                    assert_eq!(lat, ((PI * (1.0 - (2.0 * (320.0 + y / 32.0)) / n)).sinh().atan() * 180.0) / PI);
                }
            }
        }
        // the wood's west edge on the tile's
        let west = (549.0 / n) * 360.0 - 180.0;
        assert!(placed[0].polygon[0].iter().any(|p| p[0] == west));
    }
}
