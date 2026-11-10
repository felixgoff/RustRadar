//! Pure part of `realistic.ts` (S1.28; doc 04 §3, §7.2): the light the realistic map is drawn in.
//!
//! [`look_for`] turns the weather ([`Conditions`]) and the sun into a [`Look`]: the sun's light
//! reddened by the air it comes through ([`transmittance`]), the sky's colours, the haze and the
//! exposure. [`display_rgb`] takes a linear colour through the shaders' exposure and gamma for
//! colours worked out on the CPU; [`sky_gradient`] is the sky behind the map (the TypeScript's
//! `skyCss`, as gradient stops instead of CSS) and [`sky_view`] the inputs `Globe.svelte` gives it;
//! [`scene_sun`] and [`from_local`] the sun the map is lit by. [`building_style`] and
//! [`roof_colour`] pick each building's façade, roof and wall colour. The shader code (`rlight`
//! and the layer extensions) is the map crate's.
//!
//! Transliterated as written: `Math.min`/`Math.max` are `geom::js_min`/`js_max` (NaN stays NaN),
//! `Math.round` is [`js_round`], the glow's `toFixed(2)` alphas [`fixed_number`]; colours are linear `f64` triples, and
//! screen colours `u8` (they are whole numbers 0 to 255 in the TypeScript too).

use std::f64::consts::PI;

use rustradar_core::num::{clamp01, fixed_number, js_round};
use rustradar_core::weather::Conditions;

use crate::geom::{js_max, js_min};

/// A colour (linear light unless said otherwise), or a direction as east, north, up.
pub type Rgb = [f64; 3];

const DEG: f64 = PI / 180.0;

/// `smooth(a, b, x)`: 0 up to `a`, 1 from `b`, the cubic ease between.
fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = clamp01((x - a) / (b - a));
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn scale(a: Rgb, k: f64) -> Rgb {
    a.map(|v| v * k)
}

/// `Math.max(...c)`.
fn max3(c: Rgb) -> f64 {
    js_max(js_max(c[0], c[1]), c[2])
}

// --------------------------------------------------------------- the light

/// The light the realistic map is drawn in this frame (colours linear).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Towards the sun: east, north and up at the middle of the view.
    pub sun_dir: Rgb,
    /// The sine of the sun's altitude.
    pub sun_up: f64,
    /// The sun's light on a surface facing it (0 when clouds or the night hide it).
    pub sun_color: Rgb,
    /// The sky overhead and at the horizon, and the light thrown up by the ground.
    pub sky_zenith: Rgb,
    pub sky_horizon: Rgb,
    pub ground_color: Rgb,
    /// How strong the sky's light is on surfaces.
    pub ambient: f64,
    /// The air's colour in the distance, its extinction per metre, and from how far out it starts (fog).
    pub haze_color: Rgb,
    pub haze_density: f64,
    pub haze_start: f64,
    pub wet: f64,
    pub snow: f64,
    /// Shadows of drifting clouds, 0 to 1.
    pub cloud_shadow: f64,
    /// 0 by day to 1 at night: windows light up.
    pub night: f64,
    /// Brightening before the screen's gamma: under cloud a little, at night more.
    pub exposure: f64,
    /// 0 close in to 1 from far out over the globe, where each place has its own day: its sky's
    /// light only where the sun is up there, its towns' lights where it isn't.
    pub globe: f64,
}

/// The sun's light above the air, as the shading's units go.
pub const SUN: f64 = 3.2;
/// Optical depth of a clear atmosphere straight up (Rayleigh and aerosol), red, green, blue.
const TAU: Rgb = [0.13, 0.19, 0.33];
const DAY_ZENITH: Rgb = [0.07, 0.17, 0.48];
const DAY_HORIZON: Rgb = [0.42, 0.55, 0.75];
const DUSK_HORIZON: Rgb = [0.75, 0.38, 0.16];
const NIGHT_ZENITH: Rgb = [0.0048, 0.008, 0.019];
const NIGHT_HORIZON: Rgb = [0.014, 0.018, 0.032];
const OVERCAST_ZENITH: Rgb = [0.5, 0.52, 0.56];
const OVERCAST_HORIZON: Rgb = [0.62, 0.63, 0.66];
/// Under cloud at night, the towns' light comes back off it.
const CITY_GLOW: Rgb = [0.012, 0.008, 0.005];

/// The share of each colour of the sun's light that comes through the air at `alt_deg`, by
/// Kasten and Young's air mass.
pub fn transmittance(alt_deg: f64, haze: f64) -> Rgb {
    let alt = js_max(alt_deg, -1.0);
    let air_mass = 1.0 / ((js_max(alt, 0.0) * DEG).sin() + 0.50572 * (alt + 6.07995).powf(-1.6364));
    // hazier air dims and reddens it more
    let aerosol = js_min(1.2, haze * 2500.0);
    TAU.map(|tau| (-(tau + aerosol * 0.4) * (air_mass - 1.0) * 0.9).exp())
}

