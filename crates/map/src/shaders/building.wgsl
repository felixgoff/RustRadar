//!include lib/rlight.wgsl
// building.wgsl: the realistic map's buildings (doc 04 §7.3; `realistic.ts`'s `rbuildModule`,
// `rbuildInject` and `roofInject`, `R:559-763`): walls with Streets GL's façades, a window to each
// bay and whole storeys, flat roofs in their roofing, lit by `lib/rlight.wgsl`, the windows lit at
// night. The Rust side is `layers::buildings_realistic`; the geometry is
// `scene::tess::buildings::tessellate_realistic` (`RealisticVertex`).
//
// Entry points:
// - `vs_building` + `fs_building`: an extruded tile's walls and flat roofs (deck's extruded
//   SolidPolygonLayer with `BuildingExtension`);
// - `vs_building` + `fs_flat`: a far tile's footprints flat on the ground in each building's colour
//   and alpha, unlit (deck's `extruded: false`, where the extension is off and `UNLIT` leaves
//   `fillColors` as it is);
// - `vs_roof` + `fs_roof`: pitched roofs' meshes (item R6; `RoofExtension`'s `roofInject`): the
//   same vertex, its tangent across the slope (`cross(up, n)`) and no foot shading (`rb_z` 10), the
//   same fragment not clipped to the tile (the roofs' SimpleMeshLayer has no `ClipExtension`).
//
// What differs from the GLSL, and why (doc 04 §16.2):
// - deck's varyings `rb_uv`, `rb_n`, `rb_z` and `rb_info` are worked out on the CPU per vertex
//   (`RealisticVertex`: the wall's bays and storeys in `uv`, the façade, roof and kind in `style`,
//   the building's `fillColors` in `tint`), so the vertex shader only passes them on; a wall's
//   tangent (`rb_along / |rb_along|`) is its normal turned a quarter, `cross(up, n)` =
//   `(−n.y, n.x)`: deck's side of a counter-clockwise ring runs from its edge's end back to its
//   start (`pos` the next vertex), and `RealisticVertex::uv` grows the same way;
// - WGSL takes derivatives and implicit-LOD samples only in uniform control flow, and the GLSL
//   samples its window and plain-wall layers inside a branch on the fragment's façade. So the
//   texture coordinates' derivatives are taken once, after the wall and roof branches have joined
//   (where the GLSL takes `dFdx(uv)`), and every sample is `textureSampleGrad` at them; the
//   GLSL's mip `bias` (`detail`, `clamp(log2(dist / 1500 m), 0, 4)`) scales the gradients by
//   `2^detail`, which raises the level by `detail` as the bias does;
// - `textureLod(rbuild_glow, vec3(0.5, 0.5, layer), 16.0)` (a layer's mean glow) is
//   `textureSampleLevel` at level 16, clamped to the last level as there;
// - `mod(code, 5.0)` and `floor(code / 5.0)` are the façade and roof as integers;
// - the tile's clip (MVTLayer's `ClipExtension` by fragment, `clip_isInBounds` on the tile's
//   `[0, 1]` square) is a `discard` at the end, after every derivative.

// `textures::BuildingTextures`: the colour (sRGB), normal, mask (R roughness, G metalness, B how
// much the building's own colour tints it) and glow (R window light) arrays, 16 layers in
// `textures::BUILDING_TEXTURES`' order, and their sampler (repeat both ways, anisotropy 8).
@group(3) @binding(0) var building_color: texture_2d_array<f32>;
@group(3) @binding(1) var building_normal: texture_2d_array<f32>;
@group(3) @binding(2) var building_mask: texture_2d_array<f32>;
@group(3) @binding(3) var building_glow: texture_2d_array<f32>;
@group(3) @binding(4) var building_sampler: sampler;

// `RealisticVertex::style`'s fourth byte
const STYLE_WALL: u32 = 1u;
const STYLE_BAYS: u32 = 2u;
// ...and its third, for a roof: the face's own coordinates, and its own mapped colour
const ROOF_FACE: u32 = 1u;
const ROOF_OWN_COLOUR: u32 = 2u;
const UP: vec3<f32> = vec3<f32>(0.0, 0.0, 1.0);

// `scene::tess::buildings::RealisticVertex`.
struct BuildingIn {
    // tile units (v south) and metres up
    @location(0) position: vec3<f32>,
    // east, north, up
    @location(1) normal: vec3<f32>,
    // `rb_uv`
    @location(2) uv: vec2<f32>,
    // deck's `fillColors`: the building's colour and alpha (255 less façade + 5 · roof)
    @location(3) tint: vec4<f32>,
    // façade, roof, the roof's kind, wall and bay bits
    @location(4) style: vec4<u32>,
}

