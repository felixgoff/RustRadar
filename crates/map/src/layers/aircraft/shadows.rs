//! The aircraft's shadows on the ground, cast by the real sun (item A6; doc 03 §5.4, G:2805-2868,
//! `shadow.ts`): row 25 of doc 03 §3, `shadow-{groupId}-{detail}`, one instanced draw per mesh
//! group, every one of them before any aircraft (the aircraft layer draws this part first).
//!
//! Zoomed in past 9 the scene's mesh groups ([`AircraftScene::groups`], item A3) cast shadows,
//! faded in over zooms 9 to 11 ([`shadow_fade`], `SHADOW_ZOOM`). For each aircraft of a group, in
//! the group's order ([`casts`]):
//!
//! - toward the sun as east, north and up at the aircraft (`localSun(sunDirection(now), lat,
//!   lon)`, [`rustradar_core::sun`]), `now` the feed clock of the scene refresh
//!   (`SceneInput::feed_now_ms`, as today's `motion.now()` in `buildLayers`);
//! - its opacity `shadowOpacity(altFt, size, up) · shadowFade` from its true altitude and size (so
//!   the fade does not change with zoom): none at night, none for a sun below the horizon, fading
//!   from 6° of sun altitude down and with the slant distance; fainter than `SHADOW_MIN_ALPHA`
//!   (0.05) and it casts none;
//! - the aircraft's own orientation and scale (`[s, s, s · flatten]`) squashed onto the ground along
//!   the sun's rays from the height it is drawn at (its raised position, exaggeration included):
//!   `shadowMatrix(orientation, scale, toSun, height)`, an oblique projection that lands the shadow
//!   `height / tan(sun altitude)` from the point below the aircraft, away from the sun;
//! - placed at `[lon, lat, 0]`, coloured `[4, 8, 16, round(alpha · 255)]` (`SHADOW_COLOR`).
//!
//! Each group draws its model's whole mesh (`shape.combined`: the published model's level of
//! detail, or the built-in model), never the gear, with the mesh pipeline in the `SHADOW` state
//! ([`crate::pipelines::shadow::mesh`]: no culling, depth tested and not written, the stencil
//! compared with 0 and incremented), so a pixel is darkened once however many triangles, aircraft
//! or building shadows cover it, and buildings in front hide it. `material: false` is what today's
//! app gives the layer, which deck 9.4 draws with luma's default Phong material, lit
//! ([`MATERIAL`]; see [`crate::pipelines::mesh`]): with the shadow matrix laying the normals flat,
//! the lit colour lies between its ambient part (0.35 × `[4, 8, 16]`) and a little over the flat
//! colour, so a shadow at its darkest is 0 to 5 levels darker than `[4, 8, 16]` would make it. Not
//! pickable, no highlight.
//!
//! Specks. On the multisampled map target a pixel is shaded once, at its centre, even when the
//! triangle covers only some of its samples; a wall of the fuselage laid flat by the shadow matrix
//! is a sliver, and its normal (interpolated, not renormalised per fragment, as deck's) can then be
//! extrapolated far past unit length and blow up the specular term, so a few samples come out
//! light. On SwiftShader that is 8 of 1364 pixels well inside the test's shadows on the flat map
//! and 1 of 1346 on the globe (test `shadows_land_where_the_sun_casts_them_on_the_flat_map`).
//! deck's shaders interpolate the same way on today's (multisampled) canvas, so this is kept as
//! deck draws it; centroid interpolation of the normal in `mesh.wgsl` would remove the specks for
//! every lit mesh, and [`Material::UNLIT`] here would too, at most 5 levels lighter.
//!
//! Instances are rebuilt in each `prepare` from the scene and this frame's view (common offsets
//! from the scene refresh's centre, plan 1.4.2) and written to the GPU only when they change.
//!
//! Deviations:
//! - **No flipped sun on the globe** (plan 1.4.2, doc 03 §20.3). deck's globe frame for mesh
//!   instances is the flat one turned half a turn about up, so today's app passes `[−e, −n, up]`
//!   there with the yaw offset of 180°; the map places every mesh in a true east, north, up frame
//!   ([`crate::pipelines::mesh::local_frame`]) with `[e, n, up]` and no yaw offset (as
//!   [`rustradar_scene::aircraft::orientation_of`]). The two are the same shadow (test
//!   `the_true_frame_casts_decks_flipped_shadow`).
//! - Not yet: the terrain extension (stage 9: the shadow's anchor raised by the ground height,
//!   drawn into the terrain's cover textures). Shadows draw in the map pass only.

use std::ops::Range;

use rustradar_core::num::{clamp_u8, clamp01, js_round};
use rustradar_core::sun::{SHADOW_MIN_ALPHA, local_sun, shadow_matrix, shadow_opacity, sun_direction};
use rustradar_scene::aircraft::{AircraftScene, Group, GroupModel, MeshInstance};
use rustradar_scene::aircraft_models::ModelKind;

use super::assets::{AircraftAssets, AircraftShape};
use crate::frame::ViewFrame;
use crate::layers::{DrawCx, Init, Pass, PrepareCx};
use crate::pipelines::mesh::{Instance, Instances, Material, MeshPipeline, Style, StyleUniform};
use crate::pipelines::shadow;

