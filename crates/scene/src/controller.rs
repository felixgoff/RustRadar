//! The camera controller: how gestures and the app's own moves change the [`ViewState`] (doc 01
//! §3.1–3.8, §3.12).
//!
//! Three layers, each ported from where it lives today:
//! - **deck.gl 9.4's `GlobeController`** with the app's `NorthUpGlobeController` on top
//!   (`src/lib/controller.ts`): the gesture handlers of deck's `Controller` (drag to pan or rotate,
//!   wheel zoom, double-click, touch pinch and two-finger pan, arrow keys), the state operations of
//!   `GlobeState`/`MapState` with the north-up pan and the bearing kept by `applyConstraints`, and
//!   deck's `TransitionManager` with its `LinearInterpolator` (no easing, no inertia).
//! - **`Globe.svelte`'s own moves** (G:3245-3289, 3518-3592): `fly_to`, `jump_to`, `zoom_by`,
//!   `reset_orientation`, `set_pitch`, `move_view`, `zoom_about`, the trackpad's two-finger pan and
//!   WebKit's pinch-and-twist, and `transitionUntil`.
//! - **`trackpad.ts`**: telling a trackpad's scroll from a mouse wheel's, wheel deltas in pixels,
//!   the pinch's zoom and bearing.
//!
//! [`Controller`] holds the camera as deck's controller holds it (its props: the view, the canvas
//! size, the limits) plus the gesture in progress and the running transition. Nothing reads a
//! clock: every call that can start or advance a transition takes `now_ms` (the local clock;
//! camera transitions run on it, doc 03 §20.12). The app calls [`Controller::update`] once per
//! frame and reads [`Controller::view`].
//!
//! The three zoom/latitude couplings of deck's globe are all here (doc 01 §2.2, §2.4): the zoom
//! limits shift by `log2(cos lat)`; a pan north or south adds `log2(cos new / cos old)` to the zoom;
//! and an anchored zoom that moves the latitude adjusts the zoom the same way.
//!
//! The lock on an aircraft is a pure function here ([`locked_view`], `lockedView`, item A10, with
//! [`released_ease_back`]); the app's `state::camera` runs it each frame and re-aims through
//! [`Controller::set_view`], which covers both of today's call sites (each rendered frame, and
//! `onViewStateChange` after each gesture). Not here: the mapping of winit's events onto these
//! calls (item B8).

use glam::DMat4;
use rustradar_core::geo::{cos_angle, effective_exaggeration, elevation, meters_per_pixel, unwrap_longitude};
use rustradar_core::motion::aircraft::Pose;

use crate::camera::{
    viewport_for, zoom_adjust, Limits, ViewState, Viewport, CAMERA_HEIGHTS, FLAT_ABOVE_ZOOM, FOLLOW_MAX_PITCH, LIMITS, MAX_LATITUDE,
    MIN_PITCH,
};
use crate::geom::{js_max, js_min};

/// `Math.PI / 180`.
const DEG: f64 = std::f64::consts::PI / 180.0;
/// `180 / Math.PI` (deck's `RADIANS_TO_DEGREES`).
const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;

// ---------------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------------

/// The app's own pans keep this far from the poles, where longitude stops meaning anything
/// (`controller.ts` `MAX_LATITUDE`; `moveView` clamps to it too). deck itself allows ±90.
pub const MAX_PAN_LATITUDE: f64 = 85.0;
/// `flyTo`'s transition, milliseconds (G:3266).
pub const FLY_TO_MS: f64 = 1500.0;
/// `zoomBy`'s transition (G:3279).
pub const ZOOM_BY_MS: f64 = 250.0;
/// `resetOrientation`'s and `setPitch`'s transition (G:3284, 3288).
pub const ORIENTATION_MS: f64 = 400.0;
/// Following a train or ship: the flight there (G:3167).
pub const FOLLOW_VEHICLE_MS: f64 = 800.0;
/// Easing back inside the free camera's limits: after a lock ends closer or steeper than they
/// allow, or when the basemap changes to one that tilts less (G:3215-3221, 3756).
pub const EASE_BACK_MS: f64 = 600.0;
/// GlobeController's own transition, for double-click and the arrow keys (deck
/// `GlobeController.transition.transitionDuration`).
pub const DECK_TRANSITION_MS: f64 = 300.0;
/// The tilt key `t` and the tilt button go to this pitch, or back to 0 from above 5° (`TILT`,
/// `+page.svelte:107, 1210`).
pub const TILT: f64 = 55.0;
/// Following a train or ship, the camera flies in at least this close (G:3167).
pub const FOLLOW_VEHICLE_MIN_ZOOM: f64 = 13.0;
/// `flyTo` without a zoom lands at least this close (or at this zoom when the target is far).
pub const FLY_TO_ZOOM: f64 = 8.0;

/// deck `Controller._onWheel`: `scrollZoom` speed.
const SCROLL_ZOOM_SPEED: f64 = 0.01;
/// mjolnir `WheelInput`: a line of wheel delta is this many pixels...
const WHEEL_DELTA_PER_LINE: f64 = 40.0;
/// ...and Shift slows the zoom down to a quarter.
const SHIFT_MULTIPLIER: f64 = 0.25;
/// deck `MapState._getNewRotation`: a drag must start this far from the top or bottom edge to tilt.
const PITCH_MOUSE_THRESHOLD: f64 = 5.0;
/// ...and dragging down tilts back this much faster.
const PITCH_ACCEL: f64 = 1.2;
/// deck `isGlobeNorthUp`: bearings within this many degrees of north count as north up.
const NORTH_UP_BEARING_THRESHOLD: f64 = 1.0;
/// deck's arrow keys: pan this many pixels...
const KEY_MOVE_SPEED: f64 = 100.0;
/// ...turn this many degrees (with a modifier, left and right)...
const KEY_ROTATE_SPEED_X: f64 = 15.0;
/// ...and tilt this many (with a modifier, up and down).
const KEY_ROTATE_SPEED_Y: f64 = 10.0;
/// deck blocks drag and pinch starts this long after a double-click (`blockEvents(100)`).
const DOUBLE_CLICK_BLOCK_MS: f64 = 100.0;
/// `trackpad.ts` `wheelPixels`: a line of a trackpad's scroll is this many pixels.
const TRACKPAD_LINE_PX: f64 = 16.0;
/// `trackpad.ts` `pinched`: the smallest pinch scale taken.
const PINCH_MIN_SCALE: f64 = 0.01;
/// math.gl's `config.EPSILON`, the relative tolerance of `equals`.
const MATH_GL_EPSILON: f64 = 1e-12;

// ---------------------------------------------------------------------------------------------
// trackpad.ts
// ---------------------------------------------------------------------------------------------

/// How a wheel event's deltas are measured (the DOM's `WheelEvent.deltaMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeltaMode {
    /// `DOM_DELTA_PIXEL` (0).
    #[default]
    Pixel,
    /// `DOM_DELTA_LINE` (1).
    Line,
    /// `DOM_DELTA_PAGE` (2).
    Page,
}

/// A wheel event as `trackpad.ts` and mjolnir read it. Deltas are positive to the right and down
/// (the DOM's sign). winit's `MouseScrollDelta::PixelDelta` is a pixel delta and `LineDelta` a line
/// delta, both with the opposite sign (item B8 maps them).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WheelInput {
    pub delta_x: f64,
    pub delta_y: f64,
    pub delta_mode: DeltaMode,
    /// Ctrl is held: a trackpad's pinch arrives this way outside WebKit.
    pub ctrl_key: bool,
    /// Shift is held: the wheel zooms at a quarter of the speed.
    pub shift_key: bool,
    /// WebKit's legacy `wheelDeltaY`, where the platform reports one.
    pub wheel_delta_y: Option<f64>,
}

/// Whether a wheel event comes from a trackpad's two-finger scroll rather than a mouse wheel's
/// notches (`trackpad.ts` `isTrackpadScroll`): sideways movement, WebKit's `wheelDeltaY` at three
/// times the pixel delta (a wheel's are multiples of 120), or pixel deltas that aren't whole
/// lines. Ctrl + wheel is a pinch, so never a scroll.
pub fn is_trackpad_scroll(e: &WheelInput) -> bool {
    if e.ctrl_key {
        return false;
    }
    if e.delta_x != 0.0 {
        return true;
    }
    if let Some(legacy) = e.wheel_delta_y
        && legacy != 0.0
    {
        return legacy == -3.0 * e.delta_y;
    }
    e.delta_mode == DeltaMode::Pixel && !is_integer(e.delta_y)
}

/// A wheel event's movement in pixels, line and page deltas scaled to them (`trackpad.ts`
/// `wheelPixels`). `page_height` is the map's height in CSS pixels.
pub fn wheel_pixels(e: &WheelInput, page_height: f64) -> [f64; 2] {
    let k = match e.delta_mode {
        DeltaMode::Line => TRACKPAD_LINE_PX,
        DeltaMode::Page => page_height,
        DeltaMode::Pixel => 1.0,
    };
    [e.delta_x * k, e.delta_y * k]
}

/// A bearing in deck.gl's range, `[-180, 180)` degrees (`trackpad.ts` `wrapBearing`; the same
/// formula `NorthUpGlobeController.applyConstraints` uses).
pub fn wrap_bearing(b: f64) -> f64 {
    (((b % 360.0) + 540.0) % 360.0) - 180.0
}

/// A zoom and a bearing: where a pinch started and where it has got to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomBearing {
    pub zoom: f64,
    pub bearing: f64,
}

/// The zoom and bearing a pinch has reached from where it started (`trackpad.ts` `pinched`):
/// fingers apart double the scale for each zoom level; turned clockwise (`rotation` in degrees,
/// clockwise positive, as WebKit's `GestureEvent.rotation`), the map turns with them. `scale` is
/// cumulative since the pinch began (WebKit's `GestureEvent.scale`).
pub fn pinched(start: ZoomBearing, scale: f64, rotation: f64) -> ZoomBearing {
    ZoomBearing { zoom: start.zoom + js_max(PINCH_MIN_SCALE, scale).log2(), bearing: wrap_bearing(start.bearing - rotation) }
}

/// JavaScript's `Number.isInteger`.
fn is_integer(x: f64) -> bool {
    x.is_finite() && x.trunc() == x
}

// ---------------------------------------------------------------------------------------------
// The wheel, as mjolnir and deck read it
// ---------------------------------------------------------------------------------------------

/// The wheel delta deck's controller zooms by (mjolnir `WheelInput`): `−deltaY`, a line counted as
/// 40 pixels, a quarter with Shift. (mjolnir also divides pixel deltas by the device pixel ratio
/// in Firefox; none of the app's webviews is Firefox.)
pub fn wheel_delta(e: &WheelInput) -> f64 {
    let mut value = e.delta_y;
    if e.delta_mode == DeltaMode::Line {
        value *= WHEEL_DELTA_PER_LINE;
    }
    // `event.shiftKey && value`: 0 and NaN are falsy
    if e.shift_key && value != 0.0 && !value.is_nan() {
        value *= SHIFT_MULTIPLIER;
    }
    -value
}

/// The relative scale a wheel delta zooms by (deck `Controller._onWheel`, not smooth):
/// `2 / (1 + e^−|0.01·delta|)`, between 1 and 2, inverted for a negative delta. A typical 100-pixel
/// notch is about 1.462, or 0.548 zoom levels.
pub fn wheel_scale(delta: f64) -> f64 {
    let mut scale = 2.0 / (1.0 + (-(delta * SCROLL_ZOOM_SPEED).abs()).exp());
    if delta < 0.0 && scale != 0.0 {
        scale = 1.0 / scale;
    }
    scale
}

/// Where a point on screen is on the map, `[lon, lat]`; `None` when the result is not finite
/// (`Globe.svelte` `mapAt`, G:3518-3521). On the globe a pixel off its limb still gives a point
/// beside the limb ([`crate::camera::GlobeViewport::unproject`]).
pub fn map_at(view: &ViewState, size: [f64; 2], at: [f64; 2]) -> Option<[f64; 2]> {
    let p = viewport_for(view, size).unproject(at, 0.0);
    (p[0].is_finite() && p[1].is_finite()).then_some(p)
}

// ---------------------------------------------------------------------------------------------
// The globe's camera frame (deck `viewports/globe-utils.js` `Globe`)
// ---------------------------------------------------------------------------------------------

/// deck's camera frame for panning: the view centre on the unit sphere, the camera's up, the two
/// rotation axes of a trackball drag, and the frame's longitude, latitude and bearing. (This unit
/// sphere has lon 0 / lat 0 at `+x` and lon 90°E at `+y`, unlike the viewport's common space.)
#[derive(Debug, Clone, Copy, PartialEq)]
struct CameraFrame {
    position: [f64; 3],
    up: [f64; 3],
    axis_horizontal: [f64; 3],
    axis_vertical: [f64; 3],
    longitude: f64,
    latitude: f64,
    bearing: f64,
}

/// `Globe.toPosition`: `[lon, lat]` in degrees to the unit sphere.
fn to_position(lng: f64, lat: f64) -> [f64; 3] {
    let phi = lat * DEG;
    let lam = lng * DEG;
    let cp = phi.cos();
    [cp * lam.cos(), cp * lam.sin(), phi.sin()]
}

/// `Globe.toLngLat`.
fn to_lng_lat(v: [f64; 3]) -> [f64; 2] {
    [v[1].atan2(v[0]) * RAD_TO_DEG, math_gl_clamp(v[2], -1.0, 1.0).asin() * RAD_TO_DEG]
}

/// `Globe.tangentBasis`: the north and east tangents at a point, `(N, E)`.
fn tangent_basis(lng: f64, lat: f64) -> ([f64; 3], [f64; 3]) {
    let phi = lat * DEG;
    let lam = lng * DEG;
    let sp = phi.sin();
    let cp = phi.cos();
    let sl = lam.sin();
    let cl = lam.cos();
    ([-sp * cl, -sp * sl, cp], [-sl, cl, 0.0])
}

/// `Globe.upVector`: the camera's up at a point for a bearing.
fn up_vector(lng: f64, lat: f64, bearing: f64) -> [f64; 3] {
    let (n, e) = tangent_basis(lng, lat);
    let b = bearing * DEG;
    let cb = b.cos();
    let sb = b.sin();
    [n[0] * cb + e[0] * sb, n[1] * cb + e[1] * sb, n[2] * cb + e[2] * sb]
}

/// `Globe.bearing`: the bearing, degrees, of a camera up vector at a point.
fn frame_bearing(up: [f64; 3], lng: f64, lat: f64) -> f64 {
    let (n, e) = tangent_basis(lng, lat);
    dot(up, e).atan2(dot(up, n)) * RAD_TO_DEG
}

/// `Globe.cameraFrame`.
fn camera_frame(lng: f64, lat: f64, bearing: f64) -> CameraFrame {
    let position = to_position(lng, lat);
    let up = up_vector(lng, lat, bearing);
    let (n, e) = tangent_basis(lng, lat);
    let b = bearing * DEG;
    let cb = b.cos();
    let sb = b.sin();
    let right = [e[0] * cb - n[0] * sb, e[1] * cb - n[1] * sb, e[2] * cb - n[2] * sb];
    CameraFrame {
        position,
        up,
        axis_horizontal: cross(position, right),
        axis_vertical: cross(position, up),
        longitude: lng,
        latitude: lat,
        bearing,
    }
}

/// `Globe.rotate`: a vector turned about a unit axis by an angle in radians, through math.gl's
/// quaternion (`fromAxisRotation`, then `vec3.transformQuat`).
fn rotate(v: [f64; 3], axis: [f64; 3], angle: f64) -> [f64; 3] {
    // quat.setAxisAngle
    let half = angle * 0.5;
    let s = half.sin();
    let (qx, qy, qz, qw) = (s * axis[0], s * axis[1], s * axis[2], half.cos());
    // vec3.transformQuat
    let [x, y, z] = v;
    let mut uvx = qy * z - qz * y;
    let mut uvy = qz * x - qx * z;
    let mut uvz = qx * y - qy * x;
    let mut uuvx = qy * uvz - qz * uvy;
    let mut uuvy = qz * uvx - qx * uvz;
    let mut uuvz = qx * uvy - qy * uvx;
    let w2 = qw * 2.0;
    uvx *= w2;
    uvy *= w2;
    uvz *= w2;
    uuvx *= 2.0;
    uuvy *= 2.0;
    uuvz *= 2.0;
    [x + uvx + uuvx, y + uvy + uuvy, z + uvz + uuvz]
}

/// `Globe.rotateFrame(frame, horizontalAngle, verticalAngle)`: the trackball's drag, radians.
fn rotate_frame(frame: &CameraFrame, horizontal_angle: f64, vertical_angle: f64) -> CameraFrame {
    let mut position = rotate(frame.position, frame.axis_horizontal, horizontal_angle);
    position = rotate(position, frame.axis_vertical, vertical_angle);
    let mut up = rotate(frame.up, frame.axis_horizontal, horizontal_angle);
    up = rotate(up, frame.axis_vertical, vertical_angle);
    let [longitude, latitude] = to_lng_lat(position);
    let bearing = frame_bearing(up, longitude, latitude);
    CameraFrame { position, up, longitude, latitude, bearing, ..*frame }
}

