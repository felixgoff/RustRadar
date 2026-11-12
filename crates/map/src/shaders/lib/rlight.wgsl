// lib/rlight.wgsl: the realistic map's light (doc 04 §4; `realistic.ts`'s `rlight` shader module,
// after Streets GL): the uniform every realistic layer reads, the terms its vertex shader works out
// (deck's varyings `rl_eye`, `rl_sun`, `rl_t`, `rl_c`), and the fragment library (physically based
// shading in linear light, the sky's light, aerial perspective, the screen's gamma). Its Rust twin
// is `look::twin` (f64), which the tests hold the GPU to; keep the two in step.
//
// A realistic shader starts with `//!include lib/rlight.wgsl` (it pulls in lib/project.wgsl), binds
// `look::LightUniform` as group 2, and:
// - in its vertex shader, `let p = project_local(local); let rl = rl_vertex(p.rel, rl_mercator(local_offset(local.xy)));`
//   and hands `rl`'s four fields to the fragment shader (four `@location`s, interpolated);
// - in its fragment shader, rebuilds the `RlVary` and calls the library with it.
//
// How the GLSL became WGSL (doc 04 §16.2): the varyings are a struct passed to every function
// that read them; the mutable global `rl_glint` is `rl_shade`'s last parameter; `mod` is written
// out (`rl_mod`); `smoothstep(0.0, -0.2, up)` is `1.0 - smoothstep(-0.2, 0.0, up)`; `pow(1 − v·h, 5)`
// takes `max(…, 0)` first (the GLSL leaves a base of −1e-7 to the driver). `rl_cells` takes
// derivatives: call it in uniform control flow only.
//
// Where the port differs from deck's frames (the values are the same):
// - deck measures `rl_m` from the layer's common origin (flat) or the view centre's longitude and
//   latitude (globe); here it is measured from the view centre in both, from the vertex's world
//   Mercator offset (`local_offset`), so one uniform per frame serves every draw (`origin` and
//   `cloud_origin` are the view centre's within the periods);
// - the globe's common space is turned to the render frame (lib/project.wgsl), so `sun_globe` is
//   given in the render frame (`look::RLight::new`), and the east, north and up of a point are
//   worked out there: east = the Earth's axis × up, as deck's `vec3(-up.y, up.x, 0)` is ẑ × up.

//!include lib/project.wgsl

// web mercator's units (512 round the world) per degree of longitude (deck's MERCATOR_PER_DEGREE
// as the TypeScript writes it into the shader: toFixed(10))
const RL_MERCATOR_PER_DEGREE: f32 = 1.4222222222;
// ...and per metre at the equator (toExponential(10))
const RL_MERCATOR_PER_METRE: f32 = 1.2776039596e-5;
const RL_PI: f32 = 3.14159265;

// `look::RLight`: doc 04 §4.1's block, member for member (192 bytes; `globe` at 160, `cover` at 164,
// `eye` at 176).
struct RLight {
    sun_dir: vec3<f32>,
    sun_up: f32,
    sun_color: vec3<f32>,
    ambient: f32,
    sky_zenith: vec3<f32>,
    exposure: f32,
    sky_horizon: vec3<f32>,
    haze_density: f32,
    haze_color: vec3<f32>,
    wet: f32,
    ground_color: vec3<f32>,
    snow: f32,
    sun_globe: vec3<f32>,
    haze_start: f32,
    origin: vec2<f32>,
    period: f32,
    period_metres: f32,
    cloud_origin: vec2<f32>,
    cloud_period: f32,
    cloud_shadow: f32,
    anchor: vec2<f32>,
    night: f32,
    time: f32,
    globe: f32,
    cover: f32,
    eye: vec3<f32>,
}

@group(2) @binding(0) var<uniform> rlight: RLight;

// deck's four varyings: to the camera from here (metres east, north, up), towards the sun here
// (east, north, up: on the globe in the place's own frame, so `sun.z` is the sun's height there),
// and where it is for what is fixed to the world, in texture periods and cloud periods.
struct RlVary {
    eye: vec3<f32>,
    sun: vec3<f32>,
    t: vec2<f32>,
    c: vec2<f32>,
}

// ------------------------------------------------------------------ vertex part

// East, north and up at a point `p` of the globe (the render frame, from the Earth's centre).
fn rl_frame(p: vec3<f32>) -> mat3x3<f32> {
    let up = normalize(p);
    // the Earth's axis in the render frame (common space's +z turned by Rx(φc))
    let pole = vec3<f32>(0.0, -globals.centre.y, globals.centre.x);
    // (deck nudges east by 1e-9 so the pole has one too)
    let east = normalize(cross(pole, up) + vec3<f32>(1e-9, 0.0, 0.0));
    return mat3x3<f32>(east, cross(up, east), up);
}

// cos of the latitude half way from the view centre to a point `dlat` radians north of it, at
// least 0.01 (deck's `rl_cos`)
fn rl_cos_mid(dlat: f32) -> f32 {
    let half = sin_vers(0.5 * dlat);
    return max(globals.centre.x * (1.0 - half.y) - globals.centre.y * half.x, 0.01);
}

