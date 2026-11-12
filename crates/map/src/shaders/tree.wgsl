//!include lib/rlight.wgsl
// tree.wgsl: the realistic map's trees (doc 04 §8.3-8.4; `realistic.ts`'s `rtreeModule` and
// `rtreeInject`, `R:765-829`, on `tree-layer.ts`'s SimpleMeshLayer of `tree-models.ts`'s cards):
// Streets GL's picture of a whole tree on two crossed cards, its shape cut out, lit by
// `lib/rlight.wgsl` as a rounded crown (the crown's volume normal map with the leaves' own on it),
// in the season's colours. The Rust side is `layers::trees`; `shaders/tree_shadow.wgsl` lays the
// same cards on the ground as the trees' shadows.
//
// The vertex is deck's SimpleMeshLayer on the flat map (trees are drawn from zoom 14 only):
// the instance's place on the ground (`project_local` of its tile units) plus the card's corner
// through the instance's 3×3 matrix, metres east, north and up, in common units at the view
// centre's scale (`project_size` with `WEB_MERCATOR_AUTO_OFFSET`: `globals.centre.z`); the card's
// normal through the same matrix (`project_normal`), east, north and up on the flat map (`rt_n`).
//
// What differs from the GLSL, and why (doc 04 §16.2):
// - WGSL takes derivatives and implicit-LOD samples only in uniform control flow, and the GLSL
//   discards the fragments outside the picture's shape before it samples the colour and the
//   normals and takes the derivatives of the eye vector and the UV. So every sample and
//   derivative is taken first, and the `discard` comes at the end, where the GLSL's would have
//   left the fragment out: the same fragments, the same colour;
// - `retint ? a : b` is a `select` of both;
// - `textureLod(rtree_color, vec3(0.5, 0.5, layer), 16.0)` (the picture's mean colour) is
//   `textureSampleLevel` at level 16, clamped to the last level as there;
// - `mod(code, 10.0)` is written out; `smoothstep(0.95, 0.35, uv.y)` is `1 − smoothstep(0.35,
//   0.95, uv.y)`;
// - `if (dot(card, v) < 0.0) card = -card;` is a `select`.

// `textures::TreeTextures`: the trees' colour (sRGB), normal and mask arrays, 5 layers in
// `textures::TREE_TEXTURES`' order (repeat both ways, anisotropy 8), the crown's volume normal
// map (clamped), and what the shadows take (`tree_shadow.wgsl`).
@group(3) @binding(0) var tree_color: texture_2d_array<f32>;
@group(3) @binding(1) var tree_normal: texture_2d_array<f32>;
@group(3) @binding(2) var tree_mask: texture_2d_array<f32>;
@group(3) @binding(3) var tree_volume: texture_2d<f32>;
@group(3) @binding(4) var tree_sampler: sampler;
@group(3) @binding(5) var volume_sampler: sampler;

// `scene::trees::card_arrays`: a card's corner (metres across and up for a unit tree), its normal
// and its picture's UV (v 0 at the top of the tree).
struct CardIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

// `layers::trees::TreeInstance`: where the tree stands (its tile's units, v south), its matrix's
// columns (metres east, north, up), its colour and code (alpha `200 + layer·10 + 5 if retinted`).
struct TreeIn {
    @location(3) place: vec2<f32>,
    @location(4) c0: vec3<f32>,
    @location(5) c1: vec3<f32>,
    @location(6) c2: vec3<f32>,
    @location(7) colour: vec4<f32>,
}

struct TreeOut {
    @builtin(position) clip: vec4<f32>,
    // `rl_eye`, `rl_sun`, `rl_t`, `rl_c`
    @location(0) eye: vec3<f32>,
    @location(1) sun: vec3<f32>,
    @location(2) t: vec2<f32>,
    @location(3) c: vec2<f32>,
    // `geometry.uv`, `rt_n`
    @location(4) uv: vec2<f32>,
    @location(5) normal: vec3<f32>,
    // the instance's colour (deck's `vColor`)
    @location(6) @interpolate(flat) colour: vec4<f32>,
}

