//! Ships on the map, CPU side (item H2; doc 02 §4.1–4.5; `Globe.svelte:1590-1760`,
//! `:1813-1876`): where each ship is drawn at each scene refresh, which ships are models close in,
//! and the ship icons.
//!
//! **Per list** ([`ShipBuilder::set_list`], `shipsFor` for a new list object). A new list of ships
//! (the ship state's `on_map`, item H1) is taken in once: each ship at its reported position, in
//! its type colour at [`SHIP_ALPHA`] ([`SHIP_OLD_ALPHA`] for an old position), with its icon
//! ([`ship_icon`]: a pointed hull, or a dot without a heading), eased from where the list before
//! drew a ship of the same MMSI ([`ease_from`], shared with the trains: glide over 2 s, never back,
//! no easing over a jump). A ship with a heading also gets a [`ShipModel`] entry in its class's
//! group ([`ship_placement`]): its mesh scale, its hull's centre from the reported position as east
//! and north, and [`ShipModel::min_zoom`], the zoom at which its hull is [`SHIP_MESH_MIN_PX`] long.
//! A moored ship reports no heading and never gets a model: no telling which way to draw it.
//!
//! **Each refresh** ([`ShipBuilder::build`], `shipLayers`):
//! 1. the ships with frames to move along, or an easing to finish, are stepped on to now
//!    (`moveShips`; [`eased`]): the icons and the models share the drawn positions;
//! 2. from [`SHIP_MESH_ZOOM`] on (always the flat map), the split in quarter-zoom steps
//!    (`shipSplit`, `step = ⌊4·zoom⌋ / 4`, redone only when the step changes): a ship with a model
//!    entry is a model once `step ≥ min_zoom` ([`ShipsScene::models`], drawn by item H3), the
//!    others stay icons;
//! 3. the icons' angles on the screen (`shipAngles`), kept while neither the camera (zoom,
//!    bearing, pitch, centre) nor the list changes, not while ships merely move along their
//!    frames: 0 for a ship without a heading and for one beyond the visible radius
//!    ([`visible_radius`] round the view centre, [`cos_angle`]); else its reported heading
//!    projected on the screen from where it is drawn ([`screen_angle`], a step of
//!    [`ICON_HEADING_STEP_PX`] at the view centre's scale), falling back to `bearing − heading`;
//!    worked out in the projection drawn (the globe up to zoom 12, the flat map above);
//! 4. the icons ([`ShipIcon`], `ship-icons`): every ship below zoom 12.5, else those not drawn as
//!    models; billboards facing the screen, [`ship_icon_size`] px tall, turned by their angle, in
//!    their colour (the atlas holds the colours already, so only the alpha fades them), the ship
//!    hovered in the sidebar's list highlighted [`ICON_HIGHLIGHT`] (`hoveredShipMmsi`; the pointer
//!    does not highlight ship icons: the layer sets no `autoHighlight`). The icons' cells are in
//!    [`crate::atlas::ships`].
//!
//! Clocks: the ships' frames are timed on this machine's clock, so everything here takes the local
//! clock as whole Unix milliseconds (`Date.now()`), not the flight feed's.
//!
//! Left to other items: the models' meshes and their highlight (H3, from [`ShipsScene::models`]
//! and [`ShipsScene::hovered`]), the selection halo and ring and the pick records (H4; an icon
//! carries its ship's list [`ShipIcon::index`] and MMSI).

use std::collections::HashMap;

use rustradar_core::geo::{cos_angle, meters_per_pixel, visible_radius};
use rustradar_core::model::{Ship, f32_as_js};
use rustradar_core::ships::{ship_color, ship_icon, ship_icon_size};
use rustradar_core::trains::LatLon;

use crate::aircraft::ICON_HEADING_STEP_PX;
use crate::camera::{ViewState, viewport_for};
use crate::screen::screen_angle;
use crate::ship_models::{ShipClass, offset_east_north, ship_placement};
use crate::trains::{Ease, Way, ease_from, eased};

/// From this zoom ships under way (heading known) are drawn as 3D models; it is above
/// `FLAT_ABOVE_ZOOM`, so always in the flat projection (G:1596).
pub const SHIP_MESH_ZOOM: f64 = 12.5;
/// ...each once it would be this many pixels long; smaller ones stay icons until then (G:1598).
pub const SHIP_MESH_MIN_PX: f64 = 14.0;
/// A ship's alpha as drawn (G:1599).
pub const SHIP_ALPHA: u8 = 235;
/// Old positions are drawn faded (G:1601).
pub const SHIP_OLD_ALPHA: u8 = 105;
/// The icon of the ship hovered in the sidebar's list (`highlightColor`, G:1860).
pub const ICON_HIGHLIGHT: [u8; 4] = [55, 142, 240, 200];

