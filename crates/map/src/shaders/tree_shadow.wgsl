//!include lib/rlight.wgsl
// tree_shadow.wgsl: the realistic map's trees' shadows (doc 04 §8.5; `tree-layer.ts`'s
// `treeShadows` and `realistic.ts`'s `TreeShadowExtension`, `R:848-890`): each tree's two cards
// laid flat on the ground away from the sun, each point as far along as its height throws it
// (`shadowStep`, as a building's), 5 cm over the ground, cut to the picture's shape by the trees'
// mask, in the shadow colour fading into the distance's air (`rl_clear`). Drawn with the stencil's
// shadow rule (`SHADOW_PARAMETERS`) before the tile's trees. The Rust side is `layers::trees`.
//
// What differs from today's app, invisibly: `treeShadows` makes each tile's sheared matrices on
// the CPU, again for each half degree the sun moves; here the vertex shader shears the trees' own
// instances by `trees.step`, the step of the sun the layer holds until it has moved half a degree
// (so every tile's shadows are of that one sun, where today a tile seen again later is cast with
// the sun of that moment, at most half a degree on). The mask is sampled before the `discard`
// (uniform control flow, doc 04 §16.2).

@group(3) @binding(2) var tree_mask: texture_2d_array<f32>;
@group(3) @binding(4) var tree_sampler: sampler;

// `layers::trees::TreeUniform`: the shadows' colour (straight RGBA, 0 to 1:
// `[6, 12, 28, round(min(0.6, 0.55·sunStrength)·255)]`) and where the shadow of a point a metre
// up lands, metres east and north (`shadowStep(look.sunDir)`).
struct TreeUniform {
    shadow: vec4<f32>,
    step: vec4<f32>,
}

@group(3) @binding(6) var<uniform> trees: TreeUniform;

// As `tree.wgsl`'s.
struct CardIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct TreeIn {
    @location(3) place: vec2<f32>,
    @location(4) c0: vec3<f32>,
    @location(5) c1: vec3<f32>,
    @location(6) c2: vec3<f32>,
    @location(7) colour: vec4<f32>,
}

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) eye: vec3<f32>,
    @location(1) sun: vec3<f32>,
    @location(2) t: vec2<f32>,
    @location(3) c: vec2<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) @interpolate(flat) colour: vec4<f32>,
}

// `[x, y, z]` to `[x + z·east, y + z·north, 0]` after the tree's own transform.
fn laid(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.z * trees.step.x, c.y + c.z * trees.step.y, 0.0);
}

@vertex
fn vs_tree_shadow(card: CardIn, tree: TreeIn) -> ShadowOut {
    let m = mat3x3<f32>(laid(tree.c0), laid(tree.c1), laid(tree.c2));
    let ground = project_local(vec3<f32>(tree.place, 0.0));
    // (the trees' translation is none: the shadow's is 5 cm up)
    let offset = (m * card.position + vec3<f32>(0.0, 0.0, 0.05)) * globals.centre.z;
    let rel = ground.rel + offset;
    let rl = rl_vertex(rel, rl_mercator(local_offset(tree.place) + offset.xy));
    var out: ShadowOut;
    out.clip = globals.centre_cam * vec4<f32>(rel, 1.0);
    out.eye = rl.eye;
    out.sun = rl.sun;
    out.t = rl.t;
    out.c = rl.c;
    out.uv = card.uv;
    out.colour = tree.colour;
    return out;
}

@fragment
fn fs_tree_shadow(f: ShadowOut) -> @location(0) vec4<f32> {
    let rl = RlVary(f.eye, f.sun, f.t, f.c);
    let code = floor(f.colour.a * 255.0 + 0.5) - 200.0;
    let mask = textureSample(tree_mask, tree_sampler, f.uv, i32(floor(code / 10.0 + 0.01))).r;
    if (mask < 0.5) {
        discard;
    }
    return vec4<f32>(trees.shadow.rgb, trees.shadow.a * rl_clear(rl));
}