struct BuildingOut {
    @builtin(position) clip: vec4<f32>,
    // `rl_eye`, `rl_sun`, `rl_t`, `rl_c`
    @location(0) eye: vec3<f32>,
    @location(1) sun: vec3<f32>,
    @location(2) t: vec2<f32>,
    @location(3) c: vec2<f32>,
    // `rb_uv`, `rb_n`, `rb_tangent`
    @location(4) uv: vec2<f32>,
    @location(5) normal: vec3<f32>,
    @location(6) tangent: vec3<f32>,
    // the tile units (for the clip) and `rb_z`
    @location(7) tile: vec3<f32>,
    // `rb_tint` (with deck's alpha) and the style
    @location(8) @interpolate(flat) tint: vec4<f32>,
    @location(9) @interpolate(flat) style: vec4<u32>,
}

fn building_out(v: BuildingIn, tangent: vec3<f32>, z: f32) -> BuildingOut {
    let p = project_local(v.position);
    let rl = rl_vertex(p.rel, rl_mercator(local_offset(v.position.xy)));
    var out: BuildingOut;
    out.clip = p.clip;
    out.eye = rl.eye;
    out.sun = rl.sun;
    out.t = rl.t;
    out.c = rl.c;
    out.uv = v.uv;
    out.normal = v.normal;
    out.tangent = tangent;
    out.tile = vec3<f32>(v.position.xy, z);
    out.tint = v.tint;
    out.style = v.style;
    return out;
}

// A wall: along it, against the way round its outline goes (deck's `rb_along`, from the next
// corner to this one), and its height above the ground; a roof: east, at the roof's height.
@vertex
fn vs_building(v: BuildingIn) -> BuildingOut {
    var tangent = vec3<f32>(1.0, 0.0, 0.0);
    if ((v.style.w & STYLE_WALL) != 0u) {
        tangent = vec3<f32>(-v.normal.y, v.normal.x, 0.0) / max(length(v.normal.xy), 1e-6);
    }
    return building_out(v, tangent, v.position.z);
}

// A pitched roof's mesh (R6, `roofInject`): its tangent across the slope, no foot shading.
@vertex
fn vs_roof(v: BuildingIn) -> BuildingOut {
    return building_out(v, normalize(cross(UP, v.normal) + vec3<f32>(1e-6, 0.0, 0.0)), 10.0);
}

// MVTLayer's tile clip: the fragment's place, straight down, within the tile's square (u in
// [0, 1), v, south, in (0, 1]).
fn outside_tile(tile: vec3<f32>) -> bool {
    return tile.x < 0.0 || tile.x >= 1.0 || tile.y <= 0.0 || tile.y > 1.0;
}

@fragment
fn fs_building(b: BuildingOut) -> @location(0) vec4<f32> {
    return building_colour(b, true);
}

// A pitched roof's mesh (R6): drawn whole, over the tile's edge too.
@fragment
fn fs_roof(b: BuildingOut) -> @location(0) vec4<f32> {
    return building_colour(b, false);
}

