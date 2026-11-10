// lib/project.wgsl: where the map's vertices land on the screen (plan 1.4.2). Every map shader
// starts with `//!include lib/project.wgsl`. Its Rust twin is `frame::model`, which the test
// `projection_precision` runs against `scene::camera` in f64: keep the two in step, operation for
// operation.
//
// Bind groups: 0 = `globals` (once per frame), 1 = `frame` (once per draw, dynamic offset), 2 and
// up belong to the pipeline.
//
// Spaces (all relative to the view centre, so the GPU never sees a large number):
// - Local Mercator: a vertex's `(u, v)` (u east, v south) in units of `frame.scale` world units
//   from the frame's anchor, plus a height in metres. Tile meshes use the tile's north-west corner
//   and tile units, u and v in [0, 1].
// - World Mercator offset: the vertex minus the view centre in deck's 512-unit world (y north).
//   The anchor's offset arrives as a hi + lo pair of f32 so the sum keeps f64 precision near the
//   centre; `frame.scale` is a power of two, so `u · scale` is exact.
// - Render frame: what `globals.centre_cam` takes to clip space. On the flat map (zoom > 12) it is
//   deck's common space, east +x, north +y, up +z. On the globe it is common space turned by
//   Rx(latitude)·Rz(−longitude) of the view centre: there the centre's surface normal is −y, east
//   +x, north +z, and a vertex's position follows from its latitude and longitude differences to
//   the centre without one large intermediate value.
// - Common offset: deck's common space minus the view centre, for things the CPU places in f64
//   (aircraft, ships, trains, markers, labels); `globals.view_proj` takes it to clip space.

const PI: f32 = 3.141592653589793;
// radians per world Mercator unit (the world is 512 units round)
const RADIANS_PER_UNIT: f32 = 0.01227184630308513;
// deck's globe: a sphere of 256 common units for an Earth of 6 370 972 m
const GLOBE_RADIUS: f32 = 256.0;
const EARTH_RADIUS_M: f32 = 6370972.0;
// values of `globals.mode`
const MODE_GLOBE: u32 = 0u;
const MODE_FLAT: u32 = 1u;
// below this, sinh, cosh and atan use their series (exact to f32 there, where a GPU's own may only
// be good to a few thousand ULP); sin and cos never use the GPU's (WGSL allows an absolute 2^-11)
const SMALL: f32 = 1.0;

// `frame::Globals`.
struct Globals {
    // common offset → clip (wgpu depth range)
    view_proj: mat4x4<f32>,
    // render frame → clip; equal to `view_proj` on the flat map
    centre_cam: mat4x4<f32>,
    // globe: the view centre (raised by `position`) in the render frame, minus (0, −256, 0);
    // flat: 0
    centre_offset: vec3<f32>,
    // MODE_GLOBE or MODE_FLAT
    mode: u32,
    // cos and sin of the latitude the render frame is turned to (the view's latitude, within
    // ±89.9° on the globe); common units per metre at the view centre; flat: the raised target's
    // height in metres (`position[2]`), globe: 0
    centre: vec4<f32>,
    // the camera's common offset (common space)
    camera_common: vec3<f32>,
    // CSS pixels per common unit at the view centre (deck's `scale`)
    pixels_per_unit: f32,
    // the camera's position in the render frame
    camera_render: vec3<f32>,
    // metres per CSS pixel at the view centre (deck's `metersPerPixel`)
    metres_per_pixel: f32,
    // canvas width and height in CSS pixels, map pixels per CSS pixel, seconds of the local clock
    viewport: vec4<f32>,
    // flat: world y of the view centre minus world y of the view's latitude (0 unless `position`
    // moves the centre north or south); the rest unused
    mercator: vec4<f32>,
}

