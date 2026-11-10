//! Textures from KTX2 files (plan 1.4.13; doc 04 §5, doc 03 §4.4; `ktx2.ts`, `ktx2-transcode.ts`,
//! `realistic-textures.ts`).
//!
//! The files (`scripts/compress-textures.mjs` makes them) hold Basis Universal data: UASTC with
//! Zstandard, or ETC1S (BasisLZ) for the two Earth pictures. `basisu_c_sys` turns them, on the
//! worker pool ([`Ktx2Texture`], [`RealisticTextures`]), into one format the GPU samples as it is,
//! with the files' own mip levels:
//!
//! - [`Target::for_features`]: BC7 where there is BC (desktops; first, as some Linux drivers offer
//!   ASTC but decode it on the CPU), else ASTC 4×4 (phones, Apple's), else ETC2 (older Android),
//!   else plain RGBA8. The target is always asked for explicitly, never left to the crate's own
//!   choice, which for ETC1S would skip ASTC and give an ASTC-only device RGBA8.
//! - [`transcode`] is `ktx2-transcode.ts`'s: every level, every layer of it, layer after layer.
//!   [`transcode_with`] also leaves out the top levels larger than the device takes
//!   ([`levels_to_skip`]: the 4096-pixel Earth pictures drop their top level on a 2048 device) and
//!   stops early when its job is cancelled.
//! - [`Transcoded::upload`] makes a 2D or 2D-array texture with exactly the levels it has.
//! - Until a file has loaded, a 1×1 [`stand_in`] is bound (doc 04 §5.3); the realistic set swaps
//!   all of its textures in at once, and keeps its stand-ins if any file fails.
//!
//! The device must have been created with the [`compression_features`] its adapter offers, or every
//! target is RGBA8 (about four times the memory: some 110 MB for the realistic set).

use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;

use basisu_c_sys::common as tf;
use basisu_c_sys::extra::basisu_transcoder_init;
use basisu_c_sys::transcoder as bt;
use wgpu::util::DeviceExt as _;

use crate::jobs::{Cancel, Job, JobPoll, Pool};

/// A file's bytes: embedded (`include_bytes!`) or read at run time.
pub type Bytes = Cow<'static, [u8]>;

/// A KTX2 file's first bytes (Tauri answered a missing file with the app's page, as found).
pub const KTX2_MAGIC: [u8; 12] = [0xab, 0x4b, 0x54, 0x58, 0x20, 0x32, 0x30, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a];

/// Whether `bytes` start as a KTX2 file does.
pub fn is_ktx2(bytes: &[u8]) -> bool {
    bytes.starts_with(&KTX2_MAGIC)
}

/// The pool priority of a texture job: with the nearest selected tiles (lower runs first).
pub const PRIORITY: f64 = 0.0;

/// What the textures are transcoded to: the transcoder's formats the app uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// BC7 (`bc7-rgba-unorm[-srgb]`), transcoder format 6.
    Bc7,
    /// ASTC 4×4 LDR (`astc-4x4-unorm[-srgb]`), transcoder format 10.
    Astc,
    /// ETC2 RGBA8 with EAC alpha (`etc2-rgba8unorm[-srgb]`), transcoder format 1: for Android
    /// devices without ASTC (the web build skipped it for a luma.gl bug only).
    Etc2,
    /// Plain pixels (`rgba8unorm[-srgb]`), transcoder format 13.
    Rgba,
}

impl Target {
    /// Every target, in the order of preference.
    pub const ALL: [Target; 4] = [Target::Bc7, Target::Astc, Target::Etc2, Target::Rgba];

    /// The transcoder's `transcoder_texture_format` (`TARGET` of `ktx2-transcode.ts`, plus ETC2).
    pub const fn transcoder_format(self) -> u32 {
        match self {
            Target::Bc7 => tf::TF_BC7_RGBA,
            Target::Astc => tf::TF_ASTC_LDR_4X4_RGBA,
            Target::Etc2 => tf::TF_ETC2_RGBA,
            Target::Rgba => tf::TF_RGBA32,
        }
    }

    /// The best target a device with `features` samples: BC7, then ASTC, then ETC2, then RGBA8.
    pub fn for_features(features: wgpu::Features) -> Target {
        Target::ALL.into_iter().find(|t| features.contains(t.feature())).unwrap_or(Target::Rgba)
    }

    /// The device feature the target needs (none for RGBA8).
    pub const fn feature(self) -> wgpu::Features {
        match self {
            Target::Bc7 => wgpu::Features::TEXTURE_COMPRESSION_BC,
            Target::Astc => wgpu::Features::TEXTURE_COMPRESSION_ASTC,
            Target::Etc2 => wgpu::Features::TEXTURE_COMPRESSION_ETC2,
            Target::Rgba => wgpu::Features::empty(),
        }
    }

    /// The texture format: its colours in sRGB (`srgb`, filtered in linear light), or data.
    pub fn format(self, srgb: bool) -> wgpu::TextureFormat {
        use wgpu::TextureFormat as F;
        match (self, srgb) {
            (Target::Bc7, false) => F::Bc7RgbaUnorm,
            (Target::Bc7, true) => F::Bc7RgbaUnormSrgb,
            (Target::Astc, false) => F::Astc { block: wgpu::AstcBlock::B4x4, channel: wgpu::AstcChannel::Unorm },
            (Target::Astc, true) => F::Astc { block: wgpu::AstcBlock::B4x4, channel: wgpu::AstcChannel::UnormSrgb },
            (Target::Etc2, false) => F::Etc2Rgba8Unorm,
            (Target::Etc2, true) => F::Etc2Rgba8UnormSrgb,
            (Target::Rgba, false) => F::Rgba8Unorm,
            (Target::Rgba, true) => F::Rgba8UnormSrgb,
        }
    }

    /// Whether the target stores 4×4 blocks (all but RGBA8).
    pub const fn is_compressed(self) -> bool {
        !matches!(self, Target::Rgba)
    }
}

/// The compression features to ask a device for, of those its adapter offers (`available`): the
/// baseline needs none (plan 1.4.13), but without them every texture is RGBA8.
pub fn compression_features(available: wgpu::Features) -> wgpu::Features {
    available
        & (wgpu::Features::TEXTURE_COMPRESSION_BC
            | wgpu::Features::TEXTURE_COMPRESSION_ASTC
            | wgpu::Features::TEXTURE_COMPRESSION_ETC2)
}

/// How a device takes textures: the target and its largest texture side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadOptions {
    pub target: Target,
    /// Top levels with a side above this are left out ([`levels_to_skip`]).
    pub max_size: u32,
}

impl LoadOptions {
    /// What `device` takes: [`Target::for_features`] its features, its `max_texture_dimension_2d`.
    pub fn for_device(device: &wgpu::Device) -> LoadOptions {
        LoadOptions { target: Target::for_features(device.features()), max_size: device.limits().max_texture_dimension_2d }
    }
}

/// Why a file gave no texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureError {
    /// The bytes are no KTX2 file (Tauri's page for a missing one).
    NotKtx2,
    /// A KTX2 file the transcoder cannot read (not Basis Universal, or broken).
    Unreadable,
    /// The file's data cannot become the target.
    Unsupported(Target),
    /// A level of a layer did not transcode (a truncated file reads, then fails here).
    Level { level: u32, layer: u32 },
    /// The job was cancelled.
    Cancelled,
    /// The job was lost (it panicked, or its pool went away).
    Lost,
    /// No bytes for the file.
    Missing(&'static str),
}

