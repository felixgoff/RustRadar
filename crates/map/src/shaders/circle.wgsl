//!include lib/project.wgsl
// circle.wgsl: deck.gl 9.4's ScatterplotLayer (`scatterplot-layer-vertex.glsl`,
// `scatterplot-layer-fragment.glsl`, the WebGL path the app runs on). One instance per circle, the
// layout of `pipelines::circle::Instance`; `pipelines::circle` holds the Rust side.
//
// deck's vertex shader in this renderer's spaces. A circle's centre is a common offset (worked out
// on the CPU in f64, `ViewFrame::common_offset`). Its radius and line width become CSS pixels at the
// view centre's scale (`project_size_to_pixel`), clamped to the layer's min and max; the outer
// radius grows by half the line width when stroked, and by deck's smoothing edge of half a pixel
// when antialiased. The quad is deck's unit square (a triangle strip of four corners, made from the
// vertex index) scaled to that radius, then deck's two cases:
// - billboard: the corners are added in clip space as `pixels / viewport · 2 · focal_w`, deck's
//   `project_pixel_size_to_clipspace`, whose `focalDistance` is the clip w of the view centre
//   (`globals.centre_cam` takes the render frame's origin, the view centre, to clip space): a circle
//   at the view centre's depth has exactly its radius in pixels, one nearer the camera or further
//   from it is larger or smaller by focal_w / w, as in deck; it stays round on the screen whatever
//   the pitch and bearing;
// - ground plane (`billboard: false`): the corners become common units at the view centre's scale
//   (deck's `project_pixel_size`, pixels / `scale`) in the plane of the ground at the circle, turned
//   on the globe by deck's `project_get_orientation_matrix` of the circle's up direction (sent per
//   instance, worked out in f64), so the circle foreshortens with the ground.
// The fragment shader is deck's: the distance from the centre in those pixels, `smoothedge` over
// half a pixel each way (or `step` without antialiasing) for the outer edge and the line's inner
// edge, fill and line mixed by the line's coverage, the line alone with its coverage as alpha when
// not filled.

// `pipelines::circle::StyleUniform`: deck's `scatterplot` uniforms and the layer's opacity.
struct CircleStyle {
    // `radiusScale`, `radiusMinPixels`, `radiusMaxPixels`
    radius_scale: f32,
    radius_min_pixels: f32,
    radius_max_pixels: f32,
    // `lineWidthScale`, `lineWidthMinPixels`, `lineWidthMaxPixels`
    line_width_scale: f32,
    line_width_min_pixels: f32,
    line_width_max_pixels: f32,
    // `stroked`, `filled`: 0 or 1
    stroked: f32,
    filled: f32,
    // `antialiasing`, `billboard`: 0 or 1
    antialiasing: u32,
    billboard: u32,
    // `radiusUnits`, `lineWidthUnits`: UNIT_COMMON, UNIT_METERS or UNIT_PIXELS
    radius_units: u32,
    line_width_units: u32,
    // `layer.opacity`
    opacity: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
}

@group(2) @binding(0) var<uniform> style: CircleStyle;

// deck's `UNIT`
const UNIT_COMMON: u32 = 0u;
const UNIT_METERS: u32 = 1u;
const UNIT_PIXELS: u32 = 2u;

// deck's `SMOOTH_EDGE_RADIUS` (shaderlib/misc/geometry), CSS pixels
const SMOOTH_EDGE_RADIUS: f32 = 0.5;

struct Circle {
    // the common offset of the centre (`instancePositions`)
    @location(0) position: vec3<f32>,
    // the up direction at the centre in common space (the globe's normal there; (0, 0, 1) on the
    // flat map)
    @location(1) up: vec3<f32>,
    // `instanceRadius`
    @location(2) radius: f32,
    // `instanceLineWidths`
    @location(3) line_width: f32,
    // `instanceFillColors`, `instanceLineColors`
    @location(4) fill_color: vec4<f32>,
    @location(5) line_color: vec4<f32>,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) fill_color: vec4<f32>,
    @location(1) line_color: vec4<f32>,
    @location(2) unit_position: vec2<f32>,
    @location(3) inner_unit_radius: f32,
    @location(4) outer_radius_pixels: f32,
}

// `smoothedge`
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

