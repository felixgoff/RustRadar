//! Hover and click on the map (item A9; doc 03 §10, doc 01 §7): what is under the pointer, the
//! hover highlight, the cursor, the click rules and the hover card's data with its details loader.
//! A port of `createDeck`'s `onHover`, `onDragStart`, `onClick` and `getCursor` (G:3713-3736),
//! `setHover`, `hovered` and `hoverCard` (G:3424-3474), and of how deck 9.4 runs them
//! (`Deck._onPointerMove`, `_pickAndCallback`, `_onPointerDown`, `_onEvent`, `processPickInfo`).
//!
//! **What is pickable** ([`picks`]): deck's picking pass draws only its pickable layers, in layer
//! order, each under its own depth test, and keeps the closest object within 6 px
//! ([`rustradar_scene::pick`]). Of the layers drawn today: the place and airport labels (row 18 of
//! doc 03 §3, depth `always`), each aircraft group's airframes (row 26) and the icons (row 28)
//! ([`rustradar_scene::aircraft::AircraftScene::picks`]); before them the signals (row 12, depth
//! `always`, [`rustradar_scene::blocks::BlocksScene::picks`]) and the trains' icons or cars (row
//! 15, [`rustradar_scene::trains::TrainsScene::picks`]), item T8; before those the ships' models
//! and icons (rows 9–11, [`rustradar_scene::ships::ShipsScene::picks`]), item H4. Nothing else is
//! pickable (gear, shadows, outlines, aircraft labels, trail, route, markers, rings, the ships'
//! halo and ring, the blocks' lines, the trains' route and ring). The scene's world positions are projected with the frame's
//! camera, as deck picks the layers last drawn under the current viewport; the pixel ratio is the
//! map's scale (`useDevicePixels`).
//!
//! **Hover** ([`AppState::pick_frame`], once a frame, as `_pickAndCallback` runs once per animation
//! frame on the last pointer move):
//! - only when the pointer moved since the last frame and no button is held (deck's
//!   `leftButton || rightButton` returns before asking); a pointer that left the map, or went over
//!   the UI (a panel takes the events from the canvas: `pointerleave`), picks nothing;
//! - `cursorState.isHovering`: something was picked, or something was at the last pick (deck's
//!   leave event for the layer just left counts once), [`Pick::hovering`] for the cursor
//!   (`grabbing` while dragging, `pointer` over a pickable thing, else `grab`: `MapInput::cursor`);
//! - `onHover`: the flight under the pointer when it is an aircraft, else none; `setHover(flight,
//!   x, y)`, and when the flight's id changed, `hoveredId` ([`Pick::hovered_id`], the highlight the
//!   aircraft builder draws) and an immediate scene refresh (`render()`);
//! - `onDragStart` (a drag the controller took): `setHover(null)`: the card goes, the highlight
//!   stays until the pointer moves with no button held;
//! - the train cars' `autoHighlight` (item T8): the car picked ([`Pick::hovered_car`], which the
//!   trains' scene builder highlights), none when the pointer is off every car; a change refreshes
//!   the scene at once, as deck redraws the highlight at once; the same for the ship models' (item
//!   H4, [`Pick::hovered_ship_model`]; the ships' scene builder leaves it off while a ship is
//!   hovered in the sidebar's list);
//! - deck's tooltip (item T8, [`crate::ui::tooltip`]): `getTooltip` of what was picked, at the
//!   pointer ([`Pick::tooltip`]): a ship's ([`AppState::picked_ship`], item H4), a train's
//!   ([`AppState::picked_train`]: its row of the list the map draws), a signal's (the block
//!   layers' words for it), an airport code's (its feature's name), none for anything else;
//!   after the hover pick, a camera that moved since the last frame hides it until the next hover
//!   pick (`TooltipWidget.onViewportChange`).
//!
//! **Click** (`onClick`). deck hands a click the object picked where the press went down
//! (`_onPointerDown` keeps `_lastPointerDownInfo`; `_onEvent` passes it with the click's own `x, y`),
//! not what is under the release 300 ms later: the app calls [`AppState::pick_press`] for each press
//! the map takes. Every click the window recognises follows such a press; one handed in without a
//! press (tests) picks at its own position. Then, in order:
//! 1. an aircraft → `onselect(flight)`: the searched place cleared, then `select(id)` (a click
//!    does not fly; the same flight again asks a stream that ended again);
//! 2. a ship → `onshipselect(ship)` (item H4): the place cleared, then `selectShip(ship)` (its row
//!    of the list the map draws, [`AppState::picked_ship`]);
//! 3. a train → `ontrainselect(train)`: the place cleared, then `selectTrain(train)` (its row of the
//!    list the map draws);
//! 4. a label: an airport with a code → `onairport(code)` (`openAirport`: the board's code, here
//!    the flight state's focused airport until item U13 has the board, and the selected flight let
//!    go); a place: nothing (no deselect);
//! 5. nothing at all → `onselect(null)`: the place cleared, the ship, the train and the flight let
//!    go.
//!
//! A click on a signal (pickable, so the cursor points) does nothing.
//! Both clicks of a double click reach `onClick` (deck's `dblclick` handler is `onClick` too).
//!
//! **The hover card** ([`AppState::hover_card`], for item U18): the hovered flight's latest row in
//! the flights shown (else the one picked), its details from the selected flight's own stream when
//! it is the selected one (`ownDetails`), else from the [`HoverLoader`] (`core::hover`: a 300 ms
//! dwell, one request at a time, cached), its origin's and destination's cities, and the progress
//! (`progress_pct` when measured, else `routeProgress`). [`Pick::poll`] runs the loader's dwell
//! timer and requests (`flight_details` through the backend, the `--fixture` folder's
//! `flight_details/{id}.json`, or none).
//!
//! **What the app wires** (`app.rs`): [`AppState::pick_press`] for each press the map takes,
//! [`AppState::pick_frame`] each frame after the click timers and the camera step and before the
//! scene refresh (its [`Wants`] force the refresh and wake the loop when the card's dwell is up),
//! [`Pick::hovering`] into `MapInput::set_cursor`; the aircraft scene builder reads
//! [`Pick::hovered_id`], the trains' [`Pick::hovered_car`], the ships'
//! [`Pick::hovered_ship_model`]; the UI pass draws [`Pick::tooltip`] (`ui::tooltip::ui`). The card
//! (U18) takes [`AppState::hover_card`] before the egui pass: reading the loader's cache refreshes
//! its recency, as `info` and `status` do in `hover-details.ts`, so it
//! needs the state mutably.
//!
//! Deviations from today's app:
//! - A hover pick runs on the frame after the pointer moved, with the pointer where it is then
//!   (deck runs it in the next animation frame too, on the last move's position).
//! - deck forgets a press's object when its layer is gone by the click (a level of detail swapped
//!   in between: the click then deselects); here the press's target stands.
//! - The picked flight's row is looked up by id when the pick lands (the selected row, else the
//!   feed's): an aircraft whose flight left the feed since the last scene refresh hovers as nothing.
//! - Icons are pickable from the first scene, also in the few milliseconds the icon atlas takes to
//!   transcode at start-up, when deck has no icon layer yet.

