//! The `fill` pipeline (plan 1.4.6): deck.gl's SolidPolygonLayer as the app draws it (filled, not
//! extruded, unlit), for the land (doc 01 §5.5), the vector map's water and aeroway areas (§5.6),
//! the night (§5.10), the radar's bands (doc 02 §3.5) and ground areas. Its shader is
//! `shaders/fill.wgsl`.
//!
//! - [`Vertex`]: `(u, v, h)` in the draw's local Mercator frame ([`crate::frame::ViewFrame::local`],
//!   [`crate::frame::ViewFrame::tile`]) and an RGBA colour in gamma-space bytes (deck's
//!   `getFillColor`; the realistic ground's "paints" use the same layout with their own shader).
//!   16 bytes, `#[repr(C)]`: a producer in `scene` can build the same layout and hand it over with
//!   `bytemuck::cast_slice`.
//! - [`Fill`]: the pipeline for one draw-state preset, `SURFACE` for the land, `SURFACE_NO_CULL` for
//!   the vector map, the night and the ground radar, `RADAR_LIFTED` for the lifted radar, `SHADOW`
//!   for the shadow variant (item E7).
//! - [`Style`]: a uniform the vertex colours are multiplied by, `(1, 1, 1, opacity)` for deck's
//!   `layer.opacity` (the colour goes out as `vec4(rgb, a · opacity)`, deck's default blending).
//! - [`Mesh`]: a vertex buffer and a `u32` index buffer (any vertex type; [`crate::pipelines::bitmap`]
//!   and the earth use it too).
//!
//! Winding: the culled presets drop back faces, and front faces are counter-clockwise on the screen
//! (`state`). A triangle is front-facing from above (or from outside the globe) when it runs
//! counter-clockwise on the map with north up, which is clockwise in `(u, v)`, as `v` grows south.
//! [`counter_clockwise_on_the_map`] orders a triangle that way.

use std::marker::PhantomData;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};

use super::{Desc, build};
use crate::frame::FrameSlot;
use crate::layers::{DrawCx, Init};
use crate::shaders;
use crate::state::DrawState;

/// `shaders/fill.wgsl`.
pub const SHADER: &str = include_str!("../shaders/fill.wgsl");

/// The bind group of the [`Style`].
pub const STYLE_GROUP: u32 = 2;

/// [`Style`] colour that leaves the vertices' colours as they are.
pub const OPAQUE: [f32; 4] = [1.0; 4];

/// One vertex of a fill.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    /// `u` east and `v` south in the frame's local units, and the height in metres.
    pub position: [f32; 3],
    /// RGBA, 0–255, gamma space.
    pub color: [u8; 4],
}

impl Vertex {
    /// The vertex buffer layout: position at location 0, colour (`Unorm8x4`) at location 1.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
    };
}

/// `fill.wgsl`'s `Style`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StyleUniform {
    /// Multiplies each vertex's RGBA (0–1).
    pub color: [f32; 4],
}

/// A per-draw colour multiplier (group 2), written whenever the layer's opacity changes.
pub struct Style {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl Style {
    /// Sets the multiplier: `(1, 1, 1, opacity)` for deck's layer opacity, [`OPAQUE`] for none.
    pub fn set(&self, queue: &wgpu::Queue, color: [f32; 4]) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&StyleUniform { color }));
    }

    /// Binds it as group 2.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(STYLE_GROUP, &self.group, &[]);
    }
}

/// A uniform bind group layout of one buffer of `size` bytes (both stages).
pub(crate) fn uniform_layout(device: &wgpu::Device, label: &str, size: usize) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(size as u64),
            },
            count: None,
        }],
    })
}

/// A uniform buffer holding `value` and its bind group on `layout`.
pub(crate) fn uniform_group<T: Pod>(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    value: &T,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = wgpu::util::DeviceExt::create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::bytes_of(value),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        },
    );
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
    });
    (buffer, group)
}

/// Triangles on the GPU: vertices of type `V` and `u32` indices.
pub struct Mesh<V> {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    vertex: PhantomData<fn() -> V>,
}

impl<V: Pod> Mesh<V> {
    /// Uploads `vertices` and three `indices` per triangle. Empty input makes a mesh that draws
    /// nothing.
    pub fn new(device: &wgpu::Device, label: &str, vertices: &[V], indices: &[u32]) -> Self {
        // wgpu refuses empty vertex slices: keep a few bytes so the buffers exist
        let buffer = |contents: &[u8], usage| {
            let mut padded = contents.to_vec();
            padded.resize(contents.len().max(16), 0);
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor { label: Some(label), contents: &padded, usage },
            )
        };
        let index_count = if vertices.is_empty() { 0 } else { indices.len() as u32 };
        Mesh {
            vertices: buffer(bytemuck::cast_slice(vertices), wgpu::BufferUsages::VERTEX),
            indices: buffer(bytemuck::cast_slice(indices), wgpu::BufferUsages::INDEX),
            index_count,
            vertex: PhantomData,
        }
    }

    /// How many indices it draws (three per triangle).
    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    /// Sets its buffers (vertex slot 0) and draws it once, with the pipeline and bind groups
    /// already set.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.index_count == 0 {
            return;
        }
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// The fill pipeline for one draw-state preset.
pub struct Fill {
    pipeline: wgpu::RenderPipeline,
    style_layout: wgpu::BindGroupLayout,
}

