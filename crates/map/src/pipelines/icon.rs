//! The `icon` pipeline (plan 1.4.6, item E8): deck.gl 9.4's IconLayer, for the aircraft icons and
//! their outlines, the ship icons and the train icons. Its shader is `shaders/icon.wgsl`.
//!
//! - [`IconDef`]: one entry of deck's `iconMapping` (the cell's rectangle in the atlas, its anchor,
//!   whether it is a mask); [`IconDef::instance_def`] is deck's `getInstanceIconDef`.
//! - [`Instance`]: one icon: its common offset ([`Instance::at`] places it from `[lon, lat, metres]`
//!   in f64), size, cell, anchor offset, pixel offset, angle, colour, mask flag and hover highlight.
//! - [`IconStyle`]: deck's layer props (`sizeUnits`, `sizeScale`, `sizeBasis`, `sizeMinPixels`,
//!   `sizeMaxPixels`, `billboard`, `alphaCutoff`) and the layer's `opacity`, with deck's defaults.
//! - [`IconPipeline`]: the pipeline for one draw-state preset (`NO_CULL` for the aircraft, `SHIP`,
//!   `TRAIN`), its sampler (deck's: linear, linear between mip levels, clamped to the edge) and the
//!   corner buffer. [`Batch`]: what one deck IconLayer draws: an atlas, its [`StyleUniform`] and an
//!   instance buffer.
//!
//! What it draws, as deck does. The quad is the icon's cell, scaled so the cell's height (or width,
//! `sizeBasis`) is the icon's size in pixels (`sizeUnits` pixels, metres or common units, clamped to
//! `sizeMinPixels`–`sizeMaxPixels`), around its anchor, turned anticlockwise by the angle, moved by
//! the pixel offset (x right, y down). Then:
//! - **billboard** (`billboard: true`, deck's default): the quad faces the screen and stays upright
//!   whatever the pitch and bearing. Its corners are added in clip space at the view centre's depth
//!   (deck's `project_pixel_size_to_clipspace` with `focalDistance`, [`focal_w`]), so an icon at the
//!   view centre's depth is exactly its size, and one nearer the camera or further from it is larger
//!   or smaller by `focal_w / w`, as deck draws them in a pitched view;
//! - **ground-plane** (`billboard: false`, the train icons): the size in pixels becomes a size on the
//!   ground in common units at the view centre's scale (deck's `project_pixel_size`, pixels /
//!   `scale`), the quad lies in the plane of east and north at the icon, and so it turns with the
//!   map's bearing and foreshortens with its pitch; at the view centre's depth and pitch 0 it is the
//!   same size as a billboard. Angle 0 points the icon's top north. deck lays it along its globe
//!   frame, which is east and north turned half a turn (the train icons add `yaw0 = 180` below zoom
//!   12, doc 02 §5.5); here it lies along true east and north in both projections (plan section 6,
//!   deviation 1), so the train icons' angle is `−heading` at every zoom.
//!
//! The fragment is deck's: the atlas colour, or for a mask icon the instance's RGB with the atlas
//! alpha; alpha × opacity × the instance alpha; under `alphaCutoff` (0.05) discarded, depth
//! included; then the picking module's highlight, `mix(rgb, highlight.rgb, a_h / (a_h + a (1 − a_h)))`
//! with alpha `a_h + a (1 − a_h)`, for the instance carrying one ([`Instance::with_highlight`]; deck
//! highlights one object per layer, `highlightedObjectIndex`).
//!
//! Atlases: any filterable float 2D texture (`Rgba8Unorm`, or BC7 / ASTC / ETC2 from KTX2,
//! `textures`), not premultiplied; deck draws its icon textures with a full mip chain
//! ([`super::bitmap::upload_rgba8`] makes one for an atlas drawn on the CPU). A layer whose atlas has
//! not arrived is simply not drawn (doc 03 §4.4).
//!
//! Not ported: deck's auto-packing of icons from URLs (the app's atlases are all pre-packed), the
//! picking pass (picking is on the CPU, plan 1.4.11), and the filter hooks no layer of the app uses.

use bytemuck::{Pod, Zeroable};
use glam::{DVec4, Vec3};

use super::{Desc, build};
use crate::frame::ViewFrame;
use crate::layers::Init;
use crate::shaders;
use crate::state::DrawState;

pub use super::path::{MAX_SAFE_INTEGER, Units};

/// `shaders/icon.wgsl`.
pub const SHADER: &str = include_str!("../shaders/icon.wgsl");

/// The bind group of a [`Batch`]: its [`StyleUniform`] (binding 0), atlas (1) and sampler (2).
pub const BATCH_GROUP: u32 = 2;

/// deck's `alphaCutoff` default.
pub const ALPHA_CUTOFF: f64 = 0.05;

/// deck's quad (`positions`, a triangle strip): x right and y down across the cell, −1 to 1, so
/// `(−1, −1)` is the cell's top left. The quad winds clockwise on the screen, as deck's does.
const CORNERS: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]];

const CORNER_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 8,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
};

/// deck's `sizeBasis`: which side of the cell the size measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeBasis {
    Width,
    Height,
}

/// One icon of an atlas (an entry of deck's `iconMapping`), in atlas texels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconDef {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// The anchor from the cell's left edge; `None` is deck's default, the middle.
    pub anchor_x: Option<f64>,
    /// The anchor from the cell's top edge; `None` is the middle.
    pub anchor_y: Option<f64>,
    /// A mask icon takes its RGB from the instance colour and only its alpha from the atlas.
    pub mask: bool,
}

impl IconDef {
    /// deck's `MISSING_ICON`: a cell of no size, which draws nothing.
    pub const MISSING: IconDef = IconDef { x: 0.0, y: 0.0, width: 0.0, height: 0.0, anchor_x: None, anchor_y: None, mask: false };

    /// A cell anchored at its middle.
    pub const fn cell(x: f64, y: f64, width: f64, height: f64, mask: bool) -> IconDef {
        IconDef { x, y, width, height, anchor_x: None, anchor_y: None, mask }
    }

    /// This cell anchored at `(x, y)` texels from its top left.
    pub const fn anchored(self, x: f64, y: f64) -> IconDef {
        IconDef { anchor_x: Some(x), anchor_y: Some(y), ..self }
    }

    /// deck's `getInstanceIconDef`: `[width/2 − anchorX, height/2 − anchorY, x, y, width, height,
    /// mask ? 1 : 0]`.
    pub fn instance_def(&self) -> [f64; 7] {
        let anchor_x = self.anchor_x.unwrap_or(self.width / 2.0);
        let anchor_y = self.anchor_y.unwrap_or(self.height / 2.0);
        let mask = if self.mask { 1.0 } else { 0.0 };
        [self.width / 2.0 - anchor_x, self.height / 2.0 - anchor_y, self.x, self.y, self.width, self.height, mask]
    }
}

