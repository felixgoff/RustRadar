//!include lib/project.wgsl
// text.wgsl: deck.gl 9.4's TextLayer: its characters (MultiIconLayer: `multi-icon-layer-vertex.glsl`
// and `multi-icon-layer-fragment.glsl` with the `sdf` uniforms, the WebGL path the app runs on) and
// its background box (TextBackgroundLayer: `text-background-layer-vertex.glsl`, `-fragment.glsl`).
// `pipelines::text` holds the Rust side; `scene::text` lays the glyphs out.
//
// Characters. One instance per glyph, the layout of `pipelines::text::Glyph`: the label's common
// offset (worked out on the CPU in f64), its size, the glyph's atlas frame and its offset from the
// label's anchor in atlas pixels (`instanceOffsets`), the label's pixel offset, angle and colour.
// The quad is the frame around that offset, scaled by size / font size, turned anticlockwise by
// the angle, moved by the pixel offset (CSS pixels, y down), then y flipped; then, as for icons
// (`icon.wgsl`), a billboard adds it in clip space at the view centre's depth (deck's
// `project_pixel_size_to_clipspace` with `focalDistance`) and a ground-plane label lays it in the
// plane of east and north at the label. The fragment is deck's: with an SDF atlas, the fill is
// `smoothstep(buffer − gamma, buffer + gamma, d)`; with an outline, the colour is the outline's
// mixed into the fill's by that, and the alpha is the outline's own smoothstep at `outline_buffer`.
// deck draws a label with an outline twice, the second time with `outline_buffer = buffer`, so
// that a thick outline never covers a neighbouring glyph's fill. Alpha below the cutoff (0.001) is
// discarded, then the layer's opacity applied. Without SDF the atlas is plain coverage.
//
// Background. One instance per label: its box from `getBoundingRect` (atlas pixels from the
// anchor), scaled like the glyphs, grown by the padding (CSS pixels: left, top, right, bottom),
// turned, moved by the pixel offset. Filled, or stroked by `line_width` pixels inside its edge,
// with rounded corners when `border_radius` is set (deck's `round_rect` and `smoothedge`).
//
// Not ported (no layer of the app uses them): `getContentBox` clipping and content alignment, the
// picking pass and its highlight.

// `pipelines::text::StyleUniform`: deck's `icon`, `text` and `sdf` uniforms for one pass.
struct TextStyle {
    // `sizeScale`, `sizeMinPixels`, `sizeMaxPixels`
    size_scale: f32,
    size_min_pixels: f32,
    size_max_pixels: f32,
    // `text.fontSize`: the atlas's font size (64)
    font_size: f32,
    // `iconsTextureDim`
    atlas_size: vec2<f32>,
    // 0 or 1
    billboard: u32,
    // UNIT_COMMON, UNIT_METERS or UNIT_PIXELS
    size_units: u32,
    // `alphaCutoff`
    alpha_cutoff: f32,
    // `layer.opacity`
    opacity: f32,
    // deck's `project.focalDistance`: the clip w of a point at the view centre's depth
    focal_w: f32,
    // `sdf.enabled`: 0 or 1
    sdf: u32,
    // `sdf.buffer`, `sdf.gamma`, `sdf.outlineBuffer` (negative for no outline)
    buffer: f32,
    gamma: f32,
    outline_buffer: f32,
    pad0: f32,
    // `sdf.outlineColor`, 0 to 1
    outline_color: vec4<f32>,
    // the common space point common offsets are measured from (`ViewFrame::common_centre`)
    centre: vec3<f32>,
    pad1: f32,
}

// `pipelines::text::BackgroundUniform`: deck's `textBackground` and `text` uniforms.
struct BackgroundStyle {
    size_scale: f32,
    size_min_pixels: f32,
    size_max_pixels: f32,
    font_size: f32,
    // `borderRadius`, CSS pixels, deck's order
    border_radius: vec4<f32>,
    // `padding`: left, top, right, bottom, CSS pixels
    padding: vec4<f32>,
    billboard: u32,
    size_units: u32,
    // `stroked`: 0 or 1
    stroked: u32,
    opacity: f32,
    focal_w: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
    centre: vec3<f32>,
    pad3: f32,
}

@group(2) @binding(0) var<uniform> style: TextStyle;
@group(2) @binding(1) var atlas: texture_2d<f32>;
@group(2) @binding(2) var atlas_sampler: sampler;

// the background pipeline's group 2 holds only this (binding 3, so it never meets `style`)
@group(2) @binding(3) var<uniform> background: BackgroundStyle;

const UNIT_COMMON: u32 = 0u;
const UNIT_METERS: u32 = 1u;
const UNIT_PIXELS: u32 = 2u;

// deck's `SMOOTH_EDGE_RADIUS`
const SMOOTH_EDGE_RADIUS: f32 = 0.5;