/// A ship as the easing sees it: its reported position and heading (as JavaScript reads it) and
/// its frames.
pub fn way(ship: &Ship) -> Way<'_> {
    Way { lat: ship.lat, lon: ship.lon, heading: ship.heading.map(f32_as_js), frames: &ship.frames }
}

/// One ship that can be drawn as a model: where, how big and which way, worked out once per list
/// (`ShipModel`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShipModel {
    /// The ship's index in the list.
    pub index: usize,
    /// Where it is drawn, `[lon, lat, 0]` (the icon's position: it moves with it).
    pub position: [f64; 3],
    /// Metres per model unit across, along and up ([`crate::ship_models::ShipPlacement::scale`]).
    pub scale: [f64; 3],
    /// Its reported heading, degrees clockwise from north.
    pub heading: f64,
    /// The hull's centre from the reported position, metres east and north.
    pub east: f64,
    pub north: f64,
    /// The zoom from which it is long enough on screen to be a model:
    /// `log2(metersPerPixel(0, lat) · SHIP_MESH_MIN_PX / length)`.
    pub min_zoom: f64,
}

/// The models of one class (`ship-mesh-<class>`), in the list's order.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelGroup {
    pub cls: ShipClass,
    pub models: Vec<ShipModel>,
}

/// Close in: which models are shown and which ships are left as icons, for one quarter-zoom step.
#[derive(Debug, Clone, PartialEq)]
struct Split {
    step: f64,
    /// Per class with any shown, in the groups' order: the indices into that group's models.
    models: Vec<(usize, Vec<usize>)>,
    /// The ships left as icons, in the list's order.
    icons: Vec<usize>,
}

/// The ships of one list as drawn (`ShipData`), in the list's order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawn {
    /// `[lon, lat, 0]`, stepped on at each refresh for the moving ships.
    pub positions: Vec<[f64; 3]>,
    /// Each ship's atlas cell, [`ship_icon`].
    pub icons: Vec<String>,
    /// The type colour at [`SHIP_ALPHA`] or [`SHIP_OLD_ALPHA`].
    pub colors: Vec<[u8; 4]>,
    /// The ships that can be drawn as a model, by class, in the order each class first appears
    /// (their positions as the list came in; [`Self::positions`] has where they are now).
    models: Vec<ModelGroup>,
    /// The last split worked out for this list.
    split: Option<Split>,
    /// Index by MMSI (the last of a repeated MMSI).
    by_mmsi: HashMap<u32, usize>,
    /// The ships under way, or easing onto a new report: stepped at every refresh.
    moving: Vec<usize>,
    /// Ships easing from where they were drawn onto their new report.
    blends: Vec<Option<Ease>>,
}

impl Drawn {
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Where the ship with `mmsi` is in the list (`byMmsi`).
    pub fn index_of(&self, mmsi: u32) -> Option<usize> {
        self.by_mmsi.get(&mmsi).copied()
    }

    /// Whether the ship at `i` moves at each refresh.
    pub fn moving(&self, i: usize) -> bool {
        self.moving.contains(&i)
    }

    /// The easing of the ship at `i`, if it has one.
    pub fn blend(&self, i: usize) -> Option<&Ease> {
        self.blends.get(i).and_then(Option::as_ref)
    }

    /// Every ship that can be a model, by class, at the place it is drawn now.
    pub fn models(&self) -> Vec<ModelGroup> {
        self.models
            .iter()
            .map(|g| ModelGroup { cls: g.cls, models: g.models.iter().map(|m| self.placed(m)).collect() })
            .collect()
    }

    /// `m` at the place its ship is drawn now.
    fn placed(&self, m: &ShipModel) -> ShipModel {
        ShipModel { position: self.positions[m.index], ..*m }
    }
}

/// Which ships are models and which icons at `zoom` (`shipSplit`): in quarter steps.
pub fn split_step(zoom: f64) -> f64 {
    (zoom * 4.0).floor() / 4.0
}

/// One ship icon (an instance of `ship-icons`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShipIcon {
    /// The ship's index in the list, and its MMSI (for the pick records).
    pub index: usize,
    pub mmsi: u32,
    /// Where the ship is drawn, `[lon, lat, 0]`.
    pub position: [f64; 3],
    /// Its cell in the atlas: `hull-<key>` with a heading, `dot-<key>` without.
    pub name: String,
    /// The cell's height on the screen, px (a billboard).
    pub size_px: f64,
    /// Degrees anticlockwise (deck's `getAngle`): the heading as seen on the screen.
    pub angle: f64,
    /// Its colour; the atlas holds the colours already, so only the alpha fades it.
    pub color: [u8; 4],
    /// The ship hovered in the sidebar's list: drawn with [`ICON_HIGHLIGHT`].
    pub highlighted: bool,
}

