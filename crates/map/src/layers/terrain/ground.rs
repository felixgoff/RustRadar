//! What stands on the 3D terrain (item Z3; doc 04 §14.3, §14.5; doc 03 §9; plan 1.4.12): the
//! layers today's app raises with deck.gl's `TerrainExtension` in its *offset* mode, raised here on
//! the CPU by the drawn surface's height under them ([`rustradar_scene::terrain::heights`]).
//!
//! deck's rule (`terrain-extension.js`): a layer with per-instance anchors (`instancePositions`:
//! mesh, icon, scatterplot and text layers) or `extruded` is offset, everything else draped. An
//! offset vertex takes the ground's height under `geometry.worldPosition` with its height set to 0:
//! for an instanced layer that is the instance's anchor (a whole aircraft, car or icon is raised by
//! the ground under its anchor), for an extruded polygon layer, and a mesh layer in `METER_OFFSETS`
//! (whose model matrix is composed), each vertex's own place (a building's walls and roofs follow the
//! ground under each corner). What that makes of today's layers:
//!
//! | Raised | How, here |
//! |---|---|
//! | aircraft (models, gear, icons, labels, the selection ring), ships (icons, models), trains (icons, cars) | [`stand`]: the scene's positions are raised as the renderer takes the scene in, so the pick records read the same places |
//! | ship and train halos and rings, the train route's stops, signals and their names, the markers, sightings | their layers, by [`Ground::at`] where they place them |
//! | aircraft shadows | on the ground under the aircraft, cast from its height above it |
//! | place labels, the rail map's stations and speed labels | their layers, by [`Ground::at`] |
//! | buildings' walls and roofs (both maps), pitched roofs | per vertex, [`Standing`] |
//! | trees and their shadows | each tree by the ground under it |
//!
//! Draped instead (Z2's covers): everything flat, including the far building tiles drawn flat
//! (`extruded: false`), and the building and foot shadows.
//!
//! The ground ([`Ground`]) is the heights of the terrain tiles drawn in the last frame, made again
//! (a new [`Ground::version`]) when those tiles change; the renderer raises the scene again from
//! what the app handed it, and the layers raise their own content again where what it stands on
//! changed ([`Footing`]).
//!
//! Deviations:
//! - The trail and the route of the selected flight keep their true heights (plan section 6, item
//!   3: today they would be draped), and so do the lifted radar layers and their walls (deck drapes
//!   every radar layer, which flattens the stack onto the ground or loses it beyond the cover's
//!   near plane); the radar's ground layer is not draped (as Z2 left it).
//! - The ground platforms of the realistic map stay flat in the covers (Z2).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rustradar_scene::terrain::heights::{HeightGrid, Heights};

use crate::frame::tile_extent;
use crate::layers::TileIndex;
use crate::renderer::SceneInput;

/// The versions handed out ([`Ground::new`]): every ground made anywhere has its own.
static VERSIONS: AtomicU64 = AtomicU64::new(1);

/// The ground the 3D layers stand on: the heights of the terrain drawn, or none (the terrain off,
/// or none of its tiles in yet), which is sea level everywhere.
#[derive(Debug, Clone, Default)]
pub struct Ground {
    heights: Option<Arc<Heights>>,
    /// 0 for none; else this ground's own.
    version: u64,
}

impl PartialEq for Ground {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl Ground {
    /// The ground of `heights` (none when it holds no tile), with a version of its own.
    pub fn new(heights: Heights) -> Self {
        if heights.is_empty() {
            return Ground::default();
        }
        Ground { heights: Some(Arc::new(heights)), version: VERSIONS.fetch_add(1, Ordering::Relaxed) }
    }

    /// Sea level everywhere.
    pub fn is_none(&self) -> bool {
        self.heights.is_none()
    }

    /// Which ground it is: two grounds of the same version are the same heights; 0 for none.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn heights(&self) -> Option<&Heights> {
        self.heights.as_deref()
    }

    /// The ground's height at `[lon, lat]`, metres: 0 where no terrain tile holds it.
    pub fn at(&self, lon_lat: [f64; 2]) -> f64 {
        self.heights.as_ref().and_then(|h| h.at(lon_lat)).unwrap_or(0.0)
    }

    /// `[lon, lat, metres]` raised by the ground under it.
    pub fn lift(&self, [lon, lat, h]: [f64; 3]) -> [f64; 3] {
        [lon, lat, h + self.at([lon, lat])]
    }

