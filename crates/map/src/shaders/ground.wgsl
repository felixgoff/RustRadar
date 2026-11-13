//!include lib/rlight.wgsl
// ground.wgsl: the realistic map's ground (doc 04 §6.8; `realistic-ground.ts`'s `GroundExtension`,
// `RG:463-624`): areas and lines in Streets GL's ground textures, procedural water and fields,
// snow, wetness and, far out, the Earth's own colours, lit by `lib/rlight.wgsl`. The Rust side is
// `layers::ground` (the pipelines, the surface table, the textures); the tiles' geometry is
// `scene::tess::ground`.
//
// Entry points: `vs_area`/`fs_area` for filled areas (deck's SolidPolygonLayer: the ground-land
// rectangle, the tiles' polygons; `vs_platform`/`fs_area` for extruded ones whose sides face their
// own way, the platforms), `vs_line`/`fs_line` for lines (deck's PathLayer with widths in
// metres, round joints and caps, `geometry.uv` for the textures laid along a line; GLSL's
// `RL_LINE`: a line's sides lie a way out from its middle, and lines take no town lights).
//
// What differs from the GLSL, and why (doc 04 §16.2):
// - a feature's paint arrives as explicit integers, tint and surface id (`Uint8x4`, flat), not as
//   a colour whose alpha is 255 less the id; an id of 42 or more draws nothing (alpha 0), as there;
// - the surface table (`GS_LAYER`, `GS_METRES`, …) is a uniform the Rust side fills from
//   `scene::ground`'s `SURFACES`, two rows a surface;
// - WGSL takes derivatives only in uniform control flow, and the GLSL samples its textures and calls
//   `rl_cells` inside branches on the surface. So every derivative is taken first, of `rl_t`,
//   `rl_c` and the line's `uv`, and what the GLSL differentiates is scaled from them: `rl_cells(k·t)`
//   is `k·rl_cells(t)` for the constant scales `k` it uses, and a texture is sampled with
//   `textureSampleGrad` at the derivatives of its coordinates (the scale, a field's rotation and the
//   picture's turned `v` applied to them). Where a field's rotation changes between neighbouring
//   pixels (its edge) the GLSL's implicit derivatives jump and pick a blurred level for a pixel; here
//   they do not. The Earth's pictures are sampled before any branch;
// - `textureLod(arr, vec3(0.5, 0.5, layer), 16.0)` (a layer's mean colour) is `textureSampleLevel`
//   at level 16, clamped to the last level as there;
// - the global `rl_glint` is `rl_shade`'s last parameter; `mat2(…)` is `mat2x2` (column-major in
//   both).

const SURFACE_COUNT: u32 = 42u;
const FARMLAND: u32 = 3u;
const FIELD: u32 = 17u;
const ICE: u32 = 14u;
const UP: vec3<f32> = vec3<f32>(0.0, 0.0, 1.0);
const LUM: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// `layers::ground::GroundUniform`.
struct Ground {
    // per surface id two rows: (GS_LAYER, GS_METRES, GS_ROUGH, GS_SNOW) and
    // (GS_PAVED, GS_FLAT_R, GS_FLAT_G, GS_FLAT_B)
    surfaces: array<vec4<f32>, 84>,
    // the lines' widthMinPixels and widthMaxPixels; the rest unused
    line: vec4<f32>,
}

@group(3) @binding(0) var<uniform> ground: Ground;
// rground_color (sRGB) and rground_normal: the ground's texture arrays (doc 04 §5.2)
@group(3) @binding(1) var ground_color: texture_2d_array<f32>;
@group(3) @binding(2) var ground_normal: texture_2d_array<f32>;
// rground_water, rground_earth (sRGB; alpha 0 until loaded), rground_night (sRGB)
@group(3) @binding(3) var water_normal: texture_2d<f32>;
@group(3) @binding(4) var earth_color: texture_2d<f32>;
@group(3) @binding(5) var earth_night: texture_2d<f32>;
// repeat both ways (the ground's arrays, the water); round the world, not over the poles (the Earth)
@group(3) @binding(6) var repeat_sampler: sampler;
@group(3) @binding(7) var globe_sampler: sampler;

