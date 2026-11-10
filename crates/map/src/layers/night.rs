//! Night shading (item B5; doc 01 §5.10; row 6 of doc 03 §3): where the sun is down, the map
//! darkened in four steps toward `[2,6,14]`, as today's `nightLayer` (G:1094-1109) draws
//! `daynight.ts`'s bands with deck.gl 9.4's SolidPolygonLayer.
//!
//! What today's app does, and so this layer:
//! - **When.** On the dark and satellite basemaps while the `daylight` setting is on (the scene
//!   builder says so in [`Input::shown`]); never on the realistic basemap, whose shaders light the
//!   night side themselves (doc 01 §5.3): this layer leaves itself out there whatever the input.
//! - **What.** `core::sun::night_polygons` (`nightPolygons`): for each of the sun altitudes 0°,
//!   −6°, −12° and −18° (sunset, civil, nautical and astronomical twilight), one quad per 2°
//!   meridian strip from the first to the last latitude where the sun is lower (1° steps, ±85°) on
//!   either meridian, tapered to a point where a band ends between two meridians. The four sets in
//!   turn, all in [`NIGHT_COLOR`] `[2,6,14,46]`, each blended over what is there (deck's default
//!   blending in gamma space, rounded to bytes after each set as on WebGL's canvas): one set takes
//!   the map 46/255 of the way to `[2,6,14]`, and where all four overlap, deep night, the dark sea
//!   `(30,30,30)` becomes `(15,17,21)` and the land `(50,50,50)` becomes `(23,26,30)`, as in
//!   `docs/port/reference/dark-v8-1440x900.png`. Unlit; `SURFACE_NO_CULL`: depth-tested against the
//!   earth sphere, which hides the far side, no depth written, nothing culled. In the layer order it
//!   comes after the map, the rail map and the radar and before the buildings, the vehicles and the
//!   labels, so it darkens the first and not the others.
//! - **Each minute.** The bands are made again when `floor(now / 60 000)` changes, `now` being the
//!   feed clock (`motion.now()`, G:2523; [`SceneInput::feed_now_ms`]), from the clock of the first
//!   scene of the new minute ([`Bands`]). The minute's quads outlive the layer (Globe.svelte's
//!   `night`): turned off and on within a minute, it draws the same quads again.
//! - **Tessellated as deck does.** SolidPolygonLayer's tesselator normalises each quad (closed by
//!   repeating its first point, unless it already ends where it starts, as a quad tapered on its
//!   west side does; turned clockwise as math.gl reckons it), then, on the globe (whose viewport
//!   has `resolution` 5), cuts it with math.gl's `cutPolygonByGrid` on the 5° grid (with edge
//!   types) and triangulates each part with earcut; on the flat map (no `resolution`) it
//!   triangulates the quad whole ([`night_mesh`]). earcut reads the positions through the
//!   viewport's `projectFlat` of when the layer was made: the identity on the globe, where the app
//!   starts, so degrees. (Made on the flat map it would be world Mercator: for these convex parts
//!   earcut cuts the same ears either way.) The vertices go to the [`Fill`] pipeline in one local
//!   Mercator frame for the whole world, as the land does; on the flat map the world's copies west
//!   and east of the nearest are drawn too, as the land's are (`layers::earth`), so the night covers
//!   the land the port draws past the antimeridian.
//!
//! Deviations:
//! - **Crossing zoom 12.** deck tessellates only when the data changes (each minute) or the layer
//!   is made, with the viewport of that moment, so after the projection swaps it keeps the other
//!   projection's triangles until the minute turns: the globe's cut parts on the flat map (the
//!   tapered band ends shaped a little differently), or the flat map's whole quads on the globe,
//!   whose chords, up to 170° long, pass inside the earth sphere, so that zooming out to the globe
//!   hides the night for up to a minute. Here each projection draws the minute's quads
//!   tessellated its own way, made on its first frame in that minute: what deck draws there once
//!   the minute has turned.
//! - The world's copies on the flat map (above): deck draws one world, and today's flat map shows
//!   the ink past the antimeridian, where the port shows the land's copy.
//! - earcut is `scene::geom::earcut` (`earcutr`, a port of the earcut deck uses): for parts of a
//!   few points the same algorithm, ear for ear.
//!
//! The scene builder must set [`Input::shown`] (`daylight`, on by default); a `SceneInput::new`
//! has no night on it.

use glam::DVec2;
use rustradar_core::geo::Basemap;
use rustradar_core::sun::{NightQuad, night_polygons};
use rustradar_scene::camera::{WORLD_SIZE, lng_lat_to_world};
use rustradar_scene::geom::densify::GLOBE_RESOLUTION_DEG;
use rustradar_scene::geom::earcut::earcut;
use rustradar_scene::geom::grid_cut::{GridOptions, cut_polygon_by_grid};

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::frame::{Frame, FrameSlot};
use crate::pipelines::fill::{self, Fill, Mesh};
use crate::renderer::SceneInput;
use crate::state::DrawState;