/// How bright a colour looks (Rec. 709's weights).
fn luminance(c: Rgb) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The daylight of a clear day with the sun 40° up, which the exposure is set for (Streets GL's:
/// no adjusting); dimmer scenes are taken brighter.
const CLEAR_DAYLIGHT: f64 = 1.84;
/// The clear air's own haze, per metre: Streets GL's atmosphere at the ground (about 2 % a kilometre).
const CLEAR_AIR: f64 = 2.2e-5;
/// How thick falling rain's veil is, against the camera's distance (fog's is up to 0.86).
const RAIN_MIST: f64 = 0.2;

/// `lookFor`'s default camera distance, metres (the TypeScript's default argument).
pub const DEFAULT_CAMERA_M: f64 = 1000.0;

/// The light in some weather with the sun towards `sun` (east, north, up), the camera
/// `camera_metres` from what it looks at ([`DEFAULT_CAMERA_M`] where the TypeScript left it out).
/// As Streets GL has it, the air is clear (a couple of percent a kilometre) whatever the clouds;
/// only fog thickens it, and never so much that the map gets hard to read: at most about a fifth
/// of the way to the middle of the view, more in fog, which only begins half way there.
pub fn look_for(c: &Conditions, sun: Rgb, camera_metres: f64) -> Look {
    let sun_up = js_max(-1.0, js_min(1.0, sun[2]));
    // from far out each place on the globe has its own day (the shaders light it): the light's
    // colours are the day's, as with the sun well up, whatever the time in the middle of the view
    let globe = smooth(150_000.0, 600_000.0, camera_metres);
    let real_alt = sun_up.asin() / DEG;
    let alt_deg = real_alt + (js_max(real_alt, 35.0) - real_alt) * globe;
    // daylight from 6° below the horizon (civil twilight) to full from 15° up
    let day = smooth(-6.0, 15.0, alt_deg);
    let sun_color = scale(transmittance(alt_deg, c.haze), SUN * c.sun * smooth(-0.8, 3.0, alt_deg));
    // the clear sky, warming at its horizon around sunset
    let dusk = clamp01(1.0 - (alt_deg - 1.0).abs() / 9.0) * (1.0 - c.cloud);
    let clear_zenith = mix(NIGHT_ZENITH, DAY_ZENITH, day);
    let clear_horizon = mix(mix(NIGHT_HORIZON, DAY_HORIZON, day), scale(DUSK_HORIZON, js_max(day, 0.15)), dusk * 0.75);
    // an overcast sky: grey, darker in rain, as bright as the day; at night lit a little by the towns below
    let dim = (0.12 + 0.88 * day) * (1.0 - 0.3 * c.wet) * day;
    let glow = scale(CITY_GLOW, 1.0 - day);
    let grey_zenith = {
        let s = scale(OVERCAST_ZENITH, dim);
        [s[0] + glow[0] * 0.5, s[1] + glow[1] * 0.5, s[2] + glow[2] * 0.5]
    };
    let grey_horizon = {
        let s = scale(OVERCAST_HORIZON, dim);
        [s[0] + glow[0], s[1] + glow[1], s[2] + glow[2]]
    };
    let sky_zenith = mix(clear_zenith, grey_zenith, c.cloud);
    let sky_horizon = mix(clear_horizon, grey_horizon, c.cloud);
    let sky_light = mix(sky_horizon, sky_zenith, 0.5);
    let night = 1.0 - smooth(-7.0, 1.0, alt_deg);
    // Streets GL's light from the sky: grey and the same from every side, a fifth of the sun's
    // on a clear day; under cloud the sky gives what the sun doesn't; at night a little, so the
    // streets are dim but there to see, not black
    let overcast = luminance(mix(OVERCAST_ZENITH, OVERCAST_HORIZON, 0.5)) * (1.0 - 0.3 * c.wet);
    let ambient = (0.2 + (overcast * 1.1 - 0.2) * c.cloud) * (0.15 + 0.85 * day) + 0.08 * night;
    // the ground throws back about a fifth of what falls on it
    let sun_on_ground = js_max(sun_up, 0.0);
    let ground_color = [0, 1, 2].map(|i| 0.2 * (sky_light[i] + sun_color[i] * sun_on_ground));
    // fog is the air itself lit: whiter than a hazy sky
    let fog = clamp01((c.haze - 3e-4) / 2e-3);
    let fog_colour = scale([0.7, 0.71, 0.72], js_max(day, 0.02));
    let haze_color = mix(sky_horizon, mix(sky_light, fog_colour, 0.5), fog);
    let camera = js_max(camera_metres, 50.0);
    // falling rain: a grey veil from some way out, thickening with the distance (lighter than fog)
    let rain_mist = c.rain * (1.0 - fog);
    // a clear day as Streets GL shows it; a grey one taken a little brighter, as a camera would (more so at night)
    let daylight = luminance(sun_color) * js_max(sun_up, 0.0) + ambient;
    let adapt = js_min(1.8, js_max(1.0, (CLEAR_DAYLIGHT / js_max(daylight, 1e-3)).sqrt()));
    Look {
        sun_dir: [sun[0], sun[1], sun_up],
        sun_up,
        sun_color,
        sky_zenith,
        sky_horizon,
        ground_color,
        ambient,
        haze_color,
        haze_density: js_max(
            js_min(js_min(c.haze, CLEAR_AIR) * (1.0 - fog) + c.haze * fog, (0.22 + 0.64 * fog) / camera),
            (rain_mist * RAIN_MIST) / camera,
        ),
        haze_start: js_max(fog * 0.5, rain_mist * 0.3) * camera,
        wet: c.wet,
        snow: c.snow,
        cloud_shadow: c.cloud_shadows * 0.85,
        night,
        exposure: adapt * (1.0 + 2.2 * (1.0 - day)),
        globe,
    }
}

/// The light before the map has set any (`realistic.ts`'s initial `look`): a fine day, the sun
/// to the east, south of it, 47° up.
impl Default for Look {
    fn default() -> Self {
        let fine = Conditions { sun: 1.0, cloud: 0.05, haze: 2e-5, wet: 0.0, snow: 0.0, cloud_shadows: 0.0, rain: 0.0 };
        look_for(&fine, [0.4, -0.55, 0.73], DEFAULT_CAMERA_M)
    }
}

/// A linear colour as the screen shows it, 0–255, through the same exposure and gamma as the
/// shaders (Streets GL's: no tone curve). The TypeScript's default exposure, the current look's,
/// is the caller's to pass. (A NaN channel comes out 0.)
pub fn display_rgb(linear: Rgb, exposure: f64) -> [u8; 3] {
    linear.map(|v| js_round(255.0 * clamp01(v * exposure).powf(1.0 / 2.2)) as u8)
}

// --------------------------------------------------------------- the sky behind the map

/// One stop of the sun's glow: at `radius` pixels from its centre, `rgb` (sRGB) at `alpha`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlowStop {
    pub radius: f64,
    pub rgb: [u8; 3],
    /// As CSS was given it: to two decimals.
    pub alpha: f64,
}

/// The sun's glow, a radial gradient drawn over the sky's (normal alpha blending), its colours
/// between the stops interpolated as CSS does, in sRGB bytes, transparent past the last.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunGlow {
    /// Its centre, pixels from the top left (whole pixels).
    pub at: [f64; 2],
    pub stops: [GlowStop; 4],
}