struct GroundOut {
    @builtin(position) clip: vec4<f32>,
    // `rl_eye`, `rl_sun`, `rl_t`, `rl_c`
    @location(0) eye: vec3<f32>,
    @location(1) sun: vec3<f32>,
    @location(2) t: vec2<f32>,
    @location(3) c: vec2<f32>,
    // `rg_e`: where it is on the pictures of the Earth
    @location(4) e: vec2<f32>,
    // tint (0–255) and surface id
    @location(5) @interpolate(flat) paint: vec4<u32>,
    // `rg_wall`: an extruded area's side's normal, else 0
    @location(6) wall: vec3<f32>,
    // lines: deck's `geometry.uv` (`vPathPosition`), the corner's offset and the segment's length
    // in half widths (`vCornerOffset`, `vPathLength`); 0 for areas
    @location(7) uv: vec2<f32>,
    @location(8) corner_offset: vec2<f32>,
    @location(9) path_length: f32,
}

// `rg_e` for a point `d` (world Mercator offset from the view centre): its longitude and latitude
// on the globe (`geometry.worldPosition`), the view's centre on the flat map (`rlight.anchor`), as
// equirectangular coordinates into the whole-Earth pictures.
fn earth_place(d: vec2<f32>) -> vec2<f32> {
    var ll = rlight.anchor;
    if (globals.mode != MODE_FLAT) {
        let dx = d.x - 512.0 * round(d.x / 512.0);
        let centre_lat = atan2(globals.centre.y, globals.centre.x);
        ll = vec2<f32>(rlight.anchor.x + degrees(dx * RADIANS_PER_UNIT), degrees(centre_lat + delta_latitude(d.y * RADIANS_PER_UNIT)));
    }
    return vec2<f32>((ll.x + 180.0) / 360.0, (90.0 - ll.y) / 180.0);
}

// ------------------------------------------------------------------ areas

@vertex
fn vs_area(@location(0) position: vec3<f32>, @location(1) paint: vec4<u32>) -> GroundOut {
    let p = project_local(position);
    let d = local_offset(position.xy);
    let rl = rl_vertex(p.rel, rl_mercator(d));
    var out: GroundOut;
    out.clip = p.clip;
    out.eye = rl.eye;
    out.sun = rl.sun;
    out.t = rl.t;
    out.c = rl.c;
    out.e = earth_place(d);
    out.paint = paint;
    out.wall = vec3<f32>(0.0);
    out.uv = vec2<f32>(0.0);
    out.corner_offset = vec2<f32>(0.0);
    out.path_length = 0.0;
    return out;
}

// A station's platform standing up (item R9, `layers::ground::rails`; `railGroundLayers`' extruded
// SolidPolygonLayer, `material: false`): an area whose sides face their own way. deck sets
// `rg_wall = geometry.normal` on a side's vertices (`IS_SIDE_VERTEX`) and leaves it 0 on the top;
// here a side's normal is level (east, north) and the top's up, so its level part is `rg_wall`.
@vertex
fn vs_platform(@location(0) position: vec3<f32>, @location(1) paint: vec4<u32>, @location(2) normal: vec3<f32>) -> GroundOut {
    let p = project_local(position);
    let d = local_offset(position.xy);
    let rl = rl_vertex(p.rel, rl_mercator(d));
    var out: GroundOut;
    out.clip = p.clip;
    out.eye = rl.eye;
    out.sun = rl.sun;
    out.t = rl.t;
    out.c = rl.c;
    out.e = earth_place(d);
    out.paint = paint;
    out.wall = vec3<f32>(normal.xy, 0.0);
    out.uv = vec2<f32>(0.0);
    out.corner_offset = vec2<f32>(0.0);
    out.path_length = 0.0;
    return out;
}