    /// The ground's tiles that `tile`'s square meets (it holds them or they hold it), deepest
    /// first: what a mesh in that tile stands on. Empty where none does.
    pub fn footing(&self, tile: TileIndex) -> Footing {
        let Some(heights) = &self.heights else { return Footing::default() };
        let mut tiles: Vec<(TileIndex, Arc<HeightGrid>)> =
            heights.grids().filter(|(t, _)| t.contains(tile) || tile.contains(*t)).map(|(t, g)| (t, Arc::clone(g))).collect();
        tiles.sort_by(|a, b| b.0.z.cmp(&a.0.z).then(a.0.x.cmp(&b.0.x)).then(a.0.y.cmp(&b.0.y)));
        Footing(tiles)
    }
}

/// The terrain tiles under a tile's square, each with its grid ([`Ground::footing`]): what a mesh
/// there is raised from. Two footings are the same when they hold the same grids.
#[derive(Debug, Clone, Default)]
pub struct Footing(Vec<(TileIndex, Arc<HeightGrid>)>);

impl PartialEq for Footing {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len() && self.0.iter().zip(&other.0).all(|(a, b)| a.0 == b.0 && Arc::ptr_eq(&a.1, &b.1))
    }
}

impl Footing {
    /// No terrain under the tile: sea level.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The height at `(u, v)` of `tile` (tile units, `v` south; a little outside the square too,
    /// for a building over its edge), metres: from the deepest of its tiles holding the place, else
    /// the ground's at the nearest of them (a place past them all), 0 with none.
    pub fn at(&self, tile: TileIndex, u: f64, v: f64) -> f64 {
        let extent = tile_extent(tile.z);
        let x = (f64::from(tile.x) + u) * extent;
        let down = (f64::from(tile.y) + v) * extent;
        let mut nearest: Option<(f64, f64)> = None;
        for (t, grid) in &self.0 {
            let e = tile_extent(t.z);
            let (fu, fv) = (x / e - f64::from(t.x), down / e - f64::from(t.y));
            if (0.0..=1.0).contains(&fu) && (0.0..=1.0).contains(&fv) {
                return grid.at(fu, fv);
            }
            let off = (fu - fu.clamp(0.0, 1.0)).abs().max((fv - fv.clamp(0.0, 1.0)).abs()) * e;
            if nearest.is_none_or(|(d, _)| off < d) {
                nearest = Some((off, grid.at(fu, fv)));
            }
        }
        nearest.map_or(0.0, |(_, h)| h)
    }
}

/// The scene as it stands on `ground` (the renderer calls it as it takes a scene in, and again
/// from what the app handed it when the ground changes): the positions the app's builders worked
/// out at sea level raised by the ground under them, so that what is drawn and what is picked agree.
/// `scene.stands_on` becomes `ground`, which the layers read for what they place themselves.
pub fn stand(scene: &mut SceneInput, ground: &Ground) {
    scene.stands_on = ground.clone();
    if ground.is_none() {
        return;
    }
    let lift = |p: &mut [f64; 3]| *p = ground.lift(*p);
    // aircraft: models and their gear, icons, the labelled ones and their labels, the ring
    let aircraft = &mut scene.aircraft.scene;
    for group in &mut aircraft.groups {
        group.instances.iter_mut().for_each(|i| lift(&mut i.position));
        group.gear.iter_mut().for_each(|g| lift(&mut g.position));
    }
    aircraft.icons.iter_mut().for_each(|i| lift(&mut i.position));
    aircraft.labelled.iter_mut().for_each(|l| lift(&mut l.position));
    scene.aircraft.labels.positions.iter_mut().flatten().for_each(lift);
    if let Some(ring) = &mut scene.markers.ring {
        lift(&mut ring.position);
    }
    // ships: icons and models
    let ships = &mut scene.ships.scene;
    ships.icons.iter_mut().for_each(|i| lift(&mut i.position));
    ships.models.iter_mut().flat_map(|g| g.models.iter_mut()).for_each(|m| lift(&mut m.position));
    // trains: icons, and the cars on their levels
    let trains = &mut scene.trains.scene;
    trains.icons.iter_mut().for_each(|i| lift(&mut i.position));
    for car in &mut trains.car_layers.cars {
        car.level.height += ground.at(car.at);
    }
}

