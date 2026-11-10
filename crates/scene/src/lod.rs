//! Port of `lod-tileset.ts` (S1.26; doc 04 §13, doc 05 §13.4): tiles chosen by their distance
//! from the camera, near ones fine, far ones coarse, in every view (deck.gl does so only on the
//! globe and in flat views tilted past 60°; flat at 60° or less it takes the view's zoom
//! everywhere).
//!
//! Its rule is deck.gl's own (`tile-2d-traversal.js`: a tile is fine enough when its zoom plus
//! log2 of its distance, in view heights, reaches the view's), with the view's zoom taken out, so a
//! tile's level follows only how far it is from the camera; a bias makes all of them finer, and
//! only the zooms a source has are picked (one between goes to the finer). The traversal is deck.gl
//! 9.4.0's, and the bounding volumes, planes and culling are math.gl 4.1.0's (`@math.gl/culling`,
//! itself from Cesium), transliterated with their arithmetic in the same order.
//!
//! The ground, buildings, platforms and the 3D terrain use it ([`Lod`]); the realistic layers'
//! `Lod` values belong to their modules (doc 04 §13.1).
//!
//! What is here, and what the tile runtime does with it:
//! - the selection, [`distance_tile_indices`], for any viewport that is a [`LodViewport`] (the
//!   camera's globe and Web Mercator viewports implement it);
//! - `LodTileset.getTileIndices` (the layer's `minZoom`, the tile size's offset) and the
//!   look-ahead while zooming, [`LodTileset`]: the tileset creates (and requests) the tiles it
//!   lists, first those ahead, and asks [`LodTileset::request_priority`] before its own rule;
//! - the refinement (which cached tiles are drawn), [`keep_coarser_until_in`];
//! - `LodMVTLayer.getTileData`'s per-zoom sources, [`source_for`] and [`url_from_template`]. A
//!   source's tiles decode in tile-local coordinates (0 to 1, y south) on the flat map and to
//!   WGS84 on the globe, as the map's own tiles do;
//! - the math.gl culling it is built on ([`Plane`], [`AxisAlignedBox`], [`OrientedBox`],
//!   [`oriented_box_from_points`], [`compute_visibility`]), public for deck.gl's own traversal in
//!   `tiles`.
//!
//! Differences from the TypeScript:
//! - a [`Lod`] without levels selects nothing (the TypeScript would recurse without end);
//! - the tileset's cache is the caller's: [`LodTileset::tile_indices`] asks whether a tile is
//!   cached (`cached`), and gives back the tiles to load ahead instead of creating them;
//! - tile ids are [`TileIndex`] values, not deck.gl's `"x-y-z"` strings.

use std::f64::consts::PI;

use indexmap::IndexSet;
use rustradar_core::num::sign0;

pub use crate::bridges::TileIndex;
use crate::geom::{js_hypot, js_max, js_min};

/// deck.gl's tile size in common units: the world is 512 units round.
pub const TILE_SIZE: f64 = 512.0;
// where a tile on the globe is sampled for its box (tile-2d-traversal.js)
const REF_5: [[f64; 2]; 5] = [[0.5, 0.5], [0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]];
const REF_9: [[f64; 2]; 9] =
    [[0.5, 0.5], [0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.5], [0.5, 0.0], [1.0, 0.5], [0.5, 1.0]];
const REF_11: [[f64; 2]; 11] = [
    [0.5, 0.5],
    [0.0, 0.0],
    [0.0, 1.0],
    [1.0, 0.0],
    [1.0, 1.0],
    [0.0, 0.5],
    [0.5, 0.0],
    [1.0, 0.5],
    [0.5, 1.0],
    [0.25, 0.5],
    [0.75, 0.5],
];

/// How a tile layer picks its tiles' zooms by distance.
#[derive(Debug, Clone, PartialEq)]
pub struct Lod {
    /// The zooms with tiles, ascending.
    pub levels: Vec<u32>,
    /// Zoom levels finer than deck.gl's rule, everywhere. Default 0.
    pub bias: f64,
    /// Nothing farther than this from the camera, metres. Default infinite.
    pub max_distance: f64,
}

impl Lod {
    /// The levels, with no bias and no farthest distance.
    pub fn new(levels: impl Into<Vec<u32>>) -> Self {
        Self { levels: levels.into(), bias: 0.0, max_distance: f64::INFINITY }
    }
}

/// One of a viewport's frustum planes as deck.gl's `getFrustumPlanes()` gives it: the normal
/// points out of the frustum (the selection negates it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrustumPlane {
    pub normal: [f64; 3],
    pub distance: f64,
}

/// What the selection reads from a viewport (deck.gl's `Viewport`, `GlobeViewport` and
/// `WebMercatorViewport`). Positions are in deck.gl's common space: on the globe a sphere of
/// radius 256 units, on the flat map 512 units round the world with y growing north.
pub trait LodViewport: Sized {
    /// `viewport.isGeospatial`: it has a longitude and latitude.
    fn is_geospatial(&self) -> bool;
    /// Whether it is the globe's (`instanceof GlobeViewport`).
    fn is_globe(&self) -> bool;
    /// `viewport.projectPosition([lng, lat, z])`; only asked of the globe's. (The selection passes
    /// heights in common units where deck.gl's `projectPosition` takes metres, as deck.gl's own
    /// traversal does; both are 0 without terrain.)
    fn project_position(&self, xyz: [f64; 3]) -> [f64; 3];
    /// `viewport.getFrustumPlanes()`: left, right, bottom, top, near, far.
    fn frustum_planes(&self) -> [FrustumPlane; 6];
    /// `viewport.cameraPosition`, common space.
    fn camera_position(&self) -> [f64; 3];
    /// `viewport.distanceScales.unitsPerMeter[2]`.
    fn units_per_meter(&self) -> f64;
    /// `viewport.scale` (on the globe `2^(zoom − log2(π·cos lat))`, on the flat map `2^zoom`).
    fn scale(&self) -> f64;
    /// `viewport.zoom`.
    fn zoom(&self) -> f64;
    /// `viewport.height`, CSS pixels.
    fn height(&self) -> f64;
    /// `viewport.longitude`.
    fn longitude(&self) -> f64;
    /// `viewport.latitude`.
    fn latitude(&self) -> f64;
    /// A viewport of the same kind built from only this one's width, height, pitch and bearing and
    /// the given longitude, latitude and zoom, with deck.gl's defaults for everything else (no
    /// `position`, the default near and far multipliers): `loadAhead`'s
    /// `new viewport.constructor({ width, height, pitch, bearing, longitude, latitude, zoom })`.
    fn ahead(&self, longitude: f64, latitude: f64, zoom: f64) -> Self;
}

// --- math.gl's culling ----------------------------------------------------------

/// How a bounding volume lies against a plane or a frustum (math.gl's `INTERSECTION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Intersection {
    Outside = -1,
    Intersecting = 0,
    Inside = 1,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    // math.gl's `dot`: summed from 0, in order
    let mut product = 0.0;
    for i in 0..3 {
        product += a[i] * b[i];
    }
    product
}

fn magnitude(v: [f64; 3]) -> f64 {
    let mut length = 0.0;
    for x in v {
        length += x * x;
    }
    length.sqrt()
}

/// math.gl's `normalize`: a zero vector stays as it is.
fn normalize(v: [f64; 3]) -> [f64; 3] {
    let length = magnitude(v);
    if length != 0.0 { [v[0] / length, v[1] / length, v[2] / length] } else { v }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// A plane in Hessian normal form (math.gl's `Plane`): points `p` with `normal·p + distance = 0`,
/// positive on the normal's side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: [f64; 3],
    pub distance: f64,
}

impl Plane {
    /// `new Plane(normal, distance)`: the normal normalised.
    pub fn new(normal: [f64; 3], distance: f64) -> Self {
        Self { normal: normalize(normal), distance }
    }
}

/// math.gl's `AxisAlignedBoundingBox`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisAlignedBox {
    pub minimum: [f64; 3],
    pub maximum: [f64; 3],
    pub center: [f64; 3],
    pub half_diagonal: [f64; 3],
}

impl AxisAlignedBox {
    pub fn new(minimum: [f64; 3], maximum: [f64; 3]) -> Self {
        let center = [(minimum[0] + maximum[0]) * 0.5, (minimum[1] + maximum[1]) * 0.5, (minimum[2] + maximum[2]) * 0.5];
        Self { minimum, maximum, center, half_diagonal: sub(maximum, center) }
    }

    pub fn intersect_plane(&self, plane: &Plane) -> Intersection {
        let (h, n) = (self.half_diagonal, plane.normal);
        let e = h[0] * n[0].abs() + h[1] * n[1].abs() + h[2] * n[2].abs();
        // signed distance from center
        let s = dot(self.center, n) + plane.distance;
        if s - e > 0.0 {
            return Intersection::Inside;
        }
        if s + e < 0.0 {
            // not in front because normals point inward
            return Intersection::Outside;
        }
        Intersection::Intersecting
    }