// ------------------------------------------------------------------ lines

const EPSILON: f32 = 0.001;
// `scene::geom::path::PathSegment::types`
const START_CAP: u32 = 1u;
const END_CAP: u32 = 2u;

// `scene::geom::path::PathSegment`, as far as the ground reads it (no dashes)
struct Segment {
    @location(1) left: vec3<f32>,
    @location(2) start: vec3<f32>,
    @location(3) end: vec3<f32>,
    @location(4) right: vec3<f32>,
    @location(5) width: f32,
    @location(6) paint: vec4<u32>,
    @location(7) types: u32,
}

// -(float(flag) * 2. - 1.)
fn flip_if_true(flag: bool) -> f32 {
    return select(1.0, -1.0, flag);
}

// deck 9.4's `path-layer-vertex.glsl` (not billboarded, no dashes, no antialiasing) with the
// ground's props: `widthUnits: "meters"`, `widthScale` 1, `widthMinPixels` and `widthMaxPixels`
// from the uniform, `jointRounded` and `capRounded`; worked out as `shaders/path.wgsl` does (the
// offsets in the plane of east and north at the point).
@vertex
fn vs_line(@location(0) corner: vec2<f32>, segment: Segment) -> GroundOut {
    // corner.x: 0 at the segment's start, 1 at its end; corner.y: −1 or 1 the side, 0 the joint
    let is_end = corner.x > 0.0;
    let side_of_path = corner.y;
    let is_joint = select(0.0, 1.0, side_of_path == 0.0);
    let prev_local = select(segment.left, segment.start, is_end);
    let curr_local = select(segment.start, segment.end, is_end);
    let next_local = select(segment.end, segment.right, is_end);

    // project_size_to_pixel(width, UNIT.meters): the view centre's scale
    let width_pixels = clamp(segment.width * globals.centre.z * globals.pixels_per_unit, ground.line.x, ground.line.y) / 2.0;

    let prev_position = project_local(prev_local).rel;
    let curr_position = project_local(curr_local).rel;
    let next_position = project_local(next_local).rel;
    // project_pixel_size
    let width = vec2<f32>(width_pixels / globals.pixels_per_unit);

    // getLineJoinOffset
    var delta_a3 = curr_position - prev_position;
    var delta_b3 = next_position - curr_position;
    let rotation = local_basis(curr_local.xy);
    delta_a3 = delta_a3 * rotation;
    delta_b3 = delta_b3 * rotation;
    let delta_a = delta_a3.xy / width;
    let delta_b = delta_b3.xy / width;
    let len_a = length(delta_a);
    let len_b = length(delta_b);
    let dir_a = select(vec2<f32>(0.0, 0.0), normalize(delta_a), len_a > 0.0);
    let dir_b = select(vec2<f32>(0.0, 0.0), normalize(delta_b), len_b > 0.0);
    let perp_a = vec2<f32>(-dir_a.y, dir_a.x);
    let perp_b = vec2<f32>(-dir_b.y, dir_b.x);
    var tangent = dir_a + dir_b;
    tangent = select(perp_a, normalize(tangent), length(tangent) > 0.0);
    let miter_vec = vec2<f32>(-tangent.y, tangent.x);
    let dir = select(dir_b, dir_a, is_end);
    let perp = select(perp_b, perp_a, is_end);
    let l = select(len_b, len_a, is_end);

    let sin_half_a = abs(dot(miter_vec, perp));
    let cos_half_a = abs(dot(dir_a, miter_vec));
    let turn_direction = flip_if_true(dir_a.x * dir_b.y >= dir_a.y * dir_b.x);
    let corner_position = side_of_path * turn_direction;

    var miter_size = 1.0 / max(sin_half_a, EPSILON);
    miter_size = mix(
        min(miter_size, max(len_a, len_b) / max(cos_half_a, EPSILON)),
        miter_size,
        step(0.0, corner_position),
    );
    var offset_vec = mix(miter_vec * miter_size, perp, step(0.5, corner_position))
        * (side_of_path + is_joint * turn_direction);

    let is_start_cap = len_a == 0.0 || (!is_end && (segment.types & START_CAP) != 0u);
    let is_end_cap = len_b == 0.0 || (is_end && (segment.types & END_CAP) != 0u);
    if (is_start_cap || is_end_cap) {
        // capType 1 (round)
        offset_vec = mix(perp * side_of_path, dir * 4.0 * flip_if_true(is_start_cap), is_joint);
    }

    let offset_from_start_of_path = offset_vec + delta_a * select(0.0, 1.0, is_end);
    // the vertex's offset east and north (common units), and in the render frame
    let side = offset_vec * width;
    let rel = curr_position + rotation * vec3<f32>(side, 0.0);

    // the light's terms where the vertex is (on the globe: its middle's place, out across the
    // line's width; on the flat map: its own place)
    let d = local_offset(curr_local.xy);
    var m = rl_mercator(d) + rl_line_side(d, side / globals.centre.z);
    if (globals.mode == MODE_FLAT) {
        m = rel.xy;
    }
    let rl = rl_vertex(rel, m);

    var out: GroundOut;
    out.clip = globals.centre_cam * vec4<f32>(rel, 1.0);
    out.eye = rl.eye;
    out.sun = rl.sun;
    out.t = rl.t;
    out.c = rl.c;
    out.e = earth_place(d);
    out.paint = segment.paint;
    out.wall = vec3<f32>(0.0);
    // vPathPosition (no dashes): across −1 to 1, along from the segment's start in half widths
    out.uv = vec2<f32>(dot(offset_from_start_of_path, perp), dot(offset_from_start_of_path, dir));
    out.corner_offset = offset_vec;
    out.path_length = l;
    return out;
}