/// The longest a frame spends raising tile meshes onto the ground again, ms (at least one tile a
/// frame is raised; the others wait, as the building shadows' recasting does).
pub const STAND_BUDGET_MS: f64 = 4.0;

/// A tile mesh standing on the ground vertex by vertex (deck's offset mode for an extruded layer,
/// and for a mesh layer in metre offsets: each vertex takes the ground's height under it): the
/// ground under each vertex in a vertex buffer of its own, one `f32` a vertex (metres), which the
/// standing pipelines add to the vertex's height, so that what the shaders read of the height
/// above the building's foot (a wall's foot shading) stays as decoded.
pub struct Standing {
    /// Each vertex's place in its tile's frame, `(u, v)` (tile units, `v` south).
    places: Vec<[f32; 2]>,
    /// The ground under each vertex, metres: 0 until raised.
    buffer: wgpu::Buffer,
    /// What the buffer was raised from (empty: at sea level).
    footing: Footing,
}

impl Standing {
    /// The vertices of a mesh in its tile's frame, each `position` `(u, v, metres)`, at sea level.
    pub fn new<V>(device: &wgpu::Device, vertices: &[V], position: impl Fn(&V) -> [f32; 3]) -> Self {
        let places: Vec<[f32; 2]> = vertices.iter().map(|v| {
            let [u, v, _] = position(v);
            [u, v]
        }).collect();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ground under the vertices"),
            // (wgpu refuses an empty vertex buffer: a few bytes for a mesh without vertices)
            size: (places.len() * size_of::<f32>()).max(16) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Standing { places, buffer, footing: Footing::default() }
    }

    /// A mesh of `vertices` vertices that stays at sea level (a far tile drawn flat, which is
    /// draped): a buffer of zeros for its pipeline to read, and no places to raise.
    pub fn at_sea(device: &wgpu::Device, vertices: usize) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ground under the vertices (the sea)"),
            size: (vertices * size_of::<f32>()).max(16) as u64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        Standing { places: Vec::new(), buffer, footing: Footing::default() }
    }

    /// Whether the mesh of `tile` needs raising again for `ground`.
    pub fn stale(&self, tile: TileIndex, ground: &Ground) -> bool {
        ground.footing(tile) != self.footing
    }

    /// Raises the mesh of `tile` onto `ground` when what it stands on there changed (back to the
    /// sea where no terrain is under the tile). Whether it wrote.
    pub fn stand(&mut self, queue: &wgpu::Queue, tile: TileIndex, ground: &Ground) -> bool {
        let footing = ground.footing(tile);
        if footing == self.footing {
            return false;
        }
        if !self.places.is_empty() {
            let heights: Vec<f32> = if footing.is_empty() {
                vec![0.0; self.places.len()]
            } else {
                self.places.iter().map(|&[u, v]| footing.at(tile, f64::from(u), f64::from(v)) as f32).collect()
            };
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&heights));
        }
        self.footing = footing;
        true
    }

    /// Sets the ground under the vertices as vertex buffer `slot`.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>, slot: u32) {
        pass.set_vertex_buffer(slot, self.buffer.slice(..));
    }

    /// The bytes it keeps on the CPU.
    pub fn byte_size(&self) -> usize {
        size_of_val(self.places.as_slice())
    }
}

/// A frame's time for raising tile meshes onto the ground again ([`STAND_BUDGET_MS`]): the first
/// tile always, the others while the time lasts; those left wait for the next frames.
pub struct StandBudget {
    until: std::time::Instant,
    any: bool,
    /// Tiles that needed raising and were left for later.
    pub left: usize,
}

impl Default for StandBudget {
    fn default() -> Self {
        StandBudget { until: std::time::Instant::now() + std::time::Duration::from_secs_f64(STAND_BUDGET_MS / 1000.0), any: false, left: 0 }
    }
}

impl StandBudget {
    /// Whether a tile that needs raising is raised now (else it is counted as left).
    pub fn take(&mut self) -> bool {
        if self.any && std::time::Instant::now() > self.until {
            self.left += 1;
            return false;
        }
        self.any = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rustradar_core::geo::Basemap;
    use rustradar_core::model::TrainShape;
    use rustradar_scene::bridges::Level;
    use rustradar_scene::camera::{ViewState, WORLD_SIZE, lng_lat_to_world, viewport_for};
    use rustradar_scene::ship_models::ShipClass;
    use rustradar_scene::ships::{ModelGroup, ShipIcon, ShipModel};
    use rustradar_scene::terrain::heights::GRID_SIDE;
    use rustradar_scene::tiles::osm_tile_to_lng_lat;
    use rustradar_scene::trains::{CarShape, DrawnCar, TrainIcon};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, render_with};
    use crate::io::{self, FnFetcher};
    use crate::layers::{buildings, markers, terrain};
    use crate::renderer::MapRenderer;

