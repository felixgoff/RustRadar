//! The `text` pipeline (plan 1.4.6 and 1.4.10, item E10): deck.gl 9.4's TextLayer with SDF glyphs,
//! for the place and airport labels, the aircraft labels, the signal names, block labels and speed
//! labels. Its shader is `shaders/text.wgsl`; `rustradar_scene::text` holds the fonts, the glyph
//! atlas and the layout, `rustradar_scene::sdf` TinySDF.
//!
//! - [`AtlasTexture`]: a [`GlyphAtlas`] on the GPU (`R8Unorm`, a full mip chain averaged 2 × 2 on
//!   the CPU as WebGL's `generateMipmap` does for deck's font texture), kept up to date by
//!   [`AtlasTexture::sync`], which uploads only what the atlas drew since.
//! - [`Label`]: one text at a common offset, its size, colour, pixel offset and angle;
//!   [`Label::glyphs`] makes one [`Glyph`] instance per inked glyph of its [`LaidText`],
//!   [`Label::background`] its [`Background`] box.
//! - [`TextStyle`]: the layer's props (`sizeUnits`, `sizeScale`, `sizeMinPixels`, `sizeMaxPixels`,
//!   `billboard`, `outlineWidth`, `outlineColor`, `opacity`); [`TextStyle::LABELS`] is the app's
//!   labels' (outline 3 in `[6, 12, 19, 220]`). [`BackgroundStyle`]: `backgroundPadding`,
//!   `backgroundBorderRadius` and whether the box is stroked.
//! - [`TextPipeline`]: the glyph and background pipelines for one draw-state preset (`LABEL` for
//!   every label of the app), the atlas sampler (linear, linear between mip levels, clamped) and
//!   the corner buffers; [`TextBatch`]: what one TextLayer draws.
//!
//! What it draws, as deck does: the background boxes first (when the layer has `background`), then
//! the characters with the outline, then, for an SDF layer with an outline, the characters again
//! with `outlineBuffer` at the fill's edge, so that a thick halo never covers a neighbour's fill
//! (MultiIconLayer's `draw`). Sizes and anchors are deck's: a glyph's quad is its atlas frame
//! around its offset from the label's anchor, scaled by `size · sizeScale / fontSize`.
//!
//! deck's collision fade (CollisionFilterExtension, plan 1.4.10) multiplies the fill colour's alpha
//! in the vertex shader, not the outline's, and drops the label altogether below a fade of 0.0001:
//! a layer applies the fade from `scene::declutter` to [`Label::color`]'s alpha (through the "crisp"
//! curve for the aircraft labels) and leaves out the labels that are faded out.
//!
//! Not ported (no layer of the app uses them): `getContentBox` clipping and content alignment,
//! deck's picking pass (picking is on the CPU, plan 1.4.11).

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use rustradar_scene::text::{FontSettings, GlyphAtlas, LaidGlyph, LaidText};

use super::icon::focal_w;
use super::{Desc, build};
use crate::frame::ViewFrame;
use crate::gpu::Gpu;
use crate::layers::Init;
use crate::shaders;
use crate::state::DrawState;

pub use super::path::{MAX_SAFE_INTEGER, Units};

/// `shaders/text.wgsl`.
pub const SHADER: &str = include_str!("../shaders/text.wgsl");

/// The bind group of a [`TextBatch`]: for the glyphs its [`StyleUniform`] (binding 0), atlas (1)
/// and sampler (2); for the backgrounds its [`BackgroundUniform`] (binding 3).
pub const BATCH_GROUP: u32 = 2;

/// MultiIconLayer's `alphaCutoff` default.
pub const ALPHA_CUTOFF: f64 = 0.001;

/// MultiIconLayer's `DEFAULT_BUFFER`: where the SDF's fill edge is, 0.75 (`1 − cutoff`).
pub const DEFAULT_BUFFER: f64 = 192.0 / 256.0;

/// The labels' outline colour (`LABEL_OUTLINE`, G:725).
pub const LABEL_OUTLINE: [u8; 4] = [6, 12, 19, 220];

/// IconLayer's quad (a triangle strip), −1 to 1, y down across the frame.
const CORNERS: [[f32; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]];

/// TextBackgroundLayer's quad, 0 to 1, y down across the box.
const BOX_CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];

const CORNER_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 8,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
};

/// One glyph of one label.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Glyph {
    /// The label's common offset ([`ViewFrame::common_offset`]).
    pub position: [f32; 3],
    /// deck's `getSize`, in the style's units.
    pub size: f32,
    /// The glyph's atlas frame: x, y, width, height texels (`instanceIconFrames`).
    pub frame: [f32; 4],
    /// The frame's middle from the label's anchor, atlas pixels, x right, y down
    /// (`instanceOffsets`).
    pub offset: [f32; 2],
    /// deck's `getPixelOffset`: CSS pixels, x right, y down, after the turn.
    pub pixel_offset: [f32; 2],
    /// deck's `getAngle`: degrees anticlockwise.
    pub angle: f32,
    /// deck's `getColor`, RGBA bytes.
    pub color: [u8; 4],
}

impl Glyph {
    /// The instance buffer layout: locations 1 to 7 (0 is the corner).
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Glyph>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![
            1 => Float32x3, 2 => Float32, 3 => Float32x4, 4 => Float32x2, 5 => Float32x2, 6 => Float32, 7 => Unorm8x4
        ],
    };
}

/// One label's background box.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Background {
    /// The label's common offset.
    pub position: [f32; 3],
    pub size: f32,
    /// The text's box from its anchor, atlas pixels: x, y (down), width, height
    /// ([`LaidText::rect`], deck's `getBoundingRect`).
    pub rect: [f32; 4],
    pub pixel_offset: [f32; 2],
    pub angle: f32,
    /// deck's `getBorderWidth`, CSS pixels (drawn when the style is stroked).
    pub line_width: f32,
    /// `getBackgroundColor`.
    pub fill_color: [u8; 4],
    /// `getBorderColor`.
    pub line_color: [u8; 4],
}

impl Background {
    /// The instance buffer layout: locations 1 to 8.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Background>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &wgpu::vertex_attr_array![
            1 => Float32x3, 2 => Float32, 3 => Float32x4, 4 => Float32x2, 5 => Float32, 6 => Float32, 7 => Unorm8x4, 8 => Unorm8x4
        ],
    };

    /// With a border of `width` CSS pixels in `color` (for a stroked [`BackgroundStyle`]).
    pub fn with_border(self, color: [u8; 4], width: f64) -> Background {
        Background { line_color: color, line_width: width as f32, ..self }
    }
}

/// One text at one place: the accessors of a TextLayer for one datum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Label {
    /// Its common offset.
    pub position: Vec3,
    /// deck's `getSize`, in the style's units (pixels for every label of the app).
    pub size: f64,
    /// `getColor`; its alpha carries the collision fade.
    pub color: [u8; 4],
    /// `getPixelOffset`: CSS pixels, x right, y down.
    pub pixel_offset: [f64; 2],
    /// `getAngle`: degrees anticlockwise.
    pub angle: f64,
}

impl Label {
    /// deck's defaults for the rest: no pixel offset, no angle.
    pub fn new(position: Vec3, size: f64, color: [u8; 4]) -> Label {
        Label { position, size, color, pixel_offset: [0.0; 2], angle: 0.0 }
    }

    /// [`Self::new`] at `[lon, lat, metres]` in `view` (the scene refresh's frame).
    pub fn at(view: &ViewFrame, [lon, lat, h]: [f64; 3], size: f64, color: [u8; 4]) -> Label {
        Self::new(view.common_offset([lon, lat], h), size, color)
    }

    pub fn with_pixel_offset(self, pixel_offset: [f64; 2]) -> Label {
        Label { pixel_offset, ..self }
    }

    pub fn with_angle(self, angle: f64) -> Label {
        Label { angle, ..self }
    }

    /// The instance of one laid-out glyph of this label.
    pub fn glyph(&self, glyph: &LaidGlyph) -> Glyph {
        Glyph {
            position: self.position.into(),
            size: self.size as f32,
            frame: glyph.frame.map(|v| v as f32),
            offset: glyph.offset.map(|v| v as f32),
            pixel_offset: self.pixel_offset.map(|v| v as f32),
            angle: self.angle as f32,
            color: self.color,
        }
    }

    /// The instances of `text`'s glyphs (deck's `calculateInstanceIconDefs`).
    pub fn glyphs<'a>(&'a self, text: &'a LaidText) -> impl Iterator<Item = Glyph> + 'a {
        text.glyphs.iter().map(|g| self.glyph(g))
    }

    /// `text`'s background box in `fill` (no border).
    pub fn background(&self, text: &LaidText, fill: [u8; 4]) -> Background {
        Background {
            position: self.position.into(),
            size: self.size as f32,
            rect: text.rect.map(|v| v as f32),
            pixel_offset: self.pixel_offset.map(|v| v as f32),
            angle: self.angle as f32,
            line_width: 0.0,
            fill_color: fill,
            // TextLayer's `getBorderColor` default
            line_color: [0, 0, 0, 255],
        }
    }
}

