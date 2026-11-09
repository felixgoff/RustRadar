//! The map pass. Placeholder: a lit, slowly turning globe with a graticule,
//! enough to exercise vertex/index buffers, a uniform, depth and MSAA.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::gpu::DEPTH_FORMAT;

const SHADER: &str = r#"
struct Globals {
    // xy: clip-space scale that keeps the globe round, z: rotation (rad), w: 1 if the target is sRGB-encoded by the GPU
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs(@location(0) pos: vec3<f32>) -> VsOut {
    let c = cos(globals.params.z);
    let s = sin(globals.params.z);
    // spin about the polar (y) axis, then tilt 23 degrees toward the viewer
    let spun = vec3<f32>(pos.x * c + pos.z * s, pos.y, -pos.x * s + pos.z * c);
    let tc = cos(0.4);
    let ts = sin(0.4);
    let p = vec3<f32>(spun.x, spun.y * tc - spun.z * ts, spun.y * ts + spun.z * tc);
    var out: VsOut;
    // +z faces the viewer; wgpu depth is 0 (near) .. 1 (far)
    out.clip = vec4<f32>(p.xy * globals.params.xy, 0.5 - 0.5 * p.z, 1.0);
    out.local = pos;
    out.normal = p;
    return out;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.local);
    let lat = degrees(asin(n.y));
    let lon = degrees(atan2(n.x, n.z));
    // 15 degree graticule, about 1.2px wide whatever the zoom
    let g = vec2<f32>(lat, lon) / 15.0;
    let d = abs(fract(g - 0.5) - 0.5) / fwidth(g);
    let line = 1.0 - clamp(min(d.x, d.y) - 0.2, 0.0, 1.0);
    let light = 0.35 + 0.65 * clamp(normalize(in.normal).z, 0.0, 1.0);
    let ocean = vec3<f32>(0.106, 0.196, 0.325);
    let grid = vec3<f32>(0.216, 0.557, 0.941);
    var color = mix(ocean, grid, line * 0.8) * light;
    // Colours are authored in gamma space, like CSS. Only convert when the GPU will re-encode.
    if (globals.params.w > 0.5) {
        color = to_linear(color);
    }
    return vec4<f32>(color, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    params: [f32; 4],
}

pub struct MapPass {
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    target_is_srgb: bool,
}

impl MapPass {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat, samples: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("map"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<[f32; 3]>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(target_format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let (vertex_data, index_data) = uv_sphere(64, 32);
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globe vertices"),
            contents: bytemuck::cast_slice(&vertex_data),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("globe indices"),
            contents: bytemuck::cast_slice(&index_data),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            pipeline,
            globals,
            bind_group,
            vertices,
            indices,
            index_count: index_data.len() as u32,
            target_is_srgb: target_format.is_srgb(),
        }
    }

    /// Uploads this frame's uniforms. `size` is the target in physical pixels.
    pub fn prepare(&self, queue: &wgpu::Queue, size: [u32; 2], rotation: f32) {
        // The globe fills 80% of the shorter side.
        let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
        let radius = 0.8 * w.min(h);
        let params = [radius / w, radius / h, rotation, self.target_is_srgb as u32 as f32];
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&Globals { params }));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// Unit sphere, y up (the poles), as positions and triangle-list indices.
fn uv_sphere(segments: u16, rings: u16) -> (Vec<[f32; 3]>, Vec<u16>) {
    use std::f32::consts::PI;
    let mut vertices = Vec::with_capacity((segments as usize + 1) * (rings as usize + 1));
    for ring in 0..=rings {
        let lat = PI * (0.5 - ring as f32 / rings as f32);
        for seg in 0..=segments {
            let lon = 2.0 * PI * seg as f32 / segments as f32;
            vertices.push([lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos()]);
        }
    }
    let mut indices = Vec::with_capacity(segments as usize * rings as usize * 6);
    let row = segments + 1;
    for ring in 0..rings {
        for seg in 0..segments {
            let a = ring * row + seg;
            let b = a + row;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    (vertices, indices)
}