/// `Globe.rotateFrameToMatch`: the frame turned (rigidly, up vector with it) so the globe point at
/// `current` moves toward `target`, `strength` of the way (0 … 1).
fn rotate_frame_to_match(frame: &CameraFrame, current: [f64; 2], target: [f64; 2], strength: f64) -> CameraFrame {
    let current_position = to_position(current[0], current[1]);
    let target_position = to_position(target[0], target[1]);
    let mut rotation_axis = cross(current_position, target_position);
    let axis_length = length(rotation_axis);
    let cosine = math_gl_clamp(dot(current_position, target_position), -1.0, 1.0);
    if axis_length < 1e-12 {
        if cosine > 0.0 {
            return *frame;
        }
        rotation_axis = cross(current_position, frame.up);
        if length(rotation_axis) < 1e-12 {
            rotation_axis = cross(current_position, frame.axis_vertical);
        }
    }
    rotation_axis = normalize(rotation_axis);
    let angle = axis_length.atan2(cosine) * math_gl_clamp(strength, 0.0, 1.0);
    let position = rotate(frame.position, rotation_axis, angle);
    let up = rotate(frame.up, rotation_axis, angle);
    let [longitude, latitude] = to_lng_lat(position);
    CameraFrame { position, up, longitude, latitude, bearing: frame_bearing(up, longitude, latitude), ..*frame }
}

/// `vec3.cross`.
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    let [ax, ay, az] = a;
    let [bx, by, bz] = b;
    [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx]
}

/// `vec3.dot`.
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `vec3.length`.
fn length(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// `vec3.normalize`.
fn normalize(a: [f64; 3]) -> [f64; 3] {
    let mut len = a[0] * a[0] + a[1] * a[1] + a[2] * a[2];
    if len > 0.0 {
        len = 1.0 / len.sqrt();
    }
    [a[0] * len, a[1] * len, a[2] * len]
}

/// math.gl's `clamp(value, min, max)`: `Math.max(min, Math.min(max, value))` (NaN stays NaN).
fn math_gl_clamp(value: f64, min: f64, max: f64) -> f64 {
    js_max(min, js_min(max, value))
}

/// deck's `mod(value, divisor)`: the remainder made non-negative.
fn modulo(value: f64, divisor: f64) -> f64 {
    let modulus = value % divisor;
    if modulus < 0.0 { divisor + modulus } else { modulus }
}

/// `x || 0` for a number.
fn js_or0(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() { 0.0 } else { x }
}

/// math.gl's `lerp(a, b, t)` for numbers: `t·b + (1 − t)·a`.
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    t * b + (1.0 - t) * a
}

/// math.gl's `equals(a, b)` for numbers: equal within `1e-12` of the larger magnitude (or of 1).
fn math_gl_equals(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= MATH_GL_EPSILON * js_max(1.0, js_max(a.abs(), b.abs()))
}

/// deck `isGlobeNorthUp`: within a degree of north.
fn is_globe_north_up(bearing: f64) -> bool {
    let normalized_bearing = modulo(bearing + 180.0, 360.0) - 180.0;
    normalized_bearing.abs() < NORTH_UP_BEARING_THRESHOLD
}

// ---------------------------------------------------------------------------------------------
// The controller's state (deck `GlobeState` / `MapState` with the app's `NorthUpState`)
// ---------------------------------------------------------------------------------------------

/// What deck's controller state is built from (its viewport props): the view, the canvas size in
/// CSS pixels and the limits (`minPitch` is always [`MIN_PITCH`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Props {
    view: ViewState,
    width: f64,
    height: f64,
    limits: Limits,
}

impl Props {
    /// deck's `makeViewport(props)`: the globe up to zoom 12, the flat map above.
    fn viewport(&self) -> Viewport {
        viewport_for(&self.view, [self.width, self.height])
    }
}

/// What a gesture in progress remembers (deck's controller `state`). `start_zoom` is one field
/// shared by the pan and the zoom gestures, as in deck.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct GestureState {
    start_pan_pos: Option<[f64; 2]>,
    start_pan_camera_frame: Option<CameraFrame>,
    /// Radians of arc per pixel.
    start_pan_angular_rate: Option<f64>,
    start_zoom: Option<f64>,
    start_zoom_lng_lat: Option<[f64; 2]>,
    start_rotate_pos: Option<[f64; 2]>,
    start_bearing: Option<f64>,
    start_pitch: Option<f64>,
}

/// deck's `CONSTRAINT_AROUND`: keep the map point `position` (`[lon, lat]`) under the pixel
/// `screen_position` once the zoom is constrained.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ConstraintAround {
    position: [f64; 2],
    screen_position: [f64; 2],
}

/// `NorthUpGlobeController.applyConstraints` over `GlobeState.applyConstraints` (with no
/// `maxBounds`, as the app has none): clamp the zoom to the latitude-shifted limits; anchor a zoom
/// by turning the globe's camera frame; wrap longitude and bearing; clamp latitude to ±90 and the
/// pitch; add `zoom_adjust(new lat) − zoom_adjust(old lat)` to the zoom when the latitude moved;
/// then put the bearing back as it came in, wrapped (so zooming toward the cursor never turns the
/// map).
fn apply_constraints(props: &mut Props, around: Option<ConstraintAround>) {
    // NorthUpState: the bearing before deck's constraints
    let bearing_in = props.view.bearing;
    let latitude = props.view.latitude;
    props.view.zoom = constrain_zoom(props.view.zoom, props);
    if let Some(around) = around {
        let viewport = props.viewport();
        let anchor_strength = viewport.zoom_anchor_strength(around.screen_position);
        if anchor_strength > 0.0 {
            let current = viewport.unproject(around.screen_position, 0.0);
            let frame = camera_frame(props.view.longitude, props.view.latitude, js_or0(props.view.bearing));
            let rotated = rotate_frame_to_match(&frame, current, around.position, anchor_strength);
            props.view.longitude = rotated.longitude;
            props.view.latitude = rotated.latitude;
            props.view.bearing = rotated.bearing;
        }
    }
    let v = &mut props.view;
    if v.longitude < -180.0 || v.longitude > 180.0 {
        v.longitude = modulo(v.longitude + 180.0, 360.0) - 180.0;
    }
    if v.bearing < -180.0 || v.bearing > 180.0 {
        v.bearing = modulo(v.bearing + 180.0, 360.0) - 180.0;
    }
    v.latitude = math_gl_clamp(v.latitude, -90.0, 90.0);
    v.pitch = math_gl_clamp(v.pitch, MIN_PITCH, props.limits.max_pitch);
    // (maxBounds may extend past the globe's range: deck clamps again)
    v.latitude = math_gl_clamp(v.latitude, -90.0, 90.0);
    if v.latitude != latitude {
        v.zoom += zoom_adjust(v.latitude, true) - zoom_adjust(latitude, true);
    }
    v.bearing = wrap_bearing(bearing_in);
}

/// `GlobeState._constrainZoom` without `maxBounds`: the limits shifted by
/// `zoom_adjust(lat) − zoom_adjust(0) = log2(cos lat)` (doc 01 §2.4: at 60°N the free range is
/// −0.5 … 19).
fn constrain_zoom(zoom: f64, props: &Props) -> f64 {
    let zoom_adjustment = zoom_adjust(props.view.latitude, true) - zoom_adjust(0.0, true);
    math_gl_clamp(zoom, props.limits.min_zoom + zoom_adjustment, props.limits.max_zoom + zoom_adjustment)
}

/// One of deck's (immutable) controller states: constrained props and the gesture's memory. Each
/// operation is `_getUpdatedState`: the props and state merged with the change, constrained anew.
/// An operation that gives `None` is deck's `return this` (nothing to do).
#[derive(Debug, Clone, Copy, PartialEq)]
struct GlobeState {
    props: Props,
    state: GestureState,
}

impl GlobeState {
    /// `new ControllerState({ ...props, ...state })`: the props constrained.
    fn new(mut props: Props, state: GestureState, around: Option<ConstraintAround>) -> Self {
        apply_constraints(&mut props, around);
        GlobeState { props, state }
    }

    /// `_getUpdatedState(change)`.
    fn updated(&self, change: impl FnOnce(&mut Props, &mut GestureState)) -> Self {
        self.updated_around(None, change)
    }

    /// `_getUpdatedState({ ...change, [CONSTRAINT_AROUND]: around })`.
    fn updated_around(&self, around: Option<ConstraintAround>, change: impl FnOnce(&mut Props, &mut GestureState)) -> Self {
        let (mut props, mut state) = (self.props, self.state);
        change(&mut props, &mut state);
        GlobeState::new(props, state, around)
    }

    /// `_unproject(pos)`: the map point under a pixel, with this state's viewport.
    fn unproject(&self, pos: [f64; 2]) -> [f64; 2] {
        self.props.viewport().unproject(pos, 0.0)
    }

    /// `GlobeState.panStart`: the pointer, the camera frame and the pan's rate, radians of arc per
    /// pixel: `0.25° / 2^(zoom − zoom_adjust(lat))`.
    fn pan_start(&self, pos: [f64; 2]) -> Self {
        let ViewState { latitude, longitude, zoom, bearing, .. } = self.props.view;
        let camera_frame = camera_frame(longitude, latitude, bearing);
        let scale = 2f64.powf(zoom - zoom_adjust(latitude, true));
        let angular_rate = (0.25 / scale) * DEG;
        self.updated(|_, s| {
            s.start_pan_pos = Some(pos);
            s.start_pan_camera_frame = Some(camera_frame);
            s.start_pan_angular_rate = Some(angular_rate);
            s.start_zoom = Some(zoom);
        })
    }

    /// `NorthUpState.pan` (`controller.ts:55-73`): the drag turned into north and east by the
    /// start bearing, latitude clamped to ±85, longitude stretched by `1/max(0.15, cos lat)`, the
    /// bearing kept, and the zoom of deck's own trackball pan (its latitude-coupling term).
    fn pan(&self, pos: [f64; 2], start_pos: Option<[f64; 2]>) -> Option<Self> {
        let start = self.state.start_pan_pos.or(start_pos)?;
        let frame = self.state.start_pan_camera_frame?;
        // `!rate`: 0 and NaN are falsy
        let rate = self.state.start_pan_angular_rate.filter(|r| *r != 0.0 && !r.is_nan())?;
        // the trackball's result only for its altitude-dependent zoom correction
        let zoom = self.trackball_pan(pos, start, &frame, rate).props.view.zoom;

        let right = (start[0] - pos[0]) * rate;
        let up = (pos[1] - start[1]) * rate;
        // screen axes to north and east, undoing the camera's bearing
        let b = frame.bearing * DEG;
        let north = up * b.cos() - right * b.sin();
        let east = up * b.sin() + right * b.cos();
        let latitude = js_max(-MAX_PAN_LATITUDE, js_min(MAX_PAN_LATITUDE, frame.latitude + north / DEG));
        let longitude = frame.longitude + east / DEG / js_max(0.15, (frame.latitude * DEG).cos());
        Some(self.updated(|p, _| {
            p.view.longitude = longitude;
            p.view.latitude = latitude;
            p.view.bearing = frame.bearing;
            p.view.zoom = zoom;
        }))
    }

    /// `GlobeState.pan`, deck's trackball: the camera frame turned by the drag, and the zoom
    /// `startZoom + zoom_adjust(new lat) − zoom_adjust(start lat)` (both clamped to the poles).
    fn trackball_pan(&self, pos: [f64; 2], start: [f64; 2], frame: &CameraFrame, rate: f64) -> Self {
        let start_zoom = self.state.start_zoom.unwrap_or(self.props.view.zoom);
        let dx = start[0] - pos[0];
        let dy = start[1] - pos[1];
        let h_angle = dx * rate;
        let v_angle = -dy * rate;
        let rotated = rotate_frame(frame, h_angle, v_angle);
        let zoom = start_zoom + zoom_adjust(rotated.latitude, true) - zoom_adjust(frame.latitude, true);
        self.updated(|p, _| {
            p.view.longitude = rotated.longitude;
            p.view.latitude = rotated.latitude;
            p.view.bearing = rotated.bearing;
            p.view.zoom = zoom;
        })
    }

    /// `GlobeState.panEnd`.
    fn pan_end(&self) -> Self {
        self.updated(|_, s| {
            s.start_pan_pos = None;
            s.start_pan_camera_frame = None;
            s.start_pan_angular_rate = None;
            s.start_zoom = None;
        })
    }

    /// `GlobeState._panFromCenter(offset)`: a pan from the middle of the canvas (the arrow keys).
    fn pan_from_center(&self, offset: [f64; 2]) -> Self {
        let center = [self.props.width / 2.0, self.props.height / 2.0];
        let started = self.pan_start(center);
        started.pan([center[0] + offset[0], center[1] + offset[1]], None).unwrap_or(started).pan_end()
    }

    /// `MapState.rotateStart` (no pivot: GlobeController supplies no `getAltitude`, so the camera
    /// turns about the view centre).
    fn rotate_start(&self, pos: [f64; 2]) -> Self {
        let ViewState { bearing, pitch, .. } = self.props.view;
        self.updated(|_, s| {
            s.start_rotate_pos = Some(pos);
            s.start_bearing = Some(bearing);
            s.start_pitch = Some(pitch);
        })
    }

    /// `MapState.rotate({ pos })`.
    fn rotate(&self, pos: [f64; 2]) -> Option<Self> {
        let start_rotate_pos = self.state.start_rotate_pos?;
        let start_bearing = self.state.start_bearing?;
        let start_pitch = self.state.start_pitch?;
        let (pitch, bearing) = self.new_rotation(pos, start_rotate_pos, start_pitch, start_bearing);
        Some(self.updated(|p, _| {
            p.view.pitch = pitch;
            p.view.bearing = bearing;
        }))
    }

    /// `MapState.rotateEnd`.
    fn rotate_end(&self) -> Self {
        self.updated(|_, s| {
            s.start_rotate_pos = None;
            s.start_bearing = None;
            s.start_pitch = None;
        })
    }

    /// `MapState._getNewRotation` (doc 01 §3.4): a full-width drag turns the map 180°; dragging up
    /// to the top of the canvas reaches the maximum pitch, dragging down eases back to 0.
    /// `(pitch, bearing)`.
    fn new_rotation(&self, pos: [f64; 2], start_pos: [f64; 2], start_pitch: f64, start_bearing: f64) -> (f64, f64) {
        let delta_x = pos[0] - start_pos[0];
        let delta_y = pos[1] - start_pos[1];
        let center_y = pos[1];
        let start_y = start_pos[1];
        let Props { width, height, .. } = self.props;
        let delta_scale_x = delta_x / width;
        let mut delta_scale_y = 0.0;
        if delta_y > 0.0 {
            if (height - start_y).abs() > PITCH_MOUSE_THRESHOLD {
                // move from 0 to -1 as we drag down
                delta_scale_y = (delta_y / (start_y - height)) * PITCH_ACCEL;
            }
        } else if delta_y < 0.0 && start_y > PITCH_MOUSE_THRESHOLD {
            // move from 0 to 1 as we drag up
            delta_scale_y = 1.0 - center_y / start_y;
        }
        delta_scale_y = math_gl_clamp(delta_scale_y, -1.0, 1.0);
        let (min_pitch, max_pitch) = (MIN_PITCH, self.props.limits.max_pitch);
        let bearing = start_bearing + 180.0 * delta_scale_x;
        let mut pitch = start_pitch;
        if delta_scale_y > 0.0 {
            pitch = start_pitch + delta_scale_y * (max_pitch - start_pitch);
        } else if delta_scale_y < 0.0 {
            pitch = start_pitch - delta_scale_y * (min_pitch - start_pitch);
        }
        (pitch, bearing)
    }

    /// `MapState.zoomStart`: the map point under the pinch and the zoom it started at.
    fn zoom_start(&self, pos: [f64; 2]) -> Self {
        let lng_lat = self.unproject(pos);
        let zoom = self.props.view.zoom;
        self.updated(|_, s| {
            s.start_zoom_lng_lat = Some(lng_lat);
            s.start_zoom = Some(zoom);
        })
    }

    /// `MapState.zoom({ pos, startPos, scale })`: the zoom `log2(scale)` from where the gesture
    /// started (a pinch) or from now (a wheel notch, a double-click), with the map point under the
    /// start kept under `pos` by [`apply_constraints`].
    fn zoom(&self, pos: [f64; 2], start_pos: Option<[f64; 2]>, scale: f64) -> Option<Self> {
        let (start_zoom, start_zoom_lng_lat) = match self.state.start_zoom_lng_lat {
            // a pan's end clears the shared `startZoom`; JavaScript's `null + x` is then `x`
            Some(lng_lat) => (self.state.start_zoom.unwrap_or(0.0), lng_lat),
            None => (self.props.view.zoom, self.unproject(start_pos.unwrap_or(pos))),
        };
        let zoom = start_zoom + scale.log2();
        let around = ConstraintAround { position: start_zoom_lng_lat, screen_position: pos };
        Some(self.updated_around(Some(around), |p, _| p.view.zoom = zoom))
    }

    /// `MapState.zoomEnd`.
    fn zoom_end(&self) -> Self {
        self.updated(|_, s| {
            s.start_zoom_lng_lat = None;
            s.start_zoom = None;
        })
    }

    /// `MapState.rotateLeft` / `rotateRight` / `rotateUp` / `rotateDown`: the arrow keys with a
    /// modifier.
    fn rotate_by(&self, d_bearing: f64, d_pitch: f64) -> Self {
        self.updated(|p, _| {
            p.view.bearing += d_bearing;
            p.view.pitch += d_pitch;
        })
    }