impl fmt::Display for TextureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TextureError::NotKtx2 => f.write_str("not a KTX2 file"),
            TextureError::Unreadable => f.write_str("not a texture the transcoder reads"),
            TextureError::Unsupported(target) => write!(f, "cannot transcode to {target:?}"),
            TextureError::Level { level, layer } => write!(f, "level {level} of layer {layer} didn't transcode"),
            TextureError::Cancelled => f.write_str("transcoding cancelled"),
            TextureError::Lost => f.write_str("transcoding job lost"),
            TextureError::Missing(path) => write!(f, "{path}: no such file"),
        }
    }
}

impl std::error::Error for TextureError {}

/// One mip level: its size and its texels, all the layers in one, layer after layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// A file's texels in the target's format, for each mip level kept (as `Texture.writeData` took
/// them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcoded {
    /// The size of the first level kept (the file's own size unless top levels were skipped).
    pub width: u32,
    pub height: u32,
    /// The file's layers (1 for a plain 2D texture).
    pub layers: u32,
    /// What the texels are; RGBA8 when the target asked for was compressed and the first level
    /// kept is not made of whole 4×4 blocks.
    pub target: Target,
    /// The file's top levels left out.
    pub skipped: u32,
    pub levels: Vec<Level>,
}

/// What a file holds, from its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub levels: u32,
    /// 0 for a plain 2D texture.
    pub layers: u32,
    pub srgb: bool,
    /// ETC1S (BasisLZ) rather than UASTC.
    pub etc1s: bool,
}

/// An open KTX2 file in the transcoder, which reads the bytes where they are.
struct File<'a> {
    handle: u64,
    bytes: PhantomData<&'a [u8]>,
}

impl<'a> File<'a> {
    /// The file, ready to transcode; `None` when the transcoder cannot read it.
    fn open(bytes: &'a [u8]) -> Option<File<'a>> {
        basisu_transcoder_init();
        let len = u32::try_from(bytes.len()).ok()?;
        // SAFETY: the transcoder reads `len` bytes at that address and keeps pointing at them until
        // `bt_ktx2_close`; `File` borrows them for as long and closes in `drop`.
        let handle = unsafe { bt::bt_ktx2_open(bytes.as_ptr().addr() as u64, len) };
        if handle == 0 {
            return None;
        }
        let file = File { handle, bytes: PhantomData };
        // SAFETY: an open handle.
        unsafe { bt::bt_ktx2_start_transcoding(file.handle) }.is_ok().then_some(file)
    }

    fn info(&self) -> Info {
        // SAFETY: getters on an open handle.
        unsafe {
            Info {
                width: bt::bt_ktx2_get_width(self.handle),
                height: bt::bt_ktx2_get_height(self.handle),
                levels: bt::bt_ktx2_get_levels(self.handle),
                layers: bt::bt_ktx2_get_layers(self.handle),
                srgb: bt::bt_ktx2_is_srgb(self.handle).is_ok(),
                etc1s: bt::bt_ktx2_is_etc1s(self.handle).is_ok(),
            }
        }
    }

    /// Whether the file's data can become `target`.
    fn supports(&self, target: Target) -> bool {
        // SAFETY: getters on an open handle; plain queries on format numbers.
        unsafe {
            let format = bt::bt_ktx2_get_basis_tex_format(self.handle);
            bt::bt_basis_is_format_supported(target.transcoder_format(), format).is_ok()
        }
    }

    /// The size of `level` (`origWidth`, `origHeight` of layer 0, face 0).
    fn level_size(&self, level: u32) -> (u32, u32) {
        // SAFETY: getters on an open handle; `level` is below the file's level count.
        unsafe {
            (
                bt::bt_ktx2_get_level_orig_width(self.handle, level, 0, 0),
                bt::bt_ktx2_get_level_orig_height(self.handle, level, 0, 0),
            )
        }
    }

    /// Transcodes one layer of `level` into `out`, which holds exactly its bytes in `target`.
    fn transcode(&self, level: u32, layer: u32, target: Target, out: &mut [u8]) -> bool {
        let format = target.transcoder_format();
        // SAFETY: an open handle; `out` is writable for its whole length, which is the level's
        // transcoded size, i.e. `blocks` blocks (or pixels) of that many bytes each.
        unsafe {
            let unit = bt::bt_basis_get_bytes_per_block_or_pixel(format) as usize;
            let Ok(blocks) = u32::try_from(out.len() / unit) else { return false };
            bt::bt_ktx2_transcode_image_level(
                self.handle,
                level,
                layer,
                0,
                out.as_mut_ptr().addr() as u64,
                blocks,
                format,
                0,
                0,
                0,
                -1,
                -1,
                0,
            )
            .is_ok()
        }
    }
}

impl Drop for File<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle is open and closed once.
        unsafe { bt::bt_ktx2_close(self.handle) };
    }
}

/// The bytes of one layer of a `width` × `height` level in `target`.
pub fn level_bytes(target: Target, width: u32, height: u32) -> usize {
    // SAFETY: a plain computation on format numbers.
    unsafe { bt::bt_basis_compute_transcoded_image_size_in_bytes(target.transcoder_format(), width, height) as usize }
}

/// What a file holds, from its header, without transcoding it.
pub fn info(bytes: &[u8]) -> Result<Info, TextureError> {
    if !is_ktx2(bytes) {
        return Err(TextureError::NotKtx2);
    }
    File::open(bytes).map(|file| file.info()).ok_or(TextureError::Unreadable)
}

/// How many top levels of a `width` × `height` texture of `levels` levels to leave out so that no
/// side of the first level kept is above `max_size` (level `i` is `max(1, size >> i)`); never the
/// last level.
pub fn levels_to_skip(width: u32, height: u32, levels: u32, max_size: u32) -> u32 {
    let side = |level: u32| (width >> level).max(height >> level).max(1);
    let mut skip = 0;
    while skip + 1 < levels && side(skip) > max_size {
        skip += 1;
    }
    skip
}

/// `ktx2-transcode.ts`'s `transcode`: every level of the file, every layer of each, in `target`.
pub fn transcode(bytes: &[u8], target: Target) -> Result<Transcoded, TextureError> {
    transcode_with(bytes, LoadOptions { target, max_size: u32::MAX }, &Cancel::new())
}