/// A TextLayer's drawing props (deck's defaults in [`TextStyle::DEFAULT`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size_units: Units,
    pub size_scale: f64,
    pub size_min_pixels: f64,
    pub size_max_pixels: f64,
    /// Facing the screen, or lying on the ground.
    pub billboard: bool,
    /// `outlineWidth`, in SDF pixels of the atlas (divided by the font settings' radius for the
    /// shader); 0 for none.
    pub outline_width: f64,
    pub outline_color: [u8; 4],
    /// deck's layer `opacity`.
    pub opacity: f64,
}

impl TextStyle {
    /// deck's defaults: billboards, sizes in pixels, scale 1, no clamp, no outline (its colour
    /// black), opaque.
    pub const DEFAULT: TextStyle = TextStyle {
        size_units: Units::Pixels,
        size_scale: 1.0,
        size_min_pixels: 0.0,
        size_max_pixels: MAX_SAFE_INTEGER,
        billboard: true,
        outline_width: 0.0,
        outline_color: [0, 0, 0, 255],
        opacity: 1.0,
    };

    /// The app's labels: `outlineWidth: 3`, `outlineColor: LABEL_OUTLINE`.
    pub const LABELS: TextStyle = TextStyle { outline_width: 3.0, outline_color: LABEL_OUTLINE, ..Self::DEFAULT };

    /// With `sizeScale` (the user's label size setting).
    pub fn scaled(self, size_scale: f64) -> TextStyle {
        TextStyle { size_scale, ..self }
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The background props of a TextLayer with `background: true`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackgroundStyle {
    /// `backgroundPadding`: left, top, right, bottom, CSS pixels.
    pub padding: [f64; 4],
    /// `backgroundBorderRadius`, CSS pixels, deck's order (`[bottom-right, top-right,
    /// bottom-left, top-left]` on the screen, as its `round_rect` reads them).
    pub border_radius: [f64; 4],
    /// `stroked`: deck strokes the box when `getBorderWidth` is set.
    pub stroked: bool,
}

impl BackgroundStyle {
    /// deck's defaults: no padding, square corners, not stroked.
    pub const DEFAULT: BackgroundStyle = BackgroundStyle { padding: [0.0; 4], border_radius: [0.0; 4], stroked: false };

    /// With `backgroundPadding`: `[x, y]` (deck's `[p0, p1, p0, p1]`) or `[left, top, right,
    /// bottom]`.
    pub fn padded(self, padding: &[f64]) -> BackgroundStyle {
        let padding = match *padding {
            [x, y] => [x, y, x, y],
            [l, t, r, b] => [l, t, r, b],
            _ => panic!("backgroundPadding takes 2 or 4 values, not {padding:?}"),
        };
        BackgroundStyle { padding, ..self }
    }

    /// With `backgroundBorderRadius: r` (all corners).
    pub fn rounded(self, r: f64) -> BackgroundStyle {
        BackgroundStyle { border_radius: [r; 4], ..self }
    }
}

impl Default for BackgroundStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `text.wgsl`'s `TextStyle`: one draw of a TextLayer's characters.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StyleUniform {
    pub size_scale: f32,
    pub size_min_pixels: f32,
    pub size_max_pixels: f32,
    /// The atlas's font size.
    pub font_size: f32,
    pub atlas_size: [f32; 2],
    pub billboard: u32,
    pub size_units: u32,
    pub alpha_cutoff: f32,
    pub opacity: f32,
    /// [`focal_w`].
    pub focal_w: f32,
    /// 1 for an SDF atlas.
    pub sdf: u32,
    /// `sdf.buffer`, `sdf.gamma`, `sdf.outlineBuffer` (−1 for none).
    pub buffer: f32,
    pub gamma: f32,
    pub outline_buffer: f32,
    pub pad0: f32,
    /// 0 to 1.
    pub outline_color: [f32; 4],
    /// [`ViewFrame::common_centre`].
    pub centre: [f32; 3],
    pub pad1: f32,
}

impl StyleUniform {
    /// MultiIconLayer's `draw` for `style` over an atlas of `font` settings and `atlas_size`
    /// texels in `view`: the first draw (with the outline, if any) and, for an SDF layer with an
    /// outline, the second (`outlineBuffer` at the fill's edge).
    pub fn passes(style: &TextStyle, font: &FontSettings, atlas_size: [u32; 2], view: &ViewFrame) -> (StyleUniform, Option<StyleUniform>) {
        let first = StyleUniform { focal_w: focal_w(view) as f32, centre: view.common_centre.as_vec3().into(), ..Self::props(style, font, atlas_size) };
        let second = (font.sdf && first.outline_buffer > 0.0).then_some(StyleUniform { outline_buffer: DEFAULT_BUFFER as f32, ..first });
        (first, second)
    }

    /// `style`'s own terms, with deck's focal w (1.5) and a centre at the origin for the view's.
    fn props(style: &TextStyle, font: &FontSettings, atlas_size: [u32; 2]) -> StyleUniform {
        // TextLayer: `outlineWidth / (fontSettings.radius || 12)`; MultiIconLayer: `outlineWidth ?
        // max(smoothing, DEFAULT_BUFFER · (1 − outlineWidth)) : −1`
        let radius = if font.radius == 0.0 || font.radius.is_nan() { 12.0 } else { font.radius };
        let outline_width = style.outline_width / radius;
        let smoothing = if font.smoothing.is_finite() { font.smoothing } else { 0.1 };
        let outline_buffer = if outline_width != 0.0 && !outline_width.is_nan() { smoothing.max(DEFAULT_BUFFER * (1.0 - outline_width)) } else { -1.0 };
        StyleUniform {
            size_scale: style.size_scale as f32,
            size_min_pixels: style.size_min_pixels as f32,
            size_max_pixels: style.size_max_pixels as f32,
            font_size: font.font_size as f32,
            atlas_size: atlas_size.map(|s| s as f32),
            billboard: u32::from(style.billboard),
            size_units: style.size_units as u32,
            alpha_cutoff: ALPHA_CUTOFF as f32,
            opacity: style.opacity as f32,
            focal_w: rustradar_scene::camera::CAMERA_HEIGHTS as f32,
            sdf: u32::from(font.sdf),
            buffer: DEFAULT_BUFFER as f32,
            gamma: smoothing as f32,
            outline_buffer: outline_buffer as f32,
            pad0: 0.0,
            outline_color: style.outline_color.map(|c| f32::from(c) / 255.0),
            centre: [0.0; 3],
            pad1: 0.0,
        }
    }
}

/// `text.wgsl`'s `BackgroundStyle`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct BackgroundUniform {
    pub size_scale: f32,
    pub size_min_pixels: f32,
    pub size_max_pixels: f32,
    pub font_size: f32,
    pub border_radius: [f32; 4],
    pub padding: [f32; 4],
    pub billboard: u32,
    pub size_units: u32,
    pub stroked: u32,
    pub opacity: f32,
    pub focal_w: f32,
    pub pad0: [f32; 3],
    pub centre: [f32; 3],
    pub pad1: f32,
}

impl BackgroundUniform {
    /// TextBackgroundLayer's uniforms for `style` and `background` over `font` in `view`.
    pub fn new(style: &TextStyle, background: &BackgroundStyle, font: &FontSettings, view: &ViewFrame) -> BackgroundUniform {
        BackgroundUniform { focal_w: focal_w(view) as f32, centre: view.common_centre.as_vec3().into(), ..Self::props(style, background, font) }
    }

    fn props(style: &TextStyle, background: &BackgroundStyle, font: &FontSettings) -> BackgroundUniform {
        BackgroundUniform {
            size_scale: style.size_scale as f32,
            size_min_pixels: style.size_min_pixels as f32,
            size_max_pixels: style.size_max_pixels as f32,
            font_size: font.font_size as f32,
            border_radius: background.border_radius.map(|v| v as f32),
            padding: background.padding.map(|v| v as f32),
            billboard: u32::from(style.billboard),
            size_units: style.size_units as u32,
            stroked: u32::from(background.stroked),
            opacity: style.opacity as f32,
            focal_w: rustradar_scene::camera::CAMERA_HEIGHTS as f32,
            pad0: [0.0; 3],
            centre: [0.0; 3],
            pad1: 0.0,
        }
    }
}

/// Each mip level's width and height, from `size` down to 1 × 1.
fn mip_sizes([w, h]: [u32; 2]) -> Vec<[u32; 2]> {
    let levels = 32 - w.max(h).max(1).leading_zeros();
    (0..levels).map(|l| [(w >> l).max(1), (h >> l).max(1)]).collect()
}

/// Recomputes the mip levels below `level0` (of `size`) over the part `region` of it covers,
/// each texel the rounded mean of the 2 × 2 above it (the last row or column repeated where a size
/// is odd), and returns each level's changed region (`levels[i]` is level `i + 1`).
fn update_mips(level0: &[u8], size: [u32; 2], levels: &mut [Vec<u8>], [x, y, w, h]: [u32; 4]) -> Vec<[u32; 4]> {
    let sizes = mip_sizes(size);
    let mut regions = Vec::with_capacity(levels.len());
    let (mut x0, mut y0, mut x1, mut y1) = (x, y, x + w, y + h);
    for l in 1..sizes.len() {
        let [pw, ph] = sizes[l - 1];
        let [nw, nh] = sizes[l];
        (x0, y0) = (x0 / 2, y0 / 2);
        (x1, y1) = (x1.div_ceil(2).min(nw), y1.div_ceil(2).min(nh));
        let (above, rest) = levels.split_at_mut(l - 1);
        let src: &[u8] = if l == 1 { level0 } else { &above[l - 2] };
        let dst = &mut rest[0];
        let at = |px: u32, py: u32| u32::from(src[(py.min(ph - 1) * pw + px.min(pw - 1)) as usize]);
        for ty in y0..y1 {
            for tx in x0..x1 {
                let sum = at(2 * tx, 2 * ty) + at(2 * tx + 1, 2 * ty) + at(2 * tx, 2 * ty + 1) + at(2 * tx + 1, 2 * ty + 1);
                dst[(ty * nw + tx) as usize] = ((sum + 2) / 4) as u8;
            }
        }
        regions.push([x0, y0, x1 - x0, y1 - y0]);
    }
    regions
}

/// A [`GlyphAtlas`] on the GPU: an `R8Unorm` texture with a full mip chain.
pub struct AtlasTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    /// Levels 1 and down, on the CPU, so that a change is averaged down without reading back.
    levels: Vec<Vec<u8>>,
}

