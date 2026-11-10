//! The `path` pipeline (plan 1.4.6, item E6): deck.gl 9.4's PathLayer with PathStyleExtension's
//! dashes, as the app draws its lines: borders, roads, runways at true width, rail, the trail and
//! the route, blocks, train routes, ground lines. Its shader is `shaders/path.wgsl`.
//!
//! - [`Segment`] (`scene::geom::path::PathSegment`, made by `scene::geom::path::PathBuilder`): one
//!   instance per segment, its start and end with the vertices before and after, in the draw's
//!   local Mercator frame, with width, colour, dash array and whole-path dash metrics.
//! - [`PathStyle`]: deck's per-layer props (`widthUnits`, `widthScale`, `widthMinPixels`,
//!   `widthMaxPixels`, `jointRounded`, `capRounded`, `miterLimit`, `opacity`) and
//!   PathStyleExtension's (`dash`, `highPrecisionDash`, `dashJustified`). [`Style`] is it on the GPU
//!   (group 2).
//! - [`Mesh`]: an instance buffer of segments. [`Path`]: the pipeline for one draw-state preset.
//!
//! What deck does and this does too: the line lies flat on the ground (deck's `billboard: false`,
//! the app's only use), its width turned from pixels into common units at the view centre, so it
//! keeps its width across the view at pitch 0 and foreshortens like the ground when pitched (a line
//! running across the view at pitch 60 is half as wide on the screen as one running into it);
//! metres use the view centre's scale; joints and caps are worked out in the plane of the ground at
//! each vertex, miters are cut where they reach `miter_limit + 1` half widths from the joint; dash
//! arrays count half widths (deck's `dashUnits: 'widths'`); whole-path dashes measure along the 3D
//! path in common units ([`Dash::Path`]); justified dashes fit a whole number of periods to the
//! segment, or to the path, starting and ending on half a dash. deck's `antialiasing` prop (off by
//! default and in the app), `billboard`, `dashGapPickable`, `getOffset` and the other dash units
//! are not ported. A path on the globe should be cut every 5° by its producer
//! (`scene::geom::grid_cut::cut_polyline_by_grid`, as deck's normalisation does), or it cuts
//! through the sphere.

use bytemuck::{Pod, Zeroable};

use super::{Desc, build};
use crate::frame::FrameSlot;
use crate::layers::{DrawCx, Init};
use crate::shaders;
use crate::state::DrawState;

pub use rustradar_scene::geom::path::PathSegment as Segment;

/// `shaders/path.wgsl`.
pub const SHADER: &str = include_str!("../shaders/path.wgsl");

/// The bind group of the [`Style`].
pub const STYLE_GROUP: u32 = 2;

/// deck's `widthMaxPixels` default, `Number.MAX_SAFE_INTEGER`.
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// The corners of a segment (deck's `SEGMENT_POSITIONS`): `x` 0 at the start and 1 at the end,
/// `y` the side (−1, 1) or the joint (0).
const CORNERS: [[f32; 2]; 6] = [[0.0, 0.0], [0.0, -1.0], [0.0, 1.0], [1.0, -1.0], [1.0, 1.0], [1.0, 0.0]];

/// deck's `SEGMENT_INDICES`: the start corner, the body, the end corner.
const CORNER_INDICES: [u16; 12] = [0, 1, 2, 1, 4, 2, 1, 3, 4, 3, 5, 4];

/// The instance buffer layout of [`Segment`] (locations 1–10; 0 is the corner).
pub const SEGMENT_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: size_of::<Segment>() as u64,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &wgpu::vertex_attr_array![
        1 => Float32x3, 2 => Float32x3, 3 => Float32x3, 4 => Float32x3, 5 => Float32,
        6 => Unorm8x4, 7 => Float32x2, 8 => Float32x2, 9 => Float32x2, 10 => Uint32
    ],
};

const CORNER_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 8,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
};

/// deck's `widthUnits` (its `UNIT` numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    Common = 0,
    Meters = 1,
    Pixels = 2,
}

/// PathStyleExtension, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dash {
    /// No extension: `dash_array` is ignored.
    None,
    /// `{ dash: true }`: the pattern starts again at each segment.
    Segment,
    /// `{ dash: true, highPrecisionDash: true }`: the pattern runs along the whole path (needs the
    /// metrics of `PathBuilder::measuring`).
    Path,
}

/// A path layer's props.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathStyle {
    pub width_units: Units,
    pub width_scale: f64,
    pub width_min_pixels: f64,
    pub width_max_pixels: f64,
    pub joint_rounded: bool,
    pub cap_rounded: bool,
    pub miter_limit: f64,
    /// deck's layer `opacity`, multiplying each segment's alpha.
    pub opacity: f64,
    pub dash: Dash,
    /// `dashJustified`.
    pub dash_justified: bool,
}

impl PathStyle {
    /// deck's defaults: widths in metres, scale 1, no clamp, miter joints (limit 4), butt caps,
    /// opaque, no dashes.
    pub const DEFAULT: PathStyle = PathStyle {
        width_units: Units::Meters,
        width_scale: 1.0,
        width_min_pixels: 0.0,
        width_max_pixels: MAX_SAFE_INTEGER,
        joint_rounded: false,
        cap_rounded: false,
        miter_limit: 4.0,
        opacity: 1.0,
        dash: Dash::None,
        dash_justified: false,
    };

