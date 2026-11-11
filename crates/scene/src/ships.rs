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
//! **Models** (item H3; doc 02 §4.6, `shipMeshLayers`, G:1761-1812): each shown model of
//! [`ShipsScene::models`] carries what its instance needs: where its ship is drawn, its scale
//! (metres per model unit across, along, up), its heading (deck's orientation `[0, −heading, 0]` on
//! the flat map, where models are drawn), the hull's offset east and north (deck's translation),
//! its colour ([`SHIP_MESH_COLOR`], [`SHIP_MESH_OLD_COLOR`] for an old position: the mesh's own
//! colours, faded) and whether it is highlighted [`MESH_HIGHLIGHT`] (the ship hovered in the
//! sidebar's list, `highlightedObjectIndex`; under the pointer, [`ShipsScene::highlight`]). The
//! meshes are [`crate::ship_models`]'; the map's `layers::ship_models` draws them.
//!
//! **Pick records** (item H4; doc 02 §11, doc 03 §10.1; `pickedShip`, G:3364-3376).
//! [`ShipsScene::picks`] lists what deck's picking pass would draw of rows 9–11, in draw order,
//! each keyed by a [`ShipPick`] (the ship's place in the list drawn and its MMSI,
//! [`ShipsScene::mmsis`]; for a model, its layer and its place there, [`ModelPick`]): close in
//! every model as the box its class's mesh fills under the instance the map draws
//! ([`model_matrix`]), depth tested and written (`NO_CULL`); then every icon with a cell, depth
//! tested and not written (`SHIP_PARAMETERS`): a hull as the rectangle its glyph's ink covers
//! facing the screen, turned to its angle ([`HULL_ICON_HALF`], [`icon_shape`]), a dot as the disc
//! of its ink ([`DOT_ICON_RADIUS`]). The halo and the ring are not pickable (`pickedShip` leaves
//! out `ship-selection-*`). The model the pointer's pick lands on is highlighted
//! [`MESH_HIGHLIGHT`] ([`ShipsScene::highlight`], deck's `autoHighlight`), unless a ship is
//! hovered in the sidebar's list (its `highlightedObjectIndex` then overrides the pointer's); the
//! icons are not (no `autoHighlight` on `ship-icons`).
//!
//! **The selection halo and ring** (item H4; doc 02 §4.8, `shipSelectionLayers`, G:3291-3321):
//! [`ring_of`] the selected ship at its **reported** position (not where it is drawn), sized to
//! clear its icon zoomed out and its hull close in: `max(icon / 2 + 5, max(bow, stern) / mpp + 6)`
//! px at the view centre's metres per pixel; none while it is round the back of the globe. The
//! map draws the halo under the ships and the ring over them (`map::layers::ships`).
//!
//! Deviations in the pick records (as for the aircraft and the trains, [`crate::pick`]): a hull is
//! picked over its glyph's rectangle, a dot over its ink's disc and a model over its mesh's
//! bounding box, where deck picks the texels that pass the alpha cutoff and the mesh's triangles.

use std::collections::HashMap;
use std::sync::OnceLock;

use glam::{DMat4, DVec3};
use rustradar_core::geo::{cos_angle, meters_per_pixel, visible_radius};
use rustradar_core::model::{Ship, f32_as_js};
use rustradar_core::ships::{ship_color, ship_icon, ship_icon_size};
use rustradar_core::trains::LatLon;

