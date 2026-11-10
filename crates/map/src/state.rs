//! Draw-state presets (plan 1.4.5): deck.gl's `parameters` objects of `Globe.svelte` as wgpu
//! primitive, depth and stencil states.
//!
//! Every pipeline blends with [`BLEND`] (deck's default: colour `SrcAlpha, OneMinusSrcAlpha`, alpha
//! `One, OneMinusSrcAlpha`); a layer's opacity multiplies fragment alpha in its shader. Depth
//! compare is `LessEqual` unless stated. There is no polygon offset anywhere: order is the draw
//! order, and ground layers write no depth (doc 02 §14.3.10).
//!
//! Front faces are counter-clockwise in clip space, as in WebGL (the matrices of `frame` keep
//! GL's handedness; only depth is remapped to wgpu's `[0, 1]`). Local Mercator has `v` growing
//! south, so a ring that is counter-clockwise on the map is clockwise in `(u, v)`.
//!
//! | Preset | Cull | Depth write | Depth compare | Stencil |
//! |---|---|---|---|---|
//! | [`DrawState::DEFAULT`] | back | on | less-equal | – |
//! | [`DrawState::NO_CULL`] | none | on | less-equal | – |
//! | [`DrawState::SURFACE`] | back | off | less-equal | – |
//! | [`DrawState::SURFACE_NO_CULL`], `RADAR_LIFTED`, `SHIP`, `TRAIN`, `CLOUD` | none | off | less-equal | – |
//! | [`DrawState::LABEL`], `THROUGH_GROUND`, `RAISED_NO_CULL` | none | off | always | – |
//! | [`DrawState::SHADOW`] | none | off | less-equal | equal to 0, pass increment-clamp |

use crate::gpu::DEPTH_FORMAT;

/// deck.gl's default blending (doc 01 §4): colour `SrcAlpha, OneMinusSrcAlpha`; alpha `One,
/// OneMinusSrcAlpha`.
pub const BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// The stencil rule of every shadow (`SHADOW_PARAMETERS`, G:366-375): draw where the stencil is
/// still 0 and bump it, so a pixel is darkened at most once per frame.
pub const SHADOW_STENCIL: wgpu::StencilFaceState = wgpu::StencilFaceState {
    compare: wgpu::CompareFunction::Equal,
    fail_op: wgpu::StencilOperation::Keep,
    depth_fail_op: wgpu::StencilOperation::Keep,
    pass_op: wgpu::StencilOperation::IncrementClamp,
};

/// The stencil reference value the `SHADOW` preset compares with (set once per pass with
/// `set_stencil_reference`; 0 is also wgpu's default).
pub const SHADOW_STENCIL_REFERENCE: u32 = 0;

/// One row of the table above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawState {
    /// The face culled, `None` for none.
    pub cull: Option<wgpu::Face>,
    pub depth_write: bool,
    pub depth_compare: wgpu::CompareFunction,
    /// The stencil test for both faces, `None` for no stencil use.
    pub stencil: Option<wgpu::StencilFaceState>,
}

impl DrawState {
    /// deck's default under `GlobeView`: back faces culled, depth test and write.
    pub const DEFAULT: DrawState = DrawState {
        cull: Some(wgpu::Face::Back),
        depth_write: true,
        depth_compare: wgpu::CompareFunction::LessEqual,
        stencil: None,
    };
    /// `NO_CULL` (G:327): camera-facing quads and geometry with inconsistent winding.
    pub const NO_CULL: DrawState = DrawState { cull: None, ..Self::DEFAULT };
    /// `SURFACE` (G:332): ground layers, depth-tested against the earth sphere but writing no
    /// depth, so they paint strictly in layer order.
    pub const SURFACE: DrawState = DrawState { depth_write: false, ..Self::DEFAULT };
    /// `SURFACE_NO_CULL` (G:333).
    pub const SURFACE_NO_CULL: DrawState = DrawState { cull: None, depth_write: false, ..Self::DEFAULT };
    /// `RADAR_LIFTED` (G:1154).
    pub const RADAR_LIFTED: DrawState = Self::SURFACE_NO_CULL;
    /// `SHIP_PARAMETERS` (G:1604).
    pub const SHIP: DrawState = Self::SURFACE_NO_CULL;
    /// `TRAIN_PARAMETERS` (G:1882).
    pub const TRAIN: DrawState = Self::SURFACE_NO_CULL;
    /// `CLOUD_PARAMETERS`.
    pub const CLOUD: DrawState = Self::SURFACE_NO_CULL;
    /// `LABEL_PARAMETERS` (G:724): text ignores depth entirely (far-side labels are filtered on
    /// the CPU).
    pub const LABEL: DrawState = DrawState {
        cull: None,
        depth_write: false,
        depth_compare: wgpu::CompareFunction::Always,
        stencil: None,
    };
    /// `THROUGH_GROUND` (G:1885).
    pub const THROUGH_GROUND: DrawState = Self::LABEL;
    /// `RAISED_NO_CULL` (`rail-layer.ts:113`).
    pub const RAISED_NO_CULL: DrawState = Self::LABEL;
    /// `SHADOW_PARAMETERS` (G:366-375): every shadow (aircraft, building, foot shade, tree).
    pub const SHADOW: DrawState = DrawState {
        cull: None,
        depth_write: false,
        depth_compare: wgpu::CompareFunction::LessEqual,
        stencil: Some(SHADOW_STENCIL),
    };

