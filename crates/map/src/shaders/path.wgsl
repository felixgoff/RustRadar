//!include lib/project.wgsl
// path.wgsl: deck.gl 9.4's PathLayer (`path-layer-vertex.glsl`, `path-layer-fragment.glsl`) as the
// app draws it (`billboard: false`, no antialiasing), with PathStyleExtension's dashes
// (`path-style/shaders.glsl`: `dashUnits: 'widths'`, per segment or along the whole path
// (`highPrecisionDash`), plain or justified). The instance layout is `scene::geom::path::PathSegment`;
// `pipelines::path` holds the Rust side.
//
// deck's non-billboard path, in this renderer's spaces: the segment's points are projected to the
// render frame (`project_local(..).rel`, common units around the view centre, the globe turned by
// the centre's latitude and longitude); the joint and cap offsets are worked out in the plane of
// east and north at the current point (deck's `project_needs_rotation` frame on the globe, which
// is the same plane turned half a turn, so the offsets come out the same; the plain xy plane on the
// flat map), in units of half the line's width; and the vertex goes out at `curr + offset`.
// Widths are deck's: CSS pixels turned into common units at the view centre, so a line lies flat
// on the ground and foreshortens with pitch across the view.
//
// Operation order follows the GLSL, `mix`, `step` and `select` included. The varyings deck keeps
// for dashes only (`vPathLength`, `vPathPosition.y` stretched to the 3D length) are always worked
// out: without dashes they only feed the corner tests, which compare them with each other.

// deck's `path` uniforms (`path-layer-uniforms.js`), PathStyleExtension's `pathStyle` and the
// layer's opacity: `pipelines::path::StyleUniform`.
struct PathStyle {
    width_scale: f32,
    width_min_pixels: f32,
    width_max_pixels: f32,
    // 0 miter, 1 round
    joint_type: f32,
    // 0 butt, 1 round
    cap_type: f32,
    miter_limit: f32,
    // UNIT_COMMON, UNIT_METERS or UNIT_PIXELS
    width_units: u32,
    opacity: f32,
    // DASH_NONE, DASH_SEGMENT or DASH_PATH
    dash_mode: u32,
    // `dashJustified`: 0 or 1 (deck's `dashAlignMode`)
    dash_align_mode: f32,
    pad: vec2<f32>,
}

@group(2) @binding(0) var<uniform> style: PathStyle;

const EPSILON: f32 = 0.001;
// deck's `UNIT`
const UNIT_COMMON: u32 = 0u;
const UNIT_METERS: u32 = 1u;
const UNIT_PIXELS: u32 = 2u;
// no PathStyleExtension; `dash: true` (dashes restart at each segment); `highPrecisionDash: true`
const DASH_NONE: u32 = 0u;
const DASH_SEGMENT: u32 = 1u;
const DASH_PATH: u32 = 2u;
// `PathSegment::types`
const START_CAP: u32 = 1u;
const END_CAP: u32 = 2u;
// deck's `geometry` module
const SMOOTH_EDGE_RADIUS: f32 = 0.5;

struct Segment {
    @location(1) left: vec3<f32>,
    @location(2) start: vec3<f32>,
    @location(3) end: vec3<f32>,
    @location(4) right: vec3<f32>,
    @location(5) width: f32,
    @location(6) color: vec4<f32>,
    @location(7) dash_array: vec2<f32>,
    @location(8) dash_globe: vec2<f32>,
    @location(9) dash_flat: vec2<f32>,
    @location(10) types: u32,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) corner_offset: vec2<f32>,
    @location(2) miter_length: f32,
    @location(3) path_position: vec2<f32>,
    @location(4) path_length: f32,
    @location(5) joint_type: f32,
    @location(6) dash_array: vec2<f32>,
    @location(7) dash_offset: f32,
    @location(8) dash_path_length: f32,
}

// -(float(flag) * 2. - 1.)
fn flip_if_true(flag: bool) -> f32 {
    return select(1.0, -1.0, flag);
}

// GLSL's `mod`
fn glsl_mod(x: f32, y: f32) -> f32 {
    return x - y * floor(x / y);
}

