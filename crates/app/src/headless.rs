//! Offscreen rendering to an RGBA image: `--screenshot` and the tests.

use std::path::Path;

use crate::gpu::{Gpu, GpuError};
use crate::renderer::Renderer;

/// What the offscreen target uses: gamma-space RGBA, bytes straight to PNG.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, top row first.
    pub rgba: Vec<u8>,
    /// Adapter name, backend and MSAA sample count, for logs.
    pub adapter: String,
}

/// Renders one frame of `width`x`height` physical pixels at `pixels_per_point` scale.
pub fn render_frame(width: u32, height: u32, pixels_per_point: f32) -> Result<Frame, GpuError> {
    let gpu = Gpu::headless()?;
    let mut renderer = Renderer::new(&gpu, FORMAT);
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("screenshot"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());

    // egui lays a new Area out invisibly on its first frame, so the second one is the picture.
    for pass in 0..2 {
        let mut input = egui::RawInput {
            // A second apart, so any egui animation has finished by the frame that is kept.
            time: Some(pass as f64),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width as f32, height as f32) / pixels_per_point,
            )),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(pixels_per_point);
        renderer.render(&gpu, &view, [width, height], input, 0.0);
    }

    // Rows in a texture-to-buffer copy must be padded to 256 bytes.
    let row = width as usize * 4;
    let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot readback"),
        size: (padded_row * height as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row as u32),
                rows_per_image: None,
            },
        },
        size,
    );
    gpu.queue.submit([encoder.finish()]);

    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Device(e.to_string()))?;
    rx.recv()
        .map_err(|e| GpuError::Device(e.to_string()))?
        .map_err(|e| GpuError::Device(e.to_string()))?;
    let mapped = buffer.get_mapped_range(..).map_err(|e| GpuError::Device(e.to_string()))?;
    let mut rgba = Vec::with_capacity(row * height as usize);
    for padded in mapped.chunks_exact(padded_row) {
        rgba.extend_from_slice(&padded[..row]);
    }

    let info = gpu.adapter.get_info();
    let adapter = format!("{} ({:?}, {:?}), MSAA x{}", info.name, info.backend, info.device_type, renderer.msaa_samples());
    Ok(Frame { width, height, rgba, adapter })
}

pub fn save_png(frame: &Frame, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&frame.rgba)?;
    writer.finish()?;
    Ok(())
}