    pub fn distance_to(&self, point: [f64; 3]) -> f64 {
        self.distance_squared_to(point).sqrt()
    }

    pub fn distance_squared_to(&self, point: [f64; 3]) -> f64 {
        let offset = sub(point, self.center);
        let mut distance_squared = 0.0;
        for (o, h) in offset.into_iter().zip(self.half_diagonal) {
            let d = o.abs() - h;
            if d > 0.0 {
                distance_squared += d * d;
            }
        }
        distance_squared
    }
}

/// math.gl's `OrientedBoundingBox`: a centre and three half axes, the columns of a column-major
/// 3×3 matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrientedBox {
    pub center: [f64; 3],
    pub half_axes: [f64; 9],
}

impl OrientedBox {
    fn column(&self, i: usize) -> [f64; 3] {
        [self.half_axes[i * 3], self.half_axes[i * 3 + 1], self.half_axes[i * 3 + 2]]
    }

    pub fn intersect_plane(&self, plane: &Plane) -> Intersection {
        let (c, n, h) = (self.center, plane.normal, &self.half_axes);
        // the plane is used as if it is its normal; the first three components are assumed to be
        // normalized
        let rad_effective = (n[0] * h[0] + n[1] * h[1] + n[2] * h[2]).abs()
            + (n[0] * h[3] + n[1] * h[4] + n[2] * h[5]).abs()
            + (n[0] * h[6] + n[1] * h[7] + n[2] * h[8]).abs();
        let distance_to_plane = dot(n, c) + plane.distance;
        if distance_to_plane <= -rad_effective {
            // the entire box is on the negative side of the plane normal
            Intersection::Outside
        } else if distance_to_plane >= rad_effective {
            // the entire box is on the positive side of the plane normal
            Intersection::Inside
        } else {
            Intersection::Intersecting
        }
    }

    pub fn distance_to(&self, point: [f64; 3]) -> f64 {
        self.distance_squared_to(point).sqrt()
    }

    /// The estimated distance squared from the closest point on the box to a point (Geometric
    /// Tools for Computer Graphics 10.4.2).
    pub fn distance_squared_to(&self, point: [f64; 3]) -> f64 {
        let offset = sub(point, self.center);
        let mut distance_squared = 0.0;
        for i in 0..3 {
            let axis = self.column(i);
            let half = magnitude(axis);
            let d = dot(offset, normalize(axis)).abs() - half;
            if d > 0.0 {
                distance_squared += d * d;
            }
        }
        distance_squared
    }
}

/// `mat3.multiply(out, a, b)` (gl-matrix): `a · b`, column-major.
fn mat3_multiply(a: &[f64; 9], b: &[f64; 9]) -> [f64; 9] {
    let [a00, a01, a02, a10, a11, a12, a20, a21, a22] = *a;
    let [b00, b01, b02, b10, b11, b12, b20, b21, b22] = *b;
    [
        b00 * a00 + b01 * a10 + b02 * a20,
        b00 * a01 + b01 * a11 + b02 * a21,
        b00 * a02 + b01 * a12 + b02 * a22,
        b10 * a00 + b11 * a10 + b12 * a20,
        b10 * a01 + b11 * a11 + b12 * a21,
        b10 * a02 + b11 * a12 + b12 * a22,
        b20 * a00 + b21 * a10 + b22 * a20,
        b20 * a01 + b21 * a11 + b22 * a21,
        b20 * a02 + b21 * a12 + b22 * a22,
    ]
}

fn mat3_transpose(a: &[f64; 9]) -> [f64; 9] {
    [a[0], a[3], a[6], a[1], a[4], a[7], a[2], a[5], a[8]]
}

const IDENTITY3: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// math.gl's `getElementIndex(row, col)` for a 3×3 matrix.
const fn at3(row: usize, col: usize) -> usize {
    col * 3 + row
}

const ROW_VAL: [usize; 3] = [1, 0, 0];
const COL_VAL: [usize; 3] = [2, 2, 1];

/// The "off-diagonal" Frobenius norm of a symmetric matrix.
fn off_diagonal_frobenius_norm(m: &[f64; 9]) -> f64 {
    let mut norm = 0.0;
    for i in 0..3 {
        let temp = m[at3(COL_VAL[i], ROW_VAL[i])];
        norm += 2.0 * temp * temp;
    }
    norm.sqrt()
}

/// The Jacobi rotation that reduces a symmetric matrix's largest off-diagonal term (Golub and Van
/// Loan, Matrix Computations, 3rd ed., 8.4.2).
fn shur_decomposition(m: &[f64; 9]) -> [f64; 9] {
    let tolerance = 1e-15;
    let mut max_diagonal = 0.0;
    let mut rot_axis = 1;
    // find pivot (rot_axis) based on max diagonal of matrix
    for i in 0..3 {
        let temp = m[at3(COL_VAL[i], ROW_VAL[i])].abs();
        if temp > max_diagonal {
            rot_axis = i;
            max_diagonal = temp;
        }
    }
    let (p, q) = (ROW_VAL[rot_axis], COL_VAL[rot_axis]);
    let (mut c, mut s) = (1.0, 0.0);
    if m[at3(q, p)].abs() > tolerance {
        let (qq, pp, qp) = (m[at3(q, q)], m[at3(p, p)], m[at3(q, p)]);
        let tau = (qq - pp) / 2.0 / qp;
        let t = if tau < 0.0 { -1.0 / (-tau + (1.0 + tau * tau).sqrt()) } else { 1.0 / (tau + (1.0 + tau * tau).sqrt()) };
        c = 1.0 / (1.0 + t * t).sqrt();
        s = t * c;
    }
    let mut result = IDENTITY3;
    result[at3(p, p)] = c;
    result[at3(q, q)] = c;
    result[at3(q, p)] = s;
    result[at3(p, q)] = -s;
    result
}

/// The unitary matrix of a symmetric 3×3 matrix's eigen decomposition (its columns the
/// eigenvectors), by the classical Jacobi algorithm (Golub and Van Loan, 8.4.3): math.gl's
/// `computeEigenDecomposition(matrix).unitary`.
fn eigen_unitary(matrix: &[f64; 9]) -> [f64; 9] {
    const EIGEN_TOLERANCE: f64 = 1e-20;
    const EIGEN_MAX_SWEEPS: u32 = 10;
    let (mut count, mut sweep) = (0, 0);
    let mut unitary = IDENTITY3;
    let mut diagonal = *matrix;
    let mut norm = 0.0;
    for temp in diagonal {
        norm += temp * temp;
    }
    let epsilon = EIGEN_TOLERANCE * f64::sqrt(norm);
    while sweep < EIGEN_MAX_SWEEPS && off_diagonal_frobenius_norm(&diagonal) > epsilon {
        let j = shur_decomposition(&diagonal);
        let j_transpose = mat3_transpose(&j);
        diagonal = mat3_multiply(&diagonal, &j);
        diagonal = mat3_multiply(&j_transpose, &diagonal);
        unitary = mat3_multiply(&unitary, &j);
        count += 1;
        if count > 2 {
            sweep += 1;
            count = 0;
        }
    }
    unitary
}

/// The oriented box of some points (math.gl's `makeOrientedBoundingBoxFromPoints`: Gottschalk's
/// covariance method). No points give a box of zero size at the origin.
pub fn oriented_box_from_points(positions: &[[f64; 3]]) -> OrientedBox {
    if positions.is_empty() {
        return OrientedBox { center: [0.0; 3], half_axes: [0.0; 9] };
    }
    let mut mean = [0.0; 3];
    for p in positions {
        for i in 0..3 {
            mean[i] += p[i];
        }
    }
    let inv_length = 1.0 / positions.len() as f64;
    for m in &mut mean {
        *m *= inv_length;
    }
    let (mut exx, mut exy, mut exz, mut eyy, mut eyz, mut ezz) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for position in positions {
        let p = sub(*position, mean);
        exx += p[0] * p[0];
        exy += p[0] * p[1];
        exz += p[0] * p[2];
        eyy += p[1] * p[1];
        eyz += p[1] * p[2];
        ezz += p[2] * p[2];
    }
    exx *= inv_length;
    exy *= inv_length;
    exz *= inv_length;
    eyy *= inv_length;
    eyz *= inv_length;
    ezz *= inv_length;
    let rotation = eigen_unitary(&[exx, exy, exz, exy, eyy, eyz, exz, eyz, ezz]);
    let column = |i: usize| [rotation[i * 3], rotation[i * 3 + 1], rotation[i * 3 + 2]];
    let (v1, v2, v3) = (column(0), column(1), column(2));
    let (mut u1, mut u2, mut u3) = (-f64::MAX, -f64::MAX, -f64::MAX);
    let (mut l1, mut l2, mut l3) = (f64::MAX, f64::MAX, f64::MAX);
    for &p in positions {
        u1 = js_max(dot(p, v1), u1);
        u2 = js_max(dot(p, v2), u2);
        u3 = js_max(dot(p, v3), u3);
        l1 = js_min(dot(p, v1), l1);
        l2 = js_min(dot(p, v2), l2);
        l3 = js_min(dot(p, v3), l3);
    }
    let scaled = |v: [f64; 3], k: f64| [v[0] * k, v[1] * k, v[2] * k];
    let (v1, v2, v3) = (scaled(v1, 0.5 * (l1 + u1)), scaled(v2, 0.5 * (l2 + u2)), scaled(v3, 0.5 * (l3 + u3)));
    let center = [v1[0] + v2[0] + v3[0], v1[1] + v2[1] + v3[1], v1[2] + v2[2] + v3[2]];
    let scale = [(u1 - l1) * 0.5, (u2 - l2) * 0.5, (u3 - l3) * 0.5];
    let scale_matrix = [scale[0], 0.0, 0.0, 0.0, scale[1], 0.0, 0.0, 0.0, scale[2]];
    OrientedBox { center, half_axes: mat3_multiply(&rotation, &scale_matrix) }
}

