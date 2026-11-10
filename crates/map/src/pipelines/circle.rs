//! The `circle` pipeline (plan 1.4.6, item E9): deck.gl 9.4's ScatterplotLayer, for the selection
//! halos and rings, airport and place dots, the route's waypoints, signals and train stops. Its
//! shader is `shaders/circle.wgsl`.
//!
//! - [`Instance`]: one circle, its centre as a common offset placed in f64 on the CPU at each scene
//!   refresh ([`Instance::at`], [`crate::frame::ViewFrame::common_offset`]) with its up direction,
//!   and deck's per-instance `getRadius`, `getLineWidth`, `getFillColor` and `getLineColor`.
//! - [`CircleStyle`]: deck's layer props (`radiusUnits`, `radiusScale`, `radiusMinPixels`,
//!   `radiusMaxPixels`, the same four for the line width, `stroked`, `filled`, `billboard`,
//!   `antialiasing`) and the layer's `opacity`. [`Style`] is it on the GPU (group 2).
//! - [`Instances`]: an instance buffer, rewritten at each scene refresh. [`Circle`]: the pipeline for
//!   one draw-state preset; its `draw` binds no frame (circles use common offsets).
//!
//! What deck does and this does too: radius and line width turn into CSS pixels at the view
//! centre's scale (metres through deck's `commonUnitsPerMeter` at the view centre, in both
//! projections, as deck's globe and its flat auto-offset mode do), clamped to the min and max
//! pixels; a stroked circle's outer radius is the radius plus half the line width; edges are smoothed
//! over half a CSS pixel each way (`smoothedge`); a filled and stroked circle mixes fill and line by
//! the line's coverage, alpha included. A billboard circle is offset in clip space by deck's
//! `project_pixel_size_to_clipspace`, scaled by the clip w of the view centre (its
//! `focalDistance`): it is round on the screen and exactly its size at the view centre's depth, and
//! larger or smaller by `focal_w / w` nearer the camera or further from it in a pitched view, as in
//! deck. A ground-plane circle (`billboard: false`) lies in the ground's plane at its centre, its
//! size in common units at the view centre's scale, and foreshortens with the ground. Not ported:
//! `getPixelOffset` (the app never sets it on a scatterplot), picking colours and the highlight (no
//! scatterplot of the app is highlighted).
//!
//! The app's circle layers (`Globe.svelte`), for the items that draw them: every radius and line
//! width is in pixels ([`CircleStyle::PIXELS`]), none sets a scale, clamp or opacity.
//!
//! | Layer | Line | Billboard | Filled / stroked | Radius, colours | Draw state |
//! |---|---|---|---|---|---|
//! | `route-waypoints` | G:1432 | no | filled | 2.5, fill `[241,238,231,150]` | `SURFACE_NO_CULL` |
//! | `{id}-dots` (route and focused airports) | G:1486 | yes | both | 5, fill `[55,142,240,255]`, line `[6,12,19,230]` 2 | [`MARKER`] |
//! | `place-dot` | G:1524 | yes | both | 4, fill `[241,238,231,255]`, line `[6,12,19,230]` 2 | [`MARKER`] |
//! | `train-route-stop-spot` | G:2135 | no | both | 5, fill white, line `TRAIN_TO_STOP` 2.5 | `LABEL` |
//! | `train-route-stops` | G:2148 | no | both | 4.5, fill `TRAIN_ROUTE_CASING`, line `TRAIN_ROUTE_DASH` 2 | `LABEL` |
//! | `train-signals` | G:2337 | yes | both | 6, fill by aspect, line `BLOCK_CASING` 2 | `LABEL` |
//! | `dev-predicted`, `dev-sightings` | G:2455, G:2486 | no | stroked; both | 7; 4, line 2 | `LABEL` |
//! | `selection-ring` (aircraft) | G:3040 | yes | stroked | `screenPx(size) · 0.6 + 4`, line `[55,142,240,230]` 1.5 | [`MARKER`] |
//! | `ship-`/`train-selection-halo` | G:3310, G:3343 | yes | filled | the vehicle's, fill `[55,142,240,56]` | `LABEL` |
//! | `ship-`/`train-selection-ring` | G:3311, G:3344 | yes | stroked | the vehicle's, line `[55,142,240,235]` 2 | `LABEL` |
//!
//! `LABEL` here is `{cullMode: "none", depthWriteEnabled: false, depthCompare: "always"}`, which is
//! what [`DrawState::LABEL`] holds. deck's default colours are black, its default radius and line
//! width 1 ([`Instance::at`] starts from them).

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Vec3};

use super::path::{MAX_SAFE_INTEGER, Units};
use super::{Desc, build};
use crate::frame::ViewFrame;
use crate::layers::Init;
use crate::shaders;
use crate::state::DrawState;

/// `shaders/circle.wgsl`.
pub const SHADER: &str = include_str!("../shaders/circle.wgsl");

/// The bind group of the [`Style`].
pub const STYLE_GROUP: u32 = 2;

/// `{...NO_CULL, depthCompare: "always"}` (G:1497, G:1535, G:3051): the airport and place dots and
/// the aircraft's selection ring draw through everything, the globe included, and still write depth.
pub const MARKER: DrawState = DrawState { depth_compare: wgpu::CompareFunction::Always, ..DrawState::NO_CULL };

/// deck's default `getFillColor` and `getLineColor`.
pub const DEFAULT_COLOR: [u8; 4] = [0, 0, 0, 255];

/// The vertices of deck's quad, a triangle strip of the unit square's corners (made in the shader
/// from the vertex index).
pub const QUAD_VERTICES: u32 = 4;

/// One circle.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Instance {
    /// The common offset of its centre ([`ViewFrame::common_offset`]).
    pub position: [f32; 3],
    /// The up direction at its centre in common space: on the globe the normal of the sphere there,
    /// on the flat map `(0, 0, 1)`. A ground-plane circle lies across it.
    pub up: [f32; 3],
    /// `getRadius`, in the style's `radius_units`.
    pub radius: f32,
    /// `getLineWidth`, in the style's `line_width_units`.
    pub line_width: f32,
    /// `getFillColor`, RGBA bytes.
    pub fill_color: [u8; 4],
    /// `getLineColor`, RGBA bytes.
    pub line_color: [u8; 4],
}