    /// `MapState.shortestPathFrom`: longitude and bearing moved by a turn to within 180° of the
    /// transition's start.
    fn shortest_path_from(&self, from: &GlobeState) -> Props {
        let from_view = from.props.view;
        let mut props = self.props;
        let ViewState { bearing, longitude, .. } = props.view;
        if (bearing - from_view.bearing).abs() > 180.0 {
            props.view.bearing = if bearing < 0.0 { bearing + 360.0 } else { bearing - 360.0 };
        }
        if (longitude - from_view.longitude).abs() > 180.0 {
            props.view.longitude = if longitude < 0.0 { longitude + 360.0 } else { longitude - 360.0 };
        }
        props
    }
}

/// `GlobeViewport.panByPosition(coords, pixel)` / `WebMercatorViewport.panByPosition`: the view
/// centre `[lon, lat]` that puts the map point `coords` at `pixel` (a transition anchored at a
/// pixel, deck's `LinearInterpolator({ around })`).
///
/// On the globe the correction is weakened by the zoom anchor strength (none off the limb); north
/// up, it is abandoned when the pixel's point lies beyond ±85.05° or the longitudes differ by more
/// than 90°, and scaled down so the latitude stays within ±85.05° (doc 01 §3.5).
fn pan_by_position(viewport: &Viewport, coords: [f64; 2], pixel: [f64; 2]) -> [f64; 2] {
    match viewport {
        Viewport::Globe(g) => {
            let mut anchor_strength = g.zoom_anchor_strength(pixel);
            if anchor_strength == 0.0 {
                return [g.longitude, g.latitude];
            }
            let current = g.unproject(pixel, 0.0);
            let longitude_delta = modulo(coords[0] - current[0] + 180.0, 360.0) - 180.0;
            let latitude_delta = coords[1] - current[1];
            let crosses_pole = current[1].abs() > MAX_LATITUDE || longitude_delta.abs() > 90.0;
            if is_globe_north_up(g.bearing) && crosses_pole {
                // past a pole the anchor could come back in the other hemisphere: zoom about the
                // centre instead
                return [g.longitude, g.latitude];
            }
            if is_globe_north_up(g.bearing) && latitude_delta != 0.0 {
                // out of latitude headroom, both axes are scaled down together
                let latitude_limit = if latitude_delta > 0.0 { MAX_LATITUDE } else { -MAX_LATITUDE };
                let latitude_constraint_strength = (latitude_limit - g.latitude) / latitude_delta;
                anchor_strength = js_min(anchor_strength, js_max(0.0, latitude_constraint_strength));
            }
            let longitude = g.longitude + longitude_delta * anchor_strength;
            let latitude = js_max(js_min(g.latitude + latitude_delta * anchor_strength, 90.0), -90.0);
            [longitude, latitude]
        }
        Viewport::Mercator(m) => {
            let from_location = pixels_to_world_on_ground(&m.pixel_unprojection_matrix, pixel);
            let to_location = m.project_flat(coords);
            let translate = [to_location[0] + -from_location[0], to_location[1] + -from_location[1]];
            let new_center = [m.center.x + translate[0], m.center.y + translate[1]];
            m.unproject_flat(new_center)
        }
    }
}

/// math.gl's `pixelsToWorld([x, y], pixelUnprojectionMatrix)`: the pixel's ray cut by the ground
/// plane (z = 0) in the flat map's common space, `[x, y]`.
fn pixels_to_world_on_ground(pixel_unprojection_matrix: &DMat4, [x, y]: [f64; 2]) -> [f64; 2] {
    let m = pixel_unprojection_matrix.to_cols_array();
    let coord0 = transform_vector(&m, [x, y, 0.0, 1.0]);
    let coord1 = transform_vector(&m, [x, y, 1.0, 1.0]);
    let z0 = coord0[2];
    let z1 = coord1[2];
    let t = if z0 == z1 { 0.0 } else { (0.0 - z0) / (z1 - z0) };
    // vec2.lerp
    [coord0[0] + t * (coord1[0] - coord0[0]), coord0[1] + t * (coord1[1] - coord0[1])]
}

/// deck's `transformVector`: `vec4.transformMat4`, then scaled by `1/w`.
fn transform_vector(m: &[f64; 16], [x, y, z, w]: [f64; 4]) -> [f64; 4] {
    let out = [
        m[0] * x + m[4] * y + m[8] * z + m[12] * w,
        m[1] * x + m[5] * y + m[9] * z + m[13] * w,
        m[2] * x + m[6] * y + m[10] * z + m[14] * w,
        m[3] * x + m[7] * y + m[11] * z + m[15] * w,
    ];
    let s = 1.0 / out[3];
    [out[0] * s, out[1] * s, out[2] * s, out[3] * s]
}

// ---------------------------------------------------------------------------------------------
// Transitions (deck `LinearInterpolator`, `TransitionManager`)
// ---------------------------------------------------------------------------------------------

/// Which view props a transition interpolates (deck's `transitionProps`; for the app's own moves
/// always the keys of the target).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PropSet([bool; 5]);

impl PropSet {
    /// GlobeController's own: `['longitude', 'latitude', 'zoom', 'bearing', 'pitch']`.
    const ALL: PropSet = PropSet([true; 5]);
}

/// The five animated props in deck's order.
fn values(v: &ViewState) -> [f64; 5] {
    [v.longitude, v.latitude, v.zoom, v.bearing, v.pitch]
}

/// Writes the props present in `values` into a view.
fn set_values(v: &mut ViewState, values: [Option<f64>; 5]) {
    let fields = [&mut v.longitude, &mut v.latitude, &mut v.zoom, &mut v.bearing, &mut v.pitch];
    for (field, value) in fields.into_iter().zip(values) {
        if let Some(value) = value {
            *field = value;
        }
    }
}

/// deck's `LinearInterpolator`: the props it moves and, for a double-click, the pixel it zooms
/// about.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Interpolator {
    props: PropSet,
    around: Option<[f64; 2]>,
}

impl Interpolator {
    /// `arePropsEqual`: each compared prop equal within math.gl's epsilon.
    fn props_equal(&self, a: &ViewState, b: &ViewState) -> bool {
        let (a, b) = (values(a), values(b));
        (0..5).all(|i| !self.props.0[i] || math_gl_equals(a[i], b[i]))
    }
}

/// A transition deck is asked for: its duration (milliseconds) and interpolator.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TransitionSpec {
    duration: f64,
    interpolator: Interpolator,
}

/// What `LinearInterpolator.initializeProps` adds for `around`: the pixel at the start, where the
/// anchored map point projects at the end, the point itself, and the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
struct AroundEnds {
    start_around: [f64; 2],
    end_around: [f64; 2],
    around_position: [f64; 2],
    width: f64,
    height: f64,
}

/// A running transition.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Transition {
    /// When it started (deck's clock starts at the frame that triggers it).
    start_ms: f64,
    duration: f64,
    interpolator: Interpolator,
    /// The interpolated props at the start (as given) and at the end (constrained, shortest way).
    start: [f64; 5],
    end: [f64; 5],
    around: Option<AroundEnds>,
    /// deck's `propsInTransition`: the view the last frame produced.
    in_transition: Option<Props>,
}

impl Transition {
    /// `LinearInterpolator.interpolateProps(start, end, t)`: each prop linearly, then for a
    /// transition anchored at a pixel the centre that keeps the anchored point under the pixel
    /// moving linearly between its two ends.
    fn interpolate(&self, t: f64) -> [Option<f64>; 5] {
        let mut out = [None; 5];
        for (i, slot) in out.iter_mut().enumerate() {
            if self.interpolator.props.0[i] {
                *slot = Some(lerp(js_or0(self.start[i]), js_or0(self.end[i]), t));
            }
        }
        if let Some(a) = &self.around {
            // `makeViewport({ ...endProps, ...propsInTransition })`: the end props are only the
            // interpolated ones, so no `position`
            let mut view = ViewState { longitude: 0.0, latitude: 0.0, zoom: 0.0, bearing: 0.0, pitch: 0.0, position: None };
            set_values(&mut view, out);
            let viewport = viewport_for(&view, [a.width, a.height]);
            let pixel = [lerp(a.start_around[0], a.end_around[0], t), lerp(a.start_around[1], a.end_around[1], t)];
            let [longitude, latitude] = pan_by_position(&viewport, a.around_position, pixel);
            out[0] = Some(longitude);
            out[1] = Some(latitude);
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// The controller
// ---------------------------------------------------------------------------------------------

/// deck's interaction state (`onInteractionStateChange`): `is_dragging` drives the grabbing cursor
/// (G:3714).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InteractionState {
    pub in_transition: bool,
    pub is_dragging: bool,
    pub is_panning: bool,
    pub is_rotating: bool,
    pub is_zooming: bool,
}

/// An arrow key, as deck's keyboard handler reads it (`keyboard: true`): it pans 100 px; with a
/// modifier, left and right turn the map 15° and up and down tilt it 10°. (deck's `-` and `=` are
/// not ported: the app has one zoom binding, `+ = -` → [`Controller::zoom_by`]; plan section 6,
/// item 10.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowKey {
    Left,
    Right,
    Up,
    Down,
}

/// A partial view for the app's moves (`Partial<ViewState>`): the fields that are `Some` are set,
/// and a transition to it interpolates exactly those.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewPatch {
    pub longitude: Option<f64>,
    pub latitude: Option<f64>,
    pub zoom: Option<f64>,
    pub bearing: Option<f64>,
    pub pitch: Option<f64>,
}

impl ViewPatch {
    fn values(&self) -> [Option<f64>; 5] {
        [self.longitude, self.latitude, self.zoom, self.bearing, self.pitch]
    }

    /// `{ ...view, ...patch }`.
    pub fn apply(&self, view: &ViewState) -> ViewState {
        let mut out = *view;
        set_values(&mut out, self.values());
        out
    }
}

/// WebKit's pinch in progress (G:3564-3592): the view it started from and where between the
/// fingers.
#[derive(Debug, Clone, Copy, PartialEq)]
struct WebKitPinch {
    start: ZoomBearing,
    at: [f64; 2],
}

/// The camera and what moves it: deck's GlobeController as the app configures it, its transition
/// manager, and `Globe.svelte`'s own moves. See the module documentation.
///
/// Positions on screen are CSS pixels from the canvas's top left corner.
#[derive(Debug, Clone, PartialEq)]
pub struct Controller {
    /// deck's controller props: the view as drawn now, the canvas, the limits.
    props: Props,
    /// The gesture's memory.
    gesture: GestureState,
    interaction: InteractionState,
    /// `controllerFor(locked)`: centre zoom, drags orbit, no two-finger pan.
    locked: bool,
    /// The drag in progress pans (else it rotates).
    pan_move: bool,
    /// A two-finger touch pan in progress: where the fingers started.
    multi_pan_start_center: Option<[f64; 2]>,
    /// deck's `blockEvents`: drag and pinch starts are ignored until then.
    event_start_blocked_until: f64,
    transition: Option<Transition>,
    /// `maxZoomNow()`.
    max_zoom_now: f64,
    /// `transitionUntil`: the end of the app's latest own move.
    transition_until: f64,
    pinch: Option<WebKitPinch>,
}

impl Controller {
    /// A free camera at `view` on a canvas of `size` CSS pixels, with the dark map's limits
    /// (`createDeck`: `initialViewState: { ...view, ...LIMITS }`, `controllerFor(false)`).
    pub fn new(view: ViewState, size: [f64; 2]) -> Self {
        Controller {
            props: Props { view, width: size[0], height: size[1], limits: LIMITS },
            gesture: GestureState::default(),
            interaction: InteractionState::default(),
            locked: false,
            pan_move: false,
            multi_pan_start_center: None,
            event_start_blocked_until: f64::NEG_INFINITY,
            transition: None,
            max_zoom_now: LIMITS.max_zoom,
            transition_until: 0.0,
            pinch: None,
        }
    }

    /// The camera now: what to draw, and what `onviewchange` reports (doc 01 §3.12).
    pub fn view(&self) -> ViewState {
        self.props.view
    }

    /// The viewport of [`Self::view`] on the canvas.
    pub fn viewport(&self) -> Viewport {
        self.props.viewport()
    }

    /// The canvas, `[width, height]` in CSS pixels.
    pub fn size(&self) -> [f64; 2] {
        [self.props.width, self.props.height]
    }

    /// The canvas was resized.
    pub fn set_size(&mut self, [width, height]: [f64; 2]) {
        self.props.width = width;
        self.props.height = height;
    }

    /// The limits gestures and transitions are held to (deck's `minZoom`, `maxZoom`, `maxPitch`).
    pub fn limits(&self) -> Limits {
        self.props.limits
    }

    /// The closest zoom reachable now ([`crate::camera::max_zoom_now`]), as `fly_to` and `zoom_by`
    /// cap with it.
    pub fn max_zoom_now(&self) -> f64 {
        self.max_zoom_now
    }

    /// The app's `limits()` and `maxZoomNow()` (`camera::limits`, `camera::max_zoom_now`), each time
    /// they change (lock, basemap). Like the limits handed to deck with a new view, they take
    /// effect from the next gesture, move or transition frame; the view is not clamped now.
    pub fn set_limits(&mut self, limits: Limits, max_zoom_now: f64) {
        self.props.limits = limits;
        self.max_zoom_now = max_zoom_now;
    }

    /// Whether the camera is locked on an aircraft, train or ship.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// `controllerFor(locked)` (doc 01 §3.1): locked, the wheel and pinches zoom about the centre,
    /// every drag orbits, two fingers do not pan and the trackpad's scroll is swallowed; free, the
    /// wheel zooms about the pointer and a plain drag pans.
    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    /// deck's interaction state.
    pub fn interaction(&self) -> InteractionState {
        self.interaction
    }

    /// A drag or pinch is under way (the cursor shows "grabbing").
    pub fn is_dragging(&self) -> bool {
        self.interaction.is_dragging
    }

    /// A transition is running: the app should draw every frame and call [`Self::update`].
    pub fn in_transition(&self) -> bool {
        self.transition.is_some()
    }

    /// `transitionUntil` (G:320): when the app's latest own move ([`Self::go_to`] and the moves
    /// built on it) is due to end, local milliseconds. It gates whatever would fight a transition:
    /// the follow re-aim, vehicle centring, terrain aim, trackpad pan (G:3174, 3188, 3208, 3558).
    /// It is not moved back when a gesture breaks the transition, as today.
    pub fn transition_until(&self) -> f64 {
        self.transition_until
    }

    /// Advances the running transition to `now_ms` (deck's `updateTransition`, once a frame).
    /// Returns whether the view moved; the transition ends once its duration has passed.
    pub fn update(&mut self, now_ms: f64) -> bool {
        let Some(tr) = &self.transition else {
            return false;
        };
        // luma's timeline channel: the time since the start, within [0, duration]
        let elapsed = now_ms - tr.start_ms;
        let time = if elapsed >= tr.duration { tr.duration } else { js_max(0.0, elapsed) % tr.duration };
        let t = time / tr.duration;
        let mut merged = self.props;
        set_values(&mut merged.view, tr.interpolate(t));
        // every prop normalised, so that viewports compare in the same range
        let in_transition = GlobeState::new(merged, GestureState::default(), None).props;
        // deck reports it through `onViewStateChange` and gets it back as the controller's props
        // (then ignored as the transition's own)
        self.props = in_transition;
        if elapsed >= tr.duration {
            self.end_transition();
        } else if let Some(tr) = &mut self.transition {
            tr.in_transition = Some(in_transition);
        }
        true
    }

    /// A new view from the app, no transition (`deck.setProps({ initialViewState: { ...view,
    /// ...limits() } })`): the follow re-aim, vehicle centring and terrain aim each frame, and the
    /// ground reset when a lock ends. It is not clamped. It breaks a running transition unless it
    /// is the transition's own current view.
    pub fn set_view(&mut self, view: ViewState) {
        let next = Props { view, ..self.props };
        self.process_view_state_change(next, None, 0.0);
    }

    // -----------------------------------------------------------------------------------------
    // Gestures: deck's Controller with NorthUpGlobeController's overrides
    // -----------------------------------------------------------------------------------------

    /// A drag starts (deck `panstart`, `_onPanStart`). Free: a plain left drag pans; the right
    /// button or a modifier (`func_key`: Meta, Alt, Ctrl or Shift held) rotates. Locked: every drag
    /// rotates (`controller.ts:28-43`). Returns whether the map took it.
    pub fn pan_start(&mut self, pos: [f64; 2], right_button: bool, func_key: bool, now_ms: f64) -> bool {
        if now_ms < self.event_start_blocked_until {
            return false;
        }
        // NorthUpGlobeController: the right button is deck's other drag gesture, which would pan away
        let right_button = !self.locked && right_button;
        if !self.is_point_in_bounds(pos) {
            return false;
        }
        let mut alternate_mode = self.is_function_key_pressed(func_key) || right_button;
        if !self.locked {
            // dragMode 'pan'
            alternate_mode = !alternate_mode;
        }
        let state = self.controller_state();
        let new_state = if alternate_mode { state.pan_start(pos) } else { state.rotate_start(pos) };
        self.pan_move = alternate_mode;
        self.update_viewport(Some(new_state), None, now_ms, |s| s.is_dragging = true);
        true
    }