/// A tile's bounding volume: a box on the flat map, an oriented one on the globe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoundingVolume {
    Aligned(AxisAlignedBox),
    Oriented(OrientedBox),
}

impl BoundingVolume {
    pub fn intersect_plane(&self, plane: &Plane) -> Intersection {
        match self {
            BoundingVolume::Aligned(b) => b.intersect_plane(plane),
            BoundingVolume::Oriented(b) => b.intersect_plane(plane),
        }
    }

    pub fn distance_to(&self, point: [f64; 3]) -> f64 {
        match self {
            BoundingVolume::Aligned(b) => b.distance_to(point),
            BoundingVolume::Oriented(b) => b.distance_to(point),
        }
    }
}

/// math.gl's `CullingVolume.computeVisibility`: outside as soon as the volume is outside one plane,
/// inside when inside all of them, else intersecting.
pub fn compute_visibility(planes: &[Plane], volume: &BoundingVolume) -> Intersection {
    let mut intersect = Intersection::Inside;
    for plane in planes {
        match volume.intersect_plane(plane) {
            Intersection::Outside => return Intersection::Outside,
            Intersection::Intersecting => intersect = Intersection::Intersecting,
            Intersection::Inside => {}
        }
    }
    intersect
}

/// deck.gl's frustum planes made into the culling's: their normals turned inward
/// (`new Plane(normal.clone().negate(), distance)`).
pub fn culling_planes(planes: &[FrustumPlane; 6]) -> [Plane; 6] {
    planes.map(|p| Plane::new([-p.normal[0], -p.normal[1], -p.normal[2]], p.distance))
}

// --- the selection --------------------------------------------------------------

fn tile_lng_lat(x: f64, y: f64, z: u32) -> [f64; 2] {
    let scale = 2f64.powi(z as i32);
    let n = PI - (2.0 * PI * y) / scale;
    [(x / scale) * 360.0 - 180.0, (180.0 / PI) * (0.5 * (n.exp() - (-n).exp())).atan()]
}

fn bounding_volume<V: LodViewport>(x: u32, y: u32, z: u32, elevation: [f64; 2], globe: Option<&V>) -> BoundingVolume {
    if let Some(viewport) = globe {
        let refs: &[[f64; 2]] = if z < 1 {
            &REF_11
        } else if z < 2 {
            &REF_9
        } else {
            &REF_5
        };
        let mut points = Vec::with_capacity(refs.len() * 2);
        for &[px, py] in refs {
            let [lng, lat] = tile_lng_lat(f64::from(x) + px, f64::from(y) + py, z);
            points.push(viewport.project_position([lng, lat, elevation[0]]));
            if elevation[0] != elevation[1] {
                points.push(viewport.project_position([lng, lat, elevation[1]]));
            }
        }
        return BoundingVolume::Oriented(oriented_box_from_points(&points));
    }
    // web mercator's common space, y up
    let extent = TILE_SIZE / 2f64.powi(z as i32);
    let (ox, oy) = (f64::from(x) * extent, TILE_SIZE - (f64::from(y) + 1.0) * extent);
    BoundingVolume::Aligned(AxisAlignedBox::new([ox, oy, elevation[0]], [ox + extent, oy + extent, elevation[1]]))
}

/// Whether a tile on the globe is all on the far side of it from the camera.
fn beyond_horizon<V: LodViewport>(x: u32, y: u32, z: u32, camera: [f64; 3], viewport: &V, elevation: f64) -> bool {
    let [cx, cy, cz] = camera;
    let cam_lng = (cx.atan2(-cy) * 180.0) / PI;
    let cam_lat = ((cz / js_hypot(&[cx, cy, cz])).asin() * 180.0) / PI;
    let [west, north] = tile_lng_lat(f64::from(x), f64::from(y), z);
    let [east, south] = tile_lng_lat(f64::from(x) + 1.0, f64::from(y) + 1.0, z);
    let mid = (west + east) / 2.0;
    let lng = js_max(west, js_min(mid + (((cam_lng - mid + 540.0) % 360.0) - 180.0), east));
    let lat = js_max(south, js_min(cam_lat, north));
    let p = viewport.project_position([lng, lat, elevation]);
    p[0] * cx + p[1] * cy + p[2] * cz <= p[0] * p[0] + p[1] * p[1] + p[2] * p[2]
}

struct Traversal<'a, V> {
    globe: Option<&'a V>,
    planes: [Plane; 6],
    elevation: [f64; 2],
    camera: [f64; 3],
    k: f64,
    farthest: f64,
    bias: f64,
    offset: f64,
    levels: &'a [u32],
    lowest: u32,
    highest: u32,
}

impl<V: LodViewport> Traversal<'_, V> {
    /// The coarsest level at least as fine as `want` (`levelFor`); NaN when there is none (a
    /// NaN `want`), which no zoom reaches, as `undefined` in the TypeScript.
    fn level_for(&self, want: f64) -> f64 {
        if want >= f64::from(self.highest) {
            f64::from(self.highest)
        } else if want <= f64::from(self.lowest) {
            f64::from(self.lowest)
        } else {
            self.levels.iter().find(|&&l| f64::from(l) >= want).map_or(f64::NAN, |&l| f64::from(l))
        }
    }

    fn visit(&self, x: u32, y: u32, z: u32, out: &mut Vec<TileIndex>) {
        let volume = bounding_volume(x, y, z, self.elevation, self.globe);
        if compute_visibility(&self.planes, &volume) == Intersection::Outside {
            return;
        }
        if let Some(viewport) = self.globe
            && beyond_horizon(x, y, z, self.camera, viewport, self.elevation[1])
        {
            return;
        }
        let distance = volume.distance_to(self.camera);
        if distance > self.farthest {
            return;
        }
        if self.levels.contains(&z) {
            let want = self.offset - (js_max(distance * self.k, 1e-12).log2() - self.bias).floor();
            if f64::from(z) >= self.level_for(want) {
                out.push(TileIndex { x, y, z });
                return;
            }
        }
        if z >= self.highest {
            return;
        }
        for [dx, dy] in [[0, 0], [0, 1], [1, 0], [1, 1]] {
            self.visit(x * 2 + dx, y * 2 + dy, z + 1, out);
        }
    }
}

/// The tiles in view, each at the zoom its distance from the camera calls for.
///
/// Recursively from tile 0/0/0: a tile outside the frustum, (on the globe) wholly beyond the
/// horizon or farther than `lod.max_distance` is left out; at a zoom of `lod.levels` it is taken
/// when its zoom reaches the level `offset − floor(log2(distance · k) − bias)` calls for (snapped
/// to the next level there is, clamped to the lowest and highest), where `k = scale / 2^zoom /
/// height` takes the view's zoom out of deck.gl's distance in view heights; else its four
/// children are visited, down to the highest level. `z_range`: the lowest and highest ground in
/// metres (none: 0); `offset`: `log2(512 / tileSize) + zoomOffset`.
pub fn distance_tile_indices<V: LodViewport>(viewport: &V, lod: &Lod, z_range: Option<[f64; 2]>, offset: f64) -> Vec<TileIndex> {
    let Lod { levels, bias, max_distance } = lod;
    let (Some(&lowest), Some(&highest)) = (levels.first(), levels.last()) else { return Vec::new() };
    let upm = viewport.units_per_meter();
    let traversal = Traversal {
        globe: viewport.is_globe().then_some(viewport),
        planes: culling_planes(&viewport.frustum_planes()),
        elevation: z_range.map_or([0.0, 0.0], |[lo, hi]| [lo * upm, hi * upm]),
        camera: viewport.camera_position(),
        // deck.gl's distance is in view heights at the view's scale; without the zoom
        k: viewport.scale() / 2f64.powf(viewport.zoom()) / viewport.height(),
        farthest: max_distance * upm,
        bias: *bias,
        offset,
        levels,
        lowest,
        highest,
    };
    let mut out = Vec::new();
    traversal.visit(0, 0, 0, &mut out);
    out
}