// Where a point lies for what is fixed to the world (deck's `rl_m`), from its world Mercator offset
// `d` from the view centre (`local_offset`): on the flat map `d` itself; on the globe web mercator's
// units from the view's centre, flattened there so they fit the flat map's:
// (Δλ, Δφ / cos(φc + Δφ/2)) in degrees · 512/360.
fn rl_mercator(d: vec2<f32>) -> vec2<f32> {
    if (globals.mode == MODE_FLAT) {
        return d;
    }
    let dx = d.x - 512.0 * round(d.x / 512.0);
    let dlat = delta_latitude(d.y * RADIANS_PER_UNIT);
    return vec2<f32>(dx, degrees(dlat) * RL_MERCATOR_PER_DEGREE / rl_cos_mid(dlat));
}

// A line's vertex on the globe (deck's `RL_LINE`): what lies `side` metres east and north of its
// middle `d` adds this to `rl_mercator(d)`. (On the flat map a line's vertex has its own place.)
fn rl_line_side(d: vec2<f32>, side: vec2<f32>) -> vec2<f32> {
    if (globals.mode == MODE_FLAT) {
        return vec2<f32>(0.0);
    }
    return side * (RL_MERCATOR_PER_METRE / rl_cos_mid(delta_latitude(d.y * RADIANS_PER_UNIT)));
}

// The light's terms at a vertex at `rel` (`MapPosition.rel`: the render frame, from the view
// centre) whose place for what is fixed to the world is `m` (`rl_mercator`).
fn rl_vertex(rel: vec3<f32>, m: vec2<f32>) -> RlVary {
    var out: RlVary;
    // (draped on the 3D terrain, stage 9: seen from the map's own camera, `rlight.eye`)
    var camera = globals.camera_render;
    if (rlight.cover > 0.5) {
        camera = rlight.eye;
    }
    let to_camera = (camera - rel) / globals.centre.z;
    if (globals.mode == MODE_FLAT) {
        // the flat map's common space is east, north and up already
        out.eye = to_camera;
        out.sun = rlight.sun_dir;
    } else {
        let enu = rl_frame(rel + globals.centre_offset + vec3<f32>(0.0, -GLOBE_RADIUS, 0.0));
        out.eye = to_camera * enu;
        out.sun = rlight.sun_globe * enu;
    }
    out.t = (rlight.origin + m) / rlight.period;
    out.c = (rlight.cloud_origin + m) / rlight.cloud_period;
    return out;
}

// ------------------------------------------------------------------ fragment library

fn rl_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