/// The bands' fill (`NIGHT_COLOR`, G:1096): alpha 46/255 ≈ 0.18 a set.
pub const NIGHT_COLOR: [u8; 4] = [2, 6, 14, 46];

/// How long the bands are kept (`Math.floor(now / 60_000)`, G:1099).
pub const MINUTE_MS: f64 = 60_000.0;

/// What the scene builder hands this layer (`SceneInput::night`).
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Whether the night is shaded: the `daylight` setting (doc 06 §23, on by default). Off in a
    /// scene made with `SceneInput::new` (which has nothing on it). Ignored on the realistic
    /// basemap, which never has the layer.
    pub shown: bool,
}

impl Input {
    /// The night shaded (`daylight` on).
    pub const SHOWN: Input = Input { shown: true };
}

/// Whether `scene` has the night layer: `daylight` on, and not realistic (G:2692).
pub fn drawn(scene: &SceneInput) -> bool {
    scene.night.shown && scene.basemap != Basemap::Realistic
}

/// deck's `Polygon.normalize` of a quad, a simple polygon of `[lon, lat]` points: the points one
/// after another (each number read as `x || 0`, so NaN and −0 become 0), closed by the first point
/// unless the quad already ends where it starts (`isNestedRingClosed`), then turned clockwise as
/// math.gl reckons it (`modifyPolygonWindingDirection`: reversed unless the sign of the shoelace
/// area `Σ (xᵢ − xⱼ)(yᵢ + yⱼ) / 2` is 1, so a ring of no area is reversed too). Two numbers a
/// vertex: deck keeps a third, always 0, which changes nothing here.
pub fn normalize(quad: &NightQuad) -> Vec<f64> {
    let or_zero = |v: f64| if v == 0.0 || v.is_nan() { 0.0 } else { v };
    let mut ring: Vec<f64> = quad.iter().flatten().map(|&v| or_zero(v)).collect();
    if quad[0] != quad[3] {
        ring.extend(quad[0].map(or_zero));
    }
    let n = ring.len();
    let mut area = 0.0;
    let mut j = n - 2;
    let mut i = 0;
    while i < n {
        area += (ring[i] - ring[j]) * (ring[i + 1] + ring[j + 1]);
        j = i;
        i += 2;
    }
    area /= 2.0;
    // Math.sign(area) !== WINDING.CLOCKWISE
    let clockwise = area > 0.0;
    if !clockwise {
        let points = n / 2;
        for k in 0..points / 2 {
            ring.swap(2 * k, 2 * (points - 1 - k));
            ring.swap(2 * k + 1, 2 * (points - 1 - k) + 1);
        }
    }
    ring
}

/// `quads` as deck's SolidPolygonLayer tessellates them for the globe (`globe`: each normalised
/// quad cut on the 5° grid, each part earcut) or for the flat map (each normalised quad earcut
/// whole), in the quads' order, every vertex in [`NIGHT_COLOR`]. Vertices are in the local
/// Mercator frame of the whole world, as the land's (`earth::land_mesh`): anchor at world
/// `(0, 512)`, scale 512, so `u = x / 512`, `v = (512 − y) / 512`, at height 0. Triangles keep
/// earcut's turn (nothing is culled). A part earcut leaves no triangle in adds no vertex.
pub fn night_mesh(quads: &[NightQuad], globe: bool) -> (Vec<fill::Vertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut add = |positions: &[f64], holes: &[usize]| {
        let triangles = earcut(positions, holes, 2);
        if triangles.is_empty() {
            return;
        }
        let base = vertices.len() as u32;
        for p in positions.chunks_exact(2) {
            let [x, y] = lng_lat_to_world([p[0], p[1]]);
            vertices.push(fill::Vertex { position: [(x / WORLD_SIZE) as f32, ((WORLD_SIZE - y) / WORLD_SIZE) as f32, 0.0], color: NIGHT_COLOR });
        }
        indices.extend(triangles.into_iter().map(|i| base + i as u32));
    };
    let options = GridOptions { edge_types: true, ..GridOptions::grid(GLOBE_RESOLUTION_DEG) };
    for quad in quads {
        let ring = normalize(quad);
        if globe {
            for part in cut_polygon_by_grid(&ring, &[], &options) {
                // math.gl counts hole starts in values, earcut in vertices
                let holes: Vec<usize> = part.hole_indices.iter().map(|h| h / 2).collect();
                add(&part.positions, &holes);
            }
        } else {
            add(&ring, &[]);
        }
    }
    (vertices, indices)
}

/// The minute's bands and what each projection draws of them: today's `night` cache (the minute
/// and its quads) and the SolidPolygonLayer's tessellation of them, one per projection (see the
/// module notes). `M` is the drawn mesh (a GPU [`Mesh`] in the layer, anything in tests).
#[derive(Debug)]
pub struct Bands<M> {
    /// `floor(now / 60 000)` of [`Self::quads`]; `None` before the first (today's `-1`).
    minute: Option<f64>,
    /// `nightPolygons(now)` at the first `now` of that minute.
    quads: Vec<NightQuad>,
    /// The quads tessellated for the globe and for the flat map, made when first drawn.
    globe: Option<M>,
    flat: Option<M>,
}