use std::path::PathBuf;

use rustradar_backend::{Backend, convert};
use rustradar_core::hover::{HoverInfo, HoverLoader, HoverStatus, hover_info, route_progress};
use rustradar_core::model::{FlightDetails, LiveFlight, Ship, Train, f32_as_js};
use rustradar_map::SceneInput;
use rustradar_scene::camera::{ViewState, Viewport, viewport_for};
use rustradar_scene::pick::PickList;
use rustradar_scene::ships::{ModelPick, ShipPick};
use rustradar_scene::trains::{CarPick, TrainPick};

use super::aircraft::{Answer, NO_BACKEND};
use super::flight::{FIXTURE_DETAILS, SelectOptions};
use super::{AppState, Cx, Wants};
use crate::input::Click;
use crate::requests::Requests;
use crate::ui::tooltip::{Shown, Tip};

/// What a pick lands on, as the app's handlers tell layers apart (`info.layer`, `info.object`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An aircraft's icon or airframe (`aircraft-*`): its flight's id.
    Aircraft(u32),
    /// A place name or an airport code (`labels`): the airport's code, `None` for a place.
    Label(Option<String>),
    /// A ship's icon or model (`ship-icons`, `ship-mesh-*`, item H4).
    Ship(ShipPick),
    /// A train's icon or one of its cars (`train-icons`, `train-car-meshes-*`, item T8).
    Train(TrainPick),
    /// A signal (`train-signals`, item T8): its place in the block set's signals. A tooltip only.
    Signal(usize),
}

/// The pickable things of `scene` laid out in `viewport`, in deck's draw order: the ships' models
/// and icons (rows 9–11), the signals (row 12), the trains' icons or cars (row 15), the labels as
/// the labels layer last drew them (row 18), then the aircraft's airframes and icons (rows 26, 28).
pub fn picks(scene: &SceneInput, viewport: &Viewport) -> PickList<Target> {
    let mut list: PickList<Target> = scene.ships.scene.picks(viewport).into_iter().map(|item| item.map_key(Target::Ship)).collect();
    // the signals where they are drawn, on the 3D terrain raised by the ground under them (G3)
    let ground = &scene.stands_on;
    list.extend(scene.blocks.scene.picks_on(viewport, |at| ground.at(at)).into_iter().map(|item| item.map_key(Target::Signal)));
    let models = rustradar_map::layers::train_cars::models();
    list.extend(scene.trains.scene.picks(viewport, models).into_iter().map(|item| item.map_key(Target::Train)));
    list.extend(scene.labels.drawn.picks().into_iter().map(|item| item.map_key(Target::Label)));
    list.extend(scene.aircraft.scene.picks(viewport).into_iter().map(|item| item.map_key(Target::Aircraft)));
    list
}

/// What deck picks in `scene` at `pointer` (CSS pixels from the map's top left) with the camera at
/// `view` on a `size` CSS-pixel canvas, at the scene's map scale.
pub fn pick_at(scene: &SceneInput, view: &rustradar_scene::camera::ViewState, size: [f64; 2], pointer: [f64; 2]) -> Option<Target> {
    let viewport = viewport_for(view, size);
    let list = picks(scene, &viewport);
    list.pick(&viewport, pointer, scene.map_scale).map(|hit| hit.key.clone())
}

/// The progress bar of the hover card: how far along, and whether Flightradar24 measured it or it
/// is the great-circle estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    /// 0–1 (the card clamps it).
    pub value: f64,
    pub measured: bool,
}

/// The hover card's data (`hoverCard`, G:3451-3474), for item U18.
#[derive(Debug, Clone, PartialEq)]
pub struct HoverCard {
    /// The hovered flight's latest row.
    pub flight: LiveFlight,
    /// `origin?.city || flight.origin`, `destination?.city || flight.destination`.
    pub from: String,
    pub to: String,
    pub progress: Option<Progress>,
    pub info: Option<HoverInfo>,
    pub status: HoverStatus,
    /// Where the pointer was over the flight (`pointer`), CSS pixels: the card is placed from it.
    pub pointer: [f64; 2],
}

/// Where the hover card's details come from: the backend, the `--fixture` folder, or a test's.
pub trait HoverFeed: Send {
    /// `flightDetails(id)`.
    fn flight_details(&mut self, id: u32) -> Answer<Result<FlightDetails, String>>;
}

/// The backend's `flight_details`, on its runtime.
struct BackendDetails {
    backend: Backend,
    requests: Requests,
}

impl HoverFeed for BackendDetails {
    fn flight_details(&mut self, id: u32) -> Answer<Result<FlightDetails, String>> {
        let backend = self.backend.clone();
        self.requests.request(async move { backend.flight_details(id).await.map(convert::flight_details) }).into()
    }
}

/// The `--fixture` folder's `flight_details/{id}.json` (the selected flight's stream reads the same
/// file), read when asked for.
struct FixtureDetails {
    dir: PathBuf,
}

impl HoverFeed for FixtureDetails {
    fn flight_details(&mut self, id: u32) -> Answer<Result<FlightDetails, String>> {
        let path = self.dir.join(format!("{FIXTURE_DETAILS}/{id}.json"));
        Answer::ready(
            std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| serde_json::from_slice::<FlightDetails>(&bytes).map_err(|e| e.to_string()))
                .map_err(|e| format!("fixture {}: {e}", path.display())),
        )
    }
}

