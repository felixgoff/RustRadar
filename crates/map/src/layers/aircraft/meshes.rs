//! The aircraft drawn as lit 3D models, with their landing gear (item A5; doc 03 §5.1–5.3,
//! G:2870-2955): row 26 of doc 03 §3, for each mesh group in group order `gear-{groupId}` and then
//! `aircraft-{groupId}-{detail}` (or `aircraft-{groupId}-{detail}-{part}` for each livery part).
//! The aircraft layer draws this part after every shadow (item A6) and before the icons (A4).
//!
//! The scene builder (item A3, [`rustradar_scene::aircraft`]) has already worked out every mesh
//! instance of the refresh ([`AircraftScene::groups`]): one group per published model (`t-<name>`,
//! at its level of detail `min(detail, lods − 1)`) or built-in model (`k-<kind>`), in order of their
//! first aircraft in the feed, each aircraft raised onto its wheels, with deck's `getOrientation`
//! (`[bank, −heading, pitch]`), `getScale` (`[s, s, s · max(0.04, depth)]`,
//! `s = screenPx(size) · mpp / divisor`), its colour and, while liveries show, each part's colour.
//! This part turns them into deck's SimpleMeshLayers, drawn with
//! [`crate::pipelines::mesh::MeshPipeline::instanced`]:
//!
//! | Draw | When | Mesh | Instances | Colour | Highlight |
//! |---|---|---|---|---|---|
//! | gear (first) | the published model has gear, depth ≥ `GEAR_DEPTH` 0.8 | the gear | the aircraft with `pose.gear > 0` ([`Group::gear`]) | `GEAR_COLOR` `[178,182,190]` | none (not pickable) |
//! | airframe | `liveryAmount == 0` (zoom ≤ 6) | the whole mesh (`combined`) | every aircraft of the group | selection blue for the selected flight, else white | the hovered aircraft, `[55,142,240,150]` |
//! | one per part | `liveryAmount > 0` | the part's triangles, in the mesh's part order | every aircraft of the group | white mixed toward the livery by `liveryAmount` | the same, on every part |
//!
//! Every draw is deck's SimpleMeshLayer otherwise: the material `meshMaterial(depth)`
//! ([`material`]: lit at depth 1, the flat colour at depth 0, zoomed out), the scene's light rig
//! (`SceneInput::lights`, item T1), opaque, and the `NO_CULL` draw state (depth tested and written,
//! no culling: [`STATE`]).
//!
//! Each instance's matrix is deck's (`calculateTransformMatrix`: `R = Rz(yaw) · Ry(o[0]) · Rx(o[2])`
//! then the scale, [`crate::pipelines::mesh::orientation_matrix`]) in a true east, north, up frame
//! at the aircraft ([`crate::pipelines::mesh::local_frame`]), worked out on the CPU in f64; the
//! gear's `getTranslation` (its attach plane kept fixed as it retracts: `attachZ · (1 − gear) · s ·
//! flatten` along the airframe's up axis) moves it in that frame ([`Instance::at`]).
//!
//! Instances are rebuilt in each `prepare` from the scene and this frame's view (common offsets
//! from the scene refresh's centre, plan 1.4.2) and written to the GPU only when they change, so
//! between refreshes nothing is uploaded. The meshes are on the GPU from the first frame
//! ([`AircraftAssets`], item A2); a group whose model the assets lack draws nothing.
//!
//! Deviations:
//! - **No `yawOffset`** (plan 1.4.2, doc 03 §20.3). deck's globe frame for mesh instances is the
//!   flat one turned half a turn about up, so today's app passes `180 − heading` on the globe; here
//!   the frame is east, north, up in both projections and the orientation `[bank, −heading,
//!   pitch]` ([`rustradar_scene::aircraft::orientation_of`]). The two place every vertex at the
//!   same point (test `decks_turned_globe_frame_is_the_true_frame`), and an aircraft keeps its
//!   heading across zoom 12 (test `an_aircraft_keeps_its_heading_across_the_projection_swap`).
//! - Each mesh is uploaded once and its parts are ranges of it, where deck makes one geometry per
//!   part; the triangles drawn are the same.
//! - Not yet: the terrain extension (stage 9: the aircraft raised by the ground height).

use std::ops::Range;

use glam::DVec3;
use rustradar_core::liveries::Part;
use rustradar_scene::aircraft::{AircraftScene, Group, GroupModel, HIGHLIGHT_COLOR};
use rustradar_scene::aircraft_models::ModelKind;

use super::assets::{AircraftAssets, AircraftShape};
use crate::frame::ViewFrame;
use crate::layers::{DrawCx, Init, Pass, PrepareCx};
use crate::pipelines::mesh::{Instance, Instances, Material, MeshPipeline, Shape, Style, StyleUniform, orientation_matrix};
use crate::state::DrawState;

/// Every draw's state (`parameters: NO_CULL`): depth tested `less-equal` and written, no culling.
pub const STATE: DrawState = DrawState::NO_CULL;

/// The aircraft's and the gear's material at model depth `depth` (`meshMaterial(depth)`,
/// G:401-413): most of the light ambient at depth 1, the flat colour at depth 0.
pub fn material(depth: f64) -> Material {
    Material::aircraft(depth)
}

/// What one draw of a group draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elements {
    /// The landing gear (`gear-{groupId}`).
    Gear,
    /// The whole airframe in one colour (`aircraft-{groupId}-{detail}`).
    Combined,
    /// One livery part of the airframe (`aircraft-{groupId}-{detail}-{part}`).
    Part(Part),
}

/// The draws of one group, in today's order: the gear first when `gear` (the group has gear down
/// and its model has a gear mesh), then the airframe whole when `livery_amount` is 0, else each of
/// the mesh's `parts` in its order (G:2894-2954).
pub fn draws(gear: bool, livery_amount: f64, parts: impl IntoIterator<Item = Part>) -> Vec<Elements> {
    let mut draws = Vec::new();
    if gear {
        draws.push(Elements::Gear);
    }
    if livery_amount == 0.0 {
        draws.push(Elements::Combined);
    } else {
        draws.extend(parts.into_iter().map(Elements::Part));
    }
    draws
}

/// One instance: a model at `position` (`[lon, lat, metres]`) turned by deck's `orientation` and
/// scaled by `scale`, moved by `translation` (metres east, north, up), in `view`.
pub fn instance(view: &ViewFrame, position: [f64; 3], orientation: [f64; 3], scale: [f64; 3], translation: [f64; 3], color: [u8; 4], highlight: [u8; 4]) -> Instance {
    Instance::at(view, position, orientation_matrix(orientation, scale), DVec3::from_array(translation), color, highlight)
}

/// The airframes of `group` in `view`, in its aircraft's order, each in the colour `color` gives
/// it; the hovered one ([`Group::highlighted`]) highlighted [`HIGHLIGHT_COLOR`].
fn airframes(view: &ViewFrame, group: &Group, color: impl Fn(usize) -> [u8; 4]) -> Vec<Instance> {
    group
        .instances
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let highlight = if group.highlighted == Some(i) { HIGHLIGHT_COLOR } else { Instance::NO_HIGHLIGHT };
            instance(view, m.position, m.orientation, m.scale, [0.0; 3], color(i), highlight)
        })
        .collect()
}

/// The whole airframes of `group` (`aircraft-{groupId}-{detail}`): each aircraft in its colour
/// (selection blue or white).
pub fn airframe_instances(view: &ViewFrame, group: &Group) -> Vec<Instance> {
    airframes(view, group, |i| group.instances[i].color)
}