use crate::aircraft::{ICON_HEADING_STEP_PX, instance_matrix, local_frame};
use crate::camera::{FLAT_ABOVE_ZOOM, ViewState, Viewport, viewport_for};
use crate::geom::js_max;
use crate::pick::{DepthTest, OrientedBox, PickList, Shape};
use crate::screen::screen_angle;
use crate::ship_models::{SHIP_CLASSES, ShipClass, offset_east_north, ship_meshes, ship_placement};
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
/// A model's colour: its mesh's own colours as they are (`SHIP_MESH_COLOR`, G:1602)...
pub const SHIP_MESH_COLOR: [u8; 4] = [255, 255, 255, 255];
/// ...faded for an old position (`SHIP_MESH_OLD_COLOR`, G:1603).
pub const SHIP_MESH_OLD_COLOR: [u8; 4] = [255, 255, 255, 120];
/// The model under the pointer or hovered in the sidebar's list (`highlightColor`, G:1803).
pub const MESH_HIGHLIGHT: [u8; 4] = [55, 142, 240, 110];

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
    /// [`SHIP_MESH_COLOR`], or [`SHIP_MESH_OLD_COLOR`] for an old position (`getColor`, item H3).
    pub color: [u8; 4],
    /// Drawn with [`MESH_HIGHLIGHT`]: the ship hovered in the sidebar's list
    /// (`highlightedObjectIndex`), or under the pointer ([`ShipsScene::highlight`]); set at each
    /// refresh.
    pub highlighted: bool,
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
    /// it is on the map: its icon or its model is highlighted.
    pub hovered: Option<usize>,
    /// The MMSIs of the ships drawn, in the list's order (item H4): a pick names the ship it lands
    /// on by its place in this list and its MMSI ([`ShipPick`]).
    pub mmsis: Vec<u32>,
}

impl ShipsScene {
    /// Highlights the model of the ship at list index `index`, if it is drawn as one: deck's
    /// `autoHighlight` on the model under the pointer (the icons have none). The app marks the
    /// model its pick landed on by its layer and place instead, as deck keeps it
    /// ([`Self::highlight`], item H4).
    pub fn hover_model(&mut self, index: usize) {
        for m in self.models.iter_mut().flat_map(|g| g.models.iter_mut()) {
            if m.index == index {
                m.highlighted = true;
            }
        }
    }
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
                // the colours are the mesh's own; old positions fade
                color: if ship.old { SHIP_MESH_OLD_COLOR } else { SHIP_MESH_COLOR },
                highlighted: false,
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
                    // a ship hovered in the sidebar's list lights up as if under the pointer
                    let model = |k: usize| ShipModel { highlighted: hovered == Some(group.models[k].index), ..data.placed(&group.models[k]) };
                    ModelGroup { cls: group.cls, models: shown.iter().map(|&k| model(k)).collect() }
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
        ShipsScene { icons, models, hovered, mmsis: list.iter().map(|s| s.mmsi).collect() }
    }

    /// Where the ships are drawn (for the models, the ring and the camera's lock).
    pub fn drawn(&self) -> &Drawn {
        &self.drawn
    }
}

// --- Pick records and the selection ring (item H4) -------------------------------------------

/// The ink of a hull icon round its cell's middle, cell texels: half its width and half its
/// length, the outer half of its border included ([`crate::atlas::ships`]: the outline spans x
/// 17–47 and y 4–59 inside a 5 px border with round joins, so 35 × 60).
pub const HULL_ICON_HALF: [f64; 2] = [17.5, 30.0];
/// The ink of a dot icon round its cell's middle, cell texels: its radius of 20 and the outer half
/// of its 8 px border.
pub const DOT_ICON_RADIUS: f64 = 24.0;
/// Px the ring stands clear of the icon's half size (`shipIconSize(ship, zoom) / 2 + 5`)...
pub const RING_ICON_CLEARANCE_PX: f64 = 5.0;
/// ...and of the hull's farther end from the reported position close in (`hullPx + 6`).
pub const RING_HULL_CLEARANCE_PX: f64 = 6.0;

/// A model under the pointer as deck's `autoHighlight` keeps it (the picked layer's
/// `highlightedObjectIndex`): its layer (`ship-mesh-<class>`) and its place among that layer's
/// models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelPick {
    pub cls: ShipClass,
    pub index: usize,
}

/// What a pick of the ship layers lands on (`pickedShip`, G:3365-3376): the ship's place in the
/// list drawn (`ship-icons`' object, or the picked model's [`ShipModel::index`]) with its MMSI
/// ([`ShipsScene::mmsis`]), and for a model, which one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShipPick {
    pub ship: usize,
    pub mmsi: u32,
    pub model: Option<ModelPick>,
}