impl Instance {
    /// The instance buffer layout: position, up, radius, line width, fill and line colours at
    /// locations 0 to 5.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Instance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Float32, 3 => Float32, 4 => Unorm8x4, 5 => Unorm8x4
        ],
    };

    /// A circle centred on `[lon, lat]` at `h` metres in `view`, with deck's defaults: radius 1,
    /// line width 1, fill and line black.
    pub fn at(view: &ViewFrame, [lon, lat]: [f64; 2], h: f64) -> Instance {
        let up = if view.is_globe() { view.viewport.project_position([lon, lat, h]).normalize() } else { DVec3::Z };
        Instance {
            position: view.common_offset([lon, lat], h).into(),
            up: up.as_vec3().into(),
            radius: 1.0,
            line_width: 1.0,
            fill_color: DEFAULT_COLOR,
            line_color: DEFAULT_COLOR,
        }
    }

    /// With `getRadius` `radius`.
    pub fn radius(self, radius: f64) -> Instance {
        Instance { radius: radius as f32, ..self }
    }

    /// With `getFillColor` `color`.
    pub fn fill(self, color: [u8; 4]) -> Instance {
        Instance { fill_color: color, ..self }
    }

    /// With `getLineColor` `color` and `getLineWidth` `width`.
    pub fn line(self, color: [u8; 4], width: f64) -> Instance {
        Instance { line_color: color, line_width: width as f32, ..self }
    }

    /// Its centre's common offset.
    pub fn offset(&self) -> Vec3 {
        Vec3::from_array(self.position)
    }
}

/// A scatterplot layer's props.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircleStyle {
    pub radius_units: Units,
    pub radius_scale: f64,
    pub radius_min_pixels: f64,
    pub radius_max_pixels: f64,
    pub line_width_units: Units,
    pub line_width_scale: f64,
    pub line_width_min_pixels: f64,
    pub line_width_max_pixels: f64,
    pub stroked: bool,
    pub filled: bool,
    /// Facing the camera (`true`) or lying on the ground.
    pub billboard: bool,
    /// deck's `antialiasing`: edges smoothed over half a pixel each way, or cut at the pixel centre.
    pub antialiasing: bool,
    /// deck's layer `opacity`, multiplying the fill's and the line's alpha.
    pub opacity: f64,
}

impl CircleStyle {
    /// deck's defaults: radius and line width in metres, scale 1, no clamp, filled, not stroked, on
    /// the ground, antialiased, opaque.
    pub const DEFAULT: CircleStyle = CircleStyle {
        radius_units: Units::Meters,
        radius_scale: 1.0,
        radius_min_pixels: 0.0,
        radius_max_pixels: MAX_SAFE_INTEGER,
        line_width_units: Units::Meters,
        line_width_scale: 1.0,
        line_width_min_pixels: 0.0,
        line_width_max_pixels: MAX_SAFE_INTEGER,
        stroked: false,
        filled: true,
        billboard: false,
        antialiasing: true,
        opacity: 1.0,
    };

    /// The defaults with radius and line width in pixels (`radiusUnits: "pixels"`,
    /// `lineWidthUnits: "pixels"`: every circle of the app).
    pub const PIXELS: CircleStyle =
        CircleStyle { radius_units: Units::Pixels, line_width_units: Units::Pixels, ..Self::DEFAULT };

    /// The uniform the shader reads.
    pub fn uniform(&self) -> StyleUniform {
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        StyleUniform {
            radius_scale: self.radius_scale as f32,
            radius_min_pixels: self.radius_min_pixels as f32,
            radius_max_pixels: self.radius_max_pixels as f32,
            line_width_scale: self.line_width_scale as f32,
            line_width_min_pixels: self.line_width_min_pixels as f32,
            line_width_max_pixels: self.line_width_max_pixels as f32,
            stroked: flag(self.stroked),
            filled: flag(self.filled),
            antialiasing: u32::from(self.antialiasing),
            billboard: u32::from(self.billboard),
            radius_units: self.radius_units as u32,
            line_width_units: self.line_width_units as u32,
            opacity: self.opacity as f32,
            pad: [0.0; 3],
        }
    }
}

impl Default for CircleStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `circle.wgsl`'s `CircleStyle`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StyleUniform {
    pub radius_scale: f32,
    pub radius_min_pixels: f32,
    pub radius_max_pixels: f32,
    pub line_width_scale: f32,
    pub line_width_min_pixels: f32,
    pub line_width_max_pixels: f32,
    /// 0 or 1.
    pub stroked: f32,
    /// 0 or 1.
    pub filled: f32,
    pub antialiasing: u32,
    pub billboard: u32,
    pub radius_units: u32,
    pub line_width_units: u32,
    pub opacity: f32,
    pub pad: [f32; 3],
}

/// A [`CircleStyle`] on the GPU (group 2), one per draw that differs.
pub struct Style {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    style: CircleStyle,
}

impl Style {
    /// The style it holds.
    pub fn get(&self) -> &CircleStyle {
        &self.style
    }

    /// Changes it (written only when it differs).
    pub fn set(&mut self, queue: &wgpu::Queue, style: &CircleStyle) {
        if *style != self.style {
            self.style = *style;
            queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&style.uniform()));
        }
    }
}

/// Circles on the GPU, rewritten at each scene refresh (the buffer grows by doubling).
pub struct Instances {
    buffer: wgpu::Buffer,
    capacity: usize,
    count: u32,
    label: String,
}

impl Instances {
    const INITIAL_CAPACITY: usize = 4;

    /// An empty buffer.
    pub fn new(device: &wgpu::Device, label: &str) -> Self {
        Instances {
            buffer: Self::allocate(device, label, Self::INITIAL_CAPACITY),
            capacity: Self::INITIAL_CAPACITY,
            count: 0,
            label: label.to_string(),
        }
    }

    fn allocate(device: &wgpu::Device, label: &str, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (capacity * size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Replaces the circles.
    pub fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance]) {
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.buffer = Self::allocate(device, &self.label, self.capacity);
        }
        if !instances.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(instances));
        }
        self.count = instances.len() as u32;
    }

    /// How many there are.
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// The circle pipeline for one draw-state preset ([`MARKER`], `LABEL`, `SURFACE_NO_CULL`; see the
/// table above).
pub struct Circle {
    pipeline: wgpu::RenderPipeline,
    style_layout: wgpu::BindGroupLayout,
}

