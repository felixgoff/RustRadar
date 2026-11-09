//! Adapter and device setup shared by the window and the headless path.

use std::fmt;

/// Depth/stencil format of the map pass. WebGPU guarantees it everywhere,
/// including 4x MSAA, so no fallback is needed.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

#[derive(Debug)]
pub enum GpuError {
    /// No usable adapter: no GPU driver, or (headless) no software Vulkan/GL.
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

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// A device with no window, for screenshots and tests. Software adapters
    /// (SwiftShader, llvmpipe) are accepted. `WGPU_BACKEND` narrows the backends.
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
                // Stay inside what GLES 3 / WebGL2-class mobile GPUs offer,
                // but keep the adapter's real texture size for big rasters.
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::Device(e.to_string()))?;
        Ok(Self { instance, adapter, device, queue })
    }

    /// Highest of 4 or 1 samples that both the colour target and the depth buffer support.
    pub fn msaa_samples(&self, color_format: wgpu::TextureFormat) -> u32 {
        let ok = |format| self.adapter.get_texture_format_features(format).flags.sample_count_supported(4);
        if ok(color_format) && ok(DEPTH_FORMAT) { 4 } else { 1 }
    }
}