/// Which feed the hover card's details come from.
enum Source {
    /// Chosen at the first request: the fixture folder, else the backend, else none.
    Auto,
    /// None: every request fails at once (`flightDetails` outside the app).
    Off,
    On(Box<dyn HoverFeed>),
}

/// Hover and click on the map: what is under the pointer, the hover highlight and card, the click
/// rules.
pub struct Pick {
    /// `hoveredId`: the aircraft under the pointer, drawn highlighted.
    hovered_id: Option<u32>,
    /// `hoverFlight`: the flight the card is about (none once a drag starts).
    hover_flight: Option<LiveFlight>,
    /// `pointer`: where the pointer last was over that flight.
    card_at: [f64; 2],
    /// `cursorState.isHovering`.
    hovering: bool,
    /// Whether the last hover pick picked something (deck's `lastPickedInfo.layerId` set).
    picked_last: bool,
    /// The pointer over the map at the last frame (`None`: off the map or over the UI).
    seen: Option<[f64; 2]>,
    /// `_lastPointerDownInfo`: what the last press on the map picked (`Some(None)`: nothing).
    pressed: Option<Option<Target>>,
    /// Whether the controller was dragging at the last frame (`onDragStart`).
    dragging: bool,
    loader: HoverLoader,
    source: Source,
    /// The details request in flight, for this flight.
    request: Option<(u32, Answer<Result<FlightDetails, String>>)>,
    /// The map's tooltip (item T8): what the last hover pick showed, until the camera moves.
    tooltip: Option<Shown>,
    /// The camera and canvas at the last frame (`TooltipWidget.lastViewport`).
    last_view: Option<(ViewState, [f64; 2])>,
    /// The car under the pointer, highlighted (deck's `autoHighlight` on the car layers).
    hovered_car: Option<CarPick>,
    /// The ship model under the pointer, highlighted (deck's `autoHighlight` on the model layers).
    hovered_ship_model: Option<ModelPick>,
}

impl Default for Pick {
    fn default() -> Self {
        Pick::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pick")
            .field("hovered_id", &self.hovered_id)
            .field("hover_flight", &self.hover_flight.as_ref().map(|f| f.id))
            .field("hovering", &self.hovering)
            .field("pressed", &self.pressed)
            .field("request", &self.request.as_ref().map(|(id, _)| id))
            .finish_non_exhaustive()
    }
}

impl Pick {
    fn with_source(source: Source) -> Self {
        Pick {
            hovered_id: None,
            hover_flight: None,
            card_at: [0.0; 2],
            hovering: false,
            picked_last: false,
            seen: None,
            pressed: None,
            dragging: false,
            loader: HoverLoader::default(),
            source,
            request: None,
            tooltip: None,
            last_view: None,
            hovered_car: None,
            hovered_ship_model: None,
        }
    }

    /// The map's tooltip to draw this frame, and where (item T8, [`crate::ui::tooltip`]).
    pub fn tooltip(&self) -> Option<&Shown> {
        self.tooltip.as_ref()
    }

    /// The train car under the pointer (deck's `autoHighlight`), which the trains' scene builder
    /// highlights.
    pub fn hovered_car(&self) -> Option<&CarPick> {
        self.hovered_car.as_ref()
    }

    /// The ship model under the pointer (deck's `autoHighlight`), which the ships' scene builder
    /// highlights (item H4).
    pub fn hovered_ship_model(&self) -> Option<&ModelPick> {
        self.hovered_ship_model.as_ref()
    }

    /// The hover card's details from `feed` instead of the fixture folder or the backend; `None`:
    /// from nowhere (every request fails at once).
    pub fn with_feed(feed: Option<Box<dyn HoverFeed>>) -> Self {
        Pick::with_source(match feed {
            Some(feed) => Source::On(feed),
            None => Source::Off,
        })
    }

    /// The aircraft under the pointer (`hoveredId`), which the aircraft layers highlight.
    pub fn hovered_id(&self) -> Option<u32> {
        self.hovered_id
    }

    /// Whether the pointer is over something pickable (`cursorState.isHovering`): the cursor is a
    /// pointing hand then.
    pub fn hovering(&self) -> bool {
        self.hovering
    }

    /// The flight the hover card is about (`hoverFlight`), as it was picked.
    pub fn hover_flight(&self) -> Option<&LiveFlight> {
        self.hover_flight.as_ref()
    }

    /// What the last press on the map picked: `None` before any press, `Some(None)` for nothing.
    pub fn pressed(&self) -> Option<&Option<Target>> {
        self.pressed.as_ref()
    }

    /// This frame's step (see [`super::AppState::poll`] for the order): the details loader's dwell
    /// timer and its answers.
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = cx.clocks.local_ms as i64;
        if let Some(id) = self.loader.tick(now) {
            self.fetch(id, cx, now);
        }
        // `then` / `catch` / `finally`: an answer, and the flight waiting behind it
        while let Some((id, answer)) = &self.request {
            let Some(result) = answer.poll() else { break };
            let id = *id;
            self.request = None;
            if let Some(next) = self.loader.settled(id, result.as_ref().map_err(String::as_str), now) {
                self.fetch(next, cx, now);
            }
        }
        if let Some(due) = self.loader.deadline() {
            cx.wants.wake_at(due as f64);
        }
    }

    /// Asks for `id`'s details (`fetch(id)`); without a feed the request fails at once.
    fn fetch(&mut self, mut id: u32, cx: &Cx<'_>, now: i64) {
        if matches!(self.source, Source::Auto) {
            self.source = if let Some(dir) = cx.startup.fixture.as_deref() {
                Source::On(Box::new(FixtureDetails { dir: dir.to_path_buf() }))
            } else if let (Some(backend), Some(requests)) = (cx.backend, cx.requests) {
                Source::On(Box::new(BackendDetails { backend: backend.clone(), requests: requests.clone() }))
            } else {
                Source::Off
            };
        }
        loop {
            match &mut self.source {
                Source::On(feed) => {
                    self.request = Some((id, feed.flight_details(id)));
                    return;
                }
                Source::Auto | Source::Off => match self.loader.settled(id, Err(NO_BACKEND), now) {
                    Some(next) => id = next,
                    None => return,
                },
            }
        }
    }
}