// The building shader's colour (`rbuildInject`'s fragment part), cut at the tile's square where
// `clipped`.
fn building_colour(b: BuildingOut, clipped: bool) -> vec4<f32> {
    let rl = RlVary(b.eye, b.sun, b.t, b.c);
    let facade = f32(b.style.x);
    let roof = f32(b.style.y);
    let wall = (b.style.w & STYLE_WALL) != 0u;
    let bays = (b.style.w & STYLE_BAYS) != 0u;
    // the building's own choices from its colour: the same all round it
    let which = fract(dot(b.tint.rgb, vec3<f32>(71.23, 43.17, 36.19)));
    var tint = rl_linear(b.tint.rgb);
    var layer: f32;
    var uv: vec2<f32>;
    if (wall) {
        // plaster, brick, block, wood: the wall alone, or a window to each bay; glass towers all glass
        layer = select(facade * 2.0 + select(0.0, 1.0, bays), 8.0, facade > 3.5);
        uv = b.uv;
    } else {
        // tar, concrete, metal, tiles, eternit and two of Streets GL's flat roofs, each its metres a
        // tile (whole tiles to the textures' period, so they meet where it repeats)
        layer = 9.0 + roof;
        var metres = 18.0;
        if (roof < 0.5) {
            metres = 4.0;
        } else if (roof < 1.5) {
            metres = 10.0;
        } else if (roof < 2.5) {
            metres = 4.0;
        } else if (roof < 3.5) {
            metres = 3.0;
        } else if (roof < 4.5) {
            metres = 5.0;
        }
        // a pitched roof's (R6) lie along its own face, in metres
        if (b.style.z >= ROOF_FACE) {
            uv = b.uv / metres;
        } else {
            uv = rl.t * max(1.0, floor(rlight.period_metres / metres + 0.5));
        }
        // Streets GL's roof colours: tiles terracotta (or slate), metal pale grey-blue (or copper),
        // concrete grey; its flat roofs a little under their textures' white, so the sun doesn't
        // burn them out; tar and eternit darker
        if (roof > 2.5 && roof < 3.5) {
            tint = select(vec3<f32>(0.262, 0.262, 0.309), vec3<f32>(0.597, 0.205, 0.127), which < 0.75);
        } else if (roof > 1.5 && roof < 2.5) {
            tint = select(vec3<f32>(0.366, 0.59, 0.509), vec3<f32>(0.546, 0.644, 0.723), which < 0.8);
        } else if (roof > 0.5 && roof < 1.5) {
            tint = vec3<f32>(0.497);
        } else if (roof > 4.5) {
            tint = vec3<f32>(0.8);
        } else {
            tint = vec3<f32>(0.55);
        }
        // one with its own colour mapped
        if (b.style.z >= ROOF_OWN_COLOUR) {
            tint = rl_linear(b.tint.rgb);
        }
    }
    // (the images are stored top row first: v runs up them)
    let at = vec2<f32>(uv.x, -uv.y);
    // the textures coarser further off (past a kilometre and a half, a mip level each doubling)
    let detail = clamp(log2(length(rl.eye) / 1500.0), 0.0, 4.0);
    // how big a bay and a storey are on screen: the side, in pixels, of a square of the same area
    // (here, where the wall and roof branches have joined: the derivatives hold)
    let du = dpdx(uv);
    let dv = dpdy(uv);
    let cell_px = inverseSqrt(max(abs(du.x * dv.y - du.y * dv.x), 1e-12));
    // windows from 10 px a bay, fading out under that, gone under 5 px: there the plain wall
    let near = smoothstep(5.0, 10.0, cell_px);
    // the samples' gradients: the coordinates' own, the bias as a scale (`2^detail`)
    let bias = exp2(detail);
    let gx = vec2<f32>(du.x, -du.y) * bias;
    let gy = vec2<f32>(dv.x, -dv.y) * bias;
    let index = i32(layer);
    var c = textureSampleGrad(building_color, building_sampler, at, index, gx, gy).rgb;
    var nm = textureSampleGrad(building_normal, building_sampler, at, index, gx, gy).xyz * 2.0 - 1.0;
    var m = textureSampleGrad(building_mask, building_sampler, at, index, gx, gy).rgb;
    // a wall with windows (the same all over each wall)
    if (wall && facade < 3.5 && bays) {
        let plain = i32(facade * 2.0);
        c = mix(textureSampleGrad(building_color, building_sampler, at, plain, gx, gy).rgb, c, near);
        nm = mix(textureSampleGrad(building_normal, building_sampler, at, plain, gx, gy).xyz * 2.0 - 1.0, nm, near);
        m = mix(textureSampleGrad(building_mask, building_sampler, at, plain, gx, gy).rgb, m, near);
    }
    var albedo = c * mix(vec3<f32>(1.0), tint, m.b);
    if (!wall) {
        albedo = mix(albedo, vec3<f32>(0.8), rlight.snow * 0.92);
    }
    // the texture's u runs along the wall (or east), its v up the wall (or north)
    let n_face = normalize(b.normal);
    let t_face = normalize(b.tangent - n_face * dot(b.tangent, n_face) + vec3<f32>(1e-6));
    var b_face = UP;
    if (!wall) {
        b_face = normalize(cross(n_face, t_face));
    }
    let n = normalize(t_face * nm.x + b_face * nm.y + n_face * max(nm.z, 0.2));
    let v = normalize(rl.eye);
    // the ground shades the foot of a wall; rain makes everything shinier
    var ao = 1.0;
    if (wall) {
        ao = mix(0.55, 1.0, smoothstep(0.0, 2.5, b.tile.z));
    }
    let roughness = mix(m.r, 0.18, rlight.wet * 0.5);
    var lit = rl_shade(rl, albedo, n, v, roughness, m.g, ao, rl_sun_visible(rl), 1.0);
    // at night about half the windows are lit, warm, some brighter; where they are too small to tell
    // apart, their light evened out over the wall (the windows' share of it, lit on average)
    if (rlight.night > 0.0) {
        let pick = rl_hash(floor(uv) + b.tint.xy * 41.0, 1021.0);
        let glow = textureSampleGrad(building_glow, building_sampler, at, index, gx, gy).r;
        let each = glow * step(0.48, pick) * mix(vec3<f32>(1.0, 0.62, 0.3), vec3<f32>(1.0, 0.84, 0.62), step(0.85, pick));
        let evened = textureSampleLevel(building_glow, building_sampler, vec2<f32>(0.5, 0.5), index, 16.0).r * vec3<f32>(0.52, 0.355, 0.204);
        lit += mix(evened, each, near) * rlight.night * 0.28;
    }
    if (clipped && outside_tile(b.tile)) {
        discard;
    }
    return vec4<f32>(rl_display(rl_aerial(rl, lit)), 1.0);
}

// A far tile's footprint: the building's colour and alpha as they are (unlit).
@fragment
fn fs_flat(b: BuildingOut) -> @location(0) vec4<f32> {
    if (outside_tile(b.tile)) {
        discard;
    }
    return b.tint;
}
