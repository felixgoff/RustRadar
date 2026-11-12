//!include lib/rlight.wgsl
// overlay.wgsl: what the realistic map lays on its ground under each tile's buildings (item R5;
// doc 04 §7.4, §7.6, §12): `building-layer.ts`'s `-foot` and `-shadow` sublayers with
// `realistic.ts`'s `FootShadeExtension` and `HazeFadeExtension` (R:921-965). The Rust side is
// `layers::building_shadows`; both are drawn with the shadows' stencil rule (`DrawState::SHADOW`).
//
// Entry points:
// - `vs_shadow`/`fs_shadow`: the shadows, deck's SolidPolygonLayer as `fill.wgsl` draws it (filled,
//   not extruded, `material: false`: the colour as it is), its one colour (`getFillColor`, the
//   tile's alpha) from group 3, then what `HazeFadeExtension` injects at `fs:DECKGL_FILTER_COLOR`,
//   `color.a *= rl_clear();`.
// - `vs_foot`/`fs_foot`: the foot shade, deck 9.4's PathLayer (`path-layer-vertex.glsl`,
//   `path-layer-fragment.glsl`; not billboarded, no dashes, no antialiasing) as `path.wgsl` draws it,
//   with the `-foot` layer's props: widths in metres (`getWidth: FOOT_SHADE_M`), `widthScale` 1, no
//   pixel clamp, miter joints (limit 4), butt caps, opacity 1; then what `FootShadeExtension`
//   injects at `fs:DECKGL_FILTER_COLOR`:
//     float across = 1.0 - min(abs(geometry.uv.x), 1.0);
//     color.a *= across * rl_clear();
//   (`geometry.uv` is the path's `vPathPosition`: across the line −1 to 1).
//
// What differs from the GLSL (doc 04 §16.2): the bind groups. `rlight` is group 2 (lib/rlight.wgsl),
// so the shadows' colour is group 3 (deck's own uniforms have no group of their own); the foot
// shade's props are constants here (the layer has only the one set). `rl_clear` reads only
// `rl_eye`, so that is the one varying of the light handed on, as `hillshade.wgsl` does.

// ------------------------------------------------------------------ shadows

// `layers::building_shadows::ShadowUniform`: a tile's shadow colour, `[...shadowColor, alpha] / 255`.
struct TileShadow {
    color: vec4<f32>,
}

@group(3) @binding(0) var<uniform> tile_shadow: TileShadow;

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    // `rl_eye`: to the camera from here, metres east, north and up
    @location(0) eye: vec3<f32>,
}

// a shadow's corner on the ground: (u, v) tile units, `v` south (`positionFormat: "XY"`)
@vertex
fn vs_shadow(@location(0) position: vec2<f32>) -> ShadowOut {
    let local = vec3<f32>(position, 0.0);
    let p = project_local(local);
    let rl = rl_vertex(p.rel, rl_mercator(local_offset(position)));
    var out: ShadowOut;
    out.clip = p.clip;
    out.eye = rl.eye;
    return out;
}

@fragment
fn fs_shadow(in: ShadowOut) -> @location(0) vec4<f32> {
    // vColor = vec4(colors.rgb, colors.a * layer.opacity), opacity 1
    var color = tile_shadow.color;
    // `fs:DECKGL_FILTER_COLOR` (HAZE_FADE)
    let rl = RlVary(in.eye, vec3<f32>(0.0, 0.0, 1.0), vec2<f32>(0.0), vec2<f32>(0.0));
    color.a = color.a * rl_clear(rl);
    return color;
}

// ------------------------------------------------------------------ the foot shade

const EPSILON: f32 = 0.001;
// `scene::geom::path::PathSegment::types`
const START_CAP: u32 = 1u;
const END_CAP: u32 = 2u;
// the `-foot` PathLayer's props: deck's defaults (`jointRounded: false`, `capRounded: false`,
// `miterLimit: 4`)
const JOINT_TYPE: f32 = 0.0;
const CAP_TYPE: f32 = 0.0;
const MITER_LIMIT: f32 = 4.0;

// `scene::geom::path::PathSegment` (`pipelines::path::SEGMENT_LAYOUT`), as far as the foot shade
// reads it (no dashes)
struct Segment {
    @location(1) left: vec3<f32>,
    @location(2) start: vec3<f32>,
    @location(3) end: vec3<f32>,
    @location(4) right: vec3<f32>,
    @location(5) width: f32,
    @location(6) color: vec4<f32>,
    @location(10) types: u32,
}