/// One icon.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Instance {
    /// Its common offset ([`ViewFrame::common_offset`]).
    pub position: [f32; 3],
    /// deck's `getSize`, in the style's `size_units`.
    pub size: f32,
    /// The cell: x, y, width, height in atlas texels (`instanceIconFrames`).
    pub frame: [f32; 4],
    /// The cell's middle minus its anchor, texels (`instanceOffsets`).
    pub offset: [f32; 2],
    /// deck's `getPixelOffset`: CSS pixels, x right, y down, after the turn.
    pub pixel_offset: [f32; 2],
    /// deck's `getAngle`: degrees anticlockwise.
    pub angle: f32,
    /// 1 for a mask icon, 0 for a coloured one (`instanceColorModes`).
    pub color_mode: f32,
    /// deck's `getColor`, RGBA bytes.
    pub color: [u8; 4],
    /// The hover highlight (deck's `highlightColor` on the highlighted object), alpha 0 for none.
    pub highlight: [u8; 4],
}

impl Instance {
    /// No highlight.
    pub const NO_HIGHLIGHT: [u8; 4] = [0; 4];

    /// The instance buffer layout: locations 1 to 9 (0 is the corner).
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Instance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![
            1 => Float32x3, 2 => Float32, 3 => Float32x4, 4 => Float32x2, 5 => Float32x2, 6 => Float32,
            7 => Float32, 8 => Unorm8x4, 9 => Unorm8x4
        ],
    };

    /// The icon `icon` at the common offset `position`, `size` big in the style's units, turned
    /// `angle` degrees anticlockwise, coloured `color`; no pixel offset, no highlight.
    pub fn new(position: Vec3, icon: &IconDef, size: f64, angle: f64, color: [u8; 4]) -> Instance {
        let [ox, oy, x, y, width, height, mask] = icon.instance_def().map(|v| v as f32);
        Instance {
            position: position.into(),
            size: size as f32,
            frame: [x, y, width, height],
            offset: [ox, oy],
            pixel_offset: [0.0; 2],
            angle: angle as f32,
            color_mode: mask,
            color,
            highlight: Self::NO_HIGHLIGHT,
        }
    }

    /// [`Self::new`] at `[lon, lat, metres]` in `view` (the scene refresh's frame).
    pub fn at(view: &ViewFrame, [lon, lat, h]: [f64; 3], icon: &IconDef, size: f64, angle: f64, color: [u8; 4]) -> Instance {
        Self::new(view.common_offset([lon, lat], h), icon, size, angle, color)
    }

    /// With deck's `getPixelOffset`: CSS pixels, x right, y down.
    pub fn with_pixel_offset(self, [x, y]: [f64; 2]) -> Instance {
        Instance { pixel_offset: [x as f32, y as f32], ..self }
    }

    /// With the hover highlight `rgba`.
    pub fn with_highlight(self, rgba: [u8; 4]) -> Instance {
        Instance { highlight: rgba, ..self }
    }
}

/// An icon layer's props (deck's defaults in [`IconStyle::DEFAULT`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconStyle {
    pub size_units: Units,
    pub size_scale: f64,
    pub size_basis: SizeBasis,
    pub size_min_pixels: f64,
    pub size_max_pixels: f64,
    /// Facing the screen, or lying on the ground.
    pub billboard: bool,
    /// Fragments whose alpha (atlas × opacity × instance) is below this are discarded.
    pub alpha_cutoff: f64,
    /// deck's layer `opacity`.
    pub opacity: f64,
}

impl IconStyle {
    /// deck's defaults: billboards, sizes in pixels measuring the cell's height, scale 1, no clamp,
    /// alpha cutoff 0.05, opaque. The aircraft and ship icons.
    pub const DEFAULT: IconStyle = IconStyle {
        size_units: Units::Pixels,
        size_scale: 1.0,
        size_basis: SizeBasis::Height,
        size_min_pixels: 0.0,
        size_max_pixels: MAX_SAFE_INTEGER,
        billboard: true,
        alpha_cutoff: ALPHA_CUTOFF,
        opacity: 1.0,
    };

    /// The defaults lying on the ground (`billboard: false`): the train icons.
    pub const GROUND: IconStyle = IconStyle { billboard: false, ..Self::DEFAULT };

    /// deck's `sizePixels`: `size` in this style's units as CSS pixels in `view`, scaled and
    /// clamped. Metres and common units use the view centre's scale, as deck's shader does.
    pub fn size_pixels(&self, size: f64, view: &ViewFrame) -> f64 {
        let size = size * self.size_scale;
        let vp = &*view.viewport;
        let pixels = match self.size_units {
            Units::Meters => size * vp.distance_scales.units_per_meter[2] * vp.scale,
            Units::Common => size * vp.scale,
            Units::Pixels => size,
        };
        pixels.clamp(self.size_min_pixels, self.size_max_pixels)
    }
}

impl Default for IconStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// deck's `project.focalDistance` uniform: the clip w of a point at the view centre's depth,
/// `(projection · [0, 0, −focalDistance, 1]).w || 1` (`viewport-uniforms.js`). Billboard offsets in
/// clip space are scaled by it, so they come out at their size in pixels at that depth. 1.5 (deck's
/// camera altitude in viewport heights) for both viewports.
pub fn focal_w(view: &ViewFrame) -> f64 {
    let vp = &*view.viewport;
    let w = (vp.projection_matrix * DVec4::new(0.0, 0.0, -vp.focal_distance, 1.0)).w;
    if w == 0.0 || w.is_nan() { 1.0 } else { w }
}

/// `icon.wgsl`'s `IconStyle`: one layer's props, its atlas size and this frame's view terms.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StyleUniform {
    pub size_scale: f32,
    /// 0 width, 1 height.
    pub size_basis: f32,
    pub size_min_pixels: f32,
    pub size_max_pixels: f32,
    /// The atlas's width and height in texels (deck's `iconsTextureDim`).
    pub atlas_size: [f32; 2],
    /// 0 or 1.
    pub billboard: u32,
    /// deck's `UNIT` number.
    pub size_units: u32,
    pub alpha_cutoff: f32,
    pub opacity: f32,
    /// [`focal_w`].
    pub focal_w: f32,
    pub pad0: f32,
    /// [`ViewFrame::common_centre`]: on the globe, where a ground-plane icon's up comes from.
    pub centre: [f32; 3],
    pub pad1: f32,
}

impl StyleUniform {
    /// `style` for an atlas of `atlas_size` texels, drawn in `view`.
    pub fn new(style: &IconStyle, atlas_size: [u32; 2], view: &ViewFrame) -> StyleUniform {
        StyleUniform { focal_w: focal_w(view) as f32, centre: view.common_centre.as_vec3().into(), ..Self::props(style, atlas_size) }
    }

    /// `style`'s own terms, with deck's focal w (1.5) and a centre at the origin for the view's.
    fn props(style: &IconStyle, atlas_size: [u32; 2]) -> StyleUniform {
        StyleUniform {
            size_scale: style.size_scale as f32,
            size_basis: match style.size_basis {
                SizeBasis::Width => 0.0,
                SizeBasis::Height => 1.0,
            },
            size_min_pixels: style.size_min_pixels as f32,
            size_max_pixels: style.size_max_pixels as f32,
            atlas_size: atlas_size.map(|s| s as f32),
            billboard: u32::from(style.billboard),
            size_units: style.size_units as u32,
            alpha_cutoff: style.alpha_cutoff as f32,
            opacity: style.opacity as f32,
            focal_w: rustradar_scene::camera::CAMERA_HEIGHTS as f32,
            pad0: 0.0,
            centre: [0.0; 3],
            pad1: 0.0,
        }
    }
}