// the screen's gamma, as Streets GL shows its colours: no tone curve to dull them
fn rl_display(c: vec3<f32>) -> vec3<f32> {
    return pow(clamp(c * rlight.exposure, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
}

// GLSL's `mod`: `x − y·floor(x/y)` (WGSL's `%` truncates)
fn rl_mod(x: vec2<f32>, y: f32) -> vec2<f32> {
    return x - y * floor(x / y);
}

// value noise on a lattice `n` cells to the period, repeating with it
fn rl_hash(cell: vec2<f32>, n: f32) -> f32 {
    let c = rl_mod(cell, n);
    return fract(sin(dot(c, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn rl_noise(t: vec2<f32>, n: f32) -> f32 {
    let p = t * n;
    let i = floor(p);
    var f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(rl_hash(i, n), rl_hash(i + vec2<f32>(1.0, 0.0), n), f.x),
        mix(rl_hash(i + vec2<f32>(0.0, 1.0), n), rl_hash(i + vec2<f32>(1.0, 1.0), n), f.x),
        f.y,
    );
}

// how many cells of a lattice in texture coordinates `t` one pixel spans (more than one: too fine
// to show); derivatives: uniform control flow only
fn rl_cells(t: vec2<f32>) -> f32 {
    let d = fwidth(t);
    return max(d.x, d.y);
}

// towards the sun from here
fn rl_to_sun(rl: RlVary) -> vec3<f32> {
    return normalize(rl.sun);
}

// the sun's light here: none where the globe has turned away from it (from far out, dimming and
// reddening through the evening as its light comes through more air)
fn rl_sun_light(rl: RlVary) -> vec3<f32> {
    let up = rl.sun.z;
    let far = rlight.globe;
    let dusk = mix(vec3<f32>(1.0), vec3<f32>(1.0, 0.62, 0.38), far * (1.0 - smoothstep(0.0, 0.3, up)));
    return rlight.sun_color * dusk * smoothstep(mix(-0.02, -0.03, far), mix(0.02, 0.22, far), up);
}

// how much of the day's sky light there is here: all of it, but from far over the globe only where
// the sun is up here, fading through twilight to a little at night
fn rl_day_here(rl: RlVary) -> f32 {
    return mix(1.0, mix(0.015, 1.0, smoothstep(-0.12, 0.12, rl.sun.z)), rlight.globe);
}

// the sky's light from a direction, with the sun's glow round it; below the horizon, the ground's
fn rl_sky(rl: RlVary, dir: vec3<f32>) -> vec3<f32> {
    let up = clamp(dir.z, -1.0, 1.0);
    var sky = mix(rlight.sky_horizon, rlight.sky_zenith, sqrt(max(up, 0.0)));
    let to_sun = max(dot(dir, rl_to_sun(rl)), 0.0);
    sky += rl_sun_light(rl) * (0.04 * pow(to_sun, 6.0) + 0.5 * pow(to_sun, 900.0));
    // (the GLSL's smoothstep(0.0, -0.2, up), its edges turned round)
    return mix(sky, rlight.ground_color, 1.0 - smoothstep(-0.2, 0.0, up)) * rl_day_here(rl);
}

// all the light a surface facing `n` gets from the sky: grey, as Streets GL has it (a little less
// facing down)
fn rl_irradiance(rl: RlVary, n: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(rlight.ambient * rl_day_here(rl)) * mix(0.6, 1.0, clamp(0.5 + 0.5 * n.z, 0.0, 1.0));
}

// the split-sum environment BRDF (Karis's fit for mobile, as Streets GL has it)
fn rl_env_brdf(n_dot_v: f32, roughness: f32) -> vec2<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    return vec2<f32>(-1.04, 1.04) * a004 + r.zw;
}

// a surface lit by the sun (where `sun_visible`) and the sky: GGX, Smith-Schlick, Schlick's
// Fresnel; `glint` is how much of the sun's own reflection it shows (the GLSL's global `rl_glint`,
// 1 but where the water lowers it)
fn rl_shade(
    rl: RlVary,
    albedo: vec3<f32>,
    n: vec3<f32>,
    v: vec3<f32>,
    rough: f32,
    metalness: f32,
    ao: f32,
    sun_visible: f32,
    glint: f32,
) -> vec3<f32> {
    let l = rl_to_sun(rl);
    let roughness = clamp(rough, 0.04, 1.0);
    let n_dot_l = max(dot(n, l), 0.0);
    let n_dot_v = max(dot(n, v), 1e-3);
    let h = normalize(l + v);
    let n_dot_h = max(dot(n, h), 0.0);
    let v_dot_h = max(dot(v, h), 0.0);
    let f0 = mix(vec3<f32>(0.04), albedo, metalness);
    let fresnel = f0 + (1.0 - f0) * pow(max(1.0 - v_dot_h, 0.0), 5.0);
    let a = roughness * roughness;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    let distribution = a2 / (RL_PI * d * d);
    let k = a * 0.5;
    let vis = 0.25 / max((n_dot_l * (1.0 - k) + k) * (n_dot_v * (1.0 - k) + k), 1e-4);
    let diffuse = albedo * (1.0 - metalness);
    let direct = ((1.0 - fresnel) * diffuse + fresnel * min(distribution * vis * RL_PI, 60.0) * glint) * rl_sun_light(rl) * n_dot_l * sun_visible;
    let brdf = rl_env_brdf(n_dot_v, roughness);
    let r = reflect(-v, n);
    let env = mix(rl_sky(rl, r), rl_irradiance(rl, r) / max(rlight.ambient, 1e-3), roughness);
    let specular = env * (f0 * brdf.x + brdf.y) * (1.0 - 0.5 * roughness);
    return direct + (diffuse * rl_irradiance(rl, n) + specular) * ao;
}

// how much of what is here gets through the air to the camera (fog only from some way out, so the
// map stays readable); draped on the 3D terrain its surface takes the air, as it is drawn
fn rl_clear(rl: RlVary) -> f32 {
    if (rlight.cover > 0.5) {
        return 1.0;
    }
    return exp(-max(length(rl.eye) - rlight.haze_start, 0.0) * rlight.haze_density);
}

// the air between here and the camera: dimming and adding its own light (brighter towards the sun)
fn rl_aerial(rl: RlVary, c: vec3<f32>) -> vec3<f32> {
    let t = rl_clear(rl);
    let to_here = -rl.eye / max(length(rl.eye), 1e-3);
    let forward = pow(max(dot(to_here, rl_to_sun(rl)), 0.0), 8.0);
    let air = rlight.haze_color * rl_day_here(rl) + rl_sun_light(rl) * 0.06 * forward;
    return c * t + air * (1.0 - t);
}

// how much of the sun the clouds let through here, drifting with the wind
fn rl_sun_visible(rl: RlVary) -> f32 {
    if (rlight.cloud_shadow <= 0.0) {
        return 1.0;
    }
    let c = rl.c + vec2<f32>(rlight.time * 0.0006, rlight.time * 0.00025);
    let n = 0.55 * rl_noise(c, 4.0) + 0.3 * rl_noise(c, 9.0) + 0.15 * rl_noise(c, 23.0);
    return 1.0 - rlight.cloud_shadow * smoothstep(0.46, 0.6, n);
}