/// The sky behind the map (the TypeScript's `skyCss`, as stops instead of CSS): from
/// [`top`](Self::top) at the top of the screen down to [`horizon`](Self::horizon) at
/// [`horizon_percent`](Self::horizon_percent) of the way down, the same colour below that
/// (interpolated in sRGB bytes, as CSS does); over it the sun's glow when the sun is in view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyGradient {
    pub top: [u8; 3],
    pub horizon: [u8; 3],
    /// 0 (top) to 100 (bottom), a whole number.
    pub horizon_percent: f64,
    pub glow: Option<SunGlow>,
}

/// Space's colours, far out (at the top of the screen and at the horizon).
const SPACE_TOP: Rgb = [4.0, 7.0, 16.0];
const SPACE_HORIZON: Rgb = [12.0, 18.0, 34.0];

fn bytes(c: [u8; 3]) -> Rgb {
    c.map(f64::from)
}

/// The sky behind the map: `top` the elevation (degrees above the horizon) at the top of the
/// screen, `horizon` where the horizon is down it (0 top, 1 bottom), the sun's glow round
/// `sun_at` (pixels) when it is in view; far out, `space`, the globe hangs in space instead.
pub fn sky_gradient(l: &Look, top: f64, horizon: f64, sun_at: Option<[f64; 2]>, space: f64) -> SkyGradient {
    let elevation_colour = |deg: f64| mix(l.sky_horizon, l.sky_zenith, clamp01((deg * DEG).sin()).sqrt());
    let at_top = mix(bytes(display_rgb(elevation_colour(js_max(top, 0.0)), l.exposure)), SPACE_TOP, space).map(|v| js_round(v) as u8);
    let at_horizon = mix(bytes(display_rgb(l.haze_color, l.exposure)), SPACE_HORIZON, space).map(|v| js_round(v) as u8);
    let at = js_round(clamp01(horizon) * 100.0);
    let sun = max3(l.sun_color);
    let glow = match sun_at {
        Some(sun_at) if sun > 0.05 && space < 1.0 => {
            let glow = display_rgb([0, 1, 2].map(|i| l.sun_color[i] * 0.5 + l.sky_horizon[i]), l.exposure);
            let alpha = js_min(0.85, sun / SUN) * (1.0 - space);
            let [a, faint] = [fixed_number(alpha, 2), fixed_number(alpha * 0.55, 2)];
            Some(SunGlow {
                at: [js_round(sun_at[0]), js_round(sun_at[1])],
                stops: [
                    GlowStop { radius: 0.0, rgb: [255, 252, 240], alpha: a },
                    GlowStop { radius: 14.0, rgb: [255, 250, 235], alpha: a },
                    GlowStop { radius: 40.0, rgb: glow, alpha: faint },
                    GlowStop { radius: 320.0, rgb: glow, alpha: 0.0 },
                ],
            })
        }
        _ => None,
    };
    SkyGradient { top: at_top, horizon: at_horizon, horizon_percent: at, glow }
}

/// What the sky is drawn for (`Globe.svelte`'s `updateLook`, doc 04 §3.6): [`sky_gradient`]'s
/// `top`, `horizon`, `sun_at` and `space`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyView {
    pub top: f64,
    pub horizon: f64,
    pub sun_at: Option<[f64; 2]>,
    pub space: f64,
}

/// Over this zoom the map is flat, and the sun can be in view (`Globe.svelte`'s `FLAT_ABOVE_ZOOM`).
const FLAT_ABOVE_ZOOM: f64 = 12.0;

/// The sky's inputs for a view (`zoom`, `pitch` and `bearing` in degrees), the sun as east, north,
/// up ([`scene_sun`]), on a screen `width` × `height` CSS pixels. deck.gl's camera sees
/// 2·atan(1/3) (about 37°) top to bottom; space shows from zoom 11.5 out, wholly from 9.
pub fn sky_view(zoom: f64, pitch: f64, bearing: f64, sun: Rgb, width: f64, height: f64) -> SkyView {
    let half_fov = (1.0_f64 / 3.0).atan() / DEG;
    let tilt = js_max(1.0, pitch);
    let horizon = (1.0 - 3.0 / (tilt * DEG).tan()) / 2.0;
    // the sun on the screen, when it is in front of the camera
    let mut sun_at = None;
    if zoom > FLAT_ABOVE_ZOOM && sun[2] > -0.02 {
        let [sb, cb, sp, cp] = [(bearing * DEG).sin(), (bearing * DEG).cos(), (pitch * DEG).sin(), (pitch * DEG).cos()];
        // the camera's forward, right and up, as east, north, up
        let forward = [sb * sp, cb * sp, -cp];
        let right = [cb, -sb, 0.0];
        let up = [sb * cp, cb * cp, sp];
        let dot = |a: [f64; 3], b: Rgb| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let ahead = dot(forward, sun);
        if ahead > 0.05 {
            let f = 3.0 * (height / 2.0);
            sun_at = Some([width / 2.0 + (dot(right, sun) / ahead) * f, height / 2.0 - (dot(up, sun) / ahead) * f]);
        }
    }
    SkyView { top: tilt + half_fov - 90.0, horizon, sun_at, space: clamp01((11.5 - zoom) / 2.5) }
}

/// The sun the realistic map is lit by, as [east, north, up] at the view: the real one with day
/// and night on; otherwise always a fine afternoon, the sun in the south-west 35° up.
pub fn scene_sun(daylight: bool, real_sun: Rgb) -> Rgb {
    if daylight {
        return real_sun;
    }
    let (az, alt) = (215.0 * DEG, 35.0 * DEG);
    [az.sin() * alt.cos(), az.cos() * alt.cos(), alt.sin()]
}

/// `local` (east, north, up at `lat`, `lon`) as a direction from the Earth's centre, as
/// `core::sun::sun_direction` gives the sun's (the inverse of `core::sun::local_sun`).
pub fn from_local(local: Rgb, lat: f64, lon: f64) -> Rgb {
    let [sin_lat, cos_lat, sin_lon, cos_lon] = [(lat * DEG).sin(), (lat * DEG).cos(), (lon * DEG).sin(), (lon * DEG).cos()];
    let [e, n, u] = local;
    [
        -sin_lon * e - sin_lat * cos_lon * n + cos_lat * cos_lon * u,
        cos_lon * e - sin_lat * sin_lon * n + cos_lat * sin_lon * u,
        cos_lat * n + sin_lat * u,
    ]
}