// `rotate_by_angle`: GLSL's `mat2(cos, -sin, sin, cos) * vertex`, anticlockwise on the screen for
// a vertex with y down; the angle brought into ±180° and `sin_vers` used, as in `icon.wgsl`.
fn rotate_by_angle(vertex: vec2<f32>, angle: f32) -> vec2<f32> {
    let turned = angle - 360.0 * round(angle / 360.0);
    let sv = sin_vers(turned * PI / 180.0);
    let cos_angle = 1.0 - sv.y;
    let sin_angle = sv.x;
    return mat2x2<f32>(vec2<f32>(cos_angle, -sin_angle), vec2<f32>(sin_angle, cos_angle)) * vertex;
}

// `project_size_to_pixel(size, unit)`, metres at the view centre's scale (as `icon.wgsl`).
fn project_size_to_pixel(size: f32, unit: u32) -> f32 {
    if (unit == UNIT_METERS) {
        return size * globals.centre.z * globals.pixels_per_unit;
    }
    if (unit == UNIT_COMMON) {
        return size * globals.pixels_per_unit;
    }
    return size;
}

// East, north and up at a point of common space, as the columns of a matrix (as `icon.wgsl`).
fn ground_frame(centre: vec3<f32>, position: vec3<f32>) -> mat3x3<f32> {
    if (globals.mode == MODE_FLAT) {
        return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    }
    let uz = normalize(centre + position);
    var ux = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(uz.z) != 1.0) {
        ux = normalize(vec3<f32>(uz.y, -uz.x, 0.0));
    }
    let uy = cross(uz, ux);
    return mat3x3<f32>(-ux, -uy, uz);
}

// The clip position of `pixel_offset` (CSS pixels, y up) from the common offset `position`.
fn place(position: vec3<f32>, pixel_offset: vec2<f32>, billboard: u32, focal_w: f32, centre: vec3<f32>) -> vec4<f32> {
    if (billboard != 0u) {
        let clip = project_common(position);
        // `project_pixel_size_to_clipspace`
        let offset = pixel_offset / globals.viewport.xy * 2.0 * focal_w;
        return vec4<f32>(clip.xy + offset, clip.zw);
    }
    // `project_pixel_size`: CSS pixels to common units at the view centre's scale
    let offset_common = vec3<f32>(pixel_offset / globals.pixels_per_unit, 0.0);
    return project_common(position + ground_frame(centre, position) * offset_common);
}

struct Glyph {
    // the label's common offset (`instancePositions`)
    @location(1) position: vec3<f32>,
    // `instanceSizes`
    @location(2) size: f32,
    // `instanceIconFrames`: x, y, width, height in atlas texels
    @location(3) frame: vec4<f32>,
    // `instanceOffsets`: the frame's middle from the anchor, atlas pixels, y down
    @location(4) offset: vec2<f32>,
    // `instancePixelOffset`: CSS pixels, x right, y down
    @location(5) pixel_offset: vec2<f32>,
    // `instanceAngles`: degrees anticlockwise
    @location(6) angle: f32,
    // `instanceColors`
    @location(7) color: vec4<f32>,
}

struct GlyphVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) tex: vec2<f32>,
}

@vertex
fn vs_glyph(@location(0) positions: vec2<f32>, glyph: Glyph) -> GlyphVaryings {
    var out: GlyphVaryings;
    let icon_size = glyph.frame.zw;
    let size_pixels = clamp(
        project_size_to_pixel(glyph.size * style.size_scale, style.size_units),
        style.size_min_pixels,
        style.size_max_pixels,
    );
    let instance_scale = size_pixels / style.font_size;
    var pixel_offset = positions / 2.0 * icon_size + glyph.offset;
    pixel_offset = rotate_by_angle(pixel_offset, glyph.angle) * instance_scale;
    pixel_offset += glyph.pixel_offset;
    pixel_offset.y *= -1.0;
    out.position = place(glyph.position, pixel_offset, style.billboard, style.focal_w, style.centre);
    out.tex = mix(glyph.frame.xy, glyph.frame.xy + icon_size, (positions + 1.0) / 2.0) / style.atlas_size;
    out.color = glyph.color;
    return out;
}

@fragment
fn fs_glyph(in: GlyphVaryings) -> @location(0) vec4<f32> {
    var alpha = textureSample(atlas, atlas_sampler, in.tex).r;
    var color = in.color;
    if (style.sdf != 0u) {
        let distance = alpha;
        alpha = smoothstep(style.buffer - style.gamma, style.buffer + style.gamma, distance);
        if (style.outline_buffer > 0.0) {
            let in_fill = alpha;
            let in_border = smoothstep(style.outline_buffer - style.gamma, style.outline_buffer + style.gamma, distance);
            color = mix(style.outline_color, in.color, in_fill);
            alpha = in_border;
        }
    }
    let a = alpha * color.a;
    if (a < style.alpha_cutoff) {
        discard;
    }
    return vec4<f32>(color.rgb, a * style.opacity);
}