impl ShipsScene {
    /// The ships' pick records in `viewport`, in draw order (rows 9–11; the models and the icons
    /// are `pickable`, the halo and the ring are not):
    /// - close in, each class's models (the groups' order, then theirs) as the boxes their meshes
    ///   fill under the instances the map draws ([`model_matrix`]), depth tested and written
    ///   (`NO_CULL`);
    /// - then each icon with a cell ([`icon_shape`]), depth tested and not written
    ///   (`SHIP_PARAMETERS`).
    pub fn picks(&self, viewport: &Viewport) -> PickList<ShipPick> {
        let mut list = PickList::new();
        let mmsi = |i: usize| self.mmsis.get(i).copied().unwrap_or_default();
        for group in &self.models {
            let Some([min, max]) = model_bounds(group.cls) else { continue };
            for (index, m) in group.models.iter().enumerate() {
                let key = ShipPick { ship: m.index, mmsi: mmsi(m.index), model: Some(ModelPick { cls: group.cls, index }) };
                list.oriented_box(key, OrientedBox::from_matrix(&model_matrix(viewport, m), min, max), DepthTest::LessEqualWrite);
            }
        }
        for icon in &self.icons {
            if let Some(shape) = icon_shape(viewport, icon) {
                list.push(ShipPick { ship: icon.index, mmsi: icon.mmsi, model: None }, shape, DepthTest::LessEqual);
            }
        }
        list
    }

    /// Marks the model `pick` names as under the pointer, as deck's `autoHighlight` does: the
    /// model at that place of that class's models in this refresh; nothing when there is no such
    /// layer or model, or while a ship is hovered in the sidebar's list (`highlightedObjectIndex`
    /// is then set on every model layer, which turns their `autoHighlight` off).
    pub fn highlight(&mut self, pick: &ModelPick) {
        if self.hovered.is_some() {
            return;
        }
        if let Some(m) = self.models.iter_mut().find(|g| g.cls == pick.cls).and_then(|g| g.models.get_mut(pick.index)) {
            m.highlighted = true;
        }
    }
}

/// A model's matrix in `viewport`'s common space, as the map draws it
/// (`map::layers::ship_models::instance`): its class's unit-long mesh turned to the heading
/// (`getOrientation: [0, −heading, 0]`), stretched to the ship (`getScale`), at the ship's drawn
/// place, moved by the hull's offset east and north (`getTranslation`, metres in the frame there).
pub fn model_matrix(viewport: &Viewport, m: &ShipModel) -> DMat4 {
    let mut model = instance_matrix(viewport, m.position, [0.0, -m.heading, 0.0], m.scale);
    let offset = local_frame(viewport, [m.position[0], m.position[1]]) * DVec3::new(m.east, m.north, 0.0);
    model.w_axis += offset.extend(0.0);
    model
}

/// Each class's mesh's bounds in model space, `[min, max]` (none for a mesh without vertices).
fn model_bounds(cls: ShipClass) -> Option<[DVec3; 2]> {
    static BOUNDS: OnceLock<Vec<Option<[DVec3; 2]>>> = OnceLock::new();
    let bounds = BOUNDS.get_or_init(|| {
        SHIP_CLASSES
            .iter()
            .map(|&c| {
                let mesh = &ship_meshes()[c as usize];
                let mut points = mesh.positions.chunks_exact(3).map(|p| DVec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])));
                let first = points.next()?;
                Some(points.fold([first, first], |[lo, hi], p| [lo.min(p), hi.max(p)]))
            })
            .collect()
    });
    bounds.get(cls as usize).copied().flatten()
}

