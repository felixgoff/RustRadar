//! Trains on the map, CPU side (item T2; doc 02 §5.1–5.5, §4.4; `Globe.svelte:1624-1684`,
//! `:1878-1931`, `:2031-2071`): where each train is drawn at each scene refresh, and the train
//! icons.
//!
//! **Motion** ([`TrainMotion`], `trainsFor` and `moveTrains`). A new list of trains (the train
//! state's `on_map`, item T1) is taken in once with [`TrainMotion::set_list`]: each train at its
//! reported position and heading, coloured by its operator or else its kind at
//! [`TRAIN_ALPHA`], at its level on the track ([`crate::bridges::level_at`]: in a tunnel, on a
//! bridge, or on the ground), and eased from where the list before drew it ([`ease_from`]). At
//! every scene refresh [`TrainMotion::step`] moves the trains that have frames to move along (or
//! an easing to finish): each is drawn at [`eased`] along its frames, heading the way its current
//! frame leg runs, and its level is looked up again. A train without frames stays at its reported
//! position (even with a single frame elsewhere).
//!
//! **Easing** (shared with the ships, doc 02 §4.4): a train that a new report would put somewhere
//! else glides there over [`EASE_MS`] (a smoothstep of the offset); one that jumped further than
//! it could have gone (0.05° of latitude or 0.1° of longitude) is not eased; and one drawn more
//! than 5 cm *ahead* of where its new frames have it, going their way, never moves backwards: it
//! holds where it is drawn until its frames come past ([`Hold`]).
//!
//! **Icons** ([`TrainBuilder::build`], `trainLayers` and `train-icons`). Close in
//! ([`cars_close_in`]: on the flat map once a 26 m car is at least 3 px long) the trains are their
//! cars (item T3) and no icon is drawn; further out each train is one [`TrainIcon`] lying flat on
//! the ground (deck's `billboard: false`), turned to the way it is drawn going: its carriage, or a
//! rounded square without a heading ([`train_icon`]), [`train_icon_size`] px long at the drawn
//! heading, in its colour (alpha [`TUNNEL_TRAIN_ALPHA`] in a tunnel), the train hovered in the
//! sidebar's list highlighted [`ICON_HIGHLIGHT`]. The icons' cells are in
//! [`crate::atlas::trains`].
//!
//! **Cars** (item T3; doc 02 §5.5–5.6, `drawnCars` and `carLayers`, G:1950-2029). Close in, after
//! [`TrainBuilder::build`], [`TrainBuilder::car_layers`] lays each train in the flat viewport's
//! bounds out as its cars ([`drawn_cars`]): as many as its operator says (else four), back from
//! where it is drawn along its frames and then the track behind it (`core::train_cars`), each car
//! the chord between its ends ([`car_places`]); a train with one of the operator models
//! ([`TrainModels`], `static/trains`) has that model's cars at their own lengths, end to end, any
//! other plain cars ([`car_mesh`] by its shape) of 25.4 m with 0.6 m between, and a train whose way
//! is not known has them in a row back along its heading. Each car gets its level on the track
//! (a tunnel or a bridge's height) and its colour: the train's, or white for a model's car (its
//! livery is in its vertex colours), alpha [`TUNNEL_TRAIN_ALPHA`] in a tunnel. [`CarLayers`] holds
//! them with the width and height every car is drawn at (never thinner than 3 px), and
//! [`CarLayers::layers`] splits them into deck's layers: per mesh, in the order of its first car,
//! the cars on the ground and on bridges, then those in tunnels. The map draws them
//! (`map::layers::train_cars`).
//!
//! Clocks: the trains' frames are timed on this machine's clock, so everything here takes the
//! local clock as whole Unix milliseconds (`Date.now()`), not the flight feed's.
//!
//! Deviation (plan section 6, item 1): an icon's angle is `−heading` in the true east/north frame
//! at every zoom. deck lays ground-plane icons along its globe frame, which is turned half a turn
//! from the flat map's, so today's code adds `yaw0 = 180` below zoom 12 (G:2041); the picture is
//! the same.

use std::collections::HashMap;

use rustradar_core::geo::meters_per_pixel;
use rustradar_core::model::{Train, TrainShape, f32_as_js};
use rustradar_core::train_cars::{CAR_M, DEFAULT_CARS, GAP_M, car_lines_with};
use rustradar_core::trains::{Frame, LatLon, heading_along, position_along, train_color_of, train_icon, train_icon_size};

use crate::blocks::view_bounds;
use crate::bridges::{GROUND, Level, level_at};
use crate::camera::{FLAT_ABOVE_ZOOM, ViewState};
use crate::geom::js_max;
use crate::train_models::{CAR_HEIGHT_M, CAR_WIDTH_M, TrainModels, car_mesh, car_places};

/// A train's alpha as drawn (G:1881).
pub const TRAIN_ALPHA: u8 = 240;
/// A train in a tunnel is seen through the ground (and what stands on it), faint (G:1884).
pub const TUNNEL_TRAIN_ALPHA: u8 = 90;
/// The icon of the train hovered in the sidebar's list (`highlightColor`, G:2049).
pub const ICON_HIGHLIGHT: [u8; 4] = [55, 142, 240, 200];
/// Close in, a train is its cars once a car is at least this long on the screen, px (G:1956).
pub const CARS_MIN_PX: f64 = 3.0;
/// How long a ship or train takes to ease onto a new report, ms (G:1625).
pub const EASE_MS: f64 = 2000.0;

/// Flat-earth metres in a degree (`aheadOf`).
const M_PER_DEG: f64 = 111_320.0;
/// A vehicle drawn this far or further from its new place (degrees of latitude, of longitude)
/// jumped: it is not eased.
const JUMP_LAT_DEG: f64 = 0.05;
const JUMP_LON_DEG: f64 = 0.1;
/// Drawn more than this far ahead of its new frames (metres), a vehicle holds rather than going
/// back.
const HOLD_AHEAD_M: f64 = 0.05;

// --- Easing (shared with the ships) ---------------------------------------------------------

/// A vehicle as the easing sees it: its reported position and heading (degrees clockwise from
/// north, as JavaScript reads it) and its frames (`(lat, lon, unix ms)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Way<'a> {
    pub lat: f64,
    pub lon: f64,
    pub heading: Option<f64>,
    pub frames: &'a [Frame],
}

impl<'a> Way<'a> {
    /// A train's way.
    pub fn of_train(train: &'a Train) -> Way<'a> {
        Way { lat: train.lat, lon: train.lon, heading: train.heading.map(f32_as_js), frames: &train.frames }
    }

    /// `positionAt`: along its frames, held at their ends; its reported position without any.
    pub fn position_at(&self, now_ms: i64) -> LatLon {
        position_along(self.frames, self.lat, self.lon, now_ms)
    }

    /// `headingAt`: the way its current frame leg runs, else the reported heading.
    pub fn heading_at(&self, now_ms: i64) -> Option<f64> {
        heading_along(self.frames, self.heading, now_ms)
    }
}

/// Where a vehicle that would go back to get onto its new frames waits, and which way they run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    pub lat: f64,
    pub lon: f64,
    pub heading: f64,
}

/// An offset to ease away from `from` (Unix ms) on; or, with a [`Hold`], the place a vehicle holds
/// at until its frames pass it (`Ease`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ease {
    pub d_lat: f64,
    pub d_lon: f64,
    pub from: i64,
    pub hold: Option<Hold>,
}

/// JavaScript's truthiness of a number.
fn truthy(x: f64) -> bool {
    x != 0.0 && !x.is_nan()
}

/// `((d + 540) % 360) − 180`: a longitude difference the short way round.
fn wrap_lon(d: f64) -> f64 {
    ((d + 540.0) % 360.0) - 180.0
}

/// How far `p` is ahead of `from` going `heading` (degrees), metres (`aheadOf`).
pub fn ahead_of(p: LatLon, from: LatLon, heading: f64) -> f64 {
    let k = (from.lat * std::f64::consts::PI / 180.0).cos();
    let east = wrap_lon(p.lon - from.lon) * k * M_PER_DEG;
    let north = (p.lat - from.lat) * M_PER_DEG;
    let h = heading * std::f64::consts::PI / 180.0;
    east * h.sin() + north * h.cos()
}