/// What the ship layers draw at one scene refresh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShipsScene {
    /// `ship-icons`: every ship below [`SHIP_MESH_ZOOM`], else the ships not drawn as models.
    pub icons: Vec<ShipIcon>,
    /// From [`SHIP_MESH_ZOOM`] on: the ships drawn as models, one group per class with any
    /// (item H3 draws them).
    pub models: Vec<ModelGroup>,
    /// The index in the list of the ship hovered in the sidebar's list (`hoveredShipIndex`), if
    /// it is on the map: its icon is highlighted here, its model by H3.
    pub hovered: Option<usize>,
}

/// The icons' angles on the screen, kept while neither the camera nor the list changes
/// (`shipAngles`).
#[derive(Debug, Clone, Default)]
struct Angles {
    /// `zoom, bearing, pitch, longitude, latitude` and the list they were worked out for.
    key: Option<([f64; 5], u64)>,
    /// One per ship (a `Float32Array`).
    angles: Vec<f32>,
}

/// The ships' part of the scene refresh: the drawn positions, the split and the angles kept
/// between refreshes.
#[derive(Debug, Clone, Default)]
pub struct ShipBuilder {
    drawn: Drawn,
    /// Counts the lists taken in (the list object's identity in `shipAngles.list !== list`).
    list: u64,
    angles: Angles,
}