    /// The defaults with widths in pixels (`widthUnits: "pixels"`, most of the app's lines).
    pub const PIXELS: PathStyle = PathStyle { width_units: Units::Pixels, ..Self::DEFAULT };

    /// The uniform the shader reads.
    pub fn uniform(&self) -> StyleUniform {
        StyleUniform {
            width_scale: self.width_scale as f32,
            width_min_pixels: self.width_min_pixels as f32,
            width_max_pixels: self.width_max_pixels as f32,
            joint_type: if self.joint_rounded { 1.0 } else { 0.0 },
            cap_type: if self.cap_rounded { 1.0 } else { 0.0 },
            miter_limit: self.miter_limit as f32,
            width_units: self.width_units as u32,
            opacity: self.opacity as f32,
            dash_mode: match self.dash {
                Dash::None => 0,
                Dash::Segment => 1,
                Dash::Path => 2,
            },
            dash_align_mode: if self.dash_justified { 1.0 } else { 0.0 },
            pad: [0.0; 2],
        }
    }
}

impl Default for PathStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `path.wgsl`'s `PathStyle`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StyleUniform {
    pub width_scale: f32,
    pub width_min_pixels: f32,
    pub width_max_pixels: f32,
    pub joint_type: f32,
    pub cap_type: f32,
    pub miter_limit: f32,
    pub width_units: u32,
    pub opacity: f32,
    pub dash_mode: u32,
    pub dash_align_mode: f32,
    pub pad: [f32; 2],
}

/// A [`PathStyle`] on the GPU (group 2), one per draw that differs.
pub struct Style {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    style: PathStyle,
}

impl Style {
    /// The style it holds.
    pub fn get(&self) -> &PathStyle {
        &self.style
    }

    /// Changes it (written only when it differs).
    pub fn set(&mut self, queue: &wgpu::Queue, style: &PathStyle) {
        if *style != self.style {
            self.style = *style;
            queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&style.uniform()));
        }
    }
}

/// Segments on the GPU: one instance buffer.
pub struct Mesh {
    buffer: wgpu::Buffer,
    count: u32,
}

impl Mesh {
    /// Uploads `segments` (none makes a mesh that draws nothing).
    pub fn new(device: &wgpu::Device, label: &str, segments: &[Segment]) -> Self {
        let buffer = if segments.is_empty() {
            Self::allocate(device, label, 1)
        } else {
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::cast_slice(segments),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                },
            )
        };
        Mesh { buffer, count: segments.len() as u32 }
    }

    fn allocate(device: &wgpu::Device, label: &str, count: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (count.max(1) * size_of::<Segment>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Replaces its segments (a dynamic path: trail, route), growing the buffer when they do not
    /// fit.
    pub fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, segments: &[Segment]) {
        let bytes: &[u8] = bytemuck::cast_slice(segments);
        if bytes.len() as u64 > self.buffer.size() {
            self.buffer = Self::allocate(device, label, segments.len().next_power_of_two());
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytes);
        }
        self.count = segments.len() as u32;
    }

    /// How many segments it draws.
    pub fn len(&self) -> u32 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// The path pipeline for one draw-state preset (`SURFACE_NO_CULL` for the map's lines, `NO_CULL`
/// for the trail and the route, `THROUGH_GROUND`-like states for the train route, ...). Every path
/// layer of the app culls nothing: deck's triangles turn either way, so a culling preset would drop
/// some of them.
pub struct Path {
    pipeline: wgpu::RenderPipeline,
    style_layout: wgpu::BindGroupLayout,
    corners: wgpu::Buffer,
    indices: wgpu::Buffer,
}

