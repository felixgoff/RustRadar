//! Draws one frame: the map pass, then the egui overlay on top.

use crate::gpu::{DEPTH_FORMAT, Gpu};
use crate::map::MapPass;
use crate::{theme, ui};

/// Multisampled colour and depth buffers of the map pass, remade when the target is resized.
struct Attachments {
    size: [u32; 2],
    /// `None` without MSAA: the map then draws straight into the target.
    msaa_color: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
}

pub struct Renderer {
    target_format: wgpu::TextureFormat,
    samples: u32,
    map: MapPass,
    attachments: Option<Attachments>,
    egui_ctx: egui::Context,
    egui_renderer: egui_wgpu::Renderer,
    pub ui_state: ui::UiState,
}

impl Renderer {
    /// `target_format` is the format of the views later passed to [`Self::render`].
    /// Prefer a non-sRGB one (`Rgba8Unorm`/`Bgra8Unorm`): egui blends in gamma
    /// space like a browser does, and the CSS colours then land on screen unchanged.
    pub fn new(gpu: &Gpu, target_format: wgpu::TextureFormat) -> Self {
        let samples = gpu.msaa_samples(target_format);
        let egui_ctx = egui::Context::default();
        theme::apply(&egui_ctx);
        Self {
            target_format,
            samples,
            map: MapPass::new(&gpu.device, target_format, samples),
            attachments: None,
            // egui anti-aliases its own shapes by feathering; it draws after the MSAA resolve.
            egui_renderer: egui_wgpu::Renderer::new(&gpu.device, target_format, egui_wgpu::RendererOptions::default()),
            egui_ctx,
            ui_state: ui::UiState::default(),
        }
    }

    pub fn egui_ctx(&self) -> &egui::Context {
        &self.egui_ctx
    }

    pub fn msaa_samples(&self) -> u32 {
        self.samples
    }

    /// Renders a frame into `target` (`size` in physical pixels) and submits it.
    ///
    /// `raw_input` carries the UI events, the screen rect in points and the
    /// scale factor (egui-winit fills these in; see `headless.rs` for doing it by hand).
    /// `time` is seconds since start, for animation.
    pub fn render(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        size: [u32; 2],
        raw_input: egui::RawInput,
        time: f32,
    ) -> egui::PlatformOutput {
        let (device, queue) = (&gpu.device, &gpu.queue);
        self.ensure_attachments(device, size);
        let attachments = self.attachments.as_ref().expect("made by ensure_attachments");

        let ui_state = &mut self.ui_state;
        let output = self.egui_ctx.run_ui(raw_input, |ui| ui::draw(ui, ui_state));
        let primitives = self.egui_ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: output.pixels_per_point };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        // TexturesDelta asserts on drop that both lists were drained.
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui_renderer.update_texture(device, queue, id, delta);
            }
        }
        let mut commands = self.egui_renderer.update_buffers(device, queue, &mut encoder, &primitives, &screen);

        self.map.prepare(queue, size, time * 0.15);
        {
            let (view, resolve_target) = match &attachments.msaa_color {
                Some(msaa) => (msaa, Some(target)),
                None => (target, None),
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("map"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear_color(theme::CANVAS, self.target_format)),
                        // With MSAA only the resolved image is kept.
                        store: if resolve_target.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &attachments.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                }),
                ..Default::default()
            });
            self.map.draw(&mut pass);
        }
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &primitives, &screen);
        }
        commands.push(encoder.finish());
        queue.submit(commands);

        for id in textures.free.drain() {
            self.egui_renderer.free_texture(&id);
        }
        output.platform_output
    }

    fn ensure_attachments(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.attachments.as_ref().is_some_and(|a| a.size == size) {
            return;
        }
        let texture = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0].max(1), height: size[1].max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: self.samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        self.attachments = Some(Attachments {
            size,
            msaa_color: (self.samples > 1).then(|| texture("map msaa colour", self.target_format)),
            depth: texture("map depth", DEPTH_FORMAT),
        });
    }
}

/// A CSS colour as the clear value that reproduces it in `format`:
/// as-is for gamma-space targets, linearised for `*Srgb` ones.
fn clear_color(color: egui::Color32, format: wgpu::TextureFormat) -> wgpu::Color {
    let channel = |c: u8| {
        let c = c as f64 / 255.0;
        if !format.is_srgb() {
            c
        } else if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    wgpu::Color { r: channel(color.r()), g: channel(color.g()), b: channel(color.b()), a: 1.0 }
}
