//!include lib/project.wgsl
// fill.wgsl: deck.gl's SolidPolygonLayer as the app uses it (filled, not extruded, unlit; plan
// 1.4.6, `pipelines/fill.rs`). Triangles in the draw's local Mercator frame (group 1), each vertex
// with its own RGBA colour; the colour goes out as it is, its alpha times the layer's opacity
// (`vColor = vec4(colors.rgb, colors.a * layer.opacity)`), and blends with deck's default.

// `fill::StyleUniform`: what the layer multiplies its vertices' colours by. deck's layers only
// ever set the opacity, `(1, 1, 1, opacity)`.
struct Style {
    color: vec4<f32>,
}

@group(2) @binding(0) var<uniform> style: Style;

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> Varyings {
    var out: Varyings;
    out.clip = project_local(position).clip;
    out.color = color * style.color;
    return out;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}