/// The offset from where a vehicle is now expected to be (`to` at `now_ms`) to where it was drawn
/// (`was`, `[lon, lat]`; none for a vehicle not drawn before), to ease away (`easeFrom`): none for
/// a new one, one that hasn't moved, or one that jumped further than it could have gone. Never
/// back: drawn ahead of where its new frames have it, it holds there.
pub fn ease_from(was: Option<[f64; 2]>, to: Way<'_>, now_ms: i64) -> Option<Ease> {
    let [lon, lat] = was?;
    let target = to.position_at(now_ms);
    let d_lat = lat - target.lat;
    let d_lon = wrap_lon(lon - target.lon);
    if !(truthy(d_lat) || truthy(d_lon)) || d_lat.abs() >= JUMP_LAT_DEG || d_lon.abs() >= JUMP_LON_DEG {
        return None;
    }
    if let Some(heading) = to.heading_at(now_ms)
        && ahead_of(LatLon { lat, lon }, target, heading) > HOLD_AHEAD_M
    {
        return Some(Ease { d_lat: 0.0, d_lon: 0.0, from: now_ms, hold: Some(Hold { lat, lon, heading }) });
    }
    Some(Ease { d_lat, d_lon, from: now_ms, hold: None })
}

/// Where a vehicle is drawn at `now_ms`: along its frames, plus what is left of its easing (a
/// smoothstep), or at its hold while its frames are behind it (`eased`). The easing is dropped
/// (`blend` set to none) once it is done, or once the frames have passed the hold.
pub fn eased(v: Way<'_>, blend: &mut Option<Ease>, now_ms: i64) -> LatLon {
    let LatLon { mut lat, mut lon } = v.position_at(now_ms);
    match *blend {
        Some(Ease { hold: Some(hold), .. }) => {
            // there until its frames come past
            if ahead_of(LatLon { lat, lon }, LatLon { lat: hold.lat, lon: hold.lon }, hold.heading) < 0.0 {
                return LatLon { lat: hold.lat, lon: hold.lon };
            }
            *blend = None;
        }
        Some(Ease { d_lat, d_lon, from, hold: None }) => {
            let left = 1.0 - ((now_ms - from) as f64 / EASE_MS).min(1.0);
            let k = left * left * (3.0 - 2.0 * left);
            lat += d_lat * k;
            lon += d_lon * k;
            if left == 0.0 {
                *blend = None;
            }
        }
        None => {}
    }
    LatLon { lat, lon }
}

// --- Where the trains are drawn ---------------------------------------------------------------

/// The trains of one list as drawn (`TrainData`), in the list's order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawn {
    /// `[lon, lat, 0]`.
    pub positions: Vec<[f64; 3]>,
    /// The direction of travel as drawn, degrees clockwise from north; none when unknown.
    pub headings: Vec<Option<f64>>,
    /// The operator's or kind's colour at [`TRAIN_ALPHA`].
    pub colors: Vec<[u8; 4]>,
    /// On the ground, on a bridge or in a tunnel, where it is drawn.
    pub levels: Vec<Level>,
    /// The trains with frames to move along, or easing onto a new report: stepped at every
    /// refresh.
    moving: Vec<usize>,
    /// Index by train id (the last of a repeated id).
    by_id: HashMap<String, usize>,
    /// Trains easing from where they were drawn onto their new report.
    blends: Vec<Option<Ease>>,
}

impl Drawn {
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Where the train with `id` is in the list (`byId`).
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    /// Whether the train at `i` moves at each refresh.
    pub fn moving(&self, i: usize) -> bool {
        self.moving.contains(&i)
    }

    /// The easing of the train at `i`, if it has one.
    pub fn blend(&self, i: usize) -> Option<&Ease> {
        self.blends.get(i).and_then(Option::as_ref)
    }

    /// The colour the train at `i` is drawn in: [`TUNNEL_TRAIN_ALPHA`] in a tunnel.
    pub fn color(&self, i: usize) -> [u8; 4] {
        let c = self.colors[i];
        if self.levels[i].tunnel { [c[0], c[1], c[2], TUNNEL_TRAIN_ALPHA] } else { c }
    }
}

/// The level of `train` at `[lon, lat]` going `heading`: on the ground without structures.
fn level_of(train: &Train, at: [f64; 2], heading: Option<f64>) -> Level {
    if train.structures.is_empty() { GROUND } else { level_at(at, heading, &train.structures) }
}

/// Where the trains of the current list are drawn, kept between scene refreshes.
#[derive(Debug, Clone, Default)]
pub struct TrainMotion {
    drawn: Drawn,
}

impl TrainMotion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes in a new list of trains at `now_ms` (`trainsFor` for a new list object): each at its
    /// reported position, heading and level, eased from where the list before drew a train of the
    /// same id.
    pub fn set_list(&mut self, list: &[&Train], now_ms: i64) {
        let old = std::mem::take(&mut self.drawn);
        let mut data = Drawn::default();
        for (i, &train) in list.iter().enumerate() {
            // a new report moves the train on from where it is drawn, not with a jump
            let was = old.index_of(&train.id).map(|w| [old.positions[w][0], old.positions[w][1]]);
            let blend = ease_from(was, Way::of_train(train), now_ms);
            let heading = train.heading.map(f32_as_js);
            data.positions.push([train.lon, train.lat, 0.0]);
            data.headings.push(heading);
            let [r, g, b] = train_color_of(train);
            data.colors.push([r, g, b, TRAIN_ALPHA]);
            data.levels.push(level_of(train, [train.lon, train.lat], heading));
            data.by_id.insert(train.id.clone(), i);
            if train.frames.len() > 1 || blend.is_some() {
                data.moving.push(i);
            }
            data.blends.push(blend);
        }
        self.drawn = data;
    }

    /// Steps the trains with frames on to `now_ms` (`moveTrains`); the rest stay at their reported
    /// positions. `list` is the list last given to [`Self::set_list`].
    pub fn step(&mut self, list: &[&Train], now_ms: i64) {
        let data = &mut self.drawn;
        for &i in &data.moving {
            let Some(&train) = list.get(i) else { continue };
            let way = Way::of_train(train);
            let LatLon { lat, lon } = eased(way, &mut data.blends[i], now_ms);
            let heading = way.heading_at(now_ms);
            data.positions[i] = [lon, lat, 0.0];
            data.headings[i] = heading;
            if !train.structures.is_empty() {
                data.levels[i] = level_at([lon, lat], heading, &train.structures);
            }
        }
    }

    /// The trains as drawn.
    pub fn drawn(&self) -> &Drawn {
        &self.drawn
    }
}

// --- Icons ---------------------------------------------------------------------------------

/// Whether, at `zoom` with the view centre at `latitude`, the trains are drawn as their cars
/// rather than icons: on the flat map, once a [`CAR_M`] car is at least [`CARS_MIN_PX`] long
/// (`metersPerPixel` at the view centre; about zoom 12.2 at 59° N, 13.14 at the equator).
pub fn cars_close_in(zoom: f64, latitude: f64) -> bool {
    zoom > FLAT_ABOVE_ZOOM && CAR_M / meters_per_pixel(zoom, latitude) >= CARS_MIN_PX
}

/// One train icon (an instance of `train-icons`).
#[derive(Debug, Clone, PartialEq)]
pub struct TrainIcon {
    /// Where the train is drawn, `[lon, lat, 0]`.
    pub position: [f64; 3],
    /// Its cell in the atlas: `car-<key>` with a heading, `box-<key>` without.
    pub name: String,
    /// The cell's height on the screen, px (at the view centre's depth; it lies on the ground).
    pub size_px: f64,
    /// Degrees anticlockwise from north on the ground: `−heading`, 0 without one.
    pub angle: f64,
    /// Its colour; the atlas holds the colours already, so only the alpha fades it.
    pub color: [u8; 4],
    /// The train hovered in the sidebar's list: drawn with [`ICON_HIGHLIGHT`].
    pub highlighted: bool,
}

