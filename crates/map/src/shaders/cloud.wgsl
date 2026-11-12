//!include lib/rlight.wgsl
// cloud.wgsl: the realistic globe's clouds (doc 04 §9; `realistic.ts`'s `CloudExtension` on deck's
// `BitmapLayer`, R:892-916, G:3606-3638): EUMETSAT's infrared picture laid at the clouds' height,
// its grey turned into how much cloud there is (the warm ground clear), white, lit by the sun where
// it is up there and fading into the night, through the air to the camera. The Rust side is
// `layers::clouds` (the mesh, the picture's texture, the opacity); its CPU twin is
// `layers::clouds::twin`.
//
// A vertex is the mesh's local Mercator point (`u`, `v` south, metres up) with the picture's
// texture coordinates there, interpolated across each triangle as deck's mesh carries them
// (`createMesh`: on the globe a grid of 5° in longitude and latitude, the picture's coordinates
// linear in degrees; on the flat map one quad a strip).
//
// What differs from deck's shaders (doc 04 §16.2): the picture is a single-channel texture (its
// red: the picture is grey, and the GLSL reads `color.r` and the alpha alone; a JPEG's alpha is 1);
// BitmapLayer's `tintColor` (white), `desaturate` (0) and `transparentColor` (none) are left out,
// as they change nothing here.

// `layers::clouds::CloudUniform`
struct Cloud {
    // the layer's opacity (`1 − smoothstep(5.5, 6.5, zoom)`); the rest unused
    opacity: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
}

// the picture (its red channel, linear: deck's `rgba8unorm`) and deck's sampler for it (linear,
// linear between mip levels, clamped to the edge), and the layer's opacity
@group(3) @binding(0) var picture: texture_2d<f32>;
@group(3) @binding(1) var picture_sampler: sampler;
@group(3) @binding(2) var<uniform> cloud: Cloud;

struct CloudOut {
    @builtin(position) clip: vec4<f32>,
    // `rl_eye`, `rl_sun`, `rl_t`, `rl_c`
    @location(0) eye: vec3<f32>,
    @location(1) sun: vec3<f32>,
    @location(2) t: vec2<f32>,
    @location(3) c: vec2<f32>,
    // the picture's texture coordinates (deck's `vTexCoord`)
    @location(4) uv: vec2<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) tex: vec2<f32>) -> CloudOut {
    let p = project_local(position);
    let rl = rl_vertex(p.rel, rl_mercator(local_offset(position.xy)));
    var out: CloudOut;
    out.clip = p.clip;
    out.eye = rl.eye;
    out.sun = rl.sun;
    out.t = rl.t;
    out.c = rl.c;
    out.uv = tex;
    return out;
}

@fragment
fn fs(in: CloudOut) -> @location(0) vec4<f32> {
    // BitmapLayer's fragment: the picture's texel, its alpha times the layer's opacity
    let texel = textureSample(picture, picture_sampler, in.uv);
    let color = vec4<f32>(texel.r, texel.r, texel.r, texel.a * cloud.opacity);
    // `fs:DECKGL_FILTER_COLOR`
    let rl = RlVary(in.eye, in.sun, in.t, in.c);
    let cover = smoothstep(0.33, 0.62, color.r) * color.a;
    let lit = vec3<f32>(0.9) * (rl_sun_light(rl) * max(rl_to_sun(rl).z, 0.0) + vec3<f32>(rlight.ambient * rl_day_here(rl)));
    return vec4<f32>(rl_display(rl_aerial(rl, lit)), cover);
}