    fn sloped(base: f64) -> Arc<HeightGrid> {
        Arc::new(HeightGrid::from_fn(GRID_SIDE, move |u, v| base + 100.0 * u + 10.0 * v))
    }

    /// A ground of its own version for each set of heights, none for no tiles; a place's height
    /// and a lifted position from the deepest tile there, sea level elsewhere.
    #[test]
    fn the_ground_is_the_drawn_tiles_heights() {
        let tile = TileIndex::new(4265, 2894, 13);
        let ground = Ground::new(Heights::new([(tile, sloped(1500.0))]));
        let other = Ground::new(Heights::new([(tile, sloped(1500.0))]));
        assert!(ground.version() > 0 && ground != other && ground == ground.clone());
        assert!(Ground::new(Heights::default()).is_none());
        assert_eq!(Ground::new(Heights::default()), Ground::default());
        let lon_lat = osm_tile_to_lng_lat(4265.5, 2894.5, 13);
        assert!((ground.at(lon_lat) - 1555.0).abs() < 1e-3, "{}", ground.at(lon_lat));
        let lifted = ground.lift([lon_lat[0], lon_lat[1], 30.0]);
        assert!((lifted[2] - 1585.0).abs() < 1e-3 && lifted[0] == lon_lat[0] && lifted[1] == lon_lat[1]);
        assert_eq!(ground.at([0.0, 0.0]), 0.0);
        assert_eq!(Ground::default().lift([7.0, 46.0, 12.0]), [7.0, 46.0, 12.0]);
    }

    /// A tile's footing is the ground's tiles its square meets, deepest first; a place in the tile
    /// reads the deepest that holds it, past them all the nearest's edge; footings of the same
    /// grids are equal, of a grid made again not.
    #[test]
    fn a_tile_stands_on_the_ground_s_tiles_under_its_square() {
        let coarse = TileIndex::new(2132, 1447, 12);
        let fine = TileIndex::new(4265, 2894, 13);
        let (a, b) = (sloped(1000.0), sloped(2000.0));
        let ground = Ground::new(Heights::new([(coarse, Arc::clone(&a)), (fine, Arc::clone(&b)), (TileIndex::new(0, 0, 3), sloped(0.0))]));
        // a zoom-14 building tile in the north-east quarter of the fine tile
        let building = TileIndex::new(8531, 5788, 14);
        let footing = ground.footing(building);
        assert_eq!(footing.0.iter().map(|(t, _)| *t).collect::<Vec<_>>(), [fine, coarse]);
        // its middle is the fine tile's (0.75, 0.25)
        assert!((footing.at(building, 0.5, 0.5) - (2000.0 + 75.0 + 2.5)).abs() < 1e-3);
        // a little over the building tile's edge, still in the fine tile (its u 0.45)
        assert!((footing.at(building, -0.1, 0.5) - (2000.0 + 45.0 + 2.5)).abs() < 1e-3);
        assert_eq!(footing, ground.footing(building));
        let again = Ground::new(Heights::new([(coarse, a), (fine, sloped(2000.0))]));
        assert_ne!(footing, again.footing(building));
        // a tile no terrain tile meets
        let away = TileIndex::new(9000, 9000, 14);
        assert!(ground.footing(away).is_empty());
        assert_eq!(ground.footing(away).at(away, 0.5, 0.5), 0.0);
        assert!(Ground::default().footing(building).is_empty());
    }