/// The icons of `list` as `drawn`, at `zoom`, with the train of id `hovered` highlighted
/// (`train-icons`' accessors).
pub fn icons(list: &[&Train], drawn: &Drawn, zoom: f64, hovered: Option<&str>) -> Vec<TrainIcon> {
    // `highlightedObjectIndex`
    let hovered = hovered.and_then(|id| drawn.index_of(id));
    list.iter()
        .enumerate()
        .take(drawn.len())
        .map(|(i, train)| {
            let heading = drawn.headings[i];
            TrainIcon {
                position: drawn.positions[i],
                name: train_icon(train.kind, heading, train.operator.as_deref()),
                size_px: train_icon_size(train.kind, heading, zoom),
                angle: 0.0 - heading.unwrap_or(0.0),
                color: drawn.color(i),
                highlighted: hovered == Some(i),
            }
        })
        .collect()
}

// --- Cars (item T3) --------------------------------------------------------------------------

/// A car is never thinner on the screen than this, px, nor less than 1.3 times this tall
/// (`MIN_CAR_WIDTH_PX`, G:1958).
pub const MIN_CAR_WIDTH_PX: f64 = 3.0;
/// The colour of an operator model's car: it wears its own livery in its vertex colours
/// (`MODEL_WHITE`, G:1959).
pub const MODEL_WHITE: [u8; 4] = [255, 255, 255, 255];
/// The car under the pointer (`autoHighlight` with `highlightColor`, G:2022).
pub const CAR_HIGHLIGHT: [u8; 4] = [55, 142, 240, 110];

/// Which mesh a car is drawn with (`DrawnCar.mesh`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CarShape {
    /// A plain car ([`car_mesh`]): the train's leading car, or one of its coaches, of its shape.
    Plain { lead: bool, shape: TrainShape },
    /// Car `index` of the operator model `model` ([`TrainModels::cars`]: 0 its leading car, 1 a
    /// middle one, 2 its last).
    Model { model: String, index: usize },
}

impl CarShape {
    /// The mesh's key (`mesh.key`), which names its layers: `"<shape>-cab"` or
    /// `"<coach shape>-coach"` for a plain car, `"<model>-<index>"` for a model's.
    pub fn key(&self) -> String {
        match self {
            CarShape::Plain { lead, shape } => car_mesh(*lead, *shape).0.to_owned(),
            CarShape::Model { model, index } => format!("{model}-{index}"),
        }
    }
}

/// A car on the map (`DrawnCar`): one of the cars of the train at `train` in the list, on the
/// ground, up on a bridge or in a tunnel.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawnCar {
    /// Its middle, `[lon, lat]`: halfway between its ends.
    pub at: [f64; 2],
    /// The way it points, from its back to its front: degrees clockwise from north.
    pub heading: f64,
    /// How long it is drawn, metres: the chord between its ends.
    pub length: f64,
    /// The train it belongs to: its index in the list.
    pub train: usize,
    pub mesh: CarShape,
    /// One of an operator model's cars (all of its train's are).
    pub modelled: bool,
    /// Where it is: its height (metres up on a bridge) and whether it is in a tunnel.
    pub level: Level,
    /// `getColor`: the train's colour at [`TRAIN_ALPHA`], or [`MODEL_WHITE`] for a model's car;
    /// alpha [`TUNNEL_TRAIN_ALPHA`] in a tunnel.
    pub color: [u8; 4],
    /// Under the pointer: drawn with [`CAR_HIGHLIGHT`] (deck's `autoHighlight`). Nothing sets it
    /// here; the map's picking does (item T8).
    pub highlighted: bool,
}

/// The cars of `list`'s trains as `drawn`, at `now_ms` (`drawnCars`, G:1970-1995): each train in
/// `bounds` (`[west, south, east, north]` padded by 0.01°; none without bounds, as on the globe)
/// as many cars as its operator says, else [`DEFAULT_CARS`], back from where it is drawn along its
/// frames and then its tail ([`car_lines_with`], [`car_places`]). A train with an operator model
/// in `models` has that model's cars, each as long as the real one, end to end; any other has
/// plain cars of 25.4 m with 0.6 m between. A train whose way does not give all its cars (no
/// frames) has them in a row straight back along its heading.
pub fn drawn_cars(list: &[&Train], drawn: &Drawn, bounds: Option<[f64; 4]>, models: Option<&TrainModels>, now_ms: i64) -> Vec<DrawnCar> {
    let Some([west, south, east, north]) = bounds else {
        return Vec::new();
    };
    let mut cars = Vec::new();
    for (i, &train) in list.iter().enumerate().take(drawn.len()) {
        let [lon, lat, _] = drawn.positions[i];
        if lon < west - 0.01 || lon > east + 0.01 || lat < south - 0.01 || lat > north + 0.01 {
            continue;
        }
        let count = train.cars.unwrap_or(DEFAULT_CARS).max(1) as usize;
        // a train with a model of its own: its cars, each as long as the real one
        let model = |k: usize| models.and_then(|m| m.model_car(train.model.as_deref(), k, count));
        let modelled = model(0).is_some();
        // (the TypeScript's `model(k)!.length`: every model has its three cars)
        let length_of = |k: usize| if modelled { model(k).map_or(f64::NAN, |car| car.length) } else { CAR_M - GAP_M };
        let gap = if modelled { 0.0 } else { GAP_M };
        let mut places = car_places(&car_lines_with(&train.frames, &train.tail, count, now_ms, length_of, gap));
        if places.len() != count {
            let h = drawn.headings[i].unwrap_or(0.0) * std::f64::consts::PI / 180.0;
            let k = (lat * std::f64::consts::PI / 180.0).cos();
            let behind = (lat - h.cos() / 1000.0, lon - h.sin() / 1000.0 / if truthy(k) { k } else { 1e-6 });
            places = car_places(&car_lines_with(&[(lat, lon, now_ms)], &[(lat, lon), behind], count, now_ms, length_of, gap));
        }
        for (k, place) in places.iter().enumerate() {
            let mesh = match (model(k), models, &train.model) {
                (Some(car), Some(models), Some(name)) => {
                    let index = models.cars(name).and_then(|cars| cars.iter().position(|c| std::ptr::eq(c, car))).unwrap_or(0);
                    CarShape::Model { model: name.clone(), index }
                }
                _ => CarShape::Plain { lead: k == 0, shape: train.shape.unwrap_or(TrainShape::Regional) },
            };
            let level = level_of(train, place.at, Some(place.heading));
            let [r, g, b, a] = if modelled { MODEL_WHITE } else { drawn.colors[i] };
            let color = [r, g, b, if level.tunnel { TUNNEL_TRAIN_ALPHA } else { a }];
            cars.push(DrawnCar { at: place.at, heading: place.heading, length: place.length, train: i, mesh, modelled, level, color, highlighted: false });
        }
    }
    cars
}

/// One of the cars' mesh layers (`train-car-meshes-<key>` and `train-car-meshes-<key>-tunnel`):
/// the cars of one mesh, on the ground and on bridges or in tunnels.
#[derive(Debug, Clone, PartialEq)]
pub struct CarLayer {
    /// deck's layer id.
    pub id: String,
    /// The mesh's key ([`CarShape::key`]).
    pub key: String,
    /// The mesh the layer draws (its first car's).
    pub mesh: CarShape,
    /// In tunnels: seen faintly through the ground (`THROUGH_GROUND`); else depth tested and
    /// written (`NO_CULL`).
    pub tunnel: bool,
    /// Its cars, as indices into [`CarLayers::cars`], in their order.
    pub cars: Vec<usize>,
}

/// Close in, the trains as their cars in 3D (`carLayers`, G:1998-2029).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CarLayers {
    /// The cars of the trains in view, train by train, each train's front car first.
    pub cars: Vec<DrawnCar>,
    /// Every car's width as drawn, metres: [`CAR_WIDTH_M`], or [`MIN_CAR_WIDTH_PX`] px where that
    /// is wider, so a train far off is still seen whole.
    pub width: f64,
    /// Every car's height as drawn, metres: [`CAR_HEIGHT_M`], or 1.3 × [`MIN_CAR_WIDTH_PX`] px.
    pub height: f64,
}

