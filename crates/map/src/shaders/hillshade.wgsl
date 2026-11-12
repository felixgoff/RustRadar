//!include lib/rlight.wgsl
// hillshade.wgsl: the shaded relief on the realistic map (item R12; doc 04 §12, §14.2;
// `terrain-layer.ts`'s `hillshadeLayer` with `realistic.ts`'s `HazeFadeExtension`, R:921-937):
// deck's BitmapLayer as `bitmap.wgsl` draws it, with what the extension injects at
// `fs:DECKGL_FILTER_COLOR`, `color.a *= rl_clear();`: the relief fades into the distance's air
// rather than taking its colour. The Rust side is `layers::hillshade`; on the dark and satellite
// maps (no extension) the layer draws with `bitmap.wgsl` itself.
//
// A vertex is the tile mesh's local Mercator point (`u`, `v` south, metres up) with the image's
// texture coordinates there (`pipelines::bitmap::tile_mesh`), as `bitmap.wgsl` takes it. The light's
// vertex part runs as every realistic shader runs it (`rl_vertex`); `rl_clear` reads only `rl_eye`,
// so that is the one varying handed on.
//
// What differs from the GLSL (doc 04 §16.2): the bind groups. `rlight` is group 2 (lib/rlight.wgsl),
// so the image, its sampler and BitmapLayer's uniforms share group 3 (`bitmap.wgsl` keeps them in
// groups 2 and 3); the fragment is `bitmap.wgsl`'s, line for line.

// `pipelines::bitmap::StyleUniform`: deck's `bitmap` uniforms and the layer's opacity.
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

@group(3) @binding(0) var image: texture_2d<f32>;
@group(3) @binding(1) var image_sampler: sampler;
@group(3) @binding(2) var<uniform> style: Style;

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    @location(0) tex: vec2<f32>,
    // `rl_eye`: to the camera from here, metres east, north and up
    @location(1) eye: vec3<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) tex: vec2<f32>) -> Varyings {
    let p = project_local(position);
    let rl = rl_vertex(p.rel, rl_mercator(local_offset(position.xy)));
    var out: Varyings;
    out.clip = p.clip;
    out.tex = tex;
    out.eye = rl.eye;
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
    var color = apply_opacity(color_tint(color_desaturate(bitmap_color.rgb)), bitmap_color.a * style.opacity);
    // `fs:DECKGL_FILTER_COLOR` (HAZE_FADE): only `rl_eye` is read
    let rl = RlVary(in.eye, vec3<f32>(0.0, 0.0, 1.0), vec2<f32>(0.0), vec2<f32>(0.0));
    color.a = color.a * rl_clear(rl);
    return color;
}