// --------------------------------------------------------------- buildings' looks

/// Façades, in the order the shader has them.
pub mod facade {
    pub const PLASTER: u8 = 0;
    pub const BRICK: u8 = 1;
    /// Concrete block.
    pub const BLOCK: u8 = 2;
    pub const WOOD: u8 = 3;
    pub const GLASS: u8 = 4;
}

/// Roofs, in the order the shader has them: tar, concrete, metal, tiles, eternit, and two flat
/// roofs with their plant.
pub mod roof {
    pub const TAR: u8 = 0;
    pub const CONCRETE: u8 = 1;
    pub const METAL: u8 = 2;
    pub const TILES: u8 = 3;
    pub const ETERNIT: u8 = 4;
    /// Streets GL's `generic1`.
    pub const FLAT1: u8 = 5;
    /// Streets GL's `generic3`.
    pub const FLAT3: u8 = 6;
}

use facade::{BLOCK, BRICK, GLASS, PLASTER, WOOD};
use roof::{CONCRETE, ETERNIT, FLAT1, FLAT3, METAL, TAR, TILES};

/// How many façades there are: the alpha carries `façade + FACADES × roof`.
pub const FACADES: u8 = 5;

/// Each façade's colours (sRGB), by [`facade`]: its texture is near white where tinted, so these
/// are the walls'.
const WALL_COLOURS: [&[[u8; 3]]; 5] = [
    // plaster
    &[[238, 234, 222], [232, 220, 196], [226, 208, 172], [214, 214, 208], [236, 214, 196], [206, 220, 214], [240, 228, 180]],
    // brick
    &[[176, 92, 70], [150, 78, 60], [190, 130, 96], [128, 72, 58], [206, 176, 136]],
    // block
    &[[212, 210, 204], [196, 194, 188], [222, 216, 204]],
    // Falu red, ochre, white and grey: wooden houses as the north has them
    &[[150, 44, 32], [150, 44, 32], [214, 172, 92], [236, 234, 226], [168, 170, 166]],
    // glass
    &[[200, 214, 222], [190, 206, 200], [214, 206, 190]],
];

/// A building's façade ([`facade`]) and roof ([`roof`]) where its map data says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildingLook {
    pub facade: Option<u8>,
    pub roof: Option<u8>,
}

/// The colours OpenStreetMap's `building:colour` most often names, as #rrggbb.
fn named(name: &str) -> Option<&'static str> {
    Some(match name {
        "white" => "#f2f0ea",
        "black" => "#3a3a3a",
        "grey" | "gray" => "#9a9a9a",
        "lightgrey" | "lightgray" => "#c8c8c8",
        "darkgrey" | "darkgray" => "#6a6a6a",
        "silver" => "#c0c0c0",
        "red" => "#b04a3a",
        "maroon" => "#7a3028",
        "brown" => "#8a5a40",
        "tan" => "#d2b48c",
        "beige" => "#e6dcc0",
        "cream" => "#f0e6c8",
        "yellow" => "#e8d27a",
        "gold" => "#d8b860",
        "orange" => "#d88a50",
        "pink" => "#e0b0b0",
        "green" => "#8aa080",
        "blue" => "#8098b0",
        _ => return None,
    })
}

/// A hash of `seed`, 0 to 1, a different one for each `salt` (32-bit, as `Math.imul` and `>>>` have it).
fn hash(seed: i32, salt: i32) -> f64 {
    let mut h = (seed as u32 ^ (salt.wrapping_add(1) as u32).wrapping_mul(0x9e37_79b9)).wrapping_mul(0x85eb_ca6b);
    h = (h ^ (h >> 13)).wrapping_mul(0xc2b2_ae35);
    f64::from(h ^ (h >> 16)) / 4_294_967_296.0
}

/// One of `weights`' values, `r` (0 to 1) of the way through their weights.
fn pick_weighted(r: f64, weights: &[(u8, f64)]) -> u8 {
    let total = weights.iter().fold(0.0, |s, &(_, w)| s + w);
    let mut left = r * total;
    for &(value, w) in weights {
        if left < w {
            return value;
        }
        left -= w;
    }
    weights[weights.len() - 1].0
}

/// The roof `seed` picks for a building `height` metres high.
fn roof_for(seed: i32, height: f64) -> u8 {
    if height < 10.0 {
        pick_weighted(hash(seed, 3), &[(TILES, 0.55), (METAL, 0.2), (ETERNIT, 0.15), (TAR, 0.1)])
    } else {
        pick_weighted(hash(seed, 3), &[(TAR, 0.35), (CONCRETE, 0.25), (FLAT1, 0.2), (FLAT3, 0.15), (METAL, 0.05)])
    }
}

/// A building's roof as seen from far out, sRGB: its roof (as [`building_style`] picks it by
/// `seed` and height) in its texture's mean colour under the building shader's tint for it,
/// terracotta or slate tiles, grey-blue or copper metal, the greys of tar, concrete, eternit and
/// the flat roofs.
pub fn roof_colour(seed: i32, height: f64) -> [u8; 3] {
    let roof = roof_for(seed, height);
    let which = hash(seed, 6);
    match roof {
        TILES => {
            if which < 0.75 {
                [190, 117, 93]
            } else {
                [131, 131, 141]
            }
        }
        METAL => {
            if which < 0.8 {
                [169, 181, 190]
            } else {
                [140, 174, 163]
            }
        }
        TAR => [64, 64, 64],
        CONCRETE => [139, 139, 139],
        ETERNIT => [88, 86, 84],
        FLAT1 => [100, 99, 100],
        _ => [142, 144, 144],
    }
}