/// One livery part of `group`'s airframes (`aircraft-{groupId}-{detail}-{part}`): each aircraft in
/// that part's colour.
pub fn part_instances(view: &ViewFrame, group: &Group, part: Part) -> Vec<Instance> {
    airframes(view, group, |i| group.instances[i].part_color(part))
}

/// `base` (a group's [`airframe_instances`]) recoloured for `part`: the same matrices.
fn recolored(base: &[Instance], group: &Group, part: Part) -> Vec<Instance> {
    base.iter().zip(&group.instances).map(|(b, m)| Instance { color: m.part_color(part), ..*b }).collect()
}

/// The gear of `group` (`gear-{groupId}`), in [`Group::gear`]'s order: squashed toward its attach
/// plane by the gear's state and moved along the airframe's up axis, in [`GEAR_COLOR`], never
/// highlighted.
///
/// [`GEAR_COLOR`]: rustradar_scene::aircraft::GEAR_COLOR
pub fn gear_instances(view: &ViewFrame, group: &Group) -> Vec<Instance> {
    group.gear.iter().map(|g| instance(view, g.position, g.orientation, g.scale, g.translation, g.color, Instance::NO_HIGHLIGHT)).collect()
}

/// The mesh a group draws: a published model at a level of detail, or a built-in model.
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

    /// The airframe on the GPU, if `assets` have it.
    fn shape<'a>(&self, assets: &'a AircraftAssets) -> Option<&'a AircraftShape> {
        match self {
            Mesh::Published { name, lod } => assets.model(name)?.lods.get(*lod),
            Mesh::BuiltIn(kind) => Some(assets.built_in(*kind)),
        }
    }

    /// Its gear on the GPU, if the model has gear.
    fn gear<'a>(&self, assets: &'a AircraftAssets) -> Option<&'a Shape> {
        match self {
            Mesh::Published { name, .. } => Some(&assets.model(name)?.gear.as_ref()?.shape),
            Mesh::BuiltIn(_) => None,
        }
    }

    /// The shape and the elements `elements` draws of it.
    fn elements<'a>(&self, assets: &'a AircraftAssets, elements: Elements) -> Option<(&'a Shape, Range<u32>)> {
        match elements {
            Elements::Gear => self.gear(assets).map(|shape| (shape, 0..shape.elements())),
            Elements::Combined => self.shape(assets).map(|s| (&s.shape, s.combined())),
            Elements::Part(part) => self.shape(assets).and_then(|s| Some((&s.shape, s.part(part)?))),
        }
    }
}

/// One draw: a group's mesh, what of it, and its instances on the GPU, kept between frames.
struct Draw {
    mesh: Mesh,
    elements: Elements,
    instances: Instances,
    written: Vec<Instance>,
}

/// Row 26: every mesh group's gear and airframes.
pub struct Meshes {
    pipeline: MeshPipeline,
    /// `meshMaterial(depth)`, shared by the airframes and the gear.
    style: Style,
    /// The material [`Self::style`] holds.
    material: Material,
    /// This frame's draws (the first [`Self::active`]), reused across frames.
    draws: Vec<Draw>,
    active: usize,
}

impl Meshes {
    pub fn new(init: &Init<'_>) -> Self {
        let pipeline = MeshPipeline::instanced(init, "aircraft meshes", STATE);
        let material = material(0.0);
        let style = pipeline.style(&init.gpu.device, &StyleUniform::new(&material, 1.0, [255; 4]));
        Meshes { pipeline, style, material, draws: Vec::new(), active: 0 }
    }