impl CarLayers {
    /// The cars of `drawn_cars` at `mpp` metres to a pixel.
    pub fn new(cars: Vec<DrawnCar>, mpp: f64) -> Self {
        // never thinner than a few pixels, so a train far off is still seen whole
        let width = js_max(CAR_WIDTH_M, MIN_CAR_WIDTH_PX * mpp);
        let height = js_max(CAR_HEIGHT_M, MIN_CAR_WIDTH_PX * 1.3 * mpp);
        CarLayers { cars, width, height }
    }

    /// The mesh layers in draw order: for each mesh the cars take, in the order of its first car
    /// (`new Map(cars.map((c) => [c.mesh.key, c]))`), its cars on the ground and up on bridges,
    /// then its cars in tunnels; a layer without cars is left out (deck draws an empty layer as
    /// nothing).
    pub fn layers(&self) -> Vec<CarLayer> {
        let mut keys: Vec<(String, &CarShape)> = Vec::new();
        for car in &self.cars {
            let key = car.mesh.key();
            if !keys.iter().any(|(k, _)| *k == key) {
                keys.push((key, &car.mesh));
            }
        }
        let mut layers = Vec::new();
        for (key, mesh) in keys {
            for tunnel in [false, true] {
                let cars: Vec<usize> = (0..self.cars.len()).filter(|&c| self.cars[c].mesh.key() == key && self.cars[c].level.tunnel == tunnel).collect();
                if !cars.is_empty() {
                    let id = format!("train-car-meshes-{key}{}", if tunnel { "-tunnel" } else { "" });
                    layers.push(CarLayer { id, key: key.clone(), mesh: mesh.clone(), tunnel, cars });
                }
            }
        }
        layers
    }
}

/// The cars of `list`'s trains as `drawn`, at `now_ms`, in `view` on a canvas of `size` CSS px
/// (`carLayers`): those of the trains in the flat viewport's bounds ([`drawn_cars`]), sized at the
/// view centre's metres per pixel.
pub fn car_layers(list: &[&Train], drawn: &Drawn, view: &ViewState, size: [f64; 2], models: Option<&TrainModels>, now_ms: i64) -> CarLayers {
    let mpp = meters_per_pixel(view.zoom, view.latitude);
    CarLayers::new(drawn_cars(list, drawn, view_bounds(view, size), models, now_ms), mpp)
}

/// What the train layers draw at one scene refresh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrainsScene {
    /// `train-icons`: every train, unless they are drawn as cars or there are none.
    pub icons: Vec<TrainIcon>,
    /// Close in, the trains are drawn as their cars (item T3) and have no icon.
    pub cars: bool,
    /// Close in ([`Self::cars`]): the cars. [`TrainBuilder::build`] leaves them empty;
    /// [`TrainBuilder::car_layers`] makes them from where it drew the trains, since it needs the
    /// canvas and the operator models.
    pub car_layers: CarLayers,
}

/// The trains' part of the scene refresh: the motion kept between refreshes.
#[derive(Debug, Clone, Default)]
pub struct TrainBuilder {
    motion: TrainMotion,
}

