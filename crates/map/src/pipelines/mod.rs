//! The eight generic pipelines (plan 1.4.6), one file each, and [`build`], which gives every map
//! pipeline the same targets: the map pass's colour format with deck's blending, its sample count,
//! the depth/stencil state of a [`DrawState`] preset, and bind groups 0 (globals) and 1 (frame)
//! ahead of the pipeline's own.
//!
//! A pipeline's shader lives in `shaders/<name>.wgsl`, starts with `//!include lib/project.wgsl`,
//! and is embedded by its pipeline file (`include_str!("../shaders/<name>.wgsl")`) and compiled with
//! `shaders::module`.

pub mod bitmap;
pub mod circle;
pub mod fill;
pub mod icon;
pub mod mesh;
pub mod path;
pub mod shadow;
pub mod text;

use crate::layers::Init;
use crate::state::{self, DrawState};

/// What differs between map pipelines.
pub struct Desc<'a> {
    pub label: &'a str,
    pub module: &'a wgpu::ShaderModule,
    pub vertex_entry: &'a str,
    pub fragment_entry: &'a str,
    /// The vertex buffers, `None` for a slot the pipeline leaves unused.
    pub buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    pub topology: wgpu::PrimitiveTopology,
    pub state: DrawState,
    /// Bind group layouts after groups 0 and 1 (group 2 on).
    pub groups: &'a [&'a wgpu::BindGroupLayout],
}

/// A render pipeline for the map pass.
pub fn build(init: &Init<'_>, desc: &Desc<'_>) -> wgpu::RenderPipeline {
    let device = &init.gpu.device;
    let layout = init.bindings.pipeline_layout(device, desc.label, desc.groups);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(desc.label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: desc.module,
            entry_point: Some(desc.vertex_entry),
            compilation_options: Default::default(),
            buffers: desc.buffers,
        },
        primitive: desc.state.primitive(desc.topology),
        depth_stencil: Some(desc.state.depth_stencil()),
        multisample: init.multisample(),
        fragment: Some(wgpu::FragmentState {
            module: desc.module,
            entry_point: Some(desc.fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(state::color_target(init.color_format))],
        }),
        multiview_mask: None,
        cache: None,
    })
}