/// What an icon covers on the screen as the icon pipeline draws it (`billboard: true`): its cell
/// [`ShipIcon::size_px`] tall, facing the screen at its anchor, turned anticlockwise by its angle;
/// of the cell, the glyph's ink. A hull ([`HULL_ICON_HALF`]) is that rectangle, standing in the
/// plane through the anchor that faces the camera; a dot ([`DOT_ICON_RADIUS`]) is a disc round the
/// anchor. Both are deck's billboard size: the pixels at the view centre's depth, nearer anchors
/// larger and farther ones smaller under pitch (`project_pixel_size_to_clipspace`). `None` for an
/// icon the atlas has no cell for (deck draws nothing for it).
pub fn icon_shape(viewport: &Viewport, icon: &ShipIcon) -> Option<Shape> {
    let cell = crate::atlas::ships::cell(&icon.name)?;
    // common units per texel: the size over the cell's height, at the view centre's scale
    let k = js_max(icon.size_px, 0.0) / f64::from(cell.height) / viewport.scale;
    // the anchor as the map places it (the world copy nearest the view centre on the flat map)
    let center = instance_matrix(viewport, icon.position, [0.0; 3], [1.0; 3]).w_axis.truncate();
    let [x, y, depth] = viewport.world_to_pixels(center);
    let at = viewport.pixels_to_world_with_depth([x, y, depth]);
    // the screen's right and down at the anchor, in common space
    let right = (viewport.pixels_to_world_with_depth([x + 1.0, y, depth]) - at).normalize_or_zero();
    let down = (viewport.pixels_to_world_with_depth([x, y + 1.0, depth]) - at).normalize_or_zero();
    if icon.name.starts_with("dot-") {
        let [ex, ey, _] = viewport.world_to_pixels(center + right * (DOT_ICON_RADIUS * k));
        return Some(Shape::Point { at: [x, y, depth], radius: (ex - x).hypot(ey - y) });
    }
    // the cell's up (the bow) turned anticlockwise on the screen by the angle
    let (sin, cos) = icon.angle.to_radians().sin_cos();
    let along = -(right * sin) - down * cos;
    let across = right * cos - down * sin;
    let facing = across.cross(along).normalize_or_zero();
    Some(Shape::Box(OrientedBox { center, axes: [across, along, facing], half: [HULL_ICON_HALF[0] * k, HULL_ICON_HALF[1] * k, 0.0] }))
}

/// The selected ship's halo and ring (`shipSelectionLayers`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShipRing {
    /// The ship's reported `[lon, lat]` (at height 0; not where it is drawn).
    pub at: [f64; 2],
    /// `max(shipIconSize(ship, zoom) / 2 + 5, hullPx + 6)`, px.
    pub radius_px: f64,
}