impl AtlasTexture {
    /// The texture of `atlas`, with all it holds uploaded.
    pub fn new(gpu: &Gpu, label: &str, atlas: &mut GlyphAtlas) -> AtlasTexture {
        let size = atlas.size();
        let sizes = mip_sizes(size);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: sizes.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let levels = sizes[1..].iter().map(|[w, h]| vec![0; (w * h) as usize]).collect();
        let mut texture = AtlasTexture { texture, view, size, levels };
        atlas.take_dirty();
        texture.upload(gpu, atlas, [0, 0, size[0], size[1]]);
        texture
    }

    /// Uploads what `atlas` drew since the last call; whether there was anything.
    ///
    /// # Panics
    /// If `atlas` is not the size this texture was made for.
    pub fn sync(&mut self, gpu: &Gpu, atlas: &mut GlyphAtlas) -> bool {
        assert_eq!(atlas.size(), self.size, "an atlas texture serves one atlas");
        match atlas.take_dirty() {
            Some(region) => {
                self.upload(gpu, atlas, region);
                true
            }
            None => false,
        }
    }

    fn upload(&mut self, gpu: &Gpu, atlas: &GlyphAtlas, region: [u32; 4]) {
        let regions = update_mips(atlas.pixels(), self.size, &mut self.levels, region);
        let sizes = mip_sizes(self.size);
        let write = |mip: u32, data: &[u8], [lw, _]: [u32; 2], [x, y, w, h]: [u32; 4]| {
            if w == 0 || h == 0 {
                return;
            }
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: mip, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                &data[(y * lw + x) as usize..],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        };
        write(0, atlas.pixels(), sizes[0], region);
        for (l, r) in regions.iter().enumerate() {
            write(l as u32 + 1, &self.levels[l], sizes[l + 1], *r);
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Width and height at level 0, texels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
}

/// The text pipelines for one draw-state preset. Every text layer of the app culls nothing
/// (`LABEL` and its likes): deck's quads wind clockwise on the screen, so [`Self::new`] refuses a
/// preset that culls.
pub struct TextPipeline {
    glyphs: wgpu::RenderPipeline,
    backgrounds: wgpu::RenderPipeline,
    glyph_layout: wgpu::BindGroupLayout,
    background_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    corners: wgpu::Buffer,
    box_corners: wgpu::Buffer,
}

fn uniform_entry(binding: u32, size: usize) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

impl TextPipeline {
    /// The pipelines with `state` for the map pass of `init`.
    ///
    /// # Panics
    /// If `state` culls faces.
    pub fn new(init: &Init<'_>, label: &str, state: DrawState) -> Self {
        assert!(state.cull.is_none(), "{label}: text quads wind clockwise, so they need a preset that culls nothing");
        let device = &init.gpu.device;
        let glyph_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text batch"),
            entries: &[
                uniform_entry(0, size_of::<StyleUniform>()),
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
        let background_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text background batch"),
            entries: &[uniform_entry(3, size_of::<BackgroundUniform>())],
        });
        let module = shaders::module(device, label, SHADER);
        let glyphs = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry: "vs_glyph",
                fragment_entry: "fs_glyph",
                buffers: &[Some(CORNER_LAYOUT), Some(Glyph::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                state,
                groups: &[&glyph_layout],
            },
        );
        let backgrounds = build(
            init,
            &Desc {
                label,
                module: &module,
                vertex_entry: "vs_background",
                fragment_entry: "fs_background",
                buffers: &[Some(CORNER_LAYOUT), Some(Background::LAYOUT)],
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                state,
                groups: &[&background_layout],
            },
        );
        // deck's `DEFAULT_TEXTURE_PARAMETERS` for image props (`utils/texture.js`)
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("text atlas"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let buffer = |name: &str, corners: &[[f32; 2]; 4]| {
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor { label: Some(name), contents: bytemuck::cast_slice(corners), usage: wgpu::BufferUsages::VERTEX },
            )
        };
        TextPipeline {
            glyphs,
            backgrounds,
            glyph_layout,
            background_layout,
            sampler,
            corners: buffer("text corners", &CORNERS),
            box_corners: buffer("text box corners", &BOX_CORNERS),
        }
    }

    /// A batch drawing from `atlas`, with nothing to draw yet and deck's default style until
    /// [`TextBatch::set_style`].
    pub fn batch(&self, device: &wgpu::Device, label: &str, atlas: &AtlasTexture) -> TextBatch {
        let first = StyleUniform::props(&TextStyle::DEFAULT, &FontSettings::DEFAULT, atlas.size());
        let background = BackgroundUniform::props(&TextStyle::DEFAULT, &BackgroundStyle::DEFAULT, &FontSettings::DEFAULT);
        let uniform = |name: &str, bytes: &[u8]| {
            wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor { label: Some(name), contents: bytes, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST },
            )
        };
        let uniforms = [uniform(label, bytemuck::bytes_of(&first)), uniform(label, bytemuck::bytes_of(&first))];
        let groups = [self.glyph_group(device, label, &uniforms[0], atlas), self.glyph_group(device, label, &uniforms[1], atlas)];
        let background_uniform = uniform(label, bytemuck::bytes_of(&background));
        let background_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.background_layout,
            entries: &[wgpu::BindGroupEntry { binding: 3, resource: background_uniform.as_entire_binding() }],
        });
        TextBatch {
            label: label.to_string(),
            uniforms,
            groups,
            written: [first, first],
            second: false,
            background_uniform,
            background_group,
            background_written: background,
            atlas_size: atlas.size(),
            glyphs: TextBatch::allocate::<Glyph>(device, label, 1),
            glyph_count: 0,
            backgrounds: TextBatch::allocate::<Background>(device, label, 1),
            background_count: 0,
        }
    }

    fn glyph_group(&self, device: &wgpu::Device, label: &str, uniform: &wgpu::Buffer, atlas: &AtlasTexture) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.glyph_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(atlas.view()) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    /// Draws `batch`: its backgrounds, then its characters (twice for an SDF outline). Text reads no
    /// local Mercator frame (group 1 stays as the pass left it).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, batch: &TextBatch) {
        if batch.background_count > 0 {
            pass.set_pipeline(&self.backgrounds);
            pass.set_vertex_buffer(0, self.box_corners.slice(..));
            pass.set_vertex_buffer(1, batch.backgrounds.slice(..));
            pass.set_bind_group(BATCH_GROUP, &batch.background_group, &[]);
            pass.draw(0..BOX_CORNERS.len() as u32, 0..batch.background_count);
        }
        if batch.glyph_count > 0 {
            pass.set_pipeline(&self.glyphs);
            pass.set_vertex_buffer(0, self.corners.slice(..));
            pass.set_vertex_buffer(1, batch.glyphs.slice(..));
            pass.set_bind_group(BATCH_GROUP, &batch.groups[0], &[]);
            pass.draw(0..CORNERS.len() as u32, 0..batch.glyph_count);
            if batch.second {
                pass.set_bind_group(BATCH_GROUP, &batch.groups[1], &[]);
                pass.draw(0..CORNERS.len() as u32, 0..batch.glyph_count);
            }
        }
    }
}

/// What one TextLayer draws: its two character draws' uniforms, its background uniform, its
/// glyphs and its background boxes.
pub struct TextBatch {
    label: String,
    uniforms: [wgpu::Buffer; 2],
    groups: [wgpu::BindGroup; 2],
    written: [StyleUniform; 2],
    second: bool,
    background_uniform: wgpu::Buffer,
    background_group: wgpu::BindGroup,
    background_written: BackgroundUniform,
    atlas_size: [u32; 2],
    glyphs: wgpu::Buffer,
    glyph_count: u32,
    backgrounds: wgpu::Buffer,
    background_count: u32,
}