    /// The primitive state for this preset (counter-clockwise front faces).
    pub fn primitive(&self, topology: wgpu::PrimitiveTopology) -> wgpu::PrimitiveState {
        wgpu::PrimitiveState {
            topology,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: self.cull,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        }
    }

    /// The depth/stencil state for the map pass's [`DEPTH_FORMAT`] attachment.
    pub fn depth_stencil(&self) -> wgpu::DepthStencilState {
        let stencil = match self.stencil {
            Some(face) => wgpu::StencilState { front: face, back: face, read_mask: 0xff, write_mask: 0xff },
            None => wgpu::StencilState::default(),
        };
        wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(self.depth_write),
            depth_compare: Some(self.depth_compare),
            stencil,
            bias: wgpu::DepthBiasState::default(),
        }
    }
}

/// The colour target of every map pipeline: `format` with [`BLEND`].
pub fn color_target(format: wgpu::TextureFormat) -> wgpu::ColorTargetState {
    wgpu::ColorTargetState { format, blend: Some(BLEND), write_mask: wgpu::ColorWrites::ALL }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::CompareFunction::{Always, LessEqual};

    #[test]
    fn presets_follow_the_table_of_plan_1_4_5() {
        let row = |s: DrawState| (s.cull, s.depth_write, s.depth_compare, s.stencil.is_some());
        assert_eq!(row(DrawState::DEFAULT), (Some(wgpu::Face::Back), true, LessEqual, false));
        assert_eq!(row(DrawState::NO_CULL), (None, true, LessEqual, false));
        assert_eq!(row(DrawState::SURFACE), (Some(wgpu::Face::Back), false, LessEqual, false));
        for s in [DrawState::SURFACE_NO_CULL, DrawState::RADAR_LIFTED, DrawState::SHIP, DrawState::TRAIN, DrawState::CLOUD] {
            assert_eq!(row(s), (None, false, LessEqual, false));
        }
        for s in [DrawState::LABEL, DrawState::THROUGH_GROUND, DrawState::RAISED_NO_CULL] {
            assert_eq!(row(s), (None, false, Always, false));
        }
        assert_eq!(row(DrawState::SHADOW), (None, false, LessEqual, true));
    }

    #[test]
    fn the_shadow_preset_darkens_a_pixel_once() {
        let ds = DrawState::SHADOW.depth_stencil();
        for face in [ds.stencil.front, ds.stencil.back] {
            assert_eq!(face.compare, wgpu::CompareFunction::Equal);
            assert_eq!(face.pass_op, wgpu::StencilOperation::IncrementClamp);
            assert_eq!(face.fail_op, wgpu::StencilOperation::Keep);
            assert_eq!(face.depth_fail_op, wgpu::StencilOperation::Keep);
        }
        assert_eq!((ds.stencil.read_mask, ds.stencil.write_mask), (0xff, 0xff));
        assert_eq!(SHADOW_STENCIL_REFERENCE, 0);
        // the other presets leave the stencil alone
        let plain = DrawState::DEFAULT.depth_stencil();
        assert!(!plain.stencil.is_enabled());
        assert_eq!(plain.depth_write_enabled, Some(true));
        assert_eq!(DrawState::LABEL.depth_stencil().depth_write_enabled, Some(false));
    }

    #[test]
    fn blending_is_decks_default() {
        let t = color_target(wgpu::TextureFormat::Rgba8Unorm);
        let blend = t.blend.expect("blends");
        assert_eq!(blend.color.src_factor, wgpu::BlendFactor::SrcAlpha);
        assert_eq!(blend.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(blend.alpha.src_factor, wgpu::BlendFactor::One);
        assert_eq!(blend.alpha.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);
        assert_eq!(DrawState::NO_CULL.primitive(wgpu::PrimitiveTopology::TriangleList).front_face, wgpu::FrontFace::Ccw);
    }
}