/// [`transcode`] for a device: the top levels above `options.max_size` are left out (and never
/// transcoded), and it stops between levels once `cancel` is set.
pub fn transcode_with(bytes: &[u8], options: LoadOptions, cancel: &Cancel) -> Result<Transcoded, TextureError> {
    if !is_ktx2(bytes) {
        return Err(TextureError::NotKtx2);
    }
    let file = File::open(bytes).ok_or(TextureError::Unreadable)?;
    let info = file.info();
    if info.levels == 0 {
        return Err(TextureError::Unreadable);
    }
    let layers = info.layers.max(1);
    let skipped = levels_to_skip(info.width, info.height, info.levels, options.max_size);
    let (width, height) = file.level_size(skipped);
    // a compressed texture's first level must be whole blocks (wgpu)
    let target = if options.target.is_compressed() && (width % 4 != 0 || height % 4 != 0) { Target::Rgba } else { options.target };
    if !file.supports(target) {
        return Err(TextureError::Unsupported(target));
    }
    let mut levels = Vec::with_capacity((info.levels - skipped) as usize);
    for level in skipped..info.levels {
        if cancel.is_cancelled() {
            return Err(TextureError::Cancelled);
        }
        let (width, height) = file.level_size(level);
        let size = level_bytes(target, width, height);
        let mut data = vec![0; size * layers as usize];
        for (layer, out) in (0..layers).zip(data.chunks_exact_mut(size)) {
            if !file.transcode(level, layer, target, out) {
                return Err(TextureError::Level { level, layer });
            }
        }
        levels.push(Level { width, height, data });
    }
    Ok(Transcoded { width, height, layers, target, skipped, levels })
}

/// A texture ready to bind: its view is 2D or a 2D array, as asked for.
#[derive(Debug, Clone)]
pub struct Texture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// The size of mip level 0, in texels.
    pub size: [u32; 2],
}

impl Texture {
    fn new(texture: wgpu::Texture, array: bool) -> Texture {
        let dimension = if array { wgpu::TextureViewDimension::D2Array } else { wgpu::TextureViewDimension::D2 };
        let view = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(dimension), ..Default::default() });
        let size = [texture.width(), texture.height()];
        Texture { texture, view, size }
    }
}

impl Transcoded {
    /// The texture on `device`: a 2D array of every layer (`array`), or a plain 2D texture of the
    /// first; its colours sRGB (`srgb`) or data; exactly the levels transcoded. The device must have
    /// the target's [`Target::feature`].
    pub fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, srgb: bool, array: bool) -> Texture {
        let format = self.target.format(srgb);
        let depth = if array { self.layers } else { 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: depth },
            mip_level_count: self.levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // (COPY_SRC: read back in tests)
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let (block_width, block_height) = format.block_dimensions();
        let block = format.block_copy_size(None).expect("a colour format");
        for (mip, level) in self.levels.iter().enumerate() {
            // levels smaller than a block are written as whole blocks (their "physical" size)
            let (columns, rows) = (level.width.div_ceil(block_width), level.height.div_ceil(block_height));
            let layer_bytes = (columns * rows * block) as usize;
            debug_assert_eq!(level.data.len(), layer_bytes * self.layers as usize);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &level.data[..layer_bytes * depth as usize],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(columns * block), rows_per_image: Some(rows) },
                wgpu::Extent3d { width: columns * block_width, height: rows * block_height, depth_or_array_layers: depth },
            );
        }
        Texture::new(texture, array)
    }
}

/// The 1×1 textures bound until a file has loaded (doc 04 §5.3, `RT:102-177`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandIn {
    /// White, sRGB.
    Color,
    /// Flat: (128, 128, 255). Also the water's and the tree volume's.
    Normal,
    /// Rough, not metal, tinted all over.
    Mask,
    /// No windows lit.
    Glow,
    /// Nothing: every tree fragment is discarded until the trees' shapes arrive.
    TreeMask,
    /// Alpha 0: "not loaded, don't use" to the ground shader.
    Earth,
    /// Black.
    Night,
}

impl StandIn {
    pub const fn rgba(self) -> [u8; 4] {
        match self {
            StandIn::Color => [255, 255, 255, 255],
            StandIn::Normal => [128, 128, 255, 255],
            StandIn::Mask => [235, 0, 255, 255],
            StandIn::Glow => [0, 0, 0, 255],
            StandIn::TreeMask => [0, 0, 0, 0],
            StandIn::Earth => [0, 0, 0, 0],
            StandIn::Night => [0, 0, 0, 255],
        }
    }

    /// The colour is sRGB (filtered in linear light); the rest are data.
    pub const fn srgb(self) -> bool {
        matches!(self, StandIn::Color)
    }
}

/// A stand-in texture of `kind`: a 2D array (`array`) or a plain 2D texture. An array has two
/// layers of it: GL makes a one-layer texture a plain 2D one, which a 2D-array view cannot show
/// (sampling clamps the layer index, so every layer reads the stand-in).
pub fn stand_in(device: &wgpu::Device, queue: &wgpu::Queue, kind: StandIn, array: bool) -> Texture {
    let layers = if array { 2 } else { 1 };
    let format = if kind.srgb() { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("texture stand-in"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &kind.rgba().repeat(layers as usize),
    );
    Texture::new(texture, array)
}

/// How a file is sampled (doc 04 §5.1, doc 03 §4.4). All filter linearly, mip levels too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sampling {
    /// Repeat both ways, anisotropy 8 (`SAMPLER`, `RT:91-98`).
    Repeat,
    /// Round the world east to west, not over the poles: repeat U, clamp V, anisotropy 8.
    Globe,
    /// Clamp both ways, anisotropy 8 (the tree volume).
    Clamp,
    /// Clamp both ways, no anisotropy (the aircraft icon atlases, G:1241).
    Atlas,
}

impl Sampling {
    pub fn descriptor(self) -> wgpu::SamplerDescriptor<'static> {
        let (u, v) = match self {
            Sampling::Repeat => (wgpu::AddressMode::Repeat, wgpu::AddressMode::Repeat),
            Sampling::Globe => (wgpu::AddressMode::Repeat, wgpu::AddressMode::ClampToEdge),
            Sampling::Clamp | Sampling::Atlas => (wgpu::AddressMode::ClampToEdge, wgpu::AddressMode::ClampToEdge),
        };
        wgpu::SamplerDescriptor {
            label: Some("texture"),
            address_mode_u: u,
            address_mode_v: v,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: if self == Sampling::Atlas { 1 } else { 8 },
            ..Default::default()
        }
    }

    pub fn sampler(self, device: &wgpu::Device) -> wgpu::Sampler {
        device.create_sampler(&self.descriptor())
    }
}

/// A KTX2 file the map uses: its path under `static/` and how it is uploaded and sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureFile {
    pub path: &'static str,
    /// Colours (sRGB) or data.
    pub srgb: bool,
    /// A 2D array of its layers, or a plain 2D texture.
    pub array: bool,
    pub sampling: Sampling,
}

const fn file(path: &'static str, srgb: bool, array: bool, sampling: Sampling) -> TextureFile {
    TextureFile { path, srgb, array, sampling }
}