struct FootOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    // vCornerOffset, vMiterLength, vPathPosition (`geometry.uv`), vPathLength, vJointType
    @location(1) corner_offset: vec2<f32>,
    @location(2) miter_length: f32,
    @location(3) path_position: vec2<f32>,
    @location(4) path_length: f32,
    @location(5) joint_type: f32,
    // `rl_eye`
    @location(6) eye: vec3<f32>,
}

// -(float(flag) * 2. - 1.)
fn flip_if_true(flag: bool) -> f32 {
    return select(1.0, -1.0, flag);
}

@vertex
fn vs_foot(@location(0) corner: vec2<f32>, segment: Segment) -> FootOut {
    // corner.x: 0 at the segment's start, 1 at its end; corner.y: −1 or 1 the side, 0 the joint
    let is_end = corner.x > 0.0;
    let side_of_path = corner.y;
    let is_joint = select(0.0, 1.0, side_of_path == 0.0);
    let prev_local = select(segment.left, segment.start, is_end);
    let curr_local = select(segment.start, segment.end, is_end);
    let next_local = select(segment.end, segment.right, is_end);

    // project_size_to_pixel(width * widthScale, UNIT.meters), clamped to [0, MAX_SAFE_INTEGER]: the
    // view centre's scale (deck's `project_size()` factor is 1 on the flat map above zoom 12)
    let width_pixels = max(segment.width * globals.centre.z * globals.pixels_per_unit, 0.0) / 2.0;

    let prev_position = project_local(prev_local).rel;
    let curr_position = project_local(curr_local).rel;
    let next_position = project_local(next_local).rel;
    // project_pixel_size
    let width = vec2<f32>(width_pixels / globals.pixels_per_unit);

    // getLineJoinOffset
    var delta_a3 = curr_position - prev_position;
    var delta_b3 = next_position - curr_position;
    // east, north, up at the point (identity on the flat map); `v * M` reads v in its columns
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
    let is_cap = is_start_cap || is_end_cap;
    var joint_type = JOINT_TYPE;
    if (is_cap) {
        offset_vec = mix(perp * side_of_path, dir * CAP_TYPE * 4.0 * flip_if_true(is_start_cap), is_joint);
        joint_type = CAP_TYPE;
    }

    var miter_length = dot(offset_vec, miter_vec * turn_direction);
    miter_length = select(miter_length, is_joint, is_cap);
    let offset_from_start_of_path = offset_vec + delta_a * select(0.0, 1.0, is_end);
    let rel = curr_position + rotation * vec3<f32>(offset_vec * width, 0.0);
    // the light's terms where the vertex is (`geometry.position = currPosition + offset`)
    let rl = rl_vertex(rel, rel.xy);

    var out: FootOut;
    out.clip = globals.centre_cam * vec4<f32>(rel, 1.0);
    // vColor = vec4(instanceColors.rgb, instanceColors.a * layer.opacity), opacity 1
    out.color = segment.color;
    out.corner_offset = offset_vec;
    out.miter_length = miter_length;
    out.path_position = vec2<f32>(dot(offset_from_start_of_path, perp), dot(offset_from_start_of_path, dir));
    out.path_length = l;
    out.joint_type = joint_type;
    out.eye = rl.eye;
    return out;
}

@fragment
fn fs_foot(in: FootOut) -> @location(0) vec4<f32> {
    // path-layer-fragment.glsl: cut the corners to the round joint or cap, or to the miter limit
    if (in.path_position.y < 0.0 || in.path_position.y > in.path_length) {
        if (in.joint_type > 0.5 && length(in.corner_offset) > 1.0) {
            discard;
        }
        if (in.joint_type < 0.5 && in.miter_length > MITER_LIMIT + 1.0) {
            discard;
        }
    }
    var color = in.color;
    // `fs:DECKGL_FILTER_COLOR` (FOOT_SHADE): darkest on the line, nothing at its edges, and into
    // the distance's air
    let across = 1.0 - min(abs(in.path_position.x), 1.0);
    let rl = RlVary(in.eye, vec3<f32>(0.0, 0.0, 1.0), vec2<f32>(0.0), vec2<f32>(0.0));
    color.a = color.a * across * rl_clear(rl);
    return color;
}