impl TextBatch {
    fn allocate<T>(device: &wgpu::Device, label: &str, count: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (count.max(1) * size_of::<T>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// The layer's props for this frame's `view` (the focal depth and, on the globe, the centre a
    /// ground-plane label's up comes from), for an atlas of `font` settings, and its background
    /// props; written when they changed. Call it in every `prepare`.
    pub fn set_style(&mut self, queue: &wgpu::Queue, style: &TextStyle, background: &BackgroundStyle, font: &FontSettings, view: &ViewFrame) {
        let (first, second) = StyleUniform::passes(style, font, self.atlas_size, view);
        self.second = second.is_some();
        for (i, uniform) in [Some(first), second].into_iter().enumerate() {
            if let Some(uniform) = uniform
                && uniform != self.written[i]
            {
                queue.write_buffer(&self.uniforms[i], 0, bytemuck::bytes_of(&uniform));
                self.written[i] = uniform;
            }
        }
        let uniform = BackgroundUniform::new(style, background, font, view);
        if uniform != self.background_written {
            queue.write_buffer(&self.background_uniform, 0, bytemuck::bytes_of(&uniform));
            self.background_written = uniform;
        }
    }

    /// The first character draw's uniform as last written.
    pub fn style(&self) -> &StyleUniform {
        &self.written[0]
    }

    /// How many times the characters are drawn: 2 for an SDF layer with an outline, else 1.
    pub fn passes(&self) -> usize {
        1 + usize::from(self.second)
    }

    /// Replaces its glyphs (at each scene refresh), growing the buffer when they do not fit.
    pub fn set_glyphs(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, glyphs: &[Glyph]) {
        Self::fill(device, queue, &self.label, &mut self.glyphs, glyphs);
        self.glyph_count = glyphs.len() as u32;
    }

    /// Replaces its background boxes; none for a layer without `background`.
    pub fn set_backgrounds(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, backgrounds: &[Background]) {
        Self::fill(device, queue, &self.label, &mut self.backgrounds, backgrounds);
        self.background_count = backgrounds.len() as u32;
    }

    fn fill<T: Pod>(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, buffer: &mut wgpu::Buffer, items: &[T]) {
        let bytes: &[u8] = bytemuck::cast_slice(items);
        if bytes.len() as u64 > buffer.size() {
            *buffer = Self::allocate::<T>(device, label, items.len().next_power_of_two());
        }
        if !bytes.is_empty() {
            queue.write_buffer(buffer, 0, bytes);
        }
    }

    /// How many glyphs it draws.
    pub fn len(&self) -> u32 {
        self.glyph_count
    }

    /// How many background boxes it draws.
    pub fn background_len(&self) -> u32 {
        self.background_count
    }

    pub fn is_empty(&self) -> bool {
        self.glyph_count == 0 && self.background_count == 0
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path as FilePath;
    use std::rc::Rc;
    use std::sync::Arc;

    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::sdf::Canvas;
    use rustradar_scene::text::{AlignmentBaseline, Face, Family, FontCanvas, OVERPASS, TextAnchor, TextLayout};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::{DrawCx, Layer, PrepareCx};
    use crate::pipelines::fill::tests::wgsl_members;
    use crate::renderer::{MapRenderer, SceneInput};

    const STOCKHOLM: [f64; 2] = [18.07, 59.33];
    /// A city label's colour (doc 01 §5.9).
    const CITY: [u8; 4] = [241, 238, 231, 245];
    const SIZE: [f64; 2] = [240.0, 120.0];
    const INK: f64 = 30.0;
    /// Texture pixels per CSS pixel of 14 px text.
    const S14: f64 = 14.0 / 64.0;

    fn view([longitude, latitude]: [f64; 2], zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, pitch, bearing, position: None }
    }

    /// One label to draw, placed in each frame's view.
    #[derive(Debug, Clone)]
    struct Spec {
        at: [f64; 2],
        text: String,
        face: Face,
        layout: TextLayout,
        size: f64,
        color: [u8; 4],
        pixel_offset: [f64; 2],
        /// The background's fill and its border's colour and width.
        background: Option<([u8; 4], [u8; 4], f64)>,
        angle: f64,
    }

    impl Spec {
        fn new(at: [f64; 2], text: &str, size: f64, color: [u8; 4]) -> Spec {
            Spec { at, text: text.to_string(), face: Face::LABEL, layout: TextLayout::DEFAULT, size, color, pixel_offset: [0.0; 2], background: None, angle: 0.0 }
        }
    }

    /// One TextLayer: its atlas settings, style and labels.
    #[derive(Debug, Clone)]
    struct Draw {
        font: FontSettings,
        style: TextStyle,
        background: BackgroundStyle,
        labels: Vec<Spec>,
    }

    impl Draw {
        /// The app's labels: SDF, outline 3.
        fn labels(labels: Vec<Spec>) -> Draw {
            Draw { font: FontSettings::LABELS, style: TextStyle::LABELS, background: BackgroundStyle::DEFAULT, labels }
        }
    }

    /// An atlas and its texture.
    struct Atlas {
        cpu: GlyphAtlas,
        gpu: AtlasTexture,
    }

    /// A test layer: one batch per draw, each over the atlas of its font settings.
    struct Texts {
        pipeline: TextPipeline,
        atlases: Vec<Atlas>,
        batches: Vec<(usize, TextBatch)>,
        draws: Rc<RefCell<Vec<Draw>>>,
        drawn: usize,
    }

    impl Layer for Texts {
        fn name(&self) -> &'static str {
            "test text"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            let draws = self.draws.borrow();
            let mut laid_out = Vec::new();
            for draw in draws.iter() {
                let a = if draw.font == FontSettings::LABELS { 0 } else { 1 };
                assert!(a == 0 || draw.font == FontSettings::DEFAULT, "the test atlases are LABELS and DEFAULT");
                let atlas = &mut self.atlases[a].cpu;
                let mut glyphs = Vec::new();
                let mut backgrounds = Vec::new();
                for spec in &draw.labels {
                    let laid = atlas.layout(spec.face, &spec.text, &spec.layout);
                    let label = Label::at(cx.view, [spec.at[0], spec.at[1], 0.0], spec.size, spec.color).with_pixel_offset(spec.pixel_offset).with_angle(spec.angle);
                    glyphs.extend(label.glyphs(&laid));
                    if let Some((fill, line, width)) = spec.background {
                        backgrounds.push(label.background(&laid, fill).with_border(line, width));
                    }
                }
                laid_out.push((a, glyphs, backgrounds));
            }
            for atlas in &mut self.atlases {
                atlas.gpu.sync(cx.gpu, &mut atlas.cpu);
            }
            for (i, (draw, (a, glyphs, backgrounds))) in draws.iter().zip(laid_out).enumerate() {
                if self.batches.get(i).is_none_or(|(b, _)| *b != a) {
                    let batch = self.pipeline.batch(&cx.gpu.device, "test text", &self.atlases[a].gpu);
                    if i < self.batches.len() {
                        self.batches[i] = (a, batch);
                    } else {
                        self.batches.push((a, batch));
                    }
                }
                let batch = &mut self.batches[i].1;
                batch.set_style(&cx.gpu.queue, &draw.style, &draw.background, &draw.font, cx.view);
                batch.set_glyphs(&cx.gpu.device, &cx.gpu.queue, &glyphs);
                batch.set_backgrounds(&cx.gpu.device, &cx.gpu.queue, &backgrounds);
            }
            self.drawn = draws.len();
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, _cx: &DrawCx<'_>) {
            for (_, batch) in &self.batches[..self.drawn] {
                self.pipeline.draw(pass, batch);
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
            let pipeline = TextPipeline::new(&init, "test text", DrawState::LABEL);
            let atlases = [FontSettings::LABELS, FontSettings::DEFAULT]
                .map(|settings| {
                    let mut cpu = GlyphAtlas::new(settings);
                    let gpu = AtlasTexture::new(gpu, "test atlas", &mut cpu);
                    Atlas { cpu, gpu }
                })
                .into();
            let draws = Rc::new(RefCell::new(Vec::new()));
            renderer.add_layer(Box::new(Texts { pipeline, atlases, batches: Vec::new(), draws: draws.clone(), drawn: 0 }));
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

    fn save(image: &Image, name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots");
        std::fs::create_dir_all(dir).expect("shot folder");
        image.save_png(&FilePath::new(dir).join(name)).expect("png");
    }

    /// Where `[lon, lat]` lands, CSS pixels.
    fn screen(vf: &ViewFrame, [lon, lat]: [f64; 2]) -> [f64; 2] {
        let [x, y, _] = vf.viewport.world_to_pixels(vf.viewport.project_position([lon, lat, 0.0]));
        [x, y]
    }

    /// The bounds of the pixels `pick` keeps among `rows` × `columns` (physical pixels), in CSS
    /// pixels of a `scale` image: left, top, right, bottom.
    fn ink_box(image: &Image, scale: f64, columns: std::ops::Range<u32>, rows: std::ops::Range<u32>, pick: impl Fn([u8; 4]) -> bool) -> Option<[f64; 4]> {
        let mut found: Option<[u32; 4]> = None;
        for y in rows {
            for x in columns.clone() {
                if pick(image.pixel(x, y)) {
                    found = Some(match found {
                        None => [x, y, x + 1, y + 1],
                        Some([l, t, r, b]) => [l.min(x), t.min(y), r.max(x + 1), b.max(y + 1)],
                    });
                }
            }
        }
        found.map(|b| b.map(|v| f64::from(v) / scale))
    }

    /// The bright fill of light text over its halo: green over the middle of halo and fill.
    fn fill(p: [u8; 4]) -> bool {
        p[1] > 122
    }

    fn whole(image: &Image) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
        (0..image.width, 0..image.height)
    }

    /// The coverage of `text` laid out by epaint in Overpass at weight 600 (the app's `ui-600`
    /// family: plan 1.5) at `size` points and `ppp` pixels per point, with the first glyph's pen at
    /// `pen` (CSS pixels, its baseline's y): its glyphs' atlas texels where epaint's tessellator puts
    /// them, on a `width × height` picture of physical pixels.
    fn epaint_coverage(text: &str, size: f32, ppp: f32, [pen_x, baseline]: [f64; 2], [width, height]: [usize; 2]) -> Vec<f64> {
        use epaint::text::{FontData, FontDefinitions, FontFamily, FontId, FontTweak, Fonts, TextOptions, VariationCoords};
        let mut definitions = FontDefinitions::empty();
        let tweak = FontTweak { coords: VariationCoords::new([(b"wght", 600.0)]), ..Default::default() };
        definitions.font_data.insert("overpass".into(), Arc::new(FontData::from_static(OVERPASS[0]).tweak(tweak)));
        let family = FontFamily::Name("ui-600".into());
        definitions.families.insert(family.clone(), vec!["overpass".into()]);
        let mut fonts = Fonts::new(TextOptions::default(), definitions);
        let galley = fonts.with_pixels_per_point(ppp).layout_no_wrap(text.to_string(), FontId::new(size, family), epaint::Color32::WHITE);
        let atlas = fonts.image();
        let row = &galley.rows[0];
        let first = &row.glyphs[0];
        // the galley where the first pen and the baseline land on ours, on whole pixels as egui
        // paints it
        let ppp64 = f64::from(ppp);
        let snap = |v: f64| (v * ppp64).round() / ppp64;
        let origin = [snap(pen_x - f64::from(row.pos.x + first.pos.x)), snap(baseline - f64::from(row.pos.y + first.pos.y))];
        let mut coverage = vec![0.0; width * height];
        for g in &row.glyphs {
            let uv = g.uv_rect;
            if uv.is_nothing() {
                continue;
            }
            // `tessellate_glyphs`: the glyph's left top rounded to pixels
            let lt = [((g.pos.x + uv.offset.x) * ppp).round() / ppp, ((g.pos.y + uv.offset.y) * ppp).round() / ppp];
            let at = [((origin[0] + f64::from(row.pos.x + lt[0])) * ppp64).round() as i64, ((origin[1] + f64::from(row.pos.y + lt[1])) * ppp64).round() as i64];
            for ty in uv.min[1]..uv.max[1] {
                for tx in uv.min[0]..uv.max[0] {
                    let x = at[0] + i64::from(tx - uv.min[0]);
                    let y = at[1] + i64::from(ty - uv.min[1]);
                    let a = f64::from(atlas.pixels[usize::from(ty) * atlas.size[0] + usize::from(tx)].a()) / 255.0;
                    let i = y as usize * width + x as usize;
                    coverage[i] = f64::max(coverage[i], a);
                }
            }
        }
        coverage
    }

    /// The ink box of a coverage picture (`width × height` physical pixels, rows top first) in
    /// CSS pixels of a `scale` picture: where the largest coverage of the columns (and rows)
    /// crosses one half, between pixel centres. Left, top, right, bottom.
    fn profile_box(coverage: &[f64], [width, height]: [usize; 2], scale: f64) -> [f64; 4] {
        let columns: Vec<f64> = (0..width).map(|x| (0..height).map(|y| coverage[y * width + x]).fold(0.0, f64::max)).collect();
        let rows: Vec<f64> = (0..height).map(|y| coverage[y * width..(y + 1) * width].iter().copied().fold(0.0, f64::max)).collect();
        let rising = |p: &[f64]| {
            let i = p.iter().position(|&c| c >= 0.5).expect("ink");
            let before = if i == 0 { 0.0 } else { p[i - 1] };
            (i as f64 - 0.5) + (0.5 - before) / (p[i] - before)
        };
        let falling = |p: &[f64]| {
            let j = p.iter().rposition(|&c| c >= 0.5).expect("ink");
            let after = p.get(j + 1).copied().unwrap_or(0.0);
            (j as f64 + 0.5) + (p[j] - 0.5) / (p[j] - after)
        };
        [rising(&columns), rising(&rows), falling(&columns), falling(&rows)].map(|v| v / scale)
    }

    /// Item E10's acceptance: "Stockholm" at 14 px in `[241, 238, 231, 245]` with a dark halo;
    /// its ink box is within 1 px of the same string laid out by epaint at 14 px in `ui-600`, with
    /// the pen and the baseline where deck's layout puts them. At map scale 1 and 2.
    #[test]
    fn stockholm_at_14_px_has_a_dark_halo_and_epaints_ink_box() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        for scale in [1.0, 2.0] {
            let v = view(STOCKHOLM, 9.0, 0.0, 0.0);
            let (image, vf) = bench.shoot(&gpu, v, scale, vec![Draw::labels(vec![Spec::new(STOCKHOLM, "Stockholm", 14.0, CITY)])]);
            save(&image, &format!("e10-stockholm-x{scale}.png"));
            let [ax, ay] = screen(&vf, STOCKHOLM);
            let (columns, rows) = whole(&image);
            let ink = ink_box(&image, scale, columns.clone(), rows.clone(), fill).expect("the label drew");

            // the pen and the baseline of deck's layout (middle, center): the row's start half its
            // width left of the anchor, the baseline `baselineOffset` (16 atlas px) below it
            let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
            let laid = atlas.layout(Face::LABEL, "Stockholm", &TextLayout::DEFAULT);
            let pen = [ax + laid.box_px(14.0)[0], ay + atlas.baseline_offset(Face::LABEL) * S14];
            let size = [image.width as usize, image.height as usize];
            let epaint = profile_box(&epaint_coverage("Stockholm", 14.0, scale as f32, pen, size), size, scale);
            let ours = profile_box(&fill_coverage(&image), size, scale);
            eprintln!("scale {scale}: ink box {ours:?} (whole pixels {ink:?}), epaint's {epaint:?}, anchor {ax},{ay}");
            for (side, (a, b)) in ["left", "top", "right", "bottom"].iter().zip(ours.iter().zip(&epaint)) {
                assert!((a - b).abs() <= 1.0, "scale {scale}: the ink box's {side} is {a}, epaint's {b}");
            }

            // the fill: drawn twice (deck's two passes) it comes out nearly opaque
            let brightest = (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y))).map(|(x, y)| image.pixel(x, y)).max_by_key(|p| p[1]).expect("pixels");
            for c in 0..3 {
                let expected = f64::from(CITY[c]) * 0.961 + (f64::from(CITY[c]) * 0.961 + INK * 0.039) * 0.039;
                assert!((f64::from(brightest[c]) - expected).abs() <= 4.0, "scale {scale}: the fill is {brightest:?}");
            }

            // the halo: darker than the map round the letters, and only close to them
            let halo = |p: [u8; 4]| p[1] < 24;
            let dark = ink_box(&image, scale, columns.clone(), rows.clone(), halo).expect("a halo");
            for (side, (h, i)) in ["left", "top", "right", "bottom"].iter().zip(dark.iter().zip(&ink)) {
                assert!((h - i).abs() <= 2.0, "scale {scale}: the halo's {side} is {h}, the ink's {i}");
            }
            let count = |pick: &dyn Fn([u8; 4]) -> bool| (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y))).filter(|&(x, y)| pick(image.pixel(x, y))).count();
            let (halo_px, fill_px) = (count(&halo), count(&fill));
            eprintln!("scale {scale}: {fill_px} fill pixels, {halo_px} halo pixels");
            assert!(halo_px as f64 > 0.4 * fill_px as f64, "scale {scale}: {halo_px} halo pixels for {fill_px} of fill");
            let darkest = (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y))).map(|(x, y)| image.pixel(x, y)).min_by_key(|p| p[1]).expect("pixels");
            // the outline colour over the map: (6, 12, 19) at 220/255 over 30
            assert!(darkest[1] <= 18 && darkest[2] <= 24, "scale {scale}: the halo's darkest is {darkest:?}");
            // nothing else drawn: away from the label the map is untouched
            assert_eq!(image.pixel(2, 2), [30, 30, 30, 255]);
        }
    }

    /// The box of `text`'s outlines (their control boxes) set at `size` px with deck's advances
    /// from the pen `[x, baseline]`: left, top, right, bottom.
    fn outline_box(text: &str, size: f64, [x, baseline]: [f64; 2]) -> [f64; 4] {
        let canvas = FontCanvas::new(Face::LABEL, 64.0);
        let k = size / 64.0;
        let mut pen = x;
        let mut found = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for ch in text.chars() {
            let m = canvas.measure_text(ch);
            found = [
                found[0].min(pen - m.actual_bounding_box_left * k),
                found[1].min(baseline - m.actual_bounding_box_ascent * k),
                found[2].max(pen + m.actual_bounding_box_right * k),
                found[3].max(baseline + m.actual_bounding_box_descent * k),
            ];
            pen += m.width * k;
        }
        found
    }

    /// The fill's coverage over the halo in a label shot, from the green of the halo (12 at
    /// 220/255 over 30) to the fill's (238 at 245/255, drawn twice).
    fn fill_coverage(image: &Image) -> Vec<f64> {
        let (halo_g, fill_g) = (12.0 * 220.0 / 255.0 + INK * 35.0 / 255.0, 237.7);
        (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .map(|(x, y)| ((f64::from(image.pixel(x, y)[1]) - halo_g) / (fill_g - halo_g)).clamp(0.0, 1.0))
            .collect()
    }

    /// A name with a `j`, whose ink reaches left of its pen: its ink box is where the font's
    /// outlines put it, and the picture (`e10-sodertalje.png`) shows no gap before the `j` (today's
    /// app draws one: the reference `dark-v3-1440x900.png` reads "Södertäl je"; `scene::text` says
    /// why).
    #[test]
    fn a_j_keeps_its_place() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        let v = view(STOCKHOLM, 9.0, 0.0, 0.0);
        for text in ["Södertälje", "Stockholm"] {
            let (image, vf) = bench.shoot(&gpu, v, 1.0, vec![Draw::labels(vec![Spec::new(STOCKHOLM, text, 14.0, CITY)])]);
            if text == "Södertälje" {
                save(&image, "e10-sodertalje.png");
            }
            let [ax, ay] = screen(&vf, STOCKHOLM);
            let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
            let laid = atlas.layout(Face::LABEL, text, &TextLayout::DEFAULT);
            let pen = [ax + laid.box_px(14.0)[0], ay + atlas.baseline_offset(Face::LABEL) * S14];
            let outlines = outline_box(text, 14.0, pen);
            let size = [image.width as usize, image.height as usize];
            let ours = profile_box(&fill_coverage(&image), size, 1.0);
            eprintln!("{text}: ink box {ours:?}, the outlines' {outlines:?}");
            for (side, (a, b)) in ["left", "top", "right", "bottom"].iter().zip(ours.iter().zip(&outlines)) {
                assert!((a - b).abs() <= 0.75, "{text}: the ink box's {side} is {a}, the outlines' {b}");
            }
        }
    }

    /// A billboard label stays level on the screen when the map turns, unless it has an angle
    /// (anticlockwise); a ground-plane label (`billboard: false`) reads along east and turns with
    /// the map, on the globe and on the flat map.
    #[test]
    fn labels_turn_with_their_angle_or_the_ground() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        // a wide text: its ink box is wide when it reads across the screen, tall when up it
        let wide = |ink: [f64; 4]| (ink[2] - ink[0]) > 3.0 * (ink[3] - ink[1]);
        let tall = |ink: [f64; 4]| (ink[3] - ink[1]) > 3.0 * (ink[2] - ink[0]);
        for zoom in [6.0, 14.0] {
            for (billboard, bearing, angle, reads_across) in
                [(true, 0.0, 0.0, true), (true, 90.0, 0.0, true), (true, 0.0, 90.0, false), (false, 0.0, 0.0, true), (false, 90.0, 0.0, false), (false, 90.0, 90.0, true)]
            {
                let style = TextStyle { billboard, ..TextStyle::LABELS };
                let label = Spec { angle, ..Spec::new(STOCKHOLM, "Stockholm", 14.0, CITY) };
                let v = view(STOCKHOLM, zoom, 0.0, bearing);
                let (image, _) = bench.shoot(&gpu, v, 1.0, vec![Draw { style, ..Draw::labels(vec![label]) }]);
                let (columns, rows) = whole(&image);
                let ink = ink_box(&image, 1.0, columns, rows, fill).expect("drawn");
                let about = format!("zoom {zoom} billboard {billboard} bearing {bearing} angle {angle}: ink {ink:?}");
                assert!(if reads_across { wide(ink) } else { tall(ink) }, "{about}");
                // still the label's size: about as long as it is wide level
                let long = (ink[2] - ink[0]).max(ink[3] - ink[1]);
                assert!((long - 66.0).abs() < 2.5, "{about}");
            }
        }
    }

    /// Anchors `start`, `middle` and `end` put the text's box after, round and before its anchor,
    /// and a line break starts a row one line (the size) lower; the ink sits in the box by the
    /// font's side bearings.
    #[test]
    fn anchors_and_line_breaks_place_the_text() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        let canvas = FontCanvas::new(Face::LABEL, 64.0);
        let s = canvas.measure_text('S');
        let m = canvas.measure_text('m');
        let (lsb, rsb) = (-s.actual_bounding_box_left * S14, (m.width - m.actual_bounding_box_right) * S14);
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let v = view(STOCKHOLM, 9.0, 0.0, 0.0);
        for (anchor, baseline) in [(TextAnchor::Start, AlignmentBaseline::Top), (TextAnchor::Middle, AlignmentBaseline::Center), (TextAnchor::End, AlignmentBaseline::Bottom)] {
            let layout = TextLayout { anchor, baseline, ..TextLayout::DEFAULT };
            let spec = Spec { layout, ..Spec::new(STOCKHOLM, "Stockholm", 14.0, CITY) };
            let (image, vf) = bench.shoot(&gpu, v, 1.0, vec![Draw::labels(vec![spec])]);
            let [ax, ay] = screen(&vf, STOCKHOLM);
            let (columns, rows) = whole(&image);
            let ink = ink_box(&image, 1.0, columns, rows, fill).expect("drawn");
            let [bx, by, bw, bh] = atlas.layout(Face::LABEL, "Stockholm", &layout).box_px(14.0);
            let about = format!("{anchor:?} {baseline:?}");
            eprintln!("{about}: ink {ink:?}, box from {:?} to {:?}", [ax + bx, ay + by], [ax + bx + bw, ay + by + bh]);
            // within a pixel: the ink box is whole pixels, and a pixel at a curve's extreme is
            // less than half covered
            assert!(((ink[0] - (ax + bx)) - lsb).abs() <= 1.0, "{about}: left bearing {}, the font's {lsb}", ink[0] - (ax + bx));
            assert!((((ax + bx + bw) - ink[2]) - rsb).abs() <= 1.0, "{about}: right bearing {}, the font's {rsb}", (ax + bx + bw) - ink[2]);
            // the ink's top is the cap height above the baseline, which is 16 atlas px below the
            // box's middle
            let baseline_y = ay + by + bh / 2.0 + 16.0 * S14;
            assert!(((baseline_y - ink[1]) - s.actual_bounding_box_ascent.max(46.2) * S14).abs() <= 0.75, "{about}: ink top {}", ink[1]);
            assert!((ink[3] - baseline_y).abs() <= 0.75, "{about}: ink bottom {}", ink[3]);
            match anchor {
                TextAnchor::Start => assert!((bx, by) == (0.0, 0.0)),
                TextAnchor::Middle => assert!((bx + bw / 2.0, by + bh / 2.0) == (0.0, 0.0)),
                TextAnchor::End => assert!((bx + bw, by + bh) == (0.0, 0.0)),
            }
        }

        // two rows, centred: their baselines 14 px apart either side of the anchor
        let spec = Spec::new(STOCKHOLM, "Stock\nholm", 14.0, CITY);
        let (image, vf) = bench.shoot(&gpu, v, 1.0, vec![Draw::labels(vec![spec])]);
        save(&image, "e10-two-rows.png");
        let [ax, ay] = screen(&vf, STOCKHOLM);
        // the upper row's baseline is 3.5 px above the anchor, the lower row's ascenders reach
        // 0.4 px above it
        let split = (ay - 1.0).round() as u32;
        let upper = ink_box(&image, 1.0, 0..image.width, 0..split, fill).expect("Stock");
        let lower = ink_box(&image, 1.0, 0..image.width, split..image.height, fill).expect("holm");
        eprintln!("rows: {upper:?} {lower:?}, anchor {ax},{ay}");
        // baselines at the anchor −16 and +48 atlas px (rows 64 apart, the pair centred)
        assert!((upper[3] - (ay - 16.0 * S14)).abs() <= 0.75, "upper baseline {}", upper[3]);
        assert!((lower[3] - (ay + 48.0 * S14)).abs() <= 0.75, "lower baseline {}", lower[3]);
        assert!(((lower[1] - upper[1]) - 14.0).abs() <= 0.75, "rows {} apart", lower[1] - upper[1]);
        // each row centred on the anchor on its own, its ink a side bearing in
        let (uw, lw) = (atlas.layout(Face::LABEL, "Stock", &TextLayout::DEFAULT).box_px(14.0), atlas.layout(Face::LABEL, "holm", &TextLayout::DEFAULT).box_px(14.0));
        let lsb_h = -canvas.measure_text('h').actual_bounding_box_left * S14;
        assert!((upper[0] - (ax + uw[0]) - lsb).abs() <= 1.0, "the upper row's ink starts at {}, its box at {}", upper[0], ax + uw[0]);
        assert!((lower[0] - (ax + lw[0]) - lsb_h).abs() <= 1.0, "the lower row's ink starts at {}, its box at {}", lower[0], ax + lw[0]);
    }

    /// A background box is the text's box grown by the padding, in its colour, under the text; a
    /// stroked and rounded one has its border inside its edge and its corners cut. The plain
    /// (non-SDF) atlas draws the text in its colour, with no halo. Signal-name style: Overpass Mono
    /// 10 px, start anchor, 10 px to the right, padding `[3, 1]`.
    #[test]
    fn backgrounds_hold_the_text_and_its_padding() {
        let Some(gpu) = gpu() else { return };
        let mut bench = Bench::new(&gpu);
        let face = Face::new(Family::OverpassMono, 400);
        let text = [235, 238, 245, 255];
        let fill = [16, 18, 24, 200];
        let mut atlas = GlyphAtlas::new(FontSettings::DEFAULT);
        let [bx, by, bw, bh] = atlas.layout(face, "A12", &TextLayout::START).box_px(10.0);
        let v = view(STOCKHOLM, 12.0, 0.0, 0.0);
        let spec = Spec { face, layout: TextLayout::START, pixel_offset: [10.0, 0.0], background: Some((fill, [255, 255, 255, 255], 0.0)), ..Spec::new(STOCKHOLM, "A12", 10.0, text) };
        let draw = Draw { font: FontSettings::DEFAULT, style: TextStyle::DEFAULT, background: BackgroundStyle::DEFAULT.padded(&[3.0, 1.0]), labels: vec![spec.clone()] };
        for scale in [1.0, 2.0] {
            let (image, vf) = bench.shoot(&gpu, v, scale, vec![draw.clone()]);
            save(&image, &format!("e10-background-x{scale}.png"));
            let [ax, ay] = screen(&vf, STOCKHOLM);
            let (columns, rows) = whole(&image);
            // the box: more than half covered by the fill (its red 30 → 19)
            let boxed = ink_box(&image, scale, columns.clone(), rows.clone(), |p| p[0] < 25 || p[0] > 60).expect("a box");
            let expected = [ax + 10.0 + bx - 3.0, ay + by - 1.0, ax + 10.0 + bx + bw + 3.0, ay + by + bh + 1.0];
            eprintln!("scale {scale}: box {boxed:?}, expected {expected:?}");
            for (side, (a, b)) in ["left", "top", "right", "bottom"].iter().zip(boxed.iter().zip(&expected)) {
                assert!((a - b).abs() <= 0.75, "scale {scale}: the box's {side} is {a}, expected {b}");
            }
            // inside, away from the text, the fill over the map
            let inside = image.pixel(((expected[0] + 1.5) * scale) as u32, ((ay + by + bh / 2.0) * scale) as u32);
            let over = |c: u8| (f64::from(c) * 200.0 / 255.0 + INK * 55.0 / 255.0).round() as u8;
            for c in 0..3 {
                assert!(inside[c].abs_diff(over(fill[c])) <= 1, "scale {scale}: inside {inside:?}");
            }
            // the text in its colour over the box: plain coverage averaged down the mip levels
            // (64 px glyphs at 10 px) leaves its thin strokes partly covered, but the brightest
            // pixel is the same mix of text and box in every channel
            let brightest = (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y))).map(|(x, y)| image.pixel(x, y)).max_by_key(|p| p[1]).expect("pixels");
            let k: Vec<f64> = (0..3).map(|c| (f64::from(brightest[c]) - f64::from(over(fill[c]))) / (f64::from(text[c]) - f64::from(over(fill[c])))).collect();
            let mean = k_mean(&k);
            assert!(k.iter().all(|&c| c > 0.5 && (c - mean).abs() < 0.03), "scale {scale}: the text is {brightest:?} ({k:?} of the way from the box)");
            let darkest = (0..image.height).flat_map(|y| (0..image.width).map(move |x| (x, y))).map(|(x, y)| image.pixel(x, y)).min_by_key(|p| p[0]).expect("pixels");
            assert!(darkest[0] >= over(fill[0]) - 1, "scale {scale}: darkest {darkest:?}");
            // the glyphs sit inside the box's padding
            let glyphs = ink_box(&image, scale, columns, rows, |p| p[1] > 128).expect("text");
            assert!(glyphs[0] >= boxed[0] + 2.0 && glyphs[2] <= boxed[2] - 2.0, "scale {scale}: text {glyphs:?} in {boxed:?}");
        }

        // stroked, 2 px of white inside the edge, corners rounded by 4, padding 8 and 4
        let stroked = Spec { background: Some((fill, [255, 255, 255, 255], 2.0)), ..spec };
        let draw = Draw { background: BackgroundStyle { stroked: true, ..BackgroundStyle::DEFAULT.padded(&[8.0, 4.0]).rounded(4.0) }, labels: vec![stroked], ..draw };
        let (image, vf) = bench.shoot(&gpu, v, 1.0, vec![draw]);
        save(&image, "e10-background-stroked.png");
        let [ax, ay] = screen(&vf, STOCKHOLM);
        let (l, t, r, b) = (ax + 10.0 + bx - 8.0, ay + by - 4.0, ax + 10.0 + bx + bw + 8.0, ay + by + bh + 4.0);
        let at = |x: f64, y: f64| image.pixel(x as u32, y as u32);
        // the border: white a pixel inside each edge, half way along
        let (mid_x, mid_y) = ((l + r) / 2.0, (t + b) / 2.0);
        for (x, y) in [(l + 0.5, mid_y), (r - 0.5, mid_y), (mid_x, t + 0.5), (mid_x, b - 0.5)] {
            assert!(at(x, y)[0] > 200, "border at {x},{y}: {:?}", at(x, y));
        }
        // the corners are cut off: the map, or nearly
        for (x, y) in [(l + 0.2, t + 0.2), (r - 0.5, t + 0.2), (l + 0.2, b - 0.5), (r - 0.5, b - 0.5)] {
            assert!(at(x, y).iter().take(3).all(|&c| c.abs_diff(30) <= 3), "corner at {x},{y}: {:?}", at(x, y));
        }
        // between the border and the text, the fill
        assert!(at(l + 4.5, mid_y)[0] < 25, "fill {:?}", at(l + 4.5, mid_y));
    }

    fn k_mean(k: &[f64]) -> f64 {
        k.iter().sum::<f64>() / k.len() as f64
    }

    #[test]
    fn layouts_match_the_wgsl() {
        let (span, members) = wgsl_members(SHADER, "TextStyle");
        assert_eq!(span as usize, size_of::<StyleUniform>());
        let expected = [
            ("size_scale", std::mem::offset_of!(StyleUniform, size_scale)),
            ("size_min_pixels", std::mem::offset_of!(StyleUniform, size_min_pixels)),
            ("size_max_pixels", std::mem::offset_of!(StyleUniform, size_max_pixels)),
            ("font_size", std::mem::offset_of!(StyleUniform, font_size)),
            ("atlas_size", std::mem::offset_of!(StyleUniform, atlas_size)),
            ("billboard", std::mem::offset_of!(StyleUniform, billboard)),
            ("size_units", std::mem::offset_of!(StyleUniform, size_units)),
            ("alpha_cutoff", std::mem::offset_of!(StyleUniform, alpha_cutoff)),
            ("opacity", std::mem::offset_of!(StyleUniform, opacity)),
            ("focal_w", std::mem::offset_of!(StyleUniform, focal_w)),
            ("sdf", std::mem::offset_of!(StyleUniform, sdf)),
            ("buffer", std::mem::offset_of!(StyleUniform, buffer)),
            ("gamma", std::mem::offset_of!(StyleUniform, gamma)),
            ("outline_buffer", std::mem::offset_of!(StyleUniform, outline_buffer)),
            ("pad0", std::mem::offset_of!(StyleUniform, pad0)),
            ("outline_color", std::mem::offset_of!(StyleUniform, outline_color)),
            ("centre", std::mem::offset_of!(StyleUniform, centre)),
            ("pad1", std::mem::offset_of!(StyleUniform, pad1)),
        ];
        assert_eq!(members, expected.map(|(n, o)| (n.to_string(), o as u32)));
        let (span, members) = wgsl_members(SHADER, "BackgroundStyle");
        assert_eq!(span as usize, size_of::<BackgroundUniform>());
        let expected = [
            ("size_scale", std::mem::offset_of!(BackgroundUniform, size_scale)),
            ("size_min_pixels", std::mem::offset_of!(BackgroundUniform, size_min_pixels)),
            ("size_max_pixels", std::mem::offset_of!(BackgroundUniform, size_max_pixels)),
            ("font_size", std::mem::offset_of!(BackgroundUniform, font_size)),
            ("border_radius", std::mem::offset_of!(BackgroundUniform, border_radius)),
            ("padding", std::mem::offset_of!(BackgroundUniform, padding)),
            ("billboard", std::mem::offset_of!(BackgroundUniform, billboard)),
            ("size_units", std::mem::offset_of!(BackgroundUniform, size_units)),
            ("stroked", std::mem::offset_of!(BackgroundUniform, stroked)),
            ("opacity", std::mem::offset_of!(BackgroundUniform, opacity)),
            ("focal_w", std::mem::offset_of!(BackgroundUniform, focal_w)),
            ("pad0", std::mem::offset_of!(BackgroundUniform, pad0)),
            ("pad1", std::mem::offset_of!(BackgroundUniform, pad0) + 4),
            ("pad2", std::mem::offset_of!(BackgroundUniform, pad0) + 8),
            ("centre", std::mem::offset_of!(BackgroundUniform, centre)),
            ("pad3", std::mem::offset_of!(BackgroundUniform, pad1)),
        ];
        assert_eq!(members, expected.map(|(n, o)| (n.to_string(), o as u32)));
        // the instance attributes sit where the structs keep their fields
        let offsets: Vec<u64> = Glyph::LAYOUT.attributes.iter().map(|a| a.offset).collect();
        let fields = [
            std::mem::offset_of!(Glyph, position),
            std::mem::offset_of!(Glyph, size),
            std::mem::offset_of!(Glyph, frame),
            std::mem::offset_of!(Glyph, offset),
            std::mem::offset_of!(Glyph, pixel_offset),
            std::mem::offset_of!(Glyph, angle),
            std::mem::offset_of!(Glyph, color),
        ];
        assert_eq!(offsets, fields.map(|f| f as u64));
        assert_eq!(Glyph::LAYOUT.array_stride as usize, size_of::<Glyph>());
        let offsets: Vec<u64> = Background::LAYOUT.attributes.iter().map(|a| a.offset).collect();
        let fields = [
            std::mem::offset_of!(Background, position),
            std::mem::offset_of!(Background, size),
            std::mem::offset_of!(Background, rect),
            std::mem::offset_of!(Background, pixel_offset),
            std::mem::offset_of!(Background, angle),
            std::mem::offset_of!(Background, line_width),
            std::mem::offset_of!(Background, fill_color),
            std::mem::offset_of!(Background, line_color),
        ];
        assert_eq!(offsets, fields.map(|f| f as u64));
        assert_eq!(Background::LAYOUT.array_stride as usize, size_of::<Background>());
    }

    #[test]
    fn outline_passes_follow_decks_draw() {
        let vf = ViewFrame::new(&ViewState::INITIAL, SIZE, 1.0, 0.0);
        // the labels: outline 3 of radius 12 is 0.25, so the halo's edge is at 0.75·0.75
        let (first, second) = StyleUniform::passes(&TextStyle::LABELS, &FontSettings::LABELS, [2048, 2048], &vf);
        assert_eq!((first.sdf, first.buffer, first.gamma, first.outline_buffer), (1, 0.75, 0.1, 0.5625));
        assert_eq!(first.outline_color, LABEL_OUTLINE.map(|c| f32::from(c) / 255.0));
        assert_eq!((first.font_size, first.alpha_cutoff, first.size_units, first.billboard), (64.0, 0.001, 2, 1));
        let second = second.expect("an SDF outline draws the fill again");
        assert_eq!(second, StyleUniform { outline_buffer: 0.75, ..first });
        // a thick outline stops at the smoothing
        let thick = TextStyle { outline_width: 12.0, ..TextStyle::LABELS };
        assert_eq!(StyleUniform::passes(&thick, &FontSettings::LABELS, [8, 8], &vf).0.outline_buffer, 0.1);
        // no outline: one draw
        let (plain, none) = StyleUniform::passes(&TextStyle::DEFAULT, &FontSettings::LABELS, [8, 8], &vf);
        assert_eq!((plain.outline_buffer, none), (-1.0, None));
        // no SDF: one draw, and the shader skips the outline
        let (mask, none) = StyleUniform::passes(&TextStyle::LABELS, &FontSettings::DEFAULT, [8, 8], &vf);
        assert_eq!((mask.sdf, none), (0, None));
        // the label size setting
        assert_eq!(StyleUniform::passes(&TextStyle::LABELS.scaled(1.25), &FontSettings::LABELS, [8, 8], &vf).0.size_scale, 1.25);
    }

    #[test]
    fn background_padding_and_radius_as_deck_reads_them() {
        assert_eq!(BackgroundStyle::DEFAULT.padded(&[3.0, 1.0]).padding, [3.0, 1.0, 3.0, 1.0]);
        assert_eq!(BackgroundStyle::DEFAULT.padded(&[1.0, 2.0, 3.0, 4.0]).padding, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(BackgroundStyle::DEFAULT.rounded(4.0).border_radius, [4.0; 4]);
        let u = BackgroundUniform::props(&TextStyle::DEFAULT, &BackgroundStyle { stroked: true, ..BackgroundStyle::DEFAULT }, &FontSettings::DEFAULT);
        assert_eq!((u.stroked, u.font_size, u.billboard), (1, 64.0, 1));
    }

    /// Each mip level updated over a changed region equals the whole chain made again.
    #[test]
    fn mips_follow_the_atlas() {
        let size = [64, 40];
        let sizes = mip_sizes(size);
        assert_eq!(sizes, vec![[64, 40], [32, 20], [16, 10], [8, 5], [4, 2], [2, 1], [1, 1]]);
        let mut level0 = vec![0u8; 64 * 40];
        let mut levels: Vec<Vec<u8>> = sizes[1..].iter().map(|[w, h]| vec![0; (w * h) as usize]).collect();
        let mut seed = 7u32;
        for region in [[3, 5, 10, 7], [60, 0, 4, 40], [0, 39, 64, 1], [17, 17, 1, 1]] {
            let [x, y, w, h] = region;
            for ty in y..y + h {
                for tx in x..x + w {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    level0[(ty * 64 + tx) as usize] = (seed >> 16) as u8;
                }
            }
            update_mips(&level0, size, &mut levels, region);
            // the whole chain from scratch
            let mut fresh: Vec<Vec<u8>> = sizes[1..].iter().map(|[w, h]| vec![0; (w * h) as usize]).collect();
            update_mips(&level0, size, &mut fresh, [0, 0, 64, 40]);
            assert_eq!(levels, fresh, "after {region:?}");
        }
        // a texel is the rounded mean of the four above it
        let mut one = vec![vec![0u8; 1]];
        update_mips(&[10, 11, 12, 14], [2, 2], &mut one, [0, 0, 2, 2]);
        assert_eq!(one[0], vec![12]);
    }

    #[test]
    fn batches_grow_and_shrink() {
        let Some(gpu) = gpu() else { return };
        let renderer = MapRenderer::new(&gpu, FORMAT);
        let init = renderer.init(&gpu);
        let pipeline = TextPipeline::new(&init, "test text", DrawState::LABEL);
        let mut atlas = GlyphAtlas::with_size(FontSettings::LABELS, [256, 256]);
        let mut texture = AtlasTexture::new(&gpu, "atlas", &mut atlas);
        let mut batch = pipeline.batch(&gpu.device, "test", &texture);
        assert!(batch.is_empty());
        let vf = ViewFrame::new(&view(STOCKHOLM, 10.0, 0.0, 0.0), SIZE, 1.0, 0.0);
        let laid = atlas.layout(Face::LABEL, "Arlanda", &TextLayout::DEFAULT);
        assert!(texture.sync(&gpu, &mut atlas), "new glyphs uploaded");
        assert!(!texture.sync(&gpu, &mut atlas), "nothing new");
        let label = Label::at(&vf, [17.92, 59.65, 0.0], 12.0, [55, 142, 240, 255]);
        let glyphs: Vec<Glyph> = label.glyphs(&laid).collect();
        assert_eq!(glyphs.len(), 7);
        batch.set_glyphs(&gpu.device, &gpu.queue, &glyphs);
        batch.set_backgrounds(&gpu.device, &gpu.queue, &[label.background(&laid, [0; 4])]);
        assert_eq!((batch.len(), batch.background_len()), (7, 1));
        assert!(batch.glyphs.size() >= (7 * size_of::<Glyph>()) as u64);
        batch.set_style(&gpu.queue, &TextStyle::LABELS, &BackgroundStyle::DEFAULT, &FontSettings::LABELS, &vf);
        assert_eq!(batch.passes(), 2);
        batch.set_style(&gpu.queue, &TextStyle::DEFAULT, &BackgroundStyle::DEFAULT, &FontSettings::LABELS, &vf);
        assert_eq!((batch.passes(), batch.style().outline_buffer), (1, -1.0));
        batch.set_glyphs(&gpu.device, &gpu.queue, &glyphs[..2]);
        batch.set_backgrounds(&gpu.device, &gpu.queue, &[]);
        assert_eq!((batch.len(), batch.background_len()), (2, 0));
        batch.set_glyphs(&gpu.device, &gpu.queue, &[]);
        assert!(batch.is_empty());
    }

    #[test]
    #[should_panic(expected = "culls nothing")]
    fn a_culling_preset_is_refused() {
        let Some(gpu) = gpu() else { panic!("skipped: culls nothing") };
        let renderer = MapRenderer::new(&gpu, FORMAT);
        let init = renderer.init(&gpu);
        TextPipeline::new(&init, "test text", DrawState::DEFAULT);
    }
}