/// A building's look, picked by `seed` (the same each time it is loaded) and its height: its wall
/// colour (the one OpenStreetMap maps for it, `colour`, as a name or `#rrggbb`, toned down, where
/// it has one), and in the alpha its façade and roof (255 less façade + 5 × roof), which the
/// shader reads. `colour` is the TypeScript's `typeof colour === "string" ? colour : undefined`;
/// a mapped `look` must hold indices of [`facade`] and [`roof`].
pub fn building_style(seed: i32, height: f64, colour: Option<&str>, look: BuildingLook) -> [u8; 4] {
    let facade = look.facade.unwrap_or_else(|| {
        if height > 30.0 && hash(seed, 1) < 0.55 {
            GLASS
        } else {
            pick_weighted(
                hash(seed, 2),
                &[
                    (PLASTER, 0.42),
                    (BRICK, 0.28),
                    (BLOCK, if height > 12.0 { 0.25 } else { 0.1 }),
                    (WOOD, if height < 9.0 { 0.22 } else { 0.02 }),
                ],
            )
        }
    });
    let roof = look.roof.unwrap_or_else(|| roof_for(seed, height));
    let hex = colour.map(|c| named(&c.to_lowercase()).unwrap_or(c));
    let rgb = match hex.and_then(hex_rgb) {
        Some(v) => {
            let soften = |c: u32| js_round(f64::from(c) * 0.75 + 200.0 * 0.25) as u8;
            [soften(v >> 16), soften((v >> 8) & 255), soften(v & 255)]
        }
        None => {
            let list = WALL_COLOURS[usize::from(facade)];
            let base = list[(hash(seed, 4) * list.len() as f64).floor() as usize];
            // a little variety within each
            let j = js_round((hash(seed, 5) - 0.5) * 14.0);
            base.map(|c| js_min(255.0, js_max(0.0, f64::from(c) + j)) as u8)
        }
    };
    // (into the colour's bytes as deck.gl stores them: clamped)
    let code = (255 - (i32::from(facade) + i32::from(FACADES) * i32::from(roof))).clamp(0, 255) as u8;
    [rgb[0], rgb[1], rgb[2], code]
}