impl Path {
    /// The pipeline with `state` for the map pass of `init`.
    pub fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        let device = &init.gpu.device;
        let style_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("path style"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(size_of::<StyleUniform>() as u64),
                },
                count: None,
            }],
        });
        let module = shaders::module(device, label, SHADER);
        let pipeline = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(CORNER_LAYOUT), Some(SEGMENT_LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleList,
                state,
                groups: &[&style_layout],
            },
        );
        let buffer = |contents: &[u8], usage| {
            wgpu::util::DeviceExt::create_buffer_init(device, &wgpu::util::BufferInitDescriptor { label: Some("path corners"), contents, usage })
        };
        let corners = buffer(bytemuck::cast_slice(&CORNERS), wgpu::BufferUsages::VERTEX);
        let indices = buffer(bytemuck::cast_slice(&CORNER_INDICES), wgpu::BufferUsages::INDEX);
        Path { pipeline, style_layout, corners, indices }
    }

    /// A [`Style`] for this pipeline.
    pub fn style(&self, device: &wgpu::Device, style: &PathStyle) -> Style {
        let buffer = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("path style"),
                contents: bytemuck::bytes_of(&style.uniform()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("path style"),
            layout: &self.style_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        Style { buffer, group, style: *style }
    }

    /// Sets the pipeline and the corners; then [`Self::draw_mesh`] as often as needed.
    pub fn begin(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.corners.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
    }

    /// Draws `mesh` in the local Mercator `frame` with `style`, after [`Self::begin`].
    pub fn draw_mesh(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, frame: FrameSlot, style: &Style, mesh: &Mesh) {
        if mesh.is_empty() {
            return;
        }
        cx.bind_frame(pass, frame);
        pass.set_bind_group(STYLE_GROUP, &style.group, &[]);
        pass.set_vertex_buffer(1, mesh.buffer.slice(..));
        pass.draw_indexed(0..CORNER_INDICES.len() as u32, 0, 0..mesh.count);
    }

    /// [`Self::begin`] and [`Self::draw_mesh`].
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, frame: FrameSlot, style: &Style, mesh: &Mesh) {
        self.begin(pass);
        self.draw_mesh(pass, cx, frame, style, mesh);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path as FilePath;

    use glam::{DVec2, DVec3};
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{Viewport, ViewState, lng_lat_to_world, world_to_lng_lat};
    use rustradar_scene::geom::path::{Colors, LocalMercator, Paint, PathBuilder};

    use super::*;
    use crate::frame::power_of_two_scale;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{Layer, PrepareCx};
    use crate::renderer::{MapRenderer, SceneInput};

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const INK: f64 = 30.0;
    const STOCKHOLM: [f64; 2] = [18.07, 59.33];
    const ARLANDA: [f64; 2] = [17.9186, 59.6519];

    fn view([longitude, latitude]: [f64; 2], zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, pitch, bearing, position: None }
    }

    /// One mesh to draw: its frame, its segments and its style.
    struct Item {
        local: LocalMercator,
        segments: Vec<Segment>,
        style: PathStyle,
    }

    impl Item {
        fn new(builder: PathBuilder, style: PathStyle) -> Self {
            Item { local: builder.local(), segments: builder.finish(), style }
        }
    }

    /// A test layer: the items in order, each with its own frame and style, one pipeline.
    struct Lines {
        path: Path,
        items: Vec<(Mesh, Style, LocalMercator)>,
        slots: Vec<FrameSlot>,
    }

    impl Layer for Lines {
        fn name(&self) -> &'static str {
            "test paths"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.slots = self.items.iter().map(|(_, _, l)| cx.frames.push(&cx.view.local(l.anchor, l.scale, 0.0))).collect();
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.path.begin(pass);
            for ((mesh, style, _), slot) in self.items.iter().zip(&self.slots) {
                self.path.draw_mesh(pass, cx, *slot, style, mesh);
            }
        }
    }

    /// `items` drawn over the empty dark map, as a window of `scale_factor` shows it; the image and
    /// the frame's viewport.
    fn render(gpu: &crate::gpu::Gpu, view: ViewState, size: [f64; 2], scale_factor: f64, items: Vec<Item>) -> (Image, Viewport) {
        let scene = SceneInput::new(view, size, scale_factor, Basemap::Dark);
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let init = renderer.init(gpu);
        let path = Path::new(&init, "test path", DrawState::SURFACE_NO_CULL);
        let items = items
            .into_iter()
            .map(|i| (Mesh::new(&gpu.device, "test path", &i.segments), path.style(&gpu.device, &i.style), i.local))
            .collect();
        renderer.add_layer(Box::new(Lines { path, items, slots: Vec::new() }));
        let image = render_with(gpu, &mut renderer, &scene, scale_factor).expect("rendered");
        (image, renderer.view().viewport.clone())
    }

    fn save(image: &Image, name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots");
        std::fs::create_dir_all(dir).expect("shot folder");
        image.save_png(&FilePath::new(dir).join(name)).expect("png");
    }

    /// How much of a white line covers the pixel, 0–1, over the ink.
    fn alpha(image: &Image, x: u32, y: u32) -> f64 {
        (f64::from(image.pixel(x, y)[0]) - INK) / (255.0 - INK)
    }

    /// A frame around world Mercator `centre` for a mesh `half` world units each way.
    fn frame_around(centre: DVec2, half: f64) -> LocalMercator {
        let scale = power_of_two_scale(2.0 * half);
        LocalMercator::new(centre + DVec2::new(-scale / 2.0, scale / 2.0), scale)
    }

    /// A straight line in world Mercator through `centre`, `angle` degrees east of north, `half`
    /// world units each way, cut into `pieces`, at `h` metres; in `local`.
    fn line(local: &LocalMercator, centre: DVec2, angle: f64, half: f64, pieces: usize, h: f64) -> Vec<[f64; 3]> {
        let dir = DVec2::new(angle.to_radians().sin(), angle.to_radians().cos());
        (0..=pieces)
            .map(|i| {
                let t = -1.0 + 2.0 * i as f64 / pieces as f64;
                let [u, v] = local.from_world(centre + dir * half * t);
                [u, v, h]
            })
            .collect()
    }

    /// East and north at the view centre in common space.
    fn east_north(vp: &Viewport) -> (DVec3, DVec3) {
        match vp {
            Viewport::Globe(g) => {
                let (sl, cl) = g.longitude.to_radians().sin_cos();
                let (sp, cp) = g.latitude.to_radians().sin_cos();
                (DVec3::new(cl, sl, 0.0), DVec3::new(-sl * sp, cl * sp, cp))
            }
            Viewport::Mercator(_) => (DVec3::X, DVec3::Y),
        }
    }

    /// deck's width on the screen, at the view centre, of a line `px` wide running `angle` east of
    /// north: its ground offsets of half a width each way (common units at the centre's scale)
    /// projected, measured across the line as projected; and the line's direction on the screen.
    fn expected_width(vp: &Viewport, angle: f64, px: f64) -> (f64, DVec2) {
        let (e, n) = east_north(vp);
        let (s, c) = angle.to_radians().sin_cos();
        let along = e * s + n * c;
        let across = e * c - n * s;
        let half = px / 2.0 / vp.scale;
        let pixel = |offset: DVec3| {
            let [x, y, _] = vp.world_to_pixels(vp.center + offset);
            DVec2::new(x, y)
        };
        let direction = (pixel(along * 1e-4 * half) - pixel(-along * 1e-4 * half)).normalize();
        let width = (pixel(across * half) - pixel(-across * half)).perp_dot(direction).abs();
        (width, direction)
    }

    /// The width of the line through the image's centre: its coverage in a box of `2r` pixels
    /// around the centre over its length in the box (the line runs along `direction`).
    fn measured_width(image: &Image, direction: DVec2, r: u32) -> f64 {
        let (cx, cy) = (image.width / 2, image.height / 2);
        let mut area = 0.0;
        for y in cy - r..cy + r {
            for x in cx - r..cx + r {
                area += alpha(image, x, y);
            }
        }
        area / (2.0 * f64::from(r) / direction.x.abs().max(direction.y.abs()))
    }

    /// Item E6's acceptance: a 1 px and a 0.6 px line keep their width at zoom 3 and 11 (globe)
    /// and 15 (flat map), at pitch 0 and 60, measured by their coverage within 15 %. Pitched, deck's
    /// lines lie on the ground: one running into the view keeps its width, one running across it
    /// foreshortens (to about half at pitch 60), and that is checked against the camera too.
    #[test]
    fn one_and_six_tenths_pixel_lines_keep_their_width() {
        let Some(gpu) = gpu() else { return };
        for zoom in [3.0, 11.0, 15.0] {
            for pitch in [0.0, 60.0] {
                let v = view(STOCKHOLM, zoom, pitch, 0.0);
                let centre = DVec2::from_array(lng_lat_to_world(STOCKHOLM));
                // 150 px each way, at most 5° a piece on the globe
                let half = 150.0 / 2f64.powf(zoom);
                let local = frame_around(centre, half);
                // a few degrees off the axes, so the line crosses the pixels' samples evenly
                for (angle, into_view) in [(8.0, true), (82.0, false)] {
                    for px in [1.0, 0.6] {
                        let mut b = PathBuilder::new(local);
                        b.push(&line(&local, centre, angle, half, 64, 0.0), &Paint::solid(WHITE, px));
                        let (image, vp) = render(&gpu, v, [300.0, 300.0], 1.0, vec![Item::new(b, PathStyle::PIXELS)]);
                        let (expected, direction) = expected_width(&vp, angle, px);
                        let measured = measured_width(&image, direction, 24);
                        let about = format!("zoom {zoom} pitch {pitch} angle {angle} {px} px: measured {measured:.3}, expected {expected:.3}");
                        eprintln!("{about}");
                        assert!((measured / expected - 1.0).abs() < 0.15, "{about}");
                        if pitch == 0.0 || into_view {
                            assert!((expected / px - 1.0).abs() < 0.03, "{about}");
                            assert!((measured / px - 1.0).abs() < 0.15, "{about}");
                        } else {
                            assert!((expected / (px * 0.5) - 1.0).abs() < 0.1, "{about}");
                        }
                        if px == 1.0 && angle == 82.0 {
                            save(&image, &format!("e6-width-z{zoom}-p{pitch}.png"));
                        }
                    }
                }
            }
        }
    }

    /// Widths are CSS pixels: on a display of scale 2 a 1 px line covers 2 device pixels.
    #[test]
    fn widths_are_css_pixels() {
        let Some(gpu) = gpu() else { return };
        let v = view(STOCKHOLM, 11.0, 0.0, 0.0);
        let centre = DVec2::from_array(lng_lat_to_world(STOCKHOLM));
        let half = 150.0 / 2f64.powf(11.0);
        let local = frame_around(centre, half);
        let mut b = PathBuilder::new(local);
        b.push(&line(&local, centre, 8.0, half, 8, 0.0), &Paint::solid(WHITE, 1.0));
        let (image, vp) = render(&gpu, v, [200.0, 200.0], 2.0, vec![Item::new(b, PathStyle::PIXELS)]);
        assert_eq!(image.width, 400);
        let (expected, direction) = expected_width(&vp, 8.0, 1.0);
        let measured = measured_width(&image, direction, 30);
        assert!((measured / (2.0 * expected) - 1.0).abs() < 0.1, "{measured} device px for {expected} CSS px");
    }

    /// Metre widths use the view centre's scale (`width / metresPerPixel`), and the pixel clamps
    /// apply after the conversion, on the globe and on the flat map.
    #[test]
    fn metre_widths_follow_the_scale_and_the_pixel_clamps() {
        let Some(gpu) = gpu() else { return };
        for (zoom, metres) in [(11.0, 500.0), (15.0, 45.0)] {
            let v = view(ARLANDA, zoom, 0.0, 0.0);
            let centre = DVec2::from_array(lng_lat_to_world(ARLANDA));
            let half = 150.0 / 2f64.powf(zoom);
            let local = frame_around(centre, half);
            let width = |metres: f64, style: PathStyle| {
                let mut b = PathBuilder::new(local);
                b.push(&line(&local, centre, 8.0, half, 16, 0.0), &Paint::solid(WHITE, metres));
                let (image, vp) = render(&gpu, v, [300.0, 300.0], 1.0, vec![Item::new(b, style)]);
                let (_, direction) = expected_width(&vp, 8.0, 1.0);
                (measured_width(&image, direction, 24), vp.meters_per_pixel())
            };
            let (measured, mpp) = width(metres, PathStyle::DEFAULT);
            let expected = metres / mpp;
            assert!(expected > 4.0 && (measured / expected - 1.0).abs() < 0.05, "zoom {zoom}: {measured} px for {expected}");
            // a hairline held at 3 px, a wide line held at 6 px; scaled by width_scale first
            let (measured, _) = width(1.0, PathStyle { width_min_pixels: 3.0, ..PathStyle::DEFAULT });
            assert!((measured / 3.0 - 1.0).abs() < 0.05, "zoom {zoom}: min {measured}");
            let (measured, _) = width(metres, PathStyle { width_max_pixels: 6.0, width_scale: 3.0, ..PathStyle::DEFAULT });
            assert!((measured / 6.0 - 1.0).abs() < 0.05, "zoom {zoom}: max {measured}");
            let (measured, _) = width(metres, PathStyle { width_scale: 0.5, ..PathStyle::DEFAULT });
            assert!((measured / (expected / 2.0) - 1.0).abs() < 0.05, "zoom {zoom}: scaled {measured}");
        }
    }

    /// The runs of a line's pixels along the image's middle rows (coverage over one half).
    fn dash_runs(image: &Image) -> Vec<(u32, u32)> {
        let y = image.height / 2;
        let mut runs = Vec::new();
        let mut start = None;
        for x in 0..image.width {
            let on = alpha(image, x, y) > 0.5;
            assert_eq!(on, alpha(image, x, y - 1) > 0.5, "rows differ at {x}");
            match (on, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    runs.push((s, x));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            runs.push((s, image.width));
        }
        runs
    }

    /// A dashed line shows the expected number of dashes: a 2 px line (half width 1 px) of 41
    /// segments of 10 px with dashes `[4, 3]`; along the whole path the pattern runs on across the
    /// joints (dashes every 7 px), per segment it starts again at each one; justified, a whole
    /// number of periods fits, with half a dash at each end.
    #[test]
    fn dashed_lines_show_the_expected_dashes() {
        let Some(gpu) = gpu() else { return };
        let v = view(ARLANDA, 15.0, 0.0, 0.0);
        let centre = DVec2::from_array(lng_lat_to_world(ARLANDA));
        let half = 205.0 / 2f64.powf(15.0);
        let local = frame_around(centre, half);
        let path = line(&local, centre, 90.0, half, 41, 0.0);
        for (dash, justified, count) in [(Dash::Path, false, 59), (Dash::Segment, false, 42), (Dash::Path, true, 60), (Dash::Segment, true, 42)] {
            let mut b = PathBuilder::measuring(local);
            b.push(&path, &Paint::solid(WHITE, 2.0).dashed([4.0, 3.0]));
            let style = PathStyle { dash, dash_justified: justified, ..PathStyle::PIXELS };
            let (image, _) = render(&gpu, v, [500.0, 60.0], 1.0, vec![Item::new(b, style)]);
            let runs = dash_runs(&image);
            assert_eq!(runs.len(), count, "{dash:?} justified {justified}: {runs:?}");
            // the line spans x 45..455
            assert_eq!((runs[0].0, runs[runs.len() - 1].1), (45, 455), "{dash:?} justified {justified}");
            if dash == Dash::Path && !justified {
                assert!(runs.iter().enumerate().all(|(i, r)| *r == (45 + 7 * i as u32, 49 + 7 * i as u32)), "{runs:?}");
            }
            save(&image, &format!("e6-dash-{dash:?}-{justified}.png").to_lowercase());
        }
        // no extension: solid, whatever the dash array
        let mut b = PathBuilder::new(local);
        b.push(&path, &Paint::solid(WHITE, 2.0).dashed([4.0, 3.0]));
        let (image, _) = render(&gpu, v, [500.0, 60.0], 1.0, vec![Item::new(b, PathStyle::PIXELS)]);
        assert_eq!(dash_runs(&image), [(45, 455)]);
    }

    /// A V of two segments 20 px wide (half width 10 px) with its tip at the view centre, pointing
    /// north (up on the screen); `half_angle` is half the angle inside the V.
    fn v_shape(half_angle: f64, style: PathStyle) -> Image {
        let gpu = gpu().expect("checked by the caller");
        let v = view(ARLANDA, 15.0, 0.0, 0.0);
        let centre = DVec2::from_array(lng_lat_to_world(ARLANDA));
        let arm = 100.0 / 2f64.powf(15.0);
        let local = frame_around(centre, arm);
        let (s, c) = half_angle.to_radians().sin_cos();
        let at = |w: DVec2| {
            let [u, v] = local.from_world(w);
            [u, v, 0.0]
        };
        let mut b = PathBuilder::new(local);
        b.push(&[at(centre - DVec2::new(s, c) * arm), at(centre), at(centre + DVec2::new(s, -c) * arm)], &Paint::solid(WHITE, 20.0));
        render(&gpu, v, [240.0, 240.0], 1.0, vec![Item::new(b, style)]).0
    }

    /// Miter joints are cut where they reach the miter limit plus one (5) half widths from the
    /// joint; a blunter joint keeps its whole miter; round joints end one half width out.
    #[test]
    fn joints_are_mitered_up_to_the_limit_or_rounded() {
        if gpu().is_none() {
            return;
        }
        // the V's tip at (120, 120); half widths of 10 px north of it are rows 120 − 10·m; the
        // miter's wedge narrows to nothing at its tip, so near it a pixel is only partly covered
        let at = |image: &Image, m: f64| image.pixel(120, (120.0 - 10.0 * m) as u32);
        let drawn = |image: &Image, m: f64| at(image, m)[0] > 100;
        let blank = |image: &Image, m: f64| at(image, m) == [30, 30, 30, 255];
        let sharp = v_shape(10.0, PathStyle::PIXELS);
        save(&sharp, "e6-miter-cut.png");
        // the miter's tip would be 1 / sin 10° = 5.76 half widths out
        assert_eq!(at(&sharp, 4.5), WHITE);
        assert!(blank(&sharp, 5.1) && blank(&sharp, 5.5));
        let wider = v_shape(10.0, PathStyle { miter_limit: 6.0, ..PathStyle::PIXELS });
        assert!(drawn(&wider, 5.1) && drawn(&wider, 5.5) && blank(&wider, 6.0));
        let blunt = v_shape(30.0, PathStyle::PIXELS);
        // tip at 2 half widths
        assert!(drawn(&blunt, 1.8) && blank(&blunt, 2.3));
        let round = v_shape(30.0, PathStyle { joint_rounded: true, ..PathStyle::PIXELS });
        save(&round, "e6-round-joint.png");
        assert_eq!(at(&round, 0.8), WHITE);
        assert!(blank(&round, 1.3));
    }

    /// Butt caps end at the last point; round caps reach one half width beyond it.
    #[test]
    fn caps_are_butt_or_round() {
        let Some(gpu) = gpu() else { return };
        let v = view(ARLANDA, 15.0, 0.0, 0.0);
        let centre = DVec2::from_array(lng_lat_to_world(ARLANDA));
        let arm = 100.0 / 2f64.powf(15.0);
        let local = frame_around(centre, arm);
        let ends_at_centre = || {
            let [u0, v0] = local.from_world(centre - DVec2::new(arm, 0.0));
            let [u1, v1] = local.from_world(centre);
            let mut b = PathBuilder::new(local);
            b.push(&[[u0, v0, 0.0], [u1, v1, 0.0]], &Paint::solid(WHITE, 20.0));
            b
        };
        let (butt, _) = render(&gpu, v, [240.0, 240.0], 1.0, vec![Item::new(ends_at_centre(), PathStyle::PIXELS)]);
        let (round, _) = render(&gpu, v, [240.0, 240.0], 1.0, vec![Item::new(ends_at_centre(), PathStyle { cap_rounded: true, ..PathStyle::PIXELS })]);
        save(&round, "e6-round-cap.png");
        // the line ends at x = 120; row 120 is its middle
        assert_eq!(butt.pixel(118, 120), WHITE);
        assert_eq!(butt.pixel(121, 120), [30, 30, 30, 255]);
        assert_eq!(round.pixel(127, 120), WHITE);
        assert_eq!(round.pixel(131, 120), [30, 30, 30, 255]);
        // off the axis the cap is round: the pixel 7.5 px out and 7.5 px up is 10.6 px from the end
        assert_eq!(round.pixel(127, 112), [30, 30, 30, 255]);
        assert_eq!(round.pixel(125, 114), WHITE);
    }

    /// A 3D path: a line high up is drawn where the camera puts that height, pitched, on the globe
    /// and on the flat map.
    #[test]
    fn paths_keep_their_heights() {
        let Some(gpu) = gpu() else { return };
        for (zoom, h) in [(11.0, 1000.0), (13.5, 200.0)] {
            let v = view(ARLANDA, zoom, 45.0, 0.0);
            let centre = DVec2::from_array(lng_lat_to_world(ARLANDA));
            let half = 150.0 / 2f64.powf(zoom);
            let local = frame_around(centre, half);
            let mut b = PathBuilder::new(local);
            b.push(&line(&local, centre, 90.0, half, 16, h), &Paint::solid(WHITE, 2.0));
            let (image, vp) = render(&gpu, v, [300.0, 300.0], 1.0, vec![Item::new(b, PathStyle::PIXELS)]);
            let [lon, lat] = world_to_lng_lat(centre.to_array());
            let [x, y, _] = vp.project([lon, lat, h]);
            // the middle of the line's coverage down column x
            let column = x.floor() as u32;
            let (sum, weighted) = (0..image.height).fold((0.0, 0.0), |(s, w), row| {
                let a = alpha(&image, column, row);
                (s + a, w + a * (f64::from(row) + 0.5))
            });
            assert!(sum > 0.5, "zoom {zoom}: no line in column {column}");
            assert!((weighted / sum - y).abs() < 0.5, "zoom {zoom}: line at row {} not {y}", weighted / sum);
            assert!(y < 150.0 - 10.0, "zoom {zoom}: {h} m should rise well above the centre, at {y}");
        }
    }

    /// Opacity and per-vertex colours: each segment takes its start vertex's colour, alpha times
    /// the layer's opacity.
    #[test]
    fn colours_come_per_segment_with_the_layer_opacity() {
        let Some(gpu) = gpu() else { return };
        let v = view(ARLANDA, 15.0, 0.0, 0.0);
        let centre = DVec2::from_array(lng_lat_to_world(ARLANDA));
        let half = 100.0 / 2f64.powf(15.0);
        let local = frame_around(centre, half);
        let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
        let mut b = PathBuilder::new(local);
        let paint = Paint { color: Colors::PerVertex(&colors), ..Paint::solid(WHITE, 10.0) };
        b.push(&line(&local, centre, 90.0, half, 2, 0.0), &paint);
        let (image, _) = render(&gpu, v, [240.0, 60.0], 1.0, vec![Item::new(b, PathStyle { opacity: 0.5, ..PathStyle::PIXELS })]);
        // x 20..120 red, 120..220 green, over the ink at half opacity: 30 + 225 / 2 and 30 / 2
        let near = |got: [u8; 4], want: [u8; 4]| got.iter().zip(want).all(|(g, w)| (i32::from(*g) - i32::from(w)).abs() <= 1);
        assert!(near(image.pixel(60, 30), [142, 15, 15, 255]), "{:?}", image.pixel(60, 30));
        assert!(near(image.pixel(180, 30), [15, 142, 15, 255]), "{:?}", image.pixel(180, 30));
    }

    /// A scene to look at: at Arlanda (flat, pitched, turned) hairlines, a runway 45 m wide, a
    /// route dashed along its whole length, a justified tunnel dash with round caps, a mitered and
    /// a rounded zigzag and a closed loop; and on the globe a long dashed route cut every 5°.
    #[test]
    fn a_scene_of_paths() {
        let Some(gpu) = gpu() else { return };
        let v = view(ARLANDA, 13.5, 45.0, 30.0);
        let centre = DVec2::from_array(lng_lat_to_world(ARLANDA));
        let half = 400.0 / 2f64.powf(13.5);
        let local = frame_around(centre, half);
        let shifted = |dx: f64, dy: f64| centre + DVec2::new(dx, dy) * half;
        let mut items = Vec::new();
        let mut hair = PathBuilder::new(local);
        hair.push(&line(&local, shifted(-0.6, 0.0), 0.0, half * 0.8, 8, 0.0), &Paint::solid([118, 118, 118, 255], 1.0));
        hair.push(&line(&local, shifted(-0.5, 0.0), 0.0, half * 0.8, 8, 0.0), &Paint::solid([84, 84, 84, 255], 0.6));
        hair.push(&line(&local, shifted(0.0, -0.6), 90.0, half * 0.8, 8, 0.0), &Paint::solid([118, 118, 118, 255], 1.0));
        items.push(Item::new(hair, PathStyle::PIXELS));
        let mut runway = PathBuilder::new(local);
        runway.push(&line(&local, shifted(-0.2, 0.1), 10.0, half * 0.5, 1, 0.0), &Paint::solid([163, 119, 52, 255], 45.0));
        items.push(Item::new(runway, PathStyle::DEFAULT));
        let mut route = PathBuilder::measuring(local);
        route.push(&line(&local, shifted(0.2, 0.0), 30.0, half * 0.7, 40, 300.0), &Paint::solid([55, 142, 240, 210], 2.0).dashed([4.0, 3.0]));
        items.push(Item::new(route, PathStyle { dash: Dash::Path, ..PathStyle::PIXELS }));
        let mut tunnel = PathBuilder::new(local);
        tunnel.push(&line(&local, shifted(0.4, 0.2), 120.0, half * 0.4, 3, 0.0), &Paint::solid([241, 238, 231, 200], 4.0).dashed([2.5, 2.0]));
        items.push(Item::new(tunnel, PathStyle { dash: Dash::Segment, dash_justified: true, cap_rounded: true, ..PathStyle::PIXELS }));
        let zigzag = |dy: f64| {
            (0..6)
                .map(|i| {
                    let [u, v] = local.from_world(shifted(-0.3 + 0.12 * i as f64, dy + if i % 2 == 0 { 0.0 } else { 0.08 }));
                    [u, v, 0.0]
                })
                .collect::<Vec<_>>()
        };
        let mut miter = PathBuilder::new(local);
        miter.push(&zigzag(0.35), &Paint::solid([241, 238, 231, 245], 10.0));
        items.push(Item::new(miter, PathStyle::PIXELS));
        let mut round = PathBuilder::new(local);
        round.push(&zigzag(0.55), &Paint::solid([241, 238, 231, 245], 10.0));
        items.push(Item::new(round, PathStyle { joint_rounded: true, cap_rounded: true, ..PathStyle::PIXELS }));
        let mut loop_ = PathBuilder::new(local);
        let square: Vec<[f64; 3]> = [(0.45, -0.3), (0.6, -0.3), (0.6, -0.15), (0.45, -0.15), (0.45, -0.3)]
            .iter()
            .map(|&(x, y)| {
                let [u, v] = local.from_world(shifted(x, y));
                [u, v, 0.0]
            })
            .collect();
        loop_.push(&square, &Paint::solid([105, 80, 46, 255], 5.0));
        items.push(Item::new(loop_, PathStyle::PIXELS));
        let (image, _) = render(&gpu, v, [720.0, 450.0], 1.0, items);
        save(&image, "e6-scene-flat.png");
        assert!(image.rgba.chunks_exact(4).any(|p| p == [163, 119, 52, 255]), "no runway");

        // the globe: Stockholm to New York as a dashed route, cut every 5° of longitude or latitude
        let v = view([-20.0, 52.0], 2.5, 30.0, 0.0);
        let points: Vec<[f64; 2]> = (0..=60).map(|i| {
            let t = i as f64 / 60.0;
            [18.07 + (-74.0 - 18.07) * t, 59.33 + (40.7 - 59.33) * t + 12.0 * (std::f64::consts::PI * t).sin()]
        }).collect();
        let world: Vec<DVec2> = points.iter().map(|&p| DVec2::from_array(lng_lat_to_world(p))).collect();
        let (min, max) = world.iter().fold((world[0], world[0]), |(a, b), &w| (a.min(w), b.max(w)));
        let local = frame_around((min + max) / 2.0, (max - min).max_element() / 2.0);
        let path: Vec<[f64; 3]> = world
            .iter()
            .map(|&w| {
                let [u, v] = local.from_world(w);
                [u, v, 10_000.0]
            })
            .collect();
        let mut route = PathBuilder::measuring(local);
        route.push(&path, &Paint::solid([55, 142, 240, 210], 2.0).dashed([4.0, 3.0]));
        let mut solid = PathBuilder::new(local);
        solid.push(&path, &Paint::solid([241, 238, 231, 110], 1.5));
        let items = vec![Item::new(solid, PathStyle::PIXELS), Item::new(route, PathStyle { dash: Dash::Path, ..PathStyle::PIXELS })];
        let (image, _) = render(&gpu, v, [720.0, 450.0], 1.0, items);
        save(&image, "e6-scene-globe.png");
        assert!(image.rgba.chunks_exact(4).any(|p| p[2] > 150), "no route");
    }

    /// A dynamic mesh is rewritten in place, its buffer grown when the segments do not fit.
    #[test]
    fn meshes_are_rewritten_and_grow() {
        let Some(gpu) = gpu() else { return };
        let mut mesh = Mesh::new(&gpu.device, "test", &[]);
        assert!(mesh.is_empty());
        let mut b = PathBuilder::new(LocalMercator::new(DVec2::ZERO, 1.0));
        b.push(&[[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [0.5, 0.5, 0.0], [1.0, 0.5, 0.0]], &Paint::solid(WHITE, 1.0));
        let segments = b.finish();
        mesh.write(&gpu.device, &gpu.queue, "test", &segments);
        assert_eq!(mesh.len(), 3);
        assert!(mesh.buffer.size() >= (3 * size_of::<Segment>()) as u64);
        mesh.write(&gpu.device, &gpu.queue, "test", &segments[..1]);
        assert_eq!(mesh.len(), 1);
        mesh.write(&gpu.device, &gpu.queue, "test", &[]);
        assert!(mesh.is_empty());
    }

    #[test]
    fn layouts_match_the_wgsl() {
        let module = crate::shaders::tests::validate("path.wgsl", &crate::shaders::expand(SHADER).expect("libraries known"));
        let (span, members) = module
            .types
            .iter()
            .find_map(|(_, t)| match &t.inner {
                wgpu::naga::TypeInner::Struct { members, span } if t.name.as_deref() == Some("PathStyle") => {
                    Some((*span, members.iter().map(|m| (m.name.clone().expect("named"), m.offset)).collect::<Vec<_>>()))
                }
                _ => None,
            })
            .expect("struct PathStyle");
        assert_eq!(span as usize, size_of::<StyleUniform>());
        let expected = [
            ("width_scale", std::mem::offset_of!(StyleUniform, width_scale)),
            ("width_min_pixels", std::mem::offset_of!(StyleUniform, width_min_pixels)),
            ("width_max_pixels", std::mem::offset_of!(StyleUniform, width_max_pixels)),
            ("joint_type", std::mem::offset_of!(StyleUniform, joint_type)),
            ("cap_type", std::mem::offset_of!(StyleUniform, cap_type)),
            ("miter_limit", std::mem::offset_of!(StyleUniform, miter_limit)),
            ("width_units", std::mem::offset_of!(StyleUniform, width_units)),
            ("opacity", std::mem::offset_of!(StyleUniform, opacity)),
            ("dash_mode", std::mem::offset_of!(StyleUniform, dash_mode)),
            ("dash_align_mode", std::mem::offset_of!(StyleUniform, dash_align_mode)),
            ("pad", std::mem::offset_of!(StyleUniform, pad)),
        ];
        assert_eq!(members, expected.map(|(n, o)| (n.to_string(), o as u32)));
        // the instance attributes sit where `PathSegment` keeps its fields
        let offsets: Vec<u64> = SEGMENT_LAYOUT.attributes.iter().map(|a| a.offset).collect();
        let fields = [
            std::mem::offset_of!(Segment, left),
            std::mem::offset_of!(Segment, start),
            std::mem::offset_of!(Segment, end),
            std::mem::offset_of!(Segment, right),
            std::mem::offset_of!(Segment, width),
            std::mem::offset_of!(Segment, color),
            std::mem::offset_of!(Segment, dash_array),
            std::mem::offset_of!(Segment, dash_globe),
            std::mem::offset_of!(Segment, dash_flat),
            std::mem::offset_of!(Segment, types),
        ];
        assert_eq!(offsets, fields.map(|f| f as u64));
        assert_eq!(SEGMENT_LAYOUT.array_stride as usize, size_of::<Segment>());
        // deck's UNIT numbers and defaults
        assert_eq!((Units::Common as u32, Units::Meters as u32, Units::Pixels as u32), (0, 1, 2));
        let u = PathStyle::DEFAULT.uniform();
        assert_eq!((u.width_units, u.miter_limit, u.width_max_pixels, u.joint_type, u.cap_type), (1, 4.0, 9.007_199e15, 0.0, 0.0));
        assert_eq!((CORNERS.len(), CORNER_INDICES.len()), (6, 12));
    }
}