@vertex
fn vs_tree(card: CardIn, tree: TreeIn) -> TreeOut {
    let m = mat3x3<f32>(tree.c0, tree.c1, tree.c2);
    let ground = project_local(vec3<f32>(tree.place, 0.0));
    // (`project_size`: metres to common units at the view centre)
    let offset = (m * card.position) * globals.centre.z;
    let rel = ground.rel + offset;
    let rl = rl_vertex(rel, rl_mercator(local_offset(tree.place) + offset.xy));
    var out: TreeOut;
    out.clip = globals.centre_cam * vec4<f32>(rel, 1.0);
    out.eye = rl.eye;
    out.sun = rl.sun;
    out.t = rl.t;
    out.c = rl.c;
    out.uv = card.uv;
    out.normal = m * card.normal;
    out.colour = tree.colour;
    return out;
}

// The instance's alpha: which picture, and whether its colour is the season's.
fn tree_code(colour: vec4<f32>) -> f32 {
    return floor(colour.a * 255.0 + 0.5) - 200.0;
}

fn tree_layer(code: f32) -> i32 {
    return i32(floor(code / 10.0 + 0.01));
}

@fragment
fn fs_tree(f: TreeOut) -> @location(0) vec4<f32> {
    let rl = RlVary(f.eye, f.sun, f.t, f.c);
    let color = f.colour;
    let code = tree_code(color);
    let layer = tree_layer(code);
    let retint = code - 10.0 * floor(code / 10.0) > 2.5;
    // every sample and derivative first (uniform control flow), the shape's cut last
    let mask = textureSample(tree_mask, tree_sampler, f.uv, layer).r;
    let leaf = textureSample(tree_color, tree_sampler, f.uv, layer).rgb;
    let mean = textureSampleLevel(tree_color, tree_sampler, vec2<f32>(0.5), layer, 16.0).rgb;
    let volume = textureSample(tree_volume, volume_sampler, f.uv).xyz;
    let leaves = textureSample(tree_normal, tree_sampler, f.uv, layer).xyz;
    let dp1 = dpdx(-f.eye);
    let dp2 = dpdy(-f.eye);
    let duv1 = dpdx(f.uv);
    let duv2 = dpdy(f.uv);

    var albedo = select(leaf * pow(color.r / 0.8627, 2.2), leaf * rl_linear(color.rgb) / max(mean, vec3<f32>(1e-3)), retint);
    let v = normalize(f.eye);
    // the card's face towards the camera, and the way the picture runs across and up it there,
    // from how its texture coordinates change from pixel to pixel (Streets GL's getTBN): the
    // picture's left is on the same side of the tree from either side of the card, as its top is
    let n0 = normalize(f.normal);
    let card = select(n0, -n0, dot(n0, v) < 0.0);
    let dp2perp = cross(dp2, card);
    let dp1perp = cross(card, dp1);
    let facing = sign(dot(card, cross(dp1, dp2)));
    let across = (dp2perp * duv1.x + dp1perp * duv2.x) * facing;
    // (v runs down the picture)
    let up = -(dp2perp * duv1.y + dp1perp * duv2.y) * facing;
    let scale = inverseSqrt(max(max(dot(across, across), dot(up, up)), 1e-20));
    // reoriented normal mapping: the leaves' normals on the crown's
    let t = volume * 2.0 - 1.0 + vec3<f32>(0.0, 0.0, 1.0);
    let u = (leaves * 2.0 - 1.0) * vec3<f32>(-1.0, -1.0, 1.0);
    let m = normalize(t * dot(t, u) - u * t.z);
    let n = normalize((across * m.x + up * m.y) * scale + card * max(m.z, 0.05));
    let sun = rl_sun_visible(rl);
    // snow on the tops of the crowns; the inside and underneath shaded
    albedo = mix(albedo, vec3<f32>(0.75), rlight.snow * smoothstep(0.2, 0.7, n.z) * 0.8);
    let ao = mix(0.6, 1.0, 1.0 - smoothstep(0.35, 0.95, f.uv.y));
    var lit = rl_shade(rl, albedo, n, v, 0.9, 0.0, ao, sun, 1.0);
    // the sun through the leaves, seen from the shaded side
    lit += albedo * rl_sun_light(rl) * 0.3 * pow(max(dot(-v, rl_to_sun(rl)), 0.0), 3.0) * sun;
    if (mask < 0.5) {
        discard;
    }
    return vec4<f32>(rl_display(rl_aerial(rl, lit)), 1.0);
}