impl<M> Default for Bands<M> {
    fn default() -> Self {
        Bands { minute: None, quads: Vec::new(), globe: None, flat: None }
    }
}

impl<M> Bands<M> {
    /// What to draw at `now_ms` (the feed clock) on the globe or the flat map: the quads made
    /// again when the minute is not theirs (`night.minute !== minute`, so a NaN clock makes them
    /// every time), and their mesh for that projection, made by `build(quads, globe)` unless this
    /// minute already has it.
    pub fn mesh(&mut self, now_ms: f64, globe: bool, build: impl FnOnce(&[NightQuad], bool) -> M) -> &M {
        let minute = (now_ms / MINUTE_MS).floor();
        if self.minute != Some(minute) {
            self.minute = Some(minute);
            self.quads = night_polygons(now_ms);
            self.globe = None;
            self.flat = None;
        }
        let Bands { quads, globe: on_globe, flat, .. } = self;
        let slot = if globe { on_globe } else { flat };
        slot.get_or_insert_with(|| build(quads, globe))
    }

    /// The mesh made for the globe or the flat map in the current minute, if it was.
    pub fn get(&self, globe: bool) -> Option<&M> {
        if globe { self.globe.as_ref() } else { self.flat.as_ref() }
    }

    /// The current minute's quads.
    pub fn quads(&self) -> &[NightQuad] {
        &self.quads
    }

    /// Drops the meshes and keeps the minute's quads: the layer left the list (deck finalizes
    /// it; Globe.svelte keeps `night`).
    pub fn release(&mut self) {
        self.globe = None;
        self.flat = None;
    }
}

/// `frame` moved `dx` world units east, its offset split again into hi + lo in f64 (as the land's
/// copies are; [`crate::frame::ViewFrame::local`] would wrap the anchor back to the nearest copy).
fn shifted(frame: &Frame, dx: f64) -> Frame {
    let x = f64::from(frame.offset_hi[0]) + f64::from(frame.offset_lo[0]) + dx;
    let hi = x as f32;
    Frame { offset_hi: [hi, frame.offset_hi[1]], offset_lo: [(x - f64::from(hi)) as f32, frame.offset_lo[1]], ..*frame }
}

/// Row 6: `night` (`daylight`, not realistic).
pub struct Night {
    fill: Fill,
    style: fill::Style,
    bands: Bands<Mesh<fill::Vertex>>,
    /// This frame's projection.
    globe: bool,
    /// This frame's frames: the world, and on the flat map its copies west and east.
    slots: Vec<FrameSlot>,
}

impl Night {
    pub fn new(init: &Init<'_>) -> Self {
        let fill = Fill::new(init, "night", DrawState::SURFACE_NO_CULL);
        let style = fill.style(&init.gpu.device, fill::OPAQUE);
        Night { fill, style, bands: Bands::default(), globe: true, slots: Vec::new() }
    }
}