// --- the tileset ------------------------------------------------------------------

/// While zooming, the tiles of the view this many zoom levels further on load ahead.
pub const LOOK_AHEAD_ZOOM: f64 = 1.0;
/// At most this many tiles loaded ahead at once.
pub const MAX_AHEAD: usize = 32;
/// Tiles loaded ahead come after every tile in view (deck.gl's in-view ones are below 2e8).
pub const AHEAD_PRIORITY: f64 = 2e8;

/// Where the camera was at the last selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub longitude: f64,
    pub latitude: f64,
    pub zoom: f64,
}

/// The tileset's options the selection reads (`Opts`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TilesetOptions<'a> {
    /// How the tiles' zooms are picked; none: deck.gl's own way, by the view's zoom.
    pub lod: Option<&'a Lod>,
    /// The layer's tile size, pixels.
    pub tile_size: f64,
    pub zoom_offset: f64,
    /// The layer's `minZoom`, if it has one.
    pub min_zoom: Option<f64>,
    /// Whether the layer has an `extent`.
    pub extent: bool,
}

/// What a tileset loads for a view ([`LodTileset::tile_indices`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// The tiles to load ahead, in order, before the view's own: the tileset gets or creates each
    /// (and rebuilds its tree when there are any).
    pub ahead: Vec<TileIndex>,
    /// The tiles in view.
    pub tiles: Vec<TileIndex>,
}

/// A tileset that picks its tiles by distance (`lod` in its options; without one, deck.gl's own
/// way), and while the view zooms, loads ahead the tiles of where it is heading: as it moves on, a
/// level further in or out, so they are there when it arrives. Those wait for every tile in view,
/// and are dropped if it turns another way first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LodTileset {
    last: Option<Camera>,
    ahead: IndexSet<TileIndex>,
}

impl LodTileset {
    pub fn new() -> Self {
        Self::default()
    }

    /// The tiles for a view (`getTileIndices`), or `None` when the layer's are picked deck.gl's own
    /// way: it has no `lod`, or the viewport is not geospatial. `cached` says whether the tileset
    /// holds a tile already.
    pub fn tile_indices<V: LodViewport>(
        &mut self,
        viewport: &V,
        opts: &TilesetOptions,
        z_range: Option<[f64; 2]>,
        cached: impl Fn(TileIndex) -> bool,
    ) -> Option<Selection> {
        let lod = opts.lod.filter(|_| viewport.is_geospatial())?;
        // (the layer is hidden under its minZoom: nothing to load then)
        if let Some(min_zoom) = opts.min_zoom
            && min_zoom.is_finite()
            && !opts.extent
            && viewport.zoom() < min_zoom
        {
            return Some(Selection::default());
        }
        let offset = (TILE_SIZE / opts.tile_size).log2() + opts.zoom_offset;
        let ahead = self.load_ahead(viewport, lod, z_range, offset, cached);
        Some(Selection { ahead, tiles: distance_tile_indices(viewport, lod, z_range, offset) })
    }

    /// The look-ahead (`loadAhead`): when the zoom has moved since the last view, the tiles of the
    /// view one level further on (the point it zooms on moving with it), at most [`MAX_AHEAD`] of
    /// them, without those cached already unless they were ahead before. Standing still (or the
    /// first view) gives none and keeps the tiles ahead as they were.
    pub fn load_ahead<V: LodViewport>(
        &mut self,
        viewport: &V,
        lod: &Lod,
        z_range: Option<[f64; 2]>,
        offset: f64,
        cached: impl Fn(TileIndex) -> bool,
    ) -> Vec<TileIndex> {
        let now = Camera { longitude: viewport.longitude(), latitude: viewport.latitude(), zoom: viewport.zoom() };
        let last = self.last.replace(now);
        let dz = last.map_or(0.0, |last| now.zoom - last.zoom);
        let Some(last) = last.filter(|_| dz.abs() >= 1e-3) else { return Vec::new() };
        // on as it is going (the point it zooms on moving with it), until a level further
        let k = js_min(LOOK_AHEAD_ZOOM / dz.abs(), 50.0);
        let there = viewport.ahead(
            now.longitude + (now.longitude - last.longitude) * k,
            js_max(-85.0, js_min(85.0, now.latitude + (now.latitude - last.latitude) * k)),
            now.zoom + sign0(dz) * LOOK_AHEAD_ZOOM,
        );
        let mut ahead = IndexSet::new();
        for index in distance_tile_indices(&there, lod, z_range, offset) {
            if ahead.len() >= MAX_AHEAD {
                break;
            }
            if cached(index) && !self.ahead.contains(&index) {
                continue;
            }
            ahead.insert(index);
        }
        self.ahead = ahead;
        self.ahead.iter().copied().collect()
    }

    /// Whether a tile is being loaded ahead.
    pub fn is_ahead(&self, tile: TileIndex) -> bool {
        self.ahead.contains(&tile)
    }

    /// A request's priority when it is one loading ahead (neither selected nor visible): after
    /// every tile in view, by its distance priority (deck.gl's `_getTileDistancePriority`), where
    /// deck.gl would drop it. `None`: deck.gl's own rule applies.
    pub fn request_priority(&self, tile: TileIndex, selected: bool, visible: bool, distance_priority: f64) -> Option<f64> {
        (!selected && !visible && self.is_ahead(tile)).then_some(AHEAD_PRIORITY + distance_priority)
    }
}

// --- refinement ---------------------------------------------------------------------

/// A tile of the cache as the refinement sees it; `parent` and `children` index the same slice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefineTile {
    pub state: u8,
    pub is_selected: bool,
    pub is_visible: bool,
    pub zoom: u32,
    /// It has something to draw (`content != null`): one that failed, or isn't there, doesn't
    /// count: what is over it is drawn instead.
    pub ready: bool,
    pub parent: Option<usize>,
    pub children: Option<Vec<usize>>,
}

const VISITED: u8 = 1;
const VISIBLE: u8 = 2;

/// deck.gl's 'no-overlap' refinement (a coarser tile is drawn in place of finer ones until they
/// are all in, never with them: 3D buildings would stand twice), but a tile that came back empty
/// or failed keeps the one over it. Sets each tile's `is_visible`.
pub fn keep_coarser_until_in(tiles: &mut [RefineTile]) {
    for tile in tiles.iter_mut() {
        tile.state = 0;
    }
    for i in 0..tiles.len() {
        if !tiles[i].is_selected {
            continue;
        }
        let mut t = Some(i);
        while let Some(j) = t {
            if tiles[j].ready {
                tiles[j].state |= VISIBLE;
                break;
            }
            t = tiles[j].parent;
        }
    }
    fn finer(tiles: &mut [RefineTile], tile: usize) {
        for child in tiles[tile].children.clone().unwrap_or_default() {
            if tiles[child].ready {
                tiles[child].state |= VISIBLE;
            } else {
                finer(tiles, child);
            }
        }
    }
    let mut order: Vec<usize> = (0..tiles.len()).collect();
    order.sort_by_key(|&i| tiles[i].zoom);
    for i in order {
        tiles[i].is_visible = tiles[i].state & VISIBLE != 0;
        if let Some(children) = tiles[i].children.clone()
            && (tiles[i].is_visible || tiles[i].state & VISITED != 0)
        {
            for child in children {
                tiles[child].state = VISITED;
            }
        } else if tiles[i].is_selected {
            finer(tiles, i);
        }
    }
}

// --- per-zoom sources -----------------------------------------------------------------

/// Where a zoom's tiles come from, for a layer whose tiles are picked by distance (`TileSource`;
/// its own `fetch` is the tile runtime's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileSource {
    /// The zooms it has tiles at.
    pub zooms: Vec<u32>,
    /// Its tiles, `{z}/{x}/{y}`.
    pub url: String,
    /// The tiles' layers to read.
    pub layers: Vec<String>,
}

/// The source of a tile's zoom: the first whose zooms include it (none: the tile has no content).
pub fn source_for(sources: &[TileSource], z: u32) -> Option<&TileSource> {
    sources.iter().find(|s| s.zooms.contains(&z))
}

