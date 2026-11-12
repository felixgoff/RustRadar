//! What is drawn over the map after its pass (item R11; plan 1.4.1, 1.4.4 pass 3; doc 01 §1.4, doc
//! 04 §10): the realistic map's **tilt-shift** blur, in the composite pass, and the **rain** falling
//! in front of the camera, over the composited map and under the UI.
//!
//! - **Tilt-shift** (`tiltShift && basemap === "realistic"`, G:3781-3785, 3818-3828; [`Input`]):
//!   two layers over the map, each a CSS `backdrop-filter: blur(σ)` masked by
//!   `linear-gradient(black, transparent R, transparent calc(100% - R), black)`: σ 2 px reaching 42 %
//!   of the height in from the top and the bottom, then σ 5 px reaching 22 % ([`TILT_SHIFT`]). A
//!   layer's picture is the one under it blurred (a Gaussian of standard deviation σ CSS pixels)
//!   and mixed into it by the mask, which is 1 at the top and bottom rows and falls straight to 0 at
//!   R in from them; the second blurs what the first left. Each blur is two passes of
//!   `shaders/post.wgsl` (across, then down, [`taps`]), the last drawn onto the surface in place of
//!   the composite (with its sRGB rule). The middle of the screen (from 42 % to 58 % of the height)
//!   is the composite's picture, texel for texel.
//! - **Rain** (`RainOverlay.svelte`, shown by `updateLook`'s `showRain`, G:3640-3660; [`Rain`]), in a
//!   scene that has the overlay ([`Input::rain`]: the app's always does): while it rains where the
//!   camera is (the weather under the clouds' `rain`, [`crate::look::weather_here`]),
//!   at least [`MIN_AMOUNT`], streaks in three depths ([`DEPTHS`]), their number by the canvas's
//!   area and the rain, slanting [`SLANT`] in the wind, shorter as the camera looks down
//!   (`lengthen`), dimmer at night. The rain, the tilt and the night are taken in steps
//!   ([`RainShown`]: 0.05, 10°, 0.1) and the drops are made anew whenever a step or the canvas's
//!   size changes, as the overlay's effect re-runs. Each frame they fall by the time since the last
//!   (at most 50 ms), the local clock's: a drop past the bottom starts again above the top. While
//!   there are drops the map animates ([`Post::animating`]: the window draws every frame).
//!
//! [`Post::prepare`] reads the frame's light and the scene; [`Post::draw`] records the composite
//! (with the tilt-shift or without) and the rain onto the surface. `MapRenderer` calls both.
//!
//! Deviations, each with its reason:
//! - The blur runs on the map's own texels (`size × map_scale`; σ in map pixels), not the window's
//!   device pixels as Chromium's backdrop filter: the map is upscaled after it either way, and
//!   blurring before the upscale is the same picture within the bilinear step. Its edges are
//!   mirrored (Chromium's edge mode for backdrop filters). Each pair of texels is one bilinear
//!   sample (an exact Gaussian at texel centres, truncated at 3σ), where Skia downsamples by two
//!   for σ over 4 device pixels.
//! - The rain is drawn at the surface's resolution (today's canvas at `min(2, devicePixelRatio)`,
//!   then scaled to the screen), each streak a quad whose coverage of a pixel is worked out from its
//!   distance to the streak's line and along it (Skia's anti-aliased stroke, butt ends). Two
//!   streaks of one depth that cross blend twice (the canvas strokes each depth's path at once).
//! - `Math.random` is a SplitMix64 sequence from a fixed seed ([`SEED`]): the same drops at each
//!   start, and a test can know where they are ([`Rain::streaks`]).
//! - Reduced motion (`prefers-reduced-motion`, which today hides the rain) is not read: the app
//!   reads no system preference for it (`ui::anim`'s is never set), so the rain always shows.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use rustradar_core::geo::Basemap;
use rustradar_core::num::js_round;
use rustradar_scene::camera::ViewState;

use crate::frame::ViewFrame;
use crate::gpu::Gpu;
use crate::layers::sky;
use crate::look::{camera_altitude, frame_look, weather_here};
use crate::renderer::SceneInput;
use crate::shaders;
use crate::target::{Composite, MAP_FORMAT, MapTarget};

/// `shaders/post.wgsl`: the tilt-shift's blur passes and the rain's streaks.
pub const SHADER: &str = include_str!("shaders/post.wgsl");

/// What the scene asks of the post effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Input {
    /// The *tilt-shift* setting (`settings.tiltShift`): drawn on the realistic basemap only.
    pub tilt_shift: bool,
    /// The rain overlay (`RainOverlay`, which today's map always has): it falls while the light
    /// says it rains. The app's scene always asks for it; a scene that does not (the map's own
    /// shots of the ground under the rain preset) has none.
    pub rain: bool,
}

/// The tilt-shift's layers, in order: each blur's standard deviation (CSS pixels) and how far its
/// mask reaches in from the top and the bottom (of the height) (G:3783-3784).
pub const TILT_SHIFT: [(f64, f64); 2] = [(2.0, 0.42), (5.0, 0.22)];

/// The most taps a blur takes (`post.wgsl`'s `Blur::taps`): σ up to about 20 map pixels.
pub const MAX_TAPS: usize = 32;

/// A Gaussian blur of standard deviation `sigma` texels, truncated at 3σ, as the shader takes it:
/// the centre's weight first (offset 0), then each pair of texels `i`, `i + 1` (from 1 out) as one
/// bilinear sample at their weighted offset with their summed weight, read on both sides. The
/// weights sum to 1 over both sides. A σ of 0 or less is no blur: `[[0, 1]]`.
pub fn taps(sigma: f64) -> Vec<[f64; 2]> {
    if sigma.is_nan() || sigma <= 0.0 {
        return vec![[0.0, 1.0]];
    }
    let sigma = sigma.min(20.0);
    let radius = (3.0 * sigma).ceil() as usize;
    let weights: Vec<f64> = (0..=radius + 1).map(|i| if i > radius { 0.0 } else { (-((i * i) as f64) / (2.0 * sigma * sigma)).exp() }).collect();
    let total = weights[0] + 2.0 * weights[1..=radius].iter().sum::<f64>();
    let mut out = vec![[0.0, weights[0] / total]];
    for i in (1..=radius).step_by(2) {
        let (a, b) = (weights[i], weights[i + 1]);
        out.push([(i as f64 * a + (i + 1) as f64 * b) / (a + b), (a + b) / total]);
    }
    out.truncate(MAX_TAPS);
    out
}