    /// The drag moves (deck `panmove`).
    pub fn pan_move(&mut self, pos: [f64; 2]) -> bool {
        if !self.interaction.is_dragging {
            return false;
        }
        if self.pan_move { self.on_pan_move(pos) } else { self.on_pan_rotate(pos) }
    }

    /// The drag ends (deck `panend`; no inertia).
    pub fn pan_end(&mut self) -> bool {
        if !self.interaction.is_dragging {
            return false;
        }
        if self.pan_move { self.on_pan_move_end() } else { self.on_pan_rotate_end() }
    }

    /// GlobeController `_onPanMove`.
    fn on_pan_move(&mut self, pos: [f64; 2]) -> bool {
        let new_state = self.controller_state().pan(pos, None);
        self.update_viewport(new_state, None, 0.0, |s| {
            s.is_dragging = true;
            s.is_panning = true;
        });
        true
    }

    /// GlobeController `_onPanMoveEnd` without inertia.
    fn on_pan_move_end(&mut self) -> bool {
        let new_state = self.controller_state().pan_end();
        self.update_viewport(Some(new_state), None, 0.0, |s| {
            s.is_dragging = false;
            s.is_panning = false;
        });
        true
    }

    /// Controller `_onPanRotate`.
    fn on_pan_rotate(&mut self, pos: [f64; 2]) -> bool {
        let new_state = self.controller_state().rotate(pos);
        self.update_viewport(new_state, None, 0.0, |s| {
            s.is_dragging = true;
            s.is_rotating = true;
        });
        true
    }

    /// Controller `_onPanRotateEnd` without inertia.
    fn on_pan_rotate_end(&mut self) -> bool {
        let new_state = self.controller_state().rotate_end();
        self.update_viewport(Some(new_state), None, 0.0, |s| {
            s.is_dragging = false;
            s.is_rotating = false;
        });
        true
    }

    /// A mouse wheel's zoom at `pos` by deck's wheel `delta` ([`wheel_delta`]), no animation
    /// (deck `_onWheel`): about the pointer when free (anchor strength 1 on the flat map, easing
    /// off near the globe's limb), about the centre when locked. [`Self::wheel`] is the whole wheel
    /// event, the trackpad's scroll included.
    pub fn wheel_zoom(&mut self, pos: [f64; 2], delta: f64) -> bool {
        if !self.is_point_in_bounds(pos) {
            return false;
        }
        let scale = wheel_scale(delta);
        let zoom_position = self.zoom_position(pos);
        let new_state = self.controller_state().zoom(zoom_position, None, scale);
        self.update_viewport(new_state, None, 0.0, |s| {
            s.is_zooming = true;
            s.is_panning = true;
        });
        // no transition, so the zoom is over at once
        self.interaction.is_zooming = false;
        self.interaction.is_panning = false;
        true
    }

    /// A double-click (deck `_onDoubleClick`): zoom in one level about the pointer (the centre when
    /// locked) over 300 ms, keeping the clicked point under the pointer; with a modifier, out one
    /// level (never when locked: NorthUpGlobeController reads no modifier then). Drag and pinch
    /// starts are ignored for the next 100 ms.
    pub fn double_click(&mut self, pos: [f64; 2], func_key: bool, now_ms: f64) -> bool {
        if !self.is_point_in_bounds(pos) {
            return false;
        }
        let is_zoom_out = self.is_function_key_pressed(func_key);
        let zoom_position = self.zoom_position(pos);
        let new_state = self.controller_state().zoom(zoom_position, None, if is_zoom_out { 0.5 } else { 2.0 });
        let spec = TransitionSpec {
            duration: DECK_TRANSITION_MS,
            interpolator: Interpolator { props: PropSet::ALL, around: Some(zoom_position) },
        };
        self.update_viewport(new_state, Some(spec), now_ms, |s| {
            s.is_zooming = true;
            s.is_panning = true;
        });
        self.event_start_blocked_until = now_ms + DOUBLE_CLICK_BLOCK_MS;
        true
    }

    /// Two touches start pinching at `center` (deck `pinchstart`; trackpad pinches are the app's
    /// own, [`Self::gesture_start`]).
    pub fn pinch_start(&mut self, center: [f64; 2], now_ms: f64) -> bool {
        if now_ms < self.event_start_blocked_until || !self.is_point_in_bounds(center) {
            return false;
        }
        let zoom_position = self.zoom_position(center);
        let new_state = self.controller_state().zoom_start(zoom_position).rotate_start(center);
        self.update_viewport(Some(new_state), None, now_ms, |s| s.is_dragging = true);
        true
    }

    /// The pinch moves: `scale` is cumulative since it began (hammer's `scale`). It zooms about the
    /// fingers' centre (the screen centre when locked), the place first pinched following them; the
    /// twist does not rotate (`touchRotate: false`).
    pub fn pinch_move(&mut self, center: [f64; 2], scale: f64) -> bool {
        if !self.interaction.is_dragging {
            return false;
        }
        let zoom_position = self.zoom_position(center);
        let new_state = self.controller_state().zoom(zoom_position, None, scale);
        self.update_viewport(new_state, None, 0.0, |s| {
            s.is_dragging = true;
            s.is_panning = true;
            s.is_zooming = true;
            s.is_rotating = false;
        });
        true
    }

    /// The pinch ends (no inertia).
    pub fn pinch_end(&mut self) -> bool {
        if !self.interaction.is_dragging {
            return false;
        }
        let new_state = self.controller_state().zoom_end().rotate_end();
        self.update_viewport(Some(new_state), None, 0.0, |s| {
            s.is_dragging = false;
            s.is_panning = false;
            s.is_zooming = false;
            s.is_rotating = false;
        });
        true
    }

    /// Two touches start moving together (deck `multipanstart`, `multiTouchDrag: 'pan'`): `center`
    /// is where they are now and `delta` how far they have moved since they touched down. Free
    /// camera only.
    pub fn multi_pan_start(&mut self, center: [f64; 2], delta: [f64; 2], now_ms: f64) -> bool {
        if now_ms < self.event_start_blocked_until || self.locked || !self.is_point_in_bounds(center) {
            return false;
        }
        let start_center = [center[0] - delta[0], center[1] - delta[1]];
        let new_state = self.controller_state().pan_start(start_center);
        self.multi_pan_start_center = Some(start_center);
        self.update_viewport(Some(new_state), None, now_ms, |s| s.is_dragging = true);
        true
    }

    /// The two touches move: `delta` since they touched down.
    pub fn multi_pan_move(&mut self, delta: [f64; 2]) -> bool {
        let Some(start) = self.multi_pan_start_center else {
            return false;
        };
        if !self.interaction.is_dragging {
            return false;
        }
        self.on_pan_move([start[0] + delta[0], start[1] + delta[1]])
    }

    /// The two-finger pan ends.
    pub fn multi_pan_end(&mut self) -> bool {
        let started = self.multi_pan_start_center.take();
        if started.is_none() || !self.interaction.is_dragging {
            return false;
        }
        self.on_pan_move_end()
    }

    /// An arrow key (deck `_onKeyDown`), over deck's 300 ms transition: pan 100 px from the centre;
    /// with a modifier (`func_key`), left and right turn the map ∓15° and up and down tilt it ±10°
    /// (never when locked: no modifier is read then, so the arrows pan).
    pub fn arrow_key(&mut self, key: ArrowKey, func_key: bool, now_ms: f64) -> bool {
        let func_key = self.is_function_key_pressed(func_key);
        let state = self.controller_state();
        let new_state = match (key, func_key) {
            (ArrowKey::Left, true) => state.rotate_by(-KEY_ROTATE_SPEED_X, 0.0),
            (ArrowKey::Left, false) => state.pan_from_center([KEY_MOVE_SPEED, 0.0]),
            (ArrowKey::Right, true) => state.rotate_by(KEY_ROTATE_SPEED_X, 0.0),
            (ArrowKey::Right, false) => state.pan_from_center([-KEY_MOVE_SPEED, 0.0]),
            (ArrowKey::Up, true) => state.rotate_by(0.0, KEY_ROTATE_SPEED_Y),
            (ArrowKey::Up, false) => state.pan_from_center([0.0, KEY_MOVE_SPEED]),
            (ArrowKey::Down, true) => state.rotate_by(0.0, -KEY_ROTATE_SPEED_Y),
            (ArrowKey::Down, false) => state.pan_from_center([0.0, -KEY_MOVE_SPEED]),
        };
        let spec = TransitionSpec {
            duration: DECK_TRANSITION_MS,
            interpolator: Interpolator { props: PropSet::ALL, around: None },
        };
        self.update_viewport(Some(new_state), Some(spec), now_ms, |s| {
            if func_key {
                s.is_rotating = true;
            } else {
                s.is_panning = true;
            }
        });
        true
    }

    // -----------------------------------------------------------------------------------------
    // Globe.svelte: the wheel and the trackpad
    // -----------------------------------------------------------------------------------------

    /// A wheel event at `pos` (`Globe.svelte`'s capture listener, G:3547-3563, then deck's wheel
    /// zoom): Ctrl + wheel during a WebKit pinch is swallowed (WebKit sends both); a trackpad's
    /// two-finger scroll pans ([`Self::trackpad_pan`]); anything else zooms ([`Self::wheel_zoom`]).
    pub fn wheel(&mut self, e: &WheelInput, pos: [f64; 2], now_ms: f64) -> bool {
        if e.ctrl_key && self.pinch.is_some() {
            return true;
        }
        if !is_trackpad_scroll(e) {
            return self.wheel_zoom(pos, wheel_delta(e));
        }
        let delta = wheel_pixels(e, self.props.height);
        self.trackpad_pan(delta, now_ms);
        true
    }

    /// Two fingers moved together on a trackpad by `[dx, dy]` pixels: the map moves with them, the
    /// point under `centre + delta` becoming the centre (G:3553-3562). Nothing while locked or while
    /// the app's own move runs (`transition_until`).
    pub fn trackpad_pan(&mut self, [dx, dy]: [f64; 2], now_ms: f64) {
        if self.locked || now_ms < self.transition_until {
            return;
        }
        let centre = [self.props.width / 2.0 + dx, self.props.height / 2.0 + dy];
        if let Some(p) = map_at(&self.props.view, self.size(), centre) {
            self.move_view(ViewState { longitude: p[0], latitude: p[1], ..self.props.view });
        }
    }

    /// WebKit's `gesturestart` (a trackpad pinch on macOS): remember the zoom and bearing, and
    /// where between the fingers it started.
    pub fn gesture_start(&mut self, at: [f64; 2]) {
        let ViewState { zoom, bearing, .. } = self.props.view;
        self.pinch = Some(WebKitPinch { start: ZoomBearing { zoom, bearing }, at });
    }

    /// WebKit's `gesturechange`: `scale` and `rotation` (degrees, clockwise) since the start
    /// ([`pinched`]); zoomed and turned about where it started (the screen centre when locked).
    pub fn gesture_change(&mut self, scale: f64, rotation: f64) {
        let Some(pinch) = self.pinch else {
            return;
        };
        let ZoomBearing { zoom, bearing } = pinched(pinch.start, scale, rotation);
        let at = if self.locked { [self.props.width / 2.0, self.props.height / 2.0] } else { pinch.at };
        self.zoom_about(zoom, bearing, at);
    }

    /// WebKit's `gestureend`.
    pub fn gesture_end(&mut self) {
        self.pinch = None;
    }

    /// The camera moved by a gesture of the app's own (`moveView`, G:3524-3538): zoom clamped to the
    /// limits (unshifted, unlike deck's), latitude to ±85, bearing wrapped. No transition; a
    /// running one breaks. (Locked on an aircraft, the app then re-aims with `lockedView`.)
    pub fn move_view(&mut self, next: ViewState) {
        let lim = self.props.limits;
        let view = ViewState {
            zoom: js_max(lim.min_zoom, js_min(lim.max_zoom, next.zoom)),
            latitude: js_max(-MAX_PAN_LATITUDE, js_min(MAX_PAN_LATITUDE, next.latitude)),
            bearing: wrap_bearing(next.bearing),
            ..next
        };
        self.set_view(view);
    }

    /// Zoomed and turned, keeping the place under `at` where it is (`zoomAbout`, G:3539-3546): the
    /// centre shifted by the difference of the two map points under `at`, skipped when locked.
    pub fn zoom_about(&mut self, zoom: f64, bearing: f64, at: [f64; 2]) {
        let view = self.props.view;
        let mut next = ViewState { zoom, bearing, ..view };
        let before = if self.locked { None } else { map_at(&view, self.size(), at) };
        let after = before.and_then(|_| map_at(&next, self.size(), at));
        if let (Some(before), Some(after)) = (before, after) {
            next.longitude = next.longitude + before[0] - after[0];
            next.latitude = next.latitude + before[1] - after[1];
        }
        self.move_view(next);
    }

    // -----------------------------------------------------------------------------------------
    // Globe.svelte: programmatic moves (G:3245-3289, doc 01 §3.6)
    // -----------------------------------------------------------------------------------------

    /// `goTo(target, duration, props)`: a linear transition (no easing) of the target's fields over
    /// `duration_ms`, to `{ ...view, ...target }` within the limits, and `transition_until` set.
    /// A target equal to the view starts nothing; one during a transition takes over from where
    /// that one is.
    pub fn go_to(&mut self, target: ViewPatch, duration_ms: f64, now_ms: f64) {
        self.transition_until = now_ms + duration_ms;
        let next = Props { view: target.apply(&self.props.view), ..self.props };
        let props = PropSet(target.values().map(|v| v.is_some()));
        let spec = TransitionSpec { duration: duration_ms, interpolator: Interpolator { props, around: None } };
        self.process_view_state_change(next, Some(spec), now_ms);
    }

    /// Fly to a point over 1500 ms (`flyTo`, G:3259-3267): longitude the short way round, latitude
    /// and zoom each linear (no zoom-out-then-in). Without a zoom, a target within 2° keeps the
    /// zoom (at least 8) and a far one lands at 8; capped at [`Self::max_zoom_now`].
    pub fn fly_to(&mut self, longitude: f64, latitude: f64, zoom: Option<f64>, now_ms: f64) {
        let view = self.props.view;
        let near = cos_angle(longitude, latitude, view.longitude, view.latitude) > ((2.0 * std::f64::consts::PI) / 180.0).cos();
        let zoom = js_min(zoom.unwrap_or(if near { js_max(view.zoom, FLY_TO_ZOOM) } else { FLY_TO_ZOOM }), self.max_zoom_now);
        let target = ViewPatch {
            longitude: Some(unwrap_longitude(view.longitude, longitude)),
            latitude: Some(latitude),
            zoom: Some(zoom),
            ..ViewPatch::default()
        };
        self.go_to(target, FLY_TO_MS, now_ms);
    }

    /// Straight to a view, no flight and no clamping (`jumpTo`, G:3270-3275; the `--view` flag).
    pub fn jump_to(&mut self, target: ViewPatch) {
        let view = target.apply(&self.props.view);
        self.set_view(view);
    }

    /// Zoom by `delta` levels about the centre over 250 ms (`zoomBy`, G:3277-3280): to within
    /// `[0.5, max_zoom_now]` (20 unless locked). deck's latitude-shifted limits then hold the
    /// transition's end too (at 60°N it stops at 19).
    pub fn zoom_by(&mut self, delta: f64, now_ms: f64) {
        let zoom = js_min(self.max_zoom_now, js_max(LIMITS.min_zoom, self.props.view.zoom + delta));
        self.go_to(ViewPatch { zoom: Some(zoom), ..ViewPatch::default() }, ZOOM_BY_MS, now_ms);
    }

    /// North up and looking straight down over 400 ms (`resetOrientation`).
    pub fn reset_orientation(&mut self, now_ms: f64) {
        self.go_to(ViewPatch { bearing: Some(0.0), pitch: Some(0.0), ..ViewPatch::default() }, ORIENTATION_MS, now_ms);
    }

    /// Tilt to `pitch` over 400 ms (`setPitch`; the tilt key goes to [`TILT`] or back to 0).
    pub fn set_pitch(&mut self, pitch: f64, now_ms: f64) {
        self.go_to(ViewPatch { pitch: Some(pitch), ..ViewPatch::default() }, ORIENTATION_MS, now_ms);
    }

    /// Following a train or ship: fly there over 800 ms, at least zoom 13 (G:3160-3169). The app
    /// then centres the camera on it each frame through [`Self::set_view`] once
    /// [`Self::transition_until`] has passed.
    pub fn fly_to_vehicle(&mut self, longitude: f64, latitude: f64, now_ms: f64) {
        let target = ViewPatch {
            longitude: Some(longitude),
            latitude: Some(latitude),
            zoom: Some(js_max(self.props.view.zoom, FOLLOW_VEHICLE_MIN_ZOOM)),
            ..ViewPatch::default()
        };
        self.go_to(target, FOLLOW_VEHICLE_MS, now_ms);
    }

    // -----------------------------------------------------------------------------------------
    // deck's plumbing
    // -----------------------------------------------------------------------------------------

    /// deck's `controllerState` for an event: the current props constrained, with the gesture's
    /// memory.
    fn controller_state(&self) -> GlobeState {
        GlobeState::new(self.props, self.gesture, None)
    }

