// sky.wgsl: the sky behind the realistic map (doc 04 §3.6, doc 03 §12; today the map container's
// CSS background, `skyCss`), drawn first over the whole map target: a vertical gradient from `top`
// at the top of the screen to `horizon` at `horizon.a` of the way down, `horizon` below it, and
// over it the sun's glow, a radial gradient of four stops, composited with normal alpha blending.
// As CSS does: positions in CSS pixels at each pixel's centre, colours interpolated in sRGB bytes
// (the map target holds gamma-space values), the glow's stops in premultiplied alpha. Its CPU twin
// is `layers::sky::twin`.

//!include lib/project.wgsl

// `layers::sky::SkyUniform`
struct Sky {
    // the colour at the top of the screen (0–1 sRGB); a unused
    top: vec4<f32>,
    // the colour at the horizon and below it; a: where the horizon is, 0 (top) to 1 (bottom)
    horizon: vec4<f32>,
    // the glow's centre (CSS pixels from the top left), its alpha at 0 and 14 px, at 40 px
    glow_at: vec4<f32>,
    // the glow's colour at 40 and 320 px; a: 1 when there is a glow
    glow: vec4<f32>,
}

@group(2) @binding(0) var<uniform> sky: Sky;

// One triangle that covers the screen.
@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// Between two of the glow's stops (rgb, alpha) at `t`, premultiplied: (rgb · alpha, alpha).
fn stop_mix(a: vec4<f32>, b: vec4<f32>, t: f32) -> vec4<f32> {
    return mix(vec4<f32>(a.rgb * a.a, a.a), vec4<f32>(b.rgb * b.a, b.a), t);
}

@fragment
fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // CSS pixels: the map is drawn at `map_scale` map pixels per CSS pixel
    let css = position.xy / globals.viewport.z;
    // the horizon's line down the screen; at the very top every pixel has the horizon's colour
    let at = sky.horizon.a * globals.viewport.y;
    var t = 1.0;
    if (at > 0.0) {
        t = clamp(css.y / at, 0.0, 1.0);
    }
    var rgb = mix(sky.top.rgb, sky.horizon.rgb, t);
    if (sky.glow.a > 0.5) {
        let r = distance(css, sky.glow_at.xy);
        let alpha = sky.glow_at.z;
        let faint = sky.glow_at.w;
        let s0 = vec4<f32>(vec3<f32>(255.0, 252.0, 240.0) / 255.0, alpha);
        let s1 = vec4<f32>(vec3<f32>(255.0, 250.0, 235.0) / 255.0, alpha);
        let s2 = vec4<f32>(sky.glow.rgb, faint);
        let s3 = vec4<f32>(sky.glow.rgb, 0.0);
        var g = vec4<f32>(0.0);
        if (r < 14.0) {
            g = stop_mix(s0, s1, r / 14.0);
        } else if (r < 40.0) {
            g = stop_mix(s1, s2, (r - 14.0) / 26.0);
        } else if (r < 320.0) {
            g = stop_mix(s2, s3, (r - 40.0) / 280.0);
        }
        rgb = g.rgb + rgb * (1.0 - g.a);
    }
    return vec4<f32>(rgb, 1.0);
}