/// The tilt-shift mask's alpha at `y` (0 at the top of the screen, 1 at the bottom) for a layer
/// reaching `reach` in: `linear-gradient(black, transparent reach, transparent 100% − reach,
/// black)`'s alpha.
pub fn mask(y: f64, reach: f64) -> f64 {
    (1.0 - y.min(1.0 - y) / reach).clamp(0.0, 1.0)
}

// ------------------------------------------------------------------------------------------ rain

/// One depth of the rain (`RainOverlay.svelte`'s `DEPTHS`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Depth {
    /// Drops per million square pixels at full rain.
    pub per: f64,
    /// A streak's length, CSS pixels (before `lengthen`).
    pub length: f64,
    /// How fast it falls, CSS pixels a second.
    pub speed: f64,
    /// Its line width, CSS pixels.
    pub line: f64,
    /// Its opacity.
    pub alpha: f64,
}

/// The rain's depths, the farthest first (the nearest longest and fastest).
pub const DEPTHS: [Depth; 3] = [
    Depth { per: 300.0, length: 12.0, speed: 900.0, line: 0.8, alpha: 0.22 },
    Depth { per: 190.0, length: 22.0, speed: 1400.0, line: 1.0, alpha: 0.3 },
    Depth { per: 80.0, length: 38.0, speed: 2100.0, line: 1.4, alpha: 0.4 },
];

/// Sideways per pixel down: the wind (`SLANT`).
pub const SLANT: f64 = 0.14;

/// Less rain than this draws none (`amount < 0.02`).
pub const MIN_AMOUNT: f64 = 0.02;

/// A frame's step of the drops is at most this long, seconds (`Math.min(0.05, …)`).
pub const MAX_STEP_S: f64 = 0.05;

/// Where [`Rain`]'s random numbers start.
pub const SEED: u64 = 0x5241_494e_2f52_3131;

/// The rain as the overlay is given it (`showRain`'s `rainShown`): each input in its step, so the
/// drops are not made anew every frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RainShown {
    /// How hard it rains, 0 to 1, to 0.05.
    pub amount: f64,
    /// The camera's tilt, degrees, to 10°.
    pub pitch: f64,
    /// 0 by day to 1 at night, to 0.1.
    pub night: f64,
}

impl RainShown {
    /// No rain (`showRain(0, 0, 0)`).
    pub const NONE: RainShown = RainShown { amount: 0.0, pitch: 0.0, night: 0.0 };

    /// `showRain(amount, pitch, night)`'s steps: `round(amount · 20) / 20`, `round(pitch / 10) · 10`,
    /// `round(night · 10) / 10`.
    pub fn new(amount: f64, pitch: f64, night: f64) -> Self {
        RainShown { amount: js_round(amount * 20.0) / 20.0, pitch: js_round(pitch / 10.0) * 10.0, night: js_round(night * 10.0) / 10.0 }
    }

    /// Whether any falls (`amount >= 0.02`).
    pub fn falls(&self) -> bool {
        self.amount >= MIN_AMOUNT
    }

    /// How long the streaks are against the depths' lengths: looking straight down rain is seen end
    /// on (`0.3 + 0.7 · sin(clamp(pitch, 0, 85)°)`).
    pub fn lengthen(&self) -> f64 {
        0.3 + 0.7 * (self.pitch.clamp(0.0, 85.0) * std::f64::consts::PI / 180.0).sin()
    }

    /// A depth's streak colour, 0–255 and alpha 0–1: `rgba(s, s + 8, s + 20, alpha · (0.6 + 0.4 ·
    /// amount) · (1 − 0.5 · night))` with `s = round(215 − 120 · night)`.
    pub fn colour(&self, depth: &Depth) -> [f64; 4] {
        let shade = js_round(215.0 - 120.0 * self.night);
        [shade, shade + 8.0, shade + 20.0, depth.alpha * (0.6 + 0.4 * self.amount) * (1.0 - 0.5 * self.night)]
    }
}

/// The rain a frame shows (`updateLook`, G:3647-3660): on the realistic basemap the weather's
/// rain where the camera is (under the clouds: [`weather_here`] at the camera's altitude), the
/// camera's tilt and the light's night (`look.night`), for `view` on a canvas of `size` CSS
/// pixels at the local clock `now_ms`; on the others none.
pub fn rain_shown(basemap: Basemap, sky: &sky::Input, view: &ViewState, size: [f64; 2], now_ms: f64) -> RainShown {
    if basemap != Basemap::Realistic {
        return RainShown::NONE;
    }
    let rain = weather_here(sky.weather, camera_altitude(view, size[1])).rain;
    let look = frame_look(sky, view, size, now_ms);
    RainShown::new(rain, view.pitch, look.night)
}

/// One streak to draw, in CSS pixels: from the drop (`head`, its lowest point) up to its `tail`,
/// `width` wide, in `rgba` (0–255, alpha 0–1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Streak {
    pub head: [f64; 2],
    pub tail: [f64; 2],
    pub width: f64,
    pub rgba: [f64; 4],
}

/// A drop: where it is and its speed factor `k` (0.75 to 1.25), which also stretches its streak.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drop {
    x: f64,
    y: f64,
    k: f64,
}

