//!include lib/project.wgsl
// icon.wgsl: deck.gl 9.4's IconLayer (`icon-layer-vertex.glsl`, `icon-layer-fragment.glsl`, the
// WebGL path the app runs on) with the picking module's hover highlight. One instance per icon,
// the layout of `pipelines::icon::Instance`; `pipelines::icon` holds the Rust side.
//
// deck's vertex shader in this renderer's spaces. An icon's position is a common offset (worked
// out on the CPU in f64, `ViewFrame::common_offset`). The quad's corners are CSS pixels around it:
// the cell's corners scaled so the cell's height (or width) is the icon's size in pixels, turned
// anticlockwise by the angle, moved by the pixel offset, y up. Then deck's two cases:
// - billboard: the corners are added in clip space as `pixels / viewport · 2 · focal_w`, deck's
//   `project_pixel_size_to_clipspace`, whose `focalDistance` is the clip w of the view centre
//   (`pipelines::icon::focal_w`): so an icon at the view centre's depth is exactly its size, and
//   one nearer the camera or further from it larger or smaller by focal_w / w, as in deck (the quad
//   stays upright and square on the screen, whatever the pitch and bearing);
// - ground-plane (`billboard: false`): the corners become common units at the view centre's scale
//   (deck's `project_pixel_size`, pixels / `scale`) and lie in the plane of east and north at the
//   icon, so the quad turns and foreshortens with the ground. On the globe deck lays them along its
//   `project_get_orientation_matrix` frame, which is east and north turned half a turn (its train
//   icons add 180° there); here they lie along true east and north in both projections (plan
//   section 6, deviation 1), so an angle means the same thing on both sides of zoom 12.
// The fragment shader is deck's: the atlas colour, or the instance colour with the atlas alpha in
// mask mode; alpha times the layer's opacity and the instance alpha; fragments under the alpha
// cutoff discarded; then `picking_filterHighlightColor` for an instance with a highlight.

// `pipelines::icon::StyleUniform`: deck's `icon` uniforms, the layer's opacity and the view terms.
struct IconStyle {
    // `sizeScale`
    size_scale: f32,
    // `sizeBasis`: 0 width, 1 height
    size_basis: f32,
    // `sizeMinPixels`, `sizeMaxPixels`
    size_min_pixels: f32,
    size_max_pixels: f32,
    // `iconsTextureDim`: the atlas's width and height in texels
    atlas_size: vec2<f32>,
    // `billboard`: 0 or 1
    billboard: u32,
    // `sizeUnits`: UNIT_COMMON, UNIT_METERS or UNIT_PIXELS
    size_units: u32,
    // `alphaCutoff`
    alpha_cutoff: f32,
    // `layer.opacity`
    opacity: f32,
    // deck's `project.focalDistance`: the clip w of a point at the view centre's depth
    focal_w: f32,
    pad0: f32,
    // the common space point common offsets are measured from (`ViewFrame::common_centre`); on the
    // globe the up direction at an icon is that point plus its offset
    centre: vec3<f32>,
    pad1: f32,
}

@group(2) @binding(0) var<uniform> style: IconStyle;
@group(2) @binding(1) var atlas: texture_2d<f32>;
@group(2) @binding(2) var atlas_sampler: sampler;

// deck's `UNIT`
const UNIT_COMMON: u32 = 0u;
const UNIT_METERS: u32 = 1u;
const UNIT_PIXELS: u32 = 2u;

struct Icon {
    // the common offset of the icon's position (`instancePositions`)
    @location(1) position: vec3<f32>,
    // `instanceSizes`
    @location(2) size: f32,
    // `instanceIconFrames`: the cell's x, y, width, height in atlas texels
    @location(3) frame: vec4<f32>,
    // `instanceOffsets`: the cell's centre minus its anchor, texels
    @location(4) offset: vec2<f32>,
    // `instancePixelOffset`: CSS pixels, x right, y down
    @location(5) pixel_offset: vec2<f32>,
    // `instanceAngles`: degrees anticlockwise
    @location(6) angle: f32,
    // `instanceColorModes`: 1 for a mask icon, 0 for a coloured one
    @location(7) color_mode: f32,
    // `instanceColors`
    @location(8) color: vec4<f32>,
    // the hover highlight (`highlightColor` on the highlighted instance), alpha 0 for none
    @location(9) highlight: vec4<f32>,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) color_mode: f32,
    @location(1) color: vec4<f32>,
    @location(2) tex: vec2<f32>,
    @location(3) highlight: vec4<f32>,
}

