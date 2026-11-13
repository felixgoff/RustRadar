//!include lib/rlight.wgsl
// terrain.wgsl: the 3D surface with what is draped over it (item Z2, `layers/terrain/cover.rs`;
// doc 04 §12, §14.3; plan 1.4.12). deck.gl's TerrainLayer (a SimpleMeshLayer per height tile)
// under `TerrainExtension`'s cover: the tile's mesh as `terrain_surface.wgsl` draws it (Z1), its
// colour the cover texture over the mesh colour, and on the realistic map the air between it and
// the camera (`TERRAIN_AIR`).
//
// - Vertex: the tile's RTIN mesh in its local Mercator frame (`(u, v)` in tile units, the height in
//   metres), the height first raised to the sea's (`SeaLevelExtension`); `(u, v)` is where the
//   tile's cover is sampled (deck: `(terrainMercPos − bounds.xy) / bounds.zw`, the cover's bounds
//   being the tile's square); the light's terms (`rl_vertex`) for the air.
// - Fragment: `TERRAIN_MODE_USE_COVER`'s `color = pixel + color · (1 − pixel.a)` (the cover holds
//   colours premultiplied by their coverage, drawn over a clear transparent texture), then
//   SimpleMeshLayer's flat-shaded normal and luma's Phong lighting on that colour, as
//   `terrain_surface.wgsl` lights the bare mesh (deck's `material: false` is the default Phong
//   material, which lights the draped colour too). `fs_air` adds `TERRAIN_AIR` at the end
//   (`realistic.ts:979-983`): `rgb = rgb · t + display(air) · (1 − t) · a`, with `t = rl_clear()`
//   and `air = rl_aerial(0) / max(1 − t, 1e-4)`, in display space over the draped colour.
//
// Bind groups: 0 globals, 1 frame (lib/project.wgsl), 2 the light (`look::RLight`, binding 0, as
// lib/rlight.wgsl has it), the rig (binding 1) and the style (binding 2), 3 the tile's cover.

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

@group(2) @binding(1) var<uniform> lights: Lights;
@group(2) @binding(2) var<uniform> style: Style;
// the tile's cover (a transparent texel where it has none), premultiplied RGBA, row 0 north
@group(3) @binding(0) var cover: texture_2d<f32>;
@group(3) @binding(1) var cover_sampler: sampler;

struct Varyings {
    @builtin(position) clip: vec4<f32>,
    // the fragment in the render frame, relative to the view centre (common units)
    @location(0) rel: vec3<f32>,
    // where it is in its tile, tile units (`u` east, `v` south)
    @location(1) uv: vec2<f32>,
    // `rl_vertex`'s `eye` and `sun` (what the air needs)
    @location(2) eye: vec3<f32>,
    @location(3) sun: vec3<f32>,
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
    let local = vec3<f32>(position.xy, max(position.z, 0.0));
    let p = project_local(local);
    let rl = rl_vertex(p.rel, rl_mercator(local_offset(local.xy)));
    var out: Varyings;
    out.clip = p.clip;
    out.rel = p.rel;
    out.uv = position.xy;
    out.eye = rl.eye;
    out.sun = rl.sun;
    return out;
}

// The draped colour over the mesh colour, lit (uniform control flow: the derivatives and the
// cover's sample come before any branch).
fn surface(in: Varyings) -> vec4<f32> {
    let normal = unit(cross(dpdx(in.rel), -dpdy(in.rel)));
    let pixel = textureSample(cover, cover_sampler, in.uv);
    let color = pixel + style.color * (1.0 - pixel.a);
    let rgb = shade(color.rgb, globals.camera_render, in.rel, normal, lights.key_render);
    return vec4<f32>(rgb, color.a * style.opacity);
}

@fragment
fn fs(in: Varyings) -> @location(0) vec4<f32> {
    return surface(in);
}

// The realistic map's: with the air between the surface and the camera (`TERRAIN_AIR`).
@fragment
fn fs_air(in: Varyings) -> @location(0) vec4<f32> {
    let lit = surface(in);
    let rl = RlVary(in.eye, in.sun, vec2<f32>(0.0), vec2<f32>(0.0));
    let t = rl_clear(rl);
    let air = rl_aerial(rl, vec3<f32>(0.0)) / max(1.0 - t, 1e-4);
    return vec4<f32>(lit.rgb * t + rl_display(air) * (1.0 - t) * lit.a, lit.a);
}