impl AppState {
    /// The row a pick of flight `id` stands for (`info.object`): the selected row when it is the
    /// selected flight, else the feed's.
    fn picked_flight(&self, id: u32) -> Option<LiveFlight> {
        match self.flight.selected() {
            Some(selected) if selected.id == id => Some(selected.clone()),
            _ => self.aircraft.flight(id).cloned(),
        }
    }

    /// `ownDetails(id)`: the selected flight's streamed details, when `id` is the selected flight.
    fn own_details(&self, id: u32) -> Option<&FlightDetails> {
        if self.flight.selected_id() == Some(id) { self.flight.details() } else { None }
    }

    /// `setHover(flight, x, y)` at local time `now_ms`.
    fn set_hover(&mut self, flight: Option<LiveFlight>, at: [f64; 2], now_ms: f64) {
        if flight.is_some() {
            self.pick.card_at = at;
        }
        if flight.as_ref().map(|f| f.id) == self.pick.hover_flight.as_ref().map(|f| f.id) {
            return;
        }
        let ask = flight.as_ref().map(|f| f.id).filter(|&id| self.own_details(id).is_none());
        self.pick.loader.hover(ask, now_ms as i64);
        self.pick.hover_flight = flight;
    }

    /// A press the map took, at `at` (CSS pixels): what deck picks there for the click that may
    /// follow (`_onPointerDown`), in the scene the map draws and the camera now.
    pub fn pick_press(&mut self, scene: &SceneInput, at: [f64; 2]) {
        let target = pick_at(scene, &self.controller.view(), self.controller.size(), at);
        self.pick.pressed = Some(target);
    }

    /// This frame's hover and clicks (see the module documentation), against the scene the map
    /// draws: `pointer` is where the mouse is over the map (`None` off it or over the UI),
    /// `buttons_down` whether a button or a finger the map took is down, `clicks` the clicks and
    /// taps recognised since the last frame. Asks for a scene refresh when the highlight or the
    /// selection changed.
    pub fn pick_frame(&mut self, scene: &SceneInput, pointer: Option<[f64; 2]>, buttons_down: bool, clicks: &[Click], local_ms: f64) -> Wants {
        let mut wants = Wants::default();
        // onDragStart: no card while dragging
        let dragging = self.controller.is_dragging();
        if dragging && !self.pick.dragging {
            self.set_hover(None, [0.0; 2], local_ms);
        }
        self.pick.dragging = dragging;

        for click in clicks {
            let target = match &self.pick.pressed {
                Some(pressed) => pressed.clone(),
                None => pick_at(scene, &self.controller.view(), self.controller.size(), click.at),
            };
            wants.scene |= self.click_map(target, local_ms);
        }

        // a hover pick when the pointer moved with no button held, or left
        if pointer != self.pick.seen {
            self.pick.seen = pointer;
            let picked = match pointer {
                None => Some(None),
                Some(_) if buttons_down => None,
                Some(at) => Some(pick_at(scene, &self.controller.view(), self.controller.size(), at)),
            };
            if let Some(target) = picked {
                // the tooltip widget's `onHover`: `getTooltip(info)` at the pointer
                let at = pointer.unwrap_or([-1.0, -1.0]);
                self.pick.tooltip = target.as_ref().and_then(|t| self.tooltip_of(t, scene, local_ms)).map(|tip| Shown { tip, at });
                wants.scene |= self.hover(target, at, local_ms);
            }
        }
        // `onViewportChange`: the camera moved since the last frame, so the tooltip goes (it comes
        // back at the next hover pick)
        let view = (self.controller.view(), self.controller.size());
        if self.pick.last_view.is_some_and(|last| last != view) {
            self.pick.tooltip = None;
        }
        self.pick.last_view = Some(view);
        // a dwell timer the hover just set: the frame that asks for the details comes on time
        if let Some(due) = self.pick.loader.deadline() {
            wants.wake_at(due as f64);
        }
        wants
    }

    /// deck's hover callbacks for a pick of `target` at `at`: the cursor's state, `onHover`.
    /// Returns whether the highlight changed (`render()`).
    fn hover(&mut self, target: Option<Target>, at: [f64; 2], local_ms: f64) -> bool {
        // the layer just left gets its leave event, which counts as a result too
        self.pick.hovering = target.is_some() || self.pick.picked_last;
        self.pick.picked_last = target.is_some();
        // a car layer's `autoHighlight`: the car picked, none once the pointer leaves it (the
        // highlight is drawn by the next scene refresh, so it is asked for now)
        let car = match &target {
            Some(Target::Train(pick)) => pick.car.clone(),
            _ => None,
        };
        let car_changed = car != self.pick.hovered_car;
        self.pick.hovered_car = car;
        // the same for a ship model's (H4)
        let model = match &target {
            Some(Target::Ship(pick)) => pick.model,
            _ => None,
        };
        let model_changed = model != self.pick.hovered_ship_model;
        self.pick.hovered_ship_model = model;
        let flight = match target {
            Some(Target::Aircraft(id)) => self.picked_flight(id),
            _ => None,
        };
        let id = flight.as_ref().map(|f| f.id);
        self.set_hover(flight, at, local_ms);
        if id != self.pick.hovered_id {
            self.pick.hovered_id = id;
            return true;
        }
        car_changed || model_changed
    }

    /// The ship a pick of the ship layers stands for (`pickedShip`, item H4): its row of the list
    /// the map draws, found by its place there and checked by its MMSI (else found by its MMSI,
    /// the list having changed since the scene was drawn).
    pub fn picked_ship(&self, pick: &ShipPick) -> Option<Ship> {
        let on_map = self.ships.on_map();
        match on_map.get(pick.ship) {
            Some(ship) if ship.mmsi == pick.mmsi => Some(ship.clone()),
            _ => on_map.iter().find(|s| s.mmsi == pick.mmsi).cloned(),
        }
    }