pub const BUILDING_COLOR: TextureFile = file("realistic/building.color.ktx2", true, true, Sampling::Repeat);
pub const BUILDING_NORMAL: TextureFile = file("realistic/building.normal.ktx2", false, true, Sampling::Repeat);
pub const BUILDING_MASK: TextureFile = file("realistic/building.mask.ktx2", false, true, Sampling::Repeat);
pub const BUILDING_GLOW: TextureFile = file("realistic/building.glow.ktx2", false, true, Sampling::Repeat);
pub const GROUND_COLOR: TextureFile = file("realistic/ground.color.ktx2", true, true, Sampling::Repeat);
pub const GROUND_NORMAL: TextureFile = file("realistic/ground.normal.ktx2", false, true, Sampling::Repeat);
pub const TREE_COLOR: TextureFile = file("realistic/tree.color.ktx2", true, true, Sampling::Repeat);
pub const TREE_NORMAL: TextureFile = file("realistic/tree.normal.ktx2", false, true, Sampling::Repeat);
pub const TREE_MASK: TextureFile = file("realistic/tree.mask.ktx2", false, true, Sampling::Repeat);
pub const WATER_NORMAL: TextureFile = file("realistic/water.normal.ktx2", false, false, Sampling::Repeat);
pub const EARTH_COLOR: TextureFile = file("realistic/earth.color.ktx2", true, false, Sampling::Globe);
pub const TREE_VOLUME_NORMAL: TextureFile = file("realistic/tree-volume.normal.ktx2", false, false, Sampling::Clamp);
pub const EARTH_NIGHT: TextureFile = file("realistic/earth.night.ktx2", true, false, Sampling::Globe);
/// White with the silhouettes in alpha: 7 × 6 cells of 128 px (doc 03 §4.4).
pub const AIRCRAFT_ICONS: TextureFile = file("aircraft/icons.ktx2", false, false, Sampling::Atlas);
/// The silhouettes grown by 9 cell pixels: 7 × 6 cells of 152 px.
pub const AIRCRAFT_ICONS_OUTLINE: TextureFile = file("aircraft/icons-outline.ktx2", false, false, Sampling::Atlas);

/// The realistic map's files, in the order [`RealisticTextures`] loads them.
pub const REALISTIC_FILES: [TextureFile; 13] = [
    BUILDING_COLOR,
    BUILDING_NORMAL,
    BUILDING_MASK,
    BUILDING_GLOW,
    GROUND_COLOR,
    GROUND_NORMAL,
    TREE_COLOR,
    TREE_NORMAL,
    TREE_MASK,
    WATER_NORMAL,
    EARTH_COLOR,
    TREE_VOLUME_NORMAL,
    EARTH_NIGHT,
];

/// The buildings' layers, in the order the shaders index them (`BUILDING_TEXTURES`, doc 04 §5.2):
/// wall layer `facade·2 + (has_bays ? 1 : 0)` for façades 0-3, 8 for glass; roof layer `9 + roof`.
/// The ground's are [`rustradar_scene::ground::GROUND_TEXTURES`].
pub const BUILDING_TEXTURES: [&str; 16] = [
    "plaster-wall",
    "plaster-window",
    "brick-wall",
    "brick-window",
    "block-wall",
    "block-window",
    "wood-wall",
    "wood-window",
    "glass",
    "roof-tar",
    "roof-concrete",
    "roof-metal",
    "roof-tiles",
    "roof-eternit",
    "roof-generic1",
    "roof-generic3",
];

/// The trees' layers, in the order the shaders index them: Streets GL's (`TREE_TEXTURES`).
pub const TREE_TEXTURES: [&str; 5] = ["tree-beech", "tree-fir", "tree-linden0", "tree-linden1", "tree-oak"];

/// A KTX2 file loading on the worker pool, then its texture: `None` while it loads or if it failed
/// (doc 03 §4.4: the layer that needs it is not drawn meanwhile).
#[derive(Debug)]
pub struct Ktx2Texture {
    file: TextureFile,
    state: State,
}

#[derive(Debug)]
enum State {
    Loading(Job<Result<Transcoded, TextureError>>),
    Ready(Texture),
    Failed(TextureError),
}

impl Ktx2Texture {
    /// Starts transcoding `bytes` (`file`'s) on `pool` for a device that takes `options`.
    pub fn load(pool: &Pool, file: TextureFile, bytes: Bytes, options: LoadOptions) -> Ktx2Texture {
        let job = pool.spawn(PRIORITY, move |cancel| transcode_with(&bytes, options, cancel));
        Ktx2Texture { file, state: State::Loading(job) }
    }

    /// Uploads the texture once its job is done; true when it just became ready (the map redraws).
    /// A failure is logged and kept.
    pub fn poll(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let State::Loading(job) = &mut self.state else { return false };
        let result = match job.poll() {
            JobPoll::Pending => return false,
            JobPoll::Done(result) => result,
            JobPoll::Cancelled => Err(TextureError::Cancelled),
            JobPoll::Lost => Err(TextureError::Lost),
        };
        match result {
            Ok(transcoded) => {
                let texture = transcoded.upload(device, queue, self.file.path, self.file.srgb, self.file.array);
                self.state = State::Ready(texture);
                true
            }
            Err(e) => {
                log::warn!("{}: {e}", self.file.path);
                self.state = State::Failed(e);
                false
            }
        }
    }

    pub fn file(&self) -> &TextureFile {
        &self.file
    }

    /// The texture, once loaded.
    pub fn texture(&self) -> Option<&Texture> {
        match &self.state {
            State::Ready(texture) => Some(texture),
            _ => None,
        }
    }

    /// Why it has no texture, if it failed.
    pub fn error(&self) -> Option<&TextureError> {
        match &self.state {
            State::Failed(e) => Some(e),
            _ => None,
        }
    }

    /// Still loading.
    pub fn pending(&self) -> bool {
        matches!(self.state, State::Loading(_))
    }