    /// The scene the app hands over stands on the ground as the renderer takes it in: the ships'
    /// icons and models, the trains' icons and cars (on their bridges' heights), the selected
    /// aircraft's ring and its label, each raised by the ground under it (deck's offset mode: an
    /// instance by the ground under its anchor); on no ground the scene stays as handed over.
    #[test]
    fn the_scene_s_vehicles_and_marks_stand_on_the_ground() {
        let tile = TileIndex::new(4265, 2894, 13);
        let ground = Ground::new(Heights::new([(tile, sloped(1000.0))]));
        // a quarter of the way east and half way south: 1000 + 25 + 5
        let [lon, lat] = osm_tile_to_lng_lat(4265.25, 2894.5, 13);
        let want = 1030.0;
        let view = ViewState { latitude: lat, longitude: lon, zoom: 15.0, pitch: 45.0, bearing: 0.0, position: None };
        let mut scene = SceneInput::new(view, [400.0, 300.0], 1.0, Basemap::Dark);
        scene.terrain = terrain::Input { shown: true };
        let icon = ShipIcon { index: 0, mmsi: 7, position: [lon, lat, 0.0], name: "dot-cargo".into(), size_px: 12.0, angle: 0.0, color: [255; 4], highlighted: false };
        scene.ships.scene.icons.push(icon);
        let model = ShipModel { index: 0, position: [lon, lat, 0.0], scale: [1.0; 3], heading: 0.0, east: 0.0, north: 0.0, min_zoom: 12.5, color: [255; 4], highlighted: false };
        scene.ships.scene.models.push(ModelGroup { cls: ShipClass::Cargo, models: vec![model] });
        let train = TrainIcon { position: [lon, lat, 0.0], name: "box-x".into(), size_px: 10.0, angle: 0.0, color: [255; 4], highlighted: false };
        scene.trains.scene.icons.push(train);
        let car = DrawnCar {
            at: [lon, lat],
            heading: 0.0,
            length: 25.4,
            train: 0,
            mesh: CarShape::Plain { lead: true, shape: TrainShape::Regional },
            modelled: false,
            level: Level { tunnel: false, height: 6.0 },
            color: [255; 4],
            highlighted: false,
        };
        scene.trains.scene.car_layers.cars.push(car);
        scene.markers.ring = Some(markers::Ring { position: [lon, lat, 300.0], radius_px: 12.0 });
        scene.aircraft.labels.positions = vec![Some([lon, lat, 300.0]), None];
        let handed = scene.clone();

        stand(&mut scene, &ground);
        let near = |got: f64, want: f64| (got - want).abs() < 1e-2;
        assert_eq!(scene.stands_on, ground);
        assert!(near(scene.ships.scene.icons[0].position[2], want));
        assert!(near(scene.ships.scene.models[0].models[0].position[2], want));
        assert!(near(scene.trains.scene.icons[0].position[2], want));
        assert!(near(scene.trains.scene.car_layers.cars[0].level.height, want + 6.0));
        assert!(near(scene.markers.ring.expect("a ring").position[2], want + 300.0));
        assert!(near(scene.aircraft.labels.positions[0].expect("placed")[2], want + 300.0));
        assert_eq!(scene.aircraft.labels.positions[1], None);
        // the places themselves do not move
        assert_eq!(&scene.ships.scene.icons[0].position[..2], &[lon, lat]);

        let mut flat = handed.clone();
        stand(&mut flat, &Ground::default());
        assert!(flat.stands_on.is_none());
        assert_eq!(flat.ships.scene, handed.ships.scene);
        assert_eq!(flat.trains.scene, handed.trains.scene);
        assert_eq!(flat.markers, handed.markers);
    }

    // ------------------------------------------------------------------ on the GPU

    /// 2026-12-21 22:00 UTC: night in the Alps, so the buildings cast no shadows.
    const MIDWINTER_NIGHT: f64 = 1_797_890_400_000.0;
    /// The synthetic slope: 1200 m at the west edge of the world tile row the view is in, rising
    /// 0.3 m a metre to the east (a 17° hillside), level north to south.
    const BASE_M: f64 = 1200.0;
    const GRADE: f64 = 0.3;
    /// The buildings' height, metres.
    const HEIGHT_M: u64 = 12;
    const SIZE: [f64; 2] = [480.0, 320.0];

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// The slope's height at world point x (512 units round the world), metres east of `x0` at the
    /// latitude `lat` (math.gl's metres per unit there).
    fn slope_at(x: f64, x0: f64, lat: f64) -> f64 {
        let metres_per_unit = 40_075_016.686 * lat.to_radians().cos() / WORLD_SIZE;
        BASE_M + GRADE * (x - x0) * metres_per_unit
    }

