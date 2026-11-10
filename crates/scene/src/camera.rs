//! The camera: deck.gl 9.4's `GlobeView` as the app uses it (doc 01 §2.1–2.4), in f64.
//!
//! - [`ViewState`] is the camera as the controller and the app hold it.
//! - [`viewport_for`] makes the viewport for a view and a canvas size: a [`GlobeViewport`] (a sphere
//!   under a perspective camera) up to zoom 12, a [`MercatorViewport`] (the flat Web Mercator map,
//!   `@math.gl/web-mercator`'s matrices) above it, exactly as `GlobeView.getViewportType` swaps.
//! - Both carry deck's matrices, `project` / `unproject`, frustum planes and bounds; [`Viewport`]
//!   holds either one.
//! - [`limits`], [`follow_max_zoom`], [`max_zoom_now`], [`camera_distance_m`] and
//!   [`camera_altitude_m`] are `Globe.svelte`'s camera helpers (G:265-306, 3111, 3604-3605).
//!
//! Transliterated from `@deck.gl/core/dist/viewports/{viewport,globe-viewport,web-mercator-viewport}.js`
//! and `@math.gl/web-mercator`, with the gl-matrix functions they call (`lookAt`, `perspective`,
//! `rotateX`, `multiply`, `invert`, ...) ported operation for operation on column-major `[f64; 16]`
//! arrays, so the results agree with deck.gl's to the last few bits (`tests/data/deck/views.json`,
//! S1.32, pins them). Matrices are handed out as `glam::DMat4` (column-major, acting on column
//! vectors, like math.gl's).
//!
//! Conventions, as deck.gl's:
//! - Geographic positions are `[lon, lat, metres up]` in degrees.
//! - "Common space" is deck's world space: on the globe a sphere of radius 256 units centred at
//!   the origin (lon 0 / lat 0 at `(0, −256, 0)`, the north pole at `(0, 0, 256)`); on the flat map
//!   512 units round the world with y growing **north**.
//! - Pixels are CSS pixels from the top left, y down (deck's `topLeft: true`, the only way the app
//!   calls it; the `topLeft: false` option is not ported).
//!
//! Two traps of the flat map, both deck's: `project` turns metres into units with
//! [`mercator_units_per_meter`] at the **point's** latitude, while `unproject`'s `target_z` (and
//! [`ViewState::position`]) use the view centre's; and that scale is math.gl's average
//! circumference 40 030 km ([`MERCATOR_EARTH_CIRCUMFERENCE_M`]), not the 40 075 016.686 m of
//! [`meters_per_pixel`].

use glam::{DMat4, DVec3};
use rustradar_core::geo::Basemap;

use crate::screen::Projector;

/// Ground resolution at the view centre (`geo.ts` `metersPerPixel`): metres per pixel of a 512 px
/// world at zoom 0, with the Web Mercator equator of 40 075 016.686 m. Both projections share it at
/// the view centre.
pub use rustradar_core::geo::meters_per_pixel;

/// `Math.PI / 180` (deck's `DEGREES_TO_RADIANS`).
const DEG: f64 = std::f64::consts::PI / 180.0;
/// `180 / Math.PI` (deck's `RADIANS_TO_DEGREES`): radians come back by multiplying with this, as in
/// deck, not by dividing by [`DEG`].
const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;
const PI: f64 = std::f64::consts::PI;

/// GlobeView switches to the flat Web Mercator projection above this zoom (G:266; deck's
/// `GlobeView.getViewportType`: `zoom > 12`).
pub const FLAT_ABOVE_ZOOM: f64 = 12.0;
/// deck.gl parks the camera this many viewport heights back from its target (G:3072; deck's
/// `altitude = 1.5`).
pub const CAMERA_HEIGHTS: f64 = 1.5;
/// The globe's radius in common units (deck `GLOBE_RADIUS`).
pub const GLOBE_RADIUS: f64 = 256.0;
/// The Earth's radius the globe viewport uses, metres (deck `globe-viewport.js` `EARTH_RADIUS`).
pub const GLOBE_EARTH_RADIUS_M: f64 = 6_370_972.0;
/// math.gl's Web Mercator circumference ("average circumference (40075 km equatorial, 40007 km
/// meridional)"): the flat map's metres-to-units scale for heights and `position`.
pub const MERCATOR_EARTH_CIRCUMFERENCE_M: f64 = 40.03e6;
/// The flat map's world is 512 units wide (math.gl `TILE_SIZE`).
pub const WORLD_SIZE: f64 = 512.0;
/// The latitude that makes the Mercator world square, `2·atan(e^π) − π/2` (math.gl `MAX_LATITUDE`):
/// the globe's scale and `zoom_adjust(…, true)` clamp latitude to it.
pub const MAX_LATITUDE: f64 = 85.051129;
/// The globe tessellation step in degrees (`new GlobeView({ resolution: 5 })`, G:3687).
pub const GLOBE_RESOLUTION: f64 = 5.0;

/// The globe's near plane in viewport heights (deck `GlobeViewport` `nearZMultiplier`).
const GLOBE_NEAR_Z_MULTIPLIER: f64 = 0.5;
/// The globe's far plane multiplier (deck `GlobeViewport` `farZMultiplier`).
const GLOBE_FAR_Z_MULTIPLIER: f64 = 1.0;
/// The flat map's near plane (deck `WebMercatorViewport` `nearZMultiplier`).
const MERCATOR_NEAR_Z_MULTIPLIER: f64 = 0.1;
/// The flat map's far plane multiplier (deck `WebMercatorViewport` `farZMultiplier`).
const MERCATOR_FAR_Z_MULTIPLIER: f64 = 1.01;
/// Wheel-zoom anchoring on the globe starts to weaken where the pointer's ray passes this far from
/// the globe's centre, in radii (deck `GLOBE_ZOOM_ANCHOR_DAMPING_START_RATIO`)...
const GLOBE_ZOOM_ANCHOR_DAMPING_START_RATIO: f64 = 0.75;
/// ...and is gone from here (deck `GLOBE_ZOOM_ANCHOR_MAX_DISTANCE_RATIO`).
const GLOBE_ZOOM_ANCHOR_MAX_DISTANCE_RATIO: f64 = 1.15;
/// gl-matrix's `glMatrix.EPSILON`, used by `lookAt`.
const GL_MATRIX_EPSILON: f64 = 0.000001;

// ---------------------------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------------------------

/// The camera (`Globe.svelte` `ViewState`, doc 01 §1.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewState {
    /// Degrees, kept in [-180, 180] by the controller.
    pub longitude: f64,
    /// Degrees, clamped to [-85, 85] by the app's own pans (deck allows ±90).
    pub latitude: f64,
    /// deck.gl zoom: the world is 512 px round at the equator at zoom 0.
    pub zoom: f64,
    /// Degrees clockwise from north.
    pub bearing: f64,
    /// Degrees from straight down: 0 looks at the ground below.
    pub pitch: f64,
    /// Metres; only `[0, 0, z]` is used: the camera's target raised `z` metres (locked on an
    /// aircraft, aimed at terrain). `None` is deck's `position: undefined`.
    ///
    /// deck applies it in both projections, but only the flat one has a use for it: on the globe
    /// it moves the view centre along the polar axis (common z). The app leaves it unset there,
    /// except while the terrain aim eases a raised target back to the ground after a zoom out past
    /// 12 (G:3204-3212), and deck then shifts the centre that way; this port does the same.
    pub position: Option<[f64; 3]>,
}

impl ViewState {
    /// The view the app opens with (G:314): the whole globe, Europe and Africa in the middle.
    pub const INITIAL: ViewState =
        ViewState { longitude: 10.0, latitude: 35.0, zoom: 1.9, bearing: 0.0, pitch: 0.0, position: None };

    /// Whether this view draws the flat map (`zoom > FLAT_ABOVE_ZOOM`; G:2525 `flat`).
    pub fn is_flat(&self) -> bool {
        is_flat(self.zoom)
    }
}

/// Whether a zoom draws the flat map rather than the globe (`const flat = zoom > FLAT_ABOVE_ZOOM`).
pub fn is_flat(zoom: f64) -> bool {
    zoom > FLAT_ABOVE_ZOOM
}

// ---------------------------------------------------------------------------------------------
// Limits and the camera's distance (Globe.svelte)
// ---------------------------------------------------------------------------------------------

/// The zoom and pitch limits handed to deck.gl's controller (`{ minZoom, maxZoom, maxPitch }`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub max_pitch: f64,
}

/// The free camera on the dark and satellite maps (G:267 `LIMITS`).
pub const LIMITS: Limits = Limits { min_zoom: 0.5, max_zoom: 20.0, max_pitch: 60.0 };
/// The realistic map tilts further, to look along the streets to the horizon and the sky over it
/// (G:269 `REALISTIC_LIMITS`).
pub const REALISTIC_LIMITS: Limits = Limits { max_pitch: 75.0, ..LIMITS };
/// deck's default `minPitch`.
pub const MIN_PITCH: f64 = 0.0;
/// Following an aircraft, the camera can come down nearly level with it and circle it (G:271).
pub const FOLLOW_MAX_PITCH: f64 = 85.0;
/// ...and close in until the camera is this many of its lengths away, so it can't end up inside
/// it (G:273).
pub const FOLLOW_CLOSEST: f64 = 1.3;
/// Past this the map tiles have long run out of detail (G:275).
pub const FOLLOW_MAX_ZOOM: f64 = 23.0;
/// The canvas height `followMaxZoom` assumes before the map has one (`container?.clientHeight ?? 900`).
const FALLBACK_HEIGHT_PX: f64 = 900.0;

/// What [`limits`] reads of a camera locked on an aircraft (`follow && selected`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowLock {
    /// The selected flight's latitude (`selected.lat`), degrees.
    pub latitude: f64,
    /// [`follow_max_zoom`] for it.
    pub max_zoom: f64,
    /// How far the locked camera may tilt right now (`lockedMaxPitch`, set by `lockedView`;
    /// [`FOLLOW_MAX_PITCH`] on the flat map).
    pub max_pitch: f64,
}

/// The camera's limits, as handed to deck.gl (`limits()`, G:294-300). Locked on an aircraft they
/// open up: close in until it fills the screen and tilt down to almost level.
///
/// GlobeController shifts its zoom limits by latitude (`_constrainZoom` adds
/// `zoom_adjust(lat) − zoom_adjust(0) = log2(cos lat)`); the locked maximum undoes that so the
/// aircraft's limit is the zoom actually reached. The free limits are left shifted, as today
/// (doc 01 §2.4: at 60°N the free range is −0.5 … 19).
pub fn limits(basemap: Basemap, lock: Option<&FollowLock>) -> Limits {
    let Some(lock) = lock else {
        return if basemap == Basemap::Realistic { REALISTIC_LIMITS } else { LIMITS };
    };
    let shift = js_max(0.05, (lock.latitude * DEG).cos()).log2();
    Limits { min_zoom: LIMITS.min_zoom, max_zoom: lock.max_zoom - shift, max_pitch: lock.max_pitch }
}

