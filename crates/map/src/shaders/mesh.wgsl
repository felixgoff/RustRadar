//!include lib/project.wgsl
// mesh.wgsl: lit meshes with the scene's light rig (plan 1.4.6, `pipelines/mesh.rs`; doc 01 §4,
// doc 03 §5.2). Two vertex entries share one vertex layout (position, normal, RGB colour):
// - `vs` + `fs`: deck.gl's SimpleMeshLayer, instanced. A vertex is in model metres; each instance
//   carries the 3 × 3 matrix that takes model metres to common units (east, north and up at the
//   instance, already turned and scaled on the CPU) and the common offset of its origin. Phong per
//   fragment, as the SimpleMeshLayer fragment shader does it.
// - `vs_local` + `fs_local`: the lit extruded polygons of deck's SolidPolygonLayer, in the draw's
//   local Mercator frame (group 1); the normal is east, north, up. Phong per vertex (Gouraud), as
//   the SolidPolygonLayer vertex shader does it.
// The `shadow` variants are the same entries with the `SHADOW` draw state.
//
// Lighting is luma.gl's Phong (`phong-shaders-glsl.js`) with the lights of deck's LightingEffect:
//   colour = ambient · surface · ambientLight
//          + Σ (lambert · diffuse · surface + specular · specularColor / 255) · lightColor
// over the camera light (a point light at the camera, no attenuation) and the key light (a
// directional light), in that order; specular = pow(max(N·H, 0), shininess), only where
// lambert > 0. Light colours arrive as colour / 255 × intensity. The instanced path works in
// common space (deck lights there in both projections); the local path in the render frame,
// with the key light's direction turned into it on the CPU.

// `mesh::LightsUniform`: the light rig, once per frame.
struct Lights {
    // the ambient light's colour × intensity, 0–1
    ambient: vec3<f32>,
    // the camera light's colour × intensity
    camera: vec3<f32>,
    // the key light's colour × intensity
    key: vec3<f32>,
    // the direction the key light shines along (unit), in common space
    key_common: vec3<f32>,
    // the same in the render frame (equal to `key_common` on the flat map)
    key_render: vec3<f32>,
}

// `mesh::StyleUniform`: one draw's material and opacity.
struct Style {
    // the local entry's fill colour, RGBA 0–1 (instanced draws use their instances' colours)
    color: vec4<f32>,
    // deck's `specularColor`, 0–255
    specular: vec3<f32>,
    ambient: f32,
    diffuse: f32,
    shininess: f32,
    // deck's layer opacity: multiplies the alpha
    opacity: f32,
    // 1: unlit, the surface colour as it is (luma's `material.unlit`)
    unlit: u32,
}

@group(2) @binding(0) var<uniform> lights: Lights;
@group(2) @binding(1) var<uniform> style: Style;

struct MeshVertex {
    // instanced: model metres; local: (u, v) in the frame's units and the height in metres
    @location(0) position: vec3<f32>,
    // instanced: model space; local: east, north, up
    @location(1) normal: vec3<f32>,
    // deck's mesh `colors`, RGB 0–1 (1 where a mesh has none)
    @location(2) color: vec3<f32>,
}

// `mesh::Instance`.
struct MeshInstance {
    // model metres → common units
    @location(3) col0: vec3<f32>,
    @location(4) col1: vec3<f32>,
    @location(5) col2: vec3<f32>,
    // the common offset of the model's origin (`ViewFrame::common_offset`)
    @location(6) offset: vec3<f32>,
    // deck's `getColor`, RGBA
    @location(7) color: vec4<f32>,
    // the hover highlight colour; alpha 0 for none
    @location(8) highlight: vec4<f32>,
}

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    // the fragment's common offset
    @location(0) position: vec3<f32>,
    // unit, common space (deck's `normals_commonspace`, not renormalised per fragment)
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) highlight: vec4<f32>,
}

struct LocalVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

// `v` scaled to unit length; the zero vector stays zero (GLSL's `normalize` would give NaN, which
// a flattened shadow matrix can produce for a normal along the sun's rays).
fn unit(v: vec3<f32>) -> vec3<f32> {
    let len = length(v);
    if (len > 0.0) {
        return v / len;
    }
    return vec3<f32>(0.0);
}

// luma's `lighting_getLightColor(surfaceColor, light_direction, view_direction, normal, color)`.
fn light_one(surface: vec3<f32>, l: vec3<f32>, v: vec3<f32>, n: vec3<f32>, color: vec3<f32>) -> vec3<f32> {
    let halfway = unit(l + v);
    var lambertian = dot(l, n);
    var specular = 0.0;
    if (lambertian > 0.0) {
        let angle = max(dot(n, halfway), 0.0);
        if (angle > 0.0) {
            specular = pow(angle, style.shininess);
        }
    }
    lambertian = max(lambertian, 0.0);
    return (lambertian * style.diffuse * surface + specular * (style.specular / 255.0)) * color;
}

// luma's `lighting_getLightColor(surfaceColor, cameraPosition, position, normal)` with deck's rig:
// ambient, then the camera light (its direction is the view direction), then the key light.
fn shade(surface: vec3<f32>, camera: vec3<f32>, position: vec3<f32>, normal: vec3<f32>, key_direction: vec3<f32>) -> vec3<f32> {
    if (style.unlit != 0u) {
        return surface;
    }
    let view_direction = unit(camera - position);
    var color = style.ambient * surface * lights.ambient;
    color += light_one(surface, view_direction, view_direction, normal, lights.camera);
    color += light_one(surface, -key_direction, view_direction, normal, lights.key);
    return color;
}

// deck's `picking_filterHighlightColor`: the highlight mixed over the colour by its alpha
// (`mix(rgb, highlight.rgb, highlight.a)` with the alpha kept, for an opaque colour).
fn highlighted(color: vec4<f32>, highlight: vec4<f32>) -> vec4<f32> {
    if (highlight.a <= 0.0) {
        return color;
    }
    let alpha = highlight.a + color.a * (1.0 - highlight.a);
    return vec4<f32>(mix(color.rgb, highlight.rgb, highlight.a / alpha), alpha);
}

@vertex
fn vs(v: MeshVertex, i: MeshInstance) -> Varyings {
    let model = mat3x3<f32>(i.col0, i.col1, i.col2);
    let position = model * v.position + i.offset;
    var out: Varyings;
    out.clip = project_common(position);
    out.position = position;
    // deck's `project_normal(instanceModelMatrix * normals)`: by the instance's scale, not its
    // inverse transpose, then normalised
    out.normal = unit(model * v.normal);
    out.color = vec4<f32>(v.color * i.color.rgb, i.color.a);
    out.highlight = i.highlight;
    return out;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    let color = highlighted(in.color, in.highlight);
    let rgb = shade(color.rgb, globals.camera_common, in.position, in.normal, lights.key_common);
    return vec4<f32>(rgb, color.a * style.opacity);
}

@vertex
fn vs_local(v: MeshVertex) -> LocalVaryings {
    let p = project_local(v.position);
    let normal = unit(local_basis(v.position.xy) * v.normal);
    let surface = v.color * style.color.rgb;
    var out: LocalVaryings;
    out.clip = p.clip;
    out.color = vec4<f32>(shade(surface, globals.camera_render, p.rel, normal, lights.key_render), style.color.a * style.opacity);
    return out;
}

@fragment
fn fs_local(in: LocalVaryings) -> @location(0) vec4<f32> {
    return in.color;
}