impl Circle {
    /// The pipeline with `state` for the map pass of `init`.
    pub fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        let device = &init.gpu.device;
        let style_layout = super::fill::uniform_layout(device, "circle style", size_of::<StyleUniform>());
        let module = shaders::module(device, label, SHADER);
        let pipeline = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(Instance::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                state,
                groups: &[&style_layout],
            },
        );
        Circle { pipeline, style_layout }
    }

    /// A [`Style`] for this pipeline.
    pub fn style(&self, device: &wgpu::Device, style: &CircleStyle) -> Style {
        let (buffer, group) = super::fill::uniform_group(device, "circle style", &self.style_layout, &style.uniform());
        Style { buffer, group, style: *style }
    }

    /// Draws `instances` with `style` (the globals bound as group 0; group 1 is not read).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, style: &Style, instances: &Instances) {
        if instances.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(STYLE_GROUP, &style.group, &[]);
        pass.set_vertex_buffer(0, instances.buffer.slice(..));
        pass.draw(0..QUAD_VERTICES, 0..instances.count);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path as FilePath;

    use glam::{DMat3, DVec2, DVec4};
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{DrawCx, Layer, PrepareCx};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::renderer::{MapRenderer, SceneInput};

    const STOCKHOLM: [f64; 2] = [18.07, 59.33];
    const ARLANDA: [f64; 2] = [17.9186, 59.6519];
    const BLUE: [u8; 3] = [55, 142, 240];
    const HALO: [u8; 4] = [55, 142, 240, 56];
    const RING: [u8; 4] = [55, 142, 240, 235];

    fn view([longitude, latitude]: [f64; 2], zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, pitch, bearing, position: None }
    }

    /// One circle to draw: where, and its per-instance values.
    #[derive(Debug, Clone, Copy)]
    struct Spot {
        at: [f64; 3],
        radius: f64,
        line_width: f64,
        fill: [u8; 4],
        line: [u8; 4],
    }

    impl Spot {
        fn new(at: [f64; 2], radius: f64) -> Self {
            Spot { at: [at[0], at[1], 0.0], radius, line_width: 1.0, fill: DEFAULT_COLOR, line: DEFAULT_COLOR }
        }

        fn fill(self, fill: [u8; 4]) -> Self {
            Spot { fill, ..self }
        }

        fn line(self, line: [u8; 4], line_width: f64) -> Self {
            Spot { line, line_width, ..self }
        }

        fn instance(&self, view: &ViewFrame) -> Instance {
            Instance::at(view, [self.at[0], self.at[1]], self.at[2]).radius(self.radius).fill(self.fill).line(self.line, self.line_width)
        }
    }

    /// One draw: a style, a draw state and its circles.
    #[derive(Debug, Clone)]
    struct Item {
        style: CircleStyle,
        state: DrawState,
        spots: Vec<Spot>,
    }

    /// A test layer: the items in order, one pipeline each, their instances placed in `prepare`.
    struct Circles {
        items: Vec<(Circle, Style, Instances, Vec<Spot>)>,
    }

    impl Layer for Circles {
        fn name(&self) -> &'static str {
            "test circles"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            for (_, _, instances, spots) in &mut self.items {
                let list: Vec<Instance> = spots.iter().map(|s| s.instance(cx.view)).collect();
                instances.write(&cx.gpu.device, &cx.gpu.queue, &list);
            }
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
            for (circle, style, instances, _) in &self.items {
                circle.draw(pass, style, instances);
            }
        }
    }

    /// The scene without and with `items` (the earth shown or not), as a window of `scale_factor`
    /// shows it, the frame's view and the map pass's sample count.
    fn render(gpu: &crate::gpu::Gpu, view: ViewState, size: [f64; 2], scale_factor: f64, earth: bool, items: &[Item]) -> (Image, Image, ViewFrame, u32) {
        let mut scene = SceneInput::new(view, size, scale_factor, Basemap::Dark);
        if earth {
            scene.earth = crate::layers::earth::Input::SHOWN;
        }
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let background = render_with(gpu, &mut renderer, &scene, scale_factor).expect("rendered");
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let init = renderer.init(gpu);
        let items = items
            .iter()
            .map(|i| {
                let circle = Circle::new(&init, "test circle", i.state);
                let style = circle.style(&gpu.device, &i.style);
                (circle, style, Instances::new(&gpu.device, "test circles"), i.spots.clone())
            })
            .collect();
        renderer.add_layer(Box::new(Circles { items }));
        let image = render_with(gpu, &mut renderer, &scene, scale_factor).expect("rendered");
        (background, image, renderer.view().clone(), renderer.samples())
    }

    fn save(image: &Image, name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots");
        std::fs::create_dir_all(dir).expect("shot folder");
        image.save_png(&FilePath::new(dir).join(name)).expect("png");
    }

    // -----------------------------------------------------------------------------------------
    // deck's ScatterplotLayer on the CPU, in f64: where a pixel falls in a circle and what colour
    // the fragment shader gives it there
    // -----------------------------------------------------------------------------------------

    fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    fn smoothedge(edge: f64, x: f64) -> f64 {
        smoothstep(edge - 0.5, edge + 0.5, x)
    }

    fn step(edge: f64, x: f64) -> f64 {
        if x < edge { 0.0 } else { 1.0 }
    }

    /// deck's `project_size_to_pixel` at the view centre.
    fn size_to_pixels(vf: &ViewFrame, size: f64, units: Units) -> f64 {
        match units {
            Units::Meters => size * vf.viewport.distance_scales.units_per_meter[2] * vf.viewport.scale,
            Units::Common => size * vf.viewport.scale,
            Units::Pixels => size,
        }
    }

    /// The vertex shader's per-circle terms: the outer radius and the line width in pixels.
    fn radii(vf: &ViewFrame, style: &CircleStyle, spot: &Spot) -> (f64, f64) {
        let radius = size_to_pixels(vf, style.radius_scale * spot.radius, style.radius_units).clamp(style.radius_min_pixels, style.radius_max_pixels);
        let line = size_to_pixels(vf, style.line_width_scale * spot.line_width, style.line_width_units)
            .clamp(style.line_width_min_pixels, style.line_width_max_pixels);
        let stroked = if style.stroked { 1.0 } else { 0.0 };
        (radius + stroked * line / 2.0, line)
    }

    /// deck's fragment colour (RGBA, 0–1) at `dist` pixels from the centre, `None` where it discards.
    fn fragment(style: &CircleStyle, spot: &Spot, outer: f64, line: f64, dist: f64) -> Option<[f64; 4]> {
        let stroked = if style.stroked { 1.0 } else { 0.0 };
        let inner = 1.0 - stroked * line / outer;
        let color = |c: [u8; 4]| [c[0], c[1], c[2], c[3]].map(|v| f64::from(v) / 255.0);
        let fill = color(spot.fill);
        let line_color = color(spot.line);
        let (fill, line_color) = ([fill[0], fill[1], fill[2], fill[3] * style.opacity], [line_color[0], line_color[1], line_color[2], line_color[3] * style.opacity]);
        let in_circle = if style.antialiasing { smoothedge(dist, outer) } else { step(dist, outer) };
        if in_circle == 0.0 {
            return None;
        }
        let mut out = if style.stroked {
            let is_line = if style.antialiasing { smoothedge(inner * outer, dist) } else { step(inner * outer, dist) };
            if style.filled {
                std::array::from_fn(|i| fill[i] + (line_color[i] - fill[i]) * is_line)
            } else {
                if is_line == 0.0 {
                    return None;
                }
                [line_color[0], line_color[1], line_color[2], line_color[3] * is_line]
            }
        } else if !style.filled {
            return None;
        } else {
            fill
        };
        out[3] *= in_circle;
        Some(out)
    }

    /// deck's `focalDistance`: the clip w of the point `focal_distance` ahead of the camera.
    fn focal_w(vf: &ViewFrame) -> f64 {
        let w = (vf.viewport.projection_matrix * DVec4::new(0.0, 0.0, -vf.viewport.focal_distance, 1.0)).w;
        if w == 0.0 { 1.0 } else { w }
    }

    /// deck's `project_get_orientation_matrix(up)`.
    fn orientation(up: DVec3) -> DMat3 {
        let uz = up.normalize();
        let ux = if uz.z.abs() == 1.0 { DVec3::X } else { DVec3::new(uz.y, -uz.x, 0.0).normalize() };
        DMat3::from_cols(ux, uz.cross(ux), uz)
    }

    /// Where the point at CSS `pixel` lies in the circle's quad, in deck's pixels from its centre
    /// along the quad's axes: on the screen for a billboard (scaled by w / focal_w), in the ground's
    /// plane for the others (the point's ray met with the plane, in common units at the view
    /// centre's scale, along deck's orientation axes on the globe).
    fn quad_position(vf: &ViewFrame, style: &CircleStyle, spot: &Spot, pixel: DVec2) -> DVec2 {
        let vp = &vf.viewport;
        let centre = vp.project_position(spot.at);
        if style.billboard {
            let [x, y, _] = vp.world_to_pixels(centre);
            let w = (vp.view_projection_matrix * centre.extend(1.0)).w;
            (pixel - DVec2::new(x, y)) * w / focal_w(vf)
        } else {
            let frame = if vf.is_globe() { orientation(centre) } else { DMat3::IDENTITY };
            let up = frame.z_axis;
            let near = vp.pixels_to_world_with_depth([pixel.x, pixel.y, -1.0]);
            let far = vp.pixels_to_world_with_depth([pixel.x, pixel.y, 1.0]);
            let t = (centre - near).dot(up) / (far - near).dot(up);
            let d = near + (far - near) * t - centre;
            DVec2::new(d.dot(frame.x_axis), d.dot(frame.y_axis)) * vp.scale
        }
    }

    /// The standard positions of 4 samples in a pixel (Vulkan, D3D, Metal), or its centre alone.
    fn sample_positions(samples: u32) -> Vec<DVec2> {
        match samples {
            4 => vec![DVec2::new(0.375, 0.125), DVec2::new(0.875, 0.375), DVec2::new(0.125, 0.625), DVec2::new(0.625, 0.875)],
            1 => vec![DVec2::splat(0.5)],
            n => panic!("no sample pattern for {n} samples"),
        }
    }

    /// How far, in device pixels, a rasteriser may put a vertex from where it is sent: Vulkan
    /// requires only 4 bits of sub-pixel precision (SwiftShader has that many), so a vertex snaps
    /// by up to 1/32 px along each axis, 0.044 px diagonally. Every point of the quad's
    /// interpolation moves by at most as much.
    const SNAP_PX: f64 = 0.045;

    /// The largest stretch of the map from CSS pixels to the quad's deck pixels at `pixel`.
    fn stretch(vf: &ViewFrame, style: &CircleStyle, spot: &Spot, pixel: DVec2) -> f64 {
        let h = 1e-3;
        let dx = (quad_position(vf, style, spot, pixel + DVec2::new(h, 0.0)) - quad_position(vf, style, spot, pixel - DVec2::new(h, 0.0))) / (2.0 * h);
        let dy = (quad_position(vf, style, spot, pixel + DVec2::new(0.0, h)) - quad_position(vf, style, spot, pixel - DVec2::new(0.0, h))) / (2.0 * h);
        let t = dx.length_squared() + dy.length_squared();
        let det = dx.perp_dot(dy);
        ((t + (t * t - 4.0 * det * det).max(0.0).sqrt()) / 2.0).sqrt()
    }

    /// What deck draws, through the map pass's multisampling, as the lowest and highest bytes a
    /// pixel may take. Each circle's fragment is worked out at the pixel's centre and blended (into
    /// bytes) into the samples its quad covers, the items in order, starting from `background`;
    /// then the samples are averaged. The range allows for the rasteriser's vertex snapping
    /// ([`SNAP_PX`]): the fragment's distance from the centre may be off by that much, and a sample
    /// that close to the quad's edge may or may not be covered.
    fn expected(vf: &ViewFrame, background: &Image, scale_factor: f64, samples: u32, items: &[Item]) -> Vec<[[u8; 3]; 2]> {
        let offsets = sample_positions(samples);
        let blend = |c: [f64; 4], dst: f64, i: usize| (c[i] * 255.0 * c[3] + dst * (1.0 - c[3])).round().clamp(0.0, 255.0);
        let mut out = Vec::with_capacity((background.width * background.height) as usize);
        for y in 0..background.height {
            for x in 0..background.width {
                let p = background.pixel(x, y);
                let corner = DVec2::new(f64::from(x), f64::from(y));
                let ground = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
                // per sample, the lowest and the highest value of each channel
                let mut ranges = vec![[ground, ground]; offsets.len()];
                for item in items {
                    for spot in &item.spots {
                        let (outer, line) = radii(vf, &item.style, spot);
                        let half = if item.style.antialiasing { outer + 0.5 } else { outer };
                        let centre = (corner + 0.5) / scale_factor;
                        let slack = SNAP_PX / scale_factor * stretch(vf, &item.style, spot, centre);
                        let dist = quad_position(vf, &item.style, spot, centre).length();
                        let fragments: Vec<Option<[f64; 4]>> =
                            [dist - slack, dist, dist + slack].iter().map(|&d| fragment(&item.style, spot, outer, line, d.max(0.0))).collect();
                        if fragments.iter().all(Option::is_none) {
                            continue;
                        }
                        for (range, offset) in ranges.iter_mut().zip(&offsets) {
                            let q = quad_position(vf, &item.style, spot, (corner + *offset) / scale_factor);
                            let edge = half - q.x.abs().max(q.y.abs());
                            if edge < -slack {
                                continue;
                            }
                            let [lo, hi] = *range;
                            let mut drawn = [[f64::MAX; 3], [f64::MIN; 3]];
                            for f in &fragments {
                                for i in 0..3 {
                                    let (l, h) = match f {
                                        Some(c) => (blend(*c, lo[i], i), blend(*c, hi[i], i)),
                                        None => (lo[i], hi[i]),
                                    };
                                    drawn[0][i] = drawn[0][i].min(l);
                                    drawn[1][i] = drawn[1][i].max(h);
                                }
                            }
                            *range = if edge > slack {
                                drawn
                            } else {
                                [std::array::from_fn(|i| drawn[0][i].min(lo[i])), std::array::from_fn(|i| drawn[1][i].max(hi[i]))]
                            };
                        }
                    }
                }
                let n = ranges.len() as f64;
                let resolve = |k: usize| std::array::from_fn(|i| (ranges.iter().map(|r| r[k][i]).sum::<f64>() / n).round() as u8);
                out.push([resolve(0), resolve(1)]);
            }
        }
        out
    }

    /// The image against [`expected`]: every pixel within `tolerance` levels of its range. Returns
    /// the worst excess and the widest range met.
    fn assert_like_deck(image: &Image, expected: &[[[u8; 3]; 2]], tolerance: i32, what: &str) -> (i32, i32) {
        let mut worst = (0, 0, 0);
        let mut widest = 0;
        for y in 0..image.height {
            for x in 0..image.width {
                let p = image.pixel(x, y);
                let [lo, hi] = expected[(y * image.width + x) as usize];
                for i in 0..3 {
                    let (v, l, h) = (i32::from(p[i]), i32::from(lo[i]), i32::from(hi[i]));
                    widest = widest.max(h - l);
                    let excess = (l - v).max(v - h).max(0);
                    if excess > worst.0 {
                        worst = (excess, x, y);
                    }
                }
            }
        }
        let (d, x, y) = worst;
        let [lo, hi] = expected[(y * image.width + x) as usize];
        assert!(d <= tolerance, "{what}: pixel ({x}, {y}) is {:?}, deck draws {lo:?} to {hi:?}", image.pixel(x, y));
        (d, widest)
    }

    /// The colour of `top` blended over `bottom`, alpha `a` (0–255) times `coverage`.
    fn over(bottom: [u8; 4], top: [u8; 3], a: u8, coverage: f64) -> [f64; 3] {
        let a = f64::from(a) / 255.0 * coverage;
        std::array::from_fn(|i| f64::from(top[i]) * a + f64::from(bottom[i]) * (1.0 - a))
    }

    fn halo_and_ring(at: [f64; 2], radius: f64) -> Vec<Item> {
        let billboard = CircleStyle { billboard: true, ..CircleStyle::PIXELS };
        vec![
            Item { style: CircleStyle { filled: true, stroked: false, ..billboard }, state: DrawState::LABEL, spots: vec![Spot::new(at, radius).fill(HALO)] },
            Item { style: CircleStyle { filled: false, stroked: true, ..billboard }, state: DrawState::LABEL, spots: vec![Spot::new(at, radius).line(RING, 2.0)] },
        ]
    }

    /// A ring drawn alone in `RING`'s colour over `background`, measured in `sectors` sectors round
    /// `centre` (CSS px): each sector's (width, mean radius) in CSS px, from the ring's coverage at
    /// every pixel (the blue channel between the ground and the ring's blue at 235/255): an annulus
    /// of width w round radius r covers 2πrw.
    fn ring_sectors(image: &Image, background: &Image, centre: DVec2, scale_factor: f64, sectors: usize) -> Vec<(f64, f64)> {
        let mut area = vec![0.0; sectors];
        let mut moment = vec![0.0; sectors];
        for y in 0..image.height {
            for x in 0..image.width {
                let ground = f64::from(background.pixel(x, y)[2]);
                let top = ground + (f64::from(BLUE[2]) - ground) * f64::from(RING[3]) / 255.0;
                let coverage = ((f64::from(image.pixel(x, y)[2]) - ground) / (top - ground)).clamp(0.0, 1.5);
                if coverage == 0.0 {
                    continue;
                }
                let d = (DVec2::new(f64::from(x), f64::from(y)) + 0.5) / scale_factor - centre;
                let angle = d.y.atan2(d.x).rem_euclid(std::f64::consts::TAU);
                let k = ((angle / std::f64::consts::TAU * sectors as f64) as usize).min(sectors - 1);
                let a = coverage / (scale_factor * scale_factor);
                area[k] += a;
                moment[k] += a * d.length();
            }
        }
        (0..sectors)
            .map(|k| {
                let r = moment[k] / area[k];
                (area[k] / (std::f64::consts::TAU * r / sectors as f64), r)
            })
            .collect()
    }

    /// Item E9's acceptance: a ring of 2 px stroke `[55,142,240,235]` over a halo `[55,142,240,56]`,
    /// billboards drawn through everything (`{cull none, depth write off, depth always}`), as the
    /// ship and train selections are: on the globe (zoom 9, with the earth) and on the pitched flat
    /// map of V4, at scale factors 1 and 2. Every pixel is what deck's shader gives, blended as deck
    /// blends; the halo's inside is the blue at 56/255 over the ground, the ring's middle line the
    /// blue at 235/255 over the halo's fading edge, the ground shows outside; drawn alone, the ring
    /// is 2 CSS pixels wide round its radius all the way round.
    #[test]
    fn a_ring_over_a_halo() {
        let Some(gpu) = gpu() else { return };
        let radius = 20.0;
        let size = [120.0, 100.0];
        for (name, v, earth) in [
            ("globe", view(STOCKHOLM, 9.0, 0.0, 0.0), true),
            ("flat", view(ARLANDA, 13.5, 45.0, 30.0), false),
        ] {
            for scale_factor in [1.0, 2.0] {
                let items = halo_and_ring([v.longitude, v.latitude], radius);
                let (background, image, vf, samples) = render(&gpu, v, size, scale_factor, earth, &items);
                if scale_factor == 2.0 {
                    save(&image, &format!("e9-ring-{name}.png"));
                }
                let what = format!("{name} at scale {scale_factor}");
                let worst = assert_like_deck(&image, &expected(&vf, &background, scale_factor, samples, &items), 2, &what);
                eprintln!("{what}: off deck's range by {}, ranges up to {} wide", worst.0, worst.1);

                let s = scale_factor;
                let centre = DVec2::from_array(size) / 2.0;
                // the pixel `d` CSS px right of the centre, and its centre's distance from it
                let probe = |d: f64| {
                    let [x, y] = [((centre.x + d) * s) as u32, (centre.y * s) as u32];
                    let at = (DVec2::new(f64::from(x), f64::from(y)) + 0.5) / s;
                    (image.pixel(x, y), background.pixel(x, y), (at - centre).length())
                };
                let close = |got: [u8; 4], want: [f64; 3], tolerance: f64, part: &str| {
                    for i in 0..3 {
                        assert!((f64::from(got[i]) - want[i]).abs() <= tolerance, "{what}: {part} {got:?} against {want:?}");
                    }
                };
                // inside the halo
                let (inside, ground, _) = probe(10.0);
                close(inside, over(ground, BLUE, HALO[3], 1.0), 1.0, "halo");
                // on the ring's middle line: the ring over the halo's smoothed edge
                let (on, ground, d) = probe(radius);
                let halo = over(ground, BLUE, HALO[3], smoothedge(d, radius)).map(|c| c.round() as u8);
                close(on, over([halo[0], halo[1], halo[2], 255], BLUE, RING[3], 1.0), 2.0, "ring");
                // outside, the ground
                let (out, ground, _) = probe(radius + 3.0);
                assert_eq!(out, ground, "{what}");

                // the ring alone: 2 CSS px wide round the radius, all the way round and in each
                // eighth (whose pixels only roughly fill its wedge)
                let (background, ring, _, _) = render(&gpu, v, size, scale_factor, earth, &items[1..]);
                let [(width, r)] = ring_sectors(&ring, &background, centre, s, 1)[..] else { unreachable!() };
                assert!((width - 2.0).abs() < 0.05 && (r - radius).abs() < 0.05, "{what}: the ring is {width} px wide, {r} px out");
                for (k, (width, r)) in ring_sectors(&ring, &background, centre, s, 8).into_iter().enumerate() {
                    assert!((width - 2.0).abs() < 0.2, "{what}: the ring is {width} px wide in sector {k}");
                    assert!((r - radius).abs() < 0.1, "{what}: the ring runs {r} px out in sector {k}");
                }
            }
        }
    }

    /// The other circles of the app, against deck's shader pixel by pixel: the airport and place dots
    /// (filled and stroked billboards, [`MARKER`]), the route's waypoints (filled on the ground,
    /// `SURFACE_NO_CULL`), a train stop on the ground, at half opacity, and without antialiasing.
    #[test]
    fn fills_and_lines_mix_as_deck_draws_them() {
        let Some(gpu) = gpu() else { return };
        let pixels_billboard = CircleStyle { billboard: true, stroked: true, ..CircleStyle::PIXELS };
        for (name, v, earth) in [
            ("globe", view(STOCKHOLM, 9.0, 0.0, 0.0), true),
            ("globe pitched", view(STOCKHOLM, 7.5, 50.0, -40.0), true),
            ("flat", view(ARLANDA, 13.5, 0.0, 0.0), false),
            ("flat pitched", view(ARLANDA, 15.5, 60.0, 20.0), false),
        ] {
            let vf = ViewFrame::new(&v, [160.0, 120.0], 1.0, 0.0);
            // places 30 px apart round the centre on the screen
            let at = |dx: f64, dy: f64| vf.viewport.unproject([80.0 + 30.0 * dx, 60.0 + 30.0 * dy], 0.0);
            let ground = CircleStyle::PIXELS;
            let items = vec![
                Item {
                    style: ground,
                    state: DrawState::SURFACE_NO_CULL,
                    spots: vec![Spot::new(at(-1.0, 1.0), 2.5).fill([241, 238, 231, 150]), Spot::new(at(1.0, 1.0), 2.5).fill([241, 238, 231, 150])],
                },
                Item {
                    style: CircleStyle { stroked: true, ..ground },
                    state: DrawState::LABEL,
                    spots: vec![Spot::new(at(0.0, -1.0), 4.5).fill([20, 24, 30, 255]).line([235, 238, 245, 255], 2.0)],
                },
                Item {
                    style: CircleStyle { opacity: 0.5, ..pixels_billboard },
                    state: DrawState::LABEL,
                    spots: vec![Spot::new(at(-1.0, -1.0), 6.0).fill([90, 200, 120, 255]).line([10, 14, 20, 255], 2.0)],
                },
                Item {
                    style: CircleStyle { antialiasing: false, ..pixels_billboard },
                    state: DrawState::LABEL,
                    spots: vec![Spot::new(at(1.0, -1.0), 7.0).fill([0, 0, 0, 0]).line([255, 200, 0, 255], 2.0)],
                },
                Item {
                    style: pixels_billboard,
                    state: MARKER,
                    spots: vec![
                        Spot::new(at(0.0, 0.0), 5.0).fill([55, 142, 240, 255]).line([6, 12, 19, 230], 2.0),
                        Spot::new(at(0.0, 1.2), 4.0).fill([241, 238, 231, 255]).line([6, 12, 19, 230], 2.0),
                    ],
                },
            ];
            let (background, image, vf, samples) = render(&gpu, v, [160.0, 120.0], 1.0, earth, &items);
            save(&image, &format!("e9-dots-{}.png", name.replace(' ', "-")));
            let worst = assert_like_deck(&image, &expected(&vf, &background, 1.0, samples, &items), 2, name);
            eprintln!("{name}: off deck's range by {}, ranges up to {} wide", worst.0, worst.1);
        }
    }

    /// Radii and line widths in metres become pixels at the view centre's scale (deck's
    /// `commonUnitsPerMeter`, on the globe and on the flat map), then the pixel clamps apply.
    #[test]
    fn metre_radii_follow_the_scale_and_the_pixel_clamps() {
        let Some(gpu) = gpu() else { return };
        for v in [view(STOCKHOLM, 9.0, 0.0, 0.0), view(ARLANDA, 13.5, 0.0, 0.0)] {
            let vf = ViewFrame::new(&v, [160.0, 120.0], 1.0, 0.0);
            let metres = 25.0 / (vf.viewport.distance_scales.units_per_meter[2] * vf.viewport.scale);
            let style = CircleStyle { stroked: true, billboard: true, ..CircleStyle::DEFAULT };
            let spot = Spot::new([v.longitude, v.latitude], metres).fill([200, 60, 40, 255]).line([240, 240, 240, 255], metres / 10.0);
            for (style, outer) in [
                (style, 25.0 + 1.25),
                (CircleStyle { radius_max_pixels: 12.0, line_width_min_pixels: 4.0, ..style }, 12.0 + 2.0),
                (CircleStyle { radius_scale: 0.5, radius_min_pixels: 16.0, line_width_scale: 2.0, ..style }, 16.0 + 2.5),
            ] {
                let items = vec![Item { style, state: DrawState::LABEL, spots: vec![spot] }];
                assert!((radii(&vf, &style, &spot).0 - outer).abs() < 1e-9);
                let (background, image, vf, samples) = render(&gpu, v, [160.0, 120.0], 1.0, false, &items);
                assert_like_deck(&image, &expected(&vf, &background, 1.0, samples, &items), 2, &format!("{v:?} {style:?}"));
                // the outer edge where the clamps put it
                let row: Vec<u8> = (0..image.width).map(|x| image.pixel(x, image.height / 2)[0]).collect();
                let lit = row.iter().filter(|&&r| r > 100).count() as f64;
                assert!((lit - 2.0 * outer).abs() <= 2.0, "{v:?} {style:?}: {lit} px across, {outer} px radius");
            }
        }
    }

    /// Pitched, a billboard stays round (and is its size at the view centre's depth, larger nearer
    /// the camera by `focal_w / w`, as deck draws it) while a circle on the ground foreshortens: its
    /// height on the screen against its width is what the camera makes of a ground circle.
    #[test]
    fn billboards_stay_round_and_ground_circles_foreshorten() {
        let Some(gpu) = gpu() else { return };
        for v in [view(ARLANDA, 15.5, 60.0, 0.0), view(STOCKHOLM, 8.0, 60.0, 0.0)] {
            let size = [200.0, 160.0];
            let vf = ViewFrame::new(&v, size, 1.0, 0.0);
            let near = {
                let [lon, lat] = vf.viewport.unproject([100.0, 130.0], 0.0);
                [lon, lat]
            };
            let fill = [255, 255, 255, 255];
            for billboard in [true, false] {
                for at in [[v.longitude, v.latitude], near] {
                    let style = CircleStyle { billboard, ..CircleStyle::PIXELS };
                    let items = vec![Item { style, state: DrawState::LABEL, spots: vec![Spot::new(at, 20.0).fill(fill)] }];
                    let (background, image, vf, samples) = render(&gpu, v, size, 1.0, false, &items);
                    // the lit box
                    let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0, u32::MAX, 0);
                    let mut area = 0.0;
                    for y in 0..image.height {
                        for x in 0..image.width {
                            let a = (f64::from(image.pixel(x, y)[0]) - 30.0) / 225.0;
                            area += a;
                            if a > 0.5 {
                                (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
                            }
                        }
                    }
                    let (w, h) = (f64::from(x1 - x0 + 1), f64::from(y1 - y0 + 1));
                    let vp = &vf.viewport;
                    let centre = vp.project_position([at[0], at[1], 0.0]);
                    if billboard {
                        let k = focal_w(&vf) / (vp.view_projection_matrix * centre.extend(1.0)).w;
                        assert!((w - h).abs() <= 1.0, "{v:?} at {at:?}: billboard {w} × {h}");
                        assert!((w - 40.0 * k).abs() <= 1.5, "{v:?} at {at:?}: billboard {w} px wide, deck {}", 40.0 * k);
                        assert!((area - std::f64::consts::PI * (20.0 * k).powi(2)).abs() < 0.03 * area, "{v:?}: area {area}");
                        if at == near {
                            assert!(k > 1.1, "the near circle is nearer the camera: {k}");
                        } else {
                            assert!((k - 1.0).abs() < 1e-6, "{k}");
                        }
                    } else {
                        // a ground circle of 20 / scale common units, projected
                        let frame = if vf.is_globe() { orientation(centre) } else { DMat3::IDENTITY };
                        let points: Vec<[f64; 3]> = (0..360)
                            .map(|d| {
                                let t = f64::from(d).to_radians();
                                vp.world_to_pixels(centre + frame * DVec3::new(t.cos(), t.sin(), 0.0) * (20.0 / vp.scale))
                            })
                            .collect();
                        let ew = points.iter().map(|p| p[0]).fold(f64::MIN, f64::max) - points.iter().map(|p| p[0]).fold(f64::MAX, f64::min);
                        let eh = points.iter().map(|p| p[1]).fold(f64::MIN, f64::max) - points.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
                        assert!((w - ew).abs() <= 1.5 && (h - eh).abs() <= 1.5, "{v:?} at {at:?}: ground circle {w} × {h}, camera {ew} × {eh}");
                        // pitched 60°, the ground circle foreshortens (less so near the camera,
                        // which looks down on it more steeply)
                        assert!(h < if at == near { 0.75 } else { 0.6 } * w, "{v:?} at {at:?}: the ground circle is {w} × {h}");
                    }
                    assert_like_deck(&image, &expected(&vf, &background, 1.0, samples, &items), 2, &format!("{v:?} billboard {billboard} at {at:?}"));
                }
            }
        }
    }

    /// On the globe a ground circle away from the view centre lies along the globe's surface there
    /// (deck's `project_get_orientation_matrix` of its up direction).
    #[test]
    fn ground_circles_on_the_globe_lie_on_the_surface() {
        let Some(gpu) = gpu() else { return };
        let v = view([10.0, 45.0], 4.0, 30.0, 25.0);
        let size = [240.0, 180.0];
        let style = CircleStyle { stroked: true, ..CircleStyle::PIXELS };
        let spots = [[12.5, 47.0], [7.0, 43.5], [10.0, 48.0]].map(|at| Spot::new(at, 14.0).fill([255, 120, 0, 255]).line([255, 255, 255, 255], 3.0));
        let items = vec![Item { style, state: DrawState::LABEL, spots: spots.to_vec() }];
        let (background, image, vf, samples) = render(&gpu, v, size, 1.0, true, &items);
        save(&image, "e9-globe-ground.png");
        let worst = assert_like_deck(&image, &expected(&vf, &background, 1.0, samples, &items), 2, "globe ground circles");
        eprintln!("globe ground circles: off deck's range by {}, ranges up to {} wide", worst.0, worst.1);
    }

    /// deck's `focalDistance` (the clip w of the point the focal distance ahead of the camera) is
    /// the clip w of the view centre, which the shader reads from `globals.centre_cam`: on both
    /// projections, pitched, turned and with a raised target.
    #[test]
    fn focal_w_is_the_clip_w_of_the_view_centre() {
        for v in [
            view(STOCKHOLM, 1.9, 0.0, 0.0),
            view(STOCKHOLM, 9.0, 50.0, 30.0),
            view(ARLANDA, 13.5, 45.0, 30.0),
            view(ARLANDA, 19.0, 85.0, -120.0),
            ViewState { position: Some([0.0, 0.0, 120.0]), ..view(ARLANDA, 16.0, 60.0, 10.0) },
            ViewState { position: Some([0.0, 0.0, 3000.0]), ..view(STOCKHOLM, 11.0, 60.0, 10.0) },
        ] {
            let vf = ViewFrame::new(&v, [1100.0, 720.0], 1.0, 0.0);
            let deck = focal_w(&vf);
            let m = glam::Mat4::from_cols_array_2d(&vf.globals.centre_cam).as_dmat4();
            let shader = (m * DVec4::new(0.0, 0.0, 0.0, 1.0)).w;
            assert!((shader - deck).abs() < 1e-6 * deck, "{v:?}: shader {shader}, deck {deck}");
            // and the view centre (raised) projects there in f64
            let centre_w = (vf.viewport.view_projection_matrix * vf.viewport.center.extend(1.0)).w;
            assert!((centre_w - deck).abs() < 1e-9 * deck, "{v:?}: centre {centre_w}, deck {deck}");
        }
    }

    /// Neither filled nor stroked draws nothing; nor do no circles.
    #[test]
    fn nothing_is_drawn_unless_filled_or_stroked() {
        let Some(gpu) = gpu() else { return };
        let v = view(ARLANDA, 13.5, 0.0, 0.0);
        let spot = Spot::new(ARLANDA, 10.0).fill([255, 0, 0, 255]).line([0, 255, 0, 255], 2.0);
        let items = vec![
            Item { style: CircleStyle { filled: false, ..CircleStyle::PIXELS }, state: DrawState::LABEL, spots: vec![spot] },
            Item { style: CircleStyle::PIXELS, state: DrawState::LABEL, spots: vec![] },
        ];
        let (background, image, _, _) = render(&gpu, v, [64.0, 48.0], 1.0, false, &items);
        assert_eq!(image.rgba, background.rgba);
    }

    /// Instances are rewritten in place, their buffer grown when they do not fit; a style is written
    /// only when it changes.
    #[test]
    fn instances_are_rewritten_and_grow() {
        let Some(gpu) = gpu() else { return };
        let vf = ViewFrame::new(&view(ARLANDA, 13.5, 0.0, 0.0), [100.0, 100.0], 1.0, 0.0);
        let mut instances = Instances::new(&gpu.device, "test");
        assert!(instances.is_empty());
        let list: Vec<Instance> = (0..9).map(|i| Instance::at(&vf, [ARLANDA[0] + f64::from(i) * 1e-3, ARLANDA[1]], 0.0)).collect();
        instances.write(&gpu.device, &gpu.queue, &list);
        assert_eq!(instances.count(), 9);
        assert!(instances.buffer.size() >= (9 * size_of::<Instance>()) as u64);
        instances.write(&gpu.device, &gpu.queue, &list[..2]);
        assert_eq!(instances.count(), 2);
        instances.write(&gpu.device, &gpu.queue, &[]);
        assert!(instances.is_empty());
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        let init = renderer.init(&gpu);
        let circle = Circle::new(&init, "test", DrawState::LABEL);
        let mut style = circle.style(&gpu.device, &CircleStyle::PIXELS);
        style.set(&gpu.queue, &CircleStyle { opacity: 0.5, ..CircleStyle::PIXELS });
        assert_eq!(style.get().opacity, 0.5);
        renderer.add_layer(Box::new(Circles { items: Vec::new() }));
    }

    /// [`Instance::at`]: the common offset of the place, the globe's normal there (the flat map's
    /// up), deck's defaults.
    #[test]
    fn instances_start_from_deck_defaults() {
        let globe = ViewFrame::new(&view(STOCKHOLM, 6.0, 0.0, 0.0), [100.0, 100.0], 1.0, 0.0);
        let i = Instance::at(&globe, [20.0, 50.0], 300.0);
        assert_eq!(i.offset(), globe.common_offset([20.0, 50.0], 300.0));
        let up = DVec3::from_array(i.up.map(f64::from));
        let (sin_lon, cos_lon) = 20f64.to_radians().sin_cos();
        let (sin_lat, cos_lat) = 50f64.to_radians().sin_cos();
        assert!((up - DVec3::new(sin_lon * cos_lat, -cos_lon * cos_lat, sin_lat)).length() < 1e-6, "{up}");
        assert_eq!((i.radius, i.line_width, i.fill_color, i.line_color), (1.0, 1.0, DEFAULT_COLOR, DEFAULT_COLOR));
        let flat = ViewFrame::new(&view(ARLANDA, 14.0, 0.0, 0.0), [100.0, 100.0], 1.0, 0.0);
        let i = Instance::at(&flat, ARLANDA, 0.0).radius(5.0).fill([1, 2, 3, 4]).line([5, 6, 7, 8], 2.5);
        assert_eq!(i.up, [0.0, 0.0, 1.0]);
        assert!(i.offset().length() < 1e-4, "{}", i.offset());
        assert_eq!((i.radius, i.line_width, i.fill_color, i.line_color), (5.0, 2.5, [1, 2, 3, 4], [5, 6, 7, 8]));
    }

    #[test]
    fn layouts_match_the_wgsl() {
        let names = [
            "radius_scale",
            "radius_min_pixels",
            "radius_max_pixels",
            "line_width_scale",
            "line_width_min_pixels",
            "line_width_max_pixels",
            "stroked",
            "filled",
            "antialiasing",
            "billboard",
            "radius_units",
            "line_width_units",
            "opacity",
            "pad0",
            "pad1",
            "pad2",
        ];
        let expected: Vec<(String, u32)> = names.iter().enumerate().map(|(i, n)| (n.to_string(), 4 * i as u32)).collect();
        assert_eq!(wgsl_members(SHADER, "CircleStyle"), (size_of::<StyleUniform>() as u32, expected));
        assert_eq!(std::mem::offset_of!(StyleUniform, opacity), 48);
        assert_eq!(std::mem::offset_of!(StyleUniform, pad), 52);
        // the instance attributes sit where `Instance` keeps its fields
        let offsets: Vec<u64> = Instance::LAYOUT.attributes.iter().map(|a| a.offset).collect();
        let fields = [
            std::mem::offset_of!(Instance, position),
            std::mem::offset_of!(Instance, up),
            std::mem::offset_of!(Instance, radius),
            std::mem::offset_of!(Instance, line_width),
            std::mem::offset_of!(Instance, fill_color),
            std::mem::offset_of!(Instance, line_color),
        ];
        assert_eq!(offsets, fields.map(|f| f as u64));
        assert_eq!(Instance::LAYOUT.array_stride as usize, size_of::<Instance>());
        assert_eq!(size_of::<Instance>(), 40);
        // deck's defaults
        let d = CircleStyle::default();
        assert_eq!((d.radius_units, d.line_width_units), (Units::Meters, Units::Meters));
        assert!(d.filled && !d.stroked && !d.billboard && d.antialiasing);
        assert_eq!((d.radius_scale, d.radius_min_pixels, d.radius_max_pixels), (1.0, 0.0, MAX_SAFE_INTEGER));
        assert_eq!(MARKER, DrawState { cull: None, depth_write: true, depth_compare: wgpu::CompareFunction::Always, stencil: None });
        let u = CircleStyle::PIXELS.uniform();
        assert_eq!((u.radius_units, u.line_width_units, u.filled, u.stroked, u.antialiasing, u.billboard), (2, 2, 1.0, 0.0, 1, 0));
    }
}
