//!include lib/project.wgsl
// terrain_surface.wgsl: the 3D surface (item Z1, `layers/terrain/mesh.rs`; doc 04 §14.3): deck.gl's
// TerrainLayer, a SimpleMeshLayer per height tile with no normals, drawn in the mesh colour
// `MESH_COLOR` and lit by the scene's light rig with luma.gl's default Phong material (deck 9.4 reads
// `material: false` as that material, `pipelines/mesh.rs`).
//
// - Vertex: the tile's RTIN mesh in its local Mercator frame (`(u, v)` in tile units, the height in
//   metres), the height first raised to the sea's (`SeaLevelExtension`: `size.z = max(size.z, 0.0)`,
//   on the skirt's vertices as well).
// - Fragment: SimpleMeshLayer's flat shading for a mesh without normals,
//   `normalize(cross(dFdx(position), dFdy(position)))`, here in the render frame (deck's common space
//   on the flat map, turned on the globe as the key light's direction is); wgpu's framebuffer y runs
//   down where GL's runs up, so `dFdy` is `-dpdy`. Then `mesh.wgsl`'s lighting (ambient, the camera
//   light, the key light) per fragment, as SimpleMeshLayer's fragment shader does it.
//
// What is draped over the surface (the cover textures) and the realistic map's air over it
// (`TERRAIN_AIR`) are the terrain items Z2 and on.

// `mesh::LightsUniform`: the light rig, once per frame.
struct Lights {
    ambient: vec3<f32>,
    camera: vec3<f32>,
    key: vec3<f32>,
    // the direction the key light shines along (unit), in common space
    key_common: vec3<f32>,
    // the same in the render frame
    key_render: vec3<f32>,
}

// `mesh::StyleUniform`: the surface's colour and material.
struct Style {
    // `MESH_COLOR`, RGBA 0–1
    color: vec4<f32>,
    // `specularColor`, 0–255
    specular: vec3<f32>,
    ambient: f32,
    diffuse: f32,
    shininess: f32,
    opacity: f32,
    unlit: u32,
}

@group(2) @binding(0) var<uniform> lights: Lights;
@group(2) @binding(1) var<uniform> style: Style;

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    // the fragment in the render frame, relative to the view centre (common units)
    @location(0) rel: vec3<f32>,
}

// `v` scaled to unit length; the zero vector stays zero.
fn unit(v: vec3<f32>) -> vec3<f32> {
    let len = length(v);
    if (len > 0.0) {
        return v / len;
    }
    return vec3<f32>(0.0);
}

// luma's `lighting_getLightColor` for one light (`mesh.wgsl`'s `light_one`).
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

// The rig on a surface colour (`mesh.wgsl`'s `shade`): ambient, the camera light, the key light.
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

@vertex
fn vs(@location(0) position: vec3<f32>) -> Varyings {
    // no lower than the sea
    let p = project_local(vec3<f32>(position.xy, max(position.z, 0.0)));
    var out: Varyings;
    out.clip = p.clip;
    out.rel = p.rel;
    return out;
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    // the face's normal, from the screen derivatives (uniform control flow: before any branch)
    let normal = unit(cross(dpdx(in.rel), -dpdy(in.rel)));
    let rgb = shade(style.color.rgb, globals.camera_render, in.rel, normal, lights.key_render);
    return vec4<f32>(rgb, style.color.a * style.opacity);
}