impl ShipBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in a new list of ships at `now_ms` (`shipsFor` for a new list object): each at its
    /// reported position, eased from where the list before drew a ship of the same MMSI, with its
    /// icon, colour and model entry.
    pub fn set_list(&mut self, list: &[Ship], now_ms: i64) {
        let old = std::mem::take(&mut self.drawn);
        let mut data = Drawn::default();
        for (i, ship) in list.iter().enumerate() {
            let [r, g, b] = ship_color(ship.kind);
            let color = [r, g, b, if ship.old { SHIP_OLD_ALPHA } else { SHIP_ALPHA }];
            // a new report moves the ship on from where it is drawn, not with a jump
            let was = old.index_of(ship.mmsi).map(|w| [old.positions[w][0], old.positions[w][1]]);
            let blend = ease_from(was, way(ship), now_ms);
            data.by_mmsi.insert(ship.mmsi, i);
            if ship.frames.len() > 1 || blend.is_some() {
                data.moving.push(i);
            }
            data.blends.push(blend);
            let position = [ship.lon, ship.lat, 0.0];
            data.positions.push(position);
            data.icons.push(ship_icon(ship));
            data.colors.push(color);
            // a moored ship reports no heading: no telling which way to draw it
            let Some(heading) = ship.heading.map(f32_as_js) else { continue };
            let place = ship_placement(ship);
            let [east, north] = offset_east_north(place.right, place.ahead, heading);
            let model = ShipModel {
                index: i,
                position,
                scale: place.scale,
                heading,
                east,
                north,
                min_zoom: ((meters_per_pixel(0.0, ship.lat) * SHIP_MESH_MIN_PX) / place.length).log2(),
            };
            match data.models.iter_mut().find(|g| g.cls == place.cls) {
                Some(group) => group.models.push(model),
                None => data.models.push(ModelGroup { cls: place.cls, models: vec![model] }),
            }
        }
        self.drawn = data;
        self.list += 1;
    }

    /// Steps the moving ships on to `now_ms` (`moveShips`); the rest stay at their reported
    /// positions. `list` is the list last given to [`Self::set_list`].
    pub fn step(&mut self, list: &[Ship], now_ms: i64) {
        let data = &mut self.drawn;
        for &i in &data.moving {
            let Some(ship) = list.get(i) else { continue };
            let LatLon { lat, lon } = eased(way(ship), &mut data.blends[i], now_ms);
            data.positions[i][0] = lon;
            data.positions[i][1] = lat;
        }
    }

    /// The split at `zoom` (`shipSplit`), worked out again only when its quarter-zoom step
    /// changes.
    fn split(&mut self, zoom: f64) -> &Split {
        let step = split_step(zoom);
        let data = &mut self.drawn;
        if data.split.as_ref().is_none_or(|s| s.step != step) {
            let mut shown = vec![false; data.len()];
            let mut models = Vec::new();
            for (g, group) in data.models.iter().enumerate() {
                let big: Vec<usize> = (0..group.models.len()).filter(|&k| step >= group.models[k].min_zoom).collect();
                for &k in &big {
                    shown[group.models[k].index] = true;
                }
                if !big.is_empty() {
                    models.push((g, big));
                }
            }
            let icons = (0..data.len()).filter(|&i| !shown[i]).collect();
            data.split = Some(Split { step, models, icons });
        }
        data.split.as_ref().expect("the split")
    }

    /// The icons' angles in `view` on a canvas of `size` CSS px, worked out again when the camera
    /// or the list has changed (`shipAngles`).
    fn angles(&mut self, list: &[Ship], view: &ViewState, size: [f64; 2]) -> &[f32] {
        let ViewState { longitude, latitude, zoom, bearing, pitch, .. } = *view;
        let key = ([zoom, bearing, pitch, longitude, latitude], self.list);
        if self.angles.key != Some(key) {
            // the flat projection's viewport above zoom 12, as deck draws it
            let viewport = viewport_for(view, size);
            let step = ICON_HEADING_STEP_PX * meters_per_pixel(zoom, latitude);
            let mut angles = vec![0f32; list.len()];
            let min_cos = (visible_radius(zoom, pitch, size[0], size[1]) * std::f64::consts::PI / 180.0).cos();
            for (i, ship) in list.iter().enumerate() {
                let Some(heading) = ship.heading.map(f32_as_js) else { continue };
                if cos_angle(ship.lon, ship.lat, longitude, latitude) <= min_cos {
                    continue;
                }
                let Some(&position) = self.drawn.positions.get(i) else { continue };
                angles[i] = screen_angle(&viewport, position, heading, step).unwrap_or(bearing - heading) as f32;
            }
            self.angles = Angles { key: Some(key), angles };
        }
        &self.angles.angles
    }

    /// The ships of `list` at `now_ms` (local whole Unix ms) in `view` on a canvas of `size` CSS
    /// px (`shipLayers`): `new_list` when the list is not the one of the last refresh (it is taken
    /// in with [`Self::set_list`]); then the moving ships stepped on, the models close in and the
    /// icons, the ship with MMSI `hovered` (in the sidebar's list) highlighted.
    pub fn build(&mut self, list: &[Ship], new_list: bool, view: &ViewState, size: [f64; 2], hovered: Option<u32>, now_ms: i64) -> ShipsScene {
        if new_list {
            self.set_list(list, now_ms);
        }
        if list.is_empty() {
            return ShipsScene::default();
        }
        self.step(list, now_ms);
        let zoom = view.zoom;
        // `hoveredShipIndex`: the first ship of that MMSI
        let hovered = hovered.and_then(|mmsi| list.iter().position(|s| s.mmsi == mmsi));
        let (models, icon_data) = if zoom >= SHIP_MESH_ZOOM {
            let split = self.split(zoom).clone();
            let data = &self.drawn;
            let models = split
                .models
                .iter()
                .map(|(g, shown)| {
                    let group = &data.models[*g];
                    ModelGroup { cls: group.cls, models: shown.iter().map(|&k| data.placed(&group.models[k])).collect() }
                })
                .collect();
            (models, split.icons)
        } else {
            (Vec::new(), (0..self.drawn.len()).collect())
        };
        let angles = self.angles(list, view, size).to_vec();
        let data = &self.drawn;
        let icons = icon_data
            .into_iter()
            .filter(|&i| i < list.len())
            .map(|i| ShipIcon {
                index: i,
                mmsi: list[i].mmsi,
                position: data.positions[i],
                name: data.icons[i].clone(),
                size_px: ship_icon_size(&list[i], zoom),
                angle: f64::from(angles.get(i).copied().unwrap_or(0.0)),
                color: data.colors[i],
                highlighted: hovered == Some(i),
            })
            .collect();
        ShipsScene { icons, models, hovered }
    }

    /// Where the ships are drawn (for the models, the ring and the camera's lock).
    pub fn drawn(&self) -> &Drawn {
        &self.drawn
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::ships::ship_icon_scale;

    use super::*;
    use crate::camera::FLAT_ABOVE_ZOOM;
    use crate::ship_models::{ShipClass, ship_class_of};

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;
    /// Gothenburg harbour (V6).
    const HARBOUR: (f64, f64) = (57.70, 11.93);
    const SIZE: [f64; 2] = [1440.0, 900.0];
    const M_PER_DEG: f64 = 111_320.0;

    fn view(zoom: f64) -> ViewState {
        ViewState { longitude: HARBOUR.1, latitude: HARBOUR.0, zoom, bearing: 0.0, pitch: 0.0, position: None }
    }

    /// `(lat, lon)` `m` metres from `at` going `heading` (flat metres).
    fn ahead((lat, lon): (f64, f64), heading: f64, m: f64) -> (f64, f64) {
        let h = heading.to_radians();
        let k = lat.to_radians().cos();
        (lat + h.cos() * m / M_PER_DEG, lon + h.sin() * m / (M_PER_DEG * k))
    }

    fn ship(mmsi: u32, at: (f64, f64), kind: u8, heading: Option<f32>) -> Ship {
        Ship { mmsi, lat: at.0, lon: at.1, name: format!("SHIP {mmsi}"), kind, size: 1, heading, ..Ship::default() }
    }

    /// A ship at `from` going `heading` at `speed` m/s, with frames every 10 s from `t0` for two
    /// minutes.
    fn under_way(mmsi: u32, from: (f64, f64), heading: f32, speed: f64, t0: i64) -> Ship {
        let mut s = ship(mmsi, from, 4, Some(heading));
        s.frames = (0..=12)
            .map(|k| {
                let (lat, lon) = ahead(from, f64::from(heading), speed * 10.0 * f64::from(k));
                (lat, lon, t0 + i64::from(k) * 10_000)
            })
            .collect();
        s
    }

    /// Flat metres from `a` to `b`, `[lon, lat, _]`.
    fn metres(a: [f64; 3], b: [f64; 3]) -> f64 {
        ((b[1] - a[1]) * M_PER_DEG).hypot((b[0] - a[0]) * M_PER_DEG * a[1].to_radians().cos())
    }

    #[test]
    fn a_list_comes_in_with_its_colours_icons_and_models() {
        let mut old = ship(3, HARBOUR, 6, Some(45.0));
        old.old = true;
        let list = [ship(1, HARBOUR, 3, Some(90.0)), ship(2, HARBOUR, 5, None), old, ship(4, HARBOUR, 42, Some(0.0)), ship(5, HARBOUR, 3, Some(270.0))];
        let mut b = ShipBuilder::new();
        b.set_list(&list, NOW);
        let d = b.drawn();
        assert_eq!(d.len(), 5);
        assert_eq!(d.icons, ["hull-3", "dot-5", "hull-6", "hull-0", "hull-3"]);
        assert_eq!(d.colors, [[98, 152, 240, 235], [236, 128, 156, 235], [218, 92, 92, 105], [140, 152, 168, 235], [98, 152, 240, 235]]);
        assert_eq!(d.positions[1], [HARBOUR.1, HARBOUR.0, 0.0]);
        assert_eq!(d.index_of(4), Some(3));
        assert_eq!(d.index_of(9), None);
        assert!((0..5).all(|i| !d.moving(i) && d.blend(i).is_none()), "nothing moves, nothing eases");
        // models for the ships with a heading only, grouped by class in order of first sight:
        // the passenger ships (1, 5), the tanker, the unlisted kind as generic
        let groups = d.models();
        let classes: Vec<ShipClass> = groups.iter().map(|g| g.cls).collect();
        assert_eq!(classes, [ShipClass::Passenger, ShipClass::Tanker, ShipClass::Generic]);
        assert_eq!(groups[0].models.iter().map(|m| m.index).collect::<Vec<_>>(), [0, 4]);
        assert_eq!(groups[1].models[0].index, 2);
        assert_eq!(groups[2].models[0].index, 3);
        // the medium passenger ship is 110 m: 14 px long where the world's 512 px at zoom 0 are
        // 110 m · 512 / 14 round at this latitude
        let m = groups[0].models[0];
        let place = ship_placement(&list[0]);
        assert_eq!(place.length, 110.0);
        let want = (40_075_016.686 * HARBOUR.0.to_radians().cos() / 512.0 * 14.0 / 110.0).log2();
        assert!((m.min_zoom - want).abs() < 1e-12, "{} vs {want}", m.min_zoom);
        assert_eq!((m.heading, m.scale), (90.0, place.scale));
        // without dimensions the hull is centred on the reported position
        assert_eq!((m.east, m.north), (0.0, 0.0));
    }

    #[test]
    fn a_model_is_offset_so_its_reference_point_lands_on_the_report() {
        // 200 m long, reference point 150 m from the bow and 4 m from port of a 20 m beam: the
        // hull's centre is 50 m ahead and 6 m to starboard; heading east, that is 50 m east and 6 m
        // south
        let mut s = ship(1, HARBOUR, 4, Some(90.0));
        s.dims = Some([150, 50, 4, 16]);
        let mut b = ShipBuilder::new();
        b.set_list(&[s], NOW);
        let m = b.drawn().models()[0].models[0];
        assert!((m.east - 50.0).abs() < 1e-9 && (m.north + 6.0).abs() < 1e-9, "{} {}", m.east, m.north);
        assert!((m.min_zoom - (40_075_016.686 * HARBOUR.0.to_radians().cos() / 512.0 * 14.0 / 200.0).log2()).abs() < 1e-12);
    }

    #[test]
    fn a_new_report_eases_on_and_one_ahead_holds() {
        let first = [ship(1, HARBOUR, 4, Some(90.0)), ship(2, HARBOUR, 4, Some(90.0)), ship(3, HARBOUR, 4, Some(90.0))];
        let mut b = ShipBuilder::new();
        b.set_list(&first, NOW);
        // the next answer: ship 1 reported 40 m on east (drawn behind it: eases on), ship 2 40 m
        // back west (drawn ahead of it going east: holds), ship 3 jumped 0.2° (no easing), ship 4
        // new
        let next = [
            ship(1, ahead(HARBOUR, 90.0, 40.0), 4, Some(90.0)),
            ship(2, ahead(HARBOUR, 90.0, -40.0), 4, Some(90.0)),
            ship(3, (HARBOUR.0, HARBOUR.1 + 0.2), 4, Some(90.0)),
            ship(4, HARBOUR, 4, Some(90.0)),
        ];
        b.set_list(&next, NOW + 5000);
        let d = b.drawn();
        assert!(d.moving(0) && d.moving(1), "both have a blend to work through");
        assert!(!d.moving(2) && !d.moving(3), "a jump and a new ship are drawn at their reports");
        assert_eq!(d.blend(1).unwrap().hold.map(|h| h.heading), Some(90.0));
        // at first ship 1 is still where it was drawn; half way through the 2 s it is half way
        // (the smoothstep's middle); at the end, at its report
        let at = |b: &ShipBuilder, i: usize| b.drawn().positions[i];
        b.step(&next, NOW + 5000);
        assert!(metres(at(&b, 0), [HARBOUR.1, HARBOUR.0, 0.0]) < 1e-6);
        b.step(&next, NOW + 6000);
        let gone = metres([HARBOUR.1, HARBOUR.0, 0.0], at(&b, 0));
        assert!((gone - 20.0).abs() < 0.01, "{gone}");
        b.step(&next, NOW + 7000);
        assert_eq!(at(&b, 0), [next[0].lon, next[0].lat, 0.0]);
        assert!(b.drawn().blend(0).is_none(), "done");
        // ship 2 never goes back: it holds where it was drawn while its report is behind it
        assert_eq!(at(&b, 1), [HARBOUR.1, HARBOUR.0, 0.0]);
        assert_eq!(at(&b, 2), [next[2].lon, next[2].lat, 0.0]);
        // an empty list forgets them: the same ships after it are not eased
        b.set_list(&[], NOW + 8000);
        b.set_list(&first, NOW + 9000);
        assert!((0..3).all(|i| b.drawn().blend(i).is_none()));
    }

    #[test]
    fn a_ship_under_way_runs_along_its_frames_and_stops_at_their_end() {
        let s = under_way(1, HARBOUR, 45.0, 5.0, NOW);
        let list = [s.clone(), ship(2, HARBOUR, 0, None)];
        let mut b = ShipBuilder::new();
        let scene = b.build(&list, true, &view(11.0), SIZE, None, NOW + 30_000);
        assert!(b.drawn().moving(0) && !b.drawn().moving(1));
        let start = [s.lon, s.lat, 0.0];
        assert!((metres(start, b.drawn().positions[0]) - 150.0).abs() < 0.05, "{}", metres(start, b.drawn().positions[0]));
        // its icon is where it is drawn
        assert_eq!(scene.icons[0].position, b.drawn().positions[0]);
        // never past its last frame
        b.build(&list, false, &view(11.0), SIZE, None, NOW + 600_000);
        assert!((metres(start, b.drawn().positions[0]) - 600.0).abs() < 0.1);
    }

    #[test]
    fn close_in_big_ships_are_models_in_quarter_zoom_steps() {
        // an 80 m cargo ship (length from its dims), a 24 m fishing boat, a moored ship
        let mut cargo = ship(1, HARBOUR, 4, Some(10.0));
        cargo.dims = Some([50, 30, 6, 6]);
        let mut fishing = ship(2, HARBOUR, 5, Some(200.0));
        fishing.size = 1;
        let moored = ship(3, HARBOUR, 6, None);
        let list = [cargo.clone(), fishing.clone(), moored];
        assert_eq!(ship_class_of(&cargo), ShipClass::Cargo);
        let mpp0 = 40_075_016.686 * HARBOUR.0.to_radians().cos() / 512.0;
        let cargo_from = (mpp0 * 14.0 / 80.0).log2();
        let fishing_from = (mpp0 * 14.0 / 24.0).log2();
        assert!(cargo_from > 12.75 && cargo_from < 12.9 && fishing_from > 14.5, "{cargo_from} {fishing_from}");
        let mut b = ShipBuilder::new();
        // below 12.5 (and on the globe, at 12) every ship is an icon and nothing is a model
        for zoom in [9.0, FLAT_ABOVE_ZOOM, 12.49] {
            let scene = b.build(&list, true, &view(zoom), SIZE, None, NOW);
            assert_eq!(scene.icons.iter().map(|i| i.index).collect::<Vec<_>>(), [0, 1, 2], "{zoom}");
            assert!(scene.models.is_empty());
        }
        // past the cargo ship's zoom but in the quarter step below it (12.75): still an icon
        for zoom in [12.5, cargo_from - 0.01, cargo_from + 0.01, 12.99] {
            let scene = b.build(&list, false, &view(zoom), SIZE, None, NOW);
            assert_eq!(scene.icons.len(), 3, "{zoom}");
            assert!(scene.models.is_empty());
        }
        // from the next quarter step it is a model, and not an icon
        for zoom in [13.0, 13.2] {
            let scene = b.build(&list, false, &view(zoom), SIZE, None, NOW);
            assert_eq!(scene.icons.iter().map(|i| i.index).collect::<Vec<_>>(), [1, 2], "{zoom}");
            assert_eq!(scene.models.len(), 1);
            assert_eq!(scene.models[0].cls, ShipClass::Cargo);
            assert_eq!(scene.models[0].models[0].index, 0);
            assert_eq!(scene.models[0].models[0].position, [HARBOUR.1, HARBOUR.0, 0.0]);
        }
        // close in, the fishing boat too; the moored ship never
        let scene = b.build(&list, false, &view(16.0), SIZE, None, NOW);
        assert_eq!(scene.icons.iter().map(|i| i.index).collect::<Vec<_>>(), [2]);
        assert_eq!(scene.models.iter().map(|g| g.cls).collect::<Vec<_>>(), [ShipClass::Cargo, ShipClass::Fishing]);
        assert_eq!(scene.icons[0].name, "dot-6");
    }

    #[test]
    fn the_models_move_with_their_ships() {
        let mut s = under_way(1, HARBOUR, 90.0, 5.0, NOW);
        s.dims = Some([150, 50, 10, 10]);
        let list = [s];
        let mut b = ShipBuilder::new();
        let scene = b.build(&list, true, &view(15.0), SIZE, None, NOW + 20_000);
        assert!(scene.icons.is_empty());
        let m = scene.models[0].models[0];
        assert_eq!(m.position, b.drawn().positions[0]);
        assert!((metres([list[0].lon, list[0].lat, 0.0], m.position) - 100.0).abs() < 0.05);
        let later = b.build(&list, false, &view(15.0), SIZE, None, NOW + 40_000);
        assert!((metres(m.position, later.models[0].models[0].position) - 100.0).abs() < 0.05);
    }

    #[test]
    fn icons_are_sized_by_ship_and_zoom_and_turned_to_their_headings() {
        let mut big = ship(1, HARBOUR, 4, Some(30.0));
        big.size = 2;
        let mut small = ship(2, ahead(HARBOUR, 90.0, 500.0), 3, Some(300.0));
        small.size = 0;
        let moored = ship(3, ahead(HARBOUR, 180.0, 500.0), 8, None);
        let list = [big, small, moored];
        let mut b = ShipBuilder::new();
        for zoom in [2.0, 4.0, 6.0, 9.0] {
            let scene = b.build(&list, zoom == 2.0, &view(zoom), SIZE, None, NOW);
            let k = ship_icon_scale(zoom);
            assert_eq!(scene.icons.iter().map(|i| i.size_px).collect::<Vec<_>>(), [22.0 * k, 15.0 * k, 18.0 * k * 0.62], "{zoom}");
        }
        // a flat, north-up view (12.4: all icons): the heading anticlockwise, all but exactly (the
        // step ahead is in flat metres); none for the dot
        let scene = b.build(&list, false, &view(12.4), SIZE, None, NOW);
        let angles: Vec<f64> = scene.icons.iter().map(|i| i.angle).collect();
        assert!((angles[0] + 30.0).abs() < 1e-3, "{angles:?}");
        assert!((angles[1] + 300.0).abs() < 0.05 || (angles[1] - 60.0).abs() < 0.05, "{angles:?}");
        assert_eq!(angles[2], 0.0);
        assert_eq!(scene.icons.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["hull-4", "hull-3", "dot-8"]);
        // the map turned 40°: at the centre the angle is the bearing less the heading
        let turned = ViewState { bearing: 40.0, ..view(11.0) };
        let scene = b.build(&list, false, &turned, SIZE, None, NOW);
        assert!((scene.icons[0].angle - (40.0 - 30.0)).abs() < 0.01, "{}", scene.icons[0].angle);
        // and tilted 30°: the way ahead foreshortened up the screen, so the angle opens out
        let tilted = ViewState { pitch: 30.0, ..turned };
        let scene = b.build(&list, false, &tilted, SIZE, None, NOW);
        let want = (10f64.to_radians().tan() / 30f64.to_radians().cos()).atan().to_degrees();
        assert!((scene.icons[0].angle - want).abs() < 0.2, "{} vs {want}", scene.icons[0].angle);
        // on the globe away from the centre the meridians lean: the angle follows them, not the
        // bearing
        let far = ViewState { longitude: HARBOUR.1 - 10.0, ..view(4.0) };
        let scene = b.build(&list, false, &far, SIZE, None, NOW);
        let lean = scene.icons[0].angle + 30.0;
        assert!(lean.abs() > 2.0 && lean.abs() < 15.0, "{}", scene.icons[0].angle);
        // f32, as the TypeScript's Float32Array keeps them
        assert_eq!(scene.icons[0].angle, f64::from(scene.icons[0].angle as f32));
    }

    #[test]
    fn a_ship_beyond_the_visible_radius_is_not_turned() {
        // on the far side of the globe from the view centre
        let list = [ship(1, (-HARBOUR.0, HARBOUR.1 + 180.0), 4, Some(90.0)), ship(2, HARBOUR, 4, Some(90.0))];
        let mut b = ShipBuilder::new();
        let scene = b.build(&list, true, &view(3.0), SIZE, None, NOW);
        assert_eq!(scene.icons[0].angle, 0.0);
        assert!((scene.icons[1].angle + 90.0).abs() < 1.0, "{}", scene.icons[1].angle);
    }

    #[test]
    fn angles_are_kept_while_only_the_ships_move() {
        let list = [under_way(1, HARBOUR, 0.0, 300.0, NOW)];
        let mut b = ShipBuilder::new();
        let far = ViewState { longitude: HARBOUR.1 - 20.0, ..view(3.0) };
        let first = b.build(&list, true, &far, SIZE, None, NOW).icons[0].clone();
        // two minutes on it has gone 36 km north: the angle stays as worked out, as today's
        let moved = b.build(&list, false, &far, SIZE, None, NOW + 120_000).icons[0].clone();
        assert_ne!(first.position, moved.position);
        assert_eq!(first.angle, moved.angle);
        // the camera moves a hair: worked out again where the ship is now
        let nudged = ViewState { longitude: far.longitude + 1e-9, ..far };
        let again = b.build(&list, false, &nudged, SIZE, None, NOW + 120_000).icons[0].clone();
        assert_ne!(first.angle, again.angle);
        // and so does a new list: the ship reported standing where it started (36 km is a jump:
        // no easing)
        let fresh = b.build(&[ship(1, HARBOUR, 4, Some(0.0))], true, &nudged, SIZE, None, NOW + 120_000).icons[0].clone();
        assert_eq!(fresh.position, [HARBOUR.1, HARBOUR.0, 0.0]);
        assert_ne!(again.angle, fresh.angle);
    }

    #[test]
    fn the_ship_hovered_in_the_list_is_highlighted() {
        let mut big = ship(2, HARBOUR, 4, Some(0.0));
        big.dims = Some([200, 100, 20, 20]);
        let list = [ship(1, HARBOUR, 4, Some(0.0)), big, ship(3, HARBOUR, 4, None)];
        let mut b = ShipBuilder::new();
        let scene = b.build(&list, true, &view(10.0), SIZE, Some(3), NOW);
        assert_eq!(scene.icons.iter().map(|i| i.highlighted).collect::<Vec<_>>(), [false, false, true]);
        assert_eq!(scene.hovered, Some(2));
        assert_eq!(scene.icons.iter().map(|i| i.mmsi).collect::<Vec<_>>(), [1, 2, 3]);
        // close in the hovered big ship is a model: no icon is highlighted, H3 lights its model
        let scene = b.build(&list, false, &view(14.0), SIZE, Some(2), NOW);
        assert_eq!(scene.hovered, Some(1));
        assert!(scene.icons.iter().all(|i| !i.highlighted));
        assert!(scene.models.iter().any(|g| g.models.iter().any(|m| m.index == 1)));
        // an MMSI not on the map highlights nothing
        let scene = b.build(&list, false, &view(10.0), SIZE, Some(99), NOW);
        assert_eq!(scene.hovered, None);
        assert!(scene.icons.iter().all(|i| !i.highlighted));
    }

    #[test]
    fn no_ships_no_icons() {
        let mut b = ShipBuilder::new();
        assert_eq!(b.build(&[], true, &view(13.0), SIZE, None, NOW), ShipsScene::default());
        assert!(b.drawn().is_empty());
    }

    #[test]
    fn headings_read_as_javascript_reads_the_report() {
        let list = [ship(1, HARBOUR, 4, Some(12.3))];
        let mut b = ShipBuilder::new();
        b.set_list(&list, NOW);
        assert_eq!(b.drawn().models()[0].models[0].heading, 12.3, "not 12.300000190734863");
    }
}