impl TrainBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The trains of `list` at `now_ms` (local whole Unix ms) in `view` (`trainLayers`):
    /// `new_list` when the list is not the one of the last refresh (it is taken in with
    /// [`TrainMotion::set_list`]); then the moving trains stepped on, and their icons further
    /// out, the train of id `hovered` highlighted.
    pub fn build(&mut self, list: &[&Train], new_list: bool, view: &ViewState, hovered: Option<&str>, now_ms: i64) -> TrainsScene {
        if new_list {
            self.motion.set_list(list, now_ms);
        }
        if list.is_empty() {
            return TrainsScene::default();
        }
        self.motion.step(list, now_ms);
        // close in, the cars; further out, a train is an icon lying on the ground along its way
        if cars_close_in(view.zoom, view.latitude) {
            return TrainsScene { cars: true, ..TrainsScene::default() };
        }
        TrainsScene { icons: icons(list, self.motion.drawn(), view.zoom, hovered), ..TrainsScene::default() }
    }

    /// Close in (a [`TrainsScene::cars`] refresh), after [`Self::build`] of the same refresh: the
    /// cars of `list`'s trains where they are drawn ([`car_layers`]), in `view` on a canvas of
    /// `size` CSS px, at `now_ms`, with the operator models `models` where loaded (plain cars
    /// without).
    pub fn car_layers(&self, list: &[&Train], view: &ViewState, size: [f64; 2], models: Option<&TrainModels>, now_ms: i64) -> CarLayers {
        car_layers(list, self.motion.drawn(), view, size, models, now_ms)
    }

    /// Where the trains are drawn (for the cars, the route and the ring).
    pub fn drawn(&self) -> &Drawn {
        self.motion.drawn()
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::{Structure, TrainKind, TrainSource};

    use super::*;

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;
    const STOCKHOLM: (f64, f64) = (59.33, 18.07);

    /// `(lat, lon)` `m` metres from `at` going `heading` (the demo trains' flat step).
    fn ahead((lat, lon): (f64, f64), heading: f64, m: f64) -> (f64, f64) {
        let h = heading.to_radians();
        let k = lat.to_radians().cos();
        (lat + h.cos() * m / M_PER_DEG, lon + h.sin() * m / (M_PER_DEG * k))
    }

    fn train(id: &str, at: (f64, f64), heading: Option<f32>) -> Train {
        Train {
            id: id.into(),
            source: TrainSource::Trafikverket,
            name: format!("Train {id}"),
            kind: TrainKind::Regional,
            lat: at.0,
            lon: at.1,
            heading,
            speed_kmh: None,
            destination: None,
            operator: None,
            delay_minutes: None,
            position_unix: NOW / 1000,
            frames: Vec::new(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: None,
            tail: Vec::new(),
            structures: Vec::new(),
            ended_unix: None,
            shape: None,
            model: None,
        }
    }

    /// A train at `from` running `heading` at `speed` m/s, with frames every 2 s from `t0` for a
    /// minute.
    fn running(id: &str, from: (f64, f64), heading: f64, speed: f64, t0: i64) -> Train {
        let mut t = train(id, from, Some(heading as f32));
        t.frames = (0..=30)
            .map(|k| {
                let (lat, lon) = ahead(from, heading, speed * 2.0 * f64::from(k));
                (lat, lon, t0 + i64::from(k) * 2000)
            })
            .collect();
        t
    }

    /// Flat metres from `a` to `b`, `(lat, lon)`.
    fn metres(a: LatLon, b: LatLon) -> f64 {
        ((b.lat - a.lat) * M_PER_DEG).hypot(wrap_lon(b.lon - a.lon) * M_PER_DEG * a.lat.to_radians().cos())
    }

    fn ll((lat, lon): (f64, f64)) -> LatLon {
        LatLon { lat, lon }
    }

    #[test]
    fn ahead_of_measures_along_the_heading() {
        let from = ll(STOCKHOLM);
        // (the longitude difference goes through `+ 540`, so it keeps about 1e-13° of precision:
        // micrometres)
        assert!((ahead_of(ll(ahead(STOCKHOLM, 0.0, 100.0)), from, 0.0) - 100.0).abs() < 1e-6);
        assert!((ahead_of(ll(ahead(STOCKHOLM, 0.0, 100.0)), from, 180.0) + 100.0).abs() < 1e-6);
        // east, with the longitude scaled at `from`'s latitude
        assert!((ahead_of(ll(ahead(STOCKHOLM, 90.0, 50.0)), from, 90.0) - 50.0).abs() < 1e-6);
        // across the way: nothing ahead
        assert!(ahead_of(ll(ahead(STOCKHOLM, 90.0, 50.0)), from, 0.0).abs() < 1e-6);
        // 45° off: cos 45° of it
        let d = ahead_of(ll(ahead(STOCKHOLM, 70.0, 100.0)), from, 25.0);
        assert!((d - 100.0 * 45f64.to_radians().cos()).abs() < 1e-6, "{d}");
        // the short way across the antimeridian
        let d = ahead_of(LatLon { lat: 0.0, lon: -179.999 }, LatLon { lat: 0.0, lon: 179.999 }, 90.0);
        assert!((d - 0.002 * M_PER_DEG).abs() < 1e-6, "{d}");
    }

    #[test]
    fn a_new_vehicle_one_that_did_not_move_or_one_that_jumped_is_not_eased() {
        let t = train("a", STOCKHOLM, Some(90.0));
        let way = Way::of_train(&t);
        assert_eq!(ease_from(None, way, NOW), None, "not drawn before");
        assert_eq!(ease_from(Some([STOCKHOLM.1, STOCKHOLM.0]), way, NOW), None, "drawn where it is");
        // (59.33 + 0.05 − 59.33 is a hair under 0.05: a jump is measured as the difference comes out)
        assert_eq!(ease_from(Some([STOCKHOLM.1, STOCKHOLM.0 + 0.050_001]), way, NOW), None, "0.05° of latitude is a jump");
        assert_eq!(ease_from(Some([STOCKHOLM.1 - 0.100_001, STOCKHOLM.0]), way, NOW), None, "0.1° of longitude is a jump");
        assert!(ease_from(Some([STOCKHOLM.1 - 0.0999, STOCKHOLM.0 + 0.0499]), way, NOW).is_some(), "just short of a jump");
        // the longitude the short way: 359.99° apart is 0.01°
        let t = train("b", (0.0, 179.995), None);
        let e = ease_from(Some([-179.995, 0.0]), Way::of_train(&t), NOW).expect("eased");
        assert!((e.d_lon - 0.01).abs() < 1e-9, "{}", e.d_lon);
    }

    #[test]
    fn drawn_behind_it_eases_on_drawn_ahead_it_holds() {
        // heading east; drawn 30 m behind its new place
        let t = train("a", STOCKHOLM, Some(90.0));
        let way = Way::of_train(&t);
        let behind = ahead(STOCKHOLM, 90.0, -30.0);
        let e = ease_from(Some([behind.1, behind.0]), way, NOW).expect("eased");
        assert_eq!(e.hold, None);
        assert_eq!((e.d_lat, e.d_lon, e.from), (behind.0 - STOCKHOLM.0, wrap_lon(behind.1 - STOCKHOLM.1), NOW));
        // drawn 30 m ahead: it holds there, facing the way it goes
        let before = ahead(STOCKHOLM, 90.0, 30.0);
        let e = ease_from(Some([before.1, before.0]), way, NOW).expect("held");
        assert_eq!(e, Ease { d_lat: 0.0, d_lon: 0.0, from: NOW, hold: Some(Hold { lat: before.0, lon: before.1, heading: 90.0 }) });
        // 4 cm ahead is not ahead
        let close = ahead(STOCKHOLM, 90.0, 0.04);
        assert_eq!(ease_from(Some([close.1, close.0]), way, NOW).expect("eased").hold, None);
        // without a heading nothing is ahead: it eases back
        let t = train("b", STOCKHOLM, None);
        assert_eq!(ease_from(Some([before.1, before.0]), Way::of_train(&t), NOW).expect("eased").hold, None);
        // the heading is the frames' when they have one: frames running west make east "behind"
        let mut t = running("c", STOCKHOLM, 270.0, 10.0, NOW);
        t.heading = Some(90.0);
        let e = ease_from(Some([before.1, before.0]), Way::of_train(&t), NOW).expect("eased");
        assert_eq!(e.hold, None);
    }

    #[test]
    fn an_offset_eases_away_by_a_smoothstep_over_two_seconds() {
        let t = train("a", STOCKHOLM, Some(90.0));
        let way = Way::of_train(&t);
        let mut blend = Some(Ease { d_lat: 0.001, d_lon: -0.002, from: NOW, hold: None });
        let at = |blend: &mut Option<Ease>, ms: i64| eased(way, blend, NOW + ms);
        let p = at(&mut blend, 0);
        assert_eq!((p.lat, p.lon), (STOCKHOLM.0 + 0.001, STOCKHOLM.1 - 0.002), "all of it at first");
        // half way through: smoothstep(0.5) = 0.5
        let p = at(&mut blend, 1000);
        assert!((p.lat - (STOCKHOLM.0 + 0.0005)).abs() < 1e-12 && (p.lon - (STOCKHOLM.1 - 0.001)).abs() < 1e-12);
        // a quarter left: 0.25² · 2.5 = 0.15625
        let p = at(&mut blend, 1500);
        assert!((p.lat - (STOCKHOLM.0 + 0.001 * 0.15625)).abs() < 1e-12, "{}", p.lat);
        assert!(blend.is_some());
        // done: dropped
        let p = at(&mut blend, 2000);
        assert_eq!((p.lat, p.lon), STOCKHOLM);
        assert_eq!(blend, None);
        // past the end at once
        let mut blend = Some(Ease { d_lat: 0.001, d_lon: 0.0, from: NOW, hold: None });
        assert_eq!(at(&mut blend, 5000), ll(STOCKHOLM));
        assert_eq!(blend, None);
    }

    #[test]
    fn a_held_vehicle_waits_until_its_frames_come_past_and_never_runs_backward() {
        // its new frames start 50 m behind where it is drawn and run east at 10 m/s
        let drawn = ahead(STOCKHOLM, 90.0, 50.0);
        let t = running("a", STOCKHOLM, 90.0, 10.0, NOW);
        let way = Way::of_train(&t);
        let mut blend = ease_from(Some([drawn.1, drawn.0]), way, NOW);
        assert!(blend.is_some_and(|b| b.hold.is_some()), "{blend:?}");
        let mut last = f64::NEG_INFINITY;
        let mut released = None;
        for ms in (0..=10_000).step_by(100) {
            let p = eased(way, &mut blend, NOW + ms);
            let along = ahead_of(p, ll(STOCKHOLM), 90.0);
            assert!(along >= last - 1e-9, "backward at {ms} ms: {along} after {last}");
            last = along;
            if ms < 5000 {
                assert_eq!(p, ll(drawn), "held at {ms} ms");
            } else if released.is_none() && blend.is_none() {
                released = Some(ms);
            }
        }
        // the frames reach the hold at 5 s; it lets go there (or a step later, by a rounding of
        // the interpolation), on its frames
        assert!(matches!(released, Some(5000 | 5100)), "{released:?}");
        let p = eased(way, &mut blend, NOW + 8000);
        assert!((metres(p, ll(STOCKHOLM)) - 80.0).abs() < 1e-6);
    }

    #[test]
    fn a_new_list_eases_each_train_from_where_the_old_one_drew_it() {
        let mut motion = TrainMotion::new();
        // first list: a train running east from Stockholm, one standing still
        let a = running("a", STOCKHOLM, 90.0, 10.0, NOW);
        let still = train("still", ahead(STOCKHOLM, 0.0, 500.0), None);
        motion.set_list(&[&a, &still], NOW);
        let d = motion.drawn();
        assert_eq!(d.len(), 2);
        assert_eq!(d.positions[0], [a.lon, a.lat, 0.0]);
        assert!(d.moving(0) && !d.moving(1));
        assert_eq!(d.blend(0), None, "nothing drawn before");
        motion.step(&[&a, &still], NOW + 10_000);
        let at = motion.drawn().positions[0];
        assert!((metres(ll(STOCKHOLM), LatLon { lat: at[1], lon: at[0] }) - 100.0).abs() < 1e-6);
        assert_eq!(motion.drawn().positions[1], [still.lon, still.lat, 0.0]);
        // a new report 20 m further on than it is drawn: it eases forward, in a new order
        let a2 = running("a", ahead(STOCKHOLM, 90.0, 120.0), 90.0, 10.0, NOW + 10_000);
        motion.set_list(&[&still, &a2], NOW + 10_000);
        let d = motion.drawn();
        assert_eq!(d.index_of("a"), Some(1));
        assert_eq!(d.positions[1], [a2.lon, a2.lat, 0.0], "at its report until stepped");
        let blend = *d.blend(1).expect("eased");
        assert_eq!(blend.hold, None);
        assert!((blend.d_lon - (at[0] - a2.lon)).abs() < 1e-12);
        motion.step(&[&still, &a2], NOW + 10_000);
        let p = motion.drawn().positions[1];
        assert!((p[0] - at[0]).abs() < 1e-12 && (p[1] - at[1]).abs() < 1e-12, "where it was drawn: {p:?} {at:?}");
        // a new report 20 m behind: it holds
        let a3 = running("a", ahead(STOCKHOLM, 90.0, 80.0), 90.0, 10.0, NOW + 10_000);
        motion.set_list(&[&a3], NOW + 10_000);
        assert!(motion.drawn().blend(0).is_some_and(|b| b.hold.is_some()));
        // an empty list forgets them: the next list eases nothing
        motion.set_list(&[], NOW + 10_000);
        motion.set_list(&[&a3], NOW + 10_000);
        assert_eq!(motion.drawn().blend(0), None);
    }

    #[test]
    fn colours_by_operator_or_kind_and_faint_in_a_tunnel() {
        let mut sj = train("sj", STOCKHOLM, Some(10.0));
        sj.operator = Some("SJ".into());
        let mut tunnel = running("t", STOCKHOLM, 0.0, 20.0, NOW);
        tunnel.kind = TrainKind::Suburban;
        let a = ahead(STOCKHOLM, 0.0, 100.0);
        let b = ahead(STOCKHOLM, 0.0, 300.0);
        tunnel.structures = vec![Structure { tunnel: true, line: vec![[a.1, a.0], [b.1, b.0]], joined: [false, false] }];
        let mut motion = TrainMotion::new();
        let list = [&sj, &tunnel];
        motion.set_list(&list, NOW);
        let d = motion.drawn();
        assert_eq!(d.colors, [[232, 232, 236, 240], [112, 190, 232, 240]]);
        assert!(!d.levels[1].tunnel, "it starts short of the tunnel");
        assert_eq!(d.color(1), [112, 190, 232, 240]);
        // 10 s on it is 200 m along: in the tunnel
        motion.step(&list, NOW + 10_000);
        let d = motion.drawn();
        assert!(d.levels[1].tunnel);
        assert_eq!(d.color(1), [112, 190, 232, 90]);
        let icons = icons(&list, d, 9.0, None);
        assert_eq!(icons[1].color, [112, 190, 232, 90]);
        assert_eq!(icons[0].color, [232, 232, 236, 240]);
        // past it, out again
        motion.step(&list, NOW + 20_000);
        assert!(!motion.drawn().levels[1].tunnel);
    }

    #[test]
    fn icons_turn_with_the_drawn_heading_and_size_by_kind_and_zoom() {
        let mut fast = running("fast", STOCKHOLM, 250.0, 22.0, NOW);
        fast.kind = TrainKind::HighSpeed;
        fast.operator = Some("SJ".into());
        // its report says north, its frames west-south-west: the frames win once it moves
        fast.heading = Some(0.0);
        let mut parked = train("parked", ahead(STOCKHOLM, 90.0, 200.0), None);
        parked.kind = TrainKind::Other;
        let list = [&fast, &parked];
        let mut builder = TrainBuilder::new();
        let view = ViewState { longitude: STOCKHOLM.1, latitude: STOCKHOLM.0, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };
        let scene = builder.build(&list, true, &view, None, NOW + 1000);
        assert!(!scene.cars);
        assert_eq!(scene.icons.len(), 2);
        let [f, p] = [&scene.icons[0], &scene.icons[1]];
        assert_eq!(f.name, "car-op0");
        // the great circle of a short leg stepped flat: 250° within a thousandth
        assert!((f.angle + 250.0).abs() < 1e-3, "{}", f.angle);
        assert_eq!(f.size_px, 24.0);
        assert!(!f.highlighted);
        let pos = builder.drawn().positions[0];
        assert_eq!(f.position, pos);
        assert!((metres(ll(STOCKHOLM), LatLon { lat: pos[1], lon: pos[0] }) - 22.0).abs() < 1e-6);
        // no heading: the square, smaller, not turned
        assert_eq!(p.name, "box-other");
        assert_eq!(p.angle, 0.0);
        assert_eq!(p.size_px, 19.0 * 0.8);
        assert_eq!(p.position, [parked.lon, parked.lat, 0.0]);
        // further out, smaller (in steps)
        for (zoom, scale) in [(3.9, 0.7), (4.0, 0.82), (5.9, 0.82), (6.0, 0.92), (8.99, 0.92), (12.0, 1.0)] {
            let scene = builder.build(&list, false, &ViewState { zoom, ..view }, None, NOW + 1000);
            assert_eq!(scene.icons[0].size_px, 24.0 * scale, "zoom {zoom}");
        }
        // the train hovered in the list
        let scene = builder.build(&list, false, &view, Some("parked"), NOW + 1000);
        assert_eq!(scene.icons.iter().map(|i| i.highlighted).collect::<Vec<_>>(), [false, true]);
        let scene = builder.build(&list, false, &view, Some("gone"), NOW + 1000);
        assert!(scene.icons.iter().all(|i| !i.highlighted));
    }

    #[test]
    fn close_in_the_trains_are_cars_and_have_no_icons() {
        // 26 m is 3 px at 8.67 m/px: about zoom 12.2 at 59° N, 13.14 at the equator
        assert!(!cars_close_in(12.0, 59.33), "the globe: icons");
        assert!(!cars_close_in(12.1, 59.33));
        assert!(cars_close_in(12.3, 59.33));
        assert!(!cars_close_in(13.1, 0.0));
        assert!(cars_close_in(13.2, 0.0));
        let threshold = (meters_per_pixel(0.0, 0.0) * 3.0 / 26.0).log2();
        assert!((threshold - 13.14).abs() < 0.01, "{threshold}");
        assert!(!cars_close_in(threshold - 1e-9, 0.0) && cars_close_in(threshold + 1e-9, 0.0));
        let a = running("a", STOCKHOLM, 90.0, 10.0, NOW);
        let list = [&a];
        let mut builder = TrainBuilder::new();
        let view = ViewState { longitude: STOCKHOLM.1, latitude: STOCKHOLM.0, zoom: 14.0, bearing: 0.0, pitch: 0.0, position: None };
        let scene = builder.build(&list, true, &view, None, NOW + 5000);
        assert_eq!(scene, TrainsScene { cars: true, ..TrainsScene::default() });
        // they still move close in
        let pos = builder.drawn().positions[0];
        assert!((metres(ll(STOCKHOLM), LatLon { lat: pos[1], lon: pos[0] }) - 50.0).abs() < 1e-6);
        // no trains: nothing at all
        assert_eq!(builder.build(&[], true, &view, None, NOW), TrainsScene::default());
        assert!(builder.drawn().is_empty());
    }

    #[test]
    fn a_train_with_one_frame_elsewhere_stays_at_its_report() {
        let mut t = train("one", STOCKHOLM, Some(45.0));
        t.frames = vec![(STOCKHOLM.0 + 0.001, STOCKHOLM.1, NOW)];
        let list = [&t];
        let mut motion = TrainMotion::new();
        motion.set_list(&list, NOW);
        motion.step(&list, NOW + 1000);
        assert!(!motion.drawn().moving(0));
        assert_eq!(motion.drawn().positions[0], [STOCKHOLM.1, STOCKHOLM.0, 0.0]);
        assert_eq!(motion.drawn().headings[0], Some(45.0));
        // eased onto it from elsewhere, it moves until the easing is done, then stays
        let was = ahead(STOCKHOLM, 0.0, -10.0);
        let mut first = train("one", was, Some(45.0));
        first.frames.clear();
        let mut motion = TrainMotion::new();
        motion.set_list(&[&first], NOW);
        motion.set_list(&list, NOW);
        assert!(motion.drawn().moving(0));
        motion.step(&list, NOW + 2000);
        // the single frame is where it is drawn once the easing ends
        assert_eq!(motion.drawn().positions[0], [STOCKHOLM.1, STOCKHOLM.0 + 0.001, 0.0]);
    }

    // --- cars ---

    /// Flat metres between two `[lon, lat]` points.
    fn apart(a: [f64; 2], b: [f64; 2]) -> f64 {
        metres(LatLon { lat: a[1], lon: a[0] }, LatLon { lat: b[1], lon: b[0] })
    }

    fn lonlat((lat, lon): (f64, f64)) -> [f64; 2] {
        [lon, lat]
    }

    /// The front and back of `car`: half its length ahead of and behind its middle.
    fn ends(car: &DrawnCar) -> ([f64; 2], [f64; 2]) {
        let at = (car.at[1], car.at[0]);
        (lonlat(ahead(at, car.heading, car.length / 2.0)), lonlat(ahead(at, car.heading, -car.length / 2.0)))
    }

    /// Wide bounds round Stockholm.
    const AROUND: Option<[f64; 4]> = Some([17.9, 59.2, 18.2, 59.4]);

    /// `list` taken in and stepped to `now`, and its cars there.
    fn cars_of(list: &[&Train], models: Option<&TrainModels>, now: i64) -> Vec<DrawnCar> {
        let mut motion = TrainMotion::new();
        motion.set_list(list, NOW);
        motion.step(list, now);
        drawn_cars(list, motion.drawn(), AROUND, models, now)
    }

    /// The operator models of `static/trains`.
    fn models() -> TrainModels {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../static/trains");
        let read = |name: &str| std::fs::read(dir.join(name)).expect("static/trains");
        TrainModels::parse(&read("index.json"), &read("meshes.bin")).expect("the models")
    }

    /// Running east at 10 m/s from Stockholm, its track known 200 m back, three cars.
    fn eastward() -> Train {
        let mut t = running("east", STOCKHOLM, 90.0, 10.0, NOW);
        t.tail = [0.0, 100.0, 200.0].map(|m| ahead(STOCKHOLM, 90.0, -m)).to_vec();
        t.cars = Some(3);
        t
    }

    #[test]
    fn cars_run_back_from_the_head_along_the_tail_front_car_first() {
        let t = eastward();
        let cars = cars_of(&[&t], None, NOW);
        assert_eq!(cars.len(), 3);
        // standing where its frames start: the front car's front there, each 25.4 m long and
        // 0.6 m behind the one before, pointing east, back along the tail
        for (k, car) in cars.iter().enumerate() {
            let (front, back) = ends(car);
            assert!((apart(front, lonlat(STOCKHOLM)) - 26.0 * k as f64).abs() < 1e-6, "car {k}: {car:?}");
            assert!((apart(back, lonlat(STOCKHOLM)) - (26.0 * k as f64 + 25.4)).abs() < 1e-6, "car {k}");
            assert!(back[0] < front[0] && back[0] < STOCKHOLM.1, "behind the head, to the west");
            assert!((car.heading - 90.0).abs() < 1e-6 && (car.length - 25.4).abs() < 1e-6);
            assert_eq!((car.train, car.modelled, car.level, car.color), (0, false, GROUND, [96, 212, 160, 240]));
        }
        // 5 s on, 50 m along its frames: the front car's front there
        let cars = cars_of(&[&t], None, NOW + 5000);
        let (front, _) = ends(&cars[0]);
        assert!(apart(front, lonlat(ahead(STOCKHOLM, 90.0, 50.0))) < 1e-6);
        // the last car's back 3 · 26 − 0.6 m behind it
        let (_, back) = ends(&cars[2]);
        assert!((apart(back, front) - (3.0 * 26.0 - 0.6)).abs() < 1e-6);
    }

    #[test]
    fn cars_follow_a_bend_in_the_tail_as_chords() {
        // the track behind runs 50 m west, then south
        let corner = ahead(STOCKHOLM, 90.0, -50.0);
        let mut t = eastward();
        t.tail = vec![STOCKHOLM, corner, ahead(corner, 0.0, -100.0)];
        let cars = cars_of(&[&t], None, NOW);
        // the first along the straight; the second across the corner, shorter (its chord) and
        // turned; the third along the run south, pointing north (to its front)
        assert!((cars[0].heading - 90.0).abs() < 1e-6);
        assert!(cars[1].length < 25.0 && cars[1].heading > 80.0 && cars[1].heading < 90.0, "{:?}", cars[1]);
        assert!(cars[2].heading.min(360.0 - cars[2].heading) < 1e-6 && (cars[2].length - 25.4).abs() < 1e-6, "{:?}", cars[2]);
        let (_, back) = ends(&cars[2]);
        assert!(back[1] < corner.0, "south of the corner");
    }

    #[test]
    fn as_many_cars_as_the_operator_says_else_four_at_least_one() {
        let mut t = eastward();
        for (cars, want) in [(Some(3), 3), (Some(12), 12), (None, 4), (Some(0), 1), (Some(1), 1)] {
            t.cars = cars;
            assert_eq!(cars_of(&[&t], None, NOW).len(), want, "{cars:?}");
        }
    }

    #[test]
    fn a_train_without_a_way_has_its_cars_in_a_row_back_along_its_heading() {
        // no frames, no tail: straight back from where it stands
        let mut t = train("still", STOCKHOLM, Some(90.0));
        t.cars = Some(2);
        let cars = cars_of(&[&t], None, NOW);
        assert_eq!(cars.len(), 2);
        let (front, _) = ends(&cars[0]);
        assert!(apart(front, lonlat(STOCKHOLM)) < 1e-6);
        for car in &cars {
            assert!((car.heading - 90.0).abs() < 1e-6 && (car.length - 25.4).abs() < 1e-6 && car.at[0] < STOCKHOLM.1);
        }
        // without a heading: back to the south, pointing north
        let mut t = train("box", STOCKHOLM, None);
        t.cars = Some(2);
        let cars = cars_of(&[&t], None, NOW);
        assert!(cars.iter().all(|c| c.heading.min(360.0 - c.heading) < 1e-6 && c.at[1] < STOCKHOLM.0), "{cars:?}");
        // a single frame elsewhere lays out no car (each would be a point there): the row too, from
        // where it is drawn
        let mut t = train("one", STOCKHOLM, Some(0.0));
        t.frames = vec![(STOCKHOLM.0 + 0.01, STOCKHOLM.1, NOW)];
        t.cars = Some(2);
        let cars = cars_of(&[&t], None, NOW);
        let (front, _) = ends(&cars[0]);
        assert!(cars.len() == 2 && apart(front, lonlat(STOCKHOLM)) < 1e-6, "{cars:?}");
    }

    #[test]
    fn only_the_trains_in_view_have_cars_and_none_without_bounds() {
        let a = eastward();
        let near = train("near", (59.405, 18.0), Some(0.0));
        let far = train("far", (59.42, 18.0), Some(0.0));
        let west = train("west", (59.3, 17.895), Some(0.0));
        let list = [&a, &near, &far, &west];
        let cars = cars_of(&list, None, NOW);
        // inside the bounds padded by 0.01°
        let trains: Vec<usize> = cars.iter().map(|c| c.train).collect();
        assert_eq!(trains, [0, 0, 0, 1, 1, 1, 1, 3, 3, 3, 3]);
        let mut motion = TrainMotion::new();
        motion.set_list(&list, NOW);
        assert!(drawn_cars(&list, motion.drawn(), None, None, NOW).is_empty(), "the globe");
        // the bounds of the view: the flat viewport's, none on the globe
        let view = ViewState { longitude: STOCKHOLM.1, latitude: STOCKHOLM.0, zoom: 16.0, bearing: 0.0, pitch: 0.0, position: None };
        let layers = car_layers(&list, motion.drawn(), &view, [800.0, 600.0], None, NOW);
        assert_eq!(layers.cars.iter().map(|c| c.train).collect::<Vec<_>>(), [0, 0, 0]);
        let globe = ViewState { zoom: 12.0, ..view };
        assert!(car_layers(&list, motion.drawn(), &globe, [800.0, 600.0], None, NOW).cars.is_empty());
    }

    #[test]
    fn plain_cars_take_the_trains_shape_and_colour() {
        let mut t = eastward();
        t.cars = Some(3);
        let keys = |t: &Train| cars_of(&[t], None, NOW).iter().map(|c| c.mesh.key()).collect::<Vec<_>>();
        assert_eq!(keys(&t), ["regional-cab", "regional-coach", "regional-coach"], "regional without a shape");
        for (shape, cab, coach) in [
            (TrainShape::HighSpeed, "highSpeed-cab", "regional-coach"),
            (TrainShape::DoubleDeck, "doubleDeck-cab", "doubleDeck-coach"),
            (TrainShape::Loco, "loco-cab", "regional-coach"),
            (TrainShape::Railcar, "railcar-cab", "regional-coach"),
        ] {
            t.shape = Some(shape);
            assert_eq!(keys(&t), [cab, coach, coach]);
        }
        let cars = cars_of(&[&t], None, NOW);
        assert_eq!(cars[0].mesh, CarShape::Plain { lead: true, shape: TrainShape::Railcar });
        assert_eq!(cars[1].mesh, CarShape::Plain { lead: false, shape: TrainShape::Railcar });
        // SJ's silver at the trains' alpha
        t.operator = Some("SJ".into());
        assert!(cars_of(&[&t], None, NOW).iter().all(|c| c.color == [232, 232, 236, 240]));
    }

    #[test]
    fn a_trains_operator_model_gives_its_cars_their_own_lengths_end_to_end_in_white() {
        let models = models();
        let mut t = eastward();
        t.tail = [0.0, 200.0].map(|m| ahead(STOCKHOLM, 90.0, -m)).to_vec();
        t.model = Some("x2000".into());
        t.cars = Some(5);
        let cars = cars_of(&[&t], Some(&models), NOW);
        let keys: Vec<String> = cars.iter().map(|c| c.mesh.key()).collect();
        assert_eq!(keys, ["x2000-0", "x2000-1", "x2000-1", "x2000-1", "x2000-2"]);
        assert_eq!(cars[4].mesh, CarShape::Model { model: "x2000".into(), index: 2 });
        let lengths: Vec<f64> = cars.iter().map(|c| c.length).collect();
        for (l, want) in lengths.iter().zip([17.7, 25.0, 25.0, 25.0, 22.7]) {
            assert!((l - want).abs() < 1e-6, "{lengths:?}");
        }
        // no gap between them: each front where the car before ends
        for pair in cars.windows(2) {
            assert!(apart(ends(&pair[0]).1, ends(&pair[1]).0) < 1e-6);
        }
        assert!(cars.iter().all(|c| c.modelled && c.color == MODEL_WHITE));
        // a one-car train with a model: its lead car
        t.cars = Some(1);
        assert_eq!(cars_of(&[&t], Some(&models), NOW)[0].mesh.key(), "x2000-0");
        // models not loaded, or a model they lack: plain cars
        t.cars = Some(2);
        assert_eq!(cars_of(&[&t], None, NOW)[0].mesh.key(), "regional-cab");
        t.model = Some("nothing".into());
        let cars = cars_of(&[&t], Some(&models), NOW);
        assert!(!cars[0].modelled && cars[0].color[3] == 240 && (cars[0].length - 25.4).abs() < 1e-6);
    }

    #[test]
    fn each_car_has_its_own_level_faint_in_a_tunnel_raised_on_a_bridge() {
        let line = |from: f64, to: f64| vec![lonlat(ahead(STOCKHOLM, 90.0, -from)), lonlat(ahead(STOCKHOLM, 90.0, -to))];
        // a tunnel 30 to 60 m back: the second car (its middle 38.7 m back) is in it
        let mut t = eastward();
        t.structures = vec![Structure { tunnel: true, line: line(30.0, 60.0), joined: [false, false] }];
        let cars = cars_of(&[&t], None, NOW);
        assert_eq!(cars.iter().map(|c| c.level.tunnel).collect::<Vec<_>>(), [false, true, false]);
        assert_eq!(cars.iter().map(|c| c.color[3]).collect::<Vec<_>>(), [240, 90, 240]);
        // a bridge 30 to 130 m back: the second car on its ramp, the third on its deck
        t.structures = vec![Structure { tunnel: false, line: line(30.0, 130.0), joined: [false, false] }];
        let cars = cars_of(&[&t], None, NOW);
        assert_eq!(cars[0].level, GROUND);
        assert!(cars[1].level.height > 0.0 && cars[1].level.height < 6.0, "{:?}", cars[1].level);
        assert_eq!(cars[2].level, Level { tunnel: false, height: 6.0 });
        assert!(cars.iter().all(|c| c.color[3] == 240));
        // a model's car in a tunnel: white, faint
        let mut m = eastward();
        m.model = Some("x40".into());
        m.structures = vec![Structure { tunnel: true, line: line(0.0, 200.0), joined: [false, false] }];
        assert!(cars_of(&[&m], Some(&models()), NOW).iter().all(|c| c.color == [255, 255, 255, 90]));
    }

    #[test]
    fn cars_are_never_thinner_than_three_pixels() {
        let close = CarLayers::new(Vec::new(), 0.6);
        assert_eq!((close.width, close.height), (3.1, 4.1));
        let far = CarLayers::new(Vec::new(), 2.0);
        assert_eq!((far.width, far.height), (6.0, 3.0 * 1.3 * 2.0));
        // at zoom 12.3 over Stockholm (8 m/px), a 3.1 m car would be 0.4 px wide
        let mpp = meters_per_pixel(12.3, STOCKHOLM.0);
        let layers = CarLayers::new(Vec::new(), mpp);
        assert!((layers.width / mpp - 3.0).abs() < 1e-12 && (layers.height / mpp - 3.9).abs() < 1e-12);
    }

    #[test]
    fn a_layer_per_mesh_in_order_of_its_first_car_tunnels_after() {
        let mut regional = eastward();
        regional.cars = Some(4);
        let mut fast = running("fast", ahead(STOCKHOLM, 0.0, 300.0), 90.0, 10.0, NOW);
        fast.tail = [0.0, 200.0].map(|m| ahead(ahead(STOCKHOLM, 0.0, 300.0), 90.0, -m)).to_vec();
        fast.shape = Some(TrainShape::HighSpeed);
        fast.cars = Some(3);
        let a = ahead(STOCKHOLM, 0.0, 300.0);
        // its second car (38.7 m back) in a tunnel, its third (64.7 m back) out again
        fast.structures = vec![Structure { tunnel: true, line: vec![lonlat(ahead(a, 90.0, -30.0)), lonlat(ahead(a, 90.0, -50.0))], joined: [false, false] }];
        let layers = CarLayers::new(cars_of(&[&regional, &fast], None, NOW), 0.6);
        let ids: Vec<(String, Vec<usize>)> = layers.layers().into_iter().map(|l| (l.id, l.cars)).collect();
        assert_eq!(
            ids,
            [
                ("train-car-meshes-regional-cab".to_string(), vec![0]),
                ("train-car-meshes-regional-coach".to_string(), vec![1, 2, 3, 6]),
                ("train-car-meshes-regional-coach-tunnel".to_string(), vec![5]),
                ("train-car-meshes-highSpeed-cab".to_string(), vec![4]),
            ]
        );
        let tunnel = &layers.layers()[2];
        assert!(tunnel.tunnel && tunnel.key == "regional-coach" && tunnel.mesh == CarShape::Plain { lead: false, shape: TrainShape::Regional });
        assert!(CarLayers::default().layers().is_empty());
    }

    #[test]
    fn close_in_the_builder_lays_out_the_cars_where_it_drew_the_trains() {
        let t = eastward();
        let list = [&t];
        let mut builder = TrainBuilder::new();
        let view = ViewState { longitude: STOCKHOLM.1, latitude: STOCKHOLM.0, zoom: 16.0, bearing: 0.0, pitch: 0.0, position: None };
        let scene = builder.build(&list, true, &view, None, NOW + 2000);
        assert!(scene.cars && scene.car_layers.cars.is_empty());
        let layers = builder.car_layers(&list, &view, [800.0, 600.0], None, NOW + 2000);
        assert_eq!(layers.cars.len(), 3);
        let (front, _) = ends(&layers.cars[0]);
        let drawn = builder.drawn().positions[0];
        assert!(apart(front, [drawn[0], drawn[1]]) < 1e-6, "the front car's front where the train is drawn");
        assert_eq!((layers.width, layers.height), (3.1, 4.1));
    }

    #[test]
    fn headings_read_as_javascript_reads_the_report() {
        let t = train("h", STOCKHOLM, Some(12.3));
        let mut motion = TrainMotion::new();
        motion.set_list(&[&t], NOW);
        assert_eq!(motion.drawn().headings[0], Some(12.3), "not 12.300000190734863");
    }
}