impl Fill {
    /// The pipeline with `state` (for the map pass of `init`).
    pub fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        let device = &init.gpu.device;
        let style_layout = uniform_layout(device, "fill style", size_of::<StyleUniform>());
        let module = shaders::module(device, label, SHADER);
        let pipeline = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(Vertex::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state,
                groups: &[&style_layout],
            },
        );
        Fill { pipeline, style_layout }
    }

    /// A style for this pipeline, multiplying the vertex colours by `color`.
    pub fn style(&self, device: &wgpu::Device, color: [f32; 4]) -> Style {
        let (buffer, group) = uniform_group(device, "fill style", &self.style_layout, &StyleUniform { color });
        Style { buffer, group }
    }

    /// Sets the pipeline and `style`; then bind a frame and draw meshes.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>, style: &Style) {
        pass.set_pipeline(&self.pipeline);
        style.bind(pass);
    }

    /// Draws `mesh` in the local Mercator `frame` with `style`.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, frame: FrameSlot, style: &Style, mesh: &Mesh<Vertex>) {
        self.bind(pass, style);
        cx.bind_frame(pass, frame);
        mesh.draw(pass);
    }
}

/// The triangle `[a, b, c]` of `points` (`[x east, y north]`, any planar map coordinates) in the
/// order that is counter-clockwise on the map, so the culled presets keep it.
pub fn counter_clockwise_on_the_map(points: &[[f64; 2]], [a, b, c]: [u32; 3]) -> [u32; 3] {
    let [p, q, r] = [a, b, c].map(|i| points[i as usize]);
    let cross = (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    if cross < 0.0 { [a, c, b] } else { [a, b, c] }
}

#[cfg(test)]
pub(crate) mod tests {
    use glam::DVec2;
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, lng_lat_to_world};

    use super::*;
    use crate::frame::power_of_two_scale;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{Layer, PrepareCx};
    use crate::renderer::{MapRenderer, SceneInput};

    /// The members of WGSL struct `name` in `source` with their offsets, and its size.
    pub(crate) fn wgsl_members(source: &str, name: &str) -> (u32, Vec<(String, u32)>) {
        let module = crate::shaders::tests::validate(name, &crate::shaders::expand(source).expect("libraries known"));
        let ty = module.types.iter().find(|(_, t)| t.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no struct {name}"));
        match &ty.1.inner {
            wgpu::naga::TypeInner::Struct { members, span } => {
                (*span, members.iter().map(|m| (m.name.clone().expect("named"), m.offset)).collect())
            }
            other => panic!("{name} is {other:?}"),
        }
    }

    #[test]
    fn layouts_match_the_wgsl() {
        assert_eq!(size_of::<Vertex>(), 16);
        assert_eq!(std::mem::offset_of!(Vertex, color), 12);
        assert_eq!(Vertex::LAYOUT.array_stride, 16);
        assert_eq!(wgsl_members(SHADER, "Style"), (16, vec![("color".to_string(), 0)]));
    }

    #[test]
    fn triangles_are_turned_counter_clockwise_on_the_map() {
        let points = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        assert_eq!(counter_clockwise_on_the_map(&points, [0, 1, 2]), [0, 1, 2]);
        assert_eq!(counter_clockwise_on_the_map(&points, [0, 2, 1]), [0, 1, 2]);
        assert_eq!(counter_clockwise_on_the_map(&points, [2, 1, 0]), [2, 0, 1]);
    }

    /// A test layer: squares of local Mercator around the view centre, each with its own style and
    /// winding, drawn in order with one [`Fill`].
    pub(crate) struct Squares {
        fill: Fill,
        /// mesh, style, frame anchor, scale
        squares: Vec<(Mesh<Vertex>, Style, DVec2, f64)>,
        slots: Vec<FrameSlot>,
    }

    impl Squares {
        pub(crate) fn new(init: &Init<'_>, state: DrawState) -> Self {
            Squares { fill: Fill::new(init, "test fill", state), squares: Vec::new(), slots: Vec::new() }
        }

        /// A square of `scale` world units with its north-west corner at `anchor`, in `color`,
        /// styled by `style`; `clockwise` reverses its triangles (back faces from above).
        pub(crate) fn add(&mut self, init: &Init<'_>, anchor: DVec2, scale: f64, color: [u8; 4], style: [f32; 4], clockwise: bool) {
            let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
            let vertices: Vec<Vertex> = corners.iter().map(|&[u, v]| Vertex { position: [u, v, 0.0], color }).collect();
            // on the map (north up): (0,1) (1,1) (1,0) (0,0)
            let map: Vec<[f64; 2]> = corners.iter().map(|&[u, v]| [f64::from(u), -f64::from(v)]).collect();
            let mut indices = Vec::new();
            for t in [[0, 1, 2], [0, 2, 3]] {
                let [a, b, c] = counter_clockwise_on_the_map(&map, t);
                indices.extend(if clockwise { [a, c, b] } else { [a, b, c] });
            }
            let device = &init.gpu.device;
            self.squares.push((Mesh::new(device, "square", &vertices, &indices), self.fill.style(device, style), anchor, scale));
        }
    }

    impl Layer for Squares {
        fn name(&self) -> &'static str {
            "test squares"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.slots = self.squares.iter().map(|(_, _, anchor, scale)| cx.frames.push(&cx.view.local(*anchor, *scale, 0.0))).collect();
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            for ((mesh, style, _, _), slot) in self.squares.iter().zip(&self.slots) {
                self.fill.draw(pass, cx, *slot, style, mesh);
            }
        }
    }

    fn render(gpu: &crate::gpu::Gpu, view: ViewState, build: impl FnOnce(&Init<'_>, DVec2, f64) -> Squares) -> Image {
        let scene = SceneInput::new(view, [200.0, 200.0], 1.0, Basemap::Dark);
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let centre = DVec2::from_array(lng_lat_to_world([view.longitude, view.latitude]));
        // a square of 60 to 120 px (a world unit is about 2^zoom px at the centre in both
        // projections), its middle on the view centre
        let scale = power_of_two_scale(60.0 / 2f64.powf(view.zoom));
        let anchor = centre + DVec2::new(-scale / 2.0, scale / 2.0);
        let squares = build(&renderer.init(gpu), anchor, scale);
        renderer.add_layer(Box::new(squares));
        render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered")
    }

    /// One layer of the night's `[2,6,14,46]` over the ink, as deck blends it, and a red square at
    /// half opacity; four night layers stack as doc 01 §5.10 says.
    #[test]
    fn fills_blend_as_deck_draws_them() {
        let Some(gpu) = gpu() else { return };
        let views = [
            ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, ..ViewState::INITIAL },
            ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, bearing: 30.0, pitch: 45.0, position: None },
        ];
        for view in views {
            let night = [2, 6, 14, 46];
            let one = render(&gpu, view, |init, anchor, scale| {
                let mut s = Squares::new(init, DrawState::SURFACE_NO_CULL);
                s.add(init, anchor, scale, night, OPAQUE, false);
                s
            });
            // 30 + (c − 30)·46/255: 24.95, 25.67, 27.11
            assert_eq!(one.pixel(100, 100), [25, 26, 27, 255], "{view:?}");
            assert_eq!(one.pixel(2, 2), [30, 30, 30, 255], "{view:?}");
            let four = render(&gpu, view, |init, anchor, scale| {
                let mut s = Squares::new(init, DrawState::SURFACE_NO_CULL);
                for _ in 0..4 {
                    s.add(init, anchor, scale, night, OPAQUE, false);
                }
                s
            });
            let expected = (0..4).fold([30.0f64, 30.0, 30.0], |c, _| {
                std::array::from_fn(|i| (c[i] + (f64::from(night[i]) - c[i]) * 46.0 / 255.0).round())
            });
            let got = four.pixel(100, 100);
            for i in 0..3 {
                assert!((f64::from(got[i]) - expected[i]).abs() <= 1.0, "{view:?}: {got:?} against {expected:?}");
            }
            // doc 01 §5.10 and item B5: full night is (15,17,21) ± 2
            for (g, e) in got.iter().zip([15, 17, 21]) {
                assert!((i32::from(*g) - e).abs() <= 2, "{got:?}");
            }
            let half = render(&gpu, view, |init, anchor, scale| {
                let mut s = Squares::new(init, DrawState::SURFACE);
                s.add(init, anchor, scale, [255, 0, 0, 255], [1.0, 1.0, 1.0, 0.5], false);
                s
            });
            let p = half.pixel(100, 100);
            assert!((i32::from(p[0]) - 143).abs() <= 1 && p[1] == 15 && p[2] == 15, "{view:?}: {p:?}");
        }
    }

    /// The culled presets keep triangles that run counter-clockwise on the map and drop the others,
    /// on the globe and on the flat map; `SURFACE_NO_CULL` keeps both.
    #[test]
    fn culling_keeps_counter_clockwise_triangles() {
        let Some(gpu) = gpu() else { return };
        for view in [
            ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 25.0, pitch: 50.0, position: None },
            ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, bearing: 30.0, pitch: 45.0, position: None },
        ] {
            for (state, clockwise, shown) in [
                (DrawState::SURFACE, false, true),
                (DrawState::SURFACE, true, false),
                (DrawState::DEFAULT, true, false),
                (DrawState::SURFACE_NO_CULL, true, true),
            ] {
                let image = render(&gpu, view, |init, anchor, scale| {
                    let mut s = Squares::new(init, state);
                    s.add(init, anchor, scale, [200, 100, 50, 255], OPAQUE, clockwise);
                    s
                });
                let expected = if shown { [200, 100, 50, 255] } else { [30, 30, 30, 255] };
                assert_eq!(image.pixel(100, 100), expected, "{view:?} {state:?} clockwise {clockwise}");
            }
        }
    }
}
