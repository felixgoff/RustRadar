//!include lib/project.wgsl
// radar.wgsl: the weather radar's band polygons as today's `radarPolygons` draws them (doc 02 §3.5,
// G:1172-1203; `layers/radar/measured.rs`): deck's SolidPolygonLayer, filled, not extruded, unlit
// (`material: false`), the vertices' own RGBA with the layer's opacity, in the draw's local
// Mercator frame (group 1). The fill shader (`fill.wgsl`) with one thing more: the vertex's third
// coordinate is stretched by `style.z_scale` before the frame's `z_offset` lifts it, which is what
// the layer's model matrix does today.
//
// - A slice (flat, `h` 0 at every vertex) at `z` metres: `z_scale` anything, the frame's
//   `z_offset` = z (`modelMatrix` translating by z).
// - A wall (`h` 0 at the foot, 1 at the top) from `bottom` up to `z`: `z_scale` = z − bottom, the
//   frame's `z_offset` = bottom (`[1,0,0,0, 0,1,0,0, 0,0,z−bottom,0, 0,0,bottom,1]`). deck applies
//   that matrix to longitude, latitude and metres before projecting, so the height is exactly
//   `bottom + h·(z − bottom)` metres on the globe and on the flat map alike.

// `measured::StyleUniform`.
struct Style {
    // what each vertex's RGBA is multiplied by: (1, 1, 1, opacity)
    color: vec4<f32>,
    // metres per unit of the vertex's third coordinate
    z_scale: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
}

@group(2) @binding(0) var<uniform> style: Style;

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> Varyings {
    var out: Varyings;
    out.clip = project_local(vec3<f32>(position.xy, position.z * style.z_scale)).clip;
    out.color = color * style.color;
    return out;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    return in.color;
}
