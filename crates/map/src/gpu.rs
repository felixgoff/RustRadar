//! Adapter, device and queue, shared by the window and the headless path (moved from the spike's
//! `crates/app/src/gpu.rs`; plan 1.4.13 for the limits).
//!
//! Baseline: WebGL2-class limits raised to the adapter's texture sizes, and no feature the map
//! needs, so GLES 3.0 Android devices stay in scope. No compute, no storage buffers. The one
//! optional extra: the texture compression features the adapter offers (BC, ASTC, ETC2,
//! [`textures::compression_features`]), so the KTX2 textures go to the GPU as BC7, ASTC 4×4 or
//! ETC2 where they can ([`textures::Target::for_features`]) instead of four times the memory in
//! RGBA8. A device without any of them still draws everything.

use std::fmt;

use crate::textures;

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
                // only what the adapter has: optional, the baseline needs none
                required_features: textures::compression_features(adapter.features()),
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

    /// The format the KTX2 textures go to the GPU in on this device ([`textures::LoadOptions::
    /// for_device`]): BC7, ASTC 4×4, ETC2 or RGBA8.
    pub fn texture_target(&self) -> textures::Target {
        textures::LoadOptions::for_device(&self.device).target
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

#[cfg(test)]
mod tests {
    use super::testing::gpu;
    use crate::textures::{self, Target};

    /// Gap 3 of stage 2: the device has every compression feature its adapter offers, so the
    /// textures' target is the best one the adapter can sample, not always RGBA8.
    #[test]
    fn the_device_asks_for_the_texture_compression_the_adapter_has() {
        let Some(gpu) = gpu() else { return };
        let offered = textures::compression_features(gpu.adapter.features());
        assert_eq!(textures::compression_features(gpu.device.features()), offered);
        let target = gpu.texture_target();
        assert_eq!(target, Target::for_features(offered));
        eprintln!("{}: compression {offered:?}, KTX2 textures as {target:?}", gpu.describe());
        // (SwiftShader offers BC and ETC2, and ASTC: BC7 goes first)
        if gpu.adapter.get_info().name.contains("SwiftShader") {
            assert_eq!(target, Target::Bc7);
        }
    }
}