// `project_pixel_size_to_clipspace`: CSS pixels to a clip-space offset at the view centre's depth,
// `focalDistance` being the clip w of the view centre.
fn project_pixel_size_to_clipspace(pixels: vec2<f32>) -> vec2<f32> {
    let focal_w = (globals.centre_cam * vec4<f32>(0.0, 0.0, 0.0, 1.0)).w;
    let offset = pixels / globals.viewport.xy * 2.0;
    return offset * focal_w;
}

// `project_get_orientation_matrix(up)`.
fn orientation_matrix(up: vec3<f32>) -> mat3x3<f32> {
    let uz = normalize(up);
    var ux = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(uz.z) != 1.0) {
        ux = normalize(vec3<f32>(uz.y, -uz.x, 0.0));
    }
    let uy = cross(uz, ux);
    return mat3x3<f32>(ux, uy, uz);
}

@vertex
fn vs(@builtin(vertex_index) index: u32, circle: Circle) -> Varyings {
    var out: Varyings;
    // deck's quad: (−1, −1), (1, −1), (−1, 1), (1, 1) as a triangle strip
    let positions = vec2<f32>(f32(index & 1u) * 2.0 - 1.0, f32(index >> 1u) * 2.0 - 1.0);
    var outer_radius_pixels = clamp(
        project_size_to_pixel(style.radius_scale * circle.radius, style.radius_units),
        style.radius_min_pixels,
        style.radius_max_pixels,
    );
    let line_width_pixels = clamp(
        project_size_to_pixel(style.line_width_scale * circle.line_width, style.line_width_units),
        style.line_width_min_pixels,
        style.line_width_max_pixels,
    );
    outer_radius_pixels += style.stroked * line_width_pixels / 2.0;
    var edge_padding = 1.0;
    if (style.antialiasing != 0u) {
        edge_padding = (outer_radius_pixels + SMOOTH_EDGE_RADIUS) / outer_radius_pixels;
    }
    let unit_position = edge_padding * positions;
    out.unit_position = unit_position;
    out.inner_unit_radius = 1.0 - style.stroked * line_width_pixels / outer_radius_pixels;
    out.outer_radius_pixels = outer_radius_pixels;
    if (style.billboard != 0u) {
        let clip = project_common(circle.position);
        let offset = edge_padding * positions * outer_radius_pixels;
        out.position = vec4<f32>(clip.xy + project_pixel_size_to_clipspace(offset), clip.zw);
    } else {
        var offset = vec3<f32>(edge_padding * positions * (outer_radius_pixels / globals.pixels_per_unit), 0.0);
        // `project_needs_rotation`: the offset is east, north, up; on the globe it is turned so the
        // ground lies along the globe's surface
        if (globals.mode == MODE_GLOBE) {
            offset = orientation_matrix(circle.up) * offset;
        }
        out.position = project_common(circle.position + offset);
    }
    out.fill_color = vec4<f32>(circle.fill_color.rgb, circle.fill_color.a * style.opacity);
    out.line_color = vec4<f32>(circle.line_color.rgb, circle.line_color.a * style.opacity);
    return out;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    let dist_to_center = length(in.unit_position) * in.outer_radius_pixels;
    var in_circle: f32;
    if (style.antialiasing != 0u) {
        in_circle = smoothedge(dist_to_center, in.outer_radius_pixels);
    } else {
        in_circle = step(dist_to_center, in.outer_radius_pixels);
    }
    if (in_circle == 0.0) {
        discard;
    }
    var frag_color: vec4<f32>;
    if (style.stroked > 0.5) {
        var is_line: f32;
        if (style.antialiasing != 0u) {
            is_line = smoothedge(in.inner_unit_radius * in.outer_radius_pixels, dist_to_center);
        } else {
            is_line = step(in.inner_unit_radius * in.outer_radius_pixels, dist_to_center);
        }
        if (style.filled > 0.5) {
            frag_color = mix(in.fill_color, in.line_color, is_line);
        } else {
            if (is_line == 0.0) {
                discard;
            }
            frag_color = vec4<f32>(in.line_color.rgb, in.line_color.a * is_line);
        }
    } else if (style.filled < 0.5) {
        discard;
    } else {
        frag_color = in.fill_color;
    }
    frag_color.a *= in_circle;
    return frag_color;
}
