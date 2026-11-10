//! Adapter, device and queue, shared by the window and the headless path (moved from the spike's
//! `crates/app/src/gpu.rs`; plan 1.4.13 for the limits).
//!
//! Baseline: WebGL2-class limits raised to the adapter's texture sizes, and no optional feature, so
//! GLES 3.0 Android devices stay in scope. No compute, no storage buffers.

use std::fmt;

/// Depth/stencil format of the map pass. WebGPU guarantees it everywhere, including 4× MSAA, so no
/// fallback is needed; the stencil is for shadows (plan 1.4.5).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Why a device could not be had.
#[derive(Debug)]
pub enum GpuError {
    /// No usable adapter: no GPU driver, or (headless) no software Vulkan/GL. GPU tests print
    /// `skipped: …` and pass on this one.
    NoAdapter(String),
    Device(String),
    Surface(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(e) => write!(f, "no graphics adapter: {e}"),
            Self::Device(e) => write!(f, "cannot open graphics device: {e}"),
            Self::Surface(e) => write!(f, "cannot create window surface: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// The wgpu objects every part of the renderer needs.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// A device with no window, for screenshots and tests. Software adapters (SwiftShader,
    /// llvmpipe, lavapipe) are accepted. `WGPU_BACKEND` narrows the backends.
    pub fn headless() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        pollster::block_on(Self::open(instance, None))
    }

    /// A device able to present to `surface`.
    pub fn for_surface(instance: wgpu::Instance, surface: &wgpu::Surface<'_>) -> Result<Self, GpuError> {
        pollster::block_on(Self::open(instance, Some(surface)))
    }

    async fn open(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Self, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::NoAdapter(e.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rustradar"),
                // Stay inside what GLES 3 / WebGL2-class mobile GPUs offer, but keep the adapter's
                // real texture size for big rasters.
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::Device(e.to_string()))?;
        Ok(Self { instance, adapter, device, queue })
    }

    /// Highest of 4 or 1 samples that both the colour format and the depth buffer support.
    pub fn msaa_samples(&self, color_format: wgpu::TextureFormat) -> u32 {
        let ok = |format| self.adapter.get_texture_format_features(format).flags.sample_count_supported(4);
        if ok(color_format) && ok(DEPTH_FORMAT) { 4 } else { 1 }
    }

    /// "Adapter name (backend, device type)", for logs.
    pub fn describe(&self) -> String {
        let info = self.adapter.get_info();
        format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type)
    }
}

/// One device for the GPU tests of this crate, used one test at a time: SwiftShader crashes when
/// several tests open their own devices at once.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use super::{Gpu, GpuError};

    /// The shared device, or `None` (after printing `skipped: …`) when there is no adapter.
    pub(crate) fn gpu() -> Option<MutexGuard<'static, Gpu>> {
        static GPU: OnceLock<Option<Mutex<Gpu>>> = OnceLock::new();
        let gpu = GPU.get_or_init(|| match Gpu::headless() {
            Ok(gpu) => Some(Mutex::new(gpu)),
            Err(GpuError::NoAdapter(e)) => {
                eprintln!("skipped: {e}");
                None
            }
            Err(e) => panic!("{e}"),
        });
        match gpu {
            Some(gpu) => Some(gpu.lock().unwrap_or_else(|poisoned| poisoned.into_inner())),
            None => {
                eprintln!("skipped: no graphics adapter");
                None
            }
        }
    }
}
