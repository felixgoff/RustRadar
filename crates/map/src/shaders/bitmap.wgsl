//!include lib/project.wgsl
// bitmap.wgsl: deck.gl's BitmapLayer as the app uses it for Web Mercator tile images (imagery,
// hillshade, the OpenRailwayMap raster; plan 1.4.6, `pipelines/bitmap.rs`). A mesh in the draw's
// local Mercator frame (group 1) carries texture coordinates that run linearly in Mercator, so the
// image is Mercator-correct on the globe as on the flat map (`_imageCoordinateSystem: CARTESIAN`);
// the texture is group 2, the style group 3. The fragment is deck's: desaturate, tint, opacity,
// then the transparent colour blended under it.

// `bitmap::StyleUniform`: deck's `bitmap` uniforms and the layer's opacity.
struct Style {
    // `tintColor / 255`
    tint: vec3<f32>,
    // `desaturate`, 0–1
    desaturate: f32,
    // `transparentColor / 255`
    transparent: vec4<f32>,
    // `layer.opacity`
    opacity: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
}

@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;
@group(3) @binding(0) var<uniform> style: Style;

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    @location(0) tex: vec2<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) tex: vec2<f32>) -> Varyings {
    var out: Varyings;
    out.clip = project_local(position).clip;
    out.tex = tex;
    return out;
}

// apply desaturation
fn color_desaturate(color: vec3<f32>) -> vec3<f32> {
    let luminance = (color.r + color.g + color.b) * 0.333333333;
    return mix(color, vec3<f32>(luminance), style.desaturate);
}

// apply tint
fn color_tint(color: vec3<f32>) -> vec3<f32> {
    return color * style.tint;
}

// blend with background color
fn apply_opacity(color: vec3<f32>, alpha: f32) -> vec4<f32> {
    if (style.transparent.a == 0.0) {
        return vec4<f32>(color, alpha);
    }
    let blended_alpha = alpha + style.transparent.a * (1.0 - alpha);
    let highlight_ratio = alpha / blended_alpha;
    let blended_rgb = mix(style.transparent.rgb, color, highlight_ratio);
    return vec4<f32>(blended_rgb, blended_alpha);
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    let bitmap_color = textureSample(image, image_sampler, in.tex);
    return apply_opacity(color_tint(color_desaturate(bitmap_color.rgb)), bitmap_color.a * style.opacity);
}
