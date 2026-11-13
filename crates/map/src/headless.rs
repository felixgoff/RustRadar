//! The map without a window (plan 1.2, section 8): render a [`SceneInput`] to an RGBA image, for
//! tests and the app's `--screenshot`.
//!
//! [`render`] does it all on a fresh device; [`render_with`] reuses a device and a renderer;
//! [`Offscreen`] is the stand-in surface (the app's headless path draws its egui pass onto it too,
//! then reads it back).

use std::path::Path;
use std::time::{Duration, Instant};

use crate::gpu::{Gpu, GpuError};
use crate::renderer::{MapRenderer, SceneInput};

/// The offscreen surface's format: gamma-space RGBA, bytes straight to PNG.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// How long a shot ([`render_with`], the app's `--screenshot`) waits for layers that are still
/// loading, unless [`WAIT_VAR`] says otherwise ([`max_wait`]). Release builds settle every named
/// view within seconds (P1), so their 30 s only bounds a shot whose work never comes; debug
/// builds decode and tessellate tiles many times slower and keep the 60 s they had.
pub const MAX_WAIT: Duration = if cfg!(debug_assertions) { Duration::from_secs(60) } else { Duration::from_secs(30) };

/// The environment variable that sets the wait of [`max_wait`], in seconds (a number ≥ 0).
pub const WAIT_VAR: &str = "RUSTRADAR_SHOT_WAIT";

/// The wait for loading layers: [`WAIT_VAR`] seconds when set, else [`MAX_WAIT`].
pub fn max_wait() -> Duration {
    wait_from(std::env::var(WAIT_VAR).ok().as_deref())
}

/// The wait `value` (seconds, as [`WAIT_VAR`] holds it) asks for: [`MAX_WAIT`] when there is none
/// or it is not a number ≥ 0.
pub fn wait_from(value: Option<&str>) -> Duration {
    value.and_then(|v| v.trim().parse::<f64>().ok()).and_then(|s| Duration::try_from_secs_f64(s).ok()).unwrap_or(MAX_WAIT)
}

/// A picture read back from the GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, top row first.
    pub rgba: Vec<u8>,
}

impl Image {
    /// The pixel at `(x, y)`, from the top left.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    /// Writes a PNG.
    pub fn save_png(&self, path: &Path) -> Result<(), image::ImageError> {
        image::save_buffer(path, &self.rgba, self.width, self.height, image::ExtendedColorType::Rgba8)
    }
}

/// A texture standing in for the window's surface.
pub struct Offscreen {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

impl Offscreen {
    /// A `size`-pixel [`FORMAT`] texture to render into and read back.
    pub fn new(gpu: &Gpu, size: [u32; 2]) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Offscreen { texture, view, size }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Copies the texture back (waits for the GPU).
    pub fn read(&self, gpu: &Gpu) -> Result<Image, GpuError> {
        read_texture(gpu, &self.texture, self.size)
    }
}

/// Reads a single-sample RGBA8 texture of `size` back to the CPU.
pub fn read_texture(gpu: &Gpu, texture: &wgpu::Texture, size: [u32; 2]) -> Result<Image, GpuError> {
    let [width, height] = size;
    // rows of a texture-to-buffer copy are padded to 256 bytes
    let row = width as usize * 4;
    let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded_row * height as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row as u32), rows_per_image: None },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Device(e.to_string()))?;
    rx.recv().map_err(|e| GpuError::Device(e.to_string()))?.map_err(|e| GpuError::Device(e.to_string()))?;
    let mapped = buffer.get_mapped_range(..).map_err(|e| GpuError::Device(e.to_string()))?;
    let mut rgba = Vec::with_capacity(row * height as usize);
    for padded in mapped.chunks_exact(padded_row) {
        rgba.extend_from_slice(&padded[..row]);
    }
    Ok(Image { width, height, rgba })
}

/// The surface size of `scene` on a display of `scale_factor` device pixels per CSS pixel.
pub fn surface_size(scene: &SceneInput, scale_factor: f64) -> [u32; 2] {
    [(scene.size[0] * scale_factor).round().max(1.0) as u32, (scene.size[1] * scale_factor).round().max(1.0) as u32]
}

/// Renders `scene` as a window of `scale_factor` would show it (without the UI), on a new device.
pub fn render(scene: &SceneInput, scale_factor: f64) -> Result<Image, GpuError> {
    let gpu = Gpu::headless()?;
    let mut renderer = MapRenderer::new(&gpu, FORMAT);
    render_with(&gpu, &mut renderer, scene, scale_factor)
}