/// The icon pipeline for one draw-state preset. Every icon layer of the app culls nothing
/// (`NO_CULL`, `SHIP`, `TRAIN`): deck's quads wind clockwise on the screen, so a culling preset
/// would drop them all, and [`Self::new`] refuses one.
pub struct IconPipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    corners: wgpu::Buffer,
}

impl IconPipeline {
    /// The pipeline with `state` for the map pass of `init`.
    ///
    /// # Panics
    /// If `state` culls faces.
    pub fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        assert!(state.cull.is_none(), "{label}: icon quads wind clockwise, so they need a preset that culls nothing");
        let device = &init.gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("icon batch"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(size_of::<StyleUniform>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let module = shaders::module(device, label, SHADER);
        let pipeline = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry: "vs",
                fragment_entry: "fs",
                buffers: &[Some(CORNER_LAYOUT), Some(Instance::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                state,
                groups: &[&layout],
            },
        );
        // the icon manager's DEFAULT_SAMPLER_PARAMETERS (`icon-layer/icon-manager.js`)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("icon atlas"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let corners = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("icon corners"),
                contents: bytemuck::cast_slice(&CORNERS),
                usage: wgpu::BufferUsages::VERTEX,
            },
        );
        IconPipeline { pipeline, layout, sampler, corners }
    }

    /// A batch drawing from `atlas` (`atlas_size` texels at mip level 0), with no icons yet and
    /// [`IconStyle::DEFAULT`] until [`Batch::set_style`].
    pub fn batch(&self, device: &wgpu::Device, label: &str, atlas: &wgpu::TextureView, atlas_size: [u32; 2]) -> Batch {
        let style = StyleUniform::props(&IconStyle::DEFAULT, atlas_size);
        let uniform = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&style),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let group = self.group(device, label, &uniform, atlas);
        Batch {
            label: label.to_string(),
            uniform,
            group,
            atlas_size,
            written: style,
            instances: Batch::allocate(device, label, 1),
            count: 0,
        }
    }

    fn group(&self, device: &wgpu::Device, label: &str, uniform: &wgpu::Buffer, atlas: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(atlas) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    /// Sets the pipeline and the corners; then [`Self::draw_batch`] as often as needed.
    pub fn begin(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.corners.slice(..));
    }

    /// Draws `batch`'s icons, after [`Self::begin`]. Icons read no local Mercator frame (group 1
    /// stays as the pass left it).
    pub fn draw_batch(&self, pass: &mut wgpu::RenderPass<'_>, batch: &Batch) {
        if batch.is_empty() {
            return;
        }
        pass.set_bind_group(BATCH_GROUP, &batch.group, &[]);
        pass.set_vertex_buffer(1, batch.instances.slice(..));
        pass.draw(0..CORNERS.len() as u32, 0..batch.count);
    }

    /// [`Self::begin`] and [`Self::draw_batch`].
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, batch: &Batch) {
        self.begin(pass);
        self.draw_batch(pass, batch);
    }
}

/// What one deck IconLayer draws: an atlas, the layer's [`StyleUniform`] and its instances.
pub struct Batch {
    label: String,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    atlas_size: [u32; 2],
    written: StyleUniform,
    instances: wgpu::Buffer,
    count: u32,
}

impl Batch {
    fn allocate(device: &wgpu::Device, label: &str, count: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (count.max(1) * size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Draws from `atlas` (`atlas_size` texels) from now on: a loaded texture replacing a stand-in.
    pub fn set_atlas(&mut self, pipeline: &IconPipeline, device: &wgpu::Device, atlas: &wgpu::TextureView, atlas_size: [u32; 2]) {
        self.group = pipeline.group(device, &self.label, &self.uniform, atlas);
        self.atlas_size = atlas_size;
    }

    /// The layer's props for this frame's `view` (the focal depth and, on the globe, the centre a
    /// ground-plane icon's up is worked out from), written when they changed. Call it in every
    /// `prepare`.
    pub fn set_style(&mut self, queue: &wgpu::Queue, style: &IconStyle, view: &ViewFrame) {
        let uniform = StyleUniform::new(style, self.atlas_size, view);
        if uniform != self.written {
            queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
            self.written = uniform;
        }
    }

    /// The uniform last written.
    pub fn style(&self) -> &StyleUniform {
        &self.written
    }

    /// Replaces its icons (at each scene refresh), growing the buffer when they do not fit.
    pub fn set_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, instances: &[Instance]) {
        let bytes: &[u8] = bytemuck::cast_slice(instances);
        if bytes.len() as u64 > self.instances.size() {
            self.instances = Self::allocate(device, &self.label, instances.len().next_power_of_two());
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.instances, 0, bytes);
        }
        self.count = instances.len() as u32;
    }

    /// How many icons it draws.
    pub fn len(&self) -> u32 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path as FilePath;
    use std::rc::Rc;

    use glam::{DVec2, DVec3};
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{Viewport, ViewState};

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{DrawCx, Layer, PrepareCx};
    use crate::pipelines::bitmap::upload_rgba8;
    use crate::renderer::{MapRenderer, SceneInput};

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const INK: f64 = 30.0;
    const STOCKHOLM: [f64; 2] = [18.07, 59.33];
    const SIZE: [f64; 2] = [240.0, 240.0];

    // The test atlas: 128 × 64 texels, each cell 16 × 16 (the wide one 32 × 16) inside a block of its
    // own colour 2 texels wider each way, so a quad's edge samples the cell's colour and its coverage
    // is the quad's.
    const ATLAS: [u32; 2] = [128, 64];
    /// Red over blue: the top half red, the bottom half blue (both opaque).
    const UP: IconDef = IconDef::cell(2.0, 2.0, 16.0, 16.0, false);
    /// Opaque white.
    const BLOCK: IconDef = IconDef::cell(22.0, 2.0, 16.0, 16.0, false);
    /// White at alpha 10 (0.039, under deck's cutoff).
    const FAINT: IconDef = IconDef::cell(42.0, 2.0, 16.0, 16.0, false);
    /// White at alpha 20 (0.078, over it).
    const LIGHT: IconDef = IconDef::cell(62.0, 2.0, 16.0, 16.0, false);
    /// Opaque `(200, 100, 50)`.
    const ORANGE: IconDef = IconDef::cell(82.0, 2.0, 16.0, 16.0, false);
    /// Opaque white, 32 × 16.
    const WIDE: IconDef = IconDef::cell(2.0, 22.0, 32.0, 16.0, false);

    fn atlas_texels() -> Vec<u8> {
        let mut rgba = vec![0u8; (ATLAS[0] * ATLAS[1] * 4) as usize];
        let mut paint = |x0: u32, y0: u32, w: u32, h: u32, c: [u8; 4]| {
            for y in y0..y0 + h {
                for x in x0..x0 + w {
                    let i = ((y * ATLAS[0] + x) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&c);
                }
            }
        };
        paint(0, 0, 20, 10, [255, 0, 0, 255]);
        paint(0, 10, 20, 10, [0, 0, 255, 255]);
        paint(20, 0, 20, 20, WHITE);
        paint(40, 0, 20, 20, [255, 255, 255, 10]);
        paint(60, 0, 20, 20, [255, 255, 255, 20]);
        paint(80, 0, 20, 20, [200, 100, 50, 255]);
        paint(0, 20, 36, 20, WHITE);
        rgba
    }