    /// `isPointInBounds`: the pixel is on the canvas.
    fn is_point_in_bounds(&self, pos: [f64; 2]) -> bool {
        pos[0] >= 0.0 && pos[0] <= self.props.width && pos[1] >= 0.0 && pos[1] <= self.props.height
    }

    /// NorthUpGlobeController `isFunctionKeyPressed`: no modifier counts while locked.
    fn is_function_key_pressed(&self, func_key: bool) -> bool {
        !self.locked && func_key
    }

    /// `getZoomPosition`: the pointer, or locked (`zoomAround: 'center'`) the view centre's pixel.
    fn zoom_position(&self, pos: [f64; 2]) -> [f64; 2] {
        if !self.locked {
            return pos;
        }
        let viewport = self.controller_state().props.viewport();
        let [x, y, _] = viewport.world_to_pixels(viewport.center);
        [x, y]
    }

    /// `updateViewport(newState, extraProps, interactionState)`: keep the gesture's memory, set the
    /// interaction state, and if the state changed hand its props on as the new view (deck's
    /// `onViewStateChange`, fed back as the controller's props), with a transition when asked.
    fn update_viewport(
        &mut self,
        new_state: Option<GlobeState>,
        transition: Option<TransitionSpec>,
        now_ms: f64,
        interaction: impl FnOnce(&mut InteractionState),
    ) {
        if let Some(new_state) = &new_state {
            self.gesture = new_state.state;
        }
        interaction(&mut self.interaction);
        if let Some(new_state) = new_state {
            self.process_view_state_change(new_state.props, transition, now_ms);
        }
    }

    /// `TransitionManager.processViewStateChange`: take the new props; start a transition to them
    /// when asked and they differ, else break a running transition unless they are its own.
    /// Returns whether a transition started.
    fn process_view_state_change(&mut self, next: Props, transition: Option<TransitionSpec>, now_ms: f64) -> bool {
        // `_isTransitionEnabled`: a positive duration
        let transition = transition.filter(|t| t.duration > 0.0);
        let current = std::mem::replace(&mut self.props, next);
        if self.should_ignore_viewport_change(&current, &next, transition.as_ref()) {
            return false;
        }
        match transition {
            Some(spec) => {
                // interrupted (deck's default, BREAK): carry on from where the old one got to
                let start = self.transition.and_then(|t| t.in_transition).unwrap_or(current);
                self.trigger_transition(start, spec, now_ms);
                true
            }
            None => {
                self.end_transition();
                false
            }
        }
    }

    /// `_shouldIgnoreViewportChange`: during a transition, props that are its own current view; out
    /// of one, a transition to props equal to the current ones, or any props without a transition
    /// (they are simply taken).
    fn should_ignore_viewport_change(&self, current: &Props, next: &Props, transition: Option<&TransitionSpec>) -> bool {
        if let Some(tr) = &self.transition {
            return tr.in_transition.is_some_and(|p| tr.interpolator.props_equal(&next.view, &p.view));
        }
        match transition {
            Some(spec) => spec.interpolator.props_equal(&current.view, &next.view),
            None => true,
        }
    }

    /// `_triggerTransition(startProps, endProps)`: the end constrained and moved the short way
    /// round from the start, the interpolator initialised, and the first frame (t = 0) at once.
    fn trigger_transition(&mut self, start: Props, spec: TransitionSpec, now_ms: f64) {
        let start_state = GlobeState::new(start, GestureState::default(), None);
        let end = GlobeState::new(self.props, GestureState::default(), None).shortest_path_from(&start_state);
        let around = spec.interpolator.around.map(|around| {
            let around_position = start.viewport().unproject(around, 0.0);
            let [x, y, _] = end.viewport().project([around_position[0], around_position[1], 0.0]);
            AroundEnds { start_around: around, end_around: [x, y], around_position, width: end.width, height: end.height }
        });
        // `transition.start` interrupts the old one first
        self.end_transition();
        self.transition = Some(Transition {
            start_ms: now_ms,
            duration: spec.duration,
            interpolator: spec.interpolator,
            start: values(&start.view),
            end: values(&end.view),
            around,
            in_transition: None,
        });
        self.interaction.in_transition = true;
        self.update(now_ms);
    }