// `rotate_by_angle`: GLSL's `mat2(cos, -sin, sin, cos) * vertex`, anticlockwise on the screen for
// a vertex with y down. The angle is brought into ±180° first and `sin_vers` used (lib/project.wgsl)
// rather than the GPU's sin and cos, which WGSL leaves loose outside [-π, π].
fn rotate_by_angle(vertex: vec2<f32>, angle: f32) -> vec2<f32> {
    let turned = angle - 360.0 * round(angle / 360.0);
    let angle_radian = turned * PI / 180.0;
    let sv = sin_vers(angle_radian);
    let cos_angle = 1.0 - sv.y;
    let sin_angle = sv.x;
    let rotation_matrix = mat2x2<f32>(vec2<f32>(cos_angle, -sin_angle), vec2<f32>(sin_angle, cos_angle));
    return rotation_matrix * vertex;
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

// `project_pixel_size_to_clipspace`: CSS pixels to a clip-space offset at the view centre's depth.
fn project_pixel_size_to_clipspace(pixels: vec2<f32>) -> vec2<f32> {
    let offset = pixels / globals.viewport.xy * 2.0;
    return offset * style.focal_w;
}

// East, north and up at the icon in common space, as the columns of a matrix: the identity on the
// flat map; on the globe deck's `project_get_orientation_matrix(up)` with its first two columns
// negated (true east and north).
fn ground_frame(position: vec3<f32>) -> mat3x3<f32> {
    if (globals.mode == MODE_FLAT) {
        return mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    }
    let uz = normalize(style.centre + position);
    var ux = vec3<f32>(1.0, 0.0, 0.0);
    if (abs(uz.z) != 1.0) {
        ux = normalize(vec3<f32>(uz.y, -uz.x, 0.0));
    }
    let uy = cross(uz, ux);
    return mat3x3<f32>(-ux, -uy, uz);
}

@vertex
fn vs(@location(0) positions: vec2<f32>, icon: Icon) -> Varyings {
    var out: Varyings;
    let icon_size = icon.frame.zw;
    let size_pixels = clamp(
        project_size_to_pixel(icon.size * style.size_scale, style.size_units),
        style.size_min_pixels,
        style.size_max_pixels,
    );
    let icon_constraint = select(icon_size.y, icon_size.x, style.size_basis == 0.0);
    let instance_scale = select(size_pixels / icon_constraint, 0.0, icon_constraint == 0.0);
    var pixel_offset = positions / 2.0 * icon_size + icon.offset;
    pixel_offset = rotate_by_angle(pixel_offset, icon.angle) * instance_scale;
    pixel_offset += icon.pixel_offset;
    pixel_offset.y *= -1.0;
    if (style.billboard != 0u) {
        let clip = project_common(icon.position);
        let offset = project_pixel_size_to_clipspace(pixel_offset);
        out.position = vec4<f32>(clip.xy + offset, clip.zw);
    } else {
        let offset_common = vec3<f32>(pixel_offset / globals.pixels_per_unit, 0.0);
        out.position = project_common(icon.position + ground_frame(icon.position) * offset_common);
    }
    out.tex = mix(icon.frame.xy, icon.frame.xy + icon_size, (positions.xy + 1.0) / 2.0) / style.atlas_size;
    out.color = icon.color;
    out.color_mode = icon.color_mode;
    out.highlight = icon.highlight;
    return out;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    let tex_color = textureSample(atlas, atlas_sampler, in.tex);
    let color = mix(tex_color.rgb, in.color.rgb, in.color_mode);
    let a = tex_color.a * style.opacity * in.color.a;
    if (a < style.alpha_cutoff) {
        discard;
    }
    var frag_color = vec4<f32>(color, a);
    // picking_filterHighlightColor
    if (in.highlight.a > 0.0) {
        let high_light_alpha = in.highlight.a;
        let blended_alpha = high_light_alpha + frag_color.a * (1.0 - high_light_alpha);
        let high_light_ratio = high_light_alpha / blended_alpha;
        let blended_rgb = mix(frag_color.rgb, in.highlight.rgb, high_light_ratio);
        frag_color = vec4<f32>(blended_rgb, blended_alpha);
    }
    return frag_color;
}