// ------------------------------------------------------------------ the surfaces

// a layer's mean colour (the texture's last level)
fn mean_colour(layer: f32) -> vec3<f32> {
    return textureSampleLevel(ground_color, repeat_sampler, vec2<f32>(0.5, 0.5), i32(layer), 16.0).rgb;
}

// `fs:DECKGL_FILTER_COLOR`: the colour of the ground of `g`, an area's or a line's (`line`, GLSL's
// `RL_LINE`).
fn ground_colour(g: GroundOut, line: bool) -> vec4<f32> {
    // what the GLSL differentiates, in uniform control flow
    let dtdx = dpdx(g.t);
    let dtdy = dpdy(g.t);
    let cells_t = rl_cells(g.t);
    let cells_c = rl_cells(g.c);
    let duvdx = dpdx(g.uv);
    let duvdy = dpdy(g.uv);
    let earth = textureSample(earth_color, globe_sampler, g.e);
    let night = textureSample(earth_night, globe_sampler, g.e);

    let id = g.paint.w;
    if (id >= SURFACE_COUNT) {
        return vec4<f32>(0.0);
    }
    let rl = RlVary(g.eye, g.sun, g.t, g.c);
    let row = ground.surfaces[2u * id];
    let more = ground.surfaces[2u * id + 1u];
    let paint = vec3<f32>(g.paint.xyz) / 255.0;
    var tint = rl_linear(paint);
    var layer = row.x;
    let v = normalize(rl.eye);
    var n = UP;
    var albedo: vec3<f32>;
    var rough = row.z;
    var glint = 1.0;
    // texture coordinates: fixed to the world, whole repeats to the period; or along a line, across
    // its width (and their derivatives)
    var uv: vec2<f32>;
    var gx: vec2<f32>;
    var gy: vec2<f32>;
    if (row.y > 0.0) {
        let k = max(1.0, floor(rlight.period_metres / row.y + 0.5));
        uv = g.t * k;
        gx = dtdx * k;
        gy = dtdy * k;
    } else {
        uv = vec2<f32>(g.uv.x * 0.5 + 0.5, g.uv.y * 0.5);
        gx = duvdx * 0.5;
        gy = duvdy * 0.5;
    }
    if (layer < 0.0) {
        // water: Streets GL's ripples, two of them drifting across each other, under a mirror of
        // the sky
        let k = max(1.0, floor(rlight.period_metres / 18.0 + 0.5));
        let w = g.t * k;
        let n1 = textureSampleGrad(water_normal, repeat_sampler, w + vec2<f32>(rlight.time * 0.011, rlight.time * 0.007), dtdx * k, dtdy * k).xyz * 2.0 - 1.0;
        let n2 = textureSampleGrad(water_normal, repeat_sampler, w * 1.7 - vec2<f32>(rlight.time * 0.008, -rlight.time * 0.012), dtdx * (k * 1.7), dtdy * (k * 1.7)).xyz * 2.0 - 1.0;
        n = normalize(vec3<f32>(n1.xy + n2.xy, n1.z * n2.z) * vec3<f32>(0.6, 0.6, 1.0));
        albedo = vec3<f32>(0.012, 0.025, 0.035) * tint * 2.0;
        // ripples too small to see from up here still break up the sun's reflection: a soft
        // sheen, not a mirror's spot
        let unseen = smoothstep(0.04, 0.5, k * cells_t);
        rough = mix(rough, 0.25, unseen);
        glint = 1.0 - 0.8 * unseen;
    } else {
        // Fields: the land round points scattered over it (40 to the clouds' period), so each
        // field has straight edges and its own shape, size and lie; to each its crop (one of
        // Streets GL's three) and its rows turned its own way; the crops' colours drawn closer
        // together the smaller the fields look, and fields too small to see take the crops' mean.
        // A field mapped as one (Streets GL's tiles) has its crop, rows and shade in its colour.
        var c3 = vec3<f32>(0.0);
        var crops = 0.0;
        var even = 0.0;
        var shade = 1.0;
        let field = id == FARMLAND || id == FIELD;
        if (field) {
            let p = g.c * 40.0;
            var seed: f32;
            var a: f32;
            if (id == FIELD) {
                seed = paint.b;
                layer += floor(paint.g * 2.999);
                a = 3.14159 * paint.r;
                tint = vec3<f32>(1.0);
            } else {
                let cell = floor(p);
                var best = 1e9;
                var near = cell;
                for (var y = -1; y <= 1; y++) {
                    for (var x = -1; x <= 1; x++) {
                        let o = cell + vec2<f32>(f32(x), f32(y));
                        let q = o + 0.1 + 0.8 * vec2<f32>(rl_hash(o, 40.0), rl_hash(o + 17.0, 40.0));
                        let dist = dot(p - q, p - q);
                        if (dist < best) {
                            best = dist;
                            near = o;
                        }
                    }
                }
                seed = rl_hash(near + 29.0, 40.0);
                layer += floor(seed * 2.999);
                a = 6.2832 * fract(seed * 7.31);
            }
            let turn = mat2x2<f32>(vec2<f32>(cos(a), sin(a)), vec2<f32>(-sin(a), cos(a)));
            uv = turn * uv;
            gx = turn * gx;
            gy = turn * gy;
            let first = ground.surfaces[2u * FARMLAND].x;
            c3 = (mean_colour(first) + mean_colour(first + 1.0) + mean_colour(first + 2.0)) / 3.0;
            even = mix(0.5, 0.85, smoothstep(0.004, 0.04, 40.0 * cells_c));
            shade = 0.93 + 0.14 * fract(seed * 3.7);
            crops = smoothstep(0.35, 0.9, 3.0 * cells_t);
        }
        // (the images are stored top row first: v runs up them)
        let at = vec2<f32>(uv.x, -uv.y);
        let at_dx = vec2<f32>(gx.x, -gx.y);
        let at_dy = vec2<f32>(gy.x, -gy.y);
        var c = textureSampleGrad(ground_color, repeat_sampler, at, i32(layer), at_dx, at_dy);
        if (field) {
            let own = mean_colour(layer);
            var rgb = c.rgb * mix(vec3<f32>(1.0), c3 / max(own, vec3<f32>(1e-3)), even) * shade;
            rgb = mix(rgb, c3, crops);
            c = vec4<f32>(rgb, c.a);
        }
        let nm = textureSampleGrad(ground_normal, repeat_sampler, at, i32(layer), at_dx, at_dy).xyz * 2.0 - 1.0;
        n = select(normalize(vec3<f32>(nm.xy, max(nm.z, 0.3))), normalize(g.wall), dot(g.wall, g.wall) > 0.0);
        // a little unevenness over tens of metres, so the repeats don't show (gone where it would
        // only speckle)
        let grain = 1.0 - smoothstep(0.25, 1.0, 32.0 * cells_t);
        albedo = c.rgb * tint * (1.0 + 0.28 * grain * (rl_noise(g.t, 32.0) - 0.5));
        // a road or path: its plain colour, flat
        if (more.y >= 0.0) {
            albedo = more.yzw * tint;
            n = UP;
        }
        // Zoomed out the map's data hardly says what the land is: there its colour is the Earth's
        // own, from the picture of it, taking over as a pixel grows past 100 m or so. Closer in,
        // land nothing is mapped on keeps the Earth's colour where that is dry (sand, rock), in the
        // terrain's detail; where it is green, Streets GL's biome green.
        let far = earth.a * smoothstep(120.0, 1200.0, cells_t * rlight.period_metres);
        if (id == 0u) {
            // (the detail's own unevenness, around its tinted mean, on the Earth's colour)
            let mean = mean_colour(layer) * tint;
            let dry = earth.a * smoothstep(0.03, 0.15, (earth.r - earth.g) / (earth.r + earth.g + 1e-4));
            albedo = mix(albedo, earth.rgb * dot(albedo, LUM) / max(dot(mean, LUM), 1e-4), dry);
        }
        if (id != ICE) {
            albedo = mix(albedo, earth.rgb, far);
        }
        // snow lies on the land, slush on the roads; rain darkens what is paved and makes it shine
        // (the weather where the camera is: not the whole world's, zoomed out)
        let snow = select(rlight.snow * row.w * (1.0 - far), 1.0, id == ICE);
        albedo = mix(albedo, vec3<f32>(0.82, 0.84, 0.88) * mix(0.75, 1.0, row.w), snow);
        let wet = rlight.wet * mix(0.35, 1.0, more.x) * (1.0 - far);
        albedo *= 1.0 - 0.45 * wet;
        rough = mix(rough, 0.12, wet * more.x * (1.0 - rlight.snow));
        n = normalize(mix(n, UP, wet * 0.6));
    }
    var lit = rl_shade(rl, albedo, n, v, rough, 0.0, 1.0, rl_sun_visible(rl), glint);
    // from far over the globe, its towns' lights where the sun has set (NASA's Black Marble); not
    // on lines
    if (!line && rlight.globe > 0.0) {
        let dark = 1.0 - smoothstep(-0.12, 0.03, rl.sun.z);
        lit += rl_linear(night.rgb) * 0.55 * rlight.globe * dark;
    }
    return vec4<f32>(rl_display(rl_aerial(rl, lit)), 1.0);
}

@fragment
fn fs_area(g: GroundOut) -> @location(0) vec4<f32> {
    return ground_colour(g, false);
}

// path-layer-fragment.glsl (no antialiasing): round joints and caps cut to their circles, then the
// ground's colour (worked out first, for its derivatives).
@fragment
fn fs_line(g: GroundOut) -> @location(0) vec4<f32> {
    let colour = ground_colour(g, true);
    if ((g.uv.y < 0.0 || g.uv.y > g.path_length) && length(g.corner_offset) > 1.0) {
        discard;
    }
    return colour;
}