    /// A transition ends or is interrupted (`_onTransitionEnd`).
    fn end_transition(&mut self) {
        if self.transition.take().is_some() {
            self.interaction.in_transition = false;
            self.interaction.is_zooming = false;
            self.interaction.is_panning = false;
            self.interaction.is_rotating = false;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The lock on an aircraft (Globe.svelte `lockedView`, G:3089-3129; doc 01 §3.9, doc 03 §11.4)
// ---------------------------------------------------------------------------------------------

/// Locked on the globe, the aircraft's height may reach this much of the camera's
/// (`LOCK_CAMERA_ABOVE`, G:3074).
pub const LOCK_CAMERA_ABOVE: f64 = 0.9;
/// Metres per degree, to push the locked camera's aim ahead along the bearing
/// (`METRES_PER_DEGREE`, G:3070).
pub const METRES_PER_DEGREE: f64 = 111_320.0;

/// What [`locked_view`] gives: the view aimed at the aircraft, and how far the locked camera may
/// tilt now (`lockedMaxPitch`, which `lockedView` sets as it goes and `limits()` hands to deck).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LockedView {
    pub view: ViewState,
    /// [`FOLLOW_MAX_PITCH`] on the flat map; on the globe the tilt that keeps the camera above the
    /// aircraft, at most that.
    pub max_pitch: f64,
}

/// The view `v` with its centre moved so the camera is locked on an aircraft drawn at `pose`
/// (`lockedView(v, flight)`, G:3089-3129): `pose` is the one the model is drawn at
/// (`motion.pose(flight, motion.now())`, or the camera shakes against it), raised by
/// `elevation(pose.altFt, effectiveExaggeration(exaggeration, v.zoom))` as the aircraft is drawn.
/// `height_px` is the map's height in CSS pixels (`container.clientHeight`).
///
/// - **Flat map** (zoom above 12): the camera's target is raised to the aircraft's own height
///   (`position: [0, 0, altitude]`), which centres it at any tilt, bearing and zoom.
/// - **Globe**: the camera aims at a point on the ground, so once tilted an aircraft at altitude
///   rides above the centre of the screen; the aim is pushed ahead along the bearing until the
///   aircraft is on the camera's axis. That only works while the camera stays above the
///   aircraft, so the tilt stops where it would sink below it (`acos(altitude / (0.9 · camera))`,
///   at most 85°), and `position` is unset.
pub fn locked_view(v: &ViewState, pose: &Pose, exaggeration: f64, height_px: f64) -> LockedView {
    let (lon, lat) = (pose.lon, pose.lat);
    let altitude = elevation(pose.alt_ft, effective_exaggeration(exaggeration, v.zoom));
    if v.zoom > FLAT_ABOVE_ZOOM {
        let view = ViewState { longitude: lon, latitude: lat, position: Some([0.0, 0.0, altitude]), ..*v };
        return LockedView { view, max_pitch: FOLLOW_MAX_PITCH };
    }
    let camera = CAMERA_HEIGHTS * height_px * meters_per_pixel(v.zoom, lat);
    let max_pitch = js_min(FOLLOW_MAX_PITCH, js_min(1.0, altitude / (LOCK_CAMERA_ABOVE * camera)).acos() / DEG);
    let tilt = js_min(v.pitch, max_pitch);
    let pitch = tilt * DEG;
    let height = js_min(altitude, LOCK_CAMERA_ABOVE * camera * pitch.cos());
    let lift = height * pitch.tan();
    let latitude = js_max(-MAX_PAN_LATITUDE, js_min(MAX_PAN_LATITUDE, lat + (lift * (v.bearing * DEG).cos()) / METRES_PER_DEGREE));
    let longitude = lon + (lift * (v.bearing * DEG).sin()) / (METRES_PER_DEGREE * js_max(0.05, (lat * DEG).cos()));
    LockedView { view: ViewState { longitude, latitude, pitch: tilt, position: None, ..*v }, max_pitch }
}

/// Released from a lock closer or steeper than the free camera allows (G:3213-3221): where to
/// ease back to over [`EASE_BACK_MS`] (`goTo({ zoom: min(zoom, 20), pitch: min(pitch, 60) }, 600,
/// ["zoom", "pitch"])`), or `None` when the view is within zoom 20 and pitch 60. Those are
/// [`LIMITS`]' values on every basemap, the realistic map's 75° included, as today.
pub fn released_ease_back(view: &ViewState) -> Option<ViewPatch> {
    (view.zoom > LIMITS.max_zoom || view.pitch > LIMITS.max_pitch).then(|| ViewPatch {
        zoom: Some(js_min(view.zoom, LIMITS.max_zoom)),
        pitch: Some(js_min(view.pitch, LIMITS.max_pitch)),
        ..ViewPatch::default()
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{REALISTIC_LIMITS, Viewport};
    use rustradar_core::assert_close;

    /// The canvas every test uses, CSS pixels.
    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// deck's timeline stood at this when each golden scenario started.
    const T0: f64 = 1000.0;

    fn view(longitude: f64, latitude: f64, zoom: f64, bearing: f64, pitch: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing, pitch, position: None }
    }

    fn row(v: &ViewState) -> [f64; 5] {
        [v.longitude, v.latitude, v.zoom, v.bearing, v.pitch]
    }

    /// One step of a scenario, as the golden runs fed deck's controller.
    #[derive(Debug, Clone, Copy)]
    enum Step {
        /// panstart at a pixel: left button, no modifier.
        Drag(f64, f64),
        /// panstart with the right button.
        RightDrag(f64, f64),
        Move(f64, f64),
        End,
        /// A wheel event with deck's (mjolnir's) delta.
        Wheel(f64, f64, f64),
        /// A double-click, with or without a modifier.
        DoubleClick(f64, f64, bool),
        PinchStart(f64, f64),
        Pinch(f64, f64, f64),
        PinchEnd,
        /// multipanstart at the fingers' centre, moved so far since they touched down.
        TwoFingers(f64, f64, f64, f64),
        TwoFingersMove(f64, f64),
        TwoFingersEnd,
        Key(ArrowKey, bool),
        /// The clock moves to this many milliseconds after the start, and the transition with it.
        At(f64),
        /// `goTo(target, ms)`.
        GoTo(ViewPatch, f64),
    }

    /// Runs the steps and records the view after each (the golden runs' `viewState`).
    fn scenario(initial: ViewState, locked: bool, steps: &[Step]) -> Vec<[f64; 5]> {
        let mut c = Controller::new(initial, SIZE);
        c.set_locked(locked);
        let mut now = T0;
        let mut out = vec![];
        for step in steps {
            match *step {
                Step::Drag(x, y) => _ = c.pan_start([x, y], false, false, now),
                Step::RightDrag(x, y) => _ = c.pan_start([x, y], true, false, now),
                Step::Move(x, y) => _ = c.pan_move([x, y]),
                Step::End => _ = c.pan_end(),
                Step::Wheel(x, y, delta) => _ = c.wheel_zoom([x, y], delta),
                Step::DoubleClick(x, y, func_key) => _ = c.double_click([x, y], func_key, now),
                Step::PinchStart(x, y) => _ = c.pinch_start([x, y], now),
                Step::Pinch(x, y, scale) => _ = c.pinch_move([x, y], scale),
                Step::PinchEnd => _ = c.pinch_end(),
                Step::TwoFingers(x, y, dx, dy) => _ = c.multi_pan_start([x, y], [dx, dy], now),
                Step::TwoFingersMove(dx, dy) => _ = c.multi_pan_move([dx, dy]),
                Step::TwoFingersEnd => _ = c.multi_pan_end(),
                Step::Key(key, func_key) => _ = c.arrow_key(key, func_key, now),
                Step::At(t) => {
                    now = T0 + t;
                    c.update(now);
                }
                Step::GoTo(target, ms) => c.go_to(target, ms, now),
            }
            out.push(row(&c.view()));
        }
        out
    }

    /// Each value within `tolerance` of deck's.
    fn assert_rows(name: &str, actual: &[[f64; 5]], expected: &[[f64; 5]], tolerance: f64) {
        assert_eq!(actual.len(), expected.len(), "{name}: steps");
        for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
            for k in 0..5 {
                assert!(
                    (a[k] - e[k]).abs() <= tolerance,
                    "{name}, step {i}, {}: got {}, deck {} (all: {a:?} vs {e:?})",
                    ["longitude", "latitude", "zoom", "bearing", "pitch"][k],
                    a[k],
                    e[k]
                );
            }
        }
    }

    // Golden runs. Each is deck 9.4.0's `GlobeController` with the app's `NorthUpGlobeController`
    // (`src/lib/controller.ts`) and the app's Mercator anchor patch (G:257-261), driven headless
    // under bun on a 1100 × 720 canvas with `controllerFor(locked)` and `LIMITS`: the controller's
    // `handleEvent` fed the events of the matching test, its `onViewStateChange` fed back through
    // `setProps` as deck's view manager does, the app's `goTo` as `setProps({ ...view, ...target,
    // ...LIMITS, transitionDuration, transitionInterpolator: new LinearInterpolator(keys) })`, and
    // a luma.gl `Timeline` stepped for the clock. Each row is the view after one step.

    const PAN_GLOBE: &[[f64; 5]] = &[
        [10.0, 35.0, 1.9, 0.0, 0.0],
        [3.686733182094036, 31.552316386646982, 1.9592042126827254, 0.0, 0.0],
        [-21.566334089529814, 59.133785293471135, 1.5328570036609546, 0.0, 0.0],
        [-21.566334089529814, 59.133785293471135, 1.5328570036609546, 0.0, 0.0],
    ];
    const PAN_TURNED: &[[f64; 5]] = &[
        [10.0, 50.0, 5.0, 30.0, 20.0],
        [10.409848491389228, 52.03393919696811, 4.9367262573037, 30.0, 20.0],
        [10.409848491389228, 52.03393919696811, 4.9367262573037, 30.0, 20.0],
    ];
    const PAN_FLAT: &[[f64; 5]] = &[
        [18.07, 59.33, 14.0, 0.0, 0.0],
        [18.071438106988644, 59.33097809036875, 13.999958471566135, 0.0, 0.0],
        [18.071438106988644, 59.33097809036875, 13.999958471566135, 0.0, 0.0],
    ];
    const PAN_NORTH: &[[f64; 5]] = &[
        [10.0, 40.0, 5.0, 0.0, 0.0],
        [10.0, 47.52062373383083, 4.8181534000723225, 0.0, 0.0],
        [10.0, 47.52062373383083, 4.8181534000723225, 0.0, 0.0],
    ];

    #[test]
    fn pans_as_deck_does() {
        use Step::*;
        let pan = scenario(view(10.0, 35.0, 1.9, 0.0, 0.0), false, &[Drag(550.0, 360.0), Move(580.0, 340.0), Move(700.0, 500.0), End]);
        assert_rows("pan on the globe", &pan, PAN_GLOBE, 1e-9);
        let turned = scenario(view(10.0, 50.0, 5.0, 30.0, 20.0), false, &[Drag(400.0, 300.0), Move(450.0, 420.0), End]);
        assert_rows("pan, turned and tilted", &turned, PAN_TURNED, 1e-9);
        let flat = scenario(view(18.07, 59.33, 14.0, 0.0, 0.0), false, &[Drag(550.0, 360.0), Move(520.0, 400.0), End]);
        assert_rows("pan on the flat map", &flat, PAN_FLAT, 1e-9);
        let north = scenario(view(10.0, 40.0, 5.0, 0.0, 0.0), false, &[Drag(550.0, 200.0), Move(550.0, 600.0), End]);
        assert_rows("pan north", &north, PAN_NORTH, 1e-9);
    }

    const WHEEL_GLOBE: &[[f64; 5]] = &[
        [14.705376274521116, 52.93389521088411, 3.4552431409848037, 0.0, 0.0],
        [10.309904507262225, 49.82384428504265, 3.0052696341947227, 0.0, 0.0],
    ];
    const WHEEL_FLAT: &[[f64; 5]] = &[[18.066337486572245, 59.32957276275959, 14.548077056489486, 20.0, 30.0]];
    const WHEEL_LIMB: &[[f64; 5]] = &[
        [10.0, 35.0, 2.4480589169169518, 0.0, 0.0],
        [-14.495517700339043, 32.50465376121903, 3.0381143058720523, 0.0, 0.0],
    ];
    const WHEEL_LOCKED: &[[f64; 5]] = &[[10.0, 50.0, 6.548058916916952, 40.0, 30.0]];
    const WHEEL_LIMIT_60N: &[[f64; 5]] = &[[10.0, 60.0, 19.0, 0.0, 0.0], [10.0, 60.0, 19.0, 0.0, 0.0]];

    #[test]
    fn wheel_zooms_as_deck_does() {
        use Step::*;
        let globe = scenario(view(10.0, 50.0, 3.0, 0.0, 0.0), false, &[Wheel(700.0, 200.0, 100.0), Wheel(700.0, 200.0, -100.0)]);
        assert_rows("wheel on the globe", &globe, WHEEL_GLOBE, 1e-9);
        let flat = scenario(view(18.07, 59.33, 14.0, 20.0, 30.0), false, &[Wheel(300.0, 500.0, 100.0)]);
        assert_rows("wheel on the flat map", &flat, WHEEL_FLAT, 1e-9);
        // off the globe there is no anchor; part way to the limb a weakened one
        let limb = scenario(view(10.0, 35.0, 1.9, 0.0, 0.0), false, &[Wheel(60.0, 360.0, 100.0), Wheel(200.0, 360.0, 100.0)]);
        assert_rows("wheel near the limb", &limb, WHEEL_LIMB, 1e-9);
        let locked = scenario(view(10.0, 50.0, 6.0, 40.0, 30.0), true, &[Wheel(700.0, 200.0, 100.0)]);
        assert_rows("wheel, locked", &locked, WHEEL_LOCKED, 1e-9);
        let limit = scenario(view(10.0, 60.0, 18.5, 0.0, 0.0), false, &[Wheel(550.0, 360.0, 400.0), Wheel(550.0, 360.0, 400.0)]);
        assert_rows("wheel against the limit at 60°N", &limit, WHEEL_LIMIT_60N, 1e-9);
    }

    const ROTATE: &[[f64; 5]] = &[
        [10.0, 35.0, 3.0, 0.0, 0.0],
        [10.0, 35.0, 3.0, 45.0, 43.333333333333336],
        [10.0, 35.0, 3.0, -40.90909090909088, 0.0],
        [10.0, 35.0, 3.0, -40.90909090909088, 0.0],
    ];
    const ROTATE_LOCKED: &[[f64; 5]] = &[
        [10.0, 35.0, 3.0, 0.0, 0.0],
        [10.0, 35.0, 3.0, 8.18181818181813, 9.999999999999998],
        [10.0, 35.0, 3.0, 8.18181818181813, 9.999999999999998],
    ];

    #[test]
    fn drags_rotate_as_deck_does() {
        use Step::*;
        let free = scenario(view(10.0, 35.0, 3.0, 0.0, 0.0), false, &[RightDrag(550.0, 360.0), Move(825.0, 100.0), Move(300.0, 600.0), End]);
        assert_rows("right-drag", &free, ROTATE, 1e-9);
        // locked, a plain left drag orbits
        let locked = scenario(view(10.0, 35.0, 3.0, 0.0, 0.0), true, &[Drag(550.0, 360.0), Move(600.0, 300.0), End]);
        assert_rows("locked drag", &locked, ROTATE_LOCKED, 1e-9);
    }

    const DBLCLICK: &[[f64; 5]] = &[
        [10.0, 50.0, 4.0, 0.0, 0.0],
        [12.030421591678365, 50.896638186563116, 4.476466419597537, 0.0, 0.0],
        [13.455863728631622, 51.52629595709493, 4.952932839195075, 0.0, 0.0],
        [13.455863728631622, 51.52629595709493, 4.952932839195075, 0.0, 0.0],
    ];
    const DBLCLICK_OUT: &[[f64; 5]] = &[
        [10.0, 50.0, 4.0, 10.0, 20.0],
        [17.992303111003245, 52.82310841864113, 2.910869847060969, 10.0, 20.0],
    ];
    const DBLCLICK_FLAT: &[[f64; 5]] = &[
        [18.069999999999997, 59.33000000000003, 13.5, 0.0, 0.0],
        [18.07187785111525, 59.33070256571066, 13.833309234154086, 0.0, 0.0],
        [18.074551403442115, 59.33170273538003, 14.499927702462251, 0.0, 0.0],
    ];

    #[test]
    fn double_clicks_as_deck_does() {
        use Step::*;
        let globe = scenario(view(10.0, 50.0, 4.0, 0.0, 0.0), false, &[DoubleClick(700.0, 250.0, false), At(150.0), At(300.0), At(400.0)]);
        assert_rows("double-click", &globe, DBLCLICK, 1e-9);
        let out = scenario(view(10.0, 50.0, 4.0, 10.0, 20.0), false, &[DoubleClick(400.0, 500.0, true), At(300.0)]);
        assert_rows("double-click with Shift", &out, DBLCLICK_OUT, 1e-9);
        let flat = scenario(view(18.07, 59.33, 13.5, 0.0, 0.0), false, &[DoubleClick(700.0, 250.0, false), At(100.0), At(300.0)]);
        assert_rows("double-click on the flat map", &flat, DBLCLICK_FLAT, 1e-9);
    }

    const PINCH: &[[f64; 5]] = &[
        [10.0, 50.0, 4.0, 0.0, 0.0],
        [9.251018628379597, 50.93963529529852, 4.556290056663766, 0.0, 0.0],
        [9.251018628379597, 50.93963529529852, 4.556290056663766, 0.0, 0.0],
    ];
    const MULTIPAN: &[[f64; 5]] = &[
        [10.0, 50.0, 4.0, 0.0, 0.0],
        [7.447455968958293, 50.94658289019208, 3.972009800242797, 0.0, 0.0],
        [7.447455968958293, 50.94658289019208, 3.972009800242797, 0.0, 0.0],
    ];

    #[test]
    fn touch_gestures_as_deck_does() {
        use Step::*;
        let pinch = scenario(view(10.0, 50.0, 4.0, 0.0, 0.0), false, &[PinchStart(600.0, 300.0), Pinch(650.0, 320.0, 1.5), PinchEnd]);
        assert_rows("pinch", &pinch, PINCH, 1e-9);
        let two = scenario(view(10.0, 50.0, 4.0, 0.0, 0.0), false, &[TwoFingers(600.0, 300.0, 12.0, 0.0), TwoFingersMove(52.0, 30.0), TwoFingersEnd]);
        assert_rows("two-finger pan", &two, MULTIPAN, 1e-9);
    }

    const KEYS: &[[f64; 5]] = &[
        [10.0, 50.0, 4.0, 0.0, 0.0],
        [5.091261478765947, 50.0, 4.003097242711851, 0.0, 0.0],
        [5.091261478765947, 50.0, 4.003097242711851, 0.0, 0.0],
        [5.091261478765947, 50.0, 4.003097242711851, 0.0, 5.0],
        [5.091261478765947, 50.0, 4.003097242711851, 0.0, 10.0],
        [5.091261478765947, 50.0, 4.003097242711851, 0.0, 10.0],
        [5.091261478765947, 50.0, 4.003097242711851, 15.0, 10.0],
    ];

    #[test]
    fn arrow_keys_as_deck_does() {
        use Step::*;
        let keys = scenario(
            view(10.0, 50.0, 4.0, 0.0, 0.0),
            false,
            &[Key(ArrowKey::Left, false), At(300.0), Key(ArrowKey::Up, true), At(450.0), At(600.0), Key(ArrowKey::Right, true), At(900.0)],
        );
        assert_rows("arrow keys", &keys, KEYS, 1e-9);
    }

    const FLY: &[[f64; 5]] = &[
        [10.0, 35.0, 1.9, 0.0, 0.0],
        [14.035, 47.165, 6.45, 0.0, 0.0],
        [18.07, 59.33, 11.0, 0.0, 0.0],
        [18.07, 59.33, 11.0, 0.0, 0.0],
    ];
    const FLY_ANTIMERIDIAN: &[[f64; 5]] = &[
        [170.0, 10.0, 4.0, 20.0, 10.0],
        [175.0, 6.25, 4.5, 20.0, 10.0],
        [-175.0, -1.25, 5.5, 20.0, 10.0],
        [-170.0, -5.0, 6.0, 20.0, 10.0],
    ];
    const ZOOM_BY_60N: &[[f64; 5]] = &[[10.0, 60.0, 18.7, 0.0, 0.0], [10.0, 60.0, 18.85, 0.0, 0.0], [10.0, 60.0, 19.0, 0.0, 0.0]];
    const RESET: &[[f64; 5]] = &[[10.0, 50.0, 4.0, -170.0, 50.0], [10.0, 50.0, 4.0, -127.5, 37.5], [10.0, 50.0, 4.0, 0.0, 0.0]];
    const INTERRUPT: &[[f64; 5]] = &[
        [10.0, 35.0, 1.9, 0.0, 0.0],
        [12.690000000000001, 43.11, 4.933333333333334, 0.0, 0.0],
        [12.690000000000001, 43.11, 4.933333333333334, 0.0, 0.0],
        [13.228000000000002, 44.732, 5.54, 0.0, 0.0],
        [12.432955349812419, 43.48530730114401, 4.924435589407062, 0.0, 0.0],
        [12.432955349812419, 43.48530730114401, 4.924435589407062, 0.0, 0.0],
        [12.432955349812419, 43.48530730114401, 4.924435589407062, 0.0, 0.0],
    ];
    const RETARGET: &[[f64; 5]] = &[
        [10.0, 35.0, 1.9, 0.0, 0.0],
        [12.690000000000001, 43.11, 4.933333333333334, 0.0, 0.0],
        [12.690000000000001, 43.11, 4.933333333333334, 0.0, 0.0],
        [12.690000000000001, 43.11, 4.966666666666667, 0.0, 0.0],
        [12.690000000000001, 43.11, 5.0, 0.0, 0.0],
    ];

    fn to(longitude: f64, latitude: f64, zoom: f64) -> ViewPatch {
        ViewPatch { longitude: Some(longitude), latitude: Some(latitude), zoom: Some(zoom), ..ViewPatch::default() }
    }

    #[test]
    fn transitions_as_deck_does() {
        use Step::*;
        let start = view(10.0, 35.0, 1.9, 0.0, 0.0);
        let fly = scenario(start, false, &[GoTo(to(18.07, 59.33, 11.0), 1500.0), At(750.0), At(1500.0), At(1600.0)]);
        assert_rows("flight", &fly, FLY, 1e-9);
        // `flyTo` unwraps the longitude (170 → 190); deck wraps it back on the way
        let across = scenario(view(170.0, 10.0, 4.0, 20.0, 10.0), false, &[GoTo(to(190.0, -5.0, 6.0), 1500.0), At(375.0), At(1125.0), At(1500.0)]);
        assert_rows("over the antimeridian", &across, FLY_ANTIMERIDIAN, 1e-9);
        // the end held to the latitude-shifted limit
        let zoom = ViewPatch { zoom: Some(19.7), ..ViewPatch::default() };
        let zoom_by = scenario(view(10.0, 60.0, 18.7, 0.0, 0.0), false, &[GoTo(zoom, 250.0), At(125.0), At(250.0)]);
        assert_rows("zoom in at 60°N", &zoom_by, ZOOM_BY_60N, 1e-9);
        let level = ViewPatch { bearing: Some(0.0), pitch: Some(0.0), ..ViewPatch::default() };
        let reset = scenario(view(10.0, 50.0, 4.0, -170.0, 50.0), false, &[GoTo(level, 400.0), At(100.0), At(400.0)]);
        assert_rows("north up", &reset, RESET, 1e-9);
        // a drag's start leaves the flight going; its first move breaks it
        let interrupt =
            scenario(start, false, &[GoTo(to(18.07, 59.33, 11.0), 1500.0), At(500.0), Drag(550.0, 360.0), At(600.0), Move(560.0, 380.0), At(700.0), End]);
        assert_rows("a drag breaks a flight", &interrupt, INTERRUPT, 1e-9);
        // a new move carries on from where the flight got to
        let zoom = ViewPatch { zoom: Some(5.0), ..ViewPatch::default() };
        let retarget = scenario(start, false, &[GoTo(to(18.07, 59.33, 11.0), 1500.0), At(500.0), GoTo(zoom, 250.0), At(625.0), At(750.0)]);
        assert_rows("a move during a flight", &retarget, RETARGET, 1e-9);
    }

    // -----------------------------------------------------------------------------------------
    // trackpad.test.ts
    // -----------------------------------------------------------------------------------------

    /// The TypeScript test's `wheel(over)`: `over` is written over `WheelInput::default()` (no
    /// movement, pixel mode, no Ctrl) with struct update syntax at each call.
    fn wheel(over: WheelInput) -> WheelInput {
        over
    }

    #[test]
    fn a_two_finger_scroll_is_told_from_a_mouse_wheels_notch() {
        let w = WheelInput::default();
        assert!(is_trackpad_scroll(&wheel(WheelInput { delta_x: 3.0, delta_y: 1.0, ..w })));
        // WebKit: a trackpad's wheelDeltaY is -3× its pixels; a wheel's notch is 120
        assert!(is_trackpad_scroll(&wheel(WheelInput { delta_y: 4.0, wheel_delta_y: Some(-12.0), ..w })));
        assert!(!is_trackpad_scroll(&wheel(WheelInput { delta_y: 4.0, wheel_delta_y: Some(-120.0), ..w })));
        // elsewhere: fractional pixels from a trackpad, whole lines from a wheel
        assert!(is_trackpad_scroll(&wheel(WheelInput { delta_y: 2.5, ..w })));
        assert!(!is_trackpad_scroll(&wheel(WheelInput { delta_y: 3.0, delta_mode: DeltaMode::Line, ..w })));
        // a pinch (ctrl + wheel) is a zoom, not a scroll
        assert!(!is_trackpad_scroll(&wheel(WheelInput { delta_y: 2.5, ctrl_key: true, ..w })));
    }

    #[test]
    fn wheel_movement_in_pixels() {
        let w = WheelInput::default();
        assert_eq!(wheel_pixels(&wheel(WheelInput { delta_x: 2.0, delta_y: 3.0, ..w }), 800.0), [2.0, 3.0]);
        assert_eq!(wheel_pixels(&wheel(WheelInput { delta_y: 3.0, delta_mode: DeltaMode::Line, ..w }), 800.0), [0.0, 48.0]);
        assert_eq!(wheel_pixels(&wheel(WheelInput { delta_y: 1.0, delta_mode: DeltaMode::Page, ..w }), 800.0), [0.0, 800.0]);
    }

    #[test]
    fn pinching_apart_zooms_in_twisting_turns_the_map() {
        let start = ZoomBearing { zoom: 10.0, bearing: 20.0 };
        assert_close!(pinched(start, 2.0, 0.0).zoom, 11.0);
        assert_close!(pinched(start, 0.5, 0.0).zoom, 9.0);
        assert_close!(pinched(start, 1.0, 30.0).bearing, -10.0);
        assert_eq!(wrap_bearing(190.0), -170.0);
    }

    // -----------------------------------------------------------------------------------------
    // The plan's acceptance (E2) and the rest
    // -----------------------------------------------------------------------------------------

    /// deck's pan turns the globe 0.25° per pixel at scale 1 (`panStart`'s `angularRate`), where a
    /// pixel at the view centre spans `180 / (π·256)` = 0.2238°: the map runs this much ahead of
    /// the pointer.
    const DECK_PAN_LEAD: f64 = 0.25 * 256.0 * std::f64::consts::PI / 180.0;

    /// Where the map point first under `from` is on screen after a drag from `from` to `to`.
    fn dragged(c: &mut Controller, from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
        let grabbed = map_at(&c.view(), c.size(), from).expect("on the map");
        assert!(c.pan_start(from, false, false, 0.0));
        assert!(c.pan_move(to));
        assert!(c.pan_end());
        let [x, y, _] = c.viewport().project([grabbed[0], grabbed[1], 0.0]);
        [x, y]
    }

    /// The point grabbed follows the pointer: the same way, north staying up, at deck's own rate
    /// (1.117 times the pointer's travel at the view centre; doc 01 §3.3 "tracks the pointer
    /// approximately"). Exact parity with deck is pinned by [`pans_as_deck_does`].
    #[test]
    fn a_pan_keeps_the_grabbed_point_under_the_pointer() {
        let centre = [SIZE[0] / 2.0, SIZE[1] / 2.0];
        for v in [view(10.0, 0.0, 2.0, 0.0, 0.0), view(10.0, 50.0, 6.0, 0.0, 0.0), view(18.07, 59.33, 15.0, 0.0, 0.0), view(-70.0, -30.0, 9.0, 30.0, 0.0)] {
            for d in [[6.0, -4.0], [-5.0, 0.0], [0.0, 7.0]] {
                let mut c = Controller::new(v, SIZE);
                let at = dragged(&mut c, centre, [centre[0] + d[0], centre[1] + d[1]]);
                let moved = [at[0] - centre[0], at[1] - centre[1]];
                for k in 0..2 {
                    assert!((moved[k] - DECK_PAN_LEAD * d[k]).abs() < 0.02, "{v:?} dragged {d:?}: the grabbed point moved {moved:?}");
                }
                assert_eq!(c.view().bearing, v.bearing, "a pan keeps the bearing");
            }
        }
        // a long drag still carries the point most of the way with the pointer
        let mut c = Controller::new(view(10.0, 50.0, 6.0, 0.0, 0.0), SIZE);
        let at = dragged(&mut c, [300.0, 500.0], [700.0, 300.0]);
        assert!((at[0] - 700.0).abs() < 0.15 * 400.0 && (at[1] - 300.0).abs() < 0.15 * 200.0, "{at:?}");
    }

    /// The globe is drawn `1/cos lat` larger, so deck adds `log2(cos new / cos old)` to the zoom as
    /// a pan moves the latitude: the map keeps its size on screen (doc 01 §3.3).
    #[test]
    fn panning_north_changes_zoom_by_log2_of_cos_new_over_cos_old() {
        for (v, dy) in [(view(10.0, 40.0, 5.0, 0.0, 0.0), 400.0), (view(10.0, 40.0, 5.0, 0.0, 0.0), -300.0), (view(18.0, 59.3, 14.0, 0.0, 0.0), 200.0)] {
            let mut c = Controller::new(v, SIZE);
            c.pan_start([550.0, 300.0], false, false, 0.0);
            c.pan_move([550.0, 300.0 + dy]);
            let after = c.view();
            assert_eq!(after.longitude, v.longitude);
            assert!((after.latitude > v.latitude) == (dy > 0.0), "dragging down moves north");
            let expected = ((after.latitude * DEG).cos() / (v.latitude * DEG).cos()).log2();
            assert!((after.zoom - v.zoom - expected).abs() < 1e-12, "{v:?} → {after:?}: zoom changed by {}, not {expected}", after.zoom - v.zoom);
        }
        // the latitude stops at 85 (the app's limit, not deck's 90)
        let mut c = Controller::new(view(10.0, 80.0, 3.0, 0.0, 0.0), SIZE);
        c.pan_start([550.0, 100.0], false, false, 0.0);
        c.pan_move([550.0, 700.0]);
        assert_eq!(c.view().latitude, MAX_PAN_LATITUDE);
    }

    /// Runs the controller's transition to its end and returns the view.
    fn settle(c: &mut Controller, now: f64) -> ViewState {
        c.update(now + 10_000.0);
        assert!(!c.in_transition());
        c.view()
    }

    #[test]
    fn zoom_by_clamps_to_half_and_twenty() {
        let now = 5_000.0;
        let mut c = Controller::new(view(10.0, 0.0, 19.6, 0.0, 0.0), SIZE);
        c.zoom_by(1.0, now);
        assert!(c.in_transition());
        assert_eq!(c.transition_until(), now + ZOOM_BY_MS);
        c.update(now + ZOOM_BY_MS / 2.0);
        assert_close!(c.view().zoom, 19.8, 12);
        c.update(now + ZOOM_BY_MS);
        assert!(!c.in_transition(), "done after 250 ms");
        assert_eq!(c.view().zoom, 20.0);
        c.zoom_by(5.0, now + 1000.0);
        assert!(!c.in_transition(), "already there: nothing to animate");
        assert_eq!(settle(&mut c, now).zoom, 20.0);

        let mut c = Controller::new(view(10.0, 0.0, 1.0, 0.0, 0.0), SIZE);
        c.zoom_by(-5.0, now);
        assert_eq!(settle(&mut c, now).zoom, 0.5);

        // locked on an aircraft the limit is its own
        let mut c = Controller::new(view(10.0, 0.0, 20.0, 0.0, 0.0), SIZE);
        c.set_limits(Limits { max_zoom: 22.3, ..LIMITS }, 22.3);
        c.zoom_by(5.0, now);
        assert_close!(settle(&mut c, now).zoom, 22.3, 12);
    }

    /// deck clamps the zoom to `[min + log2(cos lat), max + log2(cos lat)]` (doc 01 §2.4); the
    /// app's own `moveView` clamps to the plain limits.
    #[test]
    fn the_zoom_limits_shift_with_latitude() {
        let mut c = Controller::new(view(10.0, 60.0, 1.0, 0.0, 0.0), SIZE);
        for _ in 0..10 {
            c.wheel_zoom([550.0, 360.0], -400.0);
        }
        assert_close!(c.view().zoom, -0.5, 12);
        let mut c = Controller::new(view(10.0, 60.0, 18.0, 0.0, 0.0), SIZE);
        for _ in 0..10 {
            c.wheel_zoom([550.0, 360.0], 400.0);
        }
        assert_close!(c.view().zoom, 19.0, 12);
        c.move_view(view(10.0, 60.0, 25.0, 0.0, 0.0));
        assert_eq!(c.view().zoom, 20.0);
        c.move_view(view(10.0, 60.0, -3.0, 0.0, 0.0));
        assert_eq!(c.view().zoom, 0.5);
        // and at the equator the two agree
        let mut c = Controller::new(view(10.0, 0.0, 18.0, 0.0, 0.0), SIZE);
        for _ in 0..10 {
            c.wheel_zoom([550.0, 360.0], 400.0);
        }
        assert_close!(c.view().zoom, 20.0, 12);
    }

    #[test]
    fn fly_to_lands_on_its_target() {
        let now = 2_000.0;
        // far: a regional view at zoom 8, over 1500 ms, linear
        let mut c = Controller::new(ViewState::INITIAL, SIZE);
        c.fly_to(18.07, 59.33, None, now);
        assert_eq!(c.transition_until(), now + FLY_TO_MS);
        c.update(now + FLY_TO_MS / 2.0);
        let half = c.view();
        assert_close!(half.longitude, (10.0 + 18.07) / 2.0, 12);
        assert_close!(half.latitude, (35.0 + 59.33) / 2.0, 12);
        assert_close!(half.zoom, (1.9 + 8.0) / 2.0, 12);
        c.update(now + FLY_TO_MS - 1.0);
        assert!(c.in_transition());
        c.update(now + FLY_TO_MS);
        assert!(!c.in_transition());
        assert_eq!(row(&c.view()), [18.07, 59.33, 8.0, 0.0, 0.0]);

        // near (within 2°): the zoom is kept if closer than 8
        let mut c = Controller::new(view(18.0, 59.0, 10.5, 0.0, 0.0), SIZE);
        c.fly_to(18.07, 59.33, None, now);
        assert_eq!(settle(&mut c, now).zoom, 10.5);
        let mut c = Controller::new(view(18.0, 59.0, 5.0, 0.0, 0.0), SIZE);
        c.fly_to(18.07, 59.33, None, now);
        assert_eq!(settle(&mut c, now).zoom, 8.0);
        // an asked-for zoom, capped by the closest zoom reachable now (20), then by deck's
        // latitude-shifted limit at the end: 20 + log2(cos 59.33°)
        let mut c = Controller::new(view(18.0, 59.0, 5.0, 0.0, 0.0), SIZE);
        c.fly_to(18.07, 59.33, Some(25.0), now);
        assert_close!(settle(&mut c, now).zoom, 20.0 + (59.33 * DEG).cos().log2(), 12);
        let mut c = Controller::new(view(18.0, 0.0, 5.0, 0.0, 0.0), SIZE);
        c.fly_to(18.07, 0.0, Some(25.0), now);
        assert_eq!(settle(&mut c, now).zoom, 20.0);

        // the short way round over the antimeridian, ending wrapped
        let mut c = Controller::new(view(170.0, 10.0, 4.0, 0.0, 0.0), SIZE);
        c.fly_to(-170.0, 10.0, Some(4.0), now);
        c.update(now + FLY_TO_MS / 4.0);
        assert_close!(c.view().longitude, 175.0, 12);
        c.update(now + FLY_TO_MS * 0.75);
        assert_close!(c.view().longitude, -175.0, 12);
        assert_close!(settle(&mut c, now).longitude, -170.0, 12);
    }

    #[test]
    fn reset_orientation_and_set_pitch_end_level_and_tilted() {
        let now = 0.0;
        let mut c = Controller::new(view(10.0, 50.0, 6.0, 40.0, 50.0), SIZE);
        c.reset_orientation(now);
        c.update(now + ORIENTATION_MS / 2.0);
        assert_eq!([c.view().bearing, c.view().pitch], [20.0, 25.0]);
        c.update(now + ORIENTATION_MS);
        assert_eq!(row(&c.view()), [10.0, 50.0, 6.0, 0.0, 0.0]);

        c.set_pitch(TILT, now + 1000.0);
        assert_eq!(c.transition_until(), now + 1000.0 + ORIENTATION_MS);
        c.update(now + 1000.0 + ORIENTATION_MS);
        assert_eq!(c.view().pitch, TILT);
        // the end is held to the limits: 60° here, 75° on the realistic map
        c.set_pitch(70.0, now + 2000.0);
        assert_eq!(settle(&mut c, now).pitch, 60.0);
        c.set_limits(REALISTIC_LIMITS, 20.0);
        c.set_pitch(70.0, now + 3000.0);
        assert_eq!(settle(&mut c, now).pitch, 70.0);
    }

    #[test]
    fn a_move_to_where_the_camera_is_starts_no_transition() {
        let mut c = Controller::new(view(10.0, 50.0, 6.0, 0.0, 0.0), SIZE);
        c.set_pitch(0.0, 100.0);
        assert!(!c.in_transition());
        // `transitionUntil` is set all the same, as today
        assert_eq!(c.transition_until(), 100.0 + ORIENTATION_MS);
        assert!(!c.update(200.0));
    }

    #[test]
    fn set_view_breaks_a_transition_unless_it_is_its_own_view() {
        let mut c = Controller::new(ViewState::INITIAL, SIZE);
        c.fly_to(18.07, 59.33, Some(9.0), 0.0);
        c.update(500.0);
        // the transition's own view (as deck hears it back) leaves it running
        c.set_view(c.view());
        assert!(c.in_transition());
        // anything else breaks it, and is taken as it is
        let elsewhere = view(-3.0, 40.0, 30.0, 0.0, 0.0);
        c.set_view(elsewhere);
        assert!(!c.in_transition());
        assert_eq!(c.view(), elsewhere, "not clamped");
        assert!(!c.update(1500.0));
        // `jump_to` is the same, field by field
        c.jump_to(ViewPatch { zoom: Some(4.0), ..ViewPatch::default() });
        assert_eq!(c.view(), view(-3.0, 40.0, 4.0, 0.0, 0.0));
        // a wheel zoom during a flight breaks it too
        c.fly_to(18.07, 59.33, Some(9.0), 2000.0);
        c.update(2500.0);
        c.wheel_zoom([550.0, 360.0], 100.0);
        assert!(!c.in_transition());
    }

    /// The point under the pointer stays there: exactly on the flat map (anchor strength 1),
    /// within a pixel on the globe at the equator; away from it deck's latitude coupling moves it
    /// (pinned by [`wheel_zooms_as_deck_does`]).
    #[test]
    fn a_wheel_zoom_keeps_the_point_under_the_pointer() {
        for (v, p, tolerance) in [
            (view(18.07, 59.33, 15.0, 0.0, 0.0), [900.0, 100.0], 0.05),
            (view(18.07, 59.33, 14.0, 20.0, 30.0), [300.0, 500.0], 0.05),
            (view(10.0, 0.0, 4.0, 0.0, 0.0), [800.0, 500.0], 1.0),
        ] {
            for delta in [100.0, -100.0] {
                let mut c = Controller::new(v, SIZE);
                let grabbed = map_at(&v, SIZE, p).unwrap();
                assert!(c.wheel_zoom(p, delta));
                assert!((c.view().zoom - v.zoom).abs() > 0.4, "it zoomed");
                let [x, y, _] = c.viewport().project([grabbed[0], grabbed[1], 0.0]);
                assert!((x - p[0]).hypot(y - p[1]) < tolerance, "{v:?}, delta {delta}: the point moved to {:?} from {p:?}", [x, y]);
                assert_eq!(c.view().bearing, v.bearing, "zooming never turns the map");
            }
        }
        // off the canvas the map does not take it
        let mut c = Controller::new(ViewState::INITIAL, SIZE);
        assert!(!c.wheel_zoom([-1.0, 10.0], 100.0));
        assert_eq!(c.view(), ViewState::INITIAL);
    }

    #[test]
    fn locked_the_camera_orbits_and_zooms_about_the_centre() {
        let v = view(10.0, 50.0, 8.0, 10.0, 20.0);
        let mut c = Controller::new(v, SIZE);
        c.set_locked(true);
        // a plain drag orbits: up to the top tilts to the limit, sideways turns
        assert!(c.pan_start([550.0, 360.0], false, false, 0.0));
        c.pan_move([825.0, 0.0]);
        assert_eq!([c.view().longitude, c.view().latitude], [10.0, 50.0]);
        assert_eq!(c.view().bearing, 10.0 + 180.0 * 275.0 / 1100.0);
        assert_eq!(c.view().pitch, 60.0);
        c.pan_end();
        // the right button or a modifier changes nothing
        assert!(c.pan_start([550.0, 360.0], true, true, 0.0));
        c.pan_move([550.0, 360.0]);
        assert!(c.interaction().is_rotating);
        c.pan_end();
        // the wheel zooms about the centre
        c.wheel_zoom([50.0, 50.0], 100.0);
        assert_close!(c.view().longitude, 10.0, 12);
        assert_close!(c.view().latitude, 50.0, 12);
        // double-click zooms in, modifier or not
        let zoom = c.view().zoom;
        c.double_click([50.0, 50.0], true, 1000.0);
        c.update(2000.0);
        assert_close!(c.view().zoom, zoom + 1.0, 12);
        // no two-finger pan, no trackpad pan, no shift in a WebKit pinch
        assert!(!c.multi_pan_start([550.0, 360.0], [12.0, 0.0], 5000.0));
        let before = c.view();
        c.trackpad_pan([40.0, 30.0], 5000.0);
        assert_eq!(c.view(), before);
        c.gesture_start([100.0, 100.0]);
        c.gesture_change(2.0, 0.0);
        assert_eq!([c.view().longitude, c.view().latitude], [before.longitude, before.latitude]);
        assert_close!(c.view().zoom, before.zoom + 1.0, 12);
    }

    #[test]
    fn a_right_or_modified_drag_rotates_a_free_camera() {
        for (right, func) in [(true, false), (false, true)] {
            let mut c = Controller::new(view(10.0, 35.0, 3.0, 0.0, 0.0), SIZE);
            assert!(c.pan_start([550.0, 360.0], right, func, 0.0));
            c.pan_move([1100.0, 360.0]);
            assert_eq!(row(&c.view()), [10.0, 35.0, 3.0, wrap_bearing(90.0), 0.0], "half the width turns 90°");
            assert!(c.is_dragging());
            c.pan_end();
            assert!(!c.is_dragging());
        }
        // a drag past a full turn stays wrapped
        let mut c = Controller::new(view(10.0, 35.0, 3.0, 170.0, 0.0), SIZE);
        c.pan_start([0.0, 360.0], true, false, 0.0);
        c.pan_move([1100.0, 360.0]);
        assert_eq!(c.view().bearing, wrap_bearing(350.0));
        // dragging down eases the pitch back toward 0
        let mut c = Controller::new(view(10.0, 35.0, 3.0, 0.0, 40.0), SIZE);
        c.pan_start([550.0, 360.0], true, false, 0.0);
        c.pan_move([550.0, 360.0 + 180.0]);
        assert_close!(c.view().pitch, 40.0 - 0.6 * 40.0, 12);
    }

    #[test]
    fn move_view_clamps_latitude_and_zoom_and_wraps_the_bearing() {
        let mut c = Controller::new(ViewState::INITIAL, SIZE);
        c.move_view(view(200.0, 89.0, 25.0, 190.0, 30.0));
        // longitude is left to deck's own wrapping at the next gesture
        assert_eq!(row(&c.view()), [200.0, 85.0, 20.0, -170.0, 30.0]);
        c.move_view(view(0.0, -89.0, 0.0, -540.0, 0.0));
        assert_eq!(row(&c.view()), [0.0, -85.0, 0.5, -180.0, 0.0]);
    }

    #[test]
    fn a_trackpad_scroll_pans_and_a_mouse_wheel_zooms() {
        let v = view(10.0, 50.0, 6.0, 0.0, 0.0);
        let mut c = Controller::new(v, SIZE);
        // fractional pixels: the point under centre + delta becomes the centre
        let scroll = WheelInput { delta_x: 40.5, delta_y: -20.25, ..WheelInput::default() };
        let target = map_at(&v, SIZE, [550.0 + 40.5, 360.0 - 20.25]).unwrap();
        assert!(c.wheel(&scroll, [100.0, 100.0], 0.0));
        assert_eq!([c.view().longitude, c.view().latitude], target);
        assert_eq!(c.view().zoom, 6.0);
        // whole lines: a mouse wheel, deck zooms about the pointer
        let notch = WheelInput { delta_y: 3.0, delta_mode: DeltaMode::Line, ..WheelInput::default() };
        assert!(c.wheel(&notch, [550.0, 360.0], 0.0));
        assert_close!(c.view().zoom, 6.0 + wheel_scale(-120.0).log2(), 12);
        // Ctrl + wheel is a pinch: deck zooms
        let pinch = WheelInput { delta_y: 2.5, ctrl_key: true, ..WheelInput::default() };
        let zoom = c.view().zoom;
        c.wheel(&pinch, [550.0, 360.0], 0.0);
        assert!(c.view().zoom < zoom);
        // ...unless WebKit's own pinch events are coming: then it is swallowed
        c.gesture_start([550.0, 360.0]);
        let before = c.view();
        assert!(c.wheel(&pinch, [550.0, 360.0], 0.0));
        assert_eq!(c.view(), before);
        c.gesture_end();
        // during the app's own moves the trackpad does not pan
        c.zoom_by(1.0, 1000.0);
        let before = c.view();
        c.wheel(&scroll, [100.0, 100.0], 1100.0);
        assert_eq!(c.view(), before);
        assert!(c.in_transition());
    }

    #[test]
    fn a_webkit_pinch_zooms_about_the_fingers_and_twisting_turns_the_map() {
        let v = view(10.0, 0.0, 5.0, 20.0, 0.0);
        let at = [700.0, 300.0];
        let mut c = Controller::new(v, SIZE);
        let under = map_at(&v, SIZE, at).unwrap();
        c.gesture_start(at);
        c.gesture_change(2.0, 0.0);
        assert_close!(c.view().zoom, 6.0, 12);
        // `zoomAbout` shifts the centre by the difference of the two points: close, not exact
        let [x, y, _] = c.viewport().project([under[0], under[1], 0.0]);
        assert!((x - at[0]).hypot(y - at[1]) < 3.0, "{:?}", [x, y]);
        // turned clockwise, the map turns with the fingers
        c.gesture_change(2.0, 30.0);
        assert_close!(c.view().bearing, -10.0, 12);
        // after the end nothing
        c.gesture_end();
        c.gesture_change(4.0, 0.0);
        assert_close!(c.view().zoom, 6.0, 12);
    }

    #[test]
    fn drags_and_pinches_wait_out_a_double_click() {
        let mut c = Controller::new(view(10.0, 50.0, 4.0, 0.0, 0.0), SIZE);
        assert!(c.double_click([600.0, 300.0], false, 1000.0));
        assert!(!c.pan_start([600.0, 300.0], false, false, 1050.0));
        assert!(!c.pinch_start([600.0, 300.0], 1099.0));
        assert!(!c.multi_pan_start([600.0, 300.0], [10.0, 0.0], 1099.0));
        assert!(c.pan_start([600.0, 300.0], false, false, 1100.0));
    }

    #[test]
    fn wheel_deltas_as_mjolnir_and_deck_read_them() {
        let w = WheelInput::default();
        assert_eq!(wheel_delta(&WheelInput { delta_y: 100.0, ..w }), -100.0);
        assert_eq!(wheel_delta(&WheelInput { delta_y: 3.0, delta_mode: DeltaMode::Line, ..w }), -120.0);
        assert_eq!(wheel_delta(&WheelInput { delta_y: -100.0, shift_key: true, ..w }), 25.0);
        // a 100-pixel notch: about 0.548 levels (doc 01 §3.5)
        assert_close!(wheel_scale(100.0), 2.0 / (1.0 + (-1.0f64).exp()), 15);
        assert_close!(wheel_scale(100.0).log2(), 0.548, 3);
        assert_close!(wheel_scale(-100.0) * wheel_scale(100.0), 1.0, 15);
        assert_eq!(wheel_scale(0.0), 1.0);
    }

    #[test]
    fn the_globe_camera_frame_turns_as_deck_s() {
        // at the equator a horizontal turn about the polar axis moves the longitude
        let frame = camera_frame(10.0, 0.0, 0.0);
        let turned = rotate_frame(&frame, 5.0 * DEG, 0.0);
        assert_close!(turned.longitude, 15.0, 12);
        assert_close!(turned.latitude, 0.0, 12);
        assert_close!(turned.bearing, 0.0, 12);
        // and a vertical one the latitude
        let up = rotate_frame(&frame, 0.0, 5.0 * DEG);
        assert_close!(up.longitude, 10.0, 12);
        assert_close!(up.latitude, 5.0, 12);
        // turned to match, the frame carries one point onto another, all or part of the way
        let frame = camera_frame(10.0, 50.0, 30.0);
        let matched = rotate_frame_to_match(&frame, [10.0, 50.0], [20.0, 40.0], 1.0);
        assert_close!(matched.longitude, 20.0, 9);
        assert_close!(matched.latitude, 40.0, 9);
        let half = rotate_frame_to_match(&frame, [10.0, 50.0], [10.0, 40.0], 0.5);
        assert_close!(half.latitude, 45.0, 9);
        assert_eq!(rotate_frame_to_match(&frame, [10.0, 50.0], [10.0, 50.0], 1.0), frame);
        // the frame's axes are unit and square to the position
        for axis in [frame.axis_horizontal, frame.axis_vertical] {
            assert_close!(length(axis), 1.0, 12);
            assert_close!(dot(axis, frame.position), 0.0, 12);
        }
    }

    #[test]
    fn map_at_is_finite_off_the_globe() {
        let v = ViewState::INITIAL;
        // the corner of the canvas is off the globe at zoom 1.9; deck still gives a point
        assert!(map_at(&v, SIZE, [0.0, 0.0]).is_some());
        let centre = map_at(&v, SIZE, [550.0, 360.0]).unwrap();
        assert_close!(centre[0], 10.0, 9);
        assert_close!(centre[1], 35.0, 9);
        assert!(matches!(viewport_for(&v, SIZE), Viewport::Globe(_)));
    }

    #[test]
    fn following_a_vehicle_flies_in_to_zoom_13() {
        let mut c = Controller::new(view(10.0, 50.0, 6.0, 0.0, 0.0), SIZE);
        c.fly_to_vehicle(11.9, 57.7, 0.0);
        assert_eq!(c.transition_until(), FOLLOW_VEHICLE_MS);
        assert_eq!(row(&settle(&mut c, 0.0)), [11.9, 57.7, 13.0, 0.0, 0.0]);
        let mut c = Controller::new(view(10.0, 50.0, 15.0, 0.0, 0.0), SIZE);
        c.fly_to_vehicle(11.9, 57.7, 0.0);
        assert_eq!(settle(&mut c, 0.0).zoom, 15.0);
    }
}

/// The lock on an aircraft (item A10): [`locked_view`] against cases worked out by hand from
/// G:3094-3129 (the expected values are that arithmetic done in IEEE doubles, as JavaScript does).
#[cfg(test)]
mod follow_tests {
    use rustradar_core::assert_close;
    use rustradar_core::motion::aircraft::Phase;

    use super::*;

    fn view(longitude: f64, latitude: f64, zoom: f64, bearing: f64, pitch: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing, pitch, position: None }
    }

    /// An aircraft drawn at `lon, lat`, `alt_ft` feet up (the rest of the pose is not read).
    fn pose(lon: f64, lat: f64, alt_ft: f64) -> Pose {
        Pose { lon, lat, alt_ft, heading: 123.0, track: 120.0, pitch: 4.0, bank: -3.0, gear: 0.0, phase: Phase::Cruise }
    }

    #[test]
    fn on_the_globe_the_aim_is_pushed_ahead_so_the_aircraft_is_on_the_camera_axis() {
        // zoom 8 at the equator on a 900 px canvas: the camera is 1.5 · 900 · 305.748 m = 412 760 m
        // out; 10 000 ft at true scale is 3 048 m, far below 0.9 of that, so the tilt is free up to
        // 85°; tilted 45° the aim is 3 048 · tan 45° m north of the aircraft
        let v = view(10.0, 0.0, 8.0, 0.0, 45.0);
        let locked = locked_view(&v, &pose(10.5, 0.0, 10_000.0), 1.0, 900.0);
        assert_eq!(locked.max_pitch, FOLLOW_MAX_PITCH);
        let w = locked.view;
        assert_close!(w.latitude, 3048.0 / METRES_PER_DEGREE, 12);
        assert_close!(w.latitude, 0.02738052461372619, 14);
        assert_eq!(w.longitude, 10.5, "bearing 0: nothing east");
        assert_eq!((w.zoom, w.bearing, w.pitch, w.position), (8.0, 0.0, 45.0, None));

        // turned 90° the aim moves east instead, by lift / (111 320 · cos lat); exaggeration 10 is
        // 2.8 at zoom 10, so 20 000 ft is drawn 17 068.8 m up
        let v = view(18.0, 45.0, 10.0, 90.0, 70.0);
        let locked = locked_view(&v, &pose(18.0, 45.0, 20_000.0), 10.0, 900.0);
        assert_close!(locked.max_pitch, 74.93474803422166, 11);
        assert_eq!(locked.view.pitch, 70.0, "under the cap");
        assert_close!(locked.view.longitude, 18.595770399349984, 11);
        assert_close!(locked.view.longitude - 18.0, 17_068.8 * (70.0 * DEG).tan() / (METRES_PER_DEGREE * (45.0 * DEG).cos()), 12);
        assert_close!(locked.view.latitude, 45.0, 12);
    }

    #[test]
    fn on_the_globe_the_tilt_stops_where_the_camera_would_sink_below_the_aircraft() {
        // the case above tilted 80°: held at acos(17 068.8 / (0.9 · 72 966.34 m)) = 74.93°
        let v = view(18.0, 45.0, 10.0, 90.0, 80.0);
        let locked = locked_view(&v, &pose(18.0, 45.0, 20_000.0), 10.0, 900.0);
        let camera = CAMERA_HEIGHTS * 900.0 * meters_per_pixel(10.0, 45.0);
        assert_close!(camera, 72_966.34039693215, 8);
        assert_close!(locked.max_pitch, (17_068.8 / (0.9 * camera)).acos() / DEG, 12);
        assert_eq!(locked.view.pitch, locked.max_pitch);
        assert_close!(locked.view.longitude, 18.80559696735826, 11);

        // zoom 12 is still the globe; 40 000 ft is above 0.9 of a camera 10 319 m out at 60°N on a
        // 720 px canvas: no tilt at all, the aim on the aircraft
        let v = view(18.2, 60.1, 12.0, 200.0, 50.0);
        let locked = locked_view(&v, &pose(18.0, 60.0, 40_000.0), 10.0, 720.0);
        assert_eq!(locked.max_pitch, 0.0);
        assert_eq!(locked.view, ViewState { longitude: 18.0, latitude: 60.0, zoom: 12.0, bearing: 200.0, pitch: 0.0, position: None });
    }

    #[test]
    fn on_the_ground_the_aim_is_the_aircraft_and_the_tilt_opens_to_85() {
        // a negative altitude is drawn at 0: acos(0) = 90°, held at 85; nothing to push ahead
        let v = view(18.3, 59.2, 9.0, 10.0, 40.0);
        let locked = locked_view(&v, &pose(18.0, 59.0, -50.0), 10.0, 900.0);
        assert_eq!(locked.max_pitch, 85.0);
        assert_eq!(locked.view, ViewState { longitude: 18.0, latitude: 59.0, zoom: 9.0, bearing: 10.0, pitch: 40.0, position: None });
        // and a view tilted past 85 comes back to it
        let locked = locked_view(&ViewState { pitch: 88.0, ..v }, &pose(18.0, 59.0, 0.0), 10.0, 900.0);
        assert_eq!(locked.view.pitch, 85.0);
    }

    #[test]
    fn near_the_poles_the_aim_stops_at_85_and_the_longitude_step_at_cos_0_05() {
        // zoom 6, 35 000 ft at true scale, tilted 60° north from 84.99°N: 18 477.5 m ahead would be
        // 85.156°, clamped to 85
        let v = view(0.0, 84.0, 6.0, 0.0, 60.0);
        let locked = locked_view(&v, &pose(0.0, 84.99, 35_000.0), 1.0, 900.0);
        assert_eq!(locked.max_pitch, 85.0);
        assert_eq!(locked.view.latitude, 85.0);
        assert_eq!(locked.view.longitude, 0.0);
        // turned east at 88°N: cos 88° = 0.035 is taken as 0.05, so 18 477.5 m is 3.32° of longitude
        let v = view(0.0, 84.0, 6.0, 90.0, 60.0);
        let locked = locked_view(&v, &pose(0.0, 88.0, 35_000.0), 1.0, 900.0);
        assert_close!(locked.max_pitch, 78.1287025650075, 11);
        assert_close!(locked.view.longitude, 3.3197121838204775, 11);
        assert_close!(locked.view.longitude, 10_668.0 * (60.0 * DEG).tan() / (METRES_PER_DEGREE * 0.05), 12);
        assert_eq!(locked.view.latitude, 85.0, "the pose's 88° is beyond the pans' limit");
    }

    #[test]
    fn a_bearing_to_the_north_west_moves_the_aim_both_ways() {
        // zoom 11 at 59.3°N on a 720 px canvas: exaggeration is gone (true scale at 11), 5 000 ft
        // = 1 524 m; tilted 30° the aim is 1 524 · tan 30° = 879.9 m ahead along −45°
        let v = view(18.0, 59.3, 11.0, -45.0, 30.0);
        let locked = locked_view(&v, &pose(18.0, 59.3, 5_000.0), 10.0, 720.0);
        assert_eq!(locked.max_pitch, 85.0);
        assert_close!(locked.view.latitude, 59.30558902618277, 11);
        assert_close!(locked.view.longitude, 17.989052778940426, 11);
        let lift = 1524.0 * (30.0 * DEG).tan();
        assert_close!(lift, 879.8818102449897, 9);
        assert_close!(locked.view.latitude - 59.3, lift * (-45.0 * DEG).cos() / METRES_PER_DEGREE, 12);
    }

    #[test]
    fn on_the_flat_map_the_target_is_raised_to_the_aircraft() {
        // zoom 14.5: true scale, 3 000 ft = 914.4 m; centred on the pose at any tilt and bearing,
        // which are kept, and the tilt may reach 85°
        let v = view(18.0, 59.6, 14.5, 30.0, 60.0);
        let locked = locked_view(&v, &pose(17.9, 59.65, 3000.0), 10.0, 900.0);
        assert_eq!(locked.max_pitch, FOLLOW_MAX_PITCH);
        assert_eq!(
            locked.view,
            ViewState { longitude: 17.9, latitude: 59.65, zoom: 14.5, bearing: 30.0, pitch: 60.0, position: Some([0.0, 0.0, 3000.0 * 0.3048]) }
        );
        // the flat map does not cap the tilt itself (deck's 85° limit does)
        let steep = locked_view(&ViewState { pitch: 84.0, ..v }, &pose(17.9, 59.65, 3000.0), 10.0, 900.0);
        assert_eq!(steep.view.pitch, 84.0);
        // just past the swap, a pose on the ground: raised by 0
        let ground = locked_view(&view(18.0, 59.6, 12.01, 0.0, 0.0), &pose(18.0, 59.6, 0.0), 10.0, 900.0);
        assert_eq!(ground.view.position, Some([0.0, 0.0, 0.0]));
    }

    /// Where the aircraft drawn at `p` (raised as the aircraft builder raises it) is on screen once
    /// locked from `v` on a `size` canvas: its distance from the centre, px, and its depth (NDC z).
    fn locked_on_screen(v: ViewState, p: &Pose, size: [f64; 2]) -> (LockedView, f64, f64) {
        let locked = locked_view(&v, p, 10.0, size[1]);
        let altitude = elevation(p.alt_ft, effective_exaggeration(10.0, v.zoom));
        let [x, y, depth] = viewport_for(&locked.view, size).project([p.lon, p.lat, altitude]);
        (locked, (x - size[0] / 2.0).hypot(y - size[1] / 2.0), depth)
    }

    #[test]
    fn the_locked_aircraft_is_at_the_centre_of_the_screen() {
        // what the lock is for: the pose, raised as it is drawn, projects to the middle of the
        // canvas through the viewport the map draws with, on the globe and the flat map, tilted,
        // turned, low and high. The flat map's raised target is exact; the globe's aim, pushed
        // ahead on a flat earth with 111 320 m to the degree, is within a pixel or two
        let size = [1100.0, 720.0];
        for (zoom, pitch, bearing, alt_ft, within) in [
            (5.0, 50.0, 30.0, 35_000.0, 2.0),
            (8.0, 45.0, 0.0, 10_000.0, 0.5),
            (9.0, 40.0, 250.0, 8_000.0, 0.5),
            (11.0, 55.0, 160.0, 12_000.0, 0.5),
            (12.0, 60.0, 120.0, 3_000.0, 0.1),
            (12.5, 60.0, 120.0, 3_000.0, 1e-6),
            (16.0, 80.0, 300.0, 1_200.0, 1e-6),
            (19.5, 84.0, 45.0, 200.0, 1e-6),
        ] {
            let (_, off, depth) = locked_on_screen(view(18.0, 59.0, zoom, bearing, pitch), &pose(17.95, 59.6, alt_ft), size);
            let what = format!("zoom {zoom} pitch {pitch} bearing {bearing} at {alt_ft} ft");
            eprintln!("{what}: {off:.4} px off the centre");
            assert!(off < within, "{what}: {off} px off the centre");
            assert!(depth.abs() < 1.0, "{what}: in front of the camera's near plane, depth {depth}");
        }
    }

    #[test]
    fn tilted_to_its_cap_on_the_globe_the_aircraft_comes_inside_the_near_plane_as_today() {
        // today's trap, kept: at the cap the camera is just above the aircraft (0.9 of its height
        // over the aim is the aircraft's), so the aircraft is a tenth of the camera's distance in
        // front of it, closer than deck's near plane (a third): it is not drawn at all, and the flat
        // earth's error shows tens of pixels there
        let size = [1100.0, 720.0];
        let (locked, off, depth) = locked_on_screen(view(18.0, 59.0, 10.0, 250.0, 70.0), &pose(17.95, 59.6, 20_000.0), size);
        assert!(locked.max_pitch < 70.0 && locked.view.pitch == locked.max_pitch, "{locked:?}");
        eprintln!("at the cap: {off:.1} px off the centre, depth {depth}");
        assert!(depth < -1.0, "clipped by the near plane: depth {depth}");
    }

    #[test]
    fn released_closer_or_steeper_than_the_free_camera_eases_back_to_zoom_20_and_pitch_60() {
        assert_eq!(released_ease_back(&view(18.0, 59.6, 20.0, 0.0, 60.0)), None, "within the limits");
        assert_eq!(
            released_ease_back(&view(18.0, 59.6, 21.5, 30.0, 40.0)),
            Some(ViewPatch { zoom: Some(20.0), pitch: Some(40.0), ..ViewPatch::default() })
        );
        assert_eq!(
            released_ease_back(&view(18.0, 59.6, 14.0, 30.0, 80.0)),
            Some(ViewPatch { zoom: Some(14.0), pitch: Some(60.0), ..ViewPatch::default() })
        );
    }
}