impl Layer for Night {
    fn name(&self) -> &'static str {
        "night"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        self.slots.clear();
        if !drawn(cx.scene) {
            self.bands.release();
            return;
        }
        let globe = cx.view.is_globe();
        let device = &cx.gpu.device;
        self.bands.mesh(cx.scene.feed_now_ms, globe, |quads, globe| {
            let (vertices, indices) = night_mesh(quads, globe);
            Mesh::new(device, "night", &vertices, &indices)
        });
        self.globe = globe;
        let world = cx.view.local(DVec2::new(0.0, WORLD_SIZE), WORLD_SIZE, 0.0);
        if globe {
            self.slots.push(cx.frames.push(&world));
        } else {
            for copy in [-1.0, 0.0, 1.0] {
                self.slots.push(cx.frames.push(&shifted(&world, copy * WORLD_SIZE)));
            }
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if self.slots.is_empty() || cx.pass != Pass::Map {
            return;
        }
        let Some(mesh) = self.bands.get(self.globe) else { return };
        self.fill.bind(pass, &self.style);
        for slot in &self.slots {
            cx.bind_frame(pass, *slot);
            mesh.draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Instant;

    use rustradar_scene::camera::{ViewState, world_to_lng_lat};

    use super::*;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::DirTiles;
    use crate::layers::{earth, vector_map};
    use crate::renderer::MapRenderer;

    /// 2026-03-20 12:00 UTC: noon at Greenwich two hours before the March equinox (the row's clock).
    const EQUINOX_NOON: f64 = 1_774_008_000_000.0;
    /// 2026-06-21 10:00 UTC: the reference pictures' clock (`docs/port/reference/README.md`).
    const REFERENCE_CLOCK: f64 = 1_782_036_000_000.0;
    /// 2026-12-21 22:00 UTC: deep night in Stockholm.
    const MIDWINTER_NIGHT: f64 = 1_797_890_400_000.0;

    /// The dark sea and the land under 0 to 4 bands, as the GPU rounds each set to bytes (the
    /// levels today's pictures show).
    const SEA: [[u8; 3]; 5] = [[30, 30, 30], [25, 26, 27], [21, 22, 25], [18, 19, 23], [15, 17, 21]];
    const LAND: [[u8; 3]; 5] = [[50, 50, 50], [41, 42, 44], [34, 36, 39], [28, 31, 34], [23, 26, 30]];

    fn view(latitude: f64, longitude: f64, zoom: f64, pitch: f64, bearing: f64) -> ViewState {
        ViewState { latitude, longitude, zoom, pitch, bearing, position: None }
    }

    /// `c` blended over `base` `n` times with [`NIGHT_COLOR`] as deck's blending does it, rounded
    /// to bytes after each.
    fn bands_over(base: [u8; 3], n: usize) -> [u8; 3] {
        let alpha = f64::from(NIGHT_COLOR[3]) / 255.0;
        let mut c = base.map(f64::from);
        for _ in 0..n {
            c = std::array::from_fn(|i| (c[i] + (f64::from(NIGHT_COLOR[i]) - c[i]) * alpha).round());
        }
        c.map(|v| v as u8)
    }

    #[test]
    fn the_levels_are_four_sets_of_46_over_255() {
        for n in 0..=4 {
            assert_eq!(bands_over(SEA[0], n), SEA[n]);
            assert_eq!(bands_over(LAND[0], n), LAND[n]);
        }
    }

    /// deck's `Polygon.normalize`: a quad comes back closed and clockwise (math.gl's sign, y up);
    /// one tapered on its west side already ends where it starts and is not closed again.
    #[test]
    fn a_quad_is_closed_and_turned_clockwise_as_deck_normalises_it() {
        // counter-clockwise with y up, as nightBelow makes every quad: reversed whole, closing point included
        let quad = [[0.0, -10.0], [2.0, -12.0], [2.0, 12.0], [0.0, 10.0]];
        assert_eq!(normalize(&quad), vec![0.0, -10.0, 0.0, 10.0, 2.0, 12.0, 2.0, -12.0, 0.0, -10.0]);
        // already clockwise: closed only
        let clockwise = [[0.0, -10.0], [0.0, 10.0], [2.0, 12.0], [2.0, -12.0]];
        assert_eq!(normalize(&clockwise), vec![0.0, -10.0, 0.0, 10.0, 2.0, 12.0, 2.0, -12.0, 0.0, -10.0]);
        // tapered on the west: [lon, m] twice, the ring closed as it is
        let west = [[4.0, 5.0], [6.0, 1.0], [6.0, 9.0], [4.0, 5.0]];
        assert_eq!(normalize(&west), vec![4.0, 5.0, 6.0, 9.0, 6.0, 1.0, 4.0, 5.0]);
        // tapered on the east: closed, with the doubled point kept
        let east = [[4.0, 1.0], [6.0, 5.0], [6.0, 5.0], [4.0, 9.0]];
        assert_eq!(normalize(&east), vec![4.0, 1.0, 4.0, 9.0, 6.0, 5.0, 6.0, 5.0, 4.0, 1.0]);
        // `x || 0`: −0 is 0
        let ring = normalize(&[[-0.0, -10.0], [2.0, -12.0], [2.0, 12.0], [-0.0, 10.0]]);
        assert!(ring.iter().all(|v| !(*v == 0.0 && v.is_sign_negative())), "{ring:?}");
        // no area: reversed, as Math.sign(0) is not 1
        assert_eq!(normalize(&[[0.0, 3.0], [2.0, 3.0], [2.0, 3.0], [0.0, 3.0]]), vec![0.0, 3.0, 2.0, 3.0, 2.0, 3.0, 0.0, 3.0]);
    }

    /// `[lon, lat]` of a mesh vertex (the world frame's `u`, `v`).
    fn lng_lat(v: &fill::Vertex) -> [f64; 2] {
        let x = f64::from(v.position[0]) * WORLD_SIZE;
        let y = WORLD_SIZE - f64::from(v.position[1]) * WORLD_SIZE;
        world_to_lng_lat([x, y])
    }

    /// The shoelace area of a ring of `[lon, lat]` (degrees², either turn).
    fn area(points: &[[f64; 2]]) -> f64 {
        let n = points.len();
        (0..n).map(|i| {
            let (p, q) = (points[i], points[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        }).sum::<f64>().abs() / 2.0
    }

    /// The globe's mesh is every quad cut on the 5° grid: no triangle leaves its cell, and the
    /// triangles cover the quads exactly once (their areas add up); the flat map's is the quads
    /// whole, at most two triangles each. Both in the night's colour, on the ground, in the world
    /// frame.
    #[test]
    fn the_globe_s_bands_are_cut_on_the_5_degree_grid_and_the_flat_map_s_are_whole() {
        let quads = night_polygons(EQUINOX_NOON);
        let expected: f64 = quads.iter().map(|q| area(q)).sum();
        for globe in [true, false] {
            let started = Instant::now();
            let (vertices, indices) = night_mesh(&quads, globe);
            eprintln!(
                "{} quads, {} vertices, {} triangles for the {} in {:?}",
                quads.len(),
                vertices.len(),
                indices.len() / 3,
                if globe { "globe" } else { "flat map" },
                started.elapsed()
            );
            assert!(vertices.iter().all(|v| v.color == NIGHT_COLOR && v.position[2] == 0.0));
            assert!(vertices.iter().all(|v| (0.0..=1.0).contains(&v.position[0]) && (0.0..=1.0).contains(&v.position[1])));
            assert!(indices.iter().all(|&i| (i as usize) < vertices.len()));
            let mut covered = 0.0;
            for t in indices.chunks_exact(3) {
                let corners = [0, 1, 2].map(|k| lng_lat(&vertices[t[k] as usize]));
                covered += area(&corners);
                if globe {
                    // all three corners in the cell of the triangle's middle
                    let middle = [0, 1].map(|d| corners.iter().map(|c| c[d]).sum::<f64>() / 3.0);
                    let cell = middle.map(|m| (m / GLOBE_RESOLUTION_DEG).floor() * GLOBE_RESOLUTION_DEG);
                    for c in corners {
                        for d in 0..2 {
                            assert!(c[d] >= cell[d] - 1e-4 && c[d] <= cell[d] + GLOBE_RESOLUTION_DEG + 1e-4, "{corners:?} leaves {cell:?}");
                        }
                    }
                }
            }
            assert!((covered - expected).abs() < 1e-5 * expected, "{covered} against {expected}");
            if globe {
                assert!(indices.len() / 3 > 10 * quads.len(), "cut: {} triangles", indices.len() / 3);
            } else {
                assert!(indices.len() <= 6 * quads.len());
            }
        }
    }

    /// The minute's quads are made once, from the first clock of the minute, and kept while the
    /// layer is off; each projection's mesh is made on its first frame in the minute and kept
    /// across zoom 12.
    #[test]
    fn the_bands_are_made_again_each_minute_and_tessellated_per_projection() {
        let t0 = (EQUINOX_NOON / MINUTE_MS).floor() * MINUTE_MS + 5_000.0;
        let mut bands: Bands<(bool, usize)> = Bands::default();
        let mut made = 0;
        let mut mesh = |bands: &mut Bands<(bool, usize)>, now: f64, globe: bool| {
            *bands.mesh(now, globe, |_, globe| {
                made += 1;
                (globe, made)
            })
        };
        assert_eq!(mesh(&mut bands, t0, true), (true, 1));
        assert!(!bands.quads().is_empty());
        assert_eq!(bands.quads(), night_polygons(t0).as_slice());
        // later in the minute: kept, and the quads are still the first clock's
        assert_eq!(mesh(&mut bands, t0 + 50_000.0, true), (true, 1));
        assert_eq!(bands.quads(), night_polygons(t0).as_slice());
        assert_ne!(night_polygons(t0), night_polygons(t0 + 50_000.0), "the sun moves within a minute");
        // zoom 12 crossed: the flat map's own tessellation, then the globe's again as it was
        assert_eq!(mesh(&mut bands, t0 + 51_000.0, false), (false, 2));
        assert_eq!(mesh(&mut bands, t0 + 52_000.0, true), (true, 1));
        assert_eq!(mesh(&mut bands, t0 + 53_000.0, false), (false, 2));
        assert!(bands.get(true).is_some() && bands.get(false).is_some());
        // the next minute: new quads from its first clock, the meshes made again as drawn
        let t1 = t0 + 56_000.0;
        assert_eq!(mesh(&mut bands, t1, false), (false, 3));
        assert_eq!(bands.quads(), night_polygons(t1).as_slice());
        assert!(bands.get(true).is_none());
        // off and on again within the minute: the same quads, made into a mesh again
        bands.release();
        assert!(bands.get(true).is_none() && bands.get(false).is_none());
        assert_eq!(mesh(&mut bands, t1 + 30_000.0, true), (true, 4));
        assert_eq!(bands.quads(), night_polygons(t1).as_slice());
        // a clock that is not a number makes them every time, as `NaN !== NaN`
        assert_eq!(mesh(&mut bands, f64::NAN, true), (true, 5));
        assert_eq!(mesh(&mut bands, f64::NAN, true), (true, 6));
    }

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// A scene at `now` (both clocks) with the earth, the land, on dark the vector map, and the
    /// night if `night`.
    fn scene(view: ViewState, size: [f64; 2], basemap: Basemap, now: f64, night: bool) -> SceneInput {
        let mut scene = SceneInput::new(view, size, 1.0, basemap);
        scene.local_now_ms = now;
        scene.feed_now_ms = now;
        scene.earth = earth::Input::SHOWN;
        scene.vector_map = vector_map::Input { shown: basemap == Basemap::Dark };
        if night {
            scene.night = Input::SHOWN;
        }
        scene
    }

    /// `scene` rendered with the committed tiles (none may be missing unless `misses_allowed`),
    /// saved as `name` when given.
    fn shot(gpu: &Gpu, scene: &SceneInput, name: Option<&str>, misses_allowed: bool) -> Image {
        let tiles = Arc::new(DirTiles::fixtures());
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(tiles.clone(), None);
        let image = render_with(gpu, &mut renderer, scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        assert!(misses_allowed || tiles.misses().is_empty(), "the fixtures lack {:?}", tiles.misses());
        if let Some(name) = name {
            image.save_png(&shots_dir().join(name)).expect("the shot written");
        }
        image
    }

    fn rgb(image: &Image, x: u32, y: u32) -> [u8; 3] {
        let p = image.pixel(x, y);
        [p[0], p[1], p[2]]
    }

    /// How far (levels, each channel) a shaded pixel may be from the levels above. SwiftShader,
    /// which the port is checked with, blends as WebGL did for today's pictures and gives them
    /// exactly; llvmpipe (lavapipe) rounds each blend its own way and ends up to 2 levels off
    /// after four bands (`(13,17,23)` for the deep night's `(15,17,21)`).
    const TOLERANCE: i32 = 2;

    fn within(got: [u8; 3], expected: [u8; 3], tolerance: i32) -> bool {
        got.iter().zip(expected).all(|(g, e)| (i32::from(*g) - i32::from(e)).abs() <= tolerance)
    }

    /// Item B5's acceptance, at the shots' 1100 × 720 on the dark map from the fixtures: at an
    /// equinox noon UTC the centre of `--view 0,180,1.9` (the Pacific, the night's middle) is the
    /// sea under all four bands, `(15,17,21)` ± 2, and so is nearly all of the disc round it; at
    /// `--view 0,0,1.9` the sea near the centre (the Gulf of Guinea, under the sun) stays
    /// `(30,30,30)`, nothing there touched by the night.
    #[test]
    fn deep_night_at_an_equinox_noon_is_four_bands_deep() {
        let Some(gpu) = gpu() else { return };
        let size = [1100.0, 720.0];
        let antipode = view(0.0, 180.0, 1.9, 0.0, 0.0);
        let night = shot(&gpu, &scene(antipode, size, Basemap::Dark, EQUINOX_NOON, true), Some("b5-dark-0,180,1.9-equinox-noon.png"), false);
        let day = shot(&gpu, &scene(antipode, size, Basemap::Dark, EQUINOX_NOON, false), None, false);
        let centre = rgb(&night, 550, 360);
        eprintln!("centre at 0,180: {centre:?} (without the night {:?})", rgb(&day, 550, 360));
        assert_eq!(rgb(&day, 550, 360), SEA[0], "the centre is sea");
        assert!(within(centre, SEA[4], TOLERANCE), "{centre:?}");
        // the middle of the disc: deep night over the sea (and a few islands)
        let mut deep = 0;
        let mut total = 0;
        for y in 210..510 {
            for x in 400..700 {
                total += 1;
                let (n, d) = (rgb(&night, x, y), rgb(&day, x, y));
                deep += usize::from(within(n, bands_over(d, 4), TOLERANCE));
            }
        }
        eprintln!("{deep} of {total} middle pixels four bands deep");
        assert!(deep as f64 > 0.995 * total as f64);

        let noon = view(0.0, 0.0, 1.9, 0.0, 0.0);
        let night = shot(&gpu, &scene(noon, size, Basemap::Dark, EQUINOX_NOON, true), Some("b5-dark-0,0,1.9-equinox-noon.png"), false);
        let day = shot(&gpu, &scene(noon, size, Basemap::Dark, EQUINOX_NOON, false), None, false);
        assert_eq!(rgb(&night, 550, 360), SEA[0], "the sea under the sun");
        let changed = (160..560).flat_map(|y| (350..750).map(move |x| (x, y))).filter(|&(x, y)| night.pixel(x, y) != day.pixel(x, y)).count();
        assert_eq!(changed, 0, "{changed} pixels round the subsolar point darkened");
    }

    fn reference(name: &str) -> Image {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
        let rgba = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).into_rgba8();
        Image { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
    }

    /// The map in today's pictures at 1440 × 900: right of the sidebar, left of the map controls,
    /// under the mode bar and the dev readout.
    const MAP: [u32; 4] = [360, 80, 1370, 880];

    /// For each of today's pixels inside [`MAP`] that is the sea or the land under 0 to 4 bands,
    /// whether this shot has its colour (within [`TOLERANCE`]), at the shift within 6 px that
    /// keeps most of the shaded ones (today's pictures stand about 4.5 px higher and 0.5 px left
    /// of deck's viewport; T1): per level (sea 0–4, land 0–4) the share kept and how many today
    /// has, and the shift.
    fn agreement(ours: &Image, today: &Image) -> (Vec<(f64, usize)>, [i32; 2]) {
        let classes: Vec<[u8; 3]> = SEA.iter().chain(&LAND).copied().collect();
        let [x0, y0, x1, y1] = MAP;
        let mut best = (Vec::new(), [0, 0], -1.0);
        for dy in -6..=6 {
            for dx in -6..=6 {
                let mut total = vec![0usize; classes.len()];
                let mut kept = vec![0usize; classes.len()];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let theirs = rgb(today, x, y);
                        let Some(class) = classes.iter().position(|&c| c == theirs) else { continue };
                        total[class] += 1;
                        let mine = rgb(ours, (x as i32 + dx) as u32, (y as i32 + dy) as u32);
                        kept[class] += usize::from(within(mine, theirs, TOLERANCE));
                    }
                }
                let shares: Vec<(f64, usize)> =
                    kept.iter().zip(&total).map(|(&k, &t)| (if t == 0 { 1.0 } else { k as f64 / t as f64 }, t)).collect();
                // the shift that best fits the night's levels (the plain sea and land, most of
                // the picture, fit any shift)
                let score: f64 = kept.iter().enumerate().filter(|(k, _)| k % 5 != 0).map(|(_, &n)| n).sum::<usize>() as f64;
                if score > best.2 {
                    best = (shares, [dx, dy], score);
                }
            }
        }
        (best.0, best.1)
    }

    /// Today's V8 (the Pacific at 22:00 local time) and V1 (the whole globe at 10:00 UTC on
    /// midsummer day, the western limb in the night) at 1440 × 900, as in `docs/port/reference/`:
    /// of today's pixels that are the sea or the land under 0 to 4 bands, nearly all are the same
    /// colour here, so the bands lie where today's lie and stack as deep.
    #[test]
    fn v8_and_v1_shade_the_night_where_today_s_app_does() {
        let Some(gpu) = gpu() else { return };
        let size = [1440.0, 900.0];
        for (name, view) in [("dark-v8-1440x900.png", view(0.0, 179.5, 3.0, 0.0, 0.0)), ("dark-v1-1440x900.png", ViewState::INITIAL)] {
            let ours = shot(&gpu, &scene(view, size, Basemap::Dark, REFERENCE_CLOCK, true), Some(&format!("b5-{name}")), false);
            let today = reference(name);
            let (shares, shift) = agreement(&ours, &today);
            eprintln!("{name}: shift {shift:?}");
            for (k, (share, total)) in shares.iter().enumerate() {
                let (what, level) = if k < 5 { ("sea", k) } else { ("land", k - 5) };
                eprintln!("  {what} under {level} bands: {total} pixels today, {:.1} % the same here", share * 100.0);
            }
            let sum = |levels: &dyn Fn(usize) -> bool| {
                let picked = shares.iter().enumerate().filter(|(k, _)| levels(*k));
                let (kept, total) = picked.fold((0.0, 0), |(k, t), (_, (s, n))| (k + s * *n as f64, t + n));
                (kept / total as f64, total)
            };
            let (all, total) = sum(&|_| true);
            let (shaded, shaded_total) = sum(&|k| k % 5 != 0);
            eprintln!("  all: {:.2} % of {total}; shaded: {:.2} % of {shaded_total}", all * 100.0, shaded * 100.0);
            assert!(all > 0.99, "{name}: {all:.4}");
            if name.starts_with("dark-v8") {
                // the Pacific's night: every level today shows widely is here where it is today
                for (k, (share, total)) in shares.iter().enumerate() {
                    assert!(*total < 1000 || *share > 0.95, "{name}: level {k}: {share:.3} of {total}");
                }
            } else {
                // V1: the night is a sliver along the western limb, its few shaded pixels mostly
                // in the limb's antialiased edge
                assert!(shaded_total > 1000 && shaded > 0.6, "{name}: {shaded:.3} of {shaded_total}");
            }
        }
    }

    /// Above zoom 12 the night shades the flat map as well, every tile of it: Arlanda at V4 in the
    /// midwinter night is all four bands deep, and at the reference's midsummer noon untouched. At
    /// the antimeridian the land's copy east of it is shaded too.
    #[test]
    fn the_flat_map_is_shaded_too() {
        let Some(gpu) = gpu() else { return };
        let size = [1100.0, 720.0];
        let v4 = view(59.6519, 17.9186, 13.5, 45.0, 30.0);
        let night = shot(&gpu, &scene(v4, size, Basemap::Dark, MIDWINTER_NIGHT, true), Some("b5-dark-v4-midwinter-night.png"), false);
        let day = shot(&gpu, &scene(v4, size, Basemap::Dark, MIDWINTER_NIGHT, false), None, false);
        // the land and the water of the day picture, four bands deep in the night's (a pixel on
        // an edge averages its samples, each rounded after every band, so a few are off)
        for (what, base) in [("land", 0), ("water", 1)] {
            let (plain, deep) = if base == 0 { (LAND[0], LAND[4]) } else { (SEA[0], SEA[4]) };
            let mut total = 0;
            let mut kept = 0;
            for y in 0..720 {
                for x in 0..1100 {
                    if rgb(&day, x, y) == plain {
                        total += 1;
                        kept += usize::from(within(rgb(&night, x, y), deep, TOLERANCE));
                    }
                }
            }
            eprintln!("V4 at midwinter night: {kept} of {total} {what} pixels four bands deep");
            assert!(total > 5_000 && kept as f64 > 0.995 * total as f64, "{what}: {kept} of {total}");
        }
        // and everything else (aprons, runways, taxiways, roads) darker in every channel
        let lighter: Vec<_> = (0..720)
            .flat_map(|y| (0..1100).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let (n, d) = (rgb(&night, x, y), rgb(&day, x, y));
                (0..3).any(|i| n[i] > d[i]) || n == d
            })
            .map(|(x, y)| (x, y, rgb(&day, x, y), rgb(&night, x, y)))
            .collect();
        assert!(lighter.is_empty(), "{} pixels not darkened, e.g. {:?}", lighter.len(), &lighter[..lighter.len().min(5)]);
        let noon = shot(&gpu, &scene(v4, size, Basemap::Dark, REFERENCE_CLOCK, true), None, false);
        let plain = shot(&gpu, &scene(v4, size, Basemap::Dark, REFERENCE_CLOCK, false), None, false);
        assert!(noon.rgba == plain.rgba, "the midsummer noon is shaded");

        // Taveuni at local midnight, the antimeridian a little east of the centre: only the land
        // (no tiles), all of it four bands deep, the copy east of 180° too
        let fiji = view(-16.8, 179.99, 13.0, 0.0, 0.0);
        let mut night_scene = scene(fiji, size, Basemap::Dark, EQUINOX_NOON, true);
        night_scene.vector_map = vector_map::Input::default();
        let image = shot(&gpu, &night_scene, Some("b5-dark-antimeridian-z13-midnight.png"), false);
        for (x, y) in [(100, 100), (550, 360), (1000, 600), (1090, 10)] {
            assert!(within(rgb(&image, x, y), LAND[4], TOLERANCE), "({x}, {y}): {:?}", rgb(&image, x, y));
        }
    }

    /// Crossing zoom 12 within a minute keeps the night drawn: from the flat map back to the
    /// globe, deck would draw the flat map's whole quads on the sphere, hidden inside the earth;
    /// here the globe draws its own cut bands at once.
    #[test]
    fn crossing_zoom_12_keeps_the_night() {
        let Some(gpu) = gpu() else { return };
        let size = [800.0, 600.0];
        let mut renderer = MapRenderer::new(&gpu, FORMAT);
        for zoom in [12.1, 11.9, 12.1, 11.9] {
            let mut s = scene(view(0.0, 180.0, zoom, 30.0, 0.0), size, Basemap::Dark, EQUINOX_NOON, true);
            s.vector_map = vector_map::Input::default();
            let image = render_with(&gpu, &mut renderer, &s, 1.0).expect("rendered");
            let all = (0..600).step_by(10).flat_map(|y| (0..800).step_by(10).map(move |x| (x, y)));
            let wrong: Vec<_> = all.filter(|&(x, y)| !within(rgb(&image, x, y), LAND[4], TOLERANCE)).collect();
            assert!(wrong.is_empty(), "zoom {zoom}: {} pixels not deep night, e.g. {:?} {:?}", wrong.len(), wrong[0], rgb(&image, wrong[0].0, wrong[0].1));
        }
    }

    /// The realistic basemap never has the night, whatever the input; off means off; the
    /// satellite map is shaded like the dark one.
    #[test]
    fn realistic_never_has_the_night_and_off_means_off() {
        let Some(gpu) = gpu() else { return };
        let size = [400.0, 300.0];
        let antipode = view(0.0, 180.0, 1.9, 0.0, 0.0);
        let shown = shot(&gpu, &scene(antipode, size, Basemap::Realistic, EQUINOX_NOON, true), None, true);
        let off = shot(&gpu, &scene(antipode, size, Basemap::Realistic, EQUINOX_NOON, false), None, true);
        assert!(shown.rgba == off.rgba, "realistic shaded");
        let mut realistic = scene(antipode, size, Basemap::Realistic, EQUINOX_NOON, true);
        assert!(!drawn(&realistic));
        realistic.basemap = Basemap::Satellite;
        assert!(drawn(&realistic));
        realistic.night = Input::default();
        assert!(!drawn(&realistic));

        let dark_off = shot(&gpu, &scene(antipode, size, Basemap::Dark, EQUINOX_NOON, false), None, false);
        assert_eq!(rgb(&dark_off, 200, 150), SEA[0]);
        let sat_on = shot(&gpu, &scene(antipode, size, Basemap::Satellite, EQUINOX_NOON, true), None, true);
        let sat_off = shot(&gpu, &scene(antipode, size, Basemap::Satellite, EQUINOX_NOON, false), None, true);
        let (on, off) = (rgb(&sat_on, 200, 150), rgb(&sat_off, 200, 150));
        eprintln!("satellite centre: {on:?} shaded, {off:?} not");
        assert!(within(on, bands_over(off, 4), TOLERANCE), "{on:?} against {off:?}");
    }
}