/// A tile's URL from a template (deck.gl's `getURLFromTemplate` for one template): `{x}`, `{y}`,
/// `{z}` and `{-y}` (the row counted from the south); `None` for an empty template.
pub fn url_from_template(template: &str, index: TileIndex) -> Option<String> {
    if template.is_empty() {
        return None;
    }
    let url = template.replace("{x}", &index.x.to_string()).replace("{y}", &index.y.to_string()).replace("{z}", &index.z.to_string());
    let flipped = 2f64.powi(index.z as i32) - f64::from(index.y) - 1.0;
    Some(url.replace("{-y}", &rustradar_core::num::js_string(flipped)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeSet, HashSet};

    // --- a Web Mercator viewport, as deck.gl 9.4's `WebMercatorViewport` makes one ---------
    //
    // The camera module's viewports are stage 2's (doc 01 §2); this is just what the tests need,
    // with gl-matrix's and @math.gl/web-mercator's arithmetic in the same order, checked below
    // against deck.gl's own numbers.

    const DEG: f64 = PI / 180.0;
    type Mat4 = [f64; 16];
    const IDENTITY4: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

    fn translate(a: &Mat4, [x, y, z]: [f64; 3]) -> Mat4 {
        let mut out = *a;
        out[12] = a[0] * x + a[4] * y + a[8] * z + a[12];
        out[13] = a[1] * x + a[5] * y + a[9] * z + a[13];
        out[14] = a[2] * x + a[6] * y + a[10] * z + a[14];
        out[15] = a[3] * x + a[7] * y + a[11] * z + a[15];
        out
    }

    fn rotate_x(a: &Mat4, rad: f64) -> Mat4 {
        let (s, c) = (rad.sin(), rad.cos());
        let mut out = *a;
        for i in 0..4 {
            out[4 + i] = a[4 + i] * c + a[8 + i] * s;
            out[8 + i] = a[8 + i] * c - a[4 + i] * s;
        }
        out
    }

    fn rotate_z(a: &Mat4, rad: f64) -> Mat4 {
        let (s, c) = (rad.sin(), rad.cos());
        let mut out = *a;
        for i in 0..4 {
            out[i] = a[i] * c + a[4 + i] * s;
            out[4 + i] = a[4 + i] * c - a[i] * s;
        }
        out
    }

    fn scale(a: &Mat4, [x, y, z]: [f64; 3]) -> Mat4 {
        let mut out = *a;
        for i in 0..4 {
            out[i] = a[i] * x;
            out[4 + i] = a[4 + i] * y;
            out[8 + i] = a[8 + i] * z;
        }
        out
    }

    fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
        let mut out = [0.0; 16];
        for col in 0..4 {
            let [b0, b1, b2, b3] = [b[col * 4], b[col * 4 + 1], b[col * 4 + 2], b[col * 4 + 3]];
            for row in 0..4 {
                out[col * 4 + row] = b0 * a[row] + b1 * a[4 + row] + b2 * a[8 + row] + b3 * a[12 + row];
            }
        }
        out
    }

    /// gl-matrix's `mat4.invert`, the translation column only (all `cameraPosition` reads).
    fn inverse_translation(a: &Mat4) -> [f64; 3] {
        let [a00, a01, a02, a03, a10, a11, a12, a13, a20, a21, a22, a23, a30, a31, a32, a33] = *a;
        let (b00, b01, b02) = (a00 * a11 - a01 * a10, a00 * a12 - a02 * a10, a00 * a13 - a03 * a10);
        let (b03, b04, b05) = (a01 * a12 - a02 * a11, a01 * a13 - a03 * a11, a02 * a13 - a03 * a12);
        let (b06, b07, b08) = (a20 * a31 - a21 * a30, a20 * a32 - a22 * a30, a20 * a33 - a23 * a30);
        let (b09, b10, b11) = (a21 * a32 - a22 * a31, a21 * a33 - a23 * a31, a22 * a33 - a23 * a32);
        let det = 1.0 / (b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06);
        [
            (a11 * b07 - a10 * b09 - a12 * b06) * det,
            (a00 * b09 - a01 * b07 + a02 * b06) * det,
            (a31 * b01 - a30 * b03 - a32 * b00) * det,
        ]
    }

    #[derive(Debug, Clone, Copy)]
    struct Mercator {
        width: f64,
        height: f64,
        longitude: f64,
        latitude: f64,
        zoom: f64,
        pitch: f64,
        bearing: f64,
        upm: f64,
        planes: [FrustumPlane; 6],
        camera: [f64; 3],
    }

    impl Mercator {
        fn new(width: f64, height: f64, longitude: f64, latitude: f64, zoom: f64, pitch: f64, bearing: f64) -> Self {
            let altitude: f64 = 1.5;
            let scale_ = 2f64.powf(zoom);
            let fovy = 2.0 * (0.5 / altitude).atan() * (180.0 / PI);
            // getProjectionParameters, nearZMultiplier 0.1, farZMultiplier 1.01
            let (fov_radians, pitch_radians) = (fovy * DEG, pitch * DEG);
            let focal_distance = 0.5 / (0.5 * fovy * DEG).tan();
            let fov_above_center = fov_radians * (0.5 + 0.0 / height);
            let top_half_surface_distance = (fov_above_center.sin() * focal_distance)
                / (PI / 2.0 - pitch_radians - fov_above_center).clamp(0.01, PI - 0.01).sin();
            let furthest_distance = pitch_radians.sin() * top_half_surface_distance + focal_distance;
            let far = js_min(furthest_distance * 1.01, focal_distance * 10.0);
            let near = 0.1;
            // getViewMatrix
            let mut vm = translate(&IDENTITY4, [0.0, 0.0, -altitude]);
            vm = rotate_x(&vm, -pitch * DEG);
            vm = rotate_z(&vm, bearing * DEG);
            let relative_scale = scale_ / height;
            vm = scale(&vm, [relative_scale; 3]);
            // the view centre in common space (lngLatToWorld)
            let (lambda2, phi2) = (longitude * DEG, latitude * DEG);
            let x = (512.0 * (lambda2 + PI)) / (2.0 * PI);
            let y = (512.0 * (PI + (PI / 4.0 + phi2 * 0.5).tan().ln())) / (2.0 * PI);
            let upm = 512.0 / 40.03e6 / (latitude * DEG).cos();
            let center = [x, y.clamp(-318.0, 830.0), 0.0];
            let view = translate(&multiply(&IDENTITY4, &vm), [-center[0], -center[1], -center[2]]);
            // perspective
            let f = 1.0 / (fovy * DEG / 2.0).tan();
            let nf = 1.0 / (near - far);
            let mut projection = [0.0; 16];
            projection[0] = f / (width / height);
            projection[5] = f;
            projection[11] = -1.0;
            projection[10] = (far + near) * nf;
            projection[14] = 2.0 * far * near * nf;
            let vpm = multiply(&multiply(&IDENTITY4, &projection), &view);
            // getFrustumPlanes
            let plane = |a: f64, b: f64, c: f64, d: f64| {
                let l = magnitude([a, b, c]);
                FrustumPlane { distance: d / l, normal: [-a / l, -b / l, -c / l] }
            };
            let m = &vpm;
            let planes = [
                plane(m[3] + m[0], m[7] + m[4], m[11] + m[8], m[15] + m[12]),
                plane(m[3] - m[0], m[7] - m[4], m[11] - m[8], m[15] - m[12]),
                plane(m[3] + m[1], m[7] + m[5], m[11] + m[9], m[15] + m[13]),
                plane(m[3] - m[1], m[7] - m[5], m[11] - m[9], m[15] - m[13]),
                plane(m[3] + m[2], m[7] + m[6], m[11] + m[10], m[15] + m[14]),
                plane(m[3] - m[2], m[7] - m[6], m[11] - m[10], m[15] - m[14]),
            ];
            Self { width, height, longitude, latitude, zoom, pitch, bearing, upm, planes, camera: inverse_translation(&view) }
        }
    }

    impl LodViewport for Mercator {
        fn is_geospatial(&self) -> bool {
            true
        }
        fn is_globe(&self) -> bool {
            false
        }
        fn project_position(&self, _: [f64; 3]) -> [f64; 3] {
            unreachable!("only the globe's is asked")
        }
        fn frustum_planes(&self) -> [FrustumPlane; 6] {
            self.planes
        }
        fn camera_position(&self) -> [f64; 3] {
            self.camera
        }
        fn units_per_meter(&self) -> f64 {
            self.upm
        }
        fn scale(&self) -> f64 {
            2f64.powf(self.zoom)
        }
        fn zoom(&self) -> f64 {
            self.zoom
        }
        fn height(&self) -> f64 {
            self.height
        }
        fn longitude(&self) -> f64 {
            self.longitude
        }
        fn latitude(&self) -> f64 {
            self.latitude
        }
        fn ahead(&self, longitude: f64, latitude: f64, zoom: f64) -> Self {
            Self::new(self.width, self.height, longitude, latitude, zoom, self.pitch, self.bearing)
        }
    }

    fn zooms(tiles: &[TileIndex]) -> Vec<u32> {
        tiles.iter().map(|t| t.z).collect::<BTreeSet<_>>().into_iter().collect()
    }

    fn lod(levels: &[u32], bias: f64) -> Lod {
        Lod { bias, ..Lod::new(levels) }
    }

    // --- tiles by their distance from the camera ----------------------------------

    fn view(zoom: f64, pitch: f64) -> Mercator {
        Mercator::new(1100.0, 720.0, 13.19, 55.705, zoom, pitch, 20.0)
    }

    #[test]
    fn a_tilted_view_fine_near_the_camera_coarser_off_and_only_the_zooms_there_are() {
        let tiles = distance_tile_indices(&view(16.0, 75.0), &lod(&[9, 10, 11, 12, 14, 16], 0.5), None, 0.0);
        assert!(zooms(&tiles).contains(&16));
        assert!(zooms(&tiles).contains(&14));
        assert!(tiles.iter().all(|t| [9, 10, 11, 12, 14, 16].contains(&t.z)));
        // tiles never overlap: none inside another
        let inside = |a: &TileIndex, b: &TileIndex| a.z > b.z && a.x >> (a.z - b.z) == b.x && a.y >> (a.z - b.z) == b.y;
        assert!(!tiles.iter().any(|a| tiles.iter().any(|b| inside(a, b))));
    }

    #[test]
    fn straight_down_the_same_everywhere_further_up_coarser() {
        assert_eq!(zooms(&distance_tile_indices(&view(16.0, 0.0), &lod(&[12, 14, 16], 0.5), None, 0.0)), [16]);
        assert_eq!(zooms(&distance_tile_indices(&view(13.0, 0.0), &lod(&[12, 14, 16], 0.0), None, 0.0)), [14]);
    }

    #[test]
    fn nothing_past_the_farthest() {
        let all = distance_tile_indices(&view(15.0, 75.0), &lod(&[9, 10, 11, 12, 14, 16], 2.0), None, 0.0);
        let near = distance_tile_indices(
            &view(15.0, 75.0),
            &Lod { max_distance: 10_000.0, ..lod(&[9, 10, 11, 12, 14, 16], 2.0) },
            None,
            0.0,
        );
        assert!(near.len() < all.len());
    }

    // --- loading ahead while zooming ----------------------------------------------------

    fn view60(zoom: f64) -> Mercator {
        Mercator::new(1100.0, 720.0, 13.19, 55.705, zoom, 60.0, 20.0)
    }

    fn ahead_lod() -> Lod {
        Lod { bias: 0.5, max_distance: 25_000.0, ..Lod::new([13, 14, 16]) }
    }

    fn id(i: &TileIndex) -> String {
        format!("{}/{}/{}", i.z, i.x, i.y)
    }

    /// deck.gl's `Tileset2D` as far as the tests see it: `update` selects, creates each tile it
    /// does not hold (the ahead ones first) and asks for its data (`getTileData`) once; with
    /// `maxRequests: 0` nothing is throttled or dropped.
    struct Tileset {
        set: LodTileset,
        lod: Lod,
        cache: RefCell<HashSet<TileIndex>>,
        asked: Vec<String>,
    }

    impl Tileset {
        fn new(lod: Lod) -> Self {
            Self { set: LodTileset::new(), lod, cache: RefCell::default(), asked: Vec::new() }
        }

        fn update(&mut self, viewport: &Mercator) {
            let opts = TilesetOptions { lod: Some(&self.lod), tile_size: 512.0, zoom_offset: 0.0, min_zoom: None, extent: false };
            let cache = &self.cache;
            let selection = self.set.tile_indices(viewport, &opts, None, |i| cache.borrow().contains(&i)).expect("a lod");
            for index in selection.ahead.iter().chain(&selection.tiles) {
                if self.cache.borrow_mut().insert(*index) {
                    self.asked.push(id(index));
                }
            }
        }
    }

    fn in_view(zoom: f64) -> HashSet<String> {
        distance_tile_indices(&view60(zoom), &ahead_lod(), None, 0.0).iter().map(id).collect()
    }

    #[test]
    fn zooming_in_asks_for_the_tiles_of_a_level_further_in_besides_the_views() {
        let mut set = Tileset::new(ahead_lod());
        set.update(&view60(13.8));
        set.update(&view60(14.0));
        let now = in_view(14.0);
        let extra: Vec<&String> = set.asked.iter().filter(|id| !now.contains(*id) && !in_view(13.8).contains(*id)).collect();
        assert!(!extra.is_empty());
        assert!(extra.iter().all(|id| in_view(15.0).contains(*id)));
    }

    #[test]
    fn standing_still_asks_for_nothing_more() {
        let mut set = Tileset::new(ahead_lod());
        set.update(&view60(14.0));
        assert_eq!(set.asked.iter().cloned().collect::<HashSet<_>>(), in_view(14.0));
    }

    // --- new tests -------------------------------------------------------------------------

    /// FNV-1a of the tiles as `z/x/y` joined by spaces, as the TypeScript's lists were hashed.
    fn fnv(tiles: &[TileIndex]) -> u32 {
        let joined = tiles.iter().map(id).collect::<Vec<_>>().join(" ");
        joined.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
    }

    #[test]
    fn the_test_viewport_is_deck_gls() {
        // deck.gl 9.4's WebMercatorViewport for view(16, 75), through bun
        let v = view(16.0, 75.0);
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-12 * b.abs().max(1.0);
        let camera = [274.75366684571935, 351.80421782490953, 0.004265206431751615];
        assert!((0..3).all(|i| close(v.camera[i], camera[i])), "{:?}", v.camera);
        let left = FrustumPlane { distance: -308.94768353927304, normal: [-0.9872832515001971, -0.10712916168835486, 0.1174526458754721] };
        assert!(close(v.planes[0].distance, left.distance) && (0..3).all(|i| close(v.planes[0].normal[i], left.normal[i])));
        let top = FrustumPlane { distance: -25.4333126478599, normal: [-0.020492226113304382, -0.05630192852066216, 0.9982034669914623] };
        assert!(close(v.planes[3].distance, top.distance) && (0..3).all(|i| close(v.planes[3].normal[i], top.normal[i])));
    }

    #[test]
    fn the_selection_is_the_typescripts_tile_for_tile() {
        // (view zoom, pitch, levels, bias, max distance) → the TypeScript's tile count and the hash
        // of its list in order, from `distanceTileIndices` under bun
        type Case = (f64, f64, &'static [u32], f64, f64, usize, u32);
        let cases: [Case; 8] = [
            (16.0, 75.0, &[9, 10, 11, 12, 14, 16], 0.5, f64::INFINITY, 78, 0x15c4_2554),
            (16.0, 0.0, &[12, 14, 16], 0.5, f64::INFINITY, 9, 0x31d1_00e9),
            (13.0, 0.0, &[12, 14, 16], 0.0, f64::INFINITY, 23, 0x6ef3_aadd),
            (15.0, 75.0, &[9, 10, 11, 12, 14, 16], 2.0, f64::INFINITY, 361, 0x543c_9231),
            (15.0, 75.0, &[9, 10, 11, 12, 14, 16], 2.0, 10_000.0, 316, 0xcc3d_27cf),
            (13.8, 60.0, &[13, 14, 16], 0.5, 25_000.0, 51, 0xed90_8d77),
            (14.0, 60.0, &[13, 14, 16], 0.5, 25_000.0, 69, 0x7411_9ede),
            (15.0, 60.0, &[13, 14, 16], 0.5, 25_000.0, 63, 0x4e2b_b90b),
        ];
        for (zoom, pitch, levels, bias, max_distance, count, hash) in cases {
            let tiles = distance_tile_indices(&view(zoom, pitch), &Lod { levels: levels.to_vec(), bias, max_distance }, None, 0.0);
            assert_eq!((tiles.len(), fnv(&tiles)), (count, hash), "zoom {zoom}, pitch {pitch}: {}", tiles.iter().map(id).collect::<Vec<_>>().join(" "));
        }
        let straight_down = distance_tile_indices(&view(16.0, 0.0), &lod(&[12, 14, 16], 0.5), None, 0.0);
        assert_eq!(
            straight_down.iter().map(id).collect::<Vec<_>>().join(" "),
            "16/35167/20503 16/35168/20501 16/35168/20502 16/35168/20503 16/35169/20502 16/35169/20503 16/35170/20502 16/35170/20503 16/35169/20504"
        );
    }

    #[test]
    fn zooming_asks_for_what_the_typescript_asks_for() {
        // the TypeScript's tileset asked for 51 tiles at 13.8, then 55 more at 14 (hashes of the
        // whole list, in order, under bun)
        let mut set = Tileset::new(ahead_lod());
        set.update(&view60(13.8));
        let first = set.asked.len();
        set.update(&view60(14.0));
        let joined = set.asked.join(" ");
        let hash = joined.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
        assert_eq!((first, set.asked.len(), hash), (51, 106, 0x1c64_2ccb));
    }

    #[test]
    fn levels_snap_to_the_finer_and_clamp() {
        let v = view(16.0, 0.0);
        let traversal = Traversal::<Mercator> {
            globe: None,
            planes: culling_planes(&v.frustum_planes()),
            elevation: [0.0; 2],
            camera: v.camera,
            k: 1.0,
            farthest: f64::INFINITY,
            bias: 0.0,
            offset: 0.0,
            levels: &[9, 12, 14, 16],
            lowest: 9,
            highest: 16,
        };
        assert_eq!(traversal.level_for(5.0), 9.0);
        assert_eq!(traversal.level_for(10.0), 12.0);
        assert_eq!(traversal.level_for(12.0), 12.0);
        assert_eq!(traversal.level_for(13.0), 14.0);
        assert_eq!(traversal.level_for(20.0), 16.0);
        assert!(traversal.level_for(f64::NAN).is_nan());
    }

    #[test]
    fn no_levels_select_nothing_and_a_layer_under_its_min_zoom_loads_nothing() {
        let v = view(13.0, 0.0);
        assert!(distance_tile_indices(&v, &Lod::new(Vec::<u32>::new()), None, 0.0).is_empty());
        let lod = lod(&[12, 14, 16], 0.0);
        let mut set = LodTileset::new();
        let opts = |min_zoom| TilesetOptions { lod: Some(&lod), tile_size: 512.0, zoom_offset: 0.0, min_zoom, extent: false };
        assert_eq!(set.tile_indices(&v, &opts(Some(14.0)), None, |_| false), Some(Selection::default()));
        // no lod: deck.gl's own selection
        assert_eq!(set.tile_indices(&v, &TilesetOptions { lod: None, ..opts(None) }, None, |_| false), None);
        // smaller tiles want finer levels: the offset (zooms as the TypeScript's for offsets 1 and 2)
        let at = |set: &mut LodTileset, tile_size| {
            let selection = set.tile_indices(&v, &TilesetOptions { tile_size, ..opts(Some(f64::NAN)) }, None, |_| false);
            zooms(&selection.expect("a lod").tiles)
        };
        assert_eq!(at(&mut set, 512.0), [14]);
        assert_eq!(at(&mut set, 256.0), [14]);
        assert_eq!(at(&mut set, 128.0), [16]);
    }

    #[test]
    fn tiles_ahead_wait_after_the_view_and_go_when_it_turns() {
        let lod = ahead_lod();
        let mut set = LodTileset::new();
        assert!(set.load_ahead(&view60(13.8), &lod, None, 0.0, |_| false).is_empty());
        let ahead = set.load_ahead(&view60(14.0), &lod, None, 0.0, |_| false);
        assert_eq!(ahead.len(), MAX_AHEAD);
        let first = ahead[0];
        assert_eq!(set.request_priority(first, false, false, 7.0), Some(AHEAD_PRIORITY + 7.0));
        assert_eq!(set.request_priority(first, true, false, 7.0), None);
        // standing still keeps them
        assert!(set.load_ahead(&view60(14.0), &lod, None, 0.0, |_| true).is_empty());
        assert!(set.is_ahead(first));
        // cached tiles ahead stay ahead; other cached ones are not asked for again
        let again = set.load_ahead(&view60(14.2), &lod, None, 0.0, |_| true);
        assert!(again.iter().all(|i| ahead.contains(i)));
        // zooming out instead: the tiles of a level further out
        let out = set.load_ahead(&view60(13.9), &lod, None, 0.0, |_| false);
        let further_out: HashSet<String> = distance_tile_indices(&view60(12.9), &lod, None, 0.0).iter().map(id).collect();
        assert!(!out.is_empty() && out.iter().all(|i| further_out.contains(&id(i))));
        assert_eq!(set.is_ahead(first), out.contains(&first));
    }

    fn tile(zoom: u32, selected: bool, ready: bool, parent: Option<usize>, children: &[usize]) -> RefineTile {
        RefineTile { zoom, is_selected: selected, ready, parent, children: Some(children.to_vec()), ..RefineTile::default() }
    }

    fn visible(tiles: &[RefineTile]) -> Vec<usize> {
        (0..tiles.len()).filter(|&i| tiles[i].is_visible).collect()
    }

    #[test]
    fn a_coarser_tile_stands_in_until_the_finer_ones_are_in_never_with_them() {
        // (the visible sets are the TypeScript's for the same trees)
        // 0 is the parent of 1 and 2, both selected, 1 still loading: 0 alone
        let mut tiles = vec![tile(13, false, true, None, &[1, 2]), tile(14, true, false, Some(0), &[]), tile(14, true, true, Some(0), &[])];
        keep_coarser_until_in(&mut tiles);
        assert_eq!(visible(&tiles), [0]);
        // both in: the two of them, not 0
        tiles[1].ready = true;
        keep_coarser_until_in(&mut tiles);
        assert_eq!(visible(&tiles), [1, 2]);
        // the parent selected and in, the children loading: only the parent
        let mut tiles = vec![tile(13, true, true, None, &[1, 2]), tile(14, false, false, Some(0), &[]), tile(14, false, true, Some(0), &[])];
        keep_coarser_until_in(&mut tiles);
        assert_eq!(visible(&tiles), [0]);
    }

    #[test]
    fn an_empty_or_failed_tile_keeps_the_one_over_it_and_nothing_over_it_shows_what_is_under() {
        // a failed fine tile keeps its parent for good
        let mut tiles = vec![tile(13, false, true, None, &[1, 2]), tile(14, true, false, Some(0), &[]), tile(14, true, true, Some(0), &[])];
        keep_coarser_until_in(&mut tiles);
        assert_eq!(visible(&tiles), [0]);
        // nothing ready at or over a selected tile: its ready descendants, the first on each branch
        let mut tiles = vec![
            tile(12, true, false, None, &[1, 2]),
            tile(13, false, false, Some(0), &[3]),
            tile(13, false, true, Some(0), &[4]),
            tile(14, false, true, Some(1), &[]),
            tile(14, false, true, Some(2), &[]),
        ];
        keep_coarser_until_in(&mut tiles);
        assert_eq!(visible(&tiles), [2, 3]);
        // a tile not in the tree (`children: null`) is never marked visited below
        let mut tiles = vec![RefineTile { zoom: 3, is_selected: true, ready: true, ..RefineTile::default() }];
        keep_coarser_until_in(&mut tiles);
        assert_eq!(visible(&tiles), [0]);
    }

    #[test]
    fn each_zoom_from_its_source() {
        let sources = [
            TileSource { zooms: vec![9, 10, 11, 12], url: "https://a/{z}/{x}/{y}.pbf".into(), layers: vec!["low".into()] },
            TileSource { zooms: vec![13, 14, 16], url: "https://b/{z}/{x}/{-y}".into(), layers: vec!["high".into()] },
        ];
        assert_eq!(source_for(&sources, 10).map(|s| s.layers[0].as_str()), Some("low"));
        assert_eq!(source_for(&sources, 16).map(|s| s.layers[0].as_str()), Some("high"));
        assert_eq!(source_for(&sources, 15), None);
        let index = TileIndex { x: 35167, y: 20503, z: 16 };
        assert_eq!(url_from_template(&sources[0].url, index).as_deref(), Some("https://a/16/35167/20503.pbf"));
        assert_eq!(url_from_template(&sources[1].url, index).as_deref(), Some("https://b/16/35167/45032"));
        assert_eq!(url_from_template("", index), None);
    }

    #[test]
    fn oriented_boxes_are_math_gls() {
        // (every expected value is math.gl 4.1.0's for the same points, to the bit)
        let mut corners = Vec::new();
        for x in [1.0, 3.0] {
            for y in [-1.0, 5.0] {
                for z in [0.0, 0.5] {
                    corners.push([x, y, z]);
                }
            }
        }
        // the box of an axis-aligned cuboid's corners is the cuboid
        let b = oriented_box_from_points(&corners);
        assert_eq!(b, OrientedBox { center: [2.0, 2.0, 0.25], half_axes: [1.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 0.25] });
        assert_eq!(b.distance_to([2.0, 2.0, 3.25]), 2.75);
        assert_eq!(b.distance_to([2.0, 2.0, 0.0]), 0.0);
        let plane = Plane::new([0.0, 0.0, 2.0], -1.0);
        assert_eq!(plane.normal, [0.0, 0.0, 1.0]);
        assert_eq!(b.intersect_plane(&plane), Intersection::Outside);
        assert_eq!(b.intersect_plane(&Plane::new([0.0, 0.0, -1.0], 1.0)), Intersection::Inside);
        assert_eq!(b.intersect_plane(&Plane::new([1.0, 0.0, 0.0], -2.0)), Intersection::Intersecting);
        assert_eq!(oriented_box_from_points(&[]), OrientedBox { center: [0.0; 3], half_axes: [0.0; 9] });
        // a square on its corner: its covariance is round, so the axes stay the world's
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let square = oriented_box_from_points(&[[0.0, 0.0, 0.0], [s, s, 0.0], [-s, s, 0.0], [0.0, 2.0 * s, 0.0]]);
        assert_eq!(square, OrientedBox { center: [0.0, s, 0.0], half_axes: [s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 0.0] });
        // a cloud of points, through the Jacobi sweeps
        let cloud = oriented_box_from_points(&[[1.0, 2.0, 3.0], [-4.0, 0.5, 2.0], [3.0, -1.0, -2.0], [0.25, 4.0, 1.0], [2.0, 2.0, 2.5]]);
        assert_eq!(
            cloud,
            OrientedBox {
                center: [0.6539778052671859, 1.7725619006548885, 1.0312058138858036],
                half_axes: [
                    3.494254658472788,
                    -1.0597200599043195,
                    -1.8317806958330962,
                    0.24415172410977304,
                    1.9352959912715424,
                    -0.6538684937464075,
                    0.986084787035505,
                    0.4275601174109129,
                    1.6336766373397986,
                ],
            }
        );
        assert_eq!(cloud.distance_to([10.0, -3.0, 4.0]), 6.10640804391601);
    }

    /// A viewport as `crates/scene/tests/data/deck/views.json` records it (deck.gl's own numbers,
    /// S1.32), with `GlobeViewport.projectPosition` on the globe.
    struct Recorded {
        globe: bool,
        planes: [FrustumPlane; 6],
        camera: [f64; 3],
        upm: f64,
        scale: f64,
        zoom: f64,
        height: f64,
    }

    impl Recorded {
        fn from(v: &serde_json::Value) -> Self {
            let num = |v: &serde_json::Value| v.as_f64().expect("a number");
            let three = |v: &serde_json::Value| [num(&v[0]), num(&v[1]), num(&v[2])];
            let plane = |name: &str| {
                let p = &v["frustumPlanes"][name];
                FrustumPlane { normal: three(p), distance: num(&p[3]) }
            };
            Self {
                globe: v["viewport"] == "globe",
                planes: ["left", "right", "bottom", "top", "near", "far"].map(plane),
                camera: three(&v["cameraPosition"]),
                upm: num(&v["distanceScales"]["unitsPerMeter"][2]),
                scale: num(&v["scale"]),
                zoom: num(&v["zoom"]),
                height: num(&v["height"]),
            }
        }
    }

    impl LodViewport for Recorded {
        fn is_geospatial(&self) -> bool {
            true
        }
        fn is_globe(&self) -> bool {
            self.globe
        }
        fn project_position(&self, [lng, lat, z]: [f64; 3]) -> [f64; 3] {
            const EARTH_RADIUS: f64 = 6_370_972.0;
            const GLOBE_RADIUS: f64 = 256.0;
            let (lambda, phi) = (lng * DEG, lat * DEG);
            let cos_phi = phi.cos();
            let d = (z / EARTH_RADIUS + 1.0) * GLOBE_RADIUS;
            [lambda.sin() * cos_phi * d, -lambda.cos() * cos_phi * d, phi.sin() * d]
        }
        fn frustum_planes(&self) -> [FrustumPlane; 6] {
            self.planes
        }
        fn camera_position(&self) -> [f64; 3] {
            self.camera
        }
        fn units_per_meter(&self) -> f64 {
            self.upm
        }
        fn scale(&self) -> f64 {
            self.scale
        }
        fn zoom(&self) -> f64 {
            self.zoom
        }
        fn height(&self) -> f64 {
            self.height
        }
        fn longitude(&self) -> f64 {
            unreachable!("no look-ahead here")
        }
        fn latitude(&self) -> f64 {
            unreachable!("no look-ahead here")
        }
        fn ahead(&self, _: f64, _: f64, _: f64) -> Self {
            unreachable!("no look-ahead here")
        }
    }

    #[test]
    fn on_the_globe_and_the_flat_map_the_selection_is_the_typescripts() {
        // For each of deck.gl's 24 golden views, the TypeScript's tile count and list hash (as
        // `fnv`) under bun for three Lods: the terrain's (levels 0 to 16, bias 0.5), the same with
        // ground from -100 to 3000 m, and the realistic ground's (9 to 14 and 16, bias 0.5, at
        // most 80 km). The globe's are its oriented boxes and horizon; the flat map's its boxes.
        let expected: [(&str, [(usize, u32); 3]); 24] = [
            ("globe-z0.5-equator", [(4, 0x9dc7_be7f), (4, 0x9dc7_be7f), (0, 0x811c_9dc5)]),
            ("globe-z0.5-lat45-tilted", [(4, 0x9dc7_be7f), (4, 0x9dc7_be7f), (0, 0x811c_9dc5)]),
            ("globe-z1.5-lat60-phone", [(10, 0x77d7_cda6), (10, 0x77d7_cda6), (0, 0x811c_9dc5)]),
            ("globe-z2-lat80", [(18, 0xd3f5_b24a), (18, 0xd3f5_b24a), (0, 0x811c_9dc5)]),
            ("globe-z2.5-south45", [(32, 0x1bbf_5b17), (32, 0x1bbf_5b17), (0, 0x811c_9dc5)]),
            ("globe-z3-equator-pitch60", [(24, 0x0642_7f87), (24, 0x0642_7f87), (0, 0x811c_9dc5)]),
            ("globe-z4.5-antimeridian", [(40, 0xdb6b_0e4d), (40, 0xdb6b_0e4d), (0, 0x811c_9dc5)]),
            ("globe-z5-south60", [(57, 0x100c_e31f), (57, 0x100c_e31f), (0, 0x811c_9dc5)]),
            ("globe-z6.3-lat60", [(29, 0xe872_c790), (29, 0xe872_c790), (0, 0x811c_9dc5)]),
            ("globe-z7.7-equator-phone", [(8, 0xd2f1_8cdb), (8, 0xd2f1_8cdb), (0, 0x811c_9dc5)]),
            ("globe-z9.5-lat45-pitch60", [(65, 0x2a55_9425), (65, 0x2a55_9425), (14, 0x7e29_b214)]),
            ("globe-z12-lat80", [(67, 0xc170_25dd), (67, 0xc170_25dd), (65, 0x37b9_8992)]),
            ("mercator-z12.01-lat45", [(12, 0x5216_3d97), (27, 0x6b57_cabd), (12, 0x5216_3d97)]),
            ("mercator-z13-equator-pitch60", [(47, 0x5e2f_ef4a), (65, 0xe10e_8bae), (47, 0x5e2f_ef4a)]),
            ("mercator-z13.5-south45-phone", [(6, 0xde13_c748), (15, 0x097d_63e4), (6, 0xde13_c748)]),
            ("mercator-z14-lat60-pitch75", [(93, 0x8858_9326), (116, 0x3192_9392), (138, 0x2de1_d078)]),
            ("mercator-z15-lat80", [(28, 0x23e0_1079), (68, 0xf9b6_1ebb), (51, 0xfcd8_7022)]),
            ("mercator-z16-lat45-phone", [(4, 0xff8e_6d31), (4, 0xff8e_6d31), (4, 0xff8e_6d31)]),
            ("mercator-z17-lat60-raised", [(84, 0x6d8b_8dd3), (134, 0xa92c_1e68), (109, 0xf761_08f1)]),
            ("mercator-z18-equator", [(4, 0xfcb8_2cd9), (5, 0x07d5_8399), (4, 0xfcb8_2cd9)]),
            ("mercator-z19-lat45-pitch85", [(95, 0xcbfc_265b), (99, 0xe1d9_d7c1), (138, 0x4668_32c7)]),
            ("mercator-z20-lat80", [(2, 0x11fc_1cc8), (30, 0x84bb_3cfb), (2, 0x11fc_1cc8)]),
            ("mercator-z20-antimeridian-south60", [(1, 0xd208_c24f), (1, 0xd208_c24f), (1, 0xd208_c24f)]),
            ("mercator-z23-origin-phone", [(8, 0xc287_9efb), (10, 0x008f_d239), (8, 0xc287_9efb)]),
        ];
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/deck/views.json")).expect("views.json");
        let file: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        let views = file["views"].as_array().expect("views");
        assert_eq!(views.len(), expected.len());
        let surface = Lod { bias: 0.5, ..Lod::new((0..=16).collect::<Vec<u32>>()) };
        let ground = Lod { bias: 0.5, max_distance: 80_000.0, ..Lod::new([9, 10, 11, 12, 13, 14, 16]) };
        for (v, (name, cells)) in views.iter().zip(expected) {
            assert_eq!(v["name"], name);
            let viewport = Recorded::from(v);
            let got = [
                distance_tile_indices(&viewport, &surface, None, 0.0),
                distance_tile_indices(&viewport, &surface, Some([-100.0, 3000.0]), 0.0),
                distance_tile_indices(&viewport, &ground, None, 0.0),
            ]
            .map(|tiles| (tiles.len(), fnv(&tiles)));
            assert_eq!(got, cells, "{name}");
        }
    }
}
