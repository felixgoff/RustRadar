//! The `shadow` variants (plan 1.4.6): the `mesh` and `fill` pipelines with the [`DrawState::SHADOW`]
//! state (`SHADOW_PARAMETERS`, G:366-375; doc 03 §3.1), for the aircraft shadows (a mesh flattened
//! onto the ground by `core::sun::shadow_matrix`, doc 03 §5.4) and the building, foot-shade and
//! tree shadows (polygons on the ground, docs 01 §9.3, 04 §7.6).
//!
//! The rule: no culling, depth tested (`less-equal`, so buildings in front hide a shadow) but not
//! written, and the stencil compared `equal` to 0 and incremented (clamped) where a shadow draws.
//! The map pass clears the stencil to 0 once per frame, so every pixel takes at most one shadow:
//! the wing over the fuselage does not darken twice, nor do two shadows that overlap, whichever
//! layer they come from. Draw order therefore decides which shadow's colour a pixel gets (doc 04
//! §1: foot shade, then building shadows, then the buildings; all aircraft shadows before any
//! aircraft).
//!
//! What a shadow mesh is lit with: the app draws aircraft shadows with `material: false`, which
//! deck 9.4 reads as luma's default Phong material ([`Material::DECK_DEFAULT`], lit; see
//! [`super::mesh`]), not as unlit. With the shadow matrix flattening the normals onto the ground and
//! the colour `[4,8,16]`, the lit colour is at most a few levels darker than the unlit one. Pass
//! [`Material::DECK_DEFAULT`] for what today's app draws or [`Material::UNLIT`] for the flat colour;
//! fills are always unlit.

use super::fill::Fill;
use super::mesh::MeshPipeline;
use crate::layers::Init;
use crate::state::DrawState;

#[cfg(doc)]
use super::mesh::Material;

/// The instanced mesh pipeline with the shadow rule: place each instance with
/// `mesh::Instance::from_transform` and the shadow matrix, its position on the ground.
pub fn mesh(init: &Init<'_>, label: &str) -> MeshPipeline {
    MeshPipeline::instanced(init, label, DrawState::SHADOW)
}

/// The fill pipeline with the shadow rule: ground polygons in local Mercator.
pub fn fill(init: &Init<'_>, label: &str) -> Fill {
    Fill::new(init, label, DrawState::SHADOW)
}

#[cfg(test)]
mod tests {
    use glam::{DMat3, DVec2, DVec3};
    use rustradar_scene::camera::{ViewState, lng_lat_to_world};

    use super::*;
    use crate::frame::{ViewFrame, power_of_two_scale};
    use crate::gpu::testing::gpu;
    use crate::headless::Image;
    use crate::layers::Layer;
    use crate::pipelines::fill::OPAQUE;
    use crate::pipelines::fill::tests::Squares;
    use crate::pipelines::mesh::tests::{Meshes, Placed, render, save_shot};
    use crate::pipelines::mesh::{LightRig, Material, StyleUniform, Vertex};

    const INK: [u8; 4] = [30, 30, 30, 255];
    /// The aircraft shadows' colour at half alpha.
    const SHADOW: [u8; 4] = [4, 8, 16, 128];