    /// Once a frame: this frame's gear and airframes of `scene`'s mesh groups, with the material of
    /// the scene's model depth and the scene's lights.
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, assets: &AircraftAssets, scene: &AircraftScene) {
        self.active = 0;
        if scene.groups.is_empty() {
            return;
        }
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        let frame = &scene.frame;
        let view = cx.view;
        for group in &scene.groups {
            let mesh = Mesh::of(group);
            let Some(shape) = mesh.shape(assets) else { continue };
            let gear = !group.gear.is_empty() && mesh.gear(assets).is_some();
            let base = airframe_instances(view, group);
            for elements in draws(gear, frame.livery_amount, shape.parts.iter().map(|(part, _)| *part)) {
                let instances = match elements {
                    Elements::Gear => gear_instances(view, group),
                    Elements::Combined => base.clone(),
                    Elements::Part(part) => recolored(&base, group, part),
                };
                if self.draws.len() == self.active {
                    self.draws.push(Draw { mesh: mesh.clone(), elements, instances: Instances::new(device, "aircraft mesh instances"), written: Vec::new() });
                }
                let draw = &mut self.draws[self.active];
                draw.mesh = mesh.clone();
                draw.elements = elements;
                if instances != draw.written {
                    draw.instances.write(device, queue, &instances);
                    draw.written = instances;
                }
                self.active += 1;
            }
        }
        if self.active == 0 {
            return;
        }
        let material = material(frame.depth);
        if material != self.material {
            self.style.set(queue, &StyleUniform::new(&material, 1.0, [255; 4]));
            self.material = material;
        }
        cx.set_lights(&self.pipeline);
    }

    /// Every group's gear and airframes, in group order (map pass only).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>, assets: &AircraftAssets) {
        if cx.pass != Pass::Map {
            return;
        }
        for draw in &self.draws[..self.active] {
            let Some((shape, elements)) = draw.mesh.elements(assets, draw.elements) else { continue };
            self.pipeline.draw_range(pass, &self.style, shape, elements, &draw.instances);
        }
    }

    /// This frame's draws in order: what each draws and how many instances.
    pub fn drawn(&self) -> Vec<(Elements, u32)> {
        self.draws[..self.active].iter().map(|d| (d.elements, d.instances.count())).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use glam::DMat3;
    use rustradar_core::geo::Basemap;
    use rustradar_core::geo::sphere::bearing;
    use rustradar_core::liveries::{DEFAULT_LIVERY, Livery, PARTS};
    use rustradar_core::model::{LiveFlight, f32_as_js};
    use rustradar_core::motion::aircraft::{Phase, Pose};
    use rustradar_scene::aircraft::{AircraftBuilder, GEAR_COLOR, Inputs, opaque, orientation_of, up_axis};
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

    /// 2026-06-21 12:00 UTC: daylight over Stockholm.
    const NOON_MS: f64 = 1_782_043_200_000.0;
    /// The canvas, CSS px, drawn at twice its size.
    const SIZE: [f64; 2] = [480.0, 320.0];
    const SCALE: f64 = 2.0;
    /// V4: Arlanda on the flat map, pitched and turned. Depth 1, liveries in full, gear.
    const V4: ViewState = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 13.5, bearing: 30.0, pitch: 45.0, position: None };
    /// The same on the globe at zoom 10.5, tilted past 5° so the published models are meshes, not
    /// icons. Depth 1, liveries in full, gear.
    const GLOBE: ViewState = ViewState { zoom: 10.5, bearing: 0.0, pitch: 30.0, ..V4 };
    /// Zoomed out to 6, tilted: depth 0 (the flat colours, silhouettes 4 % thick), no liveries, no
    /// gear.
    const FAR: ViewState = ViewState { zoom: 6.0, bearing: 0.0, pitch: 30.0, ..V4 };

    /// One aircraft: id, ICAO type, icon class, heading, pitch, bank, gear, the canvas point (CSS
    /// px) its ground point is under, and its height, metres.
    type Row = (u32, &'static str, &'static str, f64, f64, f64, f64, [f64; 2], f64);

    /// An A320 on the ground with its gear down, a 747 climbing and banked with its gear half
    /// retracted, a balloon (a built-in model), a PA-28 on the ground, and a Concorde (the built-in
    /// narrow-body, one and a half times its size) banked.
    const ROWS: [Row; 5] = [
        (1, "A20N", "A320", 30.0, 0.0, 0.0, 1.0, [100.0, 230.0], 0.0),
        (2, "B744", "B747", 200.0, 5.0, 20.0, 0.5, [240.0, 190.0], 300.0),
        (3, "BALL", "BALL", 0.0, 0.0, 0.0, 0.0, [360.0, 240.0], 100.0),
        (4, "C172", "C206", 100.0, 0.0, 0.0, 1.0, [400.0, 130.0], 0.0),
        (5, "CONC", "CONC", 300.0, 2.0, -10.0, 0.0, [250.0, 285.0], 150.0),
    ];

    fn flights(view: &ViewState, rows: &[Row]) -> Vec<LiveFlight> {
        let viewport = viewport_for(view, SIZE);
        rows.iter()
            .map(|&(id, typecode, icon, heading, _, _, _, xy, height)| {
                let [lon, lat] = viewport.unproject(xy, 0.0);
                LiveFlight {
                    id,
                    lat: lat as f32,
                    lon: lon as f32,
                    alt: (height / 0.3048) as i32,
                    track: heading as i32,
                    on_ground: height == 0.0,
                    callsign: format!("TST{id}"),
                    typecode: typecode.into(),
                    icon: icon.into(),
                    ..Default::default()
                }
            })
            .collect()
    }

    /// The reported position as the pose, posed as its row says.
    fn pose(rows: &[Row]) -> impl Fn(&LiveFlight) -> Pose + '_ {
        move |f| {
            let &(_, _, _, heading, pitch, bank, gear, _, height) = rows.iter().find(|r| r.0 == f.id).unwrap();
            Pose {
                lon: f32_as_js(f.lon),
                lat: f32_as_js(f.lat),
                alt_ft: height / 0.3048,
                heading,
                track: heading,
                pitch,
                bank,
                gear,
                phase: if height == 0.0 { Phase::Taxi } else { Phase::Climb },
            }
        }
    }

    /// The rows as the scene builder makes them in `view`, every one a mesh.
    fn aircraft(view: &ViewState, rows: &[Row], selected: Option<u32>, hovered: Option<u32>, livery: impl FnMut(&LiveFlight) -> Livery) -> AircraftScene {
        let inputs = Inputs { view: *view, size: SIZE, exaggeration: 1.0, show_aircraft: true, labelling: false, selected, follow: false, hovered };
        let scene = AircraftBuilder::new().build(&inputs, published(), &flights(view, rows), pose(rows), livery);
        assert_eq!(scene.mesh_count(), rows.len(), "all drawn as meshes");
        scene
    }

    fn plain(view: &ViewState, rows: &[Row]) -> AircraftScene {
        aircraft(view, rows, None, None, |_| DEFAULT_LIVERY)
    }

    /// deck's instance matrix written out from doc 03 §5.1 (`R = Rz(yaw) · Ry(o[0]) · Rx(o[2])`,
    /// then the scale), independently of the pipeline's [`orientation_matrix`].
    fn deck_matrix([p, y, r]: [f64; 3], [sx, sy, sz]: [f64; 3]) -> DMat3 {
        let (sp, cp) = p.to_radians().sin_cos();
        let (sw, cw) = y.to_radians().sin_cos();
        let (sr, cr) = r.to_radians().sin_cos();
        let col0 = DVec3::new(cw * cp, sw * cp, -sp);
        let col1 = DVec3::new(-sw * cr + cw * sp * sr, cw * cr + sw * sp * sr, cp * sr);
        let col2 = DVec3::new(sw * sr + cw * sp * cr, -cw * sr + sw * sp * cr, cp * cr);
        DMat3::from_cols(col0 * sx, col1 * sy, col2 * sz)
    }

    fn columns(instance: &Instance) -> DMat3 {
        let c = instance.columns.map(|c| DVec3::new(f64::from(c[0]), f64::from(c[1]), f64::from(c[2])));
        DMat3::from_cols(c[0], c[1], c[2])
    }

    fn close(a: DMat3, b: DMat3, tolerance: f64, what: &str) {
        let scale = b.to_cols_array().iter().fold(0.0f64, |m, v| m.max(v.abs()));
        for (k, (x, y)) in a.to_cols_array().iter().zip(b.to_cols_array()).enumerate() {
            assert!((x - y).abs() <= tolerance * scale, "{what}: element {k}: {x} against {y}");
        }
    }

    #[test]
    fn the_material_follows_the_depth() {
        let flat = material(0.0);
        assert_eq!((flat.ambient, flat.diffuse, flat.specular_color, flat.unlit), (1.0, 0.0, [0.0; 3], false));
        assert_eq!(material(1.0), Material::AIRCRAFT);
        let half = material(0.5);
        assert!((half.ambient - 0.86).abs() < 1e-12 && (half.diffuse - 0.21).abs() < 1e-12 && half.shininess == 48.0);
        // Math.round(127.5) is 128
        assert_eq!(half.specular_color, [128.0, 118.0, 100.0]);
    }

    /// The layers each group pushes (G:2894-2954): gear first when down and the model has some,
    /// then the airframe whole without liveries or one per part in the mesh's order.
    #[test]
    fn each_group_draws_todays_layers() {
        use Elements::{Combined, Gear, Part as P};
        assert_eq!(draws(false, 0.0, [Part::Top, Part::Belly]), vec![Combined]);
        assert_eq!(draws(true, 0.0, [Part::Top]), vec![Gear, Combined]);
        assert_eq!(draws(true, 0.3, [Part::Top, Part::Belly, Part::Wings, Part::Tail]), vec![Gear, P(Part::Top), P(Part::Belly), P(Part::Wings), P(Part::Tail)]);
        assert_eq!(draws(false, 1.0, []), Vec::<Elements>::new());

        let models = published().expect("the published models");
        let parts = |group: &Group| -> Vec<Part> {
            match &group.model {
                GroupModel::Published { name, lod, .. } => models.get(name).unwrap().lods[*lod].parts.iter().map(|(p, _)| *p).collect(),
                GroupModel::BuiltIn(kind) => aircraft_models::model(*kind).parts.iter().map(|(p, _)| *p).collect(),
            }
        };
        // zoomed out: no gear, each aircraft in one colour
        let far = plain(&FAR, &ROWS);
        assert_eq!((far.frame.depth, far.frame.livery_amount), (0.0, 0.0));
        for group in &far.groups {
            assert!(group.gear.is_empty());
            assert_eq!(draws(!group.gear.is_empty(), far.frame.livery_amount, parts(group)), vec![Combined], "{}", group.id());
        }
        // close in: the gear of the aircraft with it down, every part
        let close = plain(&GLOBE, &ROWS);
        assert_eq!((close.frame.depth, close.frame.livery_amount), (1.0, 1.0));
        let ids: Vec<String> = close.groups.iter().map(Group::id).collect();
        assert_eq!(ids, ["t-a320", "t-b744", "k-balloon", "t-pa28", "k-narrow"]);
        let gear: Vec<usize> = close.groups.iter().map(|g| g.gear.len()).collect();
        assert_eq!(gear, [1, 1, 0, 1, 0], "gear down on the A320, the 747 and the PA-28");
        let a320 = draws(true, 1.0, parts(&close.groups[0]));
        assert_eq!(a320, vec![Gear, P(Part::Top), P(Part::Belly), P(Part::Wings), P(Part::Tail)]);
        let balloon = draws(false, 1.0, parts(&close.groups[2]));
        assert_eq!(balloon, vec![P(Part::Tail), P(Part::Engines)], "the envelope and the basket");
        let concorde = draws(false, 1.0, parts(&close.groups[4]));
        assert_eq!(concorde, PARTS.map(P).to_vec());
    }

    /// Each instance is deck's for its aircraft (doc 03 §5.1, §5.3): the matrix of its orientation
    /// and scale in the east, north, up frame at its raised position, its colour (selection blue
    /// or white zoomed out, each part's livery colour close in), the hovered one highlighted on
    /// every part and never its gear; the gear squashed about its attach plane and moved along the
    /// airframe's up axis, in the gear's grey.
    #[test]
    fn instances_are_todays_transforms_and_colours() {
        let livery = Livery { top: [200, 30, 40], belly: [30, 200, 40], wings: [40, 30, 200], tail: [220, 200, 20], engines: [200, 20, 220] };
        for (view, name) in [(FAR, "far"), (GLOBE, "globe"), (V4, "flat")] {
            let scene = aircraft(&view, &ROWS, Some(1), Some(2), |_| livery);
            let vf = ViewFrame::new(&view, SIZE, SCALE, 0.0);
            for group in &scene.groups {
                let all = airframe_instances(&vf, group);
                assert_eq!(all.len(), group.instances.len());
                for (i, (instance, m)) in all.iter().zip(&group.instances).enumerate() {
                    let what = format!("{name}: aircraft {}", m.id);
                    let [lon, lat, h] = m.position;
                    close(columns(instance), local_frame(&vf, [lon, lat]) * deck_matrix(orientation_of(&m.pose), m.scale), 1e-6, &what);
                    assert_eq!(instance.offset, vf.common_offset([lon, lat], h).to_array(), "{what}");
                    let s = m.scale[0];
                    assert_eq!(m.scale, [s, s, s * scene.frame.flatten()], "{what}");
                    let highlight = if m.id == 2 { HIGHLIGHT_COLOR } else { Instance::NO_HIGHLIGHT };
                    assert_eq!(group.highlighted == Some(i), m.id == 2, "{what}");
                    assert_eq!(instance.highlight, highlight, "{what}");
                    let whole = if m.id == 1 { [55, 142, 240, 255] } else { [255; 4] };
                    assert_eq!(instance.color, whole, "{what}");
                    for part in PARTS {
                        let colored = part_instances(&vf, group, part);
                        let expected = if scene.frame.livery_amount == 0.0 { whole } else { opaque(livery[part]) };
                        assert_eq!(colored[i], Instance { color: expected, ..*instance }, "{what}: {part:?}");
                    }
                }
                let gear = gear_instances(&vf, group);
                assert_eq!(gear.len(), group.gear.len());
                for (instance, g) in gear.iter().zip(&group.gear) {
                    let m = &group.instances[g.instance];
                    let what = format!("{name}: aircraft {}'s gear", m.id);
                    let GroupModel::Published { gear_attach_z: Some(attach_z), .. } = group.model else { panic!("{what}: a published model with gear") };
                    let flatten = scene.frame.flatten();
                    let s = m.scale[0];
                    assert_eq!(g.scale, [s, s, s * flatten * m.pose.gear], "{what}");
                    let lift = attach_z * (1.0 - m.pose.gear) * s * flatten;
                    let up = DVec3::from_array(up_axis(m.orientation));
                    let [lon, lat, h] = m.position;
                    let frame = local_frame(&vf, [lon, lat]);
                    close(columns(instance), frame * deck_matrix(m.orientation, g.scale), 1e-6, &what);
                    let offset = vf.common_offset([lon, lat], h).as_dvec3() + frame * (up * lift);
                    let got = DVec3::from_array(instance.offset.map(f64::from));
                    assert!((got - offset).length() < 1e-5 * offset.length().max(1e-3), "{what}: {got} against {offset}");
                    assert_eq!((instance.color, instance.highlight), (opaque(GEAR_COLOR), Instance::NO_HIGHLIGHT), "{what}");
                }
            }
        }
    }

    /// In both projections the model's nose (+y) points along the heading, its right wing (+x) a
    /// quarter turn clockwise of it, its top (+z) up: measured by placing the model's axes with the
    /// instance's matrix and reading the points they reach back as longitude, latitude and height.
    #[test]
    fn the_nose_points_along_the_heading_in_both_projections() {
        let views = [
            ("globe 11.9", ViewState { zoom: 11.9, ..V4 }, [0.0, 0.0]),
            ("flat 12.1", ViewState { zoom: 12.1, ..V4 }, [0.0, 0.0]),
            ("flat V4", V4, [0.01, -0.004]),
            // the globe far from the view's centre: a frame of its own at each place
            ("globe at zoom 4", ViewState { longitude: 10.0, latitude: 45.0, zoom: 4.0, bearing: 20.0, pitch: 40.0, position: None }, [8.0, 5.0]),
        ];
        for (name, view, [dlon, dlat]) in views {
            let vf = ViewFrame::new(&view, SIZE, SCALE, 0.0);
            let at = [view.longitude + dlon, view.latitude + dlat, 120.0];
            for heading in [0.0, 8.0, 79.0, 188.0, 259.0, 317.0] {
                let pose = Pose { lon: at[0], lat: at[1], alt_ft: 0.0, heading, track: heading, pitch: 0.0, bank: 0.0, gear: 0.0, phase: Phase::Taxi };
                let instance = instance(&vf, at, orientation_of(&pose), [1.0; 3], [0.0; 3], [255; 4], Instance::NO_HIGHLIGHT);
                let m = columns(&instance);
                let origin = vf.viewport.project_position(at);
                let reach = |axis: DVec3| vf.viewport.unproject_position(origin + m * axis);
                let course = |axis: DVec3| {
                    let [lon, lat, _] = reach(axis);
                    bearing([at[0], at[1]], [lon, lat])
                };
                let turn = |a: f64, b: f64| ((a - b + 540.0) % 360.0 - 180.0).abs();
                let nose = course(DVec3::new(0.0, 10.0, 0.0));
                let wing = course(DVec3::new(10.0, 0.0, 0.0));
                assert!(turn(nose, heading) < 0.01, "{name}: heading {heading}: the nose points to {nose}");
                assert!(turn(wing, heading + 90.0) < 0.01, "{name}: heading {heading}: the right wing points to {wing}");
                let [lon, lat, h] = reach(DVec3::new(0.0, 0.0, 10.0));
                assert!((lon - at[0]).abs() < 1e-6 && (lat - at[1]).abs() < 1e-6 && (h - at[2] - 10.0).abs() < 0.01, "{name}: heading {heading}: up reaches {lon}, {lat}, {h}");
            }
        }
    }

    /// The deviation of the module's documentation is invisible: deck's globe frame (east and north
    /// turned half a turn about up) with the yaw offset of 180° places every vertex, and the gear's
    /// lift, where the true east, north, up frame does without it.
    #[test]
    fn decks_turned_globe_frame_is_the_true_frame() {
        let turned = DMat3::from_diagonal(DVec3::new(-1.0, -1.0, 1.0));
        for (heading, pitch, bank) in [(30.0, 0.0, 0.0), (200.0, 5.0, 20.0), (317.0, -3.0, -25.0), (91.0, 12.0, 8.0)] {
            let pose = Pose { lon: 0.0, lat: 0.0, alt_ft: 0.0, heading, track: heading, pitch, bank, gear: 0.4, phase: Phase::Climb };
            let ours = orientation_of(&pose);
            let deck = [bank, 180.0 - heading, pitch];
            let scale = [1.7, 1.7, 1.7 * 0.6];
            close(turned * orientation_matrix(deck, scale), orientation_matrix(ours, scale), 1e-12, &format!("heading {heading}"));
            let lift = DVec3::from_array(up_axis(ours));
            let deck_lift = turned * DVec3::from_array(up_axis(deck));
            assert!((lift - deck_lift).length() < 1e-12, "heading {heading}: {lift} against {deck_lift}");
        }
    }

    /// A test layer: [`Meshes`] with assets of its own, drawing `scene`'s meshes and nothing else.
    struct Only {
        assets: AircraftAssets,
        meshes: Meshes,
        scene: AircraftScene,
        drawn: Rc<RefCell<Vec<(Elements, u32)>>>,
    }

    impl Layer for Only {
        fn name(&self) -> &'static str {
            "a5 meshes only"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.assets.poll(cx.gpu);
            self.meshes.prepare(cx, &self.assets, &self.scene);
            *self.drawn.borrow_mut() = self.meshes.drawn();
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            self.meshes.draw(pass, cx, &self.assets);
        }
    }

    fn input(view: &ViewState) -> SceneInput {
        let mut input = SceneInput::new(*view, SIZE, SCALE, Basemap::Dark);
        input.feed_now_ms = NOON_MS;
        input.local_now_ms = NOON_MS;
        input
    }

    /// `scene`'s meshes alone over the plain map of `view`, and the draws made.
    fn shoot_only(gpu: &Gpu, view: &ViewState, scene: AircraftScene) -> (Image, Vec<(Elements, u32)>) {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        let drawn = Rc::new(RefCell::new(Vec::new()));
        let init = renderer.init(gpu);
        let layer = Only { assets: AircraftAssets::new(&init), meshes: Meshes::new(&init), scene, drawn: drawn.clone() };
        renderer.add_layer(Box::new(layer));
        let image = render_with(gpu, &mut renderer, &input(view), SCALE).expect("rendered");
        let drawn = drawn.borrow().clone();
        (image, drawn)
    }

    fn save(image: &Image, name: &str) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("shot folder");
        image.save_png(&dir.join(name)).expect("png");
    }

    type Triangle = [[f32; 3]; 3];

    fn corner(p: &[f32], i: usize) -> [f32; 3] {
        [p[3 * i], p[3 * i + 1], p[3 * i + 2]]
    }

    /// The triangles of `group`'s airframe (all its parts), model metres.
    fn airframe_triangles(group: &Group) -> Vec<Triangle> {
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

    /// The triangles of `group`'s gear, if its model has some.
    fn gear_triangles(group: &Group) -> Vec<Triangle> {
        let GroupModel::Published { name, .. } = &group.model else { return Vec::new() };
        let Some(gear) = &published().unwrap().get(name).unwrap().gear else { return Vec::new() };
        gear.mesh.indices.chunks_exact(3).map(|t| [0, 1, 2].map(|k| corner(&gear.mesh.positions, usize::from(t[k])))).collect()
    }

    /// Vulkan's (and D3D's) standard positions of the four samples in a pixel.
    const SAMPLES: [[f64; 2]; 4] = [[0.375, 0.125], [0.875, 0.375], [0.125, 0.625], [0.625, 0.875]];

    /// Marks in `samples` (four bits a device pixel) the samples the CPU puts inside `triangles`
    /// placed at `position` by `model` (model metres to metres east, north and up) and
    /// `translation`, in `view`: in f64, by deck's formulas.
    fn raster(view: &ViewFrame, triangles: &[Triangle], position: [f64; 3], model: DMat3, translation: DVec3, samples: &mut [u8], [w, h]: [u32; 2]) {
        let frame = local_frame(view, [position[0], position[1]]);
        let origin = view.viewport.project_position(position);
        let screen = |p: [f32; 3]| {
            let common = origin + frame * (model * DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])) + translation);
            let [x, y, _] = view.viewport.world_to_pixels(common);
            [x * SCALE, y * SCALE]
        };
        let edge = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
        for tri in triangles {
            let [a, b, c] = tri.map(screen);
            let area = edge(a, b, c);
            if area == 0.0 || !area.is_finite() {
                continue;
            }
            let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as u32;
            let x1 = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as u32).min(w);
            let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as u32;
            let y1 = (a[1].max(b[1]).max(c[1]).ceil().max(0.0) as u32).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    for (k, [sx, sy]) in SAMPLES.iter().enumerate() {
                        let p = [f64::from(x) + sx, f64::from(y) + sy];
                        let [u, v, s] = [edge(b, c, p), edge(c, a, p), edge(a, b, p)].map(|e| e / area);
                        if u >= 0.0 && v >= 0.0 && s >= 0.0 {
                            samples[(y * w + x) as usize] |= 1 << k;
                        }
                    }
                }
            }
        }
    }

    /// The CPU's samples of `group`'s `i`-th aircraft (its airframe and, if `with_gear`, its gear),
    /// OR-ed into `samples`.
    fn aircraft_samples(view: &ViewFrame, group: &Group, i: usize, with_gear: bool, samples: &mut [u8], size: [u32; 2]) {
        let m = &group.instances[i];
        raster(view, &airframe_triangles(group), m.position, deck_matrix(m.orientation, m.scale), DVec3::ZERO, samples, size);
        if with_gear {
            for g in group.gear.iter().filter(|g| g.instance == i) {
                raster(view, &gear_triangles(group), g.position, deck_matrix(g.orientation, g.scale), DVec3::from_array(g.translation), samples, size);
            }
        }
    }

    /// How many samples of each pixel the CPU covers with `group`'s `i`-th aircraft.
    fn aircraft_coverage(view: &ViewFrame, group: &Group, i: usize, with_gear: bool, size: [u32; 2]) -> Vec<u32> {
        let mut samples = vec![0u8; (size[0] * size[1]) as usize];
        aircraft_samples(view, group, i, with_gear, &mut samples, size);
        samples.iter().map(|s| s.count_ones()).collect()
    }

    /// `scene` drawn in flat white: depth 0 for the material (the shape keeps its depth), every
    /// colour white, no highlight. Each pixel is then the map toward white by the samples covered.
    fn white(scene: &AircraftScene) -> AircraftScene {
        let mut white = scene.clone();
        white.frame.depth = 0.0;
        for group in &mut white.groups {
            group.highlighted = None;
            for m in &mut group.instances {
                m.color = [255; 4];
                m.parts = m.parts.map(|_| [[255; 4]; 5]);
            }
            for g in &mut group.gear {
                g.color = [255; 4];
            }
        }
        white
    }

    /// How many of each pixel's four samples a flat white shot covers, read from its colour over
    /// the background's (green channel).
    fn gpu_coverage(image: &Image, bg: &Image) -> Vec<u32> {
        image
            .rgba
            .chunks_exact(4)
            .zip(bg.rgba.chunks_exact(4))
            .map(|(p, b)| {
                let range = 255 - i32::from(b[1]);
                ((i32::from(p[1]) - i32::from(b[1])) * 4 + range / 2).div_euclid(range).clamp(0, 4) as u32
            })
            .collect()
    }

    /// The pixels that differ from `bg`.
    fn changed(image: &Image, bg: &Image) -> Vec<bool> {
        image.rgba.chunks_exact(4).zip(bg.rgba.chunks_exact(4)).map(|(p, b)| p != b).collect()
    }

    /// `mask` grown by one pixel.
    fn dilate(mask: &[bool], [w, h]: [u32; 2]) -> Vec<bool> {
        let mut out = vec![false; mask.len()];
        for y in 0..h {
            for x in 0..w {
                if mask[(y * w + x) as usize] {
                    for yy in y.saturating_sub(1)..(y + 2).min(h) {
                        for xx in x.saturating_sub(1)..(x + 2).min(w) {
                            out[(yy * w + xx) as usize] = true;
                        }
                    }
                }
            }
        }
        out
    }

    /// The pixels fully covered, as are the four next to them.
    fn inside(coverage: &[u32], [w, h]: [u32; 2]) -> Vec<bool> {
        let full = |x: u32, y: u32| coverage[(y * w + x) as usize] == 4;
        let mut out = vec![false; coverage.len()];
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                out[(y * w + x) as usize] = full(x, y) && full(x - 1, y) && full(x + 1, y) && full(x, y - 1) && full(x, y + 1);
            }
        }
        out
    }

    /// The pixels with at least half their samples covered.
    fn half(coverage: &[u32]) -> Vec<bool> {
        coverage.iter().map(|&c| c >= 2).collect()
    }

    fn iou(a: &[bool], b: &[bool]) -> f64 {
        let both = a.iter().zip(b).filter(|(a, b)| **a && **b).count();
        let either = a.iter().zip(b).filter(|(a, b)| **a || **b).count();
        both as f64 / either.max(1) as f64
    }

    fn count(mask: &[bool]) -> usize {
        mask.iter().filter(|&&m| m).count()
    }

    /// The principal axes of the samples covered (`coverage` per pixel): the angle of the longer
    /// axis on screen, degrees clockwise from up in [0, 180), the shorter axis's, and the ratio of
    /// their second moments.
    fn principal_axis(coverage: &[u32], [w, _]: [u32; 2]) -> (f64, f64, f64) {
        let points: Vec<([f64; 2], f64)> =
            (0..coverage.len()).filter(|&i| coverage[i] > 0).map(|i| ([(i as u32 % w) as f64 + 0.5, (i as u32 / w) as f64 + 0.5], f64::from(coverage[i]))).collect();
        let n: f64 = points.iter().map(|(_, c)| c).sum();
        let [mx, my] = [0, 1].map(|k| points.iter().map(|(p, c)| p[k] * c).sum::<f64>() / n);
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for ([x, y], c) in &points {
            let (dx, dy) = (x - mx, y - my);
            sxx += c * dx * dx;
            syy += c * dy * dy;
            sxy += c * dx * dy;
        }
        // the eigenvector of the larger eigenvalue, as an angle from screen x (y down)
        let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let mean = (sxx + syy) / 2.0;
        let spread = (((sxx - syy) / 2.0).powi(2) + sxy * sxy).sqrt();
        // clockwise from up: screen x is 90°
        let major = (theta.to_degrees() + 90.0).rem_euclid(180.0);
        (major, (major + 90.0) % 180.0, (mean + spread) / (mean - spread))
    }

    /// The difference of two axes' angles, degrees, modulo half a turn.
    fn axis_turn(a: f64, b: f64) -> f64 {
        let d = (a - b).rem_euclid(180.0);
        d.min(180.0 - d)
    }

    /// The meshes of `rows` in `view` on the GPU against the CPU's triangles placed by deck's
    /// formulas. Drawn in flat white, each pixel's samples covered (read from its colour) are the
    /// CPU's at the standard sample positions, give or take one where an edge grazes a sample (two
    /// for a handful of pixels), for each aircraft and all together. Drawn lit, nothing changes
    /// away from the aircraft, and the pixels inside them are lit as deck lights them: no darker
    /// than their colour's ambient part, brighter where a light faces them. Returns the lit shot
    /// and the scene.
    fn check_against_cpu(gpu: &Gpu, view: &ViewState, rows: &[Row], name: &str, expect: &[(Elements, u32)]) -> (Image, AircraftScene) {
        let scene = plain(view, rows);
        let (bg, drawn) = shoot_only(gpu, view, AircraftScene::default());
        assert_eq!(drawn, vec![]);
        let (flat, drawn) = shoot_only(gpu, view, white(&scene));
        assert_eq!(drawn, expect, "{name}: the draws");
        let (image, drawn) = shoot_only(gpu, view, scene.clone());
        assert_eq!(drawn, expect, "{name}: the draws");
        save(&image, name);
        let size = [image.width, image.height];
        let vf = ViewFrame::new(view, SIZE, SCALE, 0.0);
        let got = gpu_coverage(&flat, &bg);
        let mut all = vec![0u8; got.len()];
        for group in &scene.groups {
            for (i, m) in group.instances.iter().enumerate() {
                let cpu = aircraft_coverage(&vf, group, i, true, size);
                let near = dilate(&dilate(&cpu.iter().map(|&c| c > 0).collect::<Vec<_>>(), size), size);
                let own: Vec<u32> = got.iter().zip(&near).map(|(g, n)| if *n { *g } else { 0 }).collect();
                let (cpu_n, gpu_n) = (cpu.iter().sum::<u32>(), own.iter().sum::<u32>());
                let score = iou(&half(&own), &half(&cpu));
                eprintln!("{name}: aircraft {} ({}): {cpu_n} samples on the CPU, {gpu_n} on the GPU; half-covered pixels {score:.3} alike", m.id, group.id());
                assert!(cpu_n > 400, "{name}: aircraft {} is {cpu_n} samples", m.id);
                assert!((f64::from(gpu_n) / f64::from(cpu_n) - 1.0).abs() < 0.03 && score > 0.95, "{name}: aircraft {}: {gpu_n} for {cpu_n}, {score}", m.id);
                aircraft_samples(&vf, group, i, true, &mut all, size);
            }
        }
        let cpu: Vec<u32> = all.iter().map(|s| s.count_ones()).collect();
        let (mut edges, mut off, mut two_off) = (0, 0, 0);
        for (i, (&g, &c)) in got.iter().zip(&cpu).enumerate() {
            let (x, y) = (i as u32 % size[0], i as u32 / size[0]);
            assert!(g.abs_diff(c) <= 2, "{name}: pixel ({x}, {y}) has {g} of 4 samples, its triangles {c}");
            edges += usize::from((1..4).contains(&g) || (1..4).contains(&c));
            off += usize::from(g != c);
            two_off += usize::from(g.abs_diff(c) == 2);
        }
        eprintln!("{name}: {off} of {edges} edge pixels a sample off the CPU's, {two_off} two");
        assert!((two_off as f64) < 0.01 * edges as f64 && (off as f64) < 0.25 * edges as f64, "{name}: {off} and {two_off} of {edges}");
        // lit: nothing away from the aircraft, and inside no darker than the ambient light alone
        let covered: Vec<bool> = cpu.iter().map(|&c| c > 0).collect();
        let stray = changed(&image, &bg).iter().zip(dilate(&covered, size)).filter(|(g, n)| **g && !*n).count();
        assert_eq!(stray, 0, "{name}: pixels changed away from every aircraft");
        let m = material(scene.frame.depth);
        let darkest = scene.groups.iter().flat_map(|g| g.instances.iter()).flat_map(|i| PARTS.map(|p| i.part_color(p))).map(|c| c[..3].iter().copied().min().unwrap()).min().unwrap();
        let well_inside = inside(&cpu, size);
        let mut levels = std::collections::BTreeSet::new();
        for i in (0..well_inside.len()).filter(|&i| well_inside[i]) {
            let p = image.pixel(i as u32 % size[0], i as u32 / size[0]);
            assert!(f64::from(p[0].min(p[1]).min(p[2])) >= m.ambient * f64::from(darkest) - 1.5, "{name}: {p:?} darker than ambient light on {darkest}");
            levels.insert(p[1]);
        }
        eprintln!("{name}: {} pixels well inside, {} levels of green", count(&well_inside), levels.len());
        if m.diffuse > 0.0 {
            assert!(levels.len() > 10, "{name}: lit faces differ: {} levels", levels.len());
        }
        (image, scene)
    }

    /// The draws today's app makes for `scene` (G:2894-2954), one instance each: every group's gear
    /// if down, then its airframe whole or part by part, the parts in its mesh's order.
    fn expected_draws(scene: &AircraftScene) -> Vec<(Elements, u32)> {
        let models = published().unwrap();
        scene
            .groups
            .iter()
            .flat_map(|g| {
                let parts: Vec<Part> = match &g.model {
                    GroupModel::Published { name, lod, .. } => models.get(name).unwrap().lods[*lod].parts.iter().map(|(p, _)| *p).collect(),
                    GroupModel::BuiltIn(kind) => aircraft_models::model(*kind).parts.iter().map(|(p, _)| *p).collect(),
                };
                draws(!g.gear.is_empty(), scene.frame.livery_amount, parts).into_iter().map(|e| (e, g.instances.len() as u32))
            })
            .collect()
    }

    /// On the flat map, pitched and turned (V4): each airframe in its livery parts and the gear
    /// of the three with it down, where the CPU places them.
    #[test]
    fn meshes_land_where_the_cpu_puts_them_on_the_flat_map() {
        let Some(gpu) = gpu() else { return };
        let expect = expected_draws(&plain(&V4, &ROWS));
        assert_eq!(expect.iter().filter(|(e, _)| *e == Elements::Gear).count(), 3);
        assert_eq!(expect.len(), 3 + 4 * 3 + 2 + 5, "{expect:?}");
        check_against_cpu(&gpu, &V4, &ROWS, "a5-meshes-flat.png", &expect);
    }

    /// The same on the globe at zoom 10.5: no half turn, every part where the CPU puts it.
    #[test]
    fn meshes_land_where_the_cpu_puts_them_on_the_globe() {
        let Some(gpu) = gpu() else { return };
        let expect = expected_draws(&plain(&GLOBE, &ROWS));
        assert_eq!(expect.iter().filter(|(e, _)| *e == Elements::Gear).count(), 3);
        check_against_cpu(&gpu, &GLOBE, &ROWS, "a5-meshes-globe.png", &expect);
    }

    /// Zoomed out (zoom 6, depth 0): one draw per group, the flat colours exactly: white, the
    /// selected aircraft selection blue, the hovered one white highlighted `[55,142,240,150]`.
    #[test]
    fn zoomed_out_each_aircraft_is_one_flat_colour() {
        let Some(gpu) = gpu() else { return };
        let rows = [ROWS[0], ROWS[1], ROWS[4]];
        let expect = vec![(Elements::Combined, 1); 3];
        let (_, scene) = check_against_cpu(&gpu, &FAR, &rows, "a5-meshes-far.png", &expect);
        let (image, _) = shoot_only(&gpu, &FAR, aircraft(&FAR, &rows, Some(1), Some(2), |_| DEFAULT_LIVERY));
        save(&image, "a5-meshes-far-selected.png");
        let size = [image.width, image.height];
        let vf = ViewFrame::new(&FAR, SIZE, SCALE, 0.0);
        // white mixed toward the highlight by its alpha, as the pipeline blends it
        let a = 150.0 / 255.0;
        let hovered = [55.0, 142.0, 240.0].map(|c: f64| 255.0 + (c - 255.0) * a);
        for group in &scene.groups {
            let m = &group.instances[0];
            let well_inside = inside(&aircraft_coverage(&vf, group, 0, false, size), size);
            let pixels: Vec<[u8; 4]> = (0..well_inside.len()).filter(|&i| well_inside[i]).map(|i| image.pixel(i as u32 % size[0], i as u32 / size[0])).collect();
            eprintln!("far: aircraft {}: {} pixels well inside, e.g. {:?}", m.id, pixels.len(), pixels.first());
            assert!(pixels.len() > 10, "aircraft {}: {} pixels", m.id, pixels.len());
            for p in pixels {
                match m.id {
                    1 => assert_eq!(p, [55, 142, 240, 255], "the selected aircraft"),
                    2 => assert!((0..3).all(|c| (f64::from(p[c]) - hovered[c]).abs() <= 1.0), "the hovered aircraft: {p:?} for {hovered:?}"),
                    _ => assert_eq!(p, [255; 4], "aircraft {}", m.id),
                }
            }
        }
    }

    /// Close in, each part in its livery colour: straight down on the flat map at zoom 16 with the
    /// material made flat (depth 0), the pixels well inside an aircraft are its parts' colours (but
    /// where two parts meet), each part that shows from above in many of them, and the selected
    /// aircraft is not blue.
    #[test]
    fn each_livery_part_is_drawn_in_its_colour() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { zoom: 16.0, pitch: 0.0, bearing: 0.0, ..V4 };
        let livery = Livery { top: [200, 30, 40], belly: [30, 200, 40], wings: [40, 30, 200], tail: [220, 200, 20], engines: [200, 20, 220] };
        let rows = [(1, "A20N", "A320", 30.0, 0.0, 0.0, 0.0, [130.0, 160.0], 0.0), (5, "CONC", "CONC", 300.0, 0.0, 0.0, 0.0, [340.0, 160.0], 0.0)];
        let mut scene = aircraft(&view, &rows, Some(1), None, |_| livery);
        assert_eq!(scene.frame.livery_amount, 1.0);
        scene.frame.depth = 0.0;
        let (image, drawn) = shoot_only(&gpu, &view, scene.clone());
        save(&image, "a5-meshes-liveries.png");
        assert_eq!(drawn.len(), 4 + 5, "every part of both, no gear (it is up): {drawn:?}");
        let size = [image.width, image.height];
        let vf = ViewFrame::new(&view, SIZE, SCALE, 0.0);
        for group in &scene.groups {
            let well_inside = inside(&aircraft_coverage(&vf, group, 0, false, size), size);
            let pixels: Vec<[u8; 4]> = (0..well_inside.len()).filter(|&i| well_inside[i]).map(|i| image.pixel(i as u32 % size[0], i as u32 / size[0])).collect();
            let of = |part: Part| pixels.iter().filter(|p| **p == opaque(livery[part])).count();
            let counts = PARTS.map(of);
            let exact: usize = counts.iter().sum();
            eprintln!("{}: {} pixels well inside, {exact} of them a part's colour: {counts:?} (top, belly, wings, tail, engines)", group.id(), pixels.len());
            // the edges between parts blend
            assert!(exact as f64 > 0.9 * pixels.len() as f64, "{}: {exact} of {}", group.id(), pixels.len());
            assert!(counts[0] > 200 && counts[2] > 200 && counts[3] > 20, "{}: {counts:?}", group.id());
            if group.id() == "k-narrow" {
                assert!(counts[4] > 20, "the built-in model's engines show: {counts:?}");
            }
            assert!(pixels.iter().all(|p| *p != [55, 142, 240, 255]), "{}: the selected aircraft in its livery", group.id());
        }
    }

    /// The gear: drawn with it down, squashed toward the airframe half retracted, not at all
    /// retracted; what it changes lies on the CPU's gear and nowhere else, below the airframe. Seen
    /// from low on the side, close in on the flat map.
    #[test]
    fn the_gear_hangs_under_the_airframe_and_retracts() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { zoom: 16.5, pitch: 80.0, bearing: 90.0, ..V4 };
        let row = |gear: f64| -> [Row; 1] { [(1, "A20N", "A320", 0.0, 0.0, 0.0, gear, [240.0, 200.0], 20.0)] };
        let (up, drawn) = shoot_only(&gpu, &view, plain(&view, &row(0.0)));
        assert_eq!(drawn.iter().filter(|(e, _)| *e == Elements::Gear).count(), 0, "no gear drawn retracted");
        let size = [up.width, up.height];
        let vf = ViewFrame::new(&view, SIZE, SCALE, 0.0);
        let mut changes = Vec::new();
        for gear in [1.0, 0.5] {
            let scene = plain(&view, &row(gear));
            let (down, drawn) = shoot_only(&gpu, &view, scene.clone());
            save(&down, &format!("a5-gear-{gear}.png"));
            assert_eq!(drawn[0], (Elements::Gear, 1), "gear {gear}: the gear first");
            let group = &scene.groups[0];
            let mut samples = vec![0u8; (size[0] * size[1]) as usize];
            let g = &group.gear[0];
            raster(&vf, &gear_triangles(group), g.position, deck_matrix(g.orientation, g.scale), DVec3::from_array(g.translation), &mut samples, size);
            let cpu: Vec<bool> = samples.iter().map(|&s| s != 0).collect();
            let diff = changed(&down, &up);
            let stray = diff.iter().zip(dilate(&cpu, size)).filter(|(d, n)| **d && !*n).count();
            let n = count(&diff);
            // the airframe's pixels: below them is the gear
            let airframe: Vec<bool> = aircraft_coverage(&vf, group, 0, false, size).iter().map(|&c| c > 0).collect();
            let mean_y = |mask: &[bool]| (0..mask.len()).filter(|&i| mask[i]).map(|i| f64::from(i as u32 / size[0])).sum::<f64>() / count(mask) as f64;
            eprintln!("gear {gear}: {n} pixels change, {} on the CPU's gear, {stray} away from it; the gear's middle row {:.1}, the airframe's {:.1}", count(&cpu), mean_y(&diff), mean_y(&airframe));
            assert!(n > 50, "gear {gear}: {n} pixels");
            assert_eq!(stray, 0, "gear {gear}");
            assert!(mean_y(&diff) > mean_y(&airframe), "gear {gear}: below the airframe");
            changes.push(n);
        }
        assert!(changes[1] < changes[0], "half retracted is shorter: {changes:?}");
    }

    /// Item A5's acceptance, second line: the same aircraft at zoom 11.9 (the globe) and 12.1 (the
    /// flat map), V4's tilt and turn, keeps its heading. For each heading the two shots of an A321
    /// at the view's centre (drawn flat white) cover the same pixels, with their principal axes
    /// within 2°, and each covers the CPU's pixels at that heading, not half a turn away.
    #[test]
    fn an_aircraft_keeps_its_heading_across_the_projection_swap() {
        let Some(gpu) = gpu() else { return };
        let mid = [SIZE[0] / 2.0, SIZE[1] / 2.0];
        for heading in [8.0, 79.0, 188.0, 259.0, 30.0] {
            let rows = [(1, "A321", "A320", heading, 0.0, 0.0, 0.0, mid, 0.0)];
            let turned = [(1, "A321", "A320", heading + 180.0, 0.0, 0.0, 0.0, mid, 0.0)];
            let mut shots = Vec::new();
            for zoom in [11.9, 12.1] {
                let view = ViewState { zoom, ..V4 };
                let scene = plain(&view, &rows);
                let (bg, _) = shoot_only(&gpu, &view, AircraftScene::default());
                let (image, _) = shoot_only(&gpu, &view, scene.clone());
                save(&image, &format!("a5-swap-{heading}-{zoom}.png"));
                let (flat, _) = shoot_only(&gpu, &view, white(&scene));
                let size = [image.width, image.height];
                let vf = ViewFrame::new(&view, SIZE, SCALE, 0.0);
                let got = gpu_coverage(&flat, &bg);
                let cpu = aircraft_coverage(&vf, &scene.groups[0], 0, true, size);
                let cpu_turned = aircraft_coverage(&vf, &plain(&view, &turned).groups[0], 0, true, size);
                let (right, wrong) = (iou(&half(&got), &half(&cpu)), iou(&half(&got), &half(&cpu_turned)));
                let (major, _, ratio) = principal_axis(&got, size);
                let (cpu_major, ..) = principal_axis(&cpu, size);
                eprintln!(
                    "heading {heading} at zoom {zoom}: {} samples, {right:.3} of the CPU's pixels, {wrong:.3} of them half turned; axis {major:.2}° (the CPU's {cpu_major:.2}°, moments {ratio:.2}:1)",
                    got.iter().sum::<u32>()
                );
                assert!(right > 0.95 && wrong < 0.8, "heading {heading} at zoom {zoom}: {right} against {wrong} half turned");
                assert!(axis_turn(major, cpu_major) < 0.5, "heading {heading} at zoom {zoom}: {major} against the CPU's {cpu_major}");
                shots.push((half(&got), major, ratio));
            }
            let (globe, flat) = (&shots[0], &shots[1]);
            let same = iou(&globe.0, &flat.0);
            let turn = axis_turn(globe.1, flat.1);
            eprintln!("heading {heading}: the two shots {same:.3} alike, axes {turn:.2}° apart");
            assert!(same > 0.9, "heading {heading}: {same}");
            assert!(globe.2 > 1.3 && turn < 2.0, "heading {heading}: {turn}° (moments {:.2}:1)", globe.2);
        }
    }

    /// Straight down, an airframe's principal axes are its nose and wing axes: one lies along the
    /// heading as the screen shows it (`heading − bearing`), within 2°.
    #[test]
    fn straight_down_the_principal_axis_is_the_heading() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { pitch: 0.0, ..V4 };
        let mid = [SIZE[0] / 2.0, SIZE[1] / 2.0];
        let (bg, _) = shoot_only(&gpu, &view, AircraftScene::default());
        for (typecode, icon) in [("A321", "A320"), ("B744", "B747"), ("DH8D", "Q300")] {
            for heading in [8.0, 79.0, 188.0, 259.0, 317.0] {
                let rows = [(1, typecode, icon, heading, 0.0, 0.0, 0.0, mid, 0.0)];
                let (flat, _) = shoot_only(&gpu, &view, white(&plain(&view, &rows)));
                let size = [flat.width, flat.height];
                let (major, minor, ratio) = principal_axis(&gpu_coverage(&flat, &bg), size);
                let screen = heading - view.bearing;
                let off = axis_turn(major, screen).min(axis_turn(minor, screen));
                eprintln!("{typecode} heading {heading}: axes {major:.2}° and {minor:.2}° (moments {ratio:.2}:1), the heading on screen {:.1}°: {off:.2}° off", screen.rem_euclid(180.0));
                assert!(ratio > 1.1 && off < 2.0, "{typecode} heading {heading}: {off}° (moments {ratio}:1)");
            }
        }
    }
}
