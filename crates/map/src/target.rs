//! The offscreen map target and the composite pass (plan 1.4.1, 1.4.4).
//!
//! The map is always drawn into an `Rgba8Unorm` texture (not sRGB) with 4× MSAA where the adapter
//! has it and a [`DEPTH_FORMAT`] attachment, resolved into a single-sample texture: colours from the
//! specs go in unchanged and blend in gamma space, as on the WebGL canvas (doc 01 §10.1). The
//! composite pass then draws that texture onto the surface, bilinear when the sizes differ.

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

use bytemuck::{Pod, Zeroable};

use crate::gpu::DEPTH_FORMAT;
use crate::shaders;

/// The map texture's format.
pub const MAP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The map's clear colour, `--ink` `#1e1e1e`: the "space" around the globe on dark and satellite;
/// on realistic the sky layer paints over it (doc 04 §3.6).
pub const CLEAR_RGB: [u8; 3] = [30, 30, 30];

/// [`CLEAR_RGB`] as a clear value for the gamma-space map texture.
pub const CLEAR: wgpu::Color =
    wgpu::Color { r: CLEAR_RGB[0] as f64 / 255.0, g: CLEAR_RGB[1] as f64 / 255.0, b: CLEAR_RGB[2] as f64 / 255.0, a: 1.0 };

/// The map's attachments at one size.
pub struct MapTarget {
    size: [u32; 2],
    samples: u32,
    /// `None` without MSAA: the map pass then draws straight into `resolved`.
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    resolved: wgpu::Texture,
    resolved_view: wgpu::TextureView,
    /// Unique to these textures, so bind groups made on them know when to follow.
    generation: u64,
}

impl MapTarget {
    /// Attachments of `size` map pixels with `samples` (4 or 1) samples.
    pub fn new(device: &wgpu::Device, size: [u32; 2], samples: u32) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let texture = |label, format, sample_count, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let resolved = texture(
            "map",
            MAP_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa = (samples > 1)
            .then(|| texture("map msaa", MAP_FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default()));
        let depth = texture("map depth", DEPTH_FORMAT, samples, wgpu::TextureUsages::RENDER_ATTACHMENT).create_view(&Default::default());
        let resolved_view = resolved.create_view(&Default::default());
        static GENERATIONS: AtomicU64 = AtomicU64::new(1);
        let generation = GENERATIONS.fetch_add(1, Ordering::Relaxed);
        MapTarget { size, samples, msaa, depth, resolved, resolved_view, generation }
    }

    /// Remakes the attachments if `size` changed.
    pub fn ensure_size(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if size != self.size {
            *self = Self::new(device, size, self.samples);
        }
    }

    /// Map pixels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// MSAA samples of the map pass (4, or 1 where the adapter cannot).
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// The resolved map picture (single-sample, `MAP_FORMAT`).
    pub fn texture(&self) -> &wgpu::Texture {
        &self.resolved
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.resolved_view
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Begins the map pass: colour cleared to `clear`, depth to 1, stencil to 0 (once per frame,
    /// for the shadows' rule). Only the resolved picture is kept.
    pub fn begin_pass<'e>(&self, encoder: &'e mut wgpu::CommandEncoder, clear: wgpu::Color) -> wgpu::RenderPass<'e> {
        let (view, resolve_target) = match &self.msaa {
            Some(msaa) => (msaa, Some(&self.resolved_view)),
            None => (&self.resolved_view, None),
        };
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: if resolve_target.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
            }),
            ..Default::default()
        })
    }
}

/// `composite.wgsl`'s `Composite`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
struct CompositeParams {
    target_size: [f32; 2],
    flags: u32,
    pad: u32,
}

/// Linearise for an sRGB surface (`CompositeParams::flags`).
const LINEARISE: u32 = 1;

/// The pass that draws the map texture onto a surface of one format.
pub struct Composite {
    format: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    /// The bind group and the map target generation it was made for.
    group: Option<(u64, wgpu::BindGroup)>,
}

impl Composite {
    /// A composite pass onto surfaces of `format`.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(size_of::<CompositeParams>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let shader = shaders::module(device, "composite", shaders::COMPOSITE);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("composite"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite"),
            size: size_of::<CompositeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Composite { format, pipeline, layout, sampler, params, group: None }
    }

    /// The surface format this pass draws to.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Records the pass: `map` onto `surface` (`surface_size` pixels), every pixel written.
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        map: &MapTarget,
        surface: &wgpu::TextureView,
        surface_size: [u32; 2],
    ) {
        let params = CompositeParams {
            target_size: [surface_size[0].max(1) as f32, surface_size[1].max(1) as f32],
            flags: if self.format.is_srgb() { LINEARISE } else { 0 },
            pad: 0,
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        if self.group.as_ref().is_none_or(|(generation, _)| *generation != map.generation()) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(map.view()) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
                ],
            });
            self.group = Some((map.generation(), group));
        }
        let (_, group) = self.group.as_ref().expect("made above");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface,
                depth_slice: None,
                resolve_target: None,
                // every pixel is written
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}