    /// The terrarium picture of `tile` on the slope: each pixel at the place its grid point is
    /// drawn (the loader puts grid point `p` of a 256 px tile at `−1/512 + p·(514/512)/256`), so the
    /// drawn surface is the plane itself.
    fn slope_png(tile: TileIndex, x0: f64, lat: f64) -> Vec<u8> {
        let extent = WORLD_SIZE / 2f64.powi(tile.z as i32);
        let image = image::RgbImage::from_fn(256, 256, |px, _| {
            let u = -1.0 / 512.0 + f64::from(px) * (514.0 / 512.0) / 256.0;
            let h = slope_at((f64::from(tile.x) + u) * extent, x0, lat);
            let v = ((h + 32768.0) * 256.0).round() as u32;
            image::Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8])
        });
        let mut png = Vec::new();
        image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("a PNG");
        png
    }

    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(v as u8 | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn field(out: &mut Vec<u8>, number: u64, bytes: &[u8]) {
        varint(out, (number << 3) | 2);
        varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    fn number(out: &mut Vec<u8>, number: u64, v: u64) {
        varint(out, number << 3);
        varint(out, v);
    }

    fn packed(values: &[u64]) -> Vec<u8> {
        let mut out = Vec::new();
        values.iter().for_each(|&v| varint(&mut out, v));
        out
    }

    fn zigzag(v: i64) -> u64 {
        ((v << 1) ^ (v >> 63)) as u64
    }

    /// An OpenFreeMap tile with a `building` layer of squares (`[x, y, side]` in the tile's 4096
    /// units, y down), each `render_height` [`HEIGHT_M`].
    fn buildings_tile(squares: &[[i64; 3]]) -> Vec<u8> {
        let mut layer = Vec::new();
        number(&mut layer, 15, 2);
        field(&mut layer, 1, b"building");
        for &[x, y, side] in squares {
            let corners = [(x, y), (x + side, y), (x + side, y + side), (x, y + side)];
            let mut geometry = vec![(1 << 3) | 1, zigzag(corners[0].0), zigzag(corners[0].1), (3 << 3) | 2];
            for pair in corners.windows(2) {
                geometry.extend([zigzag(pair[1].0 - pair[0].0), zigzag(pair[1].1 - pair[0].1)]);
            }
            geometry.push((1 << 3) | 7);
            let mut feature = Vec::new();
            field(&mut feature, 2, &packed(&[0, 0]));
            number(&mut feature, 3, 3);
            field(&mut feature, 4, &packed(&geometry));
            field(&mut layer, 2, &feature);
        }
        field(&mut layer, 3, b"render_height");
        let mut value = Vec::new();
        number(&mut value, 5, HEIGHT_M);
        field(&mut layer, 4, &value);
        number(&mut layer, 5, 4096);
        let mut tile = Vec::new();
        field(&mut tile, 3, &layer);
        tile
    }

    /// The buildings stand on the terrain (the row's acceptance): on a 17° hillside rising to the
    /// east from 1200 m, three 12 m buildings near the middle of their zoom-14 tile, seen from the
    /// south at pitch 55 from a camera whose target stands on the ground (Z4). Each building's
    /// south wall, under its middle, is drawn down to where the CPU puts its foot on the slope
    /// (within 1.5 px) and up to its top; the terrain there is drawn where the wall ends (the
    /// picture without the buildings). At sea level, as before, they would be 1.2 km under the
    /// hillside and not drawn at all.
    #[test]
    fn buildings_on_a_hillside_stand_on_it() {
        let Some(gpu) = gpu() else { return };
        // a place near the middle of zoom-14 tile column 8544 at 46° N
        let (lon, lat) = (7.75, 46.0);
        let [wx, wy] = lng_lat_to_world([lon, lat]);
        let extent = WORLD_SIZE / 2f64.powi(14);
        let tile = TileIndex::new((wx / extent) as u32, ((WORLD_SIZE - wy) / extent) as u32, 14);
        let x0 = f64::from(tile.x) * extent;
        // the view's centre in the tile's 4096 units
        let (cu, cv) = (wx / extent - f64::from(tile.x), (WORLD_SIZE - wy) / extent - f64::from(tile.y));
        let (cx, cy) = ((cu * 4096.0) as i64, (cv * 4096.0) as i64);
        // three squares of about 20 m (48 units) side by side, west to east, just north of the centre
        let squares = [[cx - 160, cy - 80, 48], [cx - 24, cy - 80, 48], [cx + 112, cy - 80, 48]];
        let mvt = buildings_tile(&squares);
        let fetcher = Arc::new(FnFetcher(move |source: &str, t: TileIndex| {
            Ok(match source {
                s if s == io::MAPTERHORN => Some(slope_png(t, x0, lat)),
                s if s == io::OPENFREEMAP && t == tile => Some(mvt.clone()),
                _ => None,
            })
        }));
        let ground_here = slope_at(wx, x0, lat);
        let scene = |zoom: f64, pitch: f64, shown: bool| {
            let view = ViewState { latitude: lat, longitude: lon, zoom, pitch, bearing: 0.0, position: Some([0.0, 0.0, ground_here]) };
            let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Dark);
            scene.local_now_ms = MIDWINTER_NIGHT;
            scene.feed_now_ms = MIDWINTER_NIGHT;
            scene.terrain = terrain::Input { shown: true };
            scene.buildings = buildings::Input { shown };
            scene
        };
        // (the zoom-14 tile is loaded on the way in, as one zooms in: close in its zoom-16 children
        // have no source on this map and it stands in for them)
        let shoot = |shown: bool| {
            let mut renderer = MapRenderer::new(&gpu, FORMAT);
            renderer.set_tiles(fetcher.clone(), None);
            render_with(&gpu, &mut renderer, &scene(14.4, 0.0, shown), 1.0).expect("rendered on the way in");
            let close = scene(17.3, 55.0, shown);
            let image = render_with(&gpu, &mut renderer, &close, 1.0).expect("rendered");
            assert!(!renderer.ground().is_none(), "the scene stands on the terrain");
            (image, close)
        };
        let (with, close) = shoot(true);
        let (without, _) = shoot(false);
        with.save_png(&shots_dir().join("z3-hillside-buildings.png")).expect("saved");
        without.save_png(&shots_dir().join("z3-hillside-bare.png")).expect("saved");
        let differs = |x: u32, y: u32| {
            let (a, b) = (with.pixel(x, y), without.pixel(x, y));
            (0..3).any(|c| a[c].abs_diff(b[c]) > 6)
        };
        let drawn = (0..with.height).flat_map(|y| (0..with.width).map(move |x| (x, y))).filter(|&(x, y)| differs(x, y)).count();
        assert!(drawn > 300, "the buildings are drawn: {drawn} px");

        let viewport = viewport_for(&close.view, SIZE);
        for [x, y, side] in squares {
            // under the middle of the south wall
            let (u, v) = ((x as f64 + side as f64 / 2.0) / 4096.0, (y + side) as f64 / 4096.0);
            let [lon, lat] = osm_tile_to_lng_lat(f64::from(tile.x) + u, f64::from(tile.y) + v, 14);
            let foot_h = slope_at((f64::from(tile.x) + u) * extent, x0, lat);
            let foot = viewport.project([lon, lat, foot_h]);
            let top = viewport.project([lon, lat, foot_h + HEIGHT_M as f64]);
            let sunk = viewport.project([lon, lat, 0.0]);
            let column = foot[0].round() as u32;
            assert!(column < with.width && foot[1] > 0.0 && foot[1] < SIZE[1] - 8.0, "{foot:?} on the canvas");
            let rows = (top[1].floor() as u32).saturating_sub(4)..((foot[1] + 8.0) as u32).min(with.height);
            let wall: Vec<u32> = rows.clone().filter(|&r| differs(column, r)).collect();
            let lowest = *wall.last().unwrap_or_else(|| panic!("no wall at column {column} over rows {rows:?}"));
            let gap = (f64::from(lowest) + 0.5 - foot[1]).abs();
            let tall = foot[1] - top[1];
            eprintln!(
                "building at {x},{y}: foot {:.1} m at y {:.1}, top at y {:.1}, the wall drawn {} px down to row {lowest} ({gap:.2} px off); at sea level its foot would be at y {:.0}",
                foot_h,
                foot[1],
                top[1],
                wall.len(),
                sunk[1],
            );
            assert!(gap <= 1.5, "the wall ends at row {lowest}, its foot is at y {:.2}", foot[1]);
            assert!(wall.len() as f64 >= 0.6 * tall, "{} wall pixels for a wall {tall:.1} px tall", wall.len());
            assert!((sunk[1] - foot[1]).abs() > 50.0, "sea level would be somewhere else: {sunk:?}");
        }
    }
}