/// Shadows fade in over these zooms (`SHADOW_ZOOM`, G:356): the models have full depth from zoom 9,
/// and before that a shadow would be a speck under a floor-sized silhouette.
pub const SHADOW_ZOOM: [f64; 2] = [9.0, 11.0];

/// The app's navy ink, so a shadow darkens the map rather than greying it (`SHADOW_COLOR`, G:358).
pub const SHADOW_COLOR: [u8; 3] = [4, 8, 16];

/// The shadows' material: `material: false`, which deck 9.4 draws with luma's default Phong
/// material, lit.
pub const MATERIAL: Material = Material::DECK_DEFAULT;

/// How far the shadows have faded in at `zoom` (`shadowFade`, G:2807): 0 up to zoom 9, 1 from 11.
pub fn shadow_fade(zoom: f64) -> f64 {
    clamp01((zoom - SHADOW_ZOOM[0]) / (SHADOW_ZOOM[1] - SHADOW_ZOOM[0]))
}

/// One aircraft's shadow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cast {
    /// The aircraft in its group's [`Group::instances`].
    pub instance: usize,
    /// Toward the sun as east, north and up at the aircraft (`localSun`; up is the sine of the
    /// sun's altitude).
    pub to_sun: [f64; 3],
    /// `shadowOpacity(altFt, size, up) · shadowFade`.
    pub alpha: f64,
    /// `[lon, lat, 0]`: on the ground below the aircraft.
    pub position: [f64; 3],
    /// deck's `getTransformMatrix`: `shadowMatrix(orientation, scale, toSun, height)`, column-major,
    /// metres east, north and up at [`Self::position`].
    pub transform: [f64; 16],
    /// `[4, 8, 16, round(alpha · 255)]`, the alpha read back from the `Float32Array` today's app
    /// keeps it in ([`color_alpha`]).
    pub color: [u8; 4],
}

impl Cast {
    /// The shadow as a mesh instance in `view`.
    pub fn instance(&self, view: &ViewFrame) -> Instance {
        Instance::from_transform(view, self.position, &self.transform, self.color, Instance::NO_HIGHLIGHT)
    }
}

/// The shadow of `aircraft` (the `index`-th of its group) with the sun along `sun`
/// ([`sun_direction`]) and the shadows faded in by `fade`, if it casts one (G:2820-2831).
pub fn cast(index: usize, aircraft: &MeshInstance, sun: [f64; 3], fade: f64) -> Option<Cast> {
    let pose = &aircraft.pose;
    let to_sun = local_sun(sun, pose.lat, pose.lon);
    // the true size and height, so the fade doesn't change with zoom
    let alpha = shadow_opacity(pose.alt_ft, aircraft.size_m, to_sun[2]) * fade;
    if alpha < SHADOW_MIN_ALPHA {
        return None;
    }
    let [lon, lat, height] = aircraft.position;
    // aircraft are raised onto their wheels, so the ground is the map
    let transform = shadow_matrix(aircraft.orientation, aircraft.scale, to_sun, height, 0.0);
    let [r, g, b] = SHADOW_COLOR;
    Some(Cast { instance: index, to_sun, alpha, position: [lon, lat, 0.0], transform, color: [r, g, b, color_alpha(alpha)] })
}

/// The colour's alpha for a shadow of opacity `alpha`: `Math.round(alpha[i] · 255)`, where today's
/// app keeps the opacities in a `Float32Array` (G:2818), so the product rounds from the nearest
/// `f32`.
pub fn color_alpha(alpha: f64) -> u8 {
    clamp_u8(js_round(f64::from(alpha as f32) * 255.0))
}

/// The shadows `group` casts, in its aircraft's order (the layer's `data: casting`).
pub fn casts(group: &Group, sun: [f64; 3], fade: f64) -> Vec<Cast> {
    group.instances.iter().enumerate().filter_map(|(i, aircraft)| cast(i, aircraft, sun, fade)).collect()
}

/// The mesh a group's shadows are drawn with: its model's whole mesh at the group's level of detail.
#[derive(Debug, Clone, PartialEq)]
enum Mesh {
    Published { name: String, lod: usize },
    BuiltIn(ModelKind),
}

impl Mesh {
    fn of(group: &Group) -> Mesh {
        match &group.model {
            GroupModel::Published { name, lod, .. } => Mesh::Published { name: name.clone(), lod: *lod },
            GroupModel::BuiltIn(kind) => Mesh::BuiltIn(*kind),
        }
    }

    /// The shape on the GPU, if `assets` have it.
    fn shape<'a>(&self, assets: &'a AircraftAssets) -> Option<&'a AircraftShape> {
        match self {
            Mesh::Published { name, lod } => assets.model(name)?.lods.get(*lod),
            Mesh::BuiltIn(kind) => Some(assets.built_in(*kind)),
        }
    }
}

/// One group's shadow layer: its mesh and its instances on the GPU, kept between frames.
struct Draw {
    mesh: Mesh,
    instances: Instances,
    written: Vec<Instance>,
}