fn smoothedge(edge: f32, x: f32) -> f32 {
    return smoothstep(edge - SMOOTH_EDGE_RADIUS, edge + SMOOTH_EDGE_RADIUS, x);
}

// `project_size_to_pixel(size, unit)`; deck's `project_size()` factor is 1 on the globe and on the
// flat map above zoom 12 (its auto-offset mode): metres use the view centre's scale.
fn project_size_to_pixel(size: f32, unit: u32) -> f32 {
    if (unit == UNIT_METERS) {
        return size * globals.centre.z * globals.pixels_per_unit;
    }
    if (unit == UNIT_COMMON) {
        return size * globals.pixels_per_unit;
    }
    return size;
}

@vertex
fn vs(@location(0) corner: vec2<f32>, segment: Segment) -> Varyings {
    // corner.x: 0 at the segment's start, 1 at its end; corner.y: −1 or 1 the side, 0 the joint
    let is_end = corner.x > 0.0;
    let side_of_path = corner.y;
    let is_joint = select(0.0, 1.0, side_of_path == 0.0);
    let prev_local = select(segment.left, segment.start, is_end);
    let curr_local = select(segment.start, segment.end, is_end);
    let next_local = select(segment.end, segment.right, is_end);

    let width_pixels = clamp(
        project_size_to_pixel(segment.width * style.width_scale, style.width_units),
        style.width_min_pixels,
        style.width_max_pixels,
    ) / 2.0;

    let curr_projected = project_local(curr_local);
    let prev_position = project_local(prev_local).rel;
    let curr_position = curr_projected.rel;
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

    // DASH_ENABLED: lengths along the segment in 3D
    let curr_delta3 = select(delta_b3, delta_a3, is_end);
    let curr_length_2d = length(curr_delta3.xy);
    var arc_length_ratio = 1.0;
    var path_length = l;
    if (curr_length_2d > 0.0) {
        arc_length_ratio = length(curr_delta3) / curr_length_2d;
        path_length = l * arc_length_ratio;
    }

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
    var joint_type = style.joint_type;
    if (is_cap) {
        offset_vec = mix(perp * side_of_path, dir * style.cap_type * 4.0 * flip_if_true(is_start_cap), is_joint);
        joint_type = style.cap_type;
    }

    var miter_length = dot(offset_vec, miter_vec * turn_direction);
    miter_length = select(miter_length, is_joint, is_cap);
    let offset_from_start_of_path = offset_vec + delta_a * select(0.0, 1.0, is_end);
    let path_position = vec2<f32>(
        dot(offset_from_start_of_path, perp),
        dot(offset_from_start_of_path, dir) * arc_length_ratio,
    );
    let offset = rotation * vec3<f32>(offset_vec * width, 0.0);

    // PathStyleExtension's `vs:#main-end` (dash units: widths)
    let dash_width_pixels = width.x * globals.pixels_per_unit;
    let stroke_half_width_pixels = width.x * globals.pixels_per_unit;
    let dash_array = segment.dash_array * (stroke_half_width_pixels / dash_width_pixels);
    var dash_offset = 0.0;
    var dash_path_length = 0.0;
    if (style.dash_mode == DASH_PATH) {
        let metrics = select(segment.dash_globe, segment.dash_flat, globals.mode == MODE_FLAT);
        let dash_offset_and_length = (metrics * globals.pixels_per_unit) / dash_width_pixels;
        dash_path_length = dash_offset_and_length.y;
        let dash_unit_length = dash_array.x + dash_array.y;
        var dash_period = dash_unit_length;
        if (style.dash_align_mode != 0.0) {
            dash_period = dash_path_length / max(round(dash_path_length / max(dash_unit_length, 0.0001)), 1.0);
        }
        if (dash_period > 0.0) {
            dash_offset = glsl_mod(dash_offset_and_length.x, dash_period);
        }
    }

    var out: Varyings;
    out.position = globals.centre_cam * vec4<f32>(curr_position + offset, 1.0);
    out.color = vec4<f32>(segment.color.rgb, segment.color.a * style.opacity);
    out.corner_offset = offset_vec;
    out.miter_length = miter_length;
    out.path_position = path_position;
    out.path_length = path_length;
    out.joint_type = joint_type;
    out.dash_array = dash_array;
    out.dash_offset = dash_offset;
    out.dash_path_length = dash_path_length;
    return out;
}