/// The closest zoom locked on an aircraft (`followMaxZoom`, G:281-287): until it fills the screen,
/// worked out from its real size, so the camera can't end up inside it. Between
/// [`LIMITS`]`.max_zoom` (20) and [`FOLLOW_MAX_ZOOM`] (23).
///
/// `size_m` is the aircraft's largest dimension in metres: its model's size, else its icon class's
/// (`aircraftModels.forFlight(typecode, icon)?.size ?? sizeOf(icon)`;
/// [`crate::aircraft_assets`], [`crate::aircraft_models::size_of`]). `latitude` is the flight's;
/// `container_height_px` the map's height in CSS pixels, `None` before it has one (900 is assumed).
pub fn follow_max_zoom(size_m: f64, latitude: f64, container_height_px: Option<f64>) -> f64 {
    // the camera sits CAMERA_HEIGHTS viewport heights back from its target
    let closest = (FOLLOW_CLOSEST * size_m) / (CAMERA_HEIGHTS * js_max(1.0, container_height_px.unwrap_or(FALLBACK_HEIGHT_PX)));
    let zoom = (meters_per_pixel(0.0, latitude) / closest).log2();
    js_min(FOLLOW_MAX_ZOOM, js_max(LIMITS.max_zoom, zoom))
}

/// The closest zoom reachable now, in the view's own terms (`maxZoomNow`, G:302):
/// [`follow_max_zoom`] of the selected flight while locked on it (`Some`), else 20.
pub fn max_zoom_now(follow_max_zoom: Option<f64>) -> f64 {
    follow_max_zoom.unwrap_or(LIMITS.max_zoom)
}

/// How far the camera is from its target, metres (`CAMERA_HEIGHTS · clientHeight ·
/// metersPerPixel(zoom, lat)`, G:3111, 2697; doc 01 §2.1). `height_px` is the map's height in CSS
/// pixels.
pub fn camera_distance_m(zoom: f64, latitude: f64, height_px: f64) -> f64 {
    CAMERA_HEIGHTS * height_px * meters_per_pixel(zoom, latitude)
}

/// The camera's height above the ground, metres: its distance from the middle of the view and its
/// tilt (`cameraAltitude`, G:3604-3605). The TypeScript gives `Infinity` before the map has a
/// size; here the caller has one.
pub fn camera_altitude_m(zoom: f64, latitude: f64, pitch: f64, height_px: f64) -> f64 {
    CAMERA_HEIGHTS * height_px * meters_per_pixel(zoom, latitude) * (pitch * DEG).cos()
}

/// deck's `zoomAdjust(latitude, clampToPoles)`: `log2(π·cos lat)`. The globe is drawn
/// `2^(zoom − zoom_adjust(lat))` px per common unit, so that it matches Web Mercator's scale at the
/// view centre; with `clamp_to_poles` the latitude is first clamped to ±[`MAX_LATITUDE`]
/// (GlobeController's zoom limits and pan use that form).
pub fn zoom_adjust(latitude: f64, clamp_to_poles: bool) -> f64 {
    let latitude = if clamp_to_poles { js_max(js_min(latitude, MAX_LATITUDE), -MAX_LATITUDE) } else { latitude };
    let scale_adjust = PI * ((latitude * PI) / 180.0).cos();
    scale_adjust.log2()
}

/// math.gl's `unitsPerMeter(latitude)`: flat-map common units per metre at a latitude (with the
/// 40 030 km circumference).
pub fn mercator_units_per_meter(latitude: f64) -> f64 {
    let lat_cosine = (latitude * DEG).cos();
    WORLD_SIZE / MERCATOR_EARTH_CIRCUMFERENCE_M / lat_cosine
}

/// math.gl's `lngLatToWorld`: `[lon, lat]` to the flat map's common space (512 units round, y
/// north). Unclamped; [`MercatorViewport::project_flat`] clamps y as deck does.
pub fn lng_lat_to_world([lng, lat]: [f64; 2]) -> [f64; 2] {
    let lambda2 = lng * DEG;
    let phi2 = lat * DEG;
    let x = (WORLD_SIZE * (lambda2 + PI)) / (2.0 * PI);
    let y = (WORLD_SIZE * (PI + (PI / 4.0 + phi2 * 0.5).tan().ln())) / (2.0 * PI);
    [x, y]
}

/// math.gl's `worldToLngLat`: the flat map's common space back to `[lon, lat]`.
pub fn world_to_lng_lat([x, y]: [f64; 2]) -> [f64; 2] {
    let lambda2 = (x / WORLD_SIZE) * (2.0 * PI) - PI;
    let phi2 = 2.0 * (((y / WORLD_SIZE) * (2.0 * PI) - PI).exp().atan() - PI / 4.0);
    [lambda2 * RAD_TO_DEG, phi2 * RAD_TO_DEG]
}

/// math.gl's `altitudeToFovy`: the vertical field of view, degrees, that puts the focal plane
/// `altitude` viewport heights from the camera.
pub fn altitude_to_fovy(altitude: f64) -> f64 {
    2.0 * (0.5 / altitude).atan() * RAD_TO_DEG
}

/// math.gl's `fovyToAltitude`.
pub fn fovy_to_altitude(fovy: f64) -> f64 {
    0.5 / (0.5 * fovy * DEG).tan()
}

// ---------------------------------------------------------------------------------------------
// Viewports
// ---------------------------------------------------------------------------------------------

/// deck's `distanceScales`, each `[x, y, z]`: on the globe constant, on the flat map those of the
/// view centre's latitude (math.gl `getDistanceScales`, without `highPrecision`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistanceScales {
    pub units_per_meter: [f64; 3],
    pub meters_per_unit: [f64; 3],
    pub units_per_degree: [f64; 3],
    pub degrees_per_unit: [f64; 3],
}

impl DistanceScales {
    /// `GlobeViewport`'s `getDistanceScales()`.
    fn globe() -> Self {
        let units_per_meter = GLOBE_RADIUS / GLOBE_EARTH_RADIUS_M;
        let units_per_degree = (PI / 180.0) * GLOBE_RADIUS;
        DistanceScales {
            units_per_meter: [units_per_meter; 3],
            meters_per_unit: [1.0 / units_per_meter; 3],
            units_per_degree: [units_per_degree, units_per_degree, units_per_meter],
            degrees_per_unit: [1.0 / units_per_degree, 1.0 / units_per_degree, 1.0 / units_per_meter],
        }
    }

    /// math.gl's `getDistanceScales({ latitude, longitude })`.
    fn mercator(latitude: f64) -> Self {
        let world_size = WORLD_SIZE;
        let lat_cosine = (latitude * DEG).cos();
        let units_per_degree_x = world_size / 360.0;
        let units_per_degree_y = units_per_degree_x / lat_cosine;
        let alt_units_per_meter = world_size / MERCATOR_EARTH_CIRCUMFERENCE_M / lat_cosine;
        DistanceScales {
            units_per_meter: [alt_units_per_meter; 3],
            meters_per_unit: [1.0 / alt_units_per_meter; 3],
            units_per_degree: [units_per_degree_x, units_per_degree_y, alt_units_per_meter],
            degrees_per_unit: [1.0 / units_per_degree_x, 1.0 / units_per_degree_y, 1.0 / alt_units_per_meter],
        }
    }
}

/// One frustum plane in common space, as deck's `getFrustumPlanes` returns it: the normal points
/// **out** of the frustum, and a point `p` is inside when `normal · p < distance` (deck's tile
/// traversal negates the normal before it culls: `new Plane(normal.negate(), distance)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: DVec3,
    pub distance: f64,
}

/// The six frustum planes (deck `getFrustumPlanes`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrustumPlanes {
    pub left: Plane,
    pub right: Plane,
    pub bottom: Plane,
    pub top: Plane,
    pub near: Plane,
    pub far: Plane,
}

impl FrustumPlanes {
    /// The planes in deck's object order (`Object.values(viewport.getFrustumPlanes())`): left,
    /// right, bottom, top, near, far.
    pub fn all(&self) -> [Plane; 6] {
        [self.left, self.right, self.bottom, self.top, self.near, self.far]
    }
}

/// What both viewports share: deck's base `Viewport` (its fields and `_initMatrices`).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewportBase {
    /// Canvas size in CSS pixels (deck's `width || 1`, `height || 1`).
    pub width: f64,
    pub height: f64,
    pub longitude: f64,
    /// On the globe clamped to ±90 (deck clamps it there).
    pub latitude: f64,
    /// The view's zoom, as given.
    pub zoom: f64,
    pub pitch: f64,
    pub bearing: f64,
    /// Pixels per common unit at the view centre: on the globe `2^(zoom − zoom_adjust(lat))`, on the
    /// flat map `2^zoom`.
    pub scale: f64,
    /// Vertical field of view, degrees (`2·atan(1/3)`, about 36.87°).
    pub fovy: f64,
    /// The camera's distance from its target in viewport heights (deck's `altitude`, 1.5).
    pub focal_distance: f64,
    /// Near and far planes in viewport heights, as given to the projection matrix.
    pub near: f64,
    pub far: f64,
    pub distance_scales: DistanceScales,
    /// The view centre in common space (`position` included).
    pub center: DVec3,
    /// The camera in common space (the translation of the inverse view matrix).
    pub camera_position: DVec3,
    /// The view matrix before the centre is subtracted: it acts on common space relative to the
    /// view centre.
    pub view_matrix_uncentered: DMat4,
    /// `view_matrix_uncentered · translate(−center)`.
    pub view_matrix: DMat4,
    pub projection_matrix: DMat4,
    /// `projection_matrix · view_matrix`: common space to clip space.
    pub view_projection_matrix: DMat4,
    /// `scale(width/2, −height/2, 1) · translate(1, −1, 0) · view_projection_matrix`: common space
    /// to pixels (y down; z stays NDC depth after the divide).
    pub pixel_projection_matrix: DMat4,
    /// Its inverse.
    pub pixel_unprojection_matrix: DMat4,
}

impl ViewportBase {
    /// deck's `Viewport` constructor once the subclass has made its view matrix and planes: the
    /// centre (`_initProps`) and the matrices (`_initMatrices`).
    #[allow(clippy::too_many_arguments)]
    fn new(
        view: &ViewState,
        width: f64,
        height: f64,
        latitude: f64,
        zoom: f64,
        scale: f64,
        fovy: f64,
        focal_distance: f64,
        near: f64,
        far: f64,
        distance_scales: DistanceScales,
        center_on_ground: [f64; 3],
        view_matrix_uncentered: M4,
    ) -> Self {
        // _initProps: the centre, raised by `position` (metres to units elementwise)
        let meter_offset = view.position.unwrap_or([0.0; 3]);
        let upm = distance_scales.units_per_meter;
        let center = [
            meter_offset[0] * upm[0] + center_on_ground[0],
            meter_offset[1] * upm[1] + center_on_ground[1],
            meter_offset[2] * upm[2] + center_on_ground[2],
        ];
        // _initMatrices
        let view_matrix = translate(&multiply(&IDENTITY, &view_matrix_uncentered), [-center[0], -center[1], -center[2]]);
        let projection_matrix = perspective(fovy * DEG, width / height, near, far);
        let vpm = multiply(&multiply(&IDENTITY, &projection_matrix), &view_matrix);
        let view_matrix_inverse = invert(&view_matrix).unwrap_or(view_matrix);
        let camera_position = [view_matrix_inverse[12], view_matrix_inverse[13], view_matrix_inverse[14]];
        let viewport_matrix = translate(&scale3(&IDENTITY, [width / 2.0, -height / 2.0, 1.0]), [1.0, -1.0, 0.0]);
        let pixel_projection_matrix = multiply(&viewport_matrix, &vpm);
        // deck warns and keeps `null` (its unproject then throws); here every unprojection is NaN
        let pixel_unprojection_matrix = invert(&pixel_projection_matrix).unwrap_or([f64::NAN; 16]);
        ViewportBase {
            width,
            height,
            longitude: view.longitude,
            latitude,
            zoom,
            pitch: view.pitch,
            bearing: view.bearing,
            scale,
            fovy,
            focal_distance,
            near,
            far,
            distance_scales,
            center: DVec3::from_array(center),
            camera_position: DVec3::from_array(camera_position),
            view_matrix_uncentered: DMat4::from_cols_array(&view_matrix_uncentered),
            view_matrix: DMat4::from_cols_array(&view_matrix),
            projection_matrix: DMat4::from_cols_array(&projection_matrix),
            view_projection_matrix: DMat4::from_cols_array(&vpm),
            pixel_projection_matrix: DMat4::from_cols_array(&pixel_projection_matrix),
            pixel_unprojection_matrix: DMat4::from_cols_array(&pixel_unprojection_matrix),
        }
    }

