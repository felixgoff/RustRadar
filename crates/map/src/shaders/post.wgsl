// post.wgsl: what is drawn over the realistic map after its pass (item R11; doc 01 §1.4, doc 04 §10).
//
// The tilt-shift (`Globe.svelte`'s two `.tilt-shift` layers, G:3781-3785, 3818-3828): CSS
// `backdrop-filter: blur(2px)` reaching 42 % of the height in from the top and the bottom, then
// `blur(5px)` reaching 22 %, each masked by `linear-gradient(black, transparent R, transparent
// calc(100% - R), black)`. A layer's picture is the one before it blurred (a Gaussian of that
// standard deviation, its edges mirrored), mixed into it by the mask: 1 at the top and bottom rows,
// falling straight to 0 at R in from them. The second layer blurs what the first left. Each blur is
// two passes, across then down (`fs_across`, `fs_down`); the last down pass is drawn onto the
// surface (`fs_final`), with the composite's sRGB rule (composite.wgsl). The taps are pairs of
// texels read in one bilinear sample (`post::Blur`), at the map's texels.
//
// The rain (`RainOverlay.svelte`, a 2D canvas over the map): each drop a straight streak of a
// line width with butt ends, its coverage of each pixel worked out from its distance to the
// streak's line and along it (`vs_rain`, `fs_rain`), blended over the surface as the canvas is.

// `post::BlurUniform`
struct Blur {
    // x: the tap's offset in texels, y: its weight (pairs of texels in one bilinear sample)
    taps: array<vec4<f32>, 32>,
    // 1 / the source's size (texels)
    texel: vec2<f32>,
    // how far the mask reaches in from the top and the bottom, 0 to 0.5 of the height
    reach: f32,
    // how many of `taps` are used
    count: u32,
    // the size of what is drawn (pixels)
    target_size: vec2<f32>,
    // bit 0: the surface is sRGB, linearise (fs_final)
    flags: u32,
    pad: u32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var under: texture_2d<f32>;
@group(0) @binding(2) var mirrored: sampler;
@group(0) @binding(3) var<uniform> blur: Blur;

// One triangle that covers the screen.
@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// The sRGB decoding curve, exactly (composite.wgsl's).
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

// `source` blurred along `direction` (texels) at `uv`.
fn blurred(uv: vec2<f32>, direction: vec2<f32>) -> vec3<f32> {
    var sum = textureSampleLevel(source, mirrored, uv, 0.0).rgb * blur.taps[0].y;
    for (var i = 1u; i < blur.count; i = i + 1u) {
        let tap = blur.taps[i];
        let offset = direction * blur.texel * tap.x;
        sum = sum + (textureSampleLevel(source, mirrored, uv + offset, 0.0).rgb + textureSampleLevel(source, mirrored, uv - offset, 0.0).rgb) * tap.y;
    }
    return sum;
}

// The mask's alpha at `y` (0 at the top, 1 at the bottom, at the pixel's centre): the linear
// gradient black (1) at the edges, transparent (0) from `reach` in.
fn mask(y: f32) -> f32 {
    let edge = min(y, 1.0 - y);
    return clamp(1.0 - edge / blur.reach, 0.0, 1.0);
}

// The first pass of a blur: across.
@fragment
fn fs_across(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / blur.target_size;
    return vec4<f32>(blurred(uv, vec2<f32>(1.0, 0.0)), 1.0);
}

// The second pass of the first layer: down, mixed into the picture under it by the mask.
@fragment
fn fs_down(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / blur.target_size;
    let picture = textureSampleLevel(under, mirrored, uv, 0.0).rgb;
    return vec4<f32>(mix(picture, blurred(uv, vec2<f32>(0.0, 1.0)), mask(uv.y)), 1.0);
}

// The second pass of the last layer, onto the surface: as `fs_down`, then the composite's rule.
@fragment
fn fs_final(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / blur.target_size;
    let picture = textureSampleLevel(under, mirrored, uv, 0.0).rgb;
    var rgb = mix(picture, blurred(uv, vec2<f32>(0.0, 1.0)), mask(uv.y));
    if ((blur.flags & 1u) != 0u) {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}

// ---------------------------------------------------------------------------------------- rain

// `post::RainUniform`
struct Rain {
    // the surface's size, pixels
    target_size: vec2<f32>,
    // bit 0: the surface is sRGB, linearise
    flags: u32,
    pad: u32,
}

// (a binding of its own number, so no two resources of the module share one)
@group(0) @binding(4) var<uniform> rain: Rain;

struct Streak {
    @builtin(position) position: vec4<f32>,
    // the pixel's place along the streak from its head, and across it from its line (pixels)
    @location(0) local: vec2<f32>,
    // x: the streak's length, y: its line width (pixels)
    @location(1) @interpolate(flat) size: vec2<f32>,
    @location(2) @interpolate(flat) colour: vec4<f32>,
}

// One streak (`post::StreakInstance`): from `head` to `tail` (surface pixels), `width` wide, in
// `colour` (straight alpha, 0–1). Six vertices: the streak's rectangle widened by a pixel all
// round, for the edges' coverage.
@vertex
fn vs_rain(
    @builtin(vertex_index) index: u32,
    @location(0) ends: vec4<f32>,
    @location(1) width: f32,
    @location(2) colour: vec4<f32>,
) -> Streak {
    let head = ends.xy;
    let tail = ends.zw;
    let length = distance(head, tail);
    var along = vec2<f32>(0.0, -1.0);
    if (length > 0.0) {
        along = (tail - head) / length;
    }
    let across = vec2<f32>(-along.y, along.x);
    // the corners of two triangles: (0,0) (1,0) (0,1) (0,1) (1,0) (1,1)
    let corners = array<vec2<f32>, 6>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0));
    let corner = corners[index];
    let half = width * 0.5 + 1.0;
    let t = mix(-1.0, length + 1.0, corner.x);
    let d = mix(-half, half, corner.y);
    let at = head + along * t + across * d;
    var out: Streak;
    out.position = vec4<f32>(at.x / rain.target_size.x * 2.0 - 1.0, 1.0 - at.y / rain.target_size.y * 2.0, 0.0, 1.0);
    out.local = vec2<f32>(t, d);
    out.size = vec2<f32>(length, width);
    out.colour = colour;
    return out;
}

// How much of a pixel at `x` (its centre) a span from `lo` to `hi` covers, along one axis.
fn covered(x: f32, lo: f32, hi: f32) -> f32 {
    return clamp(min(x + 0.5, hi) - max(x - 0.5, lo), 0.0, 1.0);
}

@fragment
fn fs_rain(in: Streak) -> @location(0) vec4<f32> {
    let half = in.size.y * 0.5;
    let coverage = covered(in.local.x, 0.0, in.size.x) * covered(in.local.y, -half, half);
    var rgb = in.colour.rgb;
    if ((rain.flags & 1u) != 0u) {
        rgb = srgb_to_linear(rgb);
    }
    return vec4<f32>(rgb, in.colour.a * coverage);
}