/// SplitMix64: `Math.random`'s stand-in, 0 to 1.
#[derive(Debug, Clone)]
struct Random(u64);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The rain overlay's drops (`RainOverlay.svelte`), in CSS pixels on a canvas of `size`.
#[derive(Debug, Clone)]
pub struct Rain {
    shown: RainShown,
    size: [f64; 2],
    /// Each depth's drops, in [`DEPTHS`]' order; none while it does not rain.
    drops: [Vec<Drop>; 3],
    /// The local clock at the last step (`last`), ms.
    last_ms: f64,
    random: Random,
}

impl Default for Rain {
    fn default() -> Self {
        Rain::new()
    }
}

impl Rain {
    /// No rain yet, the random numbers at [`SEED`].
    pub fn new() -> Self {
        Rain { shown: RainShown::NONE, size: [0.0, 0.0], drops: Default::default(), last_ms: 0.0, random: Random(SEED) }
    }

    /// The overlay given `shown` on a canvas of `size` CSS pixels at the local clock `now_ms`: when
    /// either changed, the drops are made anew (`$effect`), each at a random place on the canvas
    /// with a random speed factor, `round(per · amount · width · height / 1e6)` a depth; none
    /// while it does not rain or the canvas is empty.
    pub fn show(&mut self, shown: RainShown, size: [f64; 2], now_ms: f64) {
        if shown == self.shown && size == self.size {
            return;
        }
        self.shown = shown;
        self.size = size;
        self.last_ms = now_ms;
        let [width, height] = size;
        if !shown.falls() || width == 0.0 || height == 0.0 {
            self.drops = Default::default();
            return;
        }
        let random = &mut self.random;
        self.drops = DEPTHS.map(|d| {
            let count = js_round(d.per * shown.amount * width * height / 1e6).max(0.0) as usize;
            (0..count)
                .map(|_| {
                    let x = random.next() * width;
                    let y = random.next() * height;
                    let k = 0.75 + random.next() * 0.5;
                    Drop { x, y, k }
                })
                .collect()
        });
    }

    /// One frame's fall at the local clock `now_ms`: `dt = min(0.05 s, now − last)`; each drop falls
    /// `speed · k · dt` and drifts `SLANT` of that sideways; one whose streak has left the bottom
    /// starts again up to 40 px above the top, somewhere along a line the wind brings onto the
    /// canvas.
    pub fn step(&mut self, now_ms: f64) {
        let dt = ((now_ms - self.last_ms) / 1000.0).min(MAX_STEP_S);
        self.last_ms = now_ms;
        let [width, height] = self.size;
        let lengthen = self.shown.lengthen();
        let random = &mut self.random;
        for (d, drops) in DEPTHS.iter().zip(&mut self.drops) {
            let length = d.length * lengthen;
            for drop in drops {
                let fall = d.speed * drop.k * dt;
                drop.y += fall;
                drop.x += fall * SLANT;
                if drop.y - length > height {
                    drop.y = -random.next() * 40.0;
                    drop.x = random.next() * (width + height * SLANT) - height * SLANT;
                }
            }
        }
    }

    /// Whether any drop falls.
    pub fn active(&self) -> bool {
        self.drops.iter().any(|d| !d.is_empty())
    }

    /// What the rain was last shown as.
    pub fn shown(&self) -> RainShown {
        self.shown
    }

    /// The streaks as the overlay strokes them, depth by depth: from each drop to `(x − length ·
    /// SLANT · k, y − length · k)`, `length = depth.length · lengthen`.
    pub fn streaks(&self) -> Vec<Streak> {
        let lengthen = self.shown.lengthen();
        let mut out = Vec::with_capacity(self.drops.iter().map(Vec::len).sum());
        for (d, drops) in DEPTHS.iter().zip(&self.drops) {
            let length = d.length * lengthen;
            let rgba = self.shown.colour(d);
            out.extend(drops.iter().map(|drop| Streak {
                head: [drop.x, drop.y],
                tail: [drop.x - length * SLANT * drop.k, drop.y - length * drop.k],
                width: d.line,
                rgba,
            }));
        }
        out
    }
}

// ------------------------------------------------------------------------------------------- GPU

/// `post.wgsl`'s `Blur`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BlurUniform {
    taps: [[f32; 4]; MAX_TAPS],
    texel: [f32; 2],
    reach: f32,
    count: u32,
    target_size: [f32; 2],
    flags: u32,
    pad: u32,
}

impl BlurUniform {
    fn new(sigma: f64, source: [u32; 2], reach: f64, target: [u32; 2], flags: u32) -> Self {
        let list = taps(sigma);
        let mut taps = [[0.0; 4]; MAX_TAPS];
        for (slot, [offset, weight]) in taps.iter_mut().zip(&list) {
            *slot = [*offset as f32, *weight as f32, 0.0, 0.0];
        }
        BlurUniform {
            taps,
            texel: [1.0 / source[0].max(1) as f32, 1.0 / source[1].max(1) as f32],
            reach: reach as f32,
            count: list.len() as u32,
            target_size: [target[0].max(1) as f32, target[1].max(1) as f32],
            flags,
            pad: 0,
        }
    }
}

/// `post.wgsl`'s `Rain`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct RainUniform {
    target_size: [f32; 2],
    flags: u32,
    pad: u32,
}

/// One streak for `vs_rain`: its ends (surface pixels: head x, y, tail x, y), width (pixels) and
/// colour (0–1, straight alpha).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct StreakInstance {
    ends: [f32; 4],
    width: f32,
    colour: [f32; 4],
}

/// Linearise for an sRGB surface (the uniforms' `flags`).
const LINEARISE: u32 = 1;

/// The picture a tilt-shift blurs: a texture view, its size, and a key that changes whenever the
/// texture does (the map target's generation).
struct Picture<'a> {
    view: &'a wgpu::TextureView,
    size: [u32; 2],
    key: u64,
}

