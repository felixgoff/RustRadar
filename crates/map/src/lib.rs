//! RustRadar's map renderer on wgpu: pipelines, passes, tile runtime, textures and headless
//! rendering. Built up from stage 2 on (plan 1.2); it never depends on `rustradar-backend`.
//!
//! - [`gpu`]: adapter, device, queue. [`target`]: the offscreen map target and the composite pass.
//! - [`frame`]: the per-frame globals and the per-draw local Mercator frames, in f64 from
//!   `scene::camera`; [`shaders`]: the WGSL sources, with `lib/project.wgsl` (the projection).
//! - [`state`]: the draw-state presets. [`pipelines`]: the eight generic pipelines.
//! - [`layers`]: the layers in draw order; [`renderer`]: [`MapRenderer`] and [`SceneInput`].
//! - [`headless`]: a [`SceneInput`] to an RGBA image without a window.

pub mod frame;
pub mod gpu;
pub mod headless;
pub mod io;
pub mod jobs;
pub mod layers;
pub mod pipelines;
pub mod renderer;
pub mod shaders;
pub mod state;
pub mod target;
pub mod textures;
pub mod tiles;

pub use gpu::{Gpu, GpuError};
pub use renderer::{MapRenderer, SceneInput};