/// `/^#[0-9a-f]{6}$/i` and `parseInt(hex.slice(1), 16)`.
fn hex_rgb(hex: &str) -> Option<u32> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustradar_core::assert_close;
    use rustradar_core::num::{js_string, to_fixed};
    use rustradar_core::sun::local_sun;
    use rustradar_core::weather::{PRESETS, Weather, conditions_of};

    fn sun_at(alt_deg: f64, az_deg: f64) -> Rgb {
        let (alt, az) = (alt_deg * PI / 180.0, az_deg * PI / 180.0);
        [az.sin() * alt.cos(), az.cos() * alt.cos(), alt.sin()]
    }
    fn sun_at_south(alt_deg: f64) -> Rgb {
        sun_at(alt_deg, 180.0)
    }
    fn sum(c: Rgb) -> f64 {
        c[0] + c[1] + c[2]
    }
    fn look(c: Conditions, sun: Rgb) -> Look {
        look_for(&c, sun, DEFAULT_CAMERA_M)
    }

    /// The TypeScript's `skyCss` string, from the stops (to compare with what it wrote).
    fn css(sky: &SkyGradient) -> String {
        let rgb = |c: [u8; 3]| format!("rgb({} {} {})", c[0], c[1], c[2]);
        let (top, horizon, at) = (rgb(sky.top), rgb(sky.horizon), js_string(sky.horizon_percent));
        let linear = format!("linear-gradient(to bottom, {top} 0%, {horizon} {at}%, {horizon} 100%)");
        let Some(g) = sky.glow else { return linear };
        let stops: Vec<String> = g
            .stops
            .iter()
            .enumerate()
            .map(|(i, s)| {
                // (the last stop's alpha written as a plain 0)
                let alpha = if i == 3 { "0".to_owned() } else { to_fixed(s.alpha, 2) };
                format!("rgb({} {} {} / {alpha}) {}px", s.rgb[0], s.rgb[1], s.rgb[2], s.radius)
            })
            .collect();
        format!("radial-gradient(circle at {}px {}px, {}), {linear}", js_string(g.at[0]), js_string(g.at[1]), stops.join(", "))
    }

    // realistic map: light

    #[test]
    fn the_low_sun_comes_through_more_air_dimmer_and_redder() {
        let high = transmittance(60.0, 2e-5);
        let low = transmittance(5.0, 2e-5);
        assert!(sum(low) < sum(high));
        assert!(low[0] / low[2] > high[0] / high[2]);
        assert!(low[0] > low[2]);
    }

    #[test]
    fn sun_by_day_in_fine_weather_none_under_cloud_or_at_night() {
        let sunny = look(PRESETS.sunny, sun_at_south(40.0));
        let cloudy = look(PRESETS.cloudy, sun_at_south(40.0));
        let night = look(PRESETS.sunny, sun_at_south(-20.0));
        assert!(max3(sunny.sun_color) > 2.0);
        assert_eq!(max3(cloudy.sun_color), 0.0);
        assert_eq!(max3(night.sun_color), 0.0);
        // a blue sky overhead; a grey one under cloud; a dark one at night, its windows lit
        assert!(sunny.sky_zenith[2] > sunny.sky_zenith[0] * 3.0);
        assert!(cloudy.sky_zenith[2] / cloudy.sky_zenith[0] < 1.2);
        assert!(sum(night.sky_zenith) < 0.05);
        assert_eq!(night.night, 1.0);
        assert_eq!(sunny.night, 0.0);
    }

    #[test]
    fn clear_air_leaves_the_near_distance_clear_fog_closes_in_beyond_the_middle_of_the_view() {
        // what is left of something `metres` from the camera
        let seen = |l: &Look, metres: f64| (-js_max(metres - l.haze_start, 0.0) * l.haze_density).exp();
        // the camera 600 m out, as close in over a town
        let clear = look_for(&PRESETS.sunny, sun_at_south(40.0), 600.0);
        let fog = look_for(&PRESETS.fog, sun_at_south(40.0), 600.0);
        assert!(seen(&clear, 3000.0) > 0.9);
        assert_eq!(seen(&fog, 300.0), 1.0);
        assert!(seen(&fog, 600.0) > 0.5);
        assert!(seen(&fog, 2400.0) < 0.1);
        // far out, the map is still there to read, whatever the weather
        for preset in PRESETS.values() {
            assert!(seen(&look_for(&preset, sun_at_south(40.0), 50_000.0), 50_000.0) > 0.4);
        }
    }

    #[test]
    fn a_sunny_day_is_taken_darker_than_a_grey_one_so_it_isn_t_washed_out_the_night_brighter() {
        let sunny = look(PRESETS.sunny, sun_at_south(40.0)).exposure;
        let cloudy = look(PRESETS.cloudy, sun_at_south(40.0)).exposure;
        assert!(sunny < cloudy * 0.8);
        assert!(sunny >= 0.5);
        assert!(look(PRESETS.sunny, sun_at_south(-20.0)).exposure > cloudy * 2.0);
    }

    #[test]
    fn the_screen_s_colours_come_through_the_same_curve_as_the_shaders() {
        assert_eq!(display_rgb([0.0, 0.0, 0.0], 1.0), [0, 0, 0]);
        let (dim, bright) = (display_rgb([0.2, 0.2, 0.2], 1.0)[0], display_rgb([2.0, 2.0, 2.0], 1.0)[0]);
        assert!(bright > dim);
        // (`<= 255` holds for a u8 by its type)
        assert_eq!(bright, 255);
    }

    #[test]
    fn the_sky_behind_the_map_its_horizon_where_the_tilt_puts_it_the_sun_s_glow_when_in_view() {
        let l = look(PRESETS.sunny, sun_at_south(30.0));
        // the stops themselves...
        assert_eq!(sky_gradient(&l, 3.0, 0.25, None, 0.0).horizon_percent, 25.0);
        assert_eq!(sky_gradient(&l, 3.0, -0.5, None, 0.0).horizon_percent, 0.0);
        let glow = sky_gradient(&l, 3.0, 0.25, Some([400.0, 120.0]), 0.0).glow.expect("the sun's glow");
        assert_eq!(glow.at, [400.0, 120.0]);
        // ...and as the TypeScript wrote them
        assert!(css(&sky_gradient(&l, 3.0, 0.25, None, 0.0)).contains(" 25%"));
        assert!(css(&sky_gradient(&l, 3.0, -0.5, None, 0.0)).contains(" 0%"));
        assert!(css(&sky_gradient(&l, 3.0, 0.25, Some([400.0, 120.0]), 0.0)).contains("radial-gradient(circle at 400px 120px"));
        // under cloud, no sun to glow
        let cloudy = sky_gradient(&look(PRESETS.cloudy, sun_at_south(30.0)), 3.0, 0.25, Some([400.0, 120.0]), 0.0);
        assert!(cloudy.glow.is_none());
        assert!(!css(&cloudy).contains("radial-gradient"));
    }

    #[test]
    fn without_day_and_night_a_fine_afternoon_a_local_direction_turns_into_the_earth_s_and_back() {
        let sun = scene_sun(false, [0.0, 0.0, -1.0]);
        assert!(sun[2] > 0.5);
        assert_eq!(scene_sun(true, [0.1, 0.2, 0.3]), [0.1, 0.2, 0.3]);
        let back = local_sun(from_local(sun, 55.6, 13.0), 55.6, 13.0);
        for i in 0..3 {
            assert_close!(back[i], sun[i], 9);
        }
    }

    // realistic map: buildings

    fn facade_of(style: [u8; 4]) -> u8 {
        (255 - style[3]) % 5
    }
    fn roof_of(style: [u8; 4]) -> u8 {
        (255 - style[3]) / 5
    }

    #[test]
    fn each_building_its_own_facade_and_roof_the_same_each_time() {
        let none = BuildingLook::default();
        assert_eq!(building_style(12345, 8.0, None, none), building_style(12345, 8.0, None, none));
        let styles: Vec<[u8; 4]> = (0..400).map(|i| building_style(i * 7919, 8.0, None, none)).collect();
        let facades: std::collections::HashSet<u8> = styles.iter().map(|&s| facade_of(s)).collect();
        assert!(facades.len() > 3);
        for &s in &styles {
            assert!(facade_of(s) < 4);
            assert!(roof_of(s) < 7);
        }
        // low houses under pitched-roof materials; towers often glass
        let towers = (0..400).map(|i| building_style(i * 7919, 60.0, None, none)).filter(|&s| facade_of(s) == 4).count();
        assert!(towers > 100);
    }

    #[test]
    fn a_mapped_colour_is_kept_toned_down() {
        let none = BuildingLook::default();
        assert_eq!(building_style(1, 8.0, Some("#ffffff"), none)[..3], [241, 241, 241]);
        assert_eq!(building_style(1, 8.0, Some("white"), none)[..3], building_style(2, 8.0, Some("white"), none)[..3]);
        assert_eq!(building_style(5, 8.0, Some("brick-ish"), none), building_style(5, 8.0, None, none));
    }

    // realistic map: weather

    fn weather(over: impl FnOnce(&mut Weather)) -> Conditions {
        let mut w = Weather {
            cloud: 0.0,
            cloud_low: 0.0,
            fog: 0.0,
            precipitation: 0.0,
            symbol: "clearsky_day".to_owned(),
            temperature: 12.0,
        };
        over(&mut w);
        conditions_of(&w)
    }

    #[test]
    fn clear_sky_sun_overcast_none_broken_cloud_some_and_the_clouds_shadows() {
        assert_eq!(weather(|_| {}).sun, 1.0);
        let overcast = weather(|w| {
            (w.cloud, w.cloud_low, w.symbol) = (0.995, 0.95, "cloudy".into());
        });
        assert_eq!(overcast.sun, 0.0);
        let broken = weather(|w| {
            (w.cloud, w.cloud_low, w.symbol) = (0.55, 0.3, "partlycloudy_day".into());
        });
        assert!(broken.sun > 0.2);
        assert!(broken.sun < 0.9);
        assert!(broken.cloud_shadows > 0.5);
    }

    #[test]
    fn rain_wets_fog_closes_in_snow_lies() {
        let rain = weather(|w| (w.cloud, w.precipitation, w.symbol) = (1.0, 1.5, "rain".into()));
        assert!(rain.wet > 0.7);
        let fog = weather(|w| (w.cloud, w.fog, w.symbol) = (1.0, 0.9, "fog".into()));
        assert!(fog.haze > 1e-3);
        let snow = weather(|w| (w.cloud, w.precipitation, w.symbol, w.temperature) = (1.0, 0.5, "snow".into(), -2.0));
        assert_eq!(snow.snow, 1.0);
        // rain below freezing is snow
        let frozen = weather(|w| (w.cloud, w.precipitation, w.symbol, w.temperature) = (1.0, 0.5, "rain".into(), -1.0));
        assert_eq!(frozen.snow, 1.0);
    }

    #[test]
    fn rain_falls_more_in_a_downpour_and_veils_the_distance_fog_more() {
        assert!(weather(|w| (w.cloud, w.precipitation, w.symbol) = (1.0, 0.2, "lightrain".into())).rain > 0.3);
        assert_eq!(weather(|w| (w.cloud, w.precipitation, w.symbol) = (1.0, 3.0, "heavyrain".into())).rain, 1.0);
        assert_eq!(weather(|w| (w.cloud, w.fog, w.symbol) = (1.0, 0.9, "fog".into())).rain, 0.0);
        let clear_at = |l: &Look, m: f64| (-js_max(m - l.haze_start, 0.0) * l.haze_density).exp();
        let [cloudy, rain, fog] = [PRESETS.cloudy, PRESETS.rain, PRESETS.fog].map(|c| look_for(&c, [0.4, -0.55, 0.73], 1500.0));
        // three times as far off as the middle of the view
        assert!(clear_at(&rain, 4500.0) < clear_at(&cloudy, 4500.0) - 0.2);
        assert!(clear_at(&fog, 4500.0) < clear_at(&rain, 4500.0));
        // and near the camera all is clear
        assert_eq!(clear_at(&rain, 300.0), 1.0);
    }

    // new tests: the TypeScript's own answers (bun, realistic.ts) for the same inputs

    /// Within a few units in the last place: `sin`, `asin`, `pow`, `exp` and `sqrt` may differ in
    /// the last bit between JavaScriptCore and the C library.
    fn near(a: f64, b: f64) -> bool {
        a == b || (a - b).abs() <= 1e-13 * a.abs().max(b.abs())
    }
    fn near3(a: Rgb, b: Rgb) -> bool {
        (0..3).all(|i| near(a[i], b[i]))
    }

    #[test]
    fn transmittance_matches_the_typescript() {
        let cases: [(f64, f64, Rgb); 6] = [
            (-30.0, 0.0, [0.04120011816721661, 0.009454113863607021, 0.00030477056350122804]),
            (-0.5, 0.0, [0.023793803568191545, 0.004237790715006419, 7.563381075077447e-05]),
            (0.0, 2e-5, [0.006845613950337446, 0.0009323465446083154, 8.898065725871216e-06]),
            (5.0, 1.4e-4, [0.10421415725853848, 0.06305045346832178, 0.019519335673142596]),
            (35.0, 2e-5, [0.9049356289025153, 0.8694903422152062, 0.7920901344191015]),
            (90.0, 4.5e-3, [1.000158128972213, 1.000173683992248, 1.0002099799798043]),
        ];
        for (alt, haze, expected) in cases {
            let got = transmittance(alt, haze);
            assert!(near3(got, expected), "transmittance({alt}, {haze}) = {got:?}, not {expected:?}");
        }
    }

    #[test]
    fn look_for_matches_the_typescript() {
        // fog, the sun just up, 1.5 km out
        let l = look_for(&PRESETS.fog, [0.0, 0.99, 0.01], 1500.0);
        assert!(near3(l.sun_color, [3.4702391476799775e-09, 7.045250161641003e-10, 1.7066695927801367e-11]));
        assert!(near3(l.sky_zenith, [0.03839523791076226, 0.03821201856264329, 0.0397641388670186]));
        assert!(near3(l.sky_horizon, [0.05110954662646374, 0.04871566615166839, 0.04844083712948993]));
        assert!(near3(l.ground_color, [0.00895047846066308, 0.00869276847284022, 0.008820497599684987]));
        assert!(near3(l.haze_color, [0.1037779360484539, 0.10429654309149886, 0.10577874791082159]));
        assert!(near(l.ambient, 0.1972558289219251));
        assert!(near(l.haze_density, 0.0005733333333333334));
        assert!(near(l.haze_start, 750.0));
        assert!(near(l.exposure, 4.838997456971361));
        assert!(near(l.night, 0.00824379029842004));
        assert_eq!((l.wet, l.snow, l.cloud_shadow, l.globe), (0.35, 0.0, 0.0, 0.0));
        // rain at night, from far out over the globe
        let l = look_for(&PRESETS.rain, [0.0, 0.0, -1.0], 400_000.0);
        assert_eq!(l.sun_dir, [0.0, 0.0, -1.0]);
        assert_eq!(l.sun_color, [0.0; 3]);
        assert!(near(l.globe, 0.5829903978052127));
        assert!(near3(l.sky_zenith, [0.006, 0.004, 0.0024999999999999988]));
        assert!(near3(l.ground_color, [0.0018000000000000004, 0.0012000000000000001, 0.00075]));
        assert!(near(l.ambient, 0.146336039));
        assert!(near(l.haze_density, 5.5e-07));
        assert!(near(l.haze_start, 120_000.0));
        assert!(near(l.exposure, 5.760000000000001));
        assert_eq!(l.night, 1.0);
        // the light before the map sets one
        let l = Look::default();
        assert!(near3(l.sun_color, [3.0447308170502083, 2.9847535181701463, 2.8493609566251017]));
        assert_eq!(l.exposure, 1.0);
    }

    #[test]
    fn sky_gradient_writes_what_the_typescript_wrote() {
        let sunny = look(PRESETS.sunny, [0.4, -0.55, 0.73]);
        assert_eq!(
            css(&sky_gradient(&sunny, -5.0, 0.7, Some([400.4, 120.5]), 0.0)),
            "radial-gradient(circle at 400px 121px, rgb(255 252 240 / 0.85) 0px, rgb(255 250 235 / 0.85) 14px, \
             rgb(255 255 255 / 0.47) 40px, rgb(255 255 255 / 0) 320px), \
             linear-gradient(to bottom, rgb(174 195 223) 0%, rgb(174 195 223) 70%, rgb(174 195 223) 100%)"
        );
        let dusk = look(PRESETS.sunny, [0.1, 0.9, 0.05]);
        assert_eq!(
            css(&sky_gradient(&dusk, 20.0, 1.3, Some([-30.5, 2.5]), 0.3)),
            "radial-gradient(circle at -30px 3px, rgb(255 252 240 / 0.10) 0px, rgb(255 250 235 / 0.10) 14px, \
             rgb(255 255 227 / 0.05) 40px, rgb(255 255 227 / 0) 320px), \
             linear-gradient(to bottom, rgb(130 131 163) 0%, rgb(180 162 162) 100%, rgb(180 162 162) 100%)"
        );
        let night = look(PRESETS.snow, [0.0, 0.0, -1.0]);
        assert_eq!(
            css(&sky_gradient(&night, 70.0, 0.0, Some([1.0, 2.0]), 0.999)),
            "linear-gradient(to bottom, rgb(4 7 16) 0%, rgb(12 18 34) 0%, rgb(12 18 34) 100%)"
        );
    }

    #[test]
    fn sky_view_puts_the_horizon_and_the_sun_where_the_camera_sees_them() {
        // flat on: the horizon far above the screen, no sun in view, the top 70° short of overhead
        let v = sky_view(14.0, 0.0, 0.0, [0.0, -0.5, 0.86], 800.0, 600.0);
        assert!(v.horizon < -80.0);
        assert!(v.sun_at.is_none());
        assert_close!(v.top, 1.0 + (1.0_f64 / 3.0).atan() / DEG - 90.0, 12);
        assert_eq!(v.space, 0.0);
        // tilted 75° towards the setting sun straight ahead: in the middle across, on the horizon
        let v = sky_view(15.0, 75.0, 0.0, sun_at(0.0, 0.0), 800.0, 600.0);
        let [x, y] = v.sun_at.expect("the sun in view");
        assert_close!(x, 400.0, 9);
        assert!(v.horizon > 0.0 && v.horizon < 0.5);
        assert_close!(y / 600.0, v.horizon, 12);
        // 10° up it is above the top of the screen (the camera sees 18.4° above its middle)
        assert!(sky_view(15.0, 75.0, 0.0, sun_at(10.0, 0.0), 800.0, 600.0).sun_at.expect("ahead")[1] < 0.0);
        // turned away from it: not in view; far out: space
        assert!(sky_view(15.0, 75.0, 180.0, sun_at(10.0, 0.0), 800.0, 600.0).sun_at.is_none());
        assert_eq!(sky_view(8.0, 0.0, 0.0, sun_at(10.0, 0.0), 800.0, 600.0).space, 1.0);
        assert_close!(sky_view(10.25, 0.0, 0.0, sun_at(10.0, 0.0), 800.0, 600.0).space, 0.5, 12);
    }

    #[test]
    fn display_rgb_scene_sun_and_from_local_match_the_typescript() {
        assert_eq!(display_rgb([0.2, 0.3, 0.4], 1.0), [123, 148, 168]);
        assert_eq!(display_rgb([0.01, 0.5, 0.0031], 2.3), [46, 255, 27]);
        assert_eq!(display_rgb([0.123, 0.456, 0.789], 0.7), [84, 152, 195]);
        assert!(near3(scene_sun(false, [0.0; 3]), [-0.46984631039295427, -0.6710100716628343, 0.573576436351046]));
        assert!(near3(from_local([0.1, 0.2, 0.3], 55.6, 13.0), [-0.018142203270302304, 0.0984419531272922, 0.36052745016847615]));
        assert!(near3(from_local([-0.4, 0.5, 0.7], -33.0, -70.5), [-0.09018668183726702, -0.9436183920225325, 0.038087959462193066]));
    }

    #[test]
    fn building_style_matches_the_typescript() {
        let none = BuildingLook::default();
        // (seed, height, style, roof colour), from bun
        let cases: [(i32, f64, [u8; 4], [u8; 3]); 11] = [
            (0, 0.0, [242, 240, 232, 237], [190, 117, 93]),
            (1, 9.0, [195, 135, 101, 234], [88, 86, 84]),
            (5, 12.0, [209, 207, 201, 248], [139, 139, 139]),
            (12345, 8.9, [178, 94, 72, 244], [169, 181, 190]),
            (-1, 31.0, [185, 201, 195, 246], [139, 139, 139]),
            (-987_654_321, 60.0, [203, 217, 225, 251], [64, 64, 64]),
            (i32::MAX, 12.5, [217, 215, 209, 228], [100, 99, 100]),
            (i32::MIN, 10.0, [193, 133, 99, 254], [64, 64, 64]),
            (7919, 5.0, [239, 235, 223, 235], [88, 86, 84]),
            (99_991, 30.0, [231, 213, 177, 250], [139, 139, 139]),
            (123_456_789, 12.0, [220, 178, 98, 247], [139, 139, 139]),
        ];
        for (seed, height, style, roof) in cases {
            assert_eq!(building_style(seed, height, None, none), style, "building_style({seed}, {height})");
            assert_eq!(roof_colour(seed, height), roof, "roof_colour({seed}, {height})");
        }
        // names in any case, toned down; anything else not a colour
        assert_eq!(building_style(7, 8.0, Some("WHITE"), none), [232, 230, 226, 245]);
        assert_eq!(building_style(7, 8.0, Some("Brown"), none), [154, 118, 98, 245]);
        assert_eq!(building_style(7, 8.0, Some("#12abEF"), none), [64, 178, 229, 245]);
        for odd in ["#12ab", "#1234567", " #123456", "constructor", "toString", "brick-ish", ""] {
            assert_eq!(building_style(7, 8.0, Some(odd), none), [234, 230, 218, 245], "{odd:?}");
        }
        // a mapped façade or roof wins
        assert_eq!(building_style(3, 8.0, None, BuildingLook { facade: Some(BLOCK), roof: None }), [209, 207, 201, 238]);
        assert_eq!(building_style(3, 80.0, None, BuildingLook { facade: None, roof: Some(FLAT3) }), [197, 211, 219, 221]);
        assert_eq!(building_style(99, 5.0, None, BuildingLook { facade: Some(GLASS), roof: Some(TAR) }), [191, 207, 201, 251]);
    }
}