/// Renders `scene` with `renderer`: prepares frames until no layer is pending (at most
/// [`max_wait`]), then draws two, read back.
///
/// The frames before are prepared (the layers take their tiles and textures in, within the
/// frame's upload budget) but not drawn: nobody sees them, and on a software rasteriser a map
/// pass can take seconds (P1: five at V2's dark map on SwiftShader).
pub fn render_with(gpu: &Gpu, renderer: &mut MapRenderer, scene: &SceneInput, scale_factor: f64) -> Result<Image, GpuError> {
    let target = Offscreen::new(gpu, surface_size(scene, scale_factor));
    renderer.update(scene.clone());
    let started = Instant::now();
    let wait = max_wait();
    let poll = || gpu.device.poll(wgpu::PollType::wait_indefinitely()).map(|_| ()).map_err(|e| GpuError::Device(e.to_string()));
    loop {
        renderer.prepare(gpu, scene.local_now_ms);
        // the uploads the frame staged go to the GPU now
        gpu.queue.submit(std::iter::empty::<wgpu::CommandBuffer>());
        poll()?;
        if !renderer.pending() || started.elapsed() > wait {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..2 {
        renderer.render(gpu, target.view(), target.size(), scene.local_now_ms);
        poll()?;
    }
    target.read(gpu)
}

#[cfg(test)]
mod tests {
    use glam::DVec2;
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, lng_lat_to_world};

    use super::*;
    use crate::frame::{FrameSlot, power_of_two_scale};
    use crate::gpu::testing::gpu;
    use crate::layers::{DrawCx, Init, Layer, PrepareCx};
    use crate::pipelines::{self, Desc};
    use crate::renderer::map_scale;
    use crate::shaders;
    use crate::state::DrawState;
    use crate::target::CLEAR_RGB;

    fn assert_uniform(image: &Image, rgb: [u8; 3]) {
        let expected = [rgb[0], rgb[1], rgb[2], 255];
        let wrong = image.rgba.chunks_exact(4).filter(|p| *p != expected).count();
        assert_eq!(wrong, 0, "{wrong} of {} pixels are not {expected:?}, e.g. {:?}", image.width * image.height, image.pixel(0, 0));
    }

    /// The shot's wait (P1): `RUSTRADAR_SHOT_WAIT` seconds when it is a number ≥ 0, else
    /// [`MAX_WAIT`] (30 s in a release build, 60 s in a debug one).
    #[test]
    fn the_wait_is_read_in_seconds() {
        assert_eq!(MAX_WAIT, Duration::from_secs(if cfg!(debug_assertions) { 60 } else { 30 }));
        assert_eq!(wait_from(None), MAX_WAIT);
        assert_eq!(wait_from(Some("12.5")), Duration::from_millis(12_500));
        assert_eq!(wait_from(Some(" 0 ")), Duration::ZERO);
        for wrong in ["", "-1", "soon", "inf", "NaN"] {
            assert_eq!(wait_from(Some(wrong)), MAX_WAIT, "{wrong:?}");
        }
    }

    /// E4's acceptance: with every layer still empty, the headless frame is exactly `(30,30,30)`
    /// everywhere, through the MSAA resolve and the composite, at map scales 1, 2 and the realistic
    /// 1.5 (bilinear upscaling onto a scale-2 surface).
    #[test]
    fn headless_frame_is_exactly_ink_everywhere() {
        let Some(gpu) = gpu() else { return };
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        eprintln!("rendering with {}, MSAA x{}", gpu.describe(), renderer.samples());
        assert_eq!(CLEAR_RGB, [30, 30, 30]);
        for (basemap, scale_factor, view) in [
            (Basemap::Dark, 1.0, ViewState::INITIAL),
            (Basemap::Satellite, 2.0, ViewState { zoom: 14.0, pitch: 60.0, ..ViewState::INITIAL }),
            (Basemap::Realistic, 2.0, ViewState::INITIAL),
        ] {
            let scene = SceneInput::new(view, [320.0, 200.0], map_scale(basemap, scale_factor), basemap);
            let image = render_with(&gpu, &mut renderer, &scene, scale_factor).expect("rendered");
            assert_eq!([image.width, image.height], surface_size(&scene, scale_factor));
            assert_eq!(renderer.target().size(), scene.map_size());
            assert_uniform(&image, CLEAR_RGB);
        }
    }

    #[test]
    fn an_srgb_surface_gets_the_same_bytes() {
        let Some(gpu) = gpu() else { return };
        let srgb = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut renderer = MapRenderer::new(&gpu, srgb);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("srgb surface"),
            size: wgpu::Extent3d { width: 64, height: 48, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: srgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        renderer.update(SceneInput::new(ViewState::INITIAL, [64.0, 48.0], 1.0, Basemap::Dark));
        renderer.render(&gpu, &texture.create_view(&Default::default()), [64, 48], 0.0);
        // the stored sRGB bytes are what the map holds
        assert_uniform(&read_texture(&gpu, &texture, [64, 48]).expect("read"), CLEAR_RGB);
    }

    #[test]
    fn the_layers_come_in_the_order_of_doc_03_section_3() {
        let Some(gpu) = gpu() else { return };
        let renderer = MapRenderer::new(&gpu, FORMAT);
        let names = renderer.layer_names();
        assert_eq!(
            names,
            [
                "sky", "earth", "terrain", "imagery", "ground-land", "ground-land-cover", "ground", "ground-rails", "land", "hillshade", "clouds", "vector-map", "rail", "radar",
                "night",
                "buildings", "buildings-realistic", "trees", "ships", "blocks", "trains", "labels", "route", "markers", "aircraft", "selection-ring"
            ]
        );
        assert!(!renderer.pending());
    }

    /// A square of local Mercator around the view centre, in `(50,50,50)`, through the whole chain:
    /// a layer's `prepare` pushes its frame, `pipelines::build`, `lib/project.wgsl`, MSAA, composite.
    struct Square {
        pipeline: wgpu::RenderPipeline,
        vertices: wgpu::Buffer,
        /// world Mercator north-west corner and size
        anchor: DVec2,
        scale: f64,
        slot: Option<FrameSlot>,
    }

    const SQUARE: &str = "//!include lib/project.wgsl
@vertex
fn vs(@location(0) local: vec3<f32>) -> @builtin(position) vec4<f32> {
    return project_local(local).clip;
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4<f32>(50.0 / 255.0, 50.0 / 255.0, 50.0 / 255.0, 1.0);
}
";

    impl Square {
        fn new(init: &Init<'_>, anchor: DVec2, scale: f64) -> Self {
            let device = &init.gpu.device;
            let module = shaders::module(device, "square", SQUARE);
            let pipeline = pipelines::build(
                init,
                &Desc {
                    label: "square",
                    module: &module,
                    vertex_entry: "vs",
                    fragment_entry: "fs",
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    })],
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    state: DrawState::SURFACE_NO_CULL,
                    groups: &[],
                },
            );
            let corners: [[f32; 3]; 6] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
            let vertices = wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor { label: Some("square"), contents: bytemuck::cast_slice(&corners), usage: wgpu::BufferUsages::VERTEX },
            );
            Square { pipeline, vertices, anchor, scale, slot: None }
        }
    }

    impl Layer for Square {
        fn name(&self) -> &'static str {
            "square"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.slot = Some(cx.frames.push(&cx.view.local(self.anchor, self.scale, 0.0)));
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            pass.set_pipeline(&self.pipeline);
            cx.bind_frame(pass, self.slot.expect("prepared"));
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.draw(0..6, 0..1);
        }
    }

    #[test]
    fn a_local_mercator_square_lands_where_the_camera_puts_it() {
        let Some(gpu) = gpu() else { return };
        for view in [
            ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None },
            ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 25.0, pitch: 50.0, position: None },
            ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, bearing: 30.0, pitch: 45.0, position: None },
        ] {
            let size = [480.0, 320.0];
            let scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
            let centre = DVec2::from_array(lng_lat_to_world([view.longitude, view.latitude]));
            // a square of about 100 px around the centre
            let px_per_unit = 2f64.powf(view.zoom) / view.latitude.to_radians().cos();
            let scale = power_of_two_scale(100.0 / px_per_unit);
            let anchor = centre + DVec2::new(-scale / 2.0, scale / 2.0);
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            let square = Square::new(&renderer.init(&gpu), anchor, scale);
            renderer.add_layer(Box::new(square));
            let image = render_with(&gpu, &mut renderer, &scene, 1.0).expect("rendered");
            let vp = &renderer.view().viewport;
            let grey = [50, 50, 50, 255];
            let ink = [30, 30, 30, 255];
            assert_eq!(image.pixel(240, 160), grey, "{view:?}: no square at the centre");
            assert_eq!(image.pixel(2, 2), ink, "{view:?}");
            // just inside and just outside each corner, as scene::camera projects it
            for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                let at = |du: f64, dv: f64| {
                    let w = anchor + DVec2::new((u + du) * scale, -(v + dv) * scale);
                    let [lon, lat] = rustradar_scene::camera::world_to_lng_lat(w.to_array());
                    let [x, y, _] = vp.project([lon, lat, 0.0]);
                    image.pixel(x.round() as u32, y.round() as u32)
                };
                let inward = |c: f64| if c == 0.0 { 0.08 } else { -0.08 };
                assert_eq!(at(inward(u), inward(v)), grey, "{view:?}: inside corner {u},{v}");
                assert_eq!(at(-inward(u), -inward(v)), ink, "{view:?}: outside corner {u},{v}");
            }
            if view.zoom == 13.5 {
                std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots")).expect("shot folder");
                image.save_png(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots/e4-square.png"))).expect("png");
            }
        }
    }
}