    /// deck's `metersPerPixel`: `metersPerUnit[2] / scale`. On the globe this is
    /// [`meters_per_pixel`] with deck's radius; on the flat map it uses math.gl's circumference.
    pub fn meters_per_pixel(&self) -> f64 {
        self.distance_scales.meters_per_unit[2] / self.scale
    }

    /// math.gl's `worldToPixels`: a common-space position to `[x, y, depth]` pixels.
    pub fn world_to_pixels(&self, world: DVec3) -> [f64; 3] {
        let p = transform_vector(&self.pixel_projection_matrix.to_cols_array(), [world.x, world.y, world.z, 1.0]);
        [p[0], p[1], p[2]]
    }

    /// The pixel `[x, y, depth]` back in common space (the path of `unproject` with a depth).
    pub fn pixels_to_world_with_depth(&self, [x, y, depth]: [f64; 3]) -> DVec3 {
        let p = transform_vector(&self.pixel_unprojection_matrix.to_cols_array(), [x, y, depth, 1.0]);
        DVec3::new(p[0], p[1], p[2])
    }

    /// deck's `getFrustumPlanes` (Gribb and Hartmann's extraction from the view-projection matrix).
    pub fn frustum_planes(&self) -> FrustumPlanes {
        let m = self.view_projection_matrix.to_cols_array();
        FrustumPlanes {
            left: frustum_plane(m[3] + m[0], m[7] + m[4], m[11] + m[8], m[15] + m[12]),
            right: frustum_plane(m[3] - m[0], m[7] - m[4], m[11] - m[8], m[15] - m[12]),
            bottom: frustum_plane(m[3] + m[1], m[7] + m[5], m[11] + m[9], m[15] + m[13]),
            top: frustum_plane(m[3] - m[1], m[7] - m[5], m[11] - m[9], m[15] - m[13]),
            near: frustum_plane(m[3] + m[2], m[7] + m[6], m[11] + m[10], m[15] + m[14]),
            far: frustum_plane(m[3] - m[2], m[7] - m[6], m[11] - m[10], m[15] - m[14]),
        }
    }
}

/// deck's `getFrustumPlane(a, b, c, d)`.
fn frustum_plane(a: f64, b: f64, c: f64, d: f64) -> Plane {
    let l = (a * a + b * b + c * c).sqrt();
    Plane { normal: DVec3::new(-a / l, -b / l, -c / l), distance: d / l }
}

/// The globe: deck 9.4's `GlobeViewport`, used up to zoom 12 (doc 01 §2.2).
///
/// ```text
/// V = lookAt(eye = [0, −1.5, 0], center = 0, up = [0, 0, 1])
///     · Rx(−pitch) · Ry(−bearing) · Rx(latitude) · Rz(−longitude) · S(scale / height)
/// P = perspective(fovy, width / height, near 0.5, far 1.5 + (512·scale / height) / max(cos pitch, 0.1))
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct GlobeViewport {
    pub base: ViewportBase,
    /// Tessellation step in degrees ([`GLOBE_RESOLUTION`]).
    pub resolution: f64,
}

impl std::ops::Deref for GlobeViewport {
    type Target = ViewportBase;
    fn deref(&self) -> &ViewportBase {
        &self.base
    }
}

/// What `_getRayToGlobe` works out for a pixel.
struct RayToGlobe {
    ray_start: [f64; 4],
    ray_end: [f64; 4],
    radius: f64,
    ray_length_squared: f64,
    ray_start_distance_squared: f64,
    distance_to_center_squared: f64,
}