    fn view([longitude, latitude]: [f64; 2], zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, pitch, bearing, position: None }
    }

    /// One icon to draw, placed in each frame's view.
    #[derive(Debug, Clone, Copy)]
    struct Spec {
        at: [f64; 3],
        icon: IconDef,
        size: f64,
        angle: f64,
        color: [u8; 4],
        highlight: [u8; 4],
        pixel_offset: [f64; 2],
    }

    impl Spec {
        fn new([lon, lat]: [f64; 2], icon: IconDef, size: f64) -> Spec {
            Spec { at: [lon, lat, 0.0], icon, size, angle: 0.0, color: WHITE, highlight: Instance::NO_HIGHLIGHT, pixel_offset: [0.0; 2] }
        }

        fn instance(&self, view: &ViewFrame) -> Instance {
            Instance::at(view, self.at, &self.icon, self.size, self.angle, self.color)
                .with_pixel_offset(self.pixel_offset)
                .with_highlight(self.highlight)
        }
    }

    /// One batch: a style and its icons.
    #[derive(Debug, Clone)]
    struct Draw {
        style: IconStyle,
        icons: Vec<Spec>,
    }

    /// A test layer: the draws in order, one batch each, on the test atlas.
    struct Icons {
        pipeline: IconPipeline,
        atlas: wgpu::TextureView,
        batches: Vec<Batch>,
        draws: Rc<RefCell<Vec<Draw>>>,
        drawn: usize,
    }

    impl Layer for Icons {
        fn name(&self) -> &'static str {
            "test icons"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            let draws = self.draws.borrow();
            while self.batches.len() < draws.len() {
                self.batches.push(self.pipeline.batch(&cx.gpu.device, "test icons", &self.atlas, ATLAS));
            }
            for (batch, draw) in self.batches.iter_mut().zip(draws.iter()) {
                batch.set_style(&cx.gpu.queue, &draw.style, cx.view);
                let instances: Vec<Instance> = draw.icons.iter().map(|s| s.instance(cx.view)).collect();
                batch.set_instances(&cx.gpu.device, &cx.gpu.queue, &instances);
            }
            self.drawn = draws.len();
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
            self.pipeline.begin(pass);
            for batch in &self.batches[..self.drawn] {
                self.pipeline.draw_batch(pass, batch);
            }
        }
    }

    /// A renderer with the test layer, re-used across shots.
    struct Bench {
        renderer: MapRenderer,
        draws: Rc<RefCell<Vec<Draw>>>,
    }

    impl Bench {
        fn new(gpu: &Gpu) -> Bench {
            let mut renderer = MapRenderer::new(gpu, FORMAT);
            let init = renderer.init(gpu);
            let pipeline = IconPipeline::new(&init, "test icons", DrawState::NO_CULL);
            let texture = upload_rgba8(gpu, "test atlas", ATLAS, &atlas_texels(), false);
            let atlas = texture.create_view(&Default::default());
            let draws = Rc::new(RefCell::new(Vec::new()));
            renderer.add_layer(Box::new(Icons { pipeline, atlas, batches: Vec::new(), draws: draws.clone(), drawn: 0 }));
            Bench { renderer, draws }
        }

        /// `draws` over the empty dark map in `view` at `scale_factor`; the image and the frame.
        fn shoot(&mut self, gpu: &Gpu, view: ViewState, scale_factor: f64, draws: Vec<Draw>) -> (Image, ViewFrame) {
            *self.draws.borrow_mut() = draws;
            let scene = SceneInput::new(view, SIZE, scale_factor, Basemap::Dark);
            let image = render_with(gpu, &mut self.renderer, &scene, scale_factor).expect("rendered");
            (image, self.renderer.view().clone())
        }
    }

    fn one(style: IconStyle, spec: Spec) -> Vec<Draw> {
        vec![Draw { style, icons: vec![spec] }]
    }

    fn save(image: &Image, name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots");
        std::fs::create_dir_all(dir).expect("shot folder");
        image.save_png(&FilePath::new(dir).join(name)).expect("png");
    }

    /// East and north at `[lon, lat]` in common space (unit length on the globe).
    fn east_north(vp: &Viewport, [lon, lat]: [f64; 2]) -> (DVec3, DVec3) {
        match vp {
            Viewport::Globe(_) => {
                let (sl, cl) = lon.to_radians().sin_cos();
                let (sp, cp) = lat.to_radians().sin_cos();
                (DVec3::new(cl, sl, 0.0), DVec3::new(-sl * sp, cl * sp, cp))
            }
            Viewport::Mercator(_) => (DVec3::X, DVec3::Y),
        }
    }

    /// The f64 model of `icon.wgsl`'s vertex shader: where the point `(qx, qy)` of the quad (−1 to
    /// 1, x right and y down across the cell) lands, in CSS pixels (y down).
    fn model_point(vf: &ViewFrame, style: &IconStyle, s: &Spec, [qx, qy]: [f64; 2]) -> DVec2 {
        let vp = &vf.viewport;
        let size_pixels = style.size_pixels(s.size, vf);
        let [ox, oy, _, _, width, height, _] = s.icon.instance_def();
        let constraint = if style.size_basis == SizeBasis::Width { width } else { height };
        let scale = if constraint == 0.0 { 0.0 } else { size_pixels / constraint };
        let p = DVec2::new(qx / 2.0 * width + ox, qy / 2.0 * height + oy);
        let (sin, cos) = s.angle.to_radians().sin_cos();
        let p = DVec2::new(cos * p.x + sin * p.y, -sin * p.x + cos * p.y) * scale + DVec2::from_array(s.pixel_offset);
        let p = DVec2::new(p.x, -p.y);
        let common = vp.project_position(s.at);
        if style.billboard {
            let w = (vp.view_projection_matrix * common.extend(1.0)).w;
            let k = focal_w(vf) / w;
            let [x, y, _] = vp.world_to_pixels(common);
            DVec2::new(x + p.x * k, y - p.y * k)
        } else {
            let (e, n) = east_north(vp, [s.at[0], s.at[1]]);
            let [x, y, _] = vp.world_to_pixels(common + (e * p.x + n * p.y) / vp.scale);
            DVec2::new(x, y)
        }
    }

    /// The quad's corners in the model, in order round it, and their area (shoelace) and bounds.
    fn model_quad(vf: &ViewFrame, style: &IconStyle, s: &Spec) -> ([DVec2; 4], f64, DVec2, DVec2) {
        let corners = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]].map(|q| model_point(vf, style, s, q));
        let area = (0..4).map(|i| corners[i].perp_dot(corners[(i + 1) % 4])).sum::<f64>().abs() / 2.0;
        let min = corners.iter().fold(DVec2::INFINITY, |m, c| m.min(*c));
        let max = corners.iter().fold(DVec2::NEG_INFINITY, |m, c| m.max(*c));
        (corners, area, min, max)
    }

    /// A weighted region of an image: total weight (coverage, px²), centroid and the bounds of the
    /// pixels weighing over a half, in CSS pixels of a `scale`-factor image.
    #[derive(Debug, Clone, Copy)]
    struct Blob {
        area: f64,
        centroid: DVec2,
        min: DVec2,
        max: DVec2,
    }

    fn blob(image: &Image, scale: f64, x_range: std::ops::Range<u32>, weight: impl Fn([u8; 4]) -> f64) -> Blob {
        let (mut area, mut sum) = (0.0, DVec2::ZERO);
        let (mut min, mut max) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
        for y in 0..image.height {
            for x in x_range.clone() {
                let w = weight(image.pixel(x, y)).clamp(0.0, 1.0);
                let centre = DVec2::new(f64::from(x) + 0.5, f64::from(y) + 0.5) / scale;
                area += w;
                sum += centre * w;
                if w > 0.5 {
                    min = min.min(centre - 0.5 / scale);
                    max = max.max(centre + 0.5 / scale);
                }
            }
        }
        Blob { area: area / (scale * scale), centroid: sum / area, min, max }
    }

    /// Coverage over the ink of anything with no green: the red and blue of [`UP`].
    fn red_or_blue(p: [u8; 4]) -> f64 {
        1.0 - f64::from(p[1]) / INK
    }

    fn red(p: [u8; 4]) -> f64 {
        if p[2] < 100 { (f64::from(p[0]) - INK) / (255.0 - INK) } else { 0.0 }
    }

    fn blue(p: [u8; 4]) -> f64 {
        if p[0] < 100 { (f64::from(p[2]) - INK) / (255.0 - INK) } else { 0.0 }
    }

    fn white(p: [u8; 4]) -> f64 {
        (f64::from(p[0]) - INK) / (255.0 - INK)
    }

    fn all_columns(image: &Image) -> std::ops::Range<u32> {
        0..image.width
    }

    /// Item E8's acceptance: a billboard icon stays upright and the same size at pitch 0 and 60
    /// (and with the map turned), and a ground-plane icon foreshortens, on the globe (zoom 5) and
    /// the flat map (zoom 14). The two icons sit 60 px west and east of the view centre on its
    /// screen row, at the view centre's depth, where deck's pixel sizes are exact; each is measured
    /// in its half of the shot against the f64 model.
    #[test]
    fn billboard_icons_stay_upright_and_the_same_size_and_ground_icons_foreshorten() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        const PX: f64 = 48.0;
        for zoom in [5.0, 14.0] {
            let mut level_area = None;
            for (pitch, bearing) in [(0.0, 0.0), (60.0, 0.0), (60.0, 30.0)] {
                let v = view(STOCKHOLM, zoom, pitch, bearing);
                let vp = rustradar_scene::camera::viewport_for(&v, SIZE);
                let west = vp.unproject([SIZE[0] / 2.0 - 60.0, SIZE[1] / 2.0], 0.0);
                let east = vp.unproject([SIZE[0] / 2.0 + 60.0, SIZE[1] / 2.0], 0.0);
                let billboard = Spec::new(west, UP, PX);
                let ground = Spec::new(east, UP, PX);
                let draws = vec![
                    Draw { style: IconStyle::DEFAULT, icons: vec![billboard] },
                    Draw { style: IconStyle::GROUND, icons: vec![ground] },
                ];
                let (image, vf) = bench.shoot(&gpu, v, 1.0, draws);
                save(&image, &format!("e8-icons-zoom{zoom}-pitch{pitch}-bearing{bearing}.png"));
                let about = format!("zoom {zoom} pitch {pitch} bearing {bearing}");
                let half = image.width / 2;

                // the billboard: square, its size, red over blue straight up the screen
                let b = blob(&image, 1.0, 0..half, red_or_blue);
                let (_, area, min, max) = model_quad(&vf, &IconStyle::DEFAULT, &billboard);
                eprintln!("{about}: billboard area {:.1} (model {area:.1}), bounds {:?}–{:?} (model {min:?}–{max:?})", b.area, b.min, b.max);
                assert!((b.area / (PX * PX) - 1.0).abs() < 0.02, "{about}: billboard area {}", b.area);
                assert!((area / (PX * PX) - 1.0).abs() < 0.005, "{about}: model area {area}");
                let extent = b.max - b.min;
                assert!((extent.x - PX).abs() <= 1.0 && (extent.y - PX).abs() <= 1.0, "{about}: billboard extent {extent:?}");
                assert!((b.min - min).abs().max_element() <= 1.0, "{about}: billboard at {:?}, model {min:?}", b.min);
                let (r, bl) = (blob(&image, 1.0, 0..half, red), blob(&image, 1.0, 0..half, blue));
                let up = r.centroid - bl.centroid;
                assert!(up.x.abs() < 0.5 && (up.y + PX / 2.0).abs() < 1.5, "{about}: billboard red minus blue {up:?}");
                match level_area {
                    None => level_area = Some(b.area),
                    Some(a) => assert!((b.area / a - 1.0).abs() < 0.01, "{about}: billboard area {} against {a} at pitch 0", b.area),
                }

                // the ground-plane icon: the model's trapezoid, north (red) up the projected north
                let g = blob(&image, 1.0, half..image.width, red_or_blue);
                let (_, area, min, max) = model_quad(&vf, &IconStyle::GROUND, &ground);
                eprintln!("{about}: ground area {:.1} (model {area:.1}), bounds {:?}–{:?} (model {min:?}–{max:?})", g.area, g.min, g.max);
                assert!((g.area / area - 1.0).abs() < 0.03, "{about}: ground area {} against {area}", g.area);
                assert!((g.min - min).abs().max_element() <= 1.5 && (g.max - max).abs().max_element() <= 1.5, "{about}: ground bounds");
                let (r, bl) = (blob(&image, 1.0, half..image.width, red), blob(&image, 1.0, half..image.width, blue));
                let measured = (r.centroid - bl.centroid).normalize();
                let model = (model_point(&vf, &IconStyle::GROUND, &ground, [0.0, -0.5]) - model_point(&vf, &IconStyle::GROUND, &ground, [0.0, 0.5])).normalize();
                assert!(measured.angle_to(model).abs().to_degrees() < 3.0, "{about}: ground north {measured:?}, model {model:?}");
                if pitch == 0.0 {
                    // flat on the screen it is the billboard's square, turned with the map
                    assert!((g.area / (PX * PX) - 1.0).abs() < 0.02, "{about}: ground area {}", g.area);
                } else if bearing == 0.0 {
                    // pitched 60°, about cos 60° as tall as the billboard (a trapezoid, its near
                    // edge wider than its far one; off the axis both shift sideways, so its bounds
                    // are wider than either), and so about half its area
                    let extent = g.max - g.min;
                    assert!(extent.y / PX < 0.6 && extent.y / PX > 0.4, "{about}: ground extent {extent:?}");
                    assert!(g.area / b.area < 0.6 && g.area / b.area > 0.4, "{about}: ground area {} against {}", g.area, b.area);
                }
            }
        }
    }

    /// deck's angles turn anticlockwise on the screen; a billboard's top stays up the screen when
    /// the map turns, a ground-plane icon's top points north (true north on the globe too: plan
    /// deviation 1), whatever the bearing.
    #[test]
    fn angles_turn_anticlockwise_and_ground_icons_point_north() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        for zoom in [6.0, 15.0] {
            for bearing in [0.0, 90.0] {
                for billboard in [true, false] {
                    for angle in [0.0, 90.0, -135.0] {
                        let style = if billboard { IconStyle::DEFAULT } else { IconStyle::GROUND };
                        let v = view(STOCKHOLM, zoom, 0.0, bearing);
                        let spec = Spec { angle, ..Spec::new(STOCKHOLM, UP, 40.0) };
                        let (image, vf) = bench.shoot(&gpu, v, 1.0, one(style, spec));
                        let about = format!("zoom {zoom} bearing {bearing} billboard {billboard} angle {angle}");
                        let (r, b) = (blob(&image, 1.0, all_columns(&image), red), blob(&image, 1.0, all_columns(&image), blue));
                        let measured = (r.centroid - b.centroid).normalize();
                        // the icon's top, from its bottom, on the screen (y down): billboards turn
                        // from up the screen, ground-plane icons from north, anticlockwise
                        let from = if billboard { 0.0 } else { bearing };
                        let a = (angle + from).to_radians();
                        let expected = DVec2::new(-a.sin(), -a.cos());
                        assert!(measured.angle_to(expected).abs().to_degrees() < 2.0, "{about}: top toward {measured:?}, expected {expected:?}");
                        let model = (model_point(&vf, &style, &spec, [0.0, -0.5]) - model_point(&vf, &style, &spec, [0.0, 0.5])).normalize();
                        assert!(model.angle_to(expected).abs().to_degrees() < 0.5, "{about}: model {model:?}");
                    }
                }
            }
        }
    }

    /// Off the view centre's depth, billboards scale by `focal_w / w` as deck's do: in a view pitched
    /// 60°, an icon near the bottom of the screen draws larger than its size and one near the top
    /// smaller, each as the model says; ground-plane icons there follow the model too.
    #[test]
    fn off_centre_icons_scale_with_depth_as_decks_do() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        for zoom in [8.0, 14.0] {
            let v = view(STOCKHOLM, zoom, 60.0, 0.0);
            let vp = rustradar_scene::camera::viewport_for(&v, SIZE);
            for (y, grows) in [(200.0, true), (50.0, false)] {
                for style in [IconStyle::DEFAULT, IconStyle::GROUND] {
                    let at = vp.unproject([SIZE[0] / 2.0, y], 0.0);
                    let spec = Spec::new(at, BLOCK, 30.0);
                    let (image, vf) = bench.shoot(&gpu, v, 1.0, one(style, spec));
                    let about = format!("zoom {zoom} y {y} billboard {}", style.billboard);
                    let m = blob(&image, 1.0, all_columns(&image), white);
                    let (_, area, min, max) = model_quad(&vf, &style, &spec);
                    eprintln!("{about}: area {:.1} (model {area:.1}), bounds {:?}–{:?} (model {min:?}–{max:?})", m.area, m.min, m.max);
                    assert!((m.area / area - 1.0).abs() < 0.03, "{about}: area {} against {area}", m.area);
                    assert!((m.min - min).abs().max_element() <= 1.5 && (m.max - max).abs().max_element() <= 1.5, "{about}: bounds");
                    if style.billboard {
                        assert_eq!(area > 30.0 * 30.0 * 1.1, grows, "{about}: model area {area}");
                        assert_eq!(area < 30.0 * 30.0 / 1.1, !grows, "{about}: model area {area}");
                    }
                }
            }
        }
    }

    /// Sizes as deck works them out: pixels, metres at the view centre's scale, the min/max clamp,
    /// the width basis, the size scale, the anchor and the pixel offset; and a map scale of 2 draws
    /// twice as many map pixels.
    #[test]
    fn sizes_follow_units_clamps_basis_anchor_and_offset() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        for zoom in [7.0, 14.0] {
            let v = view(STOCKHOLM, zoom, 0.0, 0.0);
            let vf = ViewFrame::new(&v, SIZE, 1.0, 0.0);
            let mpp = vf.viewport.meters_per_pixel();
            let metres = IconStyle { size_units: Units::Meters, ..IconStyle::DEFAULT };
            let cases: [(IconStyle, Spec, [f64; 2]); 7] = [
                (IconStyle::DEFAULT, Spec::new(STOCKHOLM, BLOCK, 30.0), [30.0, 30.0]),
                (metres, Spec::new(STOCKHOLM, BLOCK, 36.0 * mpp), [36.0, 36.0]),
                (IconStyle { size_min_pixels: 20.0, ..IconStyle::DEFAULT }, Spec::new(STOCKHOLM, BLOCK, 5.0), [20.0, 20.0]),
                (IconStyle { size_max_pixels: 25.0, ..IconStyle::DEFAULT }, Spec::new(STOCKHOLM, BLOCK, 100.0), [25.0, 25.0]),
                (IconStyle { size_basis: SizeBasis::Width, ..IconStyle::DEFAULT }, Spec::new(STOCKHOLM, WIDE, 40.0), [40.0, 20.0]),
                (IconStyle::DEFAULT, Spec::new(STOCKHOLM, WIDE, 20.0), [40.0, 20.0]),
                (IconStyle { size_scale: 1.5, ..IconStyle::DEFAULT }, Spec::new(STOCKHOLM, BLOCK, 20.0), [30.0, 30.0]),
            ];
            for (i, (style, spec, [w, h])) in cases.into_iter().enumerate() {
                let (image, vf) = bench.shoot(&gpu, v, 1.0, one(style, spec));
                let m = blob(&image, 1.0, all_columns(&image), white);
                let about = format!("zoom {zoom} case {i}");
                let extent = m.max - m.min;
                assert!((m.area / (w * h) - 1.0).abs() < 0.03, "{about}: area {} against {}", m.area, w * h);
                assert!((extent.x - w).abs() <= 1.0 && (extent.y - h).abs() <= 1.0, "{about}: extent {extent:?}");
                let (_, area, ..) = model_quad(&vf, &style, &spec);
                assert!((area / (w * h) - 1.0).abs() < 0.005, "{about}: model area {area}");
            }
            // anchored at its bottom middle, the icon stands on its position; a pixel offset moves it
            // right and down after that
            let centre = DVec2::new(SIZE[0], SIZE[1]) / 2.0;
            let anchored = Spec::new(STOCKHOLM, BLOCK.anchored(8.0, 16.0), 30.0);
            for (spec, shift) in [(anchored, DVec2::ZERO), (Spec { pixel_offset: [10.0, 5.0], ..anchored }, DVec2::new(10.0, 5.0))] {
                let (image, _) = bench.shoot(&gpu, v, 1.0, one(IconStyle::DEFAULT, spec));
                let m = blob(&image, 1.0, all_columns(&image), white);
                let (min, max) = (centre + shift + DVec2::new(-15.0, -30.0), centre + shift + DVec2::new(15.0, 0.0));
                assert!((m.min - min).abs().max_element() <= 1.0 && (m.max - max).abs().max_element() <= 1.0, "zoom {zoom}: {m:?} against {min}–{max}");
            }
            // a missing icon draws nothing
            let (image, _) = bench.shoot(&gpu, v, 1.0, one(IconStyle::DEFAULT, Spec::new(STOCKHOLM, IconDef::MISSING, 30.0)));
            assert!(image.rgba.chunks_exact(4).all(|p| p == [30, 30, 30, 255]), "zoom {zoom}: the missing icon drew");
            // at map scale 2 the same icon covers four times the map pixels, the same CSS pixels
            let (image, _) = bench.shoot(&gpu, v, 2.0, one(IconStyle::DEFAULT, Spec::new(STOCKHOLM, BLOCK, 30.0)));
            let m = blob(&image, 2.0, all_columns(&image), white);
            assert!((m.area / 900.0 - 1.0).abs() < 0.02, "zoom {zoom}: area {} at scale 2", m.area);
            assert!((m.max - m.min - DVec2::splat(30.0)).abs().max_element() <= 0.5, "zoom {zoom}: {m:?} at scale 2");
        }
    }

    /// The fragment as deck's: atlas colours, or the instance colour in mask mode, alpha times the
    /// opacity and the instance alpha, the cutoff at 0.05 (opacity included), the highlight blend.
    #[test]
    fn colours_masks_opacity_cutoff_and_highlight_are_decks() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        let v = view(STOCKHOLM, 10.0, 0.0, 0.0);
        let middle = |image: &Image| image.pixel(image.width / 2, image.height / 2);
        let over_ink = |rgb: [f64; 3], a: f64| rgb.map(|c| (INK + (c - INK) * a).round() as u8);
        let close = |got: [u8; 4], want: [u8; 3], about: &str| {
            let off = (0..3).map(|i| (i32::from(got[i]) - i32::from(want[i])).abs()).max().unwrap_or(0);
            assert!(off <= 1, "{about}: {got:?}, expected {want:?}");
        };
        let mask = IconDef { mask: true, ..ORANGE };
        let tint = [10, 20, 240, 128];
        let a = 128.0 / 255.0;
        let cases: [(&str, IconStyle, Spec, [u8; 3]); 8] = [
            ("coloured icon", IconStyle::DEFAULT, Spec { color: tint, ..Spec::new(STOCKHOLM, ORANGE, 30.0) }, over_ink([200.0, 100.0, 50.0], a)),
            ("mask icon", IconStyle::DEFAULT, Spec { color: tint, ..Spec::new(STOCKHOLM, mask, 30.0) }, over_ink([10.0, 20.0, 240.0], a)),
            ("opacity", IconStyle { opacity: 0.5, ..IconStyle::DEFAULT }, Spec::new(STOCKHOLM, ORANGE, 30.0), over_ink([200.0, 100.0, 50.0], 0.5)),
            ("under the cutoff", IconStyle::DEFAULT, Spec::new(STOCKHOLM, FAINT, 30.0), [30, 30, 30]),
            ("over the cutoff", IconStyle::DEFAULT, Spec::new(STOCKHOLM, LIGHT, 30.0), over_ink([255.0; 3], 20.0 / 255.0)),
            ("opacity takes it under", IconStyle { opacity: 0.5, ..IconStyle::DEFAULT }, Spec::new(STOCKHOLM, LIGHT, 30.0), [30, 30, 30]),
            ("no cutoff", IconStyle { alpha_cutoff: 0.0, ..IconStyle::DEFAULT }, Spec::new(STOCKHOLM, FAINT, 30.0), over_ink([255.0; 3], 10.0 / 255.0)),
            // the aircraft icons' hover: white, highlight [55,142,240,150]
            (
                "highlight",
                IconStyle::DEFAULT,
                Spec { highlight: [55, 142, 240, 150], ..Spec::new(STOCKHOLM, BLOCK, 30.0) },
                {
                    let ha = 150.0 / 255.0;
                    let ratio = ha / (ha + (1.0 - ha));
                    [55.0f64, 142.0, 240.0].map(|c| (255.0 + (c - 255.0) * ratio).round() as u8)
                },
            ),
        ];
        for (about, style, spec, want) in cases {
            let (image, _) = bench.shoot(&gpu, v, 1.0, one(style, spec));
            close(middle(&image), want, about);
            if want == [30, 30, 30] {
                assert!(image.rgba.chunks_exact(4).all(|p| p == [30, 30, 30, 255]), "{about}: a fragment under the cutoff drew");
            }
        }
        // a translucent highlight over a translucent icon: alpha a_h + a (1 − a_h)
        let spec = Spec { color: [255, 255, 255, 128], highlight: [55, 142, 240, 150], ..Spec::new(STOCKHOLM, BLOCK, 30.0) };
        let (image, _) = bench.shoot(&gpu, v, 1.0, one(IconStyle::DEFAULT, spec));
        let ha = 150.0 / 255.0;
        let blended = ha + a * (1.0 - ha);
        let rgb = [55.0, 142.0, 240.0].map(|c| 255.0 + (c - 255.0) * ha / blended);
        close(middle(&image), over_ink(rgb, blended), "translucent highlight");
    }

    /// Icons write depth with `NO_CULL` (the aircraft's preset): of two at the same place the one
    /// drawn first keeps the pixels where the second is no nearer, and the globe's far side hides
    /// none of them at the view centre. Cut-off fragments leave the depth alone.
    #[test]
    fn cut_off_fragments_write_no_depth() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        let v = view(STOCKHOLM, 10.0, 0.0, 0.0);
        let higher = |icon: IconDef, h: f64| Spec { at: [STOCKHOLM[0], STOCKHOLM[1], h], ..Spec::new(STOCKHOLM, icon, 30.0) };
        // a nearer faint icon (discarded) does not hide the orange one behind it
        let draws = vec![
            Draw { style: IconStyle::DEFAULT, icons: vec![higher(FAINT, 1000.0)] },
            Draw { style: IconStyle::DEFAULT, icons: vec![higher(ORANGE, 0.0)] },
        ];
        let (image, _) = bench.shoot(&gpu, v, 1.0, draws);
        assert_eq!(image.pixel(image.width / 2, image.height / 2), [200, 100, 50, 255]);
        // a nearer opaque one does
        let draws = vec![
            Draw { style: IconStyle::DEFAULT, icons: vec![higher(BLOCK, 1000.0)] },
            Draw { style: IconStyle::DEFAULT, icons: vec![higher(ORANGE, 0.0)] },
        ];
        let (image, _) = bench.shoot(&gpu, v, 1.0, draws);
        assert_eq!(image.pixel(image.width / 2, image.height / 2), [255, 255, 255, 255]);
    }

    #[test]
    fn instance_defs_are_decks() {
        // getInstanceIconDef: anchors default to the middle
        assert_eq!(IconDef::cell(128.0, 256.0, 128.0, 64.0, true).instance_def(), [0.0, 0.0, 128.0, 256.0, 128.0, 64.0, 1.0]);
        assert_eq!(IconDef::cell(0.0, 0.0, 40.0, 20.0, false).anchored(10.0, 20.0).instance_def(), [10.0, -10.0, 0.0, 0.0, 40.0, 20.0, 0.0]);
        assert_eq!(IconDef::MISSING.instance_def(), [0.0; 7]);
        let vf = ViewFrame::new(&view(STOCKHOLM, 10.0, 30.0, 10.0), SIZE, 1.0, 0.0);
        let i = Instance::at(&vf, [18.1, 59.3, 120.0], &BLOCK.anchored(0.0, 0.0), 24.0, 45.0, [1, 2, 3, 4])
            .with_pixel_offset([3.0, -2.0])
            .with_highlight([5, 6, 7, 8]);
        assert_eq!(i.position, <[f32; 3]>::from(vf.common_offset([18.1, 59.3], 120.0)));
        assert_eq!((i.size, i.frame, i.offset, i.pixel_offset), (24.0, [22.0, 2.0, 16.0, 16.0], [8.0, 8.0], [3.0, -2.0]));
        assert_eq!((i.angle, i.color_mode, i.color, i.highlight), (45.0, 0.0, [1, 2, 3, 4], [5, 6, 7, 8]));
    }

    /// deck's `focalDistance` uniform is the clip w of the view centre: 1.5 on both viewports, and
    /// what a common offset of zero projects to in a frame of its own.
    #[test]
    fn focal_w_is_the_clip_w_of_the_view_centre() {
        for v in [view(STOCKHOLM, 2.0, 0.0, 0.0), view(STOCKHOLM, 9.0, 60.0, 30.0), view([-73.9, 40.7], 16.0, 45.0, -20.0)] {
            let vf = ViewFrame::new(&v, [800.0, 600.0], 1.0, 0.0);
            let w = focal_w(&vf);
            assert!((w - rustradar_scene::camera::CAMERA_HEIGHTS).abs() < 1e-12, "{v:?}: {w}");
            let clip = glam::Mat4::from_cols_array_2d(&vf.globals.view_proj) * glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
            assert!((f64::from(clip.w) - w).abs() < 1e-5, "{v:?}: {clip}");
            let u = StyleUniform::new(&IconStyle::DEFAULT, [64, 32], &vf);
            assert_eq!((u.focal_w, u.centre, u.atlas_size), (w as f32, <[f32; 3]>::from(vf.common_centre.as_vec3()), [64.0, 32.0]));
        }
        // `size_pixels`: metres and common units at the view centre's scale, then the clamp
        let vf = ViewFrame::new(&view(STOCKHOLM, 14.0, 0.0, 0.0), SIZE, 1.0, 0.0);
        let mpp = vf.viewport.meters_per_pixel();
        let metres = IconStyle { size_units: Units::Meters, size_max_pixels: 50.0, ..IconStyle::DEFAULT };
        assert!((metres.size_pixels(20.0 * mpp, &vf) - 20.0).abs() < 1e-9);
        assert_eq!(metres.size_pixels(80.0 * mpp, &vf), 50.0);
        let common = IconStyle { size_units: Units::Common, ..IconStyle::DEFAULT };
        assert!((common.size_pixels(1.0, &vf) - 2f64.powi(14)).abs() < 1e-6);
    }

    #[test]
    fn layouts_match_the_wgsl() {
        let module = crate::shaders::tests::validate("icon.wgsl", &crate::shaders::expand(SHADER).expect("libraries known"));
        let (span, members) = module
            .types
            .iter()
            .find_map(|(_, t)| match &t.inner {
                wgpu::naga::TypeInner::Struct { members, span } if t.name.as_deref() == Some("IconStyle") => {
                    Some((*span, members.iter().map(|m| (m.name.clone().expect("named"), m.offset)).collect::<Vec<_>>()))
                }
                _ => None,
            })
            .expect("struct IconStyle");
        assert_eq!(span as usize, size_of::<StyleUniform>());
        let expected = [
            ("size_scale", std::mem::offset_of!(StyleUniform, size_scale)),
            ("size_basis", std::mem::offset_of!(StyleUniform, size_basis)),
            ("size_min_pixels", std::mem::offset_of!(StyleUniform, size_min_pixels)),
            ("size_max_pixels", std::mem::offset_of!(StyleUniform, size_max_pixels)),
            ("atlas_size", std::mem::offset_of!(StyleUniform, atlas_size)),
            ("billboard", std::mem::offset_of!(StyleUniform, billboard)),
            ("size_units", std::mem::offset_of!(StyleUniform, size_units)),
            ("alpha_cutoff", std::mem::offset_of!(StyleUniform, alpha_cutoff)),
            ("opacity", std::mem::offset_of!(StyleUniform, opacity)),
            ("focal_w", std::mem::offset_of!(StyleUniform, focal_w)),
            ("pad0", std::mem::offset_of!(StyleUniform, pad0)),
            ("centre", std::mem::offset_of!(StyleUniform, centre)),
            ("pad1", std::mem::offset_of!(StyleUniform, pad1)),
        ];
        assert_eq!(members, expected.map(|(n, o)| (n.to_string(), o as u32)));
        // the instance attributes sit where `Instance` keeps its fields
        let offsets: Vec<u64> = Instance::LAYOUT.attributes.iter().map(|a| a.offset).collect();
        let fields = [
            std::mem::offset_of!(Instance, position),
            std::mem::offset_of!(Instance, size),
            std::mem::offset_of!(Instance, frame),
            std::mem::offset_of!(Instance, offset),
            std::mem::offset_of!(Instance, pixel_offset),
            std::mem::offset_of!(Instance, angle),
            std::mem::offset_of!(Instance, color_mode),
            std::mem::offset_of!(Instance, color),
            std::mem::offset_of!(Instance, highlight),
        ];
        assert_eq!(offsets, fields.map(|f| f as u64));
        assert_eq!(Instance::LAYOUT.array_stride as usize, size_of::<Instance>());
        // deck's defaults
        let u = StyleUniform::new(&IconStyle::DEFAULT, [1, 1], &ViewFrame::new(&ViewState::INITIAL, SIZE, 1.0, 0.0));
        assert_eq!((u.size_units, u.size_basis, u.billboard, u.alpha_cutoff, u.size_max_pixels), (2, 1.0, 1, 0.05, 9.007_199e15));
        assert_eq!(StyleUniform::new(&IconStyle::GROUND, [1, 1], &ViewFrame::new(&ViewState::INITIAL, SIZE, 1.0, 0.0)).billboard, 0);
    }

    #[test]
    fn batches_grow_and_shrink() {
        let Some(gpu) = gpu() else { return };
        let renderer = MapRenderer::new(&gpu, FORMAT);
        let init = renderer.init(&gpu);
        let pipeline = IconPipeline::new(&init, "test icons", DrawState::TRAIN);
        let texture = upload_rgba8(&gpu, "atlas", [4, 4], &[255; 64], true);
        let mut batch = pipeline.batch(&gpu.device, "test", &texture.create_view(&Default::default()), [4, 4]);
        assert!(batch.is_empty());
        let vf = ViewFrame::new(&view(STOCKHOLM, 10.0, 0.0, 0.0), SIZE, 1.0, 0.0);
        let icons: Vec<Instance> = (0..5).map(|i| Instance::at(&vf, [18.0 + f64::from(i) * 0.01, 59.3, 0.0], &BLOCK, 10.0, 0.0, WHITE)).collect();
        batch.set_instances(&gpu.device, &gpu.queue, &icons);
        assert_eq!(batch.len(), 5);
        assert!(batch.instances.size() >= (5 * size_of::<Instance>()) as u64);
        batch.set_instances(&gpu.device, &gpu.queue, &icons[..2]);
        assert_eq!(batch.len(), 2);
        batch.set_style(&gpu.queue, &IconStyle::GROUND, &vf);
        assert_eq!(batch.style().billboard, 0);
        batch.set_instances(&gpu.device, &gpu.queue, &[]);
        assert!(batch.is_empty());
    }

    #[test]
    #[should_panic(expected = "culls nothing")]
    fn a_culling_preset_is_refused() {
        let Some(gpu) = gpu() else { panic!("skipped: culls nothing") };
        let renderer = MapRenderer::new(&gpu, FORMAT);
        let init = renderer.init(&gpu);
        IconPipeline::new(&init, "test icons", DrawState::DEFAULT);
    }
}