    /// The train a pick of the train layers stands for (`pickedTrain`): its row of the list the
    /// map draws, found by its place there and checked by its id (else found by its id, the list
    /// having changed since the scene was drawn).
    pub fn picked_train(&self, pick: &TrainPick) -> Option<Train> {
        let on_map = self.trains.on_map();
        match on_map.get(pick.train) {
            Some(train) if train.id == pick.id => Some(train.clone()),
            _ => on_map.iter().find(|t| t.id == pick.id).cloned(),
        }
    }

    /// `getTooltip(info)` (G:3381-3422) for a pick of `target` in `scene` at local time `local_ms`:
    /// a ship's, a train's, a signal's or an airport code's tooltip ([`Tip`]); none for an aircraft
    /// (it has the hover card) or a place's name.
    fn tooltip_of(&self, target: &Target, scene: &SceneInput, local_ms: f64) -> Option<Tip> {
        match target {
            // icons and models alike (H4)
            Target::Ship(pick) => self.picked_ship(pick).map(|ship| Tip::ship(&ship)),
            // `trainPositionAge(train.positionUnix, Date.now())`
            Target::Train(pick) => self.picked_train(pick).map(|train| Tip::train(&train, local_ms.floor() as i64)),
            Target::Signal(index) => scene.blocks.scene.signal_tip(*index).map(|tip| Tip::signal(&tip)),
            // an `aerodrome_label`: its feature's name (`nameOf(p)`)
            Target::Label(Some(code)) => scene.labels.drawn.labels().into_iter().find(|l| l.iata.as_ref() == Some(code)).map(|l| Tip::airport(&l.name)),
            Target::Label(None) | Target::Aircraft(_) => None,
        }
    }

    /// `onClick` with what the press picked (`None`: nothing pickable). Returns whether the
    /// selection or the marks on the map changed.
    pub fn click_map(&mut self, target: Option<Target>, local_ms: f64) -> bool {
        let before = (self.flight.selected_id(), self.airport.code().map(str::to_owned), self.flight.place().is_some(), self.trains.selected_id().map(str::to_owned), self.ships.selected_mmsi());
        match target {
            // onselect(flight): `place = null; select(f.id)`
            Some(Target::Aircraft(id)) => {
                self.flight.set_place(None);
                self.select_flight(Some(id), SelectOptions::default(), local_ms);
            }
            // onairport(iata): `openAirport` (`p.iata` is truthy)
            Some(Target::Label(Some(code))) if !code.is_empty() => self.open_airport(&code, local_ms),
            // a place's name: nothing, not even a deselect
            Some(Target::Label(_)) => {}
            // onshipselect(ship): `place = null; selectShip(s)` (H4)
            Some(Target::Ship(pick)) => {
                if let Some(ship) = self.picked_ship(&pick) {
                    self.flight.set_place(None);
                    self.select_ship(Some(ship), local_ms);
                }
            }
            // ontrainselect(train): `place = null; selectTrain(t)` (T8)
            Some(Target::Train(pick)) => {
                if let Some(train) = self.picked_train(&pick) {
                    self.flight.set_place(None);
                    self.select_train(Some(train), local_ms);
                }
            }
            // a signal: pickable, but a click does nothing
            Some(Target::Signal(_)) => {}
            // onselect(null): `place = null; selectShip(null); selectTrain(null); select(null)`
            None => {
                self.flight.set_place(None);
                self.select_ship(None, local_ms);
                self.select_train(None, local_ms);
                self.select_flight(None, SelectOptions::default(), local_ms);
            }
        }
        before != (self.flight.selected_id(), self.airport.code().map(str::to_owned), self.flight.place().is_some(), self.trains.selected_id().map(str::to_owned), self.ships.selected_mmsi())
    }

    /// The hover card's data now (`hoverCard`), or `None` when no flight is hovered.
    pub fn hover_card(&mut self, local_ms: f64) -> Option<HoverCard> {
        let held = self.pick.hover_flight.as_ref()?;
        // the latest report under a resting pointer
        let shown = self.aircraft.shown(self.flight.selected());
        let flight = shown.iter().find(|f| f.id == held.id).unwrap_or(held).clone();
        let now = local_ms as i64;
        let (info, status) = match self.own_details(flight.id) {
            Some(details) => (Some(hover_info(details)), HoverStatus::Ready),
            None => (self.pick.loader.info(flight.id, now).cloned(), self.pick.loader.status(flight.id, now)),
        };
        let reference = self.aircraft.reference();
        let airport = |code: &str| reference.and_then(|r| r.airport(Some(code)));
        let (origin, destination) = (airport(&flight.origin), airport(&flight.destination));
        let city_or = |a: Option<&rustradar_core::model::Airport>, code: &str| match a {
            Some(a) if !a.city.is_empty() => a.city.clone(),
            _ => code.to_owned(),
        };
        let progress = match info.as_ref().and_then(|i| i.progress_pct) {
            Some(pct) => Some(Progress { value: f64::from(pct) / 100.0, measured: true }),
            None => route_progress(
                [f32_as_js(flight.lon), f32_as_js(flight.lat)],
                origin.map(|a| [a.lon, a.lat]),
                destination.map(|a| [a.lon, a.lat]),
            )
            .map(|value| Progress { value, measured: false }),
        };
        Some(HoverCard {
            from: city_or(origin, &flight.origin),
            to: city_or(destination, &flight.destination),
            progress,
            info,
            status,
            pointer: self.pick.card_at,
            flight,
        })
    }
}