/// `shipSelectionLayers(ship, mpp, zoom, cLon, cLat)` (G:3296-3321): the halo and ring round
/// `ship`'s report in `view`, sized to clear its icon zoomed out and its true-scale hull close in
/// (`hullPx` is the farther of its bow and stern from the reference point, at the view centre's
/// metres per pixel); none on the globe while it is round the back (`cosAngle` to the view centre
/// `<= 0`).
pub fn ring_of(ship: &Ship, view: &ViewState) -> Option<ShipRing> {
    if view.zoom <= FLAT_ABOVE_ZOOM && cos_angle(ship.lon, ship.lat, view.longitude, view.latitude) <= 0.0 {
        return None;
    }
    let mpp = meters_per_pixel(view.zoom, view.latitude);
    let hull_px = ship.dims.map_or(0.0, |d| f64::from(d[0].max(d[1])) / mpp);
    let radius_px = js_max(ship_icon_size(ship, view.zoom) / 2.0 + RING_ICON_CLEARANCE_PX, hull_px + RING_HULL_CLEARANCE_PX);
    Some(ShipRing { at: [ship.lon, ship.lat], radius_px })
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

    /// Close in, models in white (the mesh's own colours) or faded white for an old position; the
    /// ship hovered in the list, or under the pointer, highlighted; nothing else.
    #[test]
    fn models_are_white_faded_when_old_and_highlighted_when_hovered() {
        let big = |mmsi, kind, old| {
            let mut s = ship(mmsi, ahead(HARBOUR, 90.0, 300.0 * f64::from(mmsi)), kind, Some(45.0));
            s.dims = Some([60, 60, 8, 8]);
            s.old = old;
            s
        };
        let list = [big(1, 4, false), big(2, 4, true), big(3, 6, false), ship(4, HARBOUR, 6, None)];
        let mut b = ShipBuilder::new();
        let scene = b.build(&list, true, &view(15.0), SIZE, Some(2), NOW);
        let models: Vec<&ShipModel> = scene.models.iter().flat_map(|g| &g.models).collect();
        assert_eq!(models.iter().map(|m| m.index).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(models.iter().map(|m| m.color).collect::<Vec<_>>(), [SHIP_MESH_COLOR, SHIP_MESH_OLD_COLOR, SHIP_MESH_COLOR]);
        assert_eq!(models.iter().map(|m| m.highlighted).collect::<Vec<_>>(), [false, true, false]);
        assert_eq!((SHIP_MESH_COLOR, SHIP_MESH_OLD_COLOR, MESH_HIGHLIGHT), ([255; 4], [255, 255, 255, 120], [55, 142, 240, 110]));
        // the old one's icon would be faded too, but it is a model: only the moored ship is an icon
        assert_eq!(scene.icons.iter().map(|i| i.index).collect::<Vec<_>>(), [3]);
        // the hover moves on: the highlight follows it, the kept split is not marked
        let mut scene = b.build(&list, false, &view(15.0), SIZE, None, NOW);
        assert!(scene.models.iter().flat_map(|g| &g.models).all(|m| !m.highlighted));
        // under the pointer
        scene.hover_model(2);
        scene.hover_model(3);
        let lit: Vec<usize> = scene.models.iter().flat_map(|g| &g.models).filter(|m| m.highlighted).map(|m| m.index).collect();
        assert_eq!(lit, [2]);
        assert!(scene.icons.iter().all(|i| !i.highlighted), "a moored ship's icon is not lit by the pointer");
    }

    // --- Pick records and the ring (H4) ---

    fn view_at(zoom: f64, bearing: f64, pitch: f64) -> ViewState {
        ViewState { bearing, pitch, ..view(zoom) }
    }

    /// What the scene picks at `at` (CSS px) in `view`: the pick and the distance.
    fn picked(scene: &ShipsScene, view: &ViewState, at: [f64; 2]) -> Option<(ShipPick, f64)> {
        let viewport = viewport_for(view, SIZE);
        scene.picks(&viewport).pick(&viewport, at, 1.0).map(|hit| (*hit.key, hit.distance_px))
    }

    /// The screen point `px` pixels from `from` along `dir`.
    fn step(from: [f64; 2], dir: [f64; 2], px: f64) -> [f64; 2] {
        [from[0] + dir[0] * px, from[1] + dir[1] * px]
    }

    /// The unit screen direction of `heading` at `at` in `view` (a short step along it, projected).
    fn screen_dir(view: &ViewState, at: (f64, f64), heading: f64) -> [f64; 2] {
        let viewport = viewport_for(view, SIZE);
        let [x0, y0, _] = viewport.project([at.1, at.0, 0.0]);
        let (lat, lon) = ahead(at, heading, 5.0);
        let [x1, y1, _] = viewport.project([lon, lat, 0.0]);
        let n = (x1 - x0).hypot(y1 - y0);
        [(x1 - x0) / n, (y1 - y0) / n]
    }

    #[test]
    fn the_icon_footprints_are_the_glyphs_ink() {
        let atlas = crate::atlas::ships::draw();
        // the texels deck keeps: alpha at least the cutoff, 0.05
        let kept = |x: u32, y: u32| f64::from(atlas.texel(x, y)[3]) >= 0.05 * 255.0;
        for key in rustradar_core::ships::SHIP_COLOR_KEYS {
            let c = crate::atlas::ships::cell(&format!("hull-{key}")).expect("a hull cell");
            let (mut lo, mut hi) = ([u32::MAX; 2], [0u32; 2]);
            for y in c.y..c.y + c.height {
                for x in c.x..c.x + c.width {
                    if kept(x, y) {
                        lo = [lo[0].min(x - c.x), lo[1].min(y - c.y)];
                        hi = [hi[0].max(x - c.x + 1), hi[1].max(y - c.y + 1)];
                    }
                }
            }
            let middle = f64::from(c.width) / 2.0;
            for axis in 0..2 {
                // the ink spans the footprint to within a texel of anti-aliasing either side
                let (from, to) = (f64::from(lo[axis]), f64::from(hi[axis]));
                let half = HULL_ICON_HALF[axis];
                assert!((from - (middle - half)).abs() <= 1.0 && (to - (middle + half)).abs() <= 1.0, "hull-{key} axis {axis}: ink {from}–{to}, footprint ±{half}");
            }
            let c = crate::atlas::ships::cell(&format!("dot-{key}")).expect("a dot cell");
            let mut reach: f64 = 0.0;
            for y in c.y..c.y + c.height {
                for x in c.x..c.x + c.width {
                    if kept(x, y) {
                        // the texel's far corner from the middle
                        let (dx, dy) = ((f64::from(x - c.x) + 0.5 - middle).abs() + 0.5, (f64::from(y - c.y) + 0.5 - middle).abs() + 0.5);
                        reach = reach.max(dx.hypot(dy));
                    }
                }
            }
            assert!((reach - DOT_ICON_RADIUS).abs() <= 1.5, "dot-{key}: ink to {reach:.2}, footprint {DOT_ICON_RADIUS}");
        }
    }

    #[test]
    fn a_hull_is_picked_over_its_glyph_facing_the_screen_along_its_heading() {
        // a small tug (an icon until zoom 15.8) at the view centre, on the globe and the flat map,
        // turned and tilted
        for (zoom, bearing, pitch, heading) in [(11.0, 0.0, 0.0, 0.0), (11.0, 30.0, 0.0, 120.0), (13.0, 0.0, 0.0, 75.0), (13.0, -40.0, 50.0, 200.0), (9.0, 10.0, 30.0, 300.0)] {
            let view = view_at(zoom, bearing, pitch);
            let mut s = ship(7, HARBOUR, 2, Some(heading));
            s.size = 0;
            let scene = ShipBuilder::new().build(std::slice::from_ref(&s), true, &view, SIZE, None, NOW);
            assert_eq!(scene.icons.len(), 1);
            let about = format!("zoom {zoom}, bearing {bearing}, pitch {pitch}, heading {heading}");
            let viewport = viewport_for(&view, SIZE);
            let picks = scene.picks(&viewport);
            assert_eq!(picks.len(), 1);
            assert_eq!(picks.items()[0].key, ShipPick { ship: 0, mmsi: 7, model: None });
            assert_eq!(picks.items()[0].depth_test, DepthTest::LessEqual, "SHIP_PARAMETERS: tested, not written");
            let [x, y, _] = viewport.project([HARBOUR.1, HARBOUR.0, 0.0]);
            let size = scene.icons[0].size_px;
            let (long, across) = (HULL_ICON_HALF[1] / 64.0 * size, HULL_ICON_HALF[0] / 64.0 * size);
            // along the heading as the screen shows it, and square to it
            let dir = screen_dir(&view, HARBOUR, f64::from(heading));
            let side = [-dir[1], dir[0]];
            let hit = Some((ShipPick { ship: 0, mmsi: 7, model: None }, 0.0));
            assert_eq!(picked(&scene, &view, [x, y]), hit, "{about}: on the anchor");
            for d in [-1.0, 1.0] {
                assert_eq!(picked(&scene, &view, step([x, y], dir, d * (long - 1.5))), hit, "{about}: {d} along");
                assert_eq!(picked(&scene, &view, step([x, y], side, d * (across - 1.5))), hit, "{about}: {d} across");
                // the 6 px of reach round the glyph, and nothing past it
                let near = picked(&scene, &view, step([x, y], dir, d * (long + 4.0))).expect("4 px past its end");
                // (the nearest pixel inside, from the pointer's pixel: a pixel's diagonal either way)
                assert!(near.1 > 2.5 && near.1 <= 5.5, "{about}: {near:?}");
                assert_eq!(picked(&scene, &view, step([x, y], dir, d * (long + 7.5))), None, "{about}: past its end");
                assert_eq!(picked(&scene, &view, step([x, y], side, d * (across + 7.5))), None, "{about}: past its side");
            }
        }
    }

    #[test]
    fn a_dot_is_picked_over_its_ink() {
        let view = view(11.0);
        let s = ship(8, HARBOUR, 8, None);
        let scene = ShipBuilder::new().build(std::slice::from_ref(&s), true, &view, SIZE, None, NOW);
        let [x, y, _] = viewport_for(&view, SIZE).project([HARBOUR.1, HARBOUR.0, 0.0]);
        let r = DOT_ICON_RADIUS / 64.0 * scene.icons[0].size_px;
        let hit = Some((ShipPick { ship: 0, mmsi: 8, model: None }, 0.0));
        for angle in [0.0_f64, 1.0, 2.5, 4.0] {
            let (sin, cos) = angle.sin_cos();
            assert_eq!(picked(&scene, &view, [x + cos * (r - 1.5), y + sin * (r - 1.5)]), hit, "{angle}");
            assert_eq!(picked(&scene, &view, [x + cos * (r + 7.5), y + sin * (r + 7.5)]), None);
        }
    }

    #[test]
    fn an_icon_the_atlas_lacks_is_not_drawn_so_not_picked() {
        let view = view(11.0);
        let mut scene = ShipBuilder::new().build(&[ship(1, HARBOUR, 4, Some(0.0))], true, &view, SIZE, None, NOW);
        scene.icons[0].name = "hull-9".into();
        assert!(scene.picks(&viewport_for(&view, SIZE)).is_empty());
    }

    #[test]
    fn a_later_ship_wins_where_two_icons_overlap() {
        let view = view(11.0);
        let list = [ship(1, HARBOUR, 4, Some(0.0)), ship(2, ahead(HARBOUR, 0.0, 150.0), 6, Some(0.0))];
        let scene = ShipBuilder::new().build(&list, true, &view, SIZE, None, NOW);
        let viewport = viewport_for(&view, SIZE);
        let [x, y, _] = viewport.project([HARBOUR.1, HARBOUR.0, 0.0]);
        let [_, y2, _] = viewport.project([list[1].lon, list[1].lat, 0.0]);
        assert!(y - y2 > 1.0 && y - y2 < 10.0, "a few px apart: {}", y - y2);
        let long = HULL_ICON_HALF[1] / 64.0 * scene.icons[0].size_px;
        assert_eq!(picked(&scene, &view, [x, (y + y2) / 2.0]).map(|p| p.0.mmsi), Some(2), "the later one on top");
        assert_eq!(picked(&scene, &view, [x, y + long - 1.0]).map(|p| p.0.mmsi), Some(1), "the first one's stern, off the second");
        assert_eq!(scene.mmsis, [1, 2]);
    }

    #[test]
    fn close_in_a_model_is_picked_over_its_mesh_box_before_the_icons() {
        // a 140 m cargo ship (bow 100 m, stern 40 m from the reference point) heading east: a model
        // at zoom 15; a moored boat beside it stays an icon
        let view = view(15.0);
        let mut cargo = ship(1, HARBOUR, 4, Some(90.0));
        cargo.dims = Some([100, 40, 10, 10]);
        let boat = ship(2, ahead(HARBOUR, 0.0, 120.0), 8, None);
        let scene = ShipBuilder::new().build(&[cargo.clone(), boat], true, &view, SIZE, None, NOW);
        assert_eq!(scene.models.len(), 1);
        let viewport = viewport_for(&view, SIZE);
        let picks = scene.picks(&viewport);
        let keys: Vec<(ShipPick, DepthTest)> = picks.items().iter().map(|i| (i.key, i.depth_test)).collect();
        let model = ShipPick { ship: 0, mmsi: 1, model: Some(ModelPick { cls: ShipClass::Cargo, index: 0 }) };
        assert_eq!(keys, [(model, DepthTest::LessEqualWrite), (ShipPick { ship: 1, mmsi: 2, model: None }, DepthTest::LessEqual)]);
        // over the hull from near its bow to near its stern, the hull's centre 30 m ahead of the
        // report; nothing 8 px past either end
        let mpp = viewport.meters_per_pixel();
        let at = |m: f64| {
            let (lat, lon) = ahead(HARBOUR, 90.0, m);
            let [x, y, _] = viewport.project([lon, lat, 0.0]);
            [x, y]
        };
        for m in [-38.0, 0.0, 30.0, 98.0] {
            assert_eq!(picked(&scene, &view, at(m)), Some((model, 0.0)), "{m} m ahead");
        }
        assert_eq!(picked(&scene, &view, at(100.0 + 8.0 * mpp)), None);
        assert_eq!(picked(&scene, &view, at(-40.0 - 8.0 * mpp)), None);
        // the box is the mesh's under the map's instance: as long as the hull, across its beam
        let b = match picks.items()[0].shape {
            Shape::Box(b) => b,
            ref other => panic!("{other:?}"),
        };
        let units = viewport.distance_scales.units_per_meter[2];
        assert!((b.half[1] * 2.0 / units - 140.0).abs() < 1.0, "{} m long", b.half[1] * 2.0 / units);
        assert!(b.half[0] * 2.0 / units > 19.0 && b.half[0] * 2.0 / units < 21.5, "{} m across", b.half[0] * 2.0 / units);
    }

    #[test]
    fn the_model_under_the_pointer_is_highlighted_unless_one_is_hovered_in_the_list() {
        let view = view(15.0);
        let big = |mmsi: u32, kind| {
            let mut s = ship(mmsi, ahead(HARBOUR, 90.0, 300.0 * f64::from(mmsi)), kind, Some(45.0));
            s.dims = Some([60, 60, 8, 8]);
            s
        };
        let list = [big(1, 4), big(2, 4), big(3, 6)];
        let mut b = ShipBuilder::new();
        let mut scene = b.build(&list, true, &view, SIZE, None, NOW);
        scene.highlight(&ModelPick { cls: ShipClass::Cargo, index: 1 });
        scene.highlight(&ModelPick { cls: ShipClass::Cargo, index: 5 });
        scene.highlight(&ModelPick { cls: ShipClass::Passenger, index: 0 });
        let lit: Vec<usize> = scene.models.iter().flat_map(|g| &g.models).filter(|m| m.highlighted).map(|m| m.index).collect();
        assert_eq!(lit, [1]);
        // a ship hovered in the list: only it is highlighted, the pointer's pick is not
        let mut scene = b.build(&list, false, &view, SIZE, Some(3), NOW);
        scene.highlight(&ModelPick { cls: ShipClass::Cargo, index: 0 });
        let lit: Vec<usize> = scene.models.iter().flat_map(|g| &g.models).filter(|m| m.highlighted).map(|m| m.index).collect();
        assert_eq!(lit, [2]);
        // a hovered MMSI not on the map leaves the pointer's highlight on
        let mut scene = b.build(&list, false, &view, SIZE, Some(99), NOW);
        scene.highlight(&ModelPick { cls: ShipClass::Cargo, index: 0 });
        let lit: Vec<usize> = scene.models.iter().flat_map(|g| &g.models).filter(|m| m.highlighted).map(|m| m.index).collect();
        assert_eq!(lit, [0]);
    }

    #[test]
    fn the_ring_clears_the_icon_far_out_and_the_hull_close_in_at_the_report() {
        let mut s = under_way(5, HARBOUR, 90.0, 5.0, NOW - 30_000);
        s.size = 2;
        s.dims = Some([100, 40, 10, 10]);
        // zoom 11: the icon, 22 px, clears it (100 m is 3.6 px)
        let ring = ring_of(&s, &view(11.0)).expect("a ring");
        assert_eq!(ring.at, [HARBOUR.1, HARBOUR.0], "at the report, not where the ship is drawn");
        assert_eq!(ring.radius_px, 22.0 / 2.0 + 5.0);
        // zoom 16: the bow's 100 m at the view centre's metres per pixel, not the 140 m length
        let mpp = meters_per_pixel(16.0, HARBOUR.0);
        assert_eq!(ring_of(&s, &view(16.0)).expect("a ring").radius_px, 100.0 / mpp + 6.0);
        // no dimensions: the icon's
        s.dims = None;
        assert_eq!(ring_of(&s, &view(16.0)).expect("a ring").radius_px, 22.0 / 2.0 + 5.0);
        // a moored ship's dot
        s.heading = None;
        s.size = 0;
        assert_eq!(ring_of(&s, &view(8.0)).expect("a ring").radius_px, 15.0 * 0.62 / 2.0 + 5.0);
        // round the back of the globe: none; on the flat map always
        let far = ViewState { longitude: HARBOUR.1 + 180.0, latitude: -HARBOUR.0, ..view(3.0) };
        assert_eq!(ring_of(&s, &far), None);
        let flat_far = ViewState { longitude: HARBOUR.1 + 1.0, ..view(13.0) };
        assert!(ring_of(&s, &flat_far).is_some());
        assert!(ring_of(&s, &ViewState { longitude: HARBOUR.1 + 60.0, ..view(12.0) }).is_some(), "the near side of the globe");
    }
}