impl GlobeViewport {
    /// `new GlobeViewport({ ...view, width, height, resolution: 5 })`.
    pub fn new(view: &ViewState, width: f64, height: f64) -> Self {
        let altitude = CAMERA_HEIGHTS;
        let longitude = view.longitude;
        let bearing = view.bearing;
        let pitch = view.pitch;
        let zoom = view.zoom;
        // clamp to the valid range
        let latitude = js_max(js_min(view.latitude, 90.0), -90.0);
        let height = js_or1(height);
        let width = js_or1(width);
        let fovy = altitude_to_fovy(altitude);
        // the globe is enlarged by 1/cos(lat) to match Web Mercator's scale at the centre; the
        // latitude is capped for this, away from the poles' singularity
        let scale_latitude = js_max(js_min(latitude, MAX_LATITUDE), -MAX_LATITUDE);
        let scale = 2f64.powf(zoom - zoom_adjust(scale_latitude, false));
        // a tilted camera sees further across the globe
        let pitch_radians = pitch * DEG;
        let near = GLOBE_NEAR_Z_MULTIPLIER;
        let far = (altitude + (GLOBE_RADIUS * 2.0 * scale) / height / js_max(pitch_radians.cos(), 0.1)) * GLOBE_FAR_Z_MULTIPLIER;
        // the camera along −Y looking at the origin; after Rx(lat)·Rz(−lng) the target's surface
        // normal is −Y, east +X and north +Z
        let mut vm = look_at([0.0, -altitude, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        vm = rotate_x(&vm, -pitch_radians);
        vm = rotate_y(&vm, -bearing * DEG);
        vm = rotate_x(&vm, latitude * DEG);
        vm = rotate_z(&vm, -longitude * DEG);
        let s = scale / height;
        vm = scale3(&vm, [s, s, s]);
        let center_on_ground = Self::project_position_static([longitude, latitude, 0.0]);
        let base = ViewportBase::new(
            view,
            width,
            height,
            latitude,
            // the base Viewport's `opts.zoom || 0`
            js_or0(zoom),
            scale,
            fovy,
            altitude,
            near,
            far,
            DistanceScales::globe(),
            center_on_ground,
            vm,
        );
        GlobeViewport { base, resolution: GLOBE_RESOLUTION }
    }

    /// deck's `GlobeViewport.projectPosition`: `[lon, lat, metres]` to common space.
    ///
    /// ```text
    /// D = (z / 6 370 972 + 1) · 256
    /// x = sin λ · cos φ · D,  y = −cos λ · cos φ · D,  z = sin φ · D
    /// ```
    pub fn project_position(&self, xyz: [f64; 3]) -> DVec3 {
        DVec3::from_array(Self::project_position_static(xyz))
    }

    fn project_position_static([lng, lat, z]: [f64; 3]) -> [f64; 3] {
        let lambda = lng * DEG;
        let phi = lat * DEG;
        let cos_phi = phi.cos();
        let d = (z / GLOBE_EARTH_RADIUS_M + 1.0) * GLOBE_RADIUS;
        [lambda.sin() * cos_phi * d, -lambda.cos() * cos_phi * d, phi.sin() * d]
    }

    /// deck's `GlobeViewport.unprojectPosition`: common space to `[lon, lat, metres]`.
    pub fn unproject_position(&self, xyz: DVec3) -> [f64; 3] {
        let d = (xyz.x * xyz.x + xyz.y * xyz.y + xyz.z * xyz.z).sqrt();
        let phi = (xyz.z / d).asin();
        let lambda = xyz.x.atan2(-xyz.y);
        [lambda * RAD_TO_DEG, phi * RAD_TO_DEG, (d / GLOBE_RADIUS - 1.0) * GLOBE_EARTH_RADIUS_M]
    }

    /// deck's `project`: `[lon, lat, metres]` to `[x, y, depth]` pixels (depth is NDC z).
    pub fn project(&self, xyz: [f64; 3]) -> [f64; 3] {
        self.base.world_to_pixels(self.project_position(xyz))
    }

    /// deck's `unproject([x, y], { targetZ })`: the pixel's ray against the sphere raised
    /// `target_z` metres, `[lon, lat]` of the near intersection.
    ///
    /// A ray that misses the globe gives the point of its closest approach to the globe's centre
    /// (deck clamps the discriminant at 0), never NaN. That is not exactly on the limb, but beside
    /// it for a pixel just off the globe, and on the screen line from the globe's centre to the
    /// pixel (doc 01 §2.2, the golden records of `views.json`).
    pub fn unproject(&self, xy: [f64; 2], target_z: f64) -> [f64; 2] {
        let RayToGlobe { ray_start, ray_end, radius, ray_length_squared, ray_start_distance_squared, distance_to_center_squared } =
            self.ray_to_globe(xy, target_z);
        let ray_start_to_closest_approach = (ray_start_distance_squared - distance_to_center_squared).sqrt();
        let closest_approach_to_intersection = js_max(0.0, radius * radius - distance_to_center_squared).sqrt();
        let intersection_ratio = (ray_start_to_closest_approach - closest_approach_to_intersection) / ray_length_squared.sqrt();
        let coord = lerp3(ray_start, ray_end, intersection_ratio);
        let [x, y, _] = self.unproject_position(DVec3::from_array(coord));
        [x, y]
    }

    /// deck's `unproject([x, y, depth])`: the pixel at a known depth, `[lon, lat, metres]`.
    pub fn unproject_depth(&self, xyz: [f64; 3]) -> [f64; 3] {
        self.unproject_position(self.base.pixels_to_world_with_depth(xyz))
    }

    /// deck's `_getRayToGlobe`: the pixel's ray from the near plane to the far plane, and how
    /// close it passes to the globe's centre (from the triangle it makes with the centre).
    fn ray_to_globe(&self, [screen_x, screen_y]: [f64; 2], target_z: f64) -> RayToGlobe {
        let pum = self.pixel_unprojection_matrix.to_cols_array();
        let ray_start = transform_vector(&pum, [screen_x, screen_y, -1.0, 1.0]);
        let ray_end = transform_vector(&pum, [screen_x, screen_y, 1.0, 1.0]);
        let radius = (js_or0(target_z) / GLOBE_EARTH_RADIUS_M + 1.0) * GLOBE_RADIUS;
        let ray_length_squared =
            sqr_len3([ray_start[0] - ray_end[0], ray_start[1] - ray_end[1], ray_start[2] - ray_end[2], 0.0]);
        let ray_start_distance_squared = sqr_len3(ray_start);
        let ray_end_distance_squared = sqr_len3(ray_end);
        let triangle_area_squared = (4.0 * ray_start_distance_squared * ray_end_distance_squared
            - (ray_length_squared - ray_start_distance_squared - ray_end_distance_squared).powi(2))
            / 16.0;
        let distance_to_center_squared = (4.0 * triangle_area_squared) / ray_length_squared;
        RayToGlobe {
            ray_start,
            ray_end,
            radius,
            ray_length_squared,
            ray_start_distance_squared,
            distance_to_center_squared,
        }
    }

    /// deck's `getZoomAnchorStrength`: how strongly a wheel zoom at this pixel stays anchored to the
    /// point under it, 1 on the visible globe, easing (smoothstep) to 0 where the pointer's ray
    /// passes 0.75 … 1.15 radii from the globe's centre (doc 01 §3.5).
    pub fn zoom_anchor_strength(&self, xy: [f64; 2]) -> f64 {
        let distance_ratio = self.ray_distance_to_globe_center_ratio(xy);
        if distance_ratio >= GLOBE_ZOOM_ANCHOR_MAX_DISTANCE_RATIO {
            return 0.0;
        }
        let edge_progress = js_max(
            0.0,
            js_min(
                1.0,
                (distance_ratio - GLOBE_ZOOM_ANCHOR_DAMPING_START_RATIO)
                    / (GLOBE_ZOOM_ANCHOR_MAX_DISTANCE_RATIO - GLOBE_ZOOM_ANCHOR_DAMPING_START_RATIO),
            ),
        );
        let smooth_progress = edge_progress * edge_progress * (3.0 - 2.0 * edge_progress);
        1.0 - smooth_progress
    }

    /// deck's `_getRayDistanceToGlobeCenterRatio`: how far the pixel's ray passes from the globe's
    /// centre, in globe radii (below 1 it hits the globe).
    pub fn ray_distance_to_globe_center_ratio(&self, xy: [f64; 2]) -> f64 {
        let ray = self.ray_to_globe(xy, 0.0);
        js_max(0.0, ray.distance_to_center_squared).sqrt() / ray.radius
    }

    /// deck's `GlobeViewport.getBounds({ z })`: `[min lon, min lat, max lon, max lat]` from the
    /// midpoints of the four edges unprojected at `z` metres (off the globe, as [`Self::unproject`]
    /// gives it).
    pub fn bounds(&self, z: f64) -> [f64; 4] {
        let target_z = js_or0(z);
        let mut left = self.unproject([0.0, self.height / 2.0], target_z);
        let top = self.unproject([self.width / 2.0, 0.0], target_z);
        let mut right = self.unproject([self.width, self.height / 2.0], target_z);
        let bottom = self.unproject([self.width / 2.0, self.height], target_z);
        if right[0] < self.longitude {
            right[0] += 360.0;
        }
        if left[0] > self.longitude {
            left[0] -= 360.0;
        }
        [
            js_min(js_min(js_min(left[0], right[0]), top[0]), bottom[0]),
            js_min(js_min(js_min(left[1], right[1]), top[1]), bottom[1]),
            js_max(js_max(js_max(left[0], right[0]), top[0]), bottom[0]),
            js_max(js_max(js_max(left[1], right[1]), top[1]), bottom[1]),
        ]
    }
}

/// The flat map: deck 9.4's `WebMercatorViewport` with `@math.gl/web-mercator`'s matrices, used
/// above zoom 12 (doc 01 §2.3).
///
/// ```text
/// V = translate(0, 0, −1.5) · Rx(−pitch) · Rz(bearing) · S(2^zoom / height)
/// P = perspective(fovy, width / height, near 0.1, far min(1.01 · furthest, 10 · camera-to-sea-level))
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct MercatorViewport {
    pub base: ViewportBase,
}

impl std::ops::Deref for MercatorViewport {
    type Target = ViewportBase;
    fn deref(&self) -> &ViewportBase {
        &self.base
    }
}

impl MercatorViewport {
    /// `new WebMercatorViewport({ ...view, width, height })`.
    pub fn new(view: &ViewState, width: f64, height: f64) -> Self {
        let ViewState { latitude, longitude, zoom, pitch, bearing, .. } = *view;
        let altitude = CAMERA_HEIGHTS;
        let scale = 2f64.powf(zoom);
        let width = js_or1(width);
        let height = js_or1(height);
        let fovy = altitude_to_fovy(altitude);
        let raised = view.position.map(|p| [0.0, 0.0, p[2] * mercator_units_per_meter(latitude)]);
        let (near, far) = mercator_projection_parameters(height, scale, raised, pitch, fovy);
        let view_matrix_uncentered = mercator_view_matrix(height, pitch, bearing, scale, altitude);
        let center_on_ground = Self::project_position_static([longitude, latitude, 0.0]);
        let base = ViewportBase::new(
            view,
            width,
            height,
            latitude,
            // WebMercatorViewport sets `this.zoom = zoom` again after the base's `opts.zoom || 0`
            zoom,
            scale,
            fovy,
            altitude,
            near,
            far,
            DistanceScales::mercator(latitude),
            center_on_ground,
            view_matrix_uncentered,
        );
        MercatorViewport { base }
    }

    /// deck's `projectFlat`: math.gl's `lngLatToWorld`, y clamped to [−318, 830] as deck's shader
    /// clamps latitude to ±89.9.
    pub fn project_flat(&self, lng_lat: [f64; 2]) -> [f64; 2] {
        Self::project_flat_static(lng_lat)
    }

    fn project_flat_static(lng_lat: [f64; 2]) -> [f64; 2] {
        let [x, y] = lng_lat_to_world(lng_lat);
        [x, clamp(y, -318.0, 830.0)]
    }

    /// deck's `unprojectFlat`: math.gl's `worldToLngLat`.
    pub fn unproject_flat(&self, xy: [f64; 2]) -> [f64; 2] {
        world_to_lng_lat(xy)
    }

    /// deck's `WebMercatorViewport.projectPosition`: `[lon, lat, metres]` to common space; metres
    /// become units at the **point's** latitude.
    pub fn project_position(&self, xyz: [f64; 3]) -> DVec3 {
        DVec3::from_array(Self::project_position_static(xyz))
    }

    fn project_position_static([lng, lat, z]: [f64; 3]) -> [f64; 3] {
        let [x, y] = Self::project_flat_static([lng, lat]);
        [x, y, js_or0(z) * mercator_units_per_meter(lat)]
    }

    /// deck's `WebMercatorViewport.unprojectPosition`: common space to `[lon, lat, metres]`.
    pub fn unproject_position(&self, xyz: DVec3) -> [f64; 3] {
        let [x, y] = world_to_lng_lat([xyz.x, xyz.y]);
        [x, y, js_or0(xyz.z) / mercator_units_per_meter(y)]
    }

    /// deck's `project`: `[lon, lat, metres]` to `[x, y, depth]` pixels (depth is NDC z).
    pub fn project(&self, xyz: [f64; 3]) -> [f64; 3] {
        self.base.world_to_pixels(self.project_position(xyz))
    }

    /// deck's `unproject([x, y], { targetZ })`: the pixel's ray against the plane `target_z` metres
    /// up (metres to units at the **view centre's** latitude), `[lon, lat]`.
    ///
    /// A pixel above the horizon gives the plane's intersection behind the camera, as deck
    /// computes it.
    pub fn unproject(&self, [x, y]: [f64; 2], target_z: f64) -> [f64; 2] {
        // `targetZ && targetZ * unitsPerMeter[2]`, then `targetZ || 0` inside pixelsToWorld
        let target_z_world = js_or0(target_z) * self.distance_scales.units_per_meter[2];
        let coord = pixels_to_world_on_plane(&self.pixel_unprojection_matrix.to_cols_array(), [x, y], js_or0(target_z_world));
        let [lng, lat, _] = self.unproject_position(DVec3::new(coord[0], coord[1], 0.0));
        [lng, lat]
    }

    /// deck's `unproject([x, y, depth])`: the pixel at a known depth, `[lon, lat, metres]`.
    pub fn unproject_depth(&self, xyz: [f64; 3]) -> [f64; 3] {
        self.unproject_position(self.base.pixels_to_world_with_depth(xyz))
    }

    /// math.gl's `getBounds(viewport, z)` as `WebMercatorViewport.getBounds({ z })` reduces it:
    /// `[min lon, min lat, max lon, max lat]` of the frustum cut by the plane `z` metres up. When
    /// the top edge looks above the horizon the far plane stands in for it.
    pub fn bounds(&self, z: f64) -> [f64; 4] {
        let z = js_or0(z);
        let (width, height) = (self.width, self.height);
        let bottom_left = self.unproject([0.0, height], z);
        let bottom_right = self.unproject([width, height], z);
        let half_fov = 0.5 * self.fovy * DEG;
        let angle_to_ground = (90.0 - self.pitch) * DEG;
        // the top plane is parallel to the ground if half_fov == angle_to_ground
        let (top_left, top_right) = if half_fov > angle_to_ground - 0.01 {
            (self.unproject_on_far_plane(0.0, z), self.unproject_on_far_plane(width, z))
        } else {
            (self.unproject([0.0, 0.0], z), self.unproject([width, 0.0], z))
        };
        let corners = [bottom_left, bottom_right, top_right, top_left];
        [
            js_min(js_min(js_min(corners[0][0], corners[1][0]), corners[2][0]), corners[3][0]),
            js_min(js_min(js_min(corners[0][1], corners[1][1]), corners[2][1]), corners[3][1]),
            js_max(js_max(js_max(corners[0][0], corners[1][0]), corners[2][0]), corners[3][0]),
            js_max(js_max(js_max(corners[0][1], corners[1][1]), corners[2][1]), corners[3][1]),
        ]
    }

    /// math.gl's `unprojectOnFarPlane`: the far plane's point under screen column `x`, at `target_z`
    /// metres.
    fn unproject_on_far_plane(&self, x: f64, target_z: f64) -> [f64; 2] {
        let pum = self.pixel_unprojection_matrix.to_cols_array();
        let coord0 = transform_vector(&pum, [x, 0.0, 1.0, 1.0]);
        let coord1 = transform_vector(&pum, [x, self.height, 1.0, 1.0]);
        let z = target_z * self.distance_scales.units_per_meter[2];
        let t = (z - coord0[2]) / (coord1[2] - coord0[2]);
        world_to_lng_lat(lerp2([coord0[0], coord0[1]], [coord1[0], coord1[1]], t))
    }
}

/// math.gl's `getProjectionParameters` as `WebMercatorViewport` calls it (with `fovy`, no padding
/// offset, nearZMultiplier 0.1, farZMultiplier 1.01): `(near, far)` in viewport heights.
fn mercator_projection_parameters(height: f64, scale: f64, center: Option<[f64; 3]>, pitch: f64, fovy: f64) -> (f64, f64) {
    let fov_radians = fovy * DEG;
    let pitch_radians = pitch * DEG;
    // distance from the camera to the target
    let focal_distance = fovy_to_altitude(fovy);
    let mut camera_to_sea_level_distance = focal_distance;
    if let Some(center) = center {
        camera_to_sea_level_distance += (center[2] * scale) / pitch_radians.cos() / height;
    }
    // no padding: `0.5 + 0 / height`
    let fov_above_center = fov_radians * (0.5 + 0.0 / height);
    // the distance from the centre point to the centre top, in focal distance units (law of sines)
    let top_half_surface_distance = (fov_above_center.sin() * camera_to_sea_level_distance)
        / clamp(PI / 2.0 - pitch_radians - fov_above_center, 0.01, PI - 0.01).sin();
    // the z distance of the farthest fragment that should be drawn
    let furthest_distance = pitch_radians.sin() * top_half_surface_distance + camera_to_sea_level_distance;
    // Mapbox's limit
    let horizon_distance = camera_to_sea_level_distance * 10.0;
    let far = js_min(furthest_distance * MERCATOR_FAR_Z_MULTIPLIER, horizon_distance);
    (MERCATOR_NEAR_Z_MULTIPLIER, far)
}

/// math.gl's `getViewMatrix` (no `center`, as deck calls it).
fn mercator_view_matrix(height: f64, pitch: f64, bearing: f64, scale: f64, altitude: f64) -> M4 {
    // move the camera to altitude along the pitch and bearing direction
    let mut vm = translate(&IDENTITY, [0.0, 0.0, -altitude]);
    // rotate by bearing, and then by pitch (which tilts the view)
    vm = rotate_x(&vm, -pitch * DEG);
    vm = rotate_z(&vm, bearing * DEG);
    let relative_scale = scale / height;
    scale3(&vm, [relative_scale, relative_scale, relative_scale])
}

/// math.gl's `pixelsToWorld([x, y], pixelUnprojectionMatrix, targetZ)` without a depth: the pixel's
/// ray (depths 0 and 1) cut by the plane `z = target_z` in common space, `[x, y]`.
fn pixels_to_world_on_plane(pum: &M4, [x, y]: [f64; 2], target_z: f64) -> [f64; 2] {
    let coord0 = transform_vector(pum, [x, y, 0.0, 1.0]);
    let coord1 = transform_vector(pum, [x, y, 1.0, 1.0]);
    let z0 = coord0[2];
    let z1 = coord1[2];
    let t = if z0 == z1 { 0.0 } else { (js_or0(target_z) - z0) / (z1 - z0) };
    lerp2([coord0[0], coord0[1]], [coord1[0], coord1[1]], t)
}

/// One of the two viewports the app draws with (`GlobeView.makeViewport`).
#[derive(Debug, Clone, PartialEq)]
pub enum Viewport {
    /// Zoom ≤ 12.
    Globe(GlobeViewport),
    /// Zoom > 12.
    Mercator(MercatorViewport),
}

/// The viewport for a view on a canvas of `[width, height]` CSS pixels: the globe up to zoom 12,
/// the flat map above (`GlobeView.makeViewport`; doc 01 §2). The two agree in scale at the view
/// centre, so the swap is seamless there.
pub fn viewport_for(view: &ViewState, [width, height]: [f64; 2]) -> Viewport {
    if view.zoom > FLAT_ABOVE_ZOOM {
        Viewport::Mercator(MercatorViewport::new(view, width, height))
    } else {
        Viewport::Globe(GlobeViewport::new(view, width, height))
    }
}

impl std::ops::Deref for Viewport {
    type Target = ViewportBase;
    fn deref(&self) -> &ViewportBase {
        match self {
            Viewport::Globe(v) => &v.base,
            Viewport::Mercator(v) => &v.base,
        }
    }
}

impl Viewport {
    /// Whether this is the globe (zoom ≤ 12).
    pub fn is_globe(&self) -> bool {
        matches!(self, Viewport::Globe(_))
    }

    /// The globe, if this is it.
    pub fn globe(&self) -> Option<&GlobeViewport> {
        match self {
            Viewport::Globe(v) => Some(v),
            Viewport::Mercator(_) => None,
        }
    }

    /// The flat map, if this is it.
    pub fn mercator(&self) -> Option<&MercatorViewport> {
        match self {
            Viewport::Globe(_) => None,
            Viewport::Mercator(v) => Some(v),
        }
    }

    /// `[lon, lat, metres]` to common space (deck's `projectPosition`).
    pub fn project_position(&self, xyz: [f64; 3]) -> DVec3 {
        match self {
            Viewport::Globe(v) => v.project_position(xyz),
            Viewport::Mercator(v) => v.project_position(xyz),
        }
    }

    /// Common space to `[lon, lat, metres]` (deck's `unprojectPosition`).
    pub fn unproject_position(&self, xyz: DVec3) -> [f64; 3] {
        match self {
            Viewport::Globe(v) => v.unproject_position(xyz),
            Viewport::Mercator(v) => v.unproject_position(xyz),
        }
    }

    /// `[lon, lat, metres]` to `[x, y, depth]` pixels (deck's `project`). `project([lon, lat])` is
    /// this with 0 metres.
    pub fn project(&self, xyz: [f64; 3]) -> [f64; 3] {
        match self {
            Viewport::Globe(v) => v.project(xyz),
            Viewport::Mercator(v) => v.project(xyz),
        }
    }

    /// The pixel's ray against the ground raised `target_z` metres, `[lon, lat]` (deck's
    /// `unproject([x, y], { targetZ })`). Always finite on the globe: off it, the direction of the
    /// ray's closest approach to the globe's centre, next to the limb (doc 01 §2.2); `mapAt`
    /// (G:3518-3521) only rejects non-finite results.
    pub fn unproject(&self, xy: [f64; 2], target_z: f64) -> [f64; 2] {
        match self {
            Viewport::Globe(v) => v.unproject(xy, target_z),
            Viewport::Mercator(v) => v.unproject(xy, target_z),
        }
    }

    /// The pixel `[x, y, depth]` back to `[lon, lat, metres]` (deck's `unproject([x, y, depth])`).
    pub fn unproject_depth(&self, xyz: [f64; 3]) -> [f64; 3] {
        match self {
            Viewport::Globe(v) => v.unproject_depth(xyz),
            Viewport::Mercator(v) => v.unproject_depth(xyz),
        }
    }

    /// `[min lon, min lat, max lon, max lat]` of the view at `z` metres (deck's `getBounds({ z })`).
    pub fn bounds(&self, z: f64) -> [f64; 4] {
        match self {
            Viewport::Globe(v) => v.bounds(z),
            Viewport::Mercator(v) => v.bounds(z),
        }
    }

    /// Wheel-zoom anchoring at a pixel: [`GlobeViewport::zoom_anchor_strength`] on the globe, 1 on
    /// the flat map (the app's patch `WebMercatorViewport.prototype.getZoomAnchorStrength = () => 1`,
    /// G:257-261).
    pub fn zoom_anchor_strength(&self, xy: [f64; 2]) -> f64 {
        match self {
            Viewport::Globe(v) => v.zoom_anchor_strength(xy),
            Viewport::Mercator(_) => 1.0,
        }
    }
}

impl Projector for GlobeViewport {
    fn project(&self, position: [f64; 3]) -> [f64; 2] {
        let [x, y, _] = GlobeViewport::project(self, position);
        [x, y]
    }
}

impl Projector for MercatorViewport {
    fn project(&self, position: [f64; 3]) -> [f64; 2] {
        let [x, y, _] = MercatorViewport::project(self, position);
        [x, y]
    }
}

impl Projector for Viewport {
    fn project(&self, position: [f64; 3]) -> [f64; 2] {
        let [x, y, _] = Viewport::project(self, position);
        [x, y]
    }
}

// ---------------------------------------------------------------------------------------------
// JavaScript semantics
// ---------------------------------------------------------------------------------------------

/// `Math.min(a, b)`: NaN when either is NaN, and −0 below +0.
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.max(a, b)`: NaN when either is NaN, and +0 above −0.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// `x || 0` for a number: 0 for 0, −0 and NaN.
fn js_or0(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() { 0.0 } else { x }
}

/// `x || 1` for a number (deck's `width || 1`, `height || 1`).
fn js_or1(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() { 1.0 } else { x }
}

/// math.gl's `clamp(x, min, max)`: `x < min ? min : x > max ? max : x` (NaN passes).
fn clamp(x: f64, min: f64, max: f64) -> f64 {
    if x < min {
        min
    } else if x > max {
        max
    } else {
        x
    }
}

// ---------------------------------------------------------------------------------------------
// gl-matrix, as math.gl 4.1 ships it (column-major, column vectors)
// ---------------------------------------------------------------------------------------------

/// A 4 × 4 matrix in gl-matrix's layout: column-major, element (row r, column c) at `c * 4 + r`.
type M4 = [f64; 16];

const IDENTITY: M4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `mat4.lookAt(out, eye, center, up)`.
fn look_at(eye: [f64; 3], center: [f64; 3], up: [f64; 3]) -> M4 {
    let [eyex, eyey, eyez] = eye;
    let [upx, upy, upz] = up;
    let [centerx, centery, centerz] = center;
    if (eyex - centerx).abs() < GL_MATRIX_EPSILON
        && (eyey - centery).abs() < GL_MATRIX_EPSILON
        && (eyez - centerz).abs() < GL_MATRIX_EPSILON
    {
        return IDENTITY;
    }
    let mut z0 = eyex - centerx;
    let mut z1 = eyey - centery;
    let mut z2 = eyez - centerz;
    let mut len = 1.0 / (z0 * z0 + z1 * z1 + z2 * z2).sqrt();
    z0 *= len;
    z1 *= len;
    z2 *= len;
    let mut x0 = upy * z2 - upz * z1;
    let mut x1 = upz * z0 - upx * z2;
    let mut x2 = upx * z1 - upy * z0;
    len = (x0 * x0 + x1 * x1 + x2 * x2).sqrt();
    if len == 0.0 || len.is_nan() {
        x0 = 0.0;
        x1 = 0.0;
        x2 = 0.0;
    } else {
        len = 1.0 / len;
        x0 *= len;
        x1 *= len;
        x2 *= len;
    }
    let mut y0 = z1 * x2 - z2 * x1;
    let mut y1 = z2 * x0 - z0 * x2;
    let mut y2 = z0 * x1 - z1 * x0;
    len = (y0 * y0 + y1 * y1 + y2 * y2).sqrt();
    if len == 0.0 || len.is_nan() {
        y0 = 0.0;
        y1 = 0.0;
        y2 = 0.0;
    } else {
        len = 1.0 / len;
        y0 *= len;
        y1 *= len;
        y2 *= len;
    }
    [
        x0,
        y0,
        z0,
        0.0,
        x1,
        y1,
        z1,
        0.0,
        x2,
        y2,
        z2,
        0.0,
        -(x0 * eyex + x1 * eyey + x2 * eyez),
        -(y0 * eyex + y1 * eyey + y2 * eyez),
        -(z0 * eyex + z1 * eyey + z2 * eyez),
        1.0,
    ]
}

/// `mat4.perspective` (`perspectiveNO`: clip z in [−1, 1], WebGL's convention).
fn perspective(fovy: f64, aspect: f64, near: f64, far: f64) -> M4 {
    let f = 1.0 / (fovy / 2.0).tan();
    let mut out = [0.0; 16];
    out[0] = f / aspect;
    out[5] = f;
    out[11] = -1.0;
    // `far != null && far !== Infinity` (NaN takes this branch too)
    if far != f64::INFINITY {
        let nf = 1.0 / (near - far);
        out[10] = (far + near) * nf;
        out[14] = 2.0 * far * near * nf;
    } else {
        out[10] = -1.0;
        out[14] = -2.0 * near;
    }
    out
}

/// `mat4.rotateX(out, a, rad)`: `a · Rx(rad)`.
fn rotate_x(a: &M4, rad: f64) -> M4 {
    let s = rad.sin();
    let c = rad.cos();
    let (a10, a11, a12, a13) = (a[4], a[5], a[6], a[7]);
    let (a20, a21, a22, a23) = (a[8], a[9], a[10], a[11]);
    let mut out = *a;
    out[4] = a10 * c + a20 * s;
    out[5] = a11 * c + a21 * s;
    out[6] = a12 * c + a22 * s;
    out[7] = a13 * c + a23 * s;
    out[8] = a20 * c - a10 * s;
    out[9] = a21 * c - a11 * s;
    out[10] = a22 * c - a12 * s;
    out[11] = a23 * c - a13 * s;
    out
}

/// `mat4.rotateY(out, a, rad)`: `a · Ry(rad)`.
fn rotate_y(a: &M4, rad: f64) -> M4 {
    let s = rad.sin();
    let c = rad.cos();
    let (a00, a01, a02, a03) = (a[0], a[1], a[2], a[3]);
    let (a20, a21, a22, a23) = (a[8], a[9], a[10], a[11]);
    let mut out = *a;
    out[0] = a00 * c - a20 * s;
    out[1] = a01 * c - a21 * s;
    out[2] = a02 * c - a22 * s;
    out[3] = a03 * c - a23 * s;
    out[8] = a00 * s + a20 * c;
    out[9] = a01 * s + a21 * c;
    out[10] = a02 * s + a22 * c;
    out[11] = a03 * s + a23 * c;
    out
}

/// `mat4.rotateZ(out, a, rad)`: `a · Rz(rad)`.
fn rotate_z(a: &M4, rad: f64) -> M4 {
    let s = rad.sin();
    let c = rad.cos();
    let (a00, a01, a02, a03) = (a[0], a[1], a[2], a[3]);
    let (a10, a11, a12, a13) = (a[4], a[5], a[6], a[7]);
    let mut out = *a;
    out[0] = a00 * c + a10 * s;
    out[1] = a01 * c + a11 * s;
    out[2] = a02 * c + a12 * s;
    out[3] = a03 * c + a13 * s;
    out[4] = a10 * c - a00 * s;
    out[5] = a11 * c - a01 * s;
    out[6] = a12 * c - a02 * s;
    out[7] = a13 * c - a03 * s;
    out
}

/// `mat4.scale(out, a, v)`: `a · S(v)`.
fn scale3(a: &M4, [x, y, z]: [f64; 3]) -> M4 {
    [
        a[0] * x,
        a[1] * x,
        a[2] * x,
        a[3] * x,
        a[4] * y,
        a[5] * y,
        a[6] * y,
        a[7] * y,
        a[8] * z,
        a[9] * z,
        a[10] * z,
        a[11] * z,
        a[12],
        a[13],
        a[14],
        a[15],
    ]
}

/// `mat4.translate(out, a, v)`: `a · T(v)`.
fn translate(a: &M4, [x, y, z]: [f64; 3]) -> M4 {
    let mut out = *a;
    out[12] = a[0] * x + a[4] * y + a[8] * z + a[12];
    out[13] = a[1] * x + a[5] * y + a[9] * z + a[13];
    out[14] = a[2] * x + a[6] * y + a[10] * z + a[14];
    out[15] = a[3] * x + a[7] * y + a[11] * z + a[15];
    out
}

/// `mat4.multiply(out, a, b)`: `a · b`.
fn multiply(a: &M4, b: &M4) -> M4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        let (b0, b1, b2, b3) = (b[col * 4], b[col * 4 + 1], b[col * 4 + 2], b[col * 4 + 3]);
        for row in 0..4 {
            out[col * 4 + row] = b0 * a[row] + b1 * a[4 + row] + b2 * a[8 + row] + b3 * a[12 + row];
        }
    }
    out
}

/// `mat4.invert(out, a)`; `None` where gl-matrix returns `null` (a zero determinant).
fn invert(a: &M4) -> Option<M4> {
    let (a00, a01, a02, a03) = (a[0], a[1], a[2], a[3]);
    let (a10, a11, a12, a13) = (a[4], a[5], a[6], a[7]);
    let (a20, a21, a22, a23) = (a[8], a[9], a[10], a[11]);
    let (a30, a31, a32, a33) = (a[12], a[13], a[14], a[15]);
    let b00 = a00 * a11 - a01 * a10;
    let b01 = a00 * a12 - a02 * a10;
    let b02 = a00 * a13 - a03 * a10;
    let b03 = a01 * a12 - a02 * a11;
    let b04 = a01 * a13 - a03 * a11;
    let b05 = a02 * a13 - a03 * a12;
    let b06 = a20 * a31 - a21 * a30;
    let b07 = a20 * a32 - a22 * a30;
    let b08 = a20 * a33 - a23 * a30;
    let b09 = a21 * a32 - a22 * a31;
    let b10 = a21 * a33 - a23 * a31;
    let b11 = a22 * a33 - a23 * a32;
    // the determinant
    let mut det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
    // `if (!det) return null`: 0 and NaN
    if det == 0.0 || det.is_nan() {
        return None;
    }
    det = 1.0 / det;
    Some([
        (a11 * b11 - a12 * b10 + a13 * b09) * det,
        (a02 * b10 - a01 * b11 - a03 * b09) * det,
        (a31 * b05 - a32 * b04 + a33 * b03) * det,
        (a22 * b04 - a21 * b05 - a23 * b03) * det,
        (a12 * b08 - a10 * b11 - a13 * b07) * det,
        (a00 * b11 - a02 * b08 + a03 * b07) * det,
        (a32 * b02 - a30 * b05 - a33 * b01) * det,
        (a20 * b05 - a22 * b02 + a23 * b01) * det,
        (a10 * b10 - a11 * b08 + a13 * b06) * det,
        (a01 * b08 - a00 * b10 - a03 * b06) * det,
        (a30 * b04 - a31 * b02 + a33 * b00) * det,
        (a21 * b02 - a20 * b04 - a23 * b00) * det,
        (a11 * b07 - a10 * b09 - a12 * b06) * det,
        (a00 * b09 - a01 * b07 + a02 * b06) * det,
        (a31 * b01 - a30 * b03 - a32 * b00) * det,
        (a20 * b03 - a21 * b01 + a22 * b00) * det,
    ])
}

/// math.gl's `transformVector(matrix, vector)`: `vec4.transformMat4`, then the whole vector scaled
/// by `1 / w` (a multiplication by the reciprocal, as math.gl does, not a division).
fn transform_vector(m: &M4, [x, y, z, w]: [f64; 4]) -> [f64; 4] {
    let out = [
        m[0] * x + m[4] * y + m[8] * z + m[12] * w,
        m[1] * x + m[5] * y + m[9] * z + m[13] * w,
        m[2] * x + m[6] * y + m[10] * z + m[14] * w,
        m[3] * x + m[7] * y + m[11] * z + m[15] * w,
    ];
    let s = 1.0 / out[3];
    [out[0] * s, out[1] * s, out[2] * s, out[3] * s]
}

/// `vec2.lerp`.
fn lerp2(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}

/// `vec3.lerp` (of the first three elements).
fn lerp3(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 3] {
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), a[2] + t * (b[2] - a[2])]
}

/// `vec3.sqrLen` (of the first three elements).
fn sqr_len3(a: [f64; 4]) -> f64 {
    a[0] * a[0] + a[1] * a[1] + a[2] * a[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// deck.gl's own viewports for 24 views (S1.32; `scripts/port/deck-golden.ts` documents every
    /// field).
    fn golden() -> Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/deck/views.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("views.json")).expect("views.json parses")
    }

    fn f(v: &Value) -> f64 {
        v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
    }

    fn fs(v: &Value) -> Vec<f64> {
        v.as_array().unwrap_or_else(|| panic!("not an array: {v}")).iter().map(f).collect()
    }

    fn view_state(v: &Value) -> ViewState {
        let s = &v["viewState"];
        ViewState {
            longitude: f(&s["longitude"]),
            latitude: f(&s["latitude"]),
            zoom: f(&s["zoom"]),
            bearing: f(&s["bearing"]),
            pitch: f(&s["pitch"]),
            position: s.get("position").map(|p| {
                let p = fs(p);
                [p[0], p[1], p[2]]
            }),
        }
    }

    /// The largest error seen of each kind, to report the margin.
    #[derive(Default, Debug)]
    struct Worst(std::collections::BTreeMap<&'static str, f64>);

    impl Worst {
        fn see(&mut self, kind: &'static str, err: f64, tolerance: f64, what: &str) {
            assert!(err <= tolerance, "{what}: {kind} error {err:e} over {tolerance:e}");
            let worst = self.0.entry(kind).or_insert(0.0);
            *worst = worst.max(err);
        }
    }

    /// Elements compared against the largest element (some are rounding noise around 0).
    fn relative(ours: &[f64], want: &[f64]) -> f64 {
        assert_eq!(ours.len(), want.len());
        let largest = want.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        ours.iter().zip(want).fold(0.0f64, |m, (a, b)| m.max((a - b).abs())) / largest
    }

    fn scalar(ours: f64, want: f64) -> f64 {
        (ours - want).abs() / want.abs()
    }

    #[test]
    fn matches_every_golden_view() {
        let golden = golden();
        let views = golden["views"].as_array().unwrap();
        assert_eq!(views.len(), 24);
        let mut worst = Worst::default();
        for v in views {
            let name = v["name"].as_str().unwrap();
            let view = view_state(v);
            let size = [f(&v["width"]), f(&v["height"])];
            let vp = viewport_for(&view, size);
            assert_eq!(vp.is_globe(), v["viewport"] == "globe", "{name}: viewport");

            // the viewport's numbers
            for (key, ours) in [
                ("zoom", vp.zoom),
                ("scale", vp.scale),
                ("fovy", vp.fovy),
                ("focalDistance", vp.focal_distance),
                ("metersPerPixel", vp.meters_per_pixel()),
            ] {
                worst.see("scalars", scalar(ours, f(&v[key])), 1e-12, &format!("{name} {key}"));
            }
            let ds = &v["distanceScales"];
            let scales = vp.distance_scales;
            for (key, ours) in [
                ("unitsPerMeter", scales.units_per_meter),
                ("metersPerUnit", scales.meters_per_unit),
                ("unitsPerDegree", scales.units_per_degree),
                ("degreesPerUnit", scales.degrees_per_unit),
            ] {
                let want = fs(&ds[key]);
                for i in 0..3 {
                    worst.see("scalars", scalar(ours[i], want[i]), 1e-12, &format!("{name} {key}[{i}]"));
                }
            }
            worst.see("vectors", relative(&vp.center.to_array(), &fs(&v["center"])), 1e-12, &format!("{name} center"));
            worst.see(
                "vectors",
                relative(&vp.camera_position.to_array(), &fs(&v["cameraPosition"])),
                1e-12,
                &format!("{name} cameraPosition"),
            );

            // the matrices
            for (key, ours) in [
                ("viewMatrixUncentered", vp.view_matrix_uncentered),
                ("viewMatrix", vp.view_matrix),
                ("projectionMatrix", vp.projection_matrix),
                ("viewProjectionMatrix", vp.view_projection_matrix),
                ("pixelProjectionMatrix", vp.pixel_projection_matrix),
                ("pixelUnprojectionMatrix", vp.pixel_unprojection_matrix),
            ] {
                worst.see("matrices", relative(&ours.to_cols_array(), &fs(&v[key])), 1e-9, &format!("{name} {key}"));
            }

            // the frustum planes, in deck's order
            let planes = vp.frustum_planes().all();
            for (i, side) in ["left", "right", "bottom", "top", "near", "far"].into_iter().enumerate() {
                let want = fs(&v["frustumPlanes"][side]);
                let plane = planes[i];
                worst.see("plane normals", relative(&plane.normal.to_array(), &want[..3]), 1e-12, &format!("{name} {side}"));
                worst.see("plane distances", scalar(plane.distance, want[3]), 1e-12, &format!("{name} {side}"));
            }

            // getBounds(): degrees
            let bounds = vp.bounds(0.0);
            let want = fs(&v["bounds"]);
            for i in 0..4 {
                worst.see("bounds (deg)", (bounds[i] - want[i]).abs(), 1e-11, &format!("{name} bounds[{i}]"));
            }

            // project: pixels; back: unproject with the depth
            for record in v["project"].as_array().unwrap() {
                let input = fs(&record["in"]);
                let out = fs(&record["out"]);
                let back = fs(&record["back"]);
                let what = format!("{name} project {input:?}");
                let p = vp.project([input[0], input[1], input[2]]);
                worst.see("project (px)", (p[0] - out[0]).abs().max((p[1] - out[1]).abs()), 1e-6, &what);
                worst.see("project depth", (p[2] - out[2]).abs(), 1e-12, &what);
                // project([lng, lat]) gives the same x and y as with metres 0
                if input[2] == 0.0 {
                    let flat = Projector::project(&vp, [input[0], input[1], 0.0]);
                    assert_eq!(flat, [p[0], p[1]], "{what}: Projector");
                }
                // Compared in common space (at a pole the longitude is noise), relative to the
                // position: the depth path runs through the inverse of an ill-conditioned matrix,
                // and at zoom 23 one ulp of the centre's common coordinates is already 5e-7 px.
                // The worst seen is 2.4e-13 (globe-z12-lat80), 5e-5 px at zoom 23.
                let b = vp.unproject_depth([out[0], out[1], out[2]]);
                let want = vp.project_position([back[0], back[1], back[2]]);
                let gap = (vp.project_position(b) - want).length();
                worst.see("back (relative)", gap / want.length(), 1e-11, &what);
                worst.see("back (px, not asserted)", gap * vp.scale, f64::INFINITY, &what);
            }

            // unproject: degrees, and the wheel zoom's anchor strength on the globe
            for record in v["unproject"].as_array().unwrap() {
                let input = fs(&record["in"]);
                let target_z = f(&record["targetZ"]);
                let out = fs(&record["out"]);
                let what = format!("{name} unproject {input:?} at {target_z} m");
                let u = vp.unproject([input[0], input[1]], target_z);
                worst.see("unproject (deg)", (u[0] - out[0]).abs().max((u[1] - out[1]).abs()), 1e-11, &what);
                if let Some(anchor) = record.get("anchor") {
                    worst.see("anchor", (vp.zoom_anchor_strength([input[0], input[1]]) - f(anchor)).abs(), 1e-12, &what);
                }
            }
        }
        eprintln!("{worst:#?}");
    }

    fn view(longitude: f64, latitude: f64, zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing, pitch, position: None }
    }

    const DESK: [f64; 2] = [1440.0, 900.0];

    #[test]
    fn swaps_to_the_flat_map_just_above_zoom_12() {
        assert!(viewport_for(&view(18.0, 59.0, 12.0, 0.0, 0.0), DESK).is_globe());
        assert!(!viewport_for(&view(18.0, 59.0, 12.000001, 0.0, 0.0), DESK).is_globe());
        assert!(!ViewState { zoom: 12.0, ..ViewState::INITIAL }.is_flat());
        assert!(ViewState { zoom: 12.5, ..ViewState::INITIAL }.is_flat());
    }

    #[test]
    fn both_projections_agree_in_scale_at_the_view_centre_across_the_swap() {
        // a point 2 km east and 2 km north of the centre lands within 0.2 % of the same pixel
        // offset on either side of the swap (deck's two Earths differ by 0.0004 %)
        for (lat, bearing, pitch) in [(0.0, 0.0, 0.0), (59.33, 30.0, 0.0), (-45.0, 200.0, 40.0), (75.0, -60.0, 60.0)] {
            let globe = viewport_for(&view(18.07, lat, 12.0, pitch, bearing), DESK);
            let flat = viewport_for(&view(18.07, lat, 12.0 + 1e-9, pitch, bearing), DESK);
            assert!(globe.is_globe() && !flat.is_globe());
            let d_lat = 2000.0 / 111_320.0;
            let d_lon = d_lat / (lat * DEG).cos();
            let target = [18.07 + d_lon, lat + d_lat, 0.0];
            let (g, m) = (globe.project(target), flat.project(target));
            let (gx, gy) = (g[0] - 720.0, g[1] - 450.0);
            let (mx, my) = (m[0] - 720.0, m[1] - 450.0);
            let offset = gx.hypot(gy);
            assert!(offset > 50.0, "{lat}: {offset}");
            assert!((gx - mx).hypot(gy - my) < 0.002 * offset, "{lat}: globe {g:?}, flat {m:?}");
            // and the ground resolution at the centre is the same within 0.0004 % (the globe's
            // Earth of radius 6 370 972 m against math.gl's 40 030 km round)
            assert!(scalar(globe.meters_per_pixel(), flat.meters_per_pixel()) < 5e-6);
            // geo.ts' metersPerPixel takes a third Earth, 40 075 016.686 m round: 0.1 % larger
            assert!(scalar(meters_per_pixel(12.0, lat), flat.meters_per_pixel()) < 2e-3);
        }
    }

    #[test]
    fn the_globe_grows_toward_the_poles_at_a_constant_zoom() {
        // scale = 2^zoom / (π·cos lat): twice as many pixels per unit at 60° as at the equator
        let equator = viewport_for(&view(10.0, 0.0, 2.0, 0.0, 0.0), DESK);
        let north = viewport_for(&view(10.0, 60.0, 2.0, 0.0, 0.0), DESK);
        assert!(scalar(north.scale / equator.scale, 1.0 / (60.0 * DEG).cos()) < 1e-12);
        assert!(scalar(equator.scale, 4.0 / PI) < 1e-15);
        // the limb's distance from the centre, measured along the screen's x axis: the first pixel
        // whose ray misses the globe
        let limb = |vp: &Viewport| {
            let (mut inside, mut outside) = (720.0, 720.0 + 5000.0);
            for _ in 0..60 {
                let mid = (inside + outside) / 2.0;
                if vp.globe().unwrap().ray_distance_to_globe_center_ratio([mid, 450.0]) < 1.0 {
                    inside = mid;
                } else {
                    outside = mid;
                }
            }
            inside - 720.0
        };
        // It is not twice as wide, though: the camera stays 1.5 viewport heights from its target,
        // which at twice the scale is half as many units, so the perspective shrinks the nearer
        // globe. Seen from D units off the centre the limb is f·R / sqrt(D² − R²) px out, with
        // f = 1.5 heights and D = 1.5·height / scale + 256.
        let expected = |vp: &Viewport| {
            let f = CAMERA_HEIGHTS * vp.height;
            let d = CAMERA_HEIGHTS * vp.height / vp.scale + GLOBE_RADIUS;
            f * GLOBE_RADIUS / (d * d - GLOBE_RADIUS * GLOBE_RADIUS).sqrt()
        };
        let (r0, r60) = (limb(&equator), limb(&north));
        assert!((r0 - expected(&equator)).abs() < 1e-6, "{r0}");
        assert!((r60 - expected(&north)).abs() < 1e-6, "{r60}");
        assert!((r60 / r0 - 1.737).abs() < 1e-3, "{r0} {r60}");
        // past 85.051129° the scale stops growing
        let a = viewport_for(&view(0.0, 85.051129, 2.0, 0.0, 0.0), DESK);
        let b = viewport_for(&view(0.0, 89.0, 2.0, 0.0, 0.0), DESK);
        assert_eq!(a.scale, b.scale);
    }

    #[test]
    fn off_the_limb_unproject_gives_the_rays_closest_approach_next_to_the_limb() {
        let vp = viewport_for(&view(0.0, 0.0, 0.5, 0.0, 0.0), DESK);
        let globe = vp.globe().unwrap();
        for pixel in [[0.0, 0.0], [1440.0, 450.0], [720.0, 0.0], [1439.5, 0.5], [100.0, 880.0]] {
            assert!(globe.ray_distance_to_globe_center_ratio(pixel) > 1.0, "{pixel:?} is off the globe");
            let [lon, lat] = vp.unproject(pixel, 0.0);
            assert!(lon.is_finite() && lat.is_finite(), "{pixel:?}");
            // the direction of the ray's closest approach to the globe's centre: square to the ray
            let p = vp.project_position([lon, lat, 0.0]);
            let ray = (vp.pixels_to_world_with_depth([pixel[0], pixel[1], 1.0])
                - vp.pixels_to_world_with_depth([pixel[0], pixel[1], -1.0]))
            .normalize();
            assert!(ray.dot(p.normalize()).abs() < 1e-9, "{pixel:?}: not the closest approach");
            // in the plane of the pixel's ray and the centre, so it projects onto the line from the
            // globe's centre on screen to the pixel
            let [x, y, _] = vp.project([lon, lat, 0.0]);
            let cross = (x - 720.0) * (pixel[1] - 450.0) - (y - 450.0) * (pixel[0] - 720.0);
            assert!(cross.abs() < 1e-6 * (pixel[0] - 720.0).hypot(pixel[1] - 450.0).powi(2), "{pixel:?}: {x}, {y}");
            // a wheel zoom there zooms about the centre
            assert_eq!(vp.zoom_anchor_strength(pixel), 0.0);
        }
        // Just off the limb that is next to the limb: seen from the camera, nearly the globe's edge
        // (the sight line nearly tangent). The pixel where the ray passes 1.01 radii out:
        let (mut inside, mut outside) = (720.0, 1440.0);
        for _ in 0..60 {
            let mid = (inside + outside) / 2.0;
            if globe.ray_distance_to_globe_center_ratio([mid, 450.0]) < 1.01 {
                inside = mid;
            } else {
                outside = mid;
            }
        }
        let [lon, lat] = vp.unproject([inside, 450.0], 0.0);
        let p = vp.project_position([lon, lat, 0.0]);
        let tangent = (vp.camera_position - p).normalize().dot(p.normalize());
        assert!(tangent.abs() < 0.02, "{tangent}");
        // the centre of the visible globe anchors fully; the flat map always does
        assert_eq!(vp.zoom_anchor_strength([720.0, 450.0]), 1.0);
        let flat = viewport_for(&view(0.0, 0.0, 15.0, 70.0, 0.0), DESK);
        assert_eq!(flat.zoom_anchor_strength([0.0, 0.0]), 1.0);
    }

    #[test]
    fn project_and_unproject_are_inverse_on_the_ground() {
        for v in [
            view(10.0, 35.0, 1.9, 0.0, 0.0),
            view(18.07, 59.33, 9.0, 0.0, 0.0),
            view(17.9186, 59.6519, 13.5, 45.0, 30.0),
            view(18.0686, 59.3293, 15.5, 60.0, 20.0),
            view(179.5, 0.0, 3.0, 0.0, 0.0),
            view(15.6, 78.2, 6.0, 0.0, 0.0),
        ] {
            let vp = viewport_for(&v, DESK);
            // on the visible globe even at zoom 1.9 (its limb is 370 px out), below the horizon at
            // a pitch of 60
            for pixel in [[720.0, 450.0], [560.0, 560.0], [900.0, 330.0], [720.0, 650.0]] {
                for target_z in [0.0, 120.0] {
                    let [lon, lat] = vp.unproject(pixel, target_z);
                    let [x, y, _] = vp.project([lon, lat, target_z]);
                    // the flat map turns metres into units at the point's latitude going in and at the
                    // centre's coming out: 120 m a few hundred metres off the centre at 59° lands
                    // about a centimetre off, 0.01 px at zoom 15.5
                    let tolerance = if vp.is_globe() || target_z == 0.0 { 1e-6 } else { 0.05 };
                    assert!((x - pixel[0]).hypot(y - pixel[1]) < tolerance, "{v:?} {pixel:?} at {target_z} m: {x}, {y}");
                }
            }
            // the view centre is inside the frustum: on the inner side of every plane
            for plane in vp.frustum_planes().all() {
                assert!(plane.normal.dot(vp.center) < plane.distance, "{v:?}: {plane:?}");
            }
            // and the middle of the canvas
            let [x, y, _] = vp.project([v.longitude, v.latitude, 0.0]);
            assert!((x - 720.0).hypot(y - 450.0) < 1e-6, "{v:?}: {x}, {y}");
            let [lon, lat] = vp.unproject([720.0, 450.0], 0.0);
            assert!((lon - v.longitude).abs() < 1e-9 && (lat - v.latitude).abs() < 1e-9, "{v:?}: {lon}, {lat}");
        }
    }

    #[test]
    fn a_raised_target_sits_in_the_middle_of_the_flat_map() {
        // locked on an aircraft 1500 m up: `position: [0, 0, 1500]`, at any pitch and bearing
        for (pitch, bearing) in [(0.0, 0.0), (70.0, 250.0), (85.0, 10.0)] {
            let v = ViewState { position: Some([0.0, 0.0, 1500.0]), ..view(18.06, 60.0, 17.0, pitch, bearing) };
            let vp = viewport_for(&v, DESK);
            assert!(!vp.is_globe());
            assert!(scalar(vp.center.z, 1500.0 * mercator_units_per_meter(60.0)) < 1e-15);
            let [x, y, depth] = vp.project([18.06, 60.0, 1500.0]);
            assert!((x - 720.0).hypot(y - 450.0) < 1e-6, "{pitch} {bearing}: {x}, {y}");
            assert!(depth > -1.0 && depth < 1.0);
            // the ground below is further down the screen once tilted
            let [_, ground_y, _] = vp.project([18.06, 60.0, 0.0]);
            assert!(pitch == 0.0 || ground_y > 450.0 + 1.0, "{pitch}: {ground_y}");
            // and the camera is CAMERA_HEIGHTS viewport heights back from the raised target
            let units_per_height = vp.height / vp.scale;
            assert!(scalar((vp.camera_position - vp.center).length(), CAMERA_HEIGHTS * units_per_height) < 1e-9);
        }
    }

    #[test]
    fn the_camera_is_one_and_a_half_viewport_heights_from_its_target() {
        // the globe: in common units, 1.5 heights at the view centre's scale
        for v in [view(10.0, 35.0, 1.9, 0.0, 0.0), view(-40.0, 60.0, 6.3, 50.0, 45.0)] {
            let vp = viewport_for(&v, DESK);
            let distance = (vp.camera_position - vp.center).length();
            assert!(scalar(distance, CAMERA_HEIGHTS * vp.height / vp.scale) < 1e-12, "{v:?}: {distance}");
        }
        // in metres, geo.ts' way
        assert!(scalar(camera_distance_m(0.0, 0.0, 900.0), 1.5 * 900.0 * 40_075_016.686 / 512.0) < 1e-15);
        assert!(scalar(camera_distance_m(10.0, 60.0, 900.0), 1.5 * 900.0 * 40_075_016.686 * 0.5 / (512.0 * 1024.0)) < 1e-12);
        assert!(scalar(camera_altitude_m(10.0, 60.0, 60.0, 900.0), camera_distance_m(10.0, 60.0, 900.0) / 2.0) < 1e-12);
        assert_eq!(camera_altitude_m(5.0, 10.0, 0.0, 700.0), camera_distance_m(5.0, 10.0, 700.0));
    }

    #[test]
    fn free_camera_limits_depend_on_the_basemap() {
        assert_eq!(limits(Basemap::Dark, None), Limits { min_zoom: 0.5, max_zoom: 20.0, max_pitch: 60.0 });
        assert_eq!(limits(Basemap::Satellite, None), LIMITS);
        assert_eq!(limits(Basemap::Realistic, None), Limits { min_zoom: 0.5, max_zoom: 20.0, max_pitch: 75.0 });
        assert_eq!(max_zoom_now(None), 20.0);
        assert_eq!(max_zoom_now(Some(21.5)), 21.5);
    }

    #[test]
    fn locked_limits_undo_the_controllers_latitude_shift() {
        // at 60° GlobeController lowers every zoom limit by log2(cos 60°) = −1; the locked maximum
        // is raised by as much, so the aircraft's limit is the zoom reached
        let lock = FollowLock { latitude: 60.0, max_zoom: 21.25, max_pitch: 85.0 };
        let l = limits(Basemap::Dark, Some(&lock));
        assert_eq!(l.min_zoom, 0.5);
        assert!((l.max_zoom - 22.25).abs() < 1e-12, "{l:?}");
        assert_eq!(l.max_pitch, 85.0);
        // whatever the basemap
        assert_eq!(limits(Basemap::Realistic, Some(&lock)), l);
        // the shift is capped near the poles (cos below 0.05)
        let polar = limits(Basemap::Dark, Some(&FollowLock { latitude: 89.9, max_zoom: 20.0, max_pitch: 40.0 }));
        assert!((polar.max_zoom - (20.0 - 0.05f64.log2())).abs() < 1e-12, "{polar:?}");
        assert_eq!(polar.max_pitch, 40.0);
        // at the equator nothing moves
        let equator = limits(Basemap::Dark, Some(&FollowLock { latitude: 0.0, max_zoom: 20.5, max_pitch: 85.0 }));
        assert_eq!(equator.max_zoom, 20.5);
    }

    #[test]
    fn follow_max_zoom_closes_in_until_the_aircraft_fills_the_screen() {
        // an A320-sized 37.6 m aircraft at the equator on a 900 px map: the camera may come down to
        // 1.3 lengths, 1.5 heights back, so one pixel is 1.3·37.6 / 1350 m
        let mpp0: f64 = 40_075_016.686 / 512.0;
        let want = (mpp0 / (1.3 * 37.6 / 1350.0)).log2();
        assert!(want > 20.0 && want < 23.0, "{want}");
        assert!((follow_max_zoom(37.6, 0.0, Some(900.0)) - want).abs() < 1e-12);
        // no map yet: 900 px is assumed
        assert_eq!(follow_max_zoom(37.6, 0.0, None), follow_max_zoom(37.6, 0.0, Some(900.0)));
        // further north the same aircraft spans more of the zoom-0 world's pixels
        assert!((follow_max_zoom(37.6, 60.0, Some(900.0)) - (want - 1.0)).abs() < 1e-12);
        // a map twice as tall puts the camera twice as many pixels from its target, so it may come
        // a zoom level closer
        assert!((follow_max_zoom(37.6, 0.0, Some(1800.0)) - (want + 1.0)).abs() < 1e-12);
        // never below the free camera's 20, never past 23
        assert_eq!(follow_max_zoom(400.0, 0.0, Some(900.0)), 20.0);
        assert_eq!(follow_max_zoom(2.0, 0.0, Some(900.0)), 23.0);
        // a zero height counts as one pixel, and the camera may then come no closer than 20
        assert_eq!(follow_max_zoom(37.6, 0.0, Some(0.0)), 20.0);
    }

    #[test]
    fn zoom_adjust_is_log2_of_pi_cos_latitude() {
        assert!((zoom_adjust(0.0, false) - PI.log2()).abs() < 1e-15);
        assert!((zoom_adjust(60.0, false) - (PI / 2.0).log2()).abs() < 1e-12);
        assert_eq!(zoom_adjust(-60.0, true), zoom_adjust(60.0, false));
        assert_eq!(zoom_adjust(90.0, true), zoom_adjust(MAX_LATITUDE, false));
        assert_eq!(zoom_adjust(-89.0, true), zoom_adjust(-MAX_LATITUDE, false));
        // unclamped, the pole is a singularity
        assert!(zoom_adjust(90.0, false) < -40.0);
    }

    #[test]
    fn mercator_world_coordinates_grow_north() {
        assert_eq!(lng_lat_to_world([-180.0, 0.0]), [0.0, 256.0]);
        assert_eq!(lng_lat_to_world([0.0, 0.0]), [256.0, 256.0]);
        let [x, y] = lng_lat_to_world([90.0, MAX_LATITUDE]);
        assert!((x - 384.0).abs() < 1e-12 && (y - 512.0).abs() < 1e-4, "{x} {y}");
        let [lon, lat] = world_to_lng_lat(lng_lat_to_world([18.0686, 59.3293]));
        assert!((lon - 18.0686).abs() < 1e-12 && (lat - 59.3293).abs() < 1e-12);
        // the flat viewport clamps y as deck's shader clamps latitude to ±89.9
        let flat = viewport_for(&view(0.0, 0.0, 13.0, 0.0, 0.0), DESK);
        assert_eq!(flat.mercator().unwrap().project_flat([0.0, 89.99]), [256.0, 830.0]);
        assert_eq!(flat.mercator().unwrap().project_flat([0.0, -89.99]), [256.0, -318.0]);
    }

    #[test]
    fn the_globe_puts_lon_0_lat_0_at_minus_y() {
        let vp = viewport_for(&ViewState::INITIAL, DESK);
        let g = vp.globe().unwrap();
        let close = |a: DVec3, b: DVec3| (a - b).length() < 1e-12;
        assert!(close(g.project_position([0.0, 0.0, 0.0]), DVec3::new(0.0, -256.0, 0.0)));
        assert!(close(g.project_position([0.0, 90.0, 0.0]), DVec3::new(0.0, 0.0, 256.0)));
        assert!(close(g.project_position([90.0, 0.0, 0.0]), DVec3::new(256.0, 0.0, 0.0)));
        // heights scale the radius by deck's Earth
        let raised = g.project_position([0.0, 0.0, GLOBE_EARTH_RADIUS_M]);
        assert!(close(raised, DVec3::new(0.0, -512.0, 0.0)));
        let [lon, lat, z] = g.unproject_position(g.project_position([-73.98, 40.75, 1234.0]));
        assert!((lon + 73.98).abs() < 1e-12 && (lat - 40.75).abs() < 1e-12 && (z - 1234.0).abs() < 1e-6);
        assert_eq!(g.resolution, 5.0);
    }

    #[test]
    fn a_zero_sized_canvas_counts_as_one_pixel() {
        let vp = viewport_for(&ViewState::INITIAL, [0.0, 0.0]);
        assert_eq!((vp.width, vp.height), (1.0, 1.0));
        let [x, y, _] = vp.project([10.0, 35.0, 0.0]);
        assert!((x - 0.5).abs() < 1e-9 && (y - 0.5).abs() < 1e-9);
    }

    #[test]
    fn viewports_serve_screen_angle() {
        // screen::Projector: a north-up flat map points a northbound icon straight up
        let flat = viewport_for(&view(18.0, 59.0, 14.0, 0.0, 0.0), DESK);
        let angle = crate::screen::screen_angle(&flat, [18.0, 59.0, 0.0], 0.0, 100.0).unwrap();
        assert!(angle.abs() < 1e-6, "{angle}");
        // turned by the bearing
        let turned = viewport_for(&view(18.0, 59.0, 14.0, 0.0, 40.0), DESK);
        let angle = crate::screen::screen_angle(&turned, [18.0, 59.0, 0.0], 0.0, 100.0).unwrap();
        assert!((angle - 40.0).abs() < 1e-6, "{angle}");
        let globe = viewport_for(&view(18.0, 59.0, 4.0, 0.0, 40.0), DESK);
        let angle = crate::screen::screen_angle(globe.globe().unwrap(), [18.0, 59.0, 0.0], 0.0, 1000.0).unwrap();
        assert!((angle - 40.0).abs() < 1e-3, "{angle}");
    }
}