    fn take(self) -> Option<Texture> {
        match self.state {
            State::Ready(texture) => Some(texture),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct BuildingTextures {
    pub color: Texture,
    pub normal: Texture,
    /// R roughness, G metalness, B how much the building's own colour tints the texel.
    pub mask: Texture,
    /// R window emission.
    pub glow: Texture,
}

#[derive(Debug)]
pub struct GroundTextures {
    pub color: Texture,
    pub normal: Texture,
}

#[derive(Debug)]
pub struct TreeTextures {
    pub color: Texture,
    pub normal: Texture,
    /// R ≥ 0.5 inside the tree's silhouette.
    pub mask: Texture,
    /// The one normal map of a crown's volume, a plain 2D texture.
    pub volume: Texture,
}

/// The realistic map's textures (`realistic-textures.ts`): stand-ins at first (plain, flat), the
/// real ones once every file has loaded, all swapped in at once. If any file fails, the stand-ins
/// stay: plain surfaces, lit as ever.
#[derive(Debug)]
pub struct RealisticTextures {
    pub buildings: BuildingTextures,
    pub ground: GroundTextures,
    pub trees: TreeTextures,
    pub water: Texture,
    /// The whole Earth, cloud-free, for the land's colour zoomed out (alpha 0 until it loads).
    pub earth: Texture,
    /// Its lights at night, for the globe's night side (black until it loads).
    pub night: Texture,
    /// The real textures are in.
    pub ready: bool,
    loading: Vec<Ktx2Texture>,
    started: bool,
}

impl RealisticTextures {
    /// The stand-ins.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> RealisticTextures {
        let array = |kind| stand_in(device, queue, kind, true);
        let plain = |kind| stand_in(device, queue, kind, false);
        RealisticTextures {
            buildings: BuildingTextures {
                color: array(StandIn::Color),
                normal: array(StandIn::Normal),
                mask: array(StandIn::Mask),
                glow: array(StandIn::Glow),
            },
            ground: GroundTextures { color: array(StandIn::Color), normal: array(StandIn::Normal) },
            // (no tree shows until they load: a mask of none)
            trees: TreeTextures {
                color: array(StandIn::Color),
                normal: array(StandIn::Normal),
                mask: array(StandIn::TreeMask),
                volume: plain(StandIn::Normal),
            },
            water: plain(StandIn::Normal),
            earth: plain(StandIn::Earth),
            night: plain(StandIn::Night),
            ready: false,
            loading: Vec::new(),
            started: false,
        }
    }

    /// Starts loading every file of [`REALISTIC_FILES`] on `pool` (the first time only), its bytes
    /// from `bytes(path)`. A file with no bytes fails the set.
    pub fn start(&mut self, pool: &Pool, options: LoadOptions, mut bytes: impl FnMut(&'static str) -> Option<Bytes>) {
        if self.started {
            return;
        }
        self.started = true;
        for file in REALISTIC_FILES {
            let Some(data) = bytes(file.path) else {
                log::warn!("realistic textures: {}", TextureError::Missing(file.path));
                self.loading.clear();
                return;
            };
            self.loading.push(Ktx2Texture::load(pool, file, data, options));
        }
    }

    /// Uploads what has arrived; swaps every texture in once the last has (true then: the map
    /// redraws). A failed file drops the others: the stand-ins stay.
    pub fn poll(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if self.loading.is_empty() {
            return false;
        }
        for texture in &mut self.loading {
            texture.poll(device, queue);
        }
        if self.loading.iter().any(|t| t.error().is_some()) {
            // (each failure was logged by its poll)
            self.loading.clear();
            return false;
        }
        if self.loading.iter().any(Ktx2Texture::pending) {
            return false;
        }
        let mut loaded = std::mem::take(&mut self.loading).into_iter().filter_map(Ktx2Texture::take);
        let mut next = || loaded.next().expect("a texture for every file");
        // the order of REALISTIC_FILES
        self.buildings = BuildingTextures { color: next(), normal: next(), mask: next(), glow: next() };
        self.ground = GroundTextures { color: next(), normal: next() };
        let (color, normal, mask) = (next(), next(), next());
        self.water = next();
        self.earth = next();
        self.trees = TreeTextures { color, normal, mask, volume: next() };
        self.night = next();
        self.ready = true;
        true
    }

    /// Files still loading.
    pub fn pending(&self) -> bool {
        self.loading.iter().any(Ktx2Texture::pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::testing::gpu;

    fn static_file(path: &str) -> Vec<u8> {
        let full = format!("{}/../../static/{path}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&full).unwrap_or_else(|e| panic!("{full}: {e}"))
    }

    // ---- ktx2-transcode.test.ts ----

    #[test]
    fn a_texture_array_every_layer_every_mip_level_down_to_1x1() {
        let t = transcode(&static_file("realistic/tree.mask.ktx2"), Target::Rgba).unwrap();
        assert_eq!(t.layers, 5);
        assert_eq!(t.levels.len() as u32, t.width.ilog2() + 1);
        assert_eq!(t.levels.last().unwrap().width, 1);
        for l in &t.levels {
            assert_eq!(l.data.len(), (l.width * l.height * 4 * t.layers) as usize);
        }
    }

    #[test]
    fn bc7_and_astc_16_bytes_a_4x4_block() {
        for target in [Target::Bc7, Target::Astc] {
            let t = transcode(&static_file("realistic/water.normal.ktx2"), target).unwrap();
            assert_eq!(t.layers, 1);
            assert_eq!(t.levels[0].data.len(), ((t.width / 4) * (t.height / 4) * 16) as usize);
        }
    }

    #[test]
    fn the_aircraft_icons_keep_their_alpha() {
        let t = transcode(&static_file("aircraft/icons.ktx2"), Target::Rgba).unwrap();
        let alpha: std::collections::HashSet<u8> = t.levels[0].data.iter().skip(3).step_by(4).copied().collect();
        assert!(alpha.contains(&0) && alpha.contains(&255));
    }

    #[test]
    fn not_a_ktx2_file_tauri_s_page_for_a_missing_one() {
        let e = transcode(b"<!doctype html><html>", Target::Rgba).unwrap_err();
        assert_eq!(e, TextureError::NotKtx2);
        assert_eq!(e.to_string(), "not a KTX2 file");
    }

    // ---- new ----

    /// The plan's acceptance: the icon atlas to RGBA8, its size, its levels, and not blank.
    #[test]
    fn the_aircraft_icons_become_rgba8_896_by_768_with_10_levels_not_blank() {
        let t = transcode(&static_file("aircraft/icons.ktx2"), Target::Rgba).unwrap();
        assert_eq!((t.width, t.height, t.layers, t.skipped, t.target), (896, 768, 1, 0, Target::Rgba));
        assert_eq!(t.levels.len(), 10);
        for (i, l) in t.levels.iter().enumerate() {
            assert_eq!((l.width, l.height), ((896 >> i).max(1), (768 >> i).max(1)), "level {i}");
            assert_eq!(l.data.len(), (l.width * l.height * 4) as usize);
        }
        let top = &t.levels[0].data;
        let covered = top.chunks_exact(4).filter(|p| p[3] > 0).count();
        let pixels = top.len() / 4;
        // silhouettes, white: some of the atlas, far from all of it
        assert!(covered > pixels / 20 && covered < pixels / 2, "{covered} of {pixels} pixels covered");
        assert!(top.chunks_exact(4).filter(|p| p[3] == 255).all(|p| p[..3] == [255, 255, 255]));
    }

    #[test]
    fn the_outline_atlas_is_7_by_6_cells_of_152_with_11_levels() {
        let i = info(&static_file("aircraft/icons-outline.ktx2")).unwrap();
        assert_eq!((i.width, i.height, i.levels, i.layers, i.srgb, i.etc1s), (1064, 912, 11, 0, false, false));
        let t = transcode(&static_file("aircraft/icons-outline.ktx2"), Target::Bc7).unwrap();
        // 1064 × 912 is whole blocks: it stays BC7
        assert_eq!((t.target, t.levels.len()), (Target::Bc7, 11));
        assert_eq!(t.levels[0].data.len(), (1064 / 4) * (912 / 4) * 16);
        // the levels below a block are a whole block each
        assert_eq!((t.levels[10].width, t.levels[10].height, t.levels[10].data.len()), (1, 1, 16));
    }

    /// Doc 04 §5.1, measured from the headers: and the table of files agrees.
    #[test]
    fn the_files_are_as_doc_04_lists_them() {
        let expect = [
            // path, width, height, layers, levels, etc1s
            ("realistic/building.color.ktx2", 512, 512, 16, 10, false),
            ("realistic/building.normal.ktx2", 512, 512, 16, 10, false),
            ("realistic/building.mask.ktx2", 512, 512, 16, 10, false),
            ("realistic/building.glow.ktx2", 128, 128, 16, 8, false),
            ("realistic/ground.color.ktx2", 512, 512, 23, 10, false),
            ("realistic/ground.normal.ktx2", 512, 512, 23, 10, false),
            ("realistic/tree.color.ktx2", 512, 512, 5, 10, false),
            ("realistic/tree.normal.ktx2", 512, 512, 5, 10, false),
            ("realistic/tree.mask.ktx2", 512, 512, 5, 10, false),
            ("realistic/water.normal.ktx2", 256, 256, 0, 9, false),
            ("realistic/earth.color.ktx2", 4096, 2048, 0, 13, true),
            ("realistic/tree-volume.normal.ktx2", 512, 512, 0, 10, false),
            ("realistic/earth.night.ktx2", 4096, 2048, 0, 13, true),
        ];
        for (file, (path, width, height, layers, levels, etc1s)) in REALISTIC_FILES.iter().zip(expect) {
            assert_eq!(file.path, path);
            let i = info(&static_file(path)).unwrap();
            assert_eq!((i.width, i.height, i.layers, i.levels, i.etc1s), (width, height, layers, levels, etc1s), "{path}");
            assert_eq!(i.srgb, file.srgb, "{path}");
            assert_eq!(file.array, layers > 0, "{path}");
        }
        assert_eq!(info(&static_file(BUILDING_COLOR.path)).unwrap().layers as usize, BUILDING_TEXTURES.len());
        assert_eq!(info(&static_file(GROUND_COLOR.path)).unwrap().layers as usize, rustradar_scene::ground::GROUND_TEXTURES.len());
        assert_eq!(info(&static_file(TREE_COLOR.path)).unwrap().layers as usize, TREE_TEXTURES.len());
        let icons = info(&static_file(AIRCRAFT_ICONS.path)).unwrap();
        assert_eq!((icons.width, icons.height, icons.levels, icons.layers, icons.srgb), (896, 768, 10, 0, AIRCRAFT_ICONS.srgb));
    }

    #[test]
    fn targets_go_bc7_astc_etc2_rgba8() {
        use wgpu::Features as F;
        let all = F::TEXTURE_COMPRESSION_BC | F::TEXTURE_COMPRESSION_ASTC | F::TEXTURE_COMPRESSION_ETC2;
        assert_eq!(Target::for_features(all), Target::Bc7);
        assert_eq!(Target::for_features(F::TEXTURE_COMPRESSION_ASTC | F::TEXTURE_COMPRESSION_ETC2), Target::Astc);
        assert_eq!(Target::for_features(F::TEXTURE_COMPRESSION_ETC2), Target::Etc2);
        assert_eq!(Target::for_features(F::empty()), Target::Rgba);
        assert_eq!(Target::for_features(F::TEXTURE_COMPRESSION_BC | F::TEXTURE_COMPRESSION_ETC2), Target::Bc7);
        // the transcoder's numbers of the web build's TARGET
        assert_eq!([Target::Bc7, Target::Astc, Target::Rgba].map(Target::transcoder_format), [6, 10, 13]);
        assert_eq!(Target::Etc2.transcoder_format(), 1);
        assert_eq!(compression_features(all | F::DEPTH_CLIP_CONTROL), all);
        for target in Target::ALL {
            for srgb in [false, true] {
                let format = target.format(srgb);
                assert_eq!(format.is_srgb(), srgb, "{target:?}");
                assert_eq!(format.required_features(), target.feature(), "{target:?}");
                assert_eq!(format.is_compressed(), target.is_compressed());
            }
        }
    }

    /// ETC1S becomes every target when asked for it: an ASTC-only device gets ASTC, not RGBA8.
    #[test]
    fn the_earth_pictures_become_every_target() {
        let night = static_file("realistic/earth.night.ktx2");
        // skip down to 256 × 128 (levels 4..13): quick, and every target through the same code
        let options = |target| LoadOptions { target, max_size: 256 };
        let rgba = transcode_with(&night, options(Target::Rgba), &Cancel::new()).unwrap();
        assert_eq!((rgba.width, rgba.height, rgba.skipped, rgba.levels.len()), (256, 128, 4, 9));
        assert!(rgba.levels[0].data.chunks_exact(4).any(|p| p[..3] != [0, 0, 0]), "some lights");
        for target in [Target::Bc7, Target::Astc, Target::Etc2] {
            let t = transcode_with(&night, options(target), &Cancel::new()).unwrap();
            assert_eq!((t.target, t.width, t.height, t.levels.len()), (target, 256, 128, 9));
            assert_eq!(t.levels[0].data.len(), (256 / 4) * (128 / 4) * 16, "{target:?}");
        }
    }

    #[test]
    fn levels_above_the_device_are_skipped_never_the_last() {
        // the Earth pictures on a 2048 device: their top level goes
        assert_eq!(levels_to_skip(4096, 2048, 13, 2048), 1);
        assert_eq!(levels_to_skip(4096, 2048, 13, 4096), 0);
        assert_eq!(levels_to_skip(4096, 2048, 13, 8192), 0);
        assert_eq!(levels_to_skip(4096, 2048, 13, 1000), 3);
        assert_eq!(levels_to_skip(512, 512, 10, 2048), 0);
        // the taller side counts
        assert_eq!(levels_to_skip(1024, 4096, 13, 2048), 1);
        // at most down to the last level
        assert_eq!(levels_to_skip(4096, 2048, 13, 0), 12);
        assert_eq!(levels_to_skip(4096, 2048, 3, 1), 2);
        assert_eq!(levels_to_skip(8, 8, 1, 1), 0);
    }

    #[test]
    fn the_earth_on_a_2048_device_starts_at_2048_by_1024() {
        let earth = static_file("realistic/earth.color.ktx2");
        let t = transcode_with(&earth, LoadOptions { target: Target::Bc7, max_size: 2048 }, &Cancel::new()).unwrap();
        assert_eq!((t.width, t.height, t.skipped, t.levels.len(), t.target), (2048, 1024, 1, 12, Target::Bc7));
        assert_eq!((t.levels[11].width, t.levels[11].height), (1, 1));
        assert_eq!(t.levels[0].data.len(), (2048 / 4) * (1024 / 4) * 16);
    }

    /// A first level that is not whole 4×4 blocks becomes RGBA8 (wgpu takes no other).
    #[test]
    fn a_first_level_of_part_blocks_falls_back_to_rgba8() {
        let outline = static_file("aircraft/icons-outline.ktx2");
        // 1064 × 912 → 133 × 114 at level 3
        let t = transcode_with(&outline, LoadOptions { target: Target::Astc, max_size: 140 }, &Cancel::new()).unwrap();
        assert_eq!((t.width, t.height, t.skipped, t.target), (133, 114, 3, Target::Rgba));
        assert_eq!(t.levels[0].data.len(), 133 * 114 * 4);
        // and 266 × 228 at level 2 (266 is not a multiple of 4)
        let t = transcode_with(&outline, LoadOptions { target: Target::Bc7, max_size: 300 }, &Cancel::new()).unwrap();
        assert_eq!((t.width, t.height, t.skipped, t.target), (266, 228, 2, Target::Rgba));
        // 532 × 456 at level 1 is whole blocks
        let t = transcode_with(&outline, LoadOptions { target: Target::Astc, max_size: 600 }, &Cancel::new()).unwrap();
        assert_eq!((t.width, t.height, t.skipped, t.target), (532, 456, 1, Target::Astc));
    }

    #[test]
    fn a_truncated_or_broken_file_fails_without_a_crash() {
        let icons = static_file("aircraft/icons.ktx2");
        for len in [12, 40, 200, icons.len() / 2, icons.len() - 1] {
            assert!(transcode(&icons[..len], Target::Rgba).is_err(), "{len} bytes");
        }
        let mut broken = icons.clone();
        for b in &mut broken[icons.len() / 2..] {
            *b = 0x5a;
        }
        assert!(transcode(&broken, Target::Rgba).is_err());
        assert_eq!(info(&KTX2_MAGIC), Err(TextureError::Unreadable));
        assert_eq!(info(b""), Err(TextureError::NotKtx2));
    }

    #[test]
    fn a_cancelled_job_stops() {
        let cancel = Cancel::new();
        cancel.cancel();
        let icons = static_file("aircraft/icons.ktx2");
        let options = LoadOptions { target: Target::Rgba, max_size: u32::MAX };
        assert_eq!(transcode_with(&icons, options, &cancel), Err(TextureError::Cancelled));
    }

    #[test]
    fn transcoding_runs_on_the_pool_threads_at_once() {
        let pool = Pool::new(4);
        let files = ["aircraft/icons.ktx2", "realistic/water.normal.ktx2", "realistic/tree.mask.ktx2", "aircraft/icons.ktx2"];
        let mut jobs: Vec<_> = files
            .iter()
            .map(|path| {
                let bytes = static_file(path);
                pool.spawn(PRIORITY, move |cancel| transcode_with(&bytes, LoadOptions { target: Target::Bc7, max_size: 4096 }, cancel))
            })
            .collect();
        let results: Vec<_> = jobs.iter_mut().map(|job| job.wait()).collect();
        for (path, result) in files.iter().zip(results) {
            match result {
                JobPoll::Done(Ok(t)) => assert_eq!(t, transcode(&static_file(path), Target::Bc7).unwrap(), "{path}"),
                other => panic!("{path}: {other:?}"),
            }
        }
    }

    // ---- on the GPU ----

    /// Reads layer `layer` of mip level `mip` back: as many bytes as the texture holds there, in
    /// rows of whole blocks.
    fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, mip: u32, layer: u32) -> Vec<u8> {
        let format = texture.format();
        let (bw, bh) = format.block_dimensions();
        let block = format.block_copy_size(None).unwrap();
        let (width, height) = ((texture.width() >> mip).max(1), (texture.height() >> mip).max(1));
        let (columns, rows) = (width.div_ceil(bw), height.div_ceil(bh));
        let row = columns * block;
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("texture readback"),
            size: u64::from(padded * rows),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: mip,
                origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None },
            },
            wgpu::Extent3d { width: columns * bw, height: rows * bh, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = buffer.get_mapped_range(..).unwrap();
        mapped.chunks_exact(padded as usize).flat_map(|r| r[..row as usize].to_vec()).collect()
    }

    /// The bytes of layer `layer` of a transcoded level.
    fn layer_of(t: &Transcoded, mip: usize, layer: u32) -> &[u8] {
        let data = &t.levels[mip].data;
        let size = data.len() / t.layers as usize;
        &data[size * layer as usize..size * (layer as usize + 1)]
    }

    #[test]
    fn the_icons_upload_as_a_2d_texture_with_every_level() {
        let Some(gpu) = gpu() else { return };
        let t = transcode(&static_file(AIRCRAFT_ICONS.path), Target::Rgba).unwrap();
        let tex = t.upload(&gpu.device, &gpu.queue, "icons", AIRCRAFT_ICONS.srgb, AIRCRAFT_ICONS.array);
        assert_eq!(tex.size, [896, 768]);
        assert_eq!((tex.texture.mip_level_count(), tex.texture.depth_or_array_layers()), (10, 1));
        assert_eq!(tex.texture.format(), wgpu::TextureFormat::Rgba8Unorm);
        for mip in [0, 3, 9] {
            assert!(read_back(&gpu.device, &gpu.queue, &tex.texture, mip, 0) == t.levels[mip as usize].data, "level {mip}");
        }
    }

    #[test]
    fn an_array_uploads_every_layer_and_a_plain_texture_its_first() {
        let Some(gpu) = gpu() else { return };
        let t = transcode(&static_file(TREE_COLOR.path), Target::Rgba).unwrap();
        let array = t.upload(&gpu.device, &gpu.queue, "trees", TREE_COLOR.srgb, true);
        assert_eq!((array.texture.depth_or_array_layers(), array.texture.mip_level_count()), (5, 10));
        assert_eq!(array.texture.format(), wgpu::TextureFormat::Rgba8UnormSrgb);
        for (mip, layer) in [(0, 0), (0, 4), (2, 3), (9, 1)] {
            assert!(read_back(&gpu.device, &gpu.queue, &array.texture, mip, layer) == layer_of(&t, mip as usize, layer), "level {mip} layer {layer}");
        }
        let plain = t.upload(&gpu.device, &gpu.queue, "first tree", false, false);
        assert_eq!((plain.texture.depth_or_array_layers(), plain.texture.format()), (1, wgpu::TextureFormat::Rgba8Unorm));
        assert!(read_back(&gpu.device, &gpu.queue, &plain.texture, 1, 0) == layer_of(&t, 1, 0));
    }

    #[test]
    fn stand_ins_hold_their_colours() {
        let Some(gpu) = gpu() else { return };
        for kind in [StandIn::Color, StandIn::Normal, StandIn::Mask, StandIn::Glow, StandIn::TreeMask, StandIn::Earth, StandIn::Night] {
            for array in [false, true] {
                let tex = stand_in(&gpu.device, &gpu.queue, kind, array);
                assert_eq!(tex.size, [1, 1]);
                assert_eq!(tex.texture.format().is_srgb(), kind.srgb());
                assert_eq!(tex.texture.depth_or_array_layers(), if array { 2 } else { 1 });
                for layer in 0..tex.texture.depth_or_array_layers() {
                    assert_eq!(read_back(&gpu.device, &gpu.queue, &tex.texture, 0, layer), kind.rgba(), "{kind:?}");
                }
            }
        }
        assert_eq!(StandIn::Mask.rgba(), [235, 0, 255, 255]);
        assert_eq!(StandIn::Earth.rgba()[3], 0);
    }

    #[test]
    fn every_sampling_makes_a_valid_sampler() {
        let Some(gpu) = gpu() else { return };
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        for sampling in [Sampling::Repeat, Sampling::Globe, Sampling::Clamp, Sampling::Atlas] {
            let _ = sampling.sampler(&gpu.device);
        }
        assert!(pollster::block_on(scope.pop()).is_none());
        let globe = Sampling::Globe.descriptor();
        assert_eq!((globe.address_mode_u, globe.address_mode_v, globe.anisotropy_clamp), (wgpu::AddressMode::Repeat, wgpu::AddressMode::ClampToEdge, 8));
        assert_eq!(EARTH_COLOR.sampling, Sampling::Globe);
        assert_eq!(TREE_VOLUME_NORMAL.sampling, Sampling::Clamp);
    }

    #[test]
    fn the_realistic_set_swaps_in_at_once() {
        let Some(gpu) = gpu() else { return };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let pool = Pool::new(3);
        // small top levels keep the test quick: every file down to 64 texels a side
        let options = LoadOptions { target: Target::for_features(device.features()), max_size: 64 };
        let mut set = RealisticTextures::new(device, queue);
        assert!(!set.ready && !set.pending());
        assert_eq!((set.earth.size, set.ground.color.texture.depth_or_array_layers()), ([1, 1], 2));
        assert!(!set.poll(device, queue), "nothing loading");
        set.start(&pool, options, |path| Some(Cow::Owned(static_file(path))));
        assert!(set.pending());
        // the stand-ins stay until the last file is in
        let mut swaps = 0;
        loop {
            assert_eq!(set.earth.size == [1, 1], !set.ready);
            if set.poll(device, queue) {
                swaps += 1;
            }
            if set.ready {
                break;
            }
            assert!(set.pending(), "stopped without the textures");
            std::thread::yield_now();
        }
        assert_eq!(swaps, 1);
        assert_eq!(set.buildings.color.texture.depth_or_array_layers(), 16);
        assert_eq!(set.buildings.glow.size, [64, 64]);
        assert_eq!(set.ground.normal.texture.depth_or_array_layers(), 23);
        assert_eq!(set.trees.mask.texture.depth_or_array_layers(), 5);
        assert_eq!((set.trees.volume.size, set.trees.volume.texture.depth_or_array_layers()), ([64, 64], 1));
        assert_eq!(set.water.size, [64, 64]);
        // 4096 × 2048 → 64 × 32 (levels 6..13)
        assert_eq!((set.earth.size, set.earth.texture.mip_level_count()), ([64, 32], 7));
        assert_eq!(set.night.texture.mip_level_count(), 7);
        assert!(set.earth.texture.format().is_srgb() && set.buildings.color.texture.format().is_srgb());
        assert!(!set.buildings.normal.texture.format().is_srgb());
        // the earth's alpha is 1 once it is in: the ground shader uses it
        if options.target == Target::Rgba {
            let earth = read_back(device, queue, &set.earth.texture, 0, 0);
            assert!(earth.chunks_exact(4).all(|p| p[3] == 255));
        }
        assert!(!set.poll(device, queue), "once");
        set.start(&pool, options, |_| panic!("started once"));
    }

    #[test]
    fn a_missing_or_broken_file_keeps_the_stand_ins() {
        let Some(gpu) = gpu() else { return };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let pool = Pool::new(2);
        let options = LoadOptions { target: Target::Rgba, max_size: 16 };
        let mut missing = RealisticTextures::new(device, queue);
        missing.start(&pool, options, |path| (path != EARTH_NIGHT.path).then(|| Cow::Owned(static_file(path))));
        assert!(!missing.pending() && !missing.poll(device, queue) && !missing.ready);
        let mut broken = RealisticTextures::new(device, queue);
        broken.start(&pool, options, |path| {
            Some(if path == WATER_NORMAL.path { Cow::Borrowed(&b"<!doctype html>"[..]) } else { Cow::Owned(static_file(path)) })
        });
        while broken.pending() {
            assert!(!broken.poll(device, queue));
            std::thread::yield_now();
        }
        assert!(!broken.ready && broken.earth.size == [1, 1] && broken.trees.mask.size == [1, 1]);
        let mut icons = Ktx2Texture::load(&pool, AIRCRAFT_ICONS, Cow::Borrowed(&b"<html>"[..]), options);
        pool.wait_idle();
        assert!(!icons.poll(device, queue));
        assert_eq!((icons.texture().is_none(), icons.error()), (true, Some(&TextureError::NotKtx2)));
    }

    #[test]
    fn an_atlas_is_none_until_it_has_loaded() {
        let Some(gpu) = gpu() else { return };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let pool = Pool::new(1);
        let mut outline = Ktx2Texture::load(&pool, AIRCRAFT_ICONS_OUTLINE, Cow::Owned(static_file(AIRCRAFT_ICONS_OUTLINE.path)), LoadOptions::for_device(device));
        let mut arrived = 0;
        while outline.pending() {
            assert!(outline.texture().is_none());
            arrived += usize::from(outline.poll(device, queue));
            std::thread::yield_now();
        }
        assert_eq!(arrived, 1);
        let tex = outline.texture().unwrap();
        assert_eq!((tex.size, tex.texture.mip_level_count()), ([1064, 912], 11));
        assert!(!outline.poll(device, queue));
    }

    /// Mip level 0 of a plain 2D `texture` as the GPU decodes it, texel for texel, into RGBA8.
    fn decode(device: &wgpu::Device, queue: &wgpu::Queue, texture: &Texture) -> Vec<u8> {
        const SHADER: &str = "
            @group(0) @binding(0) var t: texture_2d<f32>;
            @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
                return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
            }
            @fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
                return textureLoad(t, vec2<i32>(p.xy), 0);
            }";
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("decode"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("decode"),
            layout: None,
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decode"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&texture.view) }],
        });
        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("decoded"),
            size: wgpu::Extent3d { width: texture.size[0], height: texture.size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = out.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
        read_back(device, queue, &out, 0, 0)
    }

    /// The compressed targets upload too, on a device that has them: the blocks read back as
    /// transcoded, at a level of whole blocks and at one smaller than a block; and the GPU decodes
    /// them to nearly the texels of the RGBA8 transcode.
    #[test]
    fn compressed_targets_upload_block_for_block() {
        let Some(gpu) = gpu() else { return };
        let wanted = compression_features(gpu.adapter.features());
        if wanted.is_empty() {
            eprintln!("skipped: the adapter has no texture compression");
            return;
        }
        // a second device of the same adapter, while this test holds the shared one (SwiftShader
        // must not open devices in parallel)
        let (device, queue) = pollster::block_on(gpu.adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("compressed textures"),
            required_features: wanted,
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(gpu.adapter.limits()),
            ..Default::default()
        }))
        .unwrap();
        assert_eq!(LoadOptions::for_device(&device).target, Target::for_features(wanted));
        let mut checked = Vec::new();
        for target in [Target::Bc7, Target::Astc, Target::Etc2] {
            if !wanted.contains(target.feature()) {
                continue;
            }
            for file in [AIRCRAFT_ICONS_OUTLINE, TREE_MASK] {
                let t = transcode(&static_file(file.path), target).unwrap();
                let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let tex = t.upload(&device, &queue, file.path, file.srgb, file.array);
                let last = t.levels.len() - 1;
                for (mip, layer) in [(0, t.layers - 1), (3, 0), (last, 0)] {
                    assert!(read_back(&device, &queue, &tex.texture, mip as u32, layer) == layer_of(&t, mip, layer), "{target:?} {} level {mip}", file.path);
                }
                assert!(pollster::block_on(scope.pop()).is_none(), "{target:?} {}", file.path);
            }
            let outline = static_file(AIRCRAFT_ICONS_OUTLINE.path);
            let tex = transcode(&outline, target).unwrap().upload(&device, &queue, "outline", false, false);
            let decoded = decode(&device, &queue, &tex);
            let rgba = &transcode(&outline, Target::Rgba).unwrap().levels[0].data;
            let errors: Vec<u32> = decoded.iter().zip(rgba).map(|(&a, &b)| u32::from(a.abs_diff(b))).collect();
            let mean = errors.iter().sum::<u32>() as f64 / errors.len() as f64;
            let max = errors.iter().max().copied().unwrap_or(0);
            eprintln!("{target:?} decoded against RGBA8: mean error {mean:.3}, max {max}");
            assert!(mean < 1.0 && max <= 64, "{target:?}: mean {mean}, max {max}");
            if target == Target::Bc7 {
                let shots = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
                std::fs::create_dir_all(&shots).unwrap();
                let image = crate::headless::Image { width: tex.size[0], height: tex.size[1], rgba: decoded };
                image.save_png(&shots.join("e15-outline-bc7.png")).unwrap();
            }
            checked.push(target);
        }
        eprintln!("compressed targets checked: {checked:?}");
    }
}