// PathStyleExtension's `dashPatternIntegral`: how much of [0, position] is solid.
fn dash_pattern_integral(position: f32, solid_length: f32, unit_length: f32) -> f32 {
    return floor(position / unit_length) * solid_length + min(glsl_mod(position, unit_length), solid_length);
}

// `dashPatternCoverage`: the solid share of a box `filter_width` wide around `position`.
fn dash_pattern_coverage(position: f32, solid_length: f32, unit_length: f32, filter_width: f32) -> f32 {
    let half_filter = 0.5 * filter_width;
    let reduced_position = glsl_mod(position, unit_length);
    return (dash_pattern_integral(reduced_position + half_filter, solid_length, unit_length)
        - dash_pattern_integral(reduced_position - half_filter, solid_length, unit_length)) / filter_width;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    // PathStyleExtension's `fs:#main-start`. What it differentiates is worked out first, outside
    // any branch on a varying (WGSL wants derivatives in uniform control flow); the values are only
    // used where deck uses them.
    var solid_length = in.dash_array.x;
    let gap_length = in.dash_array.y;
    var unit_length = solid_length + gap_length;
    var offset = in.dash_offset;
    if (style.dash_align_mode != 0.0) {
        let total = select(in.path_length, in.dash_path_length, style.dash_mode == DASH_PATH);
        unit_length = total / max(round(total / unit_length), 1.0);
        solid_length = min(solid_length, unit_length);
        offset = solid_length / 2.0;
        if (style.dash_mode == DASH_PATH) {
            offset += in.dash_offset;
        }
    }
    let along_path = in.path_position.y + offset;
    let unit_offset = glsl_mod(along_path, unit_length);
    let filter_width = max(fwidth(along_path), 0.0001);
    let distance_along_gap = min(unit_offset - solid_length, unit_length - unit_offset);
    let distance_to_end = length(vec2<f32>(max(distance_along_gap, 0.0), in.path_position.x));
    let cap_edge_pixels = (1.0 - distance_to_end) / max(fwidth(distance_to_end), 1e-6);

    var dash_coverage = 1.0;
    var discard_dash = false;
    if (style.dash_mode != DASH_NONE && in.dash_array.x + gap_length > 0.0 && gap_length > 0.0) {
        if (style.cap_type <= 0.5) {
            dash_coverage = dash_pattern_coverage(along_path, solid_length, unit_length, filter_width);
        } else {
            if (distance_along_gap > 0.0) {
                dash_coverage = smoothedge(0.0, cap_edge_pixels);
            }
            let sub_pixel_blend = smoothstep(unit_length, 2.0 * unit_length, filter_width);
            let bounded_solid_length = min(solid_length, unit_length);
            let effective_gap = max(unit_length - bounded_solid_length, 0.0);
            let cap_span = 2.0 * sqrt(max(1.0 - in.path_position.x * in.path_position.x, 0.0));
            let rounded_duty_cycle = clamp((bounded_solid_length + min(effective_gap, cap_span)) / unit_length, 0.0, 1.0);
            dash_coverage = mix(dash_coverage, rounded_duty_cycle, sub_pixel_blend);
        }
        dash_coverage = clamp(dash_coverage, 0.0, 1.0);
        discard_dash = dash_coverage <= 0.0;
    }

    // path-layer-fragment.glsl: cut the corners to the round joint or cap, or to the miter limit
    if (in.path_position.y < 0.0 || in.path_position.y > in.path_length) {
        if (in.joint_type > 0.5 && length(in.corner_offset) > 1.0) {
            discard;
        }
        if (in.joint_type < 0.5 && in.miter_length > style.miter_limit + 1.0) {
            discard;
        }
    }
    // `fs:#main-end`
    if (discard_dash) {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * dash_coverage);
}