// the scene builder's tests include the same file: each test module gets its own copy
#[cfg(test)]
#[path = "../../tests/common/fixtures.rs"]
#[allow(clippy::duplicate_mod)]
mod fixtures;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_core::model::{AdsbSnapshot, BoundingBox, FlightProgress, FollowEvent, LiveSnapshot, OpenSkyPoint, Runway, SessionInfo, TopFlight};
    use rustradar_scene::aircraft::icon_pick_radius;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::input::Modifiers;
    use crate::scene_builder::SceneBuilder;
    use crate::state::Startup;
    use crate::state::aircraft::{self, Feeds, LoadedReference};
    use crate::state::flight::{AdsbQuery, Flight, FlightFeeds, FollowStream};

    /// The shots' clock: 2026-06-21 10:00 UTC.
    const NOW: f64 = 1_782_036_000_000.0;
    const V3: ViewState = ViewState { latitude: 59.33, longitude: 18.07, zoom: 9.0, pitch: 0.0, bearing: 0.0, position: None };

    /// The synthetic snapshot, answered at once.
    struct Synthetic(LiveSnapshot);

    impl Feeds for Synthetic {
        fn live_flights(&mut self, _area: Option<BoundingBox>, _categories: Option<Vec<u8>>) -> Answer<Result<LiveSnapshot, String>> {
            Answer::ready(Ok(self.0.clone()))
        }

        fn top_flights(&mut self) -> Answer<Result<Vec<TopFlight>, String>> {
            Answer::ready(Ok(Vec::new()))
        }

        fn reference_data(&mut self) -> Answer<Result<LoadedReference, String>> {
            Answer::ready(Err("no reference data in this test".into()))
        }

        fn session_info(&mut self) -> Option<SessionInfo> {
            None
        }
    }

    /// The details the hover card asked for, with the senders that answer them.
    type Asked = Arc<Mutex<Vec<(u32, Sender<Result<FlightDetails, String>>)>>>;

    struct Asking(Asked);

    impl HoverFeed for Asking {
        fn flight_details(&mut self, id: u32) -> Answer<Result<FlightDetails, String>> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().push((id, tx));
            rx.into()
        }
    }

    /// A selected flight's stream that sends `details` at once and stays live.
    struct Streaming(FlightDetails);

    impl FlightFeeds for Streaming {
        fn follow_flight(&mut self, _id: u32) -> Result<FollowStream, String> {
            let (tx, rx) = mpsc::channel();
            tx.send(FollowEvent::Details(Box::new(self.0.clone()))).unwrap();
            // kept open: the stream stays live
            std::mem::forget(tx);
            Ok(FollowStream::Channel(rx))
        }

        fn unfollow_flight(&mut self) {}

        fn opensky_track(&mut self, _icao24: &str) -> Answer<Result<Vec<OpenSkyPoint>, String>> {
            Answer::ready(Err("none".into()))
        }

        fn adsb(&mut self, _query: &AdsbQuery) -> Answer<Result<AdsbSnapshot, String>> {
            Answer::ready(Err("none".into()))
        }

        fn airport_runways(&mut self, _codes: Vec<String>) -> Answer<Result<HashMap<String, Vec<Runway>>, String>> {
            Answer::ready(Err("none".into()))
        }
    }

    /// Details whose progress Flightradar24 measured at `pct` %.
    fn measured(pct: u32) -> FlightDetails {
        FlightDetails {
            progress: Some(FlightProgress { traversed_distance: 1000, remaining_distance: 1000, progress_pct: pct, ..Default::default() }),
            ..Default::default()
        }
    }

    /// The app's state at `view` on the shots' 1100 × 720 canvas with the synthetic snapshot in,
    /// the hover card's details asked of `asked`.
    fn state_at(view: ViewState, asked: &Asked) -> AppState {
        let mut state = AppState::new(Startup::default(), None, None, None, view, [1100.0, 720.0]);
        state.aircraft = aircraft::Aircraft::with_feeds(Some(Box::new(Synthetic(fixtures::synthetic_snapshot(NOW)))));
        state.flight = Flight::with_feeds(None);
        state.pick = Pick::with_feed(Some(Box::new(Asking(asked.clone()))));
        state.poll(NOW);
        state
    }

    /// The scene a refresh makes of `state` now.
    fn refresh(state: &mut AppState, builder: &mut SceneBuilder, now: f64) -> SceneInput {
        let clocks = state.clocks(now);
        builder.build(state, clocks, 1.0)
    }

    /// The pointer, in whole pixels as hammer.js reports it, whose pixel (as luma.gl reads it at
    /// pixel ratio 1: its centre, half a pixel right of and below the pointer) is nearest
    /// `target`: so a distance from an icon is measured from where deck looks.
    fn pointer_for([x, y]: [f64; 2]) -> [f64; 2] {
        [(x - 0.5).round(), (y - 0.5).round()]
    }

    fn click(at: [f64; 2]) -> Click {
        Click { at, tap_count: 1, modifiers: Modifiers::default() }
    }

    /// The icon farthest from every other aircraft on the screen, where it is, and its radius.
    fn lone_icon(state: &AppState, scene: &SceneInput) -> (u32, [f64; 2], f64) {
        let viewport = viewport_for(&state.controller.view(), state.controller.size());
        let aircraft = &scene.aircraft.scene;
        let spots: Vec<(u32, [f64; 2])> = aircraft
            .icons
            .iter()
            .map(|i| (i.id, i.position))
            .chain(aircraft.groups.iter().flat_map(|g| g.instances.iter().map(|m| (m.id, m.position))))
            .map(|(id, p)| {
                let [x, y, _] = viewport.project(p);
                (id, [x, y])
            })
            .collect();
        let inside = |[x, y]: [f64; 2]| (60.0..1040.0).contains(&x) && (60.0..660.0).contains(&y);
        let (icon, at, clear) = aircraft
            .icons
            .iter()
            .filter_map(|icon| {
                let &(_, at) = spots.iter().find(|(id, _)| *id == icon.id)?;
                let clear = spots.iter().filter(|(id, _)| *id != icon.id).map(|(_, p)| (p[0] - at[0]).hypot(p[1] - at[1])).fold(f64::INFINITY, f64::min);
                inside(at).then_some((icon, at, clear))
            })
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .expect("an icon on the screen");
        assert!(clear > 80.0, "no aircraft within 80 px of {}: {clear}", icon.id);
        (icon.id, at, icon_pick_radius(icon))
    }

    /// The acceptance line through the app: a click 5 px from an icon selects it, 8 px does not
    /// (nothing is there: the selection goes); the press decides, not where the click lands.
    #[test]
    fn a_click_5_px_from_an_icon_selects_it_8_px_lets_go() {
        let asked = Asked::default();
        let mut state = state_at(V3, &asked);
        let mut builder = SceneBuilder::default();
        let scene = refresh(&mut state, &mut builder, NOW);
        let (id, [x, y], r) = lone_icon(&state, &scene);
        for (dx, dy) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
            let near = pointer_for([x + dx * (r + 5.0), y + dy * (r + 5.0)]);
            state.pick_press(&scene, near);
            assert_eq!(state.pick.pressed(), Some(&Some(Target::Aircraft(id))));
            let wants = state.pick_frame(&scene, None, false, &[click(near)], NOW);
            assert_eq!(state.flight.selected_id(), Some(id), "5 px out along {dx},{dy}");
            assert!(wants.scene, "a new selection refreshes the scene");
            let far = pointer_for([x + dx * (r + 8.0), y + dy * (r + 8.0)]);
            state.pick_press(&scene, far);
            assert_eq!(state.pick.pressed(), Some(&None));
            let wants = state.pick_frame(&scene, None, false, &[click(far)], NOW);
            assert_eq!(state.flight.selected_id(), None, "8 px out along {dx},{dy}: nothing there, so let go");
            assert!(wants.scene);
        }
        // the click takes what the press was on, wherever it lands
        state.pick_press(&scene, [x, y]);
        state.pick_frame(&scene, None, false, &[click([x + 200.0, y])], NOW);
        assert_eq!(state.flight.selected_id(), Some(id));
        // no press before it: the click picks where it is
        let mut fresh = state_at(V3, &asked);
        let scene = refresh(&mut fresh, &mut SceneBuilder::default(), NOW);
        fresh.pick_frame(&scene, None, false, &[click([x, y])], NOW);
        assert_eq!(fresh.flight.selected_id(), Some(id));
    }

    /// The click rules: an aircraft selects (the searched place goes), an airport's code opens
    /// its board and lets the flight go, a place's name does nothing, nothing at all lets go.
    #[test]
    fn click_rules() {
        let asked = Asked::default();
        let mut state = state_at(V3, &asked);
        let id = fixtures::FIRST_ID + 3;
        let place = || Some(crate::state::flight::MarkedPlace { lon: 18.0, lat: 59.3, name: "Here".into() });
        state.flight.set_place(place());
        assert!(state.click_map(Some(Target::Aircraft(id)), NOW));
        assert_eq!(state.flight.selected_id(), Some(id));
        assert!(state.flight.place().is_none(), "a selection on the map clears the place");
        // a place's name: nothing changes
        state.flight.set_place(place());
        assert!(!state.click_map(Some(Target::Label(None)), NOW));
        assert_eq!(state.flight.selected_id(), Some(id));
        assert!(state.flight.place().is_some());
        // an airport's code: its board, the flight let go, the place kept
        assert!(state.click_map(Some(Target::Label(Some("arn".into()))), NOW));
        assert_eq!(state.airport.code(), Some("ARN"));
        assert_eq!(state.flight.selected_id(), None);
        assert!(state.flight.place().is_some());
        // nothing: let go, and the place goes
        state.click_map(Some(Target::Aircraft(id)), NOW);
        state.flight.set_place(place());
        assert!(state.click_map(None, NOW));
        assert_eq!(state.flight.selected_id(), None);
        assert!(state.flight.place().is_none());
        // nothing again changes nothing
        assert!(!state.click_map(None, NOW));
    }

    /// Hover: the aircraft under the pointer is highlighted at once (a scene refresh), the cursor
    /// points while over it and for the leave event after it, nothing is picked while a button is
    /// down, a pointer off the map picks nothing, and a drag takes the card but not the highlight.
    #[test]
    fn hover_highlights_at_once_and_points_the_cursor() {
        let asked = Asked::default();
        let mut state = state_at(V3, &asked);
        let mut builder = SceneBuilder::default();
        let scene = refresh(&mut state, &mut builder, NOW);
        let (id, [x, y], _) = lone_icon(&state, &scene);
        assert!(!state.pick.hovering());

        let wants = state.pick_frame(&scene, Some([x, y]), false, &[], NOW);
        assert!(wants.scene, "the highlight refreshes the scene now");
        assert_eq!(wants.wake_at, Some(NOW + 300.0), "back when the card's dwell is up");
        assert_eq!(state.pick.hovered_id(), Some(id));
        assert!(state.pick.hovering());
        assert_eq!(state.pick.hover_flight().map(|f| f.id), Some(id));
        // the aircraft builder draws it highlighted
        let highlighted = refresh(&mut state, &mut builder, NOW);
        let icons = &highlighted.aircraft.scene.icons;
        assert_eq!(highlighted.aircraft.scene.hovered_icon.map(|i| icons[i].id), Some(id));
        // the same place again: no pick, no refresh
        assert!(!state.pick_frame(&highlighted, Some([x, y]), false, &[], NOW).scene);
        // moving over it moves the card only
        let wants = state.pick_frame(&highlighted, Some([x + 1.0, y]), false, &[], NOW);
        assert!(!wants.scene);
        assert_eq!(state.hover_card(NOW).map(|c| c.pointer), Some([x + 1.0, y]));

        // off it: the highlight goes; the cursor points once more (the layer's leave), then not
        let empty = [x + 60.0, y];
        assert!(state.pick_frame(&highlighted, Some(empty), false, &[], NOW).scene);
        assert_eq!(state.pick.hovered_id(), None);
        assert!(state.hover_card(NOW).is_none());
        assert!(state.pick.hovering(), "deck's leave event still counts");
        state.pick_frame(&highlighted, Some([x + 61.0, y]), false, &[], NOW);
        assert!(!state.pick.hovering());

        // a button down: moves pick nothing, and the release does not either until the next move
        state.pick_frame(&highlighted, Some([x, y]), true, &[], NOW);
        assert_eq!(state.pick.hovered_id(), None);
        state.pick_frame(&highlighted, Some([x, y]), false, &[], NOW);
        assert_eq!(state.pick.hovered_id(), None);
        state.pick_frame(&highlighted, Some([x + 0.5, y]), false, &[], NOW);
        assert_eq!(state.pick.hovered_id(), Some(id));

        // a drag takes the card, not the highlight
        assert!(state.controller.pan_start([x, y], false, false, NOW));
        state.pick_frame(&highlighted, Some([x + 0.5, y]), true, &[], NOW);
        assert!(state.pick.hover_flight().is_none());
        assert_eq!(state.pick.hovered_id(), Some(id));
        state.controller.pan_end();

        // off the map (or over a panel): nothing
        assert!(state.pick_frame(&highlighted, None, false, &[], NOW).scene);
        assert_eq!(state.pick.hovered_id(), None);
        assert!(state.pick.hovering(), "the leave event");
    }

    /// The hover card asks for the flight's details once the pointer has rested on it for 300 ms,
    /// and shows Flightradar24's progress once they are in; the selected flight's own stream
    /// answers for it at once.
    #[test]
    fn the_card_asks_for_details_after_a_rest() {
        let asked = Asked::default();
        let mut state = state_at(V3, &asked);
        let mut builder = SceneBuilder::default();
        let scene = refresh(&mut state, &mut builder, NOW);
        let (id, at, _) = lone_icon(&state, &scene);
        state.pick_frame(&scene, Some(at), false, &[], NOW);
        let card = state.hover_card(NOW).expect("a card");
        assert_eq!(card.flight.id, id);
        assert_eq!((card.status, card.info.as_ref()), (HoverStatus::Loading, None));
        // no reference data: the codes stand for the cities, and no estimate without them
        assert_eq!((card.from.as_str(), card.to.as_str()), (card.flight.origin.as_str(), card.flight.destination.as_str()));
        assert_eq!(card.progress, None);
        let wants = state.poll(NOW + 100.0);
        assert_eq!(wants.wake_at, Some(NOW + 300.0), "woken when the dwell is up");
        assert!(asked.lock().unwrap().is_empty());
        state.poll(NOW + 300.0);
        let (asked_id, tx) = asked.lock().unwrap().pop().expect("asked after 300 ms");
        assert_eq!(asked_id, id);
        tx.send(Ok(measured(42))).unwrap();
        state.poll(NOW + 400.0);
        let card = state.hover_card(NOW + 400.0).expect("a card");
        assert_eq!(card.status, HoverStatus::Ready);
        assert_eq!(card.progress, Some(Progress { value: 0.42, measured: true }));

        // the selected flight's details come from its own stream: nothing is asked
        let mut state = state_at(V3, &asked);
        state.flight = Flight::with_feeds(Some(Box::new(Streaming(measured(70)))));
        state.select_flight(Some(id), SelectOptions::default(), NOW);
        state.poll(NOW);
        let scene = refresh(&mut state, &mut SceneBuilder::default(), NOW);
        assert!(state.flight.details().is_some(), "the stream delivered");
        state.pick_frame(&scene, Some(at), false, &[], NOW);
        state.poll(NOW + 500.0);
        assert!(asked.lock().unwrap().is_empty(), "nothing asked for the selected flight");
        let card = state.hover_card(NOW + 500.0).expect("a card");
        assert_eq!(card.status, HoverStatus::Ready);
        assert_eq!(card.progress, Some(Progress { value: 0.7, measured: true }));
    }

    /// Without a feed (no backend) a rested pointer's request fails at once: the card goes without.
    #[test]
    fn without_a_backend_the_card_goes_without_details() {
        let asked = Asked::default();
        let mut state = state_at(V3, &asked);
        state.pick = Pick::with_feed(None);
        let scene = refresh(&mut state, &mut SceneBuilder::default(), NOW);
        let (_, at, _) = lone_icon(&state, &scene);
        state.pick_frame(&scene, Some(at), false, &[], NOW);
        state.poll(NOW + 300.0);
        assert_eq!(state.hover_card(NOW + 300.0).map(|c| c.status), Some(HoverStatus::Unavailable));
    }

    /// On the 3D terrain a signal is picked where its disc stands, on the ground under it (item
    /// G3): the scene the renderer stood on the terrain's ground carries it (`stands_on`).
    #[test]
    fn on_the_terrain_a_signal_is_picked_where_it_stands() {
        use rustradar_core::geo::Basemap;
        use rustradar_core::model::TrainBlock;
        use rustradar_map::layers::terrain::ground::Ground;
        use rustradar_scene::blocks::{BlockBuilder, SetRef};
        use rustradar_scene::camera::viewport_for;
        use rustradar_scene::terrain::heights::{GRID_SIDE, HeightGrid, Heights};
        use rustradar_scene::tiles::osm_tile_to_lng_lat;

        // a block starting in the middle of zoom-13 tile 4500/2370, on ground 150 m up
        let tile = rustradar_map::layers::TileIndex::new(4500, 2370, 13);
        let start = osm_tile_to_lng_lat(4500.5, 2370.5, 13);
        let list = [TrainBlock { id: "b".into(), signal: Some("1".into()), lines: vec![vec![start, [start[0] + 0.002, start[1]]]] }];
        let size = [800.0, 500.0];
        let view = ViewState { longitude: start[0], latitude: start[1], zoom: 16.0, pitch: 50.0, bearing: 0.0, position: Some([0.0, 0.0, 150.0]) };
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        scene.blocks.scene = BlockBuilder::new().build(Some(SetRef { generation: 1, list: &list, structures: &[] }), &[], &view, size, NOW);
        let viewport = viewport_for(&view, size);
        let [rx, ry, _] = viewport.project([start[0], start[1], 150.0]);
        let [sx, sy, _] = viewport.project([start[0], start[1], 0.0]);
        assert!((sy - ry).abs() > 50.0 && sy < size[1], "sea level is elsewhere on the picture: {ry} against {sy}");
        // at sea level (no terrain), where it was
        assert_eq!(pick_at(&scene, &view, size, [sx, sy]), Some(Target::Signal(0)));
        assert_eq!(pick_at(&scene, &view, size, [rx, ry]), None);
        // on the terrain's ground, where its disc is drawn
        scene.stands_on = Ground::new(Heights::new([(tile, Arc::new(HeightGrid::from_fn(GRID_SIDE, |_, _| 150.0)))]));
        assert_eq!(pick_at(&scene, &view, size, [rx, ry]), Some(Target::Signal(0)));
        assert_eq!(pick_at(&scene, &view, size, [sx, sy]), None);
    }
}