/// The blur's working textures and bind groups, for one picture at one size.
struct Blurs {
    key: u64,
    size: [u32; 2],
    /// Passes 1 (the picture across) to 4 (the last layer down, onto the surface).
    groups: [wgpu::BindGroup; 4],
    /// The texture between a blur's two passes, and the first layer's picture.
    _across: wgpu::Texture,
    across_view: wgpu::TextureView,
    _first: wgpu::Texture,
    first_view: wgpu::TextureView,
}

/// The post effects on the GPU, for surfaces of one format, and the rain's drops.
pub struct Post {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    across: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    last: wgpu::RenderPipeline,
    /// The first layer's blur, the second's at the map's size and onto the surface.
    uniforms: [wgpu::Buffer; 3],
    blurs: Option<Blurs>,
    rain_pipeline: wgpu::RenderPipeline,
    rain_uniform: wgpu::Buffer,
    rain_group: wgpu::BindGroup,
    rain_buffer: Option<(wgpu::Buffer, usize)>,
    rain: Rain,
    /// This frame's: the tilt-shift on, and the canvas in CSS pixels.
    tilt_shift: bool,
    css_size: [f64; 2],
}

impl Post {
    /// The post effects for surfaces of `format`.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform_entry = |binding, visibility, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(size as u64),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post blur"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT, size_of::<BlurUniform>()),
            ],
        });
        let rain_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post rain"),
            entries: &[uniform_entry(4, wgpu::ShaderStages::VERTEX_FRAGMENT, size_of::<RainUniform>())],
        });
        let shader = shaders::module(device, "post", SHADER);
        let blur_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post blur"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let full_screen = |label, entry, target: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&blur_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: target, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let across = full_screen("post across", "fs_across", MAP_FORMAT);
        let down = full_screen("post down", "fs_down", MAP_FORMAT);
        let last = full_screen("post last", "fs_final", format);
        let rain_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post rain"),
            bind_group_layouts: &[Some(&rain_layout)],
            immediate_size: 0,
        });
        let rain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post rain"),
            layout: Some(&rain_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_rain"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<StreakInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32, 2 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_rain"),
                compilation_options: Default::default(),
                // the canvas over the map: source-over
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post mirrored"),
            address_mode_u: wgpu::AddressMode::MirrorRepeat,
            address_mode_v: wgpu::AddressMode::MirrorRepeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let buffer = |label, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let uniforms = [buffer("post blur 1", size_of::<BlurUniform>()), buffer("post blur 2", size_of::<BlurUniform>()), buffer("post blur last", size_of::<BlurUniform>())];
        let rain_uniform = buffer("post rain", size_of::<RainUniform>());
        let rain_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post rain"),
            layout: &rain_layout,
            entries: &[wgpu::BindGroupEntry { binding: 4, resource: rain_uniform.as_entire_binding() }],
        });
        Post {
            format,
            layout,
            sampler,
            across,
            down,
            last,
            uniforms,
            blurs: None,
            rain_pipeline,
            rain_uniform,
            rain_group,
            rain_buffer: None,
            rain: Rain::new(),
            tilt_shift: false,
            css_size: [0.0, 0.0],
        }
    }

    /// The surface format this draws to.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Before the frame is drawn: the tilt-shift from the scene (the realistic basemap with the
    /// setting on), the rain from this frame's camera `view` and light at the local clock
    /// `now_ms` ([`rain_shown`]) when the scene has the overlay ([`Input::rain`]), and the drops'
    /// fall since the last frame.
    pub fn prepare(&mut self, scene: &SceneInput, view: &ViewFrame, now_ms: f64) {
        self.tilt_shift = scene.basemap == Basemap::Realistic && scene.post.tilt_shift;
        if !self.tilt_shift {
            // its two map-sized textures go with it
            self.blurs = None;
        }
        self.css_size = view.size;
        let shown = if scene.post.rain { rain_shown(scene.basemap, &scene.sky, &view.view, view.size, now_ms) } else { RainShown::NONE };
        self.rain.show(shown, view.size, now_ms);
        self.rain.step(now_ms);
    }

    /// Whether the picture moves by itself (the rain falls): the window should draw every frame.
    pub fn animating(&self) -> bool {
        self.rain.active()
    }

    /// The tilt-shift is drawn this frame.
    pub fn tilt_shift(&self) -> bool {
        self.tilt_shift
    }

    /// The rain's drops.
    pub fn rain(&self) -> &Rain {
        &self.rain
    }

    /// Records the composite of `map` onto `surface` (`surface_size` pixels), through the
    /// tilt-shift when it is on, then the rain over it.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        composite: &mut Composite,
        map: &MapTarget,
        surface: &wgpu::TextureView,
        surface_size: [u32; 2],
    ) {
        if self.tilt_shift {
            let picture = Picture { view: map.view(), size: map.size(), key: map.generation() };
            self.draw_tilt_shift(gpu, encoder, &picture, surface, surface_size);
        } else {
            composite.draw(&gpu.device, &gpu.queue, encoder, map, surface, surface_size);
        }
        self.draw_rain(gpu, encoder, surface, surface_size);
    }

    /// The working textures and bind groups for `picture`, made anew when it changed.
    fn blurs(&mut self, device: &wgpu::Device, picture: &Picture<'_>) {
        if self.blurs.as_ref().is_some_and(|b| b.key == picture.key && b.size == picture.size) {
            return;
        }
        let texture = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: picture.size[0].max(1), height: picture.size[1].max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: MAP_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let across = texture("post across");
        let first = texture("post first layer");
        let across_view = across.create_view(&Default::default());
        let first_view = first.create_view(&Default::default());
        let group = |source: &wgpu::TextureView, under: &wgpu::TextureView, uniform: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post blur"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(under) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: uniform.as_entire_binding() },
                ],
            })
        };
        let [one, two, last] = &self.uniforms;
        let groups = [
            group(picture.view, picture.view, one),
            group(&across_view, picture.view, one),
            group(&first_view, &first_view, two),
            group(&across_view, &first_view, last),
        ];
        self.blurs = Some(Blurs { key: picture.key, size: picture.size, groups, _across: across, across_view, _first: first, first_view });
    }

    /// The tilt-shift's four passes: the first layer across and down (into the first layer's
    /// picture), the second across and down onto `surface`.
    fn draw_tilt_shift(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, picture: &Picture<'_>, surface: &wgpu::TextureView, surface_size: [u32; 2]) {
        self.blurs(&gpu.device, picture);
        let size = picture.size;
        // σ in the picture's texels: CSS pixels times its texels a CSS pixel
        let texels = if self.css_size[1] > 0.0 { f64::from(size[1]) / self.css_size[1] } else { 1.0 };
        let [(sigma1, reach1), (sigma2, reach2)] = TILT_SHIFT;
        let flags = if self.format.is_srgb() { LINEARISE } else { 0 };
        let values = [
            BlurUniform::new(sigma1 * texels, size, reach1, size, 0),
            BlurUniform::new(sigma2 * texels, size, reach2, size, 0),
            BlurUniform::new(sigma2 * texels, size, reach2, surface_size, flags),
        ];
        for (buffer, value) in self.uniforms.iter().zip(&values) {
            gpu.queue.write_buffer(buffer, 0, bytemuck::bytes_of(value));
        }
        let blurs = self.blurs.as_ref().expect("made above");
        let passes: [(&wgpu::RenderPipeline, &wgpu::TextureView, &wgpu::BindGroup); 4] = [
            (&self.across, &blurs.across_view, &blurs.groups[0]),
            (&self.down, &blurs.first_view, &blurs.groups[1]),
            (&self.across, &blurs.across_view, &blurs.groups[2]),
            (&self.last, surface, &blurs.groups[3]),
        ];
        for (pipeline, target, group) in passes {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post tilt-shift"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    // every pixel is written
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// The rain's streaks over `surface`, scaled from CSS pixels to its `surface_size` pixels.
    fn draw_rain(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, surface: &wgpu::TextureView, surface_size: [u32; 2]) {
        if !self.rain.active() || self.css_size[0] <= 0.0 || self.css_size[1] <= 0.0 {
            return;
        }
        let scale = [f64::from(surface_size[0]) / self.css_size[0], f64::from(surface_size[1]) / self.css_size[1]];
        let instances: Vec<StreakInstance> = self
            .rain
            .streaks()
            .iter()
            .map(|s| StreakInstance {
                ends: [(s.head[0] * scale[0]) as f32, (s.head[1] * scale[1]) as f32, (s.tail[0] * scale[0]) as f32, (s.tail[1] * scale[1]) as f32],
                width: (s.width * scale[1]) as f32,
                colour: [(s.rgba[0] / 255.0) as f32, (s.rgba[1] / 255.0) as f32, (s.rgba[2] / 255.0) as f32, s.rgba[3] as f32],
            })
            .collect();
        if instances.is_empty() {
            return;
        }
        let bytes = instances.len() * size_of::<StreakInstance>();
        if self.rain_buffer.as_ref().is_none_or(|(_, capacity)| *capacity < bytes) {
            let capacity = bytes.next_power_of_two();
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("post rain streaks"),
                size: capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.rain_buffer = Some((buffer, capacity));
        }
        let (buffer, _) = self.rain_buffer.as_ref().expect("made above");
        gpu.queue.write_buffer(buffer, 0, bytemuck::cast_slice(&instances));
        let uniform = RainUniform {
            target_size: [surface_size[0].max(1) as f32, surface_size[1].max(1) as f32],
            flags: if self.format.is_srgb() { LINEARISE } else { 0 },
            pad: 0,
        };
        gpu.queue.write_buffer(&self.rain_uniform, 0, bytemuck::bytes_of(&uniform));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post rain"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.rain_pipeline);
        pass.set_bind_group(0, &self.rain_group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..bytes as u64));
        pass.draw(0..6, 0..instances.len() as u32);
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::weather::PRESETS;

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{Image, Offscreen};
    use crate::renderer::MapRenderer;

    /// 2026-06-21 10:00 UTC: a summer morning in Stockholm (the reference pictures' clock).
    const MORNING: f64 = 1_782_036_000_000.0;
    /// 2026-12-21 22:00 UTC: night over Europe.
    const NIGHT: f64 = 1_797_890_400_000.0;

    fn view(latitude: f64, longitude: f64, zoom: f64, pitch: f64) -> ViewState {
        ViewState { latitude, longitude, zoom, pitch, bearing: 0.0, position: None }
    }

    /// A Gaussian of σ truncated at 3σ and normalised: the weight of each texel 0..=R from the
    /// centre.
    fn gaussian(sigma: f64) -> Vec<f64> {
        let radius = (3.0 * sigma).ceil() as usize;
        let w: Vec<f64> = (0..=radius).map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp()).collect();
        let total = w[0] + 2.0 * w[1..].iter().sum::<f64>();
        w.into_iter().map(|v| v / total).collect()
    }

    /// The pairs of texels read in one bilinear sample give each texel the Gaussian's own weight:
    /// they sum to one, and each pair splits back into the two texels' weights.
    #[test]
    fn the_taps_are_the_gaussian_in_pairs() {
        for sigma in [2.0, 3.0, 5.0, 7.5] {
            let taps = taps(sigma);
            let g = gaussian(sigma);
            let total = taps[0][1] + 2.0 * taps[1..].iter().map(|t| t[1]).sum::<f64>();
            assert!((total - 1.0).abs() < 1e-12, "σ {sigma}: {total}");
            assert_eq!(taps[0], [0.0, g[0]]);
            for (n, [offset, weight]) in taps[1..].iter().enumerate() {
                let i = 2 * n + 1;
                let f = offset - i as f64;
                assert!((0.0..1.0).contains(&f), "σ {sigma}: pair {i} at {offset}");
                // the bilinear sample's weights on texels i and i + 1
                assert!((weight * (1.0 - f) - g[i]).abs() < 1e-12, "σ {sigma}: texel {i}");
                let next = g.get(i + 1).copied().unwrap_or(0.0);
                assert!((weight * f - next).abs() < 1e-12, "σ {sigma}: texel {}", i + 1);
            }
            // 7.5 texels: the 5 px blur at 1.5 map pixels a CSS pixel, 23 texels each side in 12 pairs
            assert!(taps.len() <= MAX_TAPS);
        }
        assert_eq!(taps(7.5).len(), 13);
        assert_eq!(taps(0.0), vec![[0.0, 1.0]]);
        assert_eq!(taps(f64::NAN), vec![[0.0, 1.0]]);
    }

    /// `linear-gradient(black, transparent R, transparent calc(100% − R), black)`.
    #[test]
    fn the_mask_falls_from_the_edges_to_the_reach() {
        assert_eq!(mask(0.0, 0.42), 1.0);
        assert_eq!(mask(1.0, 0.42), 1.0);
        assert!((mask(0.21, 0.42) - 0.5).abs() < 1e-12);
        assert!((mask(1.0 - 0.11, 0.22) - 0.5).abs() < 1e-12);
        assert_eq!(mask(0.42, 0.42), 0.0);
        assert_eq!(mask(0.5, 0.42), 0.0);
        assert_eq!(mask(0.3, 0.22), 0.0);
    }

    /// `showRain`'s steps, with JavaScript's rounding.
    #[test]
    fn the_rain_is_shown_in_steps() {
        assert_eq!(RainShown::new(0.537, 44.9, 0.26), RainShown { amount: 0.55, pitch: 40.0, night: 0.3 });
        // halves round up, as Math.round does
        assert_eq!(RainShown::new(0.025, 45.0, 0.05), RainShown { amount: 0.05, pitch: 50.0, night: 0.1 });
        // the step of 0.05 decides: under 0.025 none falls
        assert!(!RainShown::new(0.024, 0.0, 0.0).falls());
        assert!(RainShown::new(0.025, 0.0, 0.0).falls());
        // looking straight down the streaks are 0.3 of their length; at 85° and over, all of it
        assert_eq!(RainShown::NONE.lengthen(), 0.3);
        assert!((RainShown::new(1.0, 90.0, 0.0).lengthen() - (0.3 + 0.7 * (85f64).to_radians().sin())).abs() < 1e-12);
        // by day the drops are light, at night dimmer and darker
        let day = RainShown::new(1.0, 0.0, 0.0);
        assert_eq!(day.colour(&DEPTHS[2]), [215.0, 223.0, 235.0, 0.4]);
        let night = RainShown::new(0.5, 0.0, 1.0);
        let [r, g, b, a] = night.colour(&DEPTHS[0]);
        assert_eq!([r, g, b], [95.0, 103.0, 115.0]);
        assert!((a - 0.22 * 0.8 * 0.5).abs() < 1e-12);
    }

    /// The overlay's drops: `round(per · amount · width · height / 1e6)` a depth, on the canvas,
    /// none under 0.02 of rain; a step falls by `speed · k · dt` (at most 50 ms) and drifts with the
    /// wind; a drop past the bottom starts again above the top.
    #[test]
    fn the_drops_fall_and_start_again_above() {
        let mut rain = Rain::new();
        let shown = RainShown::new(1.0, 40.0, 0.0);
        rain.show(shown, [1440.0, 900.0], 1000.0);
        let counts: Vec<usize> = rain.drops.iter().map(Vec::len).collect();
        assert_eq!(counts, [389, 246, 104]);
        assert!(rain.active());
        for drop in rain.drops.iter().flatten() {
            assert!((0.0..1440.0).contains(&drop.x) && (0.0..900.0).contains(&drop.y) && (0.75..1.25).contains(&drop.k), "{drop:?}");
        }
        // the same again: nothing made anew
        let before = rain.drops.clone();
        rain.show(shown, [1440.0, 900.0], 1500.0);
        assert_eq!(rain.drops, before);
        // 30 ms later: each drop 30 ms further down
        rain.step(1030.0);
        for (d, (now, then)) in DEPTHS.iter().zip(rain.drops.iter().zip(&before)) {
            for (a, b) in now.iter().zip(then) {
                let fall = d.speed * b.k * 0.03;
                if b.y + fall - d.length * shown.lengthen() <= 900.0 {
                    assert!((a.y - (b.y + fall)).abs() < 1e-9 && (a.x - (b.x + fall * SLANT)).abs() < 1e-9);
                }
            }
        }
        // a long wait is one step of 50 ms; every drop that left the bottom starts again above it
        let before = rain.drops.clone();
        rain.step(60_000.0);
        let mut restarted = 0;
        for (d, (now, then)) in DEPTHS.iter().zip(rain.drops.iter().zip(&before)) {
            for (a, b) in now.iter().zip(then) {
                let fall = d.speed * b.k * MAX_STEP_S;
                if b.y + fall - d.length * shown.lengthen() > 900.0 {
                    restarted += 1;
                    assert!((-40.0..=0.0).contains(&a.y) && (-900.0 * SLANT..1440.0).contains(&a.x), "{a:?}");
                } else {
                    assert!((a.y - (b.y + fall)).abs() < 1e-9);
                }
            }
        }
        assert!(restarted > 0);
        // the streaks: from each drop up and back against the wind
        let streak = rain.streaks()[0];
        let drop = rain.drops[0][0];
        let length = DEPTHS[0].length * shown.lengthen();
        assert_eq!(streak.head, [drop.x, drop.y]);
        assert_eq!(streak.tail, [drop.x - length * SLANT * drop.k, drop.y - length * drop.k]);
        assert_eq!(streak.width, 0.8);
        // a new step of the rain makes the drops anew; under 0.02 there are none
        rain.show(RainShown::new(0.5, 40.0, 0.0), [1440.0, 900.0], 2000.0);
        assert_eq!(rain.drops.iter().map(Vec::len).collect::<Vec<_>>(), [194, 123, 52]);
        rain.show(RainShown::new(0.01, 40.0, 0.0), [1440.0, 900.0], 2000.0);
        assert!(!rain.active() && rain.streaks().is_empty());
    }

    /// The rain falls where the camera is under the clouds on the realistic map: the weather's own
    /// close in (rain's 1, the preset), none from 10 km up, none on the other basemaps; at night the
    /// light's night dims it.
    #[test]
    fn it_rains_under_the_clouds_on_the_realistic_map() {
        let raining = sky::Input { shown: true, weather: Some(PRESETS.rain), daylight: true };
        let size = [1100.0, 720.0];
        let close = view(59.6519, 17.9186, 13.5, 45.0);
        let shown = rain_shown(Basemap::Realistic, &raining, &close, size, MORNING);
        assert_eq!(shown, RainShown { amount: 1.0, pitch: 50.0, night: 0.0 });
        assert_eq!(rain_shown(Basemap::Dark, &raining, &close, size, MORNING), RainShown::NONE);
        // Europe from far up: above the clouds
        assert_eq!(rain_shown(Basemap::Realistic, &raining, &view(52.0, 10.0, 4.5, 0.0), size, MORNING).amount, 0.0);
        // a fine day
        let sunny = sky::Input { weather: Some(PRESETS.sunny), ..raining };
        assert!(!rain_shown(Basemap::Realistic, &sunny, &close, size, MORNING).falls());
        // the live weather not yet come: a fine day
        assert!(!rain_shown(Basemap::Realistic, &sky::Input { weather: None, ..raining }, &close, size, MORNING).falls());
        assert_eq!(rain_shown(Basemap::Realistic, &raining, &close, size, NIGHT).night, 1.0);
    }

    /// The renderer prepares the post effects each frame: the rain animates the map while it falls
    /// on the realistic basemap, in a scene with the overlay.
    #[test]
    fn the_renderer_animates_while_it_rains() {
        let Some(gpu) = gpu() else { return };
        let mut renderer = MapRenderer::new(&gpu, crate::headless::FORMAT);
        let mut scene = SceneInput::new(view(59.6519, 17.9186, 13.5, 45.0), [1100.0, 720.0], 1.0, Basemap::Realistic);
        scene.sky = sky::Input { shown: true, weather: Some(PRESETS.rain), daylight: true };
        scene.post = Input { tilt_shift: true, rain: true };
        renderer.update(scene.clone());
        renderer.prepare(&gpu, MORNING);
        assert!(renderer.animating());
        // a scene without the overlay has no rain
        renderer.update(SceneInput { post: Input { rain: false, ..scene.post }, ..scene.clone() });
        renderer.prepare(&gpu, MORNING);
        assert!(!renderer.animating());
        scene.basemap = Basemap::Dark;
        renderer.update(scene.clone());
        renderer.prepare(&gpu, MORNING);
        assert!(!renderer.animating());
        scene.basemap = Basemap::Realistic;
        scene.sky.weather = Some(PRESETS.sunny);
        renderer.update(scene);
        renderer.prepare(&gpu, MORNING);
        assert!(!renderer.animating());
    }

    /// A test picture: 3-pixel checks in red and green against each other, blue a ramp.
    fn checks(size: [u32; 2]) -> Vec<[u8; 4]> {
        let mut out = Vec::new();
        for y in 0..size[1] {
            for x in 0..size[0] {
                let on = (x / 3 + y / 3) % 2 == 0;
                let v = if on { 230 } else { 40 };
                out.push([v, 255 - v, ((x * 7 + y * 3) % 256) as u8, 255]);
            }
        }
        out
    }

    fn picture_texture(gpu: &Gpu, size: [u32; 2], pixels: &[[u8; 4]]) -> wgpu::Texture {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test picture"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MAP_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(pixels),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size[0] * 4), rows_per_image: Some(size[1]) },
            wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        texture
    }

    /// One blur of the CPU's twin: `image` (rgb 0–255, `w` × `h`) blurred across or down with the
    /// Gaussian of σ, its edges mirrored as the sampler mirrors them.
    fn blur_twin(image: &[[f64; 3]], [w, h]: [usize; 2], sigma: f64, down: bool) -> Vec<[f64; 3]> {
        let g = gaussian(sigma);
        let mirror = |i: isize, n: usize| -> usize {
            let n = n as isize;
            let i = if i < 0 { -i - 1 } else if i >= n { 2 * n - i - 1 } else { i };
            i as usize
        };
        let mut out = vec![[0.0; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0.0; 3];
                for (k, weight) in g.iter().enumerate() {
                    let offsets: &[isize] = if k == 0 { &[0] } else { &[k as isize, -(k as isize)] };
                    for &o in offsets {
                        let (sx, sy) = if down { (x, mirror(y as isize + o, h)) } else { (mirror(x as isize + o, w), y) };
                        let p = image[sy * w + sx];
                        for c in 0..3 {
                            sum[c] += p[c] * weight;
                        }
                    }
                }
                out[y * w + x] = sum;
            }
        }
        out
    }

    /// The tilt-shift on the CPU, as the GPU draws it: each layer blurred across (stored in bytes),
    /// down and mixed by its mask (stored in bytes).
    fn tilt_shift_twin(pixels: &[[u8; 4]], size: [u32; 2]) -> Vec<[f64; 3]> {
        let dims = [size[0] as usize, size[1] as usize];
        let bytes = |v: Vec<[f64; 3]>| v.into_iter().map(|p| p.map(|c| c.clamp(0.0, 255.0).round())).collect::<Vec<_>>();
        let mut picture: Vec<[f64; 3]> = pixels.iter().map(|p| [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]).collect();
        for (sigma, reach) in TILT_SHIFT {
            let across = bytes(blur_twin(&picture, dims, sigma, false));
            let down = blur_twin(&across, dims, sigma, true);
            let mixed: Vec<[f64; 3]> = (0..dims[0] * dims[1])
                .map(|i| {
                    let m = mask((((i / dims[0]) as f64) + 0.5) / dims[1] as f64, reach);
                    [0, 1, 2].map(|c| picture[i][c] * (1.0 - m) + down[i][c] * m)
                })
                .collect();
            picture = bytes(mixed);
        }
        picture
    }

    /// The tilt-shift on the GPU is its CPU twin's: a Gaussian of 2 px mixed in up to 42 % of the
    /// height from the top and the bottom, then one of 5 px up to 22 %. The rows between 42 % and
    /// 58 % are the picture itself; the top and bottom rows' checks are blurred nearly flat.
    #[test]
    fn the_tilt_shift_blurs_the_top_and_bottom_as_its_twin() {
        let Some(gpu) = gpu() else { return };
        let size = [160, 100];
        let pixels = checks(size);
        let texture = picture_texture(&gpu, size, &pixels);
        let view = texture.create_view(&Default::default());
        let mut post = Post::new(&gpu.device, crate::headless::FORMAT);
        post.css_size = [160.0, 100.0];
        let surface = Offscreen::new(&gpu, size);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        post.draw_tilt_shift(&gpu, &mut encoder, &Picture { view: &view, size, key: u64::MAX }, surface.view(), size);
        gpu.queue.submit([encoder.finish()]);
        let image = surface.read(&gpu).expect("read back");
        let twin = tilt_shift_twin(&pixels, size);
        let (mut worst, mut total) = (0.0f64, 0.0);
        for y in 0..size[1] {
            for x in 0..size[0] {
                let i = (y * size[0] + x) as usize;
                let got = image.pixel(x, y);
                for c in 0..3 {
                    let d = (f64::from(got[c]) - twin[i][c]).abs();
                    worst = worst.max(d);
                    total += d;
                }
                // the middle is the picture, texel for texel
                if (42..58).contains(&y) {
                    assert_eq!(got, pixels[i], "({x}, {y})");
                }
            }
        }
        let mean = total / f64::from(size[0] * size[1] * 3);
        eprintln!("tilt-shift against its twin: worst {worst:.1} levels, mean {mean:.3}");
        assert!(worst <= 3.0 && mean < 0.5, "worst {worst}, mean {mean}");
        // how much the checks still show along a row: flat at the edges, full in the middle
        let contrast = |image: &Image, y: u32| {
            let reds: Vec<u8> = (10..150).map(|x| image.pixel(x, y)[0]).collect();
            f64::from(reds.iter().max().unwrap() - reds.iter().min().unwrap())
        };
        assert_eq!(contrast(&image, 50), 190.0);
        assert!(contrast(&image, 0) < 30.0 && contrast(&image, 99) < 30.0, "{} {}", contrast(&image, 0), contrast(&image, 99));
        assert!(contrast(&image, 30) > contrast(&image, 10));
    }

    /// The rain's streaks on the GPU are their CPU twin's: each pixel covered by a streak's line
    /// (across its width, along its length, ends butt) by the share its coverage gives, blended
    /// over the picture in the streaks' order.
    #[test]
    fn the_rain_streaks_are_drawn_as_their_twin() {
        let Some(gpu) = gpu() else { return };
        let size = [240u32, 160u32];
        let grey = [90u8, 100, 110, 255];
        let surface = Offscreen::new(&gpu, size);
        let mut post = Post::new(&gpu.device, crate::headless::FORMAT);
        // a canvas of half the surface's size: the streaks scaled to the screen, as on a sharp one
        post.css_size = [120.0, 80.0];
        post.rain.show(RainShown::new(1.0, 60.0, 0.0), [120.0, 80.0], 0.0);
        post.rain.step(0.0);
        assert!(post.rain.active());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("grey"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 90.0 / 255.0, g: 100.0 / 255.0, b: 110.0 / 255.0, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        post.draw_rain(&gpu, &mut encoder, surface.view(), size);
        gpu.queue.submit([encoder.finish()]);
        let image = surface.read(&gpu).expect("read back");
        // the twin, in surface pixels
        let mut twin: Vec<[f64; 3]> = vec![[90.0, 100.0, 110.0]; (size[0] * size[1]) as usize];
        let covered = |x: f64, lo: f64, hi: f64| ((x + 0.5).min(hi) - (x - 0.5).max(lo)).clamp(0.0, 1.0);
        for s in post.rain.streaks() {
            let head = [s.head[0] * 2.0, s.head[1] * 2.0];
            let tail = [s.tail[0] * 2.0, s.tail[1] * 2.0];
            let width = s.width * 2.0;
            let length = ((tail[0] - head[0]).powi(2) + (tail[1] - head[1]).powi(2)).sqrt();
            let along = [(tail[0] - head[0]) / length, (tail[1] - head[1]) / length];
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let p = [f64::from(x) + 0.5 - head[0], f64::from(y) + 0.5 - head[1]];
                    let t = p[0] * along[0] + p[1] * along[1];
                    let d = -p[0] * along[1] + p[1] * along[0];
                    let a = s.rgba[3] * covered(t, 0.0, length) * covered(d, -width / 2.0, width / 2.0);
                    if a > 0.0 {
                        let px = &mut twin[(y * size[0] + x) as usize];
                        for (value, colour) in px.iter_mut().zip(s.rgba) {
                            *value = colour * a + *value * (1.0 - a);
                        }
                    }
                }
            }
        }
        let (mut worst, mut lit, mut changed) = (0.0f64, 0, 0);
        for y in 0..size[1] {
            for x in 0..size[0] {
                let got = image.pixel(x, y);
                let want = twin[(y * size[0] + x) as usize];
                for c in 0..3 {
                    worst = worst.max((f64::from(got[c]) - want[c]).abs());
                }
                lit += usize::from(want[0] > 100.0);
                changed += usize::from(got != grey);
            }
        }
        eprintln!("rain against its twin: worst {worst:.1} levels; {lit} pixels lit well, {changed} changed");
        assert!(worst <= 2.0, "worst {worst}");
        assert!(lit > 200, "{lit}");
    }
}