// `frame::Frame`: one draw's local Mercator.
struct Frame {
    // the anchor minus the view centre in world Mercator, split as hi + lo
    offset_hi: vec2<f32>,
    offset_lo: vec2<f32>,
    // world units per local unit (a power of two)
    scale: f32,
    // metres added to every vertex's height
    z_offset: f32,
    pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> frame: Frame;

// A projected vertex.
struct MapPosition {
    clip: vec4<f32>,
    // the vertex in the render frame, relative to the view centre (common units)
    rel: vec3<f32>,
}

// (sin x, 1 − cos x) for |x| ≤ 4, to f32 precision relative to their size: the series at x/4,
// then sin 2a = 2·sin a·cos a and 1 − cos 2a = 2·sin² a twice.
fn sin_vers(x: f32) -> vec2<f32> {
    let q = 0.25 * x;
    let q2 = q * q;
    let s1 = q * (1.0 - q2 / 6.0 * (1.0 - q2 / 20.0 * (1.0 - q2 / 42.0 * (1.0 - q2 / 72.0 * (1.0 - q2 / 110.0)))));
    let v1 = q2 / 2.0 * (1.0 - q2 / 12.0 * (1.0 - q2 / 30.0 * (1.0 - q2 / 56.0 * (1.0 - q2 / 90.0 * (1.0 - q2 / 132.0)))));
    let s2 = 2.0 * s1 * (1.0 - v1);
    let v2 = 2.0 * s1 * s1;
    return vec2<f32>(2.0 * s2 * (1.0 - v2), 2.0 * s2 * s2);
}

// sinh x
fn sinh_small(x: f32) -> f32 {
    if (abs(x) < SMALL) {
        let x2 = x * x;
        return x * (1.0 + x2 / 6.0 * (1.0 + x2 / 20.0 * (1.0 + x2 / 42.0 * (1.0 + x2 / 72.0 * (1.0 + x2 / 110.0)))));
    }
    return sinh(x);
}

// cosh x − 1
fn cosh_m1_small(x: f32) -> f32 {
    let x2 = x * x;
    if (abs(x) < SMALL) {
        return x2 / 2.0 * (1.0 + x2 / 12.0 * (1.0 + x2 / 30.0 * (1.0 + x2 / 56.0 * (1.0 + x2 / 90.0 * (1.0 + x2 / 132.0)))));
    }
    return cosh(x) - 1.0;
}

// atan y: below 1 the half-angle step atan y = 2·atan(y / (1 + sqrt(1 + y²))) and the series,
// above it π/2 − atan(1/y)
fn atan_small(y: f32) -> f32 {
    let r = select(y, 1.0 / y, abs(y) > 1.0);
    let t = r / (1.0 + sqrt(1.0 + r * r));
    let t2 = t * t;
    let series = 1.0 / 13.0 - t2 * (1.0 / 15.0 - t2 * (1.0 / 17.0 - t2 / 19.0));
    let a = 2.0 * t * (1.0 - t2 * (1.0 / 3.0 - t2 * (1.0 / 5.0 - t2 * (1.0 / 7.0 - t2 * (1.0 / 9.0 - t2 * (1.0 / 11.0 - t2 * series))))));
    if (abs(y) > 1.0) {
        return sign(y) * 0.5 * PI - a;
    }
    return a;
}

// The world Mercator offset of a local point from the view centre.
fn local_offset(local: vec2<f32>) -> vec2<f32> {
    let w = vec2<f32>(local.x, -local.y) * frame.scale;
    return (w + frame.offset_hi) + frame.offset_lo;
}

// The latitude, radians, of a point `dpsi` radians of Mercator y north of the render frame's
// latitude φc, minus φc: 2·atan(cos φc·sinh(dψ/2) / (cosh(dψ/2) + sin φc·sinh(dψ/2))).
fn delta_latitude(dpsi: f32) -> f32 {
    let s = sinh_small(0.5 * dpsi);
    let c = 1.0 + cosh_m1_small(0.5 * dpsi);
    return 2.0 * atan_small(globals.centre.x * s / (c + globals.centre.y * s));
}

// The globe: a point `d` (world Mercator offset from the view centre) at `h` metres, in the render
// frame relative to the view centre. On the unit sphere, with Δλ and Δφ the differences to the
// centre and vers = 1 − cos:
//   (cos φ·sin Δλ,  −1 + vers Δφ + cos φ·cos φc·vers Δλ,  sin Δφ + cos φ·sin φc·vers Δλ)
// scaled by (1 + h/R)·256, against the centre's (0, −256, 0).
fn globe_rel(d: vec2<f32>, h: f32) -> vec3<f32> {
    let cos_c = globals.centre.x;
    let sin_c = globals.centre.y;
    // the longitude difference within ±180°
    let dx = d.x - 512.0 * round(d.x / 512.0);
    let lon = sin_vers(dx * RADIANS_PER_UNIT);
    let lat = sin_vers(delta_latitude(d.y * RADIANS_PER_UNIT));
    let cos_lat = cos_c * (1.0 - lat.y) - sin_c * lat.x;
    let lift = h / EARTH_RADIUS_M;
    let dist = GLOBE_RADIUS * (1.0 + lift);
    let p = vec3<f32>(
        dist * (cos_lat * lon.x),
        dist * (lat.y + cos_lat * cos_c * lon.y) - GLOBE_RADIUS * lift,
        dist * (lat.x + cos_lat * sin_c * lon.y),
    );
    return p - globals.centre_offset;
}

// The flat map: a point `d` at `h` metres, relative to the view centre in common space. Heights
// become units with the scale at the point's own latitude, as deck's `projectPosition` does:
// units/m at the point = units/m at the centre · (1 + f), f = cosh x − 1 + sin φc·sinh x, x the
// point's Mercator y north of the centre's latitude in radians; and the raised target's height
// comes off before the scaling, so a vertex near it keeps its precision.
fn flat_rel(d: vec2<f32>, h: f32) -> vec3<f32> {
    let x = (d.y + globals.mercator.x) * RADIANS_PER_UNIT;
    let f = cosh_m1_small(x) + globals.centre.y * sinh_small(x);
    let raised = globals.centre.w;
    let z = globals.centre.z * ((h - raised) * (1.0 + f) + raised * f);
    return vec3<f32>(d, z);
}

// A vertex of local Mercator (`local.z` in metres) through `frame`.
fn project_local(local: vec3<f32>) -> MapPosition {
    let d = local_offset(local.xy);
    let h = local.z + frame.z_offset;
    var rel: vec3<f32>;
    if (globals.mode == MODE_FLAT) {
        rel = flat_rel(d, h);
    } else {
        rel = globe_rel(d, h);
    }
    return MapPosition(globals.centre_cam * vec4<f32>(rel, 1.0), rel);
}

// A common offset (common space minus the view centre, worked out on the CPU in f64) to clip space.
fn project_common(offset: vec3<f32>) -> vec4<f32> {
    return globals.view_proj * vec4<f32>(offset, 1.0);
}

// East, north and up at a local Mercator point, as the columns of a matrix, in the render frame.
fn local_basis(local: vec2<f32>) -> mat3x3<f32> {
    if (globals.mode == MODE_FLAT) {
        return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    }
    let cos_c = globals.centre.x;
    let sin_c = globals.centre.y;
    let d = local_offset(local);
    let lon = sin_vers((d.x - 512.0 * round(d.x / 512.0)) * RADIANS_PER_UNIT);
    let lat = sin_vers(delta_latitude(d.y * RADIANS_PER_UNIT));
    let cos_lat = cos_c * (1.0 - lat.y) - sin_c * lat.x;
    let sin_lat = sin_c * (1.0 - lat.y) + cos_c * lat.x;
    let cl = 1.0 - lon.y;
    let sl = lon.x;
    // in common space turned by Rz(−λc), then Rx(φc)
    let e = vec3<f32>(cl, sl, 0.0);
    let n = vec3<f32>(-sl * sin_lat, cl * sin_lat, cos_lat);
    let u = vec3<f32>(sl * cos_lat, -cl * cos_lat, sin_lat);
    let rx = mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, cos_c, sin_c), vec3<f32>(0.0, -sin_c, cos_c));
    return mat3x3<f32>(rx * e, rx * n, rx * u);
}

// `clip` moved by `px` CSS pixels on the screen (x right, y down), whatever its depth.
fn offset_clip_px(clip: vec4<f32>, px: vec2<f32>) -> vec4<f32> {
    let ndc = vec2<f32>(px.x, -px.y) * 2.0 / globals.viewport.xy;
    return vec4<f32>(clip.xy + ndc * clip.w, clip.zw);
}