struct Box {
    @location(1) position: vec3<f32>,
    @location(2) size: f32,
    // `instanceRects`: `getBoundingRect`, atlas pixels from the anchor, y down
    @location(3) rect: vec4<f32>,
    // `instancePixelOffsets`
    @location(4) pixel_offset: vec2<f32>,
    @location(5) angle: f32,
    // `instanceLineWidths`, CSS pixels
    @location(6) line_width: f32,
    @location(7) fill_color: vec4<f32>,
    @location(8) line_color: vec4<f32>,
}

struct BoxVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) fill_color: vec4<f32>,
    @location(1) line_color: vec4<f32>,
    @location(2) line_width: f32,
    @location(3) uv: vec2<f32>,
    @location(4) dimensions: vec2<f32>,
}

@vertex
fn vs_background(@location(0) positions: vec2<f32>, b: Box) -> BoxVaryings {
    var out: BoxVaryings;
    out.uv = positions;
    out.line_width = b.line_width;
    let size_pixels = clamp(
        project_size_to_pixel(b.size * background.size_scale, background.size_units),
        background.size_min_pixels,
        background.size_max_pixels,
    );
    let instance_scale = size_pixels / background.font_size;
    out.dimensions = b.rect.zw * instance_scale + background.padding.xy + background.padding.zw;
    var pixel_offset = (positions * b.rect.zw + b.rect.xy) * instance_scale + mix(-background.padding.xy, background.padding.zw, positions);
    pixel_offset = rotate_by_angle(pixel_offset, b.angle);
    pixel_offset += b.pixel_offset;
    pixel_offset.y *= -1.0;
    out.position = place(b.position, pixel_offset, background.billboard, background.focal_w, background.centre);
    out.fill_color = vec4<f32>(b.fill_color.rgb, b.fill_color.a * background.opacity);
    out.line_color = vec4<f32>(b.line_color.rgb, b.line_color.a * background.opacity);
    return out;
}

// `smoothedge(edge, x)`
fn smoothedge(edge: f32, x: f32) -> f32 {
    return smoothstep(edge - SMOOTH_EDGE_RADIUS, edge + SMOOTH_EDGE_RADIUS, x);
}

// `round_rect(p, size, radii)`: the distance inside the rounded box, pixels.
fn round_rect(p: vec2<f32>, size: vec2<f32>, radii: vec4<f32>) -> f32 {
    let pixel_position_cb = (p - 0.5) * size;
    let size_cb = size * 0.5;
    let max_border_radius = min(size.x, size.y) * 0.5;
    var border_radius = min(radii, vec4<f32>(max_border_radius));
    let xy = select(border_radius.zw, border_radius.xy, pixel_position_cb.x > 0.0);
    border_radius = vec4<f32>(xy, border_radius.zw);
    border_radius.x = select(border_radius.y, border_radius.x, pixel_position_cb.y > 0.0);
    let q = abs(pixel_position_cb) - size_cb + border_radius.x;
    return -(min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - border_radius.x);
}

// `rect(p, size)`: the distance inside the box, pixels.
fn rect(p: vec2<f32>, size: vec2<f32>) -> f32 {
    let pixel_position = p * size;
    return min(min(pixel_position.x, size.x - pixel_position.x), min(pixel_position.y, size.y - pixel_position.y));
}

// `get_stroked_fragColor(dist)`
fn stroked_color(fill_color: vec4<f32>, line_color: vec4<f32>, line_width: f32, dist: f32) -> vec4<f32> {
    let is_border = smoothedge(dist, line_width);
    return mix(fill_color, line_color, is_border);
}

@fragment
fn fs_background(in: BoxVaryings) -> @location(0) vec4<f32> {
    var frag_color: vec4<f32>;
    if (any(background.border_radius != vec4<f32>(0.0))) {
        let dist_to_edge = round_rect(in.uv, in.dimensions, background.border_radius);
        let shape_alpha = smoothedge(-dist_to_edge, 0.0);
        if (shape_alpha == 0.0) {
            discard;
        }
        if (background.stroked != 0u) {
            frag_color = stroked_color(in.fill_color, in.line_color, in.line_width, dist_to_edge);
        } else {
            frag_color = in.fill_color;
        }
        frag_color.a *= shape_alpha;
    } else {
        if (background.stroked != 0u) {
            frag_color = stroked_color(in.fill_color, in.line_color, in.line_width, rect(in.uv, in.dimensions));
        } else {
            frag_color = in.fill_color;
        }
    }
    return frag_color;
}
