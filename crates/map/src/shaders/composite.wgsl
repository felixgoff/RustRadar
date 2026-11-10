// composite.wgsl: the resolved map texture onto the surface (plan 1.4.1, 1.4.4 pass 3). Bilinear
// where the map is drawn at a lower resolution than the surface (the realistic map caps its scale
// at 1.5); texel for texel where the sizes agree. The map's colours are gamma-space values, as on
// the WebGL canvas: they go out unchanged to a non-sRGB surface and are linearised for an sRGB
// one, which the GPU then encodes back. Tilt-shift and rain are `post.wgsl`'s (stage 8).

struct Composite {
    // size of the surface being drawn, pixels
    target_size: vec2<f32>,
    // bit 0: the surface is sRGB, linearise
    flags: u32,
    pad: u32,
}

@group(0) @binding(0) var map_texture: texture_2d<f32>;
@group(0) @binding(1) var map_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Composite;

// One triangle that covers the screen.
@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// The sRGB decoding curve, exactly, so the GPU's encoding gives the map's bytes back.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // the pixel's centre, so equal sizes sample texel centres exactly
    let uv = position.xy / params.target_size;
    var rgb = textureSampleLevel(map_texture, map_sampler, uv, 0.0).rgb;
    if ((params.flags & 1u) != 0u) {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