/// Row 25: the shadows of every mesh group.
pub struct Shadows {
    pipeline: MeshPipeline,
    style: Style,
    /// One per group that casts this frame (the first [`Self::active`]), reused across frames.
    draws: Vec<Draw>,
    active: usize,
}

impl Shadows {
    pub fn new(init: &Init<'_>) -> Self {
        let pipeline = shadow::mesh(init, "aircraft shadows");
        let style = pipeline.style(&init.gpu.device, &StyleUniform::new(&MATERIAL, 1.0, [255; 4]));
        Shadows { pipeline, style, draws: Vec::new(), active: 0 }
    }

    /// Once a frame, after [`AircraftAssets::poll`]: this frame's shadows of `scene`'s mesh groups,
    /// with the sun at the scene's feed clock.
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, assets: &AircraftAssets, scene: &AircraftScene) {
        self.active = 0;
        let fade = shadow_fade(scene.frame.zoom);
        if fade.is_nan() || fade <= 0.0 || scene.groups.is_empty() {
            return;
        }
        let sun = sun_direction(cx.scene.feed_now_ms);
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        for group in &scene.groups {
            let mesh = Mesh::of(group);
            if mesh.shape(assets).is_none() {
                continue;
            }
            let casting = casts(group, sun, fade);
            if casting.is_empty() {
                continue;
            }
            let instances: Vec<Instance> = casting.iter().map(|c| c.instance(cx.view)).collect();
            if self.draws.len() == self.active {
                self.draws.push(Draw { mesh: mesh.clone(), instances: Instances::new(device, "aircraft shadow instances"), written: Vec::new() });
            }
            let draw = &mut self.draws[self.active];
            draw.mesh = mesh;
            if instances != draw.written {
                draw.instances.write(device, queue, &instances);
                draw.written = instances;
            }
            self.active += 1;
        }
        if self.active > 0 {
            cx.set_lights(&self.pipeline);
        }
    }

    /// Every group's shadows, in group order (map pass only).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, assets: &AircraftAssets) {
        if cx.pass != Pass::Map {
            return;
        }
        for draw in &self.draws[..self.active] {
            let Some(shape) = draw.mesh.shape(assets) else { continue };
            let elements: Range<u32> = shape.combined();
            self.pipeline.draw_range(pass, &self.style, &shape.shape, elements, &draw.instances);
        }
    }

    /// How many shadows this frame draws.
    pub fn drawn(&self) -> usize {
        self.draws[..self.active].iter().map(|d| d.instances.count() as usize).sum()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use glam::{DMat3, DVec3};
    use rustradar_core::geo::Basemap;
    use rustradar_core::liveries::DEFAULT_LIVERY;
    use rustradar_core::model::{LiveFlight, f32_as_js};
    use rustradar_core::motion::aircraft::{Phase, Pose};
    use rustradar_core::sun::{SHADOW_MAX_ALPHA, sunlight};
    use rustradar_scene::aircraft::{AircraftBuilder, Inputs, orientation_of};
    use rustradar_scene::aircraft_models;
    use rustradar_scene::camera::{ViewState, viewport_for};

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::layers::Layer;
    use crate::layers::aircraft::assets::published;
    use crate::pipelines::mesh::local_frame;
    use crate::renderer::{MapRenderer, SceneInput};

    /// 2026-06-21 10:50 UTC: midsummer's local (solar) noon at Arlanda, the sun due south 53.8° up.
    const NOON_MS: f64 = 1_782_039_000_000.0;
    /// 22:50 UTC the same day: local midnight, the sun 7° below the horizon.
    const NIGHT_MS: f64 = 1_782_082_200_000.0;
    /// The canvas, CSS px, drawn at twice its size.
    const SIZE: [f64; 2] = [480.0, 320.0];
    const SCALE: f64 = 2.0;
    /// Above Arlanda, on the flat map, pitched and turned (V4's camera, its zoom just short).
    const FLAT: ViewState = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, bearing: 30.0, pitch: 45.0, position: None };
    /// The same on the globe, the shadows three quarters faded in; tilted past 5°, so the published
    /// models are meshes, not icons.
    const GLOBE: ViewState = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 10.5, bearing: 0.0, pitch: 30.0, position: None };
    /// The aircraft's height above the ground, metres.
    const HEIGHT_M: f64 = 300.0;

    /// One aircraft: id, ICAO type, icon class, heading, pitch, bank, the canvas point (CSS px) its
    /// ground point is under, and metres east of it.
    type Row = (u32, &'static str, &'static str, f64, f64, f64, [f64; 2], f64);

    /// An A320 level, a 747 banked and climbing, a balloon (a built-in model), and a second A320
    /// 15 m east of the first, so the two shadows overlap.
    const ROWS: [Row; 4] = [
        (1, "A20N", "A320", 30.0, 0.0, 0.0, [130.0, 230.0], 0.0),
        (2, "B744", "B747", 200.0, 5.0, 20.0, [260.0, 250.0], 0.0),
        (3, "BALL", "BALL", 0.0, 0.0, 0.0, [380.0, 230.0], 0.0),
        (4, "A20N", "A320", 30.0, 0.0, 0.0, [130.0, 230.0], 15.0),
    ];

    fn flights(view: &ViewState, rows: &[Row]) -> Vec<LiveFlight> {
        let viewport = viewport_for(view, SIZE);
        rows.iter()
            .map(|&(id, typecode, icon, heading, _, _, xy, east)| {
                let [lon, lat] = viewport.unproject(xy, 0.0);
                let lon = lon + east / (6_371_000.0 * lat.to_radians().cos()) * 180.0 / std::f64::consts::PI;
                LiveFlight {
                    id,
                    lat: lat as f32,
                    lon: lon as f32,
                    alt: (HEIGHT_M / 0.3048) as i32,
                    track: heading as i32,
                    callsign: format!("TST{id}"),
                    typecode: typecode.into(),
                    icon: icon.into(),
                    ..Default::default()
                }
            })
            .collect()
    }

    /// The reported position as the pose, `HEIGHT_M` up, posed as its row says.
    fn pose(rows: &[Row]) -> impl Fn(&LiveFlight) -> Pose + '_ {
        move |f| {
            let &(_, _, _, heading, pitch, bank, _, _) = rows.iter().find(|r| r.0 == f.id).unwrap();
            Pose {
                lon: f32_as_js(f.lon),
                lat: f32_as_js(f.lat),
                alt_ft: HEIGHT_M / 0.3048,
                heading,
                track: heading,
                pitch,
                bank,
                gear: 0.0,
                phase: Phase::Cruise,
            }
        }
    }

    /// The rows as the scene builder makes them in `view`: every one a mesh.
    fn aircraft(view: &ViewState, rows: &[Row]) -> AircraftScene {
        let inputs = Inputs { view: *view, size: SIZE, exaggeration: 1.0, show_aircraft: true, labelling: false, selected: None, follow: false, hovered: None };
        let scene = AircraftBuilder::new().build(&inputs, published(), &flights(view, rows), pose(rows), |_| DEFAULT_LIVERY);
        assert_eq!(scene.mesh_count(), rows.len(), "all drawn as meshes");
        scene
    }

    fn sun_at(now_ms: f64) -> [f64; 3] {
        sun_direction(now_ms)
    }

    #[test]
    fn shadows_fade_in_from_zoom_9_to_11() {
        for (zoom, fade) in [(3.0, 0.0), (9.0, 0.0), (9.5, 0.25), (10.0, 0.5), (10.5, 0.75), (11.0, 1.0), (15.0, 1.0)] {
            assert_eq!(shadow_fade(zoom), fade, "zoom {zoom}");
        }
    }

    /// Each aircraft's shadow is today's (G:2820-2862): the sun at the aircraft, the opacity from
    /// its true height and size, the aircraft's own orientation and scale squashed onto the ground
    /// from its drawn height, at its point on the ground, in the navy ink at that opacity. At noon
    /// it falls `height / tan(sun altitude)` north of the point below the aircraft.
    #[test]
    fn a_cast_is_todays_shadow() {
        let scene = aircraft(&FLAT, &ROWS);
        let sun = sun_at(NOON_MS);
        for group in &scene.groups {
            let all = casts(group, sun, 1.0);
            assert_eq!(all.len(), group.instances.len(), "every one casts at noon from 300 m");
            for (k, c) in all.iter().enumerate() {
                let a = &group.instances[c.instance];
                assert_eq!(c.instance, k);
                let to_sun = local_sun(sun, a.pose.lat, a.pose.lon);
                assert_eq!(c.to_sun, to_sun);
                let alpha = shadow_opacity(a.pose.alt_ft, a.size_m, to_sun[2]);
                assert_eq!(c.alpha, alpha);
                assert_eq!(c.position, [a.position[0], a.position[1], 0.0]);
                assert_eq!(c.transform, shadow_matrix(orientation_of(&a.pose), a.scale, to_sun, a.position[2], 0.0));
                assert_eq!(a.orientation, orientation_of(&a.pose));
                assert_eq!(c.color, [4, 8, 16, js_round(f64::from(alpha as f32) * 255.0) as u8]);
                // the sun due south, 54° up: the shadow falls north by height / tan(altitude)
                let altitude = to_sun[2].asin().to_degrees();
                assert!((altitude - 53.8).abs() < 0.1 && to_sun[0].abs() < 0.002 && to_sun[1] < 0.0, "{to_sun:?} at {altitude}°");
                let t = [c.transform[12], c.transform[13], c.transform[14]];
                let reach = a.position[2] / altitude.to_radians().tan();
                assert!((t[0].hypot(t[1]) - reach).abs() < 1e-9 && t[1] > 0.0 && t[2] == 0.0, "{t:?} for {reach}");
                // everything lands on the ground: the matrix has no third row
                assert_eq!([c.transform[2], c.transform[6], c.transform[10], c.transform[14]], [0.0; 4]);
            }
        }
        // 300 m is well inside crisp (11 lengths along the ray): the darkest a shadow gets
        let a320 = &scene.groups[0].instances[0];
        assert_eq!(a320.id, 1);
        let c = cast(0, a320, sun, 1.0).unwrap();
        assert_eq!((c.alpha, c.color[3]), (SHADOW_MAX_ALPHA, 128));
        // faded in by half at zoom 10: half as dark
        let c = cast(0, a320, sun, 0.5).unwrap();
        assert_eq!((c.alpha, c.color[3]), (0.25, 64));
    }

    /// The colour's alpha rounds from the opacity as a `Float32Array` holds it: just under 13.5 / 255
    /// in a double, the nearest `f32` is just over, and today's app draws 14, not 13.
    #[test]
    fn the_colour_alpha_rounds_from_an_f32() {
        let a = 0.052_941_176_469_588_236;
        assert!(a * 255.0 < 13.5 && f64::from(a as f32) * 255.0 > 13.5);
        assert_eq!(color_alpha(a), 14);
        assert_eq!((color_alpha(0.5), color_alpha(0.25), color_alpha(SHADOW_MIN_ALPHA)), (128, 64, 13));
    }

    /// None at night, none faded out (zoom 9), none fainter than 0.05: high up, or under a sun
    /// just over the horizon.
    #[test]
    fn no_shadow_at_night_or_too_faint() {
        let scene = aircraft(&FLAT, &ROWS);
        let group = &scene.groups[0];
        assert_eq!(casts(group, sun_at(NIGHT_MS), 1.0), vec![]);
        assert_eq!(casts(group, sun_at(NOON_MS), 0.0), vec![]);
        // an A320's shadow is gone by about 5,500 ft under a high sun
        let mut high = group.instances[0];
        high.pose.alt_ft = 6_000.0;
        assert_eq!(cast(0, &high, sun_at(NOON_MS), 1.0), None);
        high.pose.alt_ft = 2_500.0;
        let faint = cast(0, &high, sun_at(NOON_MS), 1.0).expect("still cast at 2,500 ft");
        assert!(faint.alpha < 0.3 && faint.alpha >= SHADOW_MIN_ALPHA, "{}", faint.alpha);
        // the sun 0.3° up (sunlight 0.05): 0.025, not cast even on the ground
        let mut grounded = group.instances[0];
        grounded.pose.alt_ft = 0.0;
        let low = [0.3f64.to_radians().sin(), 0.0, 0.3f64.to_radians().cos()];
        let up = local_sun(low, 0.0, 0.0)[2];
        assert!((sunlight(up) - 0.05).abs() < 1e-9);
        grounded.pose.lat = 0.0;
        grounded.pose.lon = 0.0;
        assert_eq!(cast(0, &grounded, low, 1.0), None);
    }

    /// The deviation of the module's documentation is invisible: deck's globe frame (east and north
    /// turned half a turn, the yaw offset 180° and the sun flipped) gives the same shadow as the
    /// true east, north, up frame, its east and north rows negated.
    #[test]
    fn the_true_frame_casts_decks_flipped_shadow() {
        let sun = sun_at(NOON_MS);
        for (lat, lon, heading, pitch, bank) in [(59.65, 17.92, 30.0, 0.0, 0.0), (59.65, 17.92, 200.0, 5.0, 20.0), (-33.9, 151.2, 317.0, -3.0, -25.0), (40.6, -73.8, 91.0, 12.0, 8.0)] {
            let pose = Pose { lon, lat, alt_ft: 3000.0, heading, track: heading, pitch, bank, gear: 0.0, phase: Phase::Cruise };
            let scale = [1.7, 1.7, 1.7 * 0.6];
            let [e, n, up] = local_sun(sun, lat, lon);
            let ours = shadow_matrix(orientation_of(&pose), scale, [e, n, up], 400.0, 0.0);
            let deck = shadow_matrix([bank, 180.0 - heading, pitch], scale, [-e, -n, up], 400.0, 0.0);
            for k in 0..16 {
                let expected = if k % 4 < 2 { -ours[k] } else { ours[k] };
                assert!((deck[k] - expected).abs() < 1e-9, "{k}: {} against {expected}", deck[k]);
            }
        }
    }

    /// A test layer: [`Shadows`] with assets of its own, drawing `scene`'s shadows and nothing else.
    struct Only {
        assets: AircraftAssets,
        shadows: Shadows,
        scene: AircraftScene,
        drawn: Rc<Cell<usize>>,
    }

    impl Layer for Only {
        fn name(&self) -> &'static str {
            "a6 shadows only"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.assets.poll(cx.gpu);
            self.shadows.prepare(cx, &self.assets, &self.scene);
            self.drawn.set(self.shadows.drawn());
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.shadows.draw(pass, cx, &self.assets);
        }
    }

    fn input(view: &ViewState, now_ms: f64) -> SceneInput {
        let mut input = SceneInput::new(*view, SIZE, SCALE, Basemap::Dark);
        input.feed_now_ms = now_ms;
        input.local_now_ms = now_ms;
        input
    }

    /// `scene`'s shadows alone over the plain map of `view` at `now_ms`, and how many were drawn.
    fn shoot_only(gpu: &Gpu, view: &ViewState, now_ms: f64, scene: AircraftScene, with_models: bool) -> (Image, usize) {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let drawn = Rc::new(Cell::new(usize::MAX));
        let init = renderer.init(gpu);
        let assets = AircraftAssets::with_models(&init, if with_models { published() } else { None });
        let layer = Only { assets, shadows: Shadows::new(&init), scene, drawn: drawn.clone() };
        renderer.add_layer(Box::new(layer));
        let image = render_with(gpu, &mut renderer, &input(view, now_ms), SCALE).expect("rendered");
        (image, drawn.get())
    }

    /// `aircraft` drawn by the map's own layers (the aircraft layer with its shadows).
    fn shoot(gpu: &Gpu, view: &ViewState, now_ms: f64, aircraft: AircraftScene) -> Image {
        let mut scene = input(view, now_ms);
        scene.aircraft.scene = aircraft;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        render_with(gpu, &mut renderer, &scene, SCALE).expect("rendered")
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    /// The triangles a group's shadows are drawn with: model metres, three corners each.
    fn triangles(group: &Group) -> Vec<[[f32; 3]; 3]> {
        let corner = |p: &[f32], i: usize| [p[3 * i], p[3 * i + 1], p[3 * i + 2]];
        match &group.model {
            GroupModel::Published { name, lod, .. } => {
                let mesh = &published().unwrap().get(name).unwrap().lods[*lod].mesh;
                mesh.indices.chunks_exact(3).map(|t| [0, 1, 2].map(|k| corner(&mesh.positions, usize::from(t[k])))).collect()
            }
            GroupModel::BuiltIn(kind) => {
                let p = &aircraft_models::model(*kind).combined.positions;
                (0..p.len() / 9).map(|t| [0, 1, 2].map(|k| corner(p, 3 * t + k))).collect()
            }
        }
    }

    /// The device pixels whose centres the CPU puts inside `cast`'s shadow of `group`'s mesh in
    /// `view`: each corner placed as the instance places it (in f64) and projected.
    fn raster(view: &ViewFrame, group: &Group, cast: &Cast, mask: &mut [bool], [w, h]: [u32; 2]) {
        let t = &cast.transform;
        let model = DMat3::from_cols(DVec3::new(t[0], t[1], t[2]), DVec3::new(t[4], t[5], t[6]), DVec3::new(t[8], t[9], t[10]));
        let translation = DVec3::new(t[12], t[13], t[14]);
        let [lon, lat, _] = cast.position;
        let frame = local_frame(view, [lon, lat]);
        let origin = view.viewport.project_position([lon, lat, 0.0]);
        let screen = |p: [f32; 3]| {
            let common = origin + frame * (model * DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])) + translation);
            let [x, y, _] = view.viewport.world_to_pixels(common);
            [x * SCALE, y * SCALE]
        };
        for tri in triangles(group) {
            let [a, b, c] = tri.map(screen);
            let edge = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
            let area = edge(a, b, c);
            if area == 0.0 {
                continue;
            }
            let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as u32;
            let x1 = (a[0].max(b[0]).max(c[0]).ceil() as u32).min(w);
            let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as u32;
            let y1 = (a[1].max(b[1]).max(c[1]).ceil() as u32).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                    let [u, v, s] = [edge(b, c, p), edge(c, a, p), edge(a, b, p)].map(|e| e / area);
                    if u >= 0.0 && v >= 0.0 && s >= 0.0 {
                        mask[(y * w + x) as usize] = true;
                    }
                }
            }
        }
    }

    /// How much darker `p` is than `bg` (the mean of the three channels).
    fn darker(bg: [u8; 4], p: [u8; 4]) -> f64 {
        (0..3).map(|c| f64::from(bg[c]) - f64::from(p[c])).sum::<f64>() / 3.0
    }

    /// The shadows of `rows` in `view` at noon (faded in by `fade`) against the CPU's: each shadow
    /// is there, the pixels darkened by at least half a shadow are the CPU's within a pixel's edge
    /// (intersection over union), inside a shadow every pixel is darkened exactly once (the wings
    /// over the fuselage, two shadows overlapping) to the colour of `[4,8,16]` lit by deck's
    /// default material at the cast's alpha, and nothing else changes.
    fn check_against_cpu(gpu: &Gpu, view: &ViewState, rows: &[Row], name: &str) {
        let scene = aircraft(view, rows);
        let fade = shadow_fade(view.zoom);
        let sun = sun_at(NOON_MS);
        let (bg, drawn) = shoot_only(gpu, view, NOON_MS, AircraftScene::default(), true);
        assert_eq!(drawn, 0);
        let (image, drawn) = shoot_only(gpu, view, NOON_MS, scene.clone(), true);
        save(&image, name);
        assert_eq!(drawn, rows.len(), "{name}: one shadow each");
        let (w, h) = (image.width, image.height);
        let at = |i: usize| [i as u32 % w, i as u32 / w];
        let vf = ViewFrame::new(view, SIZE, SCALE, 0.0);
        // per pixel: the alpha of the first shadow the CPU puts over it (the stencil keeps the first
        // drawn), and of the first within a pixel of it
        let mut cpu: Vec<Option<f64>> = vec![None; (w * h) as usize];
        let mut near: Vec<Option<f64>> = cpu.clone();
        // half a shadow of alpha `a` over `b`: toward [4,8,16] by half the alpha
        let half = |b: [u8; 4], a: f64| (f64::from(b[0]) - 4.0) * a / 2.0;
        for group in &scene.groups {
            for c in casts(group, sun, fade) {
                let a = f64::from(c.color[3]) / 255.0;
                let mut own = vec![false; cpu.len()];
                raster(&vf, group, &c, &mut own, [w, h]);
                let n = own.iter().filter(|&&m| m).count();
                let dark = (0..own.len()).filter(|&i| own[i] && { let [x, y] = at(i); darker(bg.pixel(x, y), image.pixel(x, y)) >= half(bg.pixel(x, y), a) }).count();
                eprintln!("{name}: aircraft {} alpha {a:.3}: {n} px, {dark} of them darkened", group.instances[c.instance].id);
                assert!(n > 60 && dark as f64 >= 0.9 * n as f64, "{name}: aircraft {}: {dark} of {n}", group.instances[c.instance].id);
                for i in (0..own.len()).filter(|&i| own[i]) {
                    cpu[i].get_or_insert(a);
                    let [x, y] = at(i);
                    for yy in y.saturating_sub(1)..(y + 2).min(h) {
                        for xx in x.saturating_sub(1)..(x + 2).min(w) {
                            near[(yy * w + xx) as usize].get_or_insert(a);
                        }
                    }
                }
            }
        }
        let (mut both, mut either, mut specks, mut inside_n) = (0, 0, 0, 0);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                let (b, p) = (bg.pixel(x, y), image.pixel(x, y));
                let Some(a) = near[i] else {
                    assert_eq!(p, b, "{name}: ({x}, {y}) changed away from every shadow");
                    continue;
                };
                let gpu_dark = darker(b, p) >= half(b, a);
                both += usize::from(gpu_dark && cpu[i].is_some());
                either += usize::from(gpu_dark || cpu[i].is_some());
                // darkened at most once
                for c in 0..3 {
                    assert!(f64::from(p[c]) >= f64::from(b[c]) * (1.0 - a) - 1.0, "{name}: ({x}, {y}) {p:?} darker than one shadow over {b:?}");
                }
                // well inside: once, by [4,8,16] lit at the alpha (at most the unlit colour), but for
                // the specks of the lit material (see the module's documentation)
                let inside = (y.saturating_sub(1)..(y + 2).min(h)).all(|yy| (x.saturating_sub(1)..(x + 2).min(w)).all(|xx| cpu[(yy * w + xx) as usize].is_some()));
                if inside {
                    let a = cpu[i].unwrap();
                    inside_n += 1;
                    let unlit = |c: usize| f64::from(b[c]) * (1.0 - a) + f64::from(SHADOW_COLOR[c]) * a;
                    if (0..3).any(|c| f64::from(p[c]) > unlit(c) + 1.0) {
                        specks += 1;
                    } else {
                        // lit by deck's default material: no darker than its ambient part alone
                        for c in 0..3 {
                            let ambient = f64::from(b[c]) * (1.0 - a) + f64::from(SHADOW_COLOR[c]) * MATERIAL.ambient * a;
                            assert!(f64::from(p[c]) >= ambient - 1.0, "{name}: ({x}, {y}) {p:?}: at least {ambient:.1} in channel {c}");
                        }
                    }
                }
            }
        }
        eprintln!("{name}: {specks} of {inside_n} pixels well inside a shadow lighter than the flat colour");
        assert!((specks as f64) < 0.02 * inside_n as f64, "{name}: {specks} specks of {inside_n}");
        let iou = both as f64 / either as f64;
        eprintln!("{name}: {both} of {either} shadow pixels agree ({iou:.3})");
        assert!(iou > 0.9, "{name}: {iou}");
    }

    /// The shadows land where the CPU casts them on the flat map, pitched and turned: three
    /// models (one banked and climbing, one built-in), two of them overlapping.
    #[test]
    fn shadows_land_where_the_sun_casts_them_on_the_flat_map() {
        let Some(gpu) = gpu() else { return };
        check_against_cpu(&gpu, &FLAT, &ROWS, "a6-shadows-flat.png");
    }

    /// The same on the globe at zoom 10.5 (three quarters faded in): no half turn, the shadows fall
    /// north as on the flat map.
    #[test]
    fn shadows_land_where_the_sun_casts_them_on_the_globe() {
        let Some(gpu) = gpu() else { return };
        // spread out: the floor makes them 24 px while the sun moves them 8 px
        let rows: [Row; 3] = [
            (1, "A20N", "A320", 30.0, 0.0, 0.0, [120.0, 200.0], 0.0),
            (2, "B744", "B747", 200.0, 5.0, 20.0, [240.0, 200.0], 0.0),
            (3, "BALL", "BALL", 0.0, 0.0, 0.0, [360.0, 200.0], 0.0),
        ];
        check_against_cpu(&gpu, &GLOBE, &rows, "a6-shadows-globe.png");
    }

    /// Nothing is drawn faded out (zoom 9), at night, or without the models on the GPU (assets that
    /// failed to load draw no published group, and the built-in ones still cast).
    #[test]
    fn no_shadow_is_drawn_faded_out_at_night_or_without_the_mesh() {
        let Some(gpu) = gpu() else { return };
        let at_nine = ViewState { zoom: 9.0, ..GLOBE };
        let (_, drawn) = shoot_only(&gpu, &at_nine, NOON_MS, aircraft(&at_nine, &ROWS[..3]), true);
        assert_eq!(drawn, 0, "faded out at zoom 9");
        let (_, drawn) = shoot_only(&gpu, &FLAT, NIGHT_MS, aircraft(&FLAT, &ROWS), true);
        assert_eq!(drawn, 0, "none at night");
        let (_, drawn) = shoot_only(&gpu, &FLAT, NOON_MS, aircraft(&FLAT, &ROWS), false);
        assert_eq!(drawn, 1, "only the balloon's");
    }

    /// Item A6's acceptance, drawn by the map's own aircraft layer: an aircraft 300 m up at local
    /// noon has a dark blob under it, where the sun casts it; at night there is none.
    #[test]
    fn an_aircraft_300_m_up_casts_a_shadow_at_noon_and_none_at_night() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { pitch: 0.0, bearing: 0.0, ..FLAT };
        let rows = [ROWS[0]];
        let plain = shoot(&gpu, &view, NOON_MS, AircraftScene::default());
        let noon = shoot(&gpu, &view, NOON_MS, aircraft(&view, &rows));
        let night = shoot(&gpu, &view, NIGHT_MS, aircraft(&view, &rows));
        let plain_night = shoot(&gpu, &view, NIGHT_MS, AircraftScene::default());
        save(&noon, "a6-noon.png");
        save(&night, "a6-night.png");
        let (w, h) = (noon.width, noon.height);
        let dark_pixels = |image: &Image, under: &Image| -> Vec<[u32; 2]> {
            (0..h).flat_map(|y| (0..w).map(move |x| [x, y])).filter(|&[x, y]| darker(under.pixel(x, y), image.pixel(x, y)) > 8.0).collect()
        };
        let dark = dark_pixels(&noon, &plain);
        let n = dark.len() as f64;
        let centroid = [0, 1].map(|k| dark.iter().map(|p| f64::from(p[k]) + 0.5).sum::<f64>() / n);
        // where the CPU casts it: the shadow's own pixels, their middle
        let scene = aircraft(&view, &rows);
        let cast = cast(0, &scene.groups[0].instances[0], sun_at(NOON_MS), 1.0).unwrap();
        let mut cpu = vec![false; (w * h) as usize];
        raster(&ViewFrame::new(&view, SIZE, SCALE, 0.0), &scene.groups[0], &cast, &mut cpu, [w, h]);
        let cpu_n = cpu.iter().filter(|&&m| m).count() as f64;
        let expected = [0, 1].map(|k| (0..cpu.len()).filter(|&i| cpu[i]).map(|i| f64::from([i as u32 % w, i as u32 / w][k]) + 0.5).sum::<f64>() / cpu_n);
        // and the point below the aircraft
        let below = viewport_for(&view, SIZE).project([cast.position[0], cast.position[1], 0.0]);
        eprintln!("noon: {n} dark pixels around {centroid:?}, the CPU's {cpu_n} around {expected:?}; below the aircraft {:?}", [below[0] * SCALE, below[1] * SCALE]);
        assert!(n > 200.0 && (n / cpu_n - 1.0).abs() < 0.1, "{n} for {cpu_n}");
        assert!((centroid[0] - expected[0]).abs() < 1.0 && (centroid[1] - expected[1]).abs() < 1.0, "{centroid:?} for {expected:?}");
        // north of the point below (up the screen) by the drawn height over the tangent of the sun's
        // altitude: 300 m and the wheels' raise / tan(53.8°) ≈ 225 m, 66 px at 3.4 m a pixel, give
        // or take where the airframe's area lies about its middle (drawn 2.2 times its size here)
        let mpp = viewport_for(&view, SIZE).meters_per_pixel();
        let height = scene.groups[0].instances[0].position[2];
        let reach = height / cast.to_sun[2].asin().tan();
        let [east, north] = [(centroid[0] - below[0] * SCALE) / SCALE * mpp, (below[1] * SCALE - centroid[1]) / SCALE * mpp];
        eprintln!("the shadow's middle {east:.1} m east and {north:.1} m north of the point below, {reach:.1} m expected");
        assert!(east.abs() < 8.0 && (north - reach).abs() < 8.0, "{east} m east, {north} m north for {reach} m");
        // the darkest a shadow gets: the map half way to the navy ink
        let [x, y] = centroid.map(|c| c as u32);
        let (p, b) = (noon.pixel(x, y), plain.pixel(x, y));
        eprintln!("at the middle {p:?} over {b:?}");
        assert!((darker(b, p) - (f64::from(b[0]) - 8.0) * 128.0 / 255.0).abs() < 4.0, "{p:?} over {b:?}");
        assert_eq!(dark_pixels(&night, &plain_night), Vec::<[u32; 2]>::new(), "no shadow at night");
        assert!((0..h).all(|y| (0..w).all(|x| night.pixel(x, y)[..3].iter().zip(&plain_night.pixel(x, y)[..3]).all(|(a, b)| a >= b))), "nothing darker at night");
    }
}