    fn view() -> ViewState {
        ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 15.0, pitch: 40.0, bearing: 20.0, position: None }
    }

    /// The ink with `SHADOW` blended over it `times` times, as the GPU rounds it.
    fn darkened(times: usize) -> [f64; 3] {
        let a = f64::from(SHADOW[3]) / 255.0;
        (0..times).fold([30.0; 3], |c, _| std::array::from_fn(|i| (c[i] + (f64::from(SHADOW[i]) - c[i]) * a).round()))
    }

    fn near(p: [u8; 4], rgb: [f64; 3]) -> bool {
        p[3] == 255 && p.iter().zip(rgb).all(|(g, e)| (f64::from(*g) - e).abs() <= 1.0)
    }

    /// Two squares of ground, the second overlapping the first's east half: the left part, the
    /// overlap and the right part along the middle row (pixels), and the squares' anchors.
    fn squares(size: [f64; 2]) -> (Vec<(DVec2, f64)>, [[u32; 2]; 3]) {
        let v = view();
        let centre = DVec2::from_array(lng_lat_to_world([v.longitude, v.latitude]));
        let scale = power_of_two_scale(50.0 / 2f64.powf(v.zoom));
        let a = centre + DVec2::new(-scale * 0.75, scale * 0.5);
        let b = a + DVec2::new(scale * 0.5, 0.0);
        let vf = ViewFrame::new(&v, size, 1.0, 0.0);
        let px = |w: DVec2| {
            let lnglat = rustradar_scene::camera::world_to_lng_lat([w.x, w.y]);
            let [x, y, _] = vf.viewport.project([lnglat[0], lnglat[1], 0.0]);
            [x.floor() as u32, y.floor() as u32]
        };
        let mid = |x: f64| px(a + DVec2::new(scale * x, -scale * 0.5));
        (vec![(a, scale), (b, scale)], [mid(0.25), mid(0.75), mid(1.25)])
    }

    /// Item E7's acceptance: two overlapping shadow quads drawn with the `SHADOW` state darken their
    /// overlap once. With fills (building and tree shadows) and with an instanced mesh flattened by
    /// the shadow matrix (aircraft shadows), lit by deck's default material and unlit; and, as the
    /// control, without the stencil (`SURFACE_NO_CULL`) the overlap darkens twice.
    #[test]
    fn overlapping_shadows_darken_once() {
        let Some(gpu) = gpu() else { return };
        let size = [240.0, 200.0];
        let (anchors, [left, overlap, right]) = squares(size);
        let once = darkened(1);
        let twice = darkened(2);
        let check = |image: &Image, what: &str, stencil: bool| {
            eprintln!("{what}: {:?} {:?} {:?}", image.pixel(left[0], left[1]), image.pixel(overlap[0], overlap[1]), image.pixel(right[0], right[1]));
            assert!(near(image.pixel(left[0], left[1]), once), "{what} left: {:?} against {once:?}", image.pixel(left[0], left[1]));
            assert!(near(image.pixel(right[0], right[1]), once), "{what} right: {:?}", image.pixel(right[0], right[1]));
            let expected = if stencil { once } else { twice };
            assert!(near(image.pixel(overlap[0], overlap[1]), expected), "{what} overlap: {:?} against {expected:?}", image.pixel(overlap[0], overlap[1]));
            assert_eq!(image.pixel(2, 2), INK, "{what}");
        };
        // fills
        for (state, stencil) in [(DrawState::SHADOW, true), (DrawState::SURFACE_NO_CULL, false)] {
            let image = render(&gpu, view(), size, |init| {
                let mut s = Squares::new(init, state);
                for &(anchor, scale) in &anchors {
                    s.add(init, anchor, scale, SHADOW, OPAQUE, false);
                }
                Box::new(s) as Box<dyn Layer>
            });
            if stencil {
                save_shot(&image, "e7-shadow-fills.png");
            }
            check(&image, &format!("fills {state:?}"), stencil);
        }
        // meshes: the same squares in model metres, standing 300 m up, flattened onto the ground by
        // the shadow matrix with a high sun from the south-west (the shadow falls north-east)
        let v = view();
        let vf = ViewFrame::new(&v, size, 1.0, 0.0);
        let side = anchors[0].1 / vf.viewport.distance_scales.units_per_meter[2];
        let square = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
            .map(|[x, y]| Vertex { position: [x, -y, 0.0], normal: [0.0, 0.0, 1.0], color: Vertex::WHITE });
        let to_sun = DVec3::new(-0.3, -0.3, 0.9).normalize().to_array();
        let height = 300.0;
        let k = [to_sun[0] / to_sun[2], to_sun[1] / to_sun[2]];
        let placed = |&(anchor, _): &(DVec2, f64)| {
            let transform = rustradar_core::sun::shadow_matrix([0.0; 3], [side, side, side], to_sun, height, 0.0);
            // the instance stands above the square's north-west corner; its shadow lands on the
            // square itself once moved back against the sun's slant
            let lnglat = rustradar_scene::camera::world_to_lng_lat([anchor.x, anchor.y]);
            let back = DVec3::new(k[0] * height, k[1] * height, 0.0);
            let model = DMat3::from_cols(
                DVec3::new(transform[0], transform[1], transform[2]),
                DVec3::new(transform[4], transform[5], transform[6]),
                DVec3::new(transform[8], transform[9], transform[10]),
            );
            Placed {
                position: [lnglat[0], lnglat[1], 0.0],
                model,
                translation: DVec3::new(transform[12], transform[13], transform[14]) + back,
                color: SHADOW,
                highlight: [0; 4],
            }
        };
        for (material, state, stencil) in [
            (Material::UNLIT, DrawState::SHADOW, true),
            (Material::UNLIT, DrawState::SURFACE_NO_CULL, false),
        ] {
            let image = render(&gpu, v, size, |init| {
                let mut m = Meshes::new(init, state, LightRig::new());
                m.add(init, &square, Some(&[0, 1, 2, 0, 2, 3]), StyleUniform::new(&material, 1.0, [255; 4]), anchors.iter().map(placed).collect());
                Box::new(m) as Box<dyn Layer>
            });
            if stencil {
                save_shot(&image, "e7-shadow-meshes.png");
            }
            check(&image, &format!("meshes {state:?}"), stencil);
        }
        // lit by deck's default material: a shade darker than the flat colour, still only once
        let image = render(&gpu, v, size, |init| {
            let mut m = Meshes::new(init, DrawState::SHADOW, LightRig::new());
            m.add(init, &square, Some(&[0, 1, 2, 0, 2, 3]), StyleUniform::new(&Material::DECK_DEFAULT, 1.0, [255; 4]), anchors.iter().map(placed).collect());
            Box::new(m) as Box<dyn Layer>
        });
        let [l, o, r] = [left, overlap, right].map(|[x, y]| image.pixel(x, y));
        eprintln!("lit shadow meshes: {l:?} {o:?} {r:?}");
        assert!(l == o && o == r, "lit: {l:?} {o:?} {r:?}");
        assert!(l[..3].iter().zip(once).all(|(g, e)| f64::from(*g) <= e + 1.0 && f64::from(*g) >= e - 4.0), "lit: {l:?} against {once:?}");
    }
}
