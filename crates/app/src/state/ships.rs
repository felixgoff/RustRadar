//! Ships (item H1; doc 06 §3.8; `+page.svelte:416-535`, `:787-812`): the ships in view and when
//! they are asked for, the selected ship and its card, and the list the map draws.
//!
//! Today's page runs this on `setTimeout`s, a `setInterval` and `$effect`s. Here every timer is a
//! deadline in local Unix ms, checked in [`Ships::poll`] once a frame and handed to the frame loop
//! as [`Wants::wake_at`](super::Wants); every effect is a comparison with what the last poll saw.
//! A poll runs, in this order:
//!
//! 1. the first poll only: the source (below) and the effects' first runs;
//! 2. the effects, in the page's order: the view effect (a new view, or ships on or off the map:
//!    the 600 ms settle, or everything stopped and cleared); the ships off the map let go of the
//!    selected one; another panel open closes the ship (one panel at a time;
//!    [`Ships::set_other_panel_open`]);
//! 3. the timers that are due: the settle or the 60 s refresh (`loadShips`), the card's 60 s
//!    refresh;
//! 4. the answers: the ships in view (dropped when a newer request was made, the ships left the
//!    map, the feed's zoom step changed or the camera has gone elsewhere), the card;
//! 5. the selected ship as now known (the list's row, else the last one seen) and the list the
//!    map draws.
//!
//! **Timing** (`+page:416-477`): the ships in view are asked for [`SHIPS_SETTLE_MS`] after the
//! camera stands, then every [`SHIPS_REFRESH_MS`]: the refresh is set again as each request
//! starts, kept apart from the settle, so a camera that never settles (following an aircraft)
//! still gets them. Each request takes a new number from the request counter (`shipsRequest`),
//! and so does turning the ships off; an answer is kept only when it carries the latest number
//! (the earlier request's [`Answer`] is dropped with it), the ships are still on, the feed's zoom
//! step ([`ship_zoom`]) is the one asked for and the view's centre is within half the visible
//! radius of where it asked ([`left_behind`]). A failure keeps the ships on show and says so
//! ([`Ships::failed`]).
//!
//! **Selecting** ([`AppState::select_ship`], `selectShip`): the same ship again does nothing;
//! another one closes the flight, the train and the airport board, lets go of the camera's
//! vehicle lock ([`Ships::following`]) and asks for its card at once and every
//! [`SHIP_DETAILS_REFRESH_MS`] while it stays selected (`loadShipCard`, with its own request
//! counter, `shipCardRequest`, bumped by every selection too). A refresh keeps the card on show;
//! only a new ship, or a retry after an error ([`Ships::retry_card`]), shows the skeletons; a
//! failed refresh keeps the card it had. The selected ship is the list's row for its MMSI as each
//! answer comes, and stays where it was last seen when the list drops it. [`AppState::pick_ship`]
//! is a pick from the list or the search: the place marker goes, the aircraft lock lets go, the
//! ship is selected and the camera flies to it at zoom 11 or closer.
//!
//! **What the others read**: [`Ships::on_map`] (`shipsOnMap`: the ships the filters keep, with the
//! selected ship after them when the filters leave it out; [`Ships::on_map_generation`] changes
//! with it), [`Ships::selected`], [`Ships::card_details`] and [`Ships::card_error`] (the panel),
//! [`Ships::ships`], [`Ships::loaded`], [`Ships::failed`], [`Ships::freshness`],
//! [`Ships::filtered_len`] and [`Ships::listed`] (the list and the filter panel), [`sizes_known`]
//! (`shipSizesKnown`), [`Ships::following`].
//!
//! **Sources**: the `--fixture` folder ([`FIXTURE_SHIPS`]: the `Ship` list `ships_in_view`
//! answers, read again at each request; [`FIXTURE_SHIP_DETAILS`]: `ship_details/{mmsi}.json`, the
//! card `ship_details` answers; a missing file answers as a failed request), else the backend
//! (`ships::in_view`, `ships::details`), else none (the page without Tauri: nothing is asked, the
//! list says it failed). [`Ships::with_feeds`] sets them instead (tests).
//!
//! Deviations from today's app:
//! - The fixture folder's ships are asked at the first frame, not 600 ms after it, so a shot
//!   (whose clock stands still) has them; later views settle as today (as the trains do).
//! - The view effect restarts its settle when the view's values change; the page restarts it on
//!   every report of the view, even of the same values.
//! - The window's visibility is not checked (`document.visibilityState`): the ships and the card
//!   are asked for while the app runs.
//! - The card's refresh runs every 60 s from the selection as a deadline, re-armed from the one it
//!   met (a frame that comes late does not shift the ones after it).
//! - Another panel opened (a flight selected, a train selected, an airport's board opened) closes
//!   the ship at the next poll, one frame later; a ship selected closes the others at once.
//! - The filters are compared by value: writing the same filters again (the list's "show all"
//!   with nothing hidden) makes no new list for the map; the page's new arrays make one.
//!
//! A selection made between two polls (a click, the list) asks for a scene refresh at the next
//! poll: the halo and ring are drawn at it.
//!
//! Left to the items that own them: the ship click on the map (H4, `onshipselect`: the place
//! cleared, then [`AppState::select_ship`]) and a click on empty map letting go of the ship
//! (`state::pick`), `closeTop` (U6: [`Ships::selected`] then `select_ship(None)`), the list's
//! hover (`hoveredShipMmsi`, for the ship scene builder, H2), the list, the filter panel and the
//! panel's Retry and Follow (U15: [`Ships::listed`], [`Ships::retry_card`],
//! [`Ships::toggle_following`]), the key C (U6), and the camera's centring on a followed ship
//! (`vehicleAt`, `vehicleLocked`: it needs the drawn position of H2's scene, and hands the lock to
//! `state::camera` with `Camera::set_vehicle_locked`).

use std::path::PathBuf;

use rustradar_backend::ships as backend_ships;
use rustradar_backend::Backend;
use rustradar_core::freshness::{Freshness, feed_freshness};
use rustradar_core::geo::{self, Area, view_area};
use rustradar_core::model::{Ship, ShipDetails};
use rustradar_core::settings::shown_kinds;
use rustradar_core::ship_list::{ShipFilters, ShipSort, ship_filter, ship_matches, sort_ships};
use rustradar_core::trains::LatLon;
use rustradar_scene::camera::ViewState;
use rustradar_scene::geom::{js_max, js_min};

use super::aircraft::Answer;
use super::flight::SelectOptions;
use super::trains::{WORLD, left_behind};
use super::{AppState, Cx, Startup};
use crate::requests::Requests;

/// The camera stands this long before the ships in view are asked for, ms (`SHIPS_SETTLE_MS`).
pub const SHIPS_SETTLE_MS: f64 = 600.0;
/// The ships in view are asked for again this often, ms (`SHIPS_REFRESH_MS`).
pub const SHIPS_REFRESH_MS: f64 = 60_000.0;
/// The selected ship's card is asked for again this often, ms (`SHIP_DETAILS_REFRESH_MS`).
pub const SHIP_DETAILS_REFRESH_MS: f64 = 60_000.0;

/// The `--fixture` folder's ships: a `Ship` list as `ships_in_view` sends it.
pub const FIXTURE_SHIPS: &str = "ships_in_view.json";
/// The `--fixture` folder's ship cards: `ship_details/{mmsi}.json`, a `ShipDetails` as
/// `ship_details` sends it.
pub const FIXTURE_SHIP_DETAILS: &str = "ship_details";

/// The list's centre when it is not sorted by distance (`ORIGIN`).
const ORIGIN: LatLon = LatLon { lat: 0.0, lon: 0.0 };

/// The feed's zoom step for a map zoom (`shipZoom`, the backend's `feed_zoom`): 2 to 16, as the
/// feed is empty from 17 up. An answer at another step has the wrong density.
pub fn ship_zoom(zoom: f64) -> f64 {
    js_min(16.0, js_max(2.0, (zoom + 1.0).floor()))
}

/// VesselFinder tells ship sizes apart above its zoom step 6 (`shipSizesKnown`).
pub fn sizes_known(zoom: f64) -> bool {
    ship_zoom(zoom) > 6.0
}

/// Where the ships' answers come from: the backend, the `--fixture` folder, or a test's.
pub trait ShipFeeds: Send {
    /// `shipsInView(area, zoom)`.
    fn ships_in_view(&mut self, area: Area, zoom: f64) -> Answer<Result<Vec<Ship>, String>>;
    /// `shipDetails(mmsi)`.
    fn ship_details(&mut self, mmsi: u32) -> Answer<Result<ShipDetails, String>>;
}

/// The backend's requests, on its runtime.
struct BackendFeeds {
    requests: Requests,
}

impl ShipFeeds for BackendFeeds {
    fn ships_in_view(&mut self, area: Area, zoom: f64) -> Answer<Result<Vec<Ship>, String>> {
        self.requests.request(backend_ships::in_view(area.west, area.south, area.east, area.north, zoom)).into()
    }

    fn ship_details(&mut self, mmsi: u32) -> Answer<Result<ShipDetails, String>> {
        self.requests.request(backend_ships::details(mmsi)).into()
    }
}

/// The `--fixture` folder's files (see the module documentation), read when asked for.
struct FixtureFeeds {
    dir: PathBuf,
}

/// `dir/name` read as JSON of type `$t`, or why not.
macro_rules! read_json {
    ($t:ty, $dir:expr, $name:expr) => {{
        let path = $dir.join($name);
        std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<$t>(&bytes).map_err(|e| e.to_string()))
            .map_err(|e| format!("fixture {}: {e}", path.display()))
    }};
}

impl ShipFeeds for FixtureFeeds {
    fn ships_in_view(&mut self, _area: Area, _zoom: f64) -> Answer<Result<Vec<Ship>, String>> {
        Answer::ready(read_json!(Vec<Ship>, self.dir, FIXTURE_SHIPS))
    }

    fn ship_details(&mut self, mmsi: u32) -> Answer<Result<ShipDetails, String>> {
        Answer::ready(read_json!(ShipDetails, self.dir, format!("{FIXTURE_SHIP_DETAILS}/{mmsi}.json")))
    }
}

/// Which ships the state shows.
enum Source {
    /// Chosen at the first poll: the fixture folder, else the backend, else none.
    Auto,
    /// None (the page without Tauri).
    Off,
    On(Box<dyn ShipFeeds>),
}

/// The request out for the ships in view: its number, and the view it asked for (`asked`).
#[derive(Debug)]
struct InFlight {
    request: u64,
    answer: Answer<Result<Vec<Ship>, String>>,
    asked: ViewState,
}

/// The card's request out: its number and the ship it is for.
#[derive(Debug)]
struct CardInFlight {
    request: u64,
    mmsi: u32,
    answer: Answer<Result<ShipDetails, String>>,
}

/// The selected ship's card as last asked for (`shipCard`): the ship's MMSI, its details once
/// they have come, or why they could not be had.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipCard {
    pub mmsi: u32,
    pub details: Option<ShipDetails>,
    pub error: Option<String>,
}

/// What the list the map draws was last made from.
#[derive(Debug, Clone, PartialEq, Default)]
struct OnMapKey {
    ships: u64,
    picked: u64,
    /// The filters `shipFilter` reads: all but the sort ([`filter_part`]).
    filters: Option<ShipFilters>,
}

/// The part of the filters the map's list depends on: everything but the sort (`shipFilter`
/// reads the kinds, the sizes, old positions and the motion, not the order).
fn filter_part(filters: &ShipFilters) -> ShipFilters {
    ShipFilters { sort: ShipSort::default(), ..filters.clone() }
}

/// Ships: the ships in view and their timing, the selected ship and its card, the list the map
/// draws.
pub struct Ships {
    source: Source,
    started: bool,

    // the ships in view
    ships: Vec<Ship>,
    /// Bumped whenever `ships` is set.
    ships_generation: u64,
    /// `shipsAt`: when the latest answer since the ships came on arrived; 0 before the first.
    ships_at: f64,
    /// `shipsFailed`.
    failed: bool,
    /// `shipsRequest`: bumped by each request and when the ships go off.
    request_counter: u64,
    request: Option<InFlight>,
    /// `shipsSettle`.
    settle_due: Option<f64>,
    /// `shipsRefresh`.
    refresh_due: Option<f64>,
    /// The view the last poll saw.
    last_view: Option<ViewState>,
    /// Ships on the map at the last poll.
    ships_were: bool,
    /// A flight, a train or an airport board is open (from the state's poll).
    other_open: bool,

    // the selected ship
    /// `pickedShip`, kept at `selectedShip` (the list's row, else the last seen).
    picked: Option<Ship>,
    picked_generation: u64,
    /// The generations `picked` was last brought up to date with.
    selected_for: (u64, u64),
    /// `followingVehicle` for the ship: the camera follows it.
    following: bool,
    /// `picked_generation` at the end of the last poll: a selection made between two polls (the
    /// map's click, the list) asks for a scene refresh at the next.
    polled_picked: u64,

    // the card
    card: Option<ShipCard>,
    /// `shipCardRequest`: bumped by each card request and each selection.
    card_counter: u64,
    card_request: Option<CardInFlight>,
    /// The card's `setInterval`.
    card_due: Option<f64>,

    // the list the map draws
    /// Indexes into `ships` the filters keep (`shipsFiltered`), in order.
    filtered: Vec<usize>,
    on_map: Vec<Ship>,
    /// `on_map` has the selected ship after the filtered ones.
    appended: bool,
    on_map_for: OnMapKey,
    on_map_generation: u64,
}

impl Default for Ships {
    /// The ships of the `--fixture` folder, else the backend, chosen at the first poll.
    fn default() -> Self {
        Ships::with_source(Source::Auto)
    }
}

impl std::fmt::Debug for Ships {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ships")
            .field("ships", &self.ships.len())
            .field("on_map", &self.on_map.len())
            .field("ships_at", &self.ships_at)
            .field("failed", &self.failed)
            .field("loading", &self.request.is_some())
            .field("selected", &self.selected_mmsi())
            .field("card", &self.card.as_ref().map(|c| (c.mmsi, c.details.is_some(), c.error.is_some())))
            .finish_non_exhaustive()
    }
}

impl Ships {
    fn with_source(source: Source) -> Self {
        Ships {
            source,
            started: false,
            ships: Vec::new(),
            ships_generation: 0,
            ships_at: 0.0,
            failed: false,
            request_counter: 0,
            request: None,
            settle_due: None,
            refresh_due: None,
            last_view: None,
            ships_were: false,
            other_open: false,
            picked: None,
            picked_generation: 0,
            selected_for: (0, 0),
            following: false,
            polled_picked: 0,
            card: None,
            card_counter: 0,
            card_request: None,
            card_due: None,
            filtered: Vec::new(),
            on_map: Vec::new(),
            appended: false,
            on_map_for: OnMapKey::default(),
            on_map_generation: 0,
        }
    }

    /// The ships on `feeds` instead of the fixture folder or the backend; `None`: none at all.
    pub fn with_feeds(feeds: Option<Box<dyn ShipFeeds>>) -> Self {
        Ships::with_source(match feeds {
            Some(feeds) => Source::On(feeds),
            None => Source::Off,
        })
    }

    // ---------------------------------------------------------------- what the others read

    /// The ships in view as last answered (`ships`), unfiltered.
    pub fn ships(&self) -> &[Ship] {
        &self.ships
    }

    /// Changes whenever [`Self::ships`] is set again.
    pub fn ships_generation(&self) -> u64 {
        self.ships_generation
    }

    /// When the latest answer since the ships came on arrived (`shipsAt`, local Unix ms); 0
    /// before the first.
    pub fn ships_at(&self) -> f64 {
        self.ships_at
    }

    /// Whether an answer has arrived since the ships came on (`shipsAt > 0`).
    pub fn loaded(&self) -> bool {
        self.ships_at > 0.0
    }

    /// Whether the latest request failed (`shipsFailed`; the ships on show stay). Without a
    /// source the list says so too (`shipsFailed || !isTauri()`).
    pub fn failed(&self) -> bool {
        self.failed || matches!(self.source, Source::Off)
    }

    /// Whether a request for the ships in view is out.
    pub fn loading(&self) -> bool {
        self.request.is_some()
    }

    /// The sidebar's age of the ships (`feedFreshness(now - shipsAt, SHIPS_REFRESH_MS)`), once
    /// they have arrived; the page shows it while ships are the focus.
    pub fn freshness(&self, now_ms: f64) -> Option<Freshness> {
        self.loaded().then(|| feed_freshness(js_max(0.0, now_ms - self.ships_at) as i64, SHIPS_REFRESH_MS as i64))
    }

    /// The ships the filters keep (`shipsFiltered`), in the answer's order, as of the last poll.
    pub fn filtered(&self) -> impl Iterator<Item = &Ship> + '_ {
        self.filtered.iter().map(|&i| &self.ships[i])
    }

    /// How many ships the filters keep (`shipsFiltered.length`: the filter panel's count).
    pub fn filtered_len(&self) -> usize {
        self.filtered.len()
    }

    /// The list's rows (`shipsListed`): the ships the filters keep that match `query`, in the
    /// order of `filters.sort`, by distance from `centre` (the view's centre in 1/20° steps,
    /// `listCentre`) when sorted so.
    pub fn listed<'a>(&'a self, filters: &ShipFilters, query: &str, centre: LatLon) -> Vec<&'a Ship> {
        let sort = filters.sort;
        // `q ? filtered.filter(...) : filtered`: an empty query matches every ship anyway
        let keep = ship_filter(filters);
        let matching = self.ships.iter().filter(|s| keep(s) && ship_matches(s, query));
        sort_ships(matching, sort, if sort == ShipSort::Distance { centre } else { ORIGIN })
    }

    /// The list the map draws (`shipsOnMap`): the ships the filters keep, and the selected ship
    /// after them when they leave it out.
    pub fn on_map(&self) -> &[Ship] {
        &self.on_map
    }

    /// Changes whenever [`Self::on_map`] is a new list, where today's `shipsOnMap` is a new array
    /// (a new answer, the filters but the sort, the selection while it is drawn after the
    /// filtered ships): the ship builder eases each ship onto it from where it is drawn.
    pub fn on_map_generation(&self) -> u64 {
        self.on_map_generation
    }

    /// The selected ship as now known (`selectedShip`).
    pub fn selected(&self) -> Option<&Ship> {
        self.picked.as_ref()
    }

    /// The selected ship's MMSI (`selectedMmsi`).
    pub fn selected_mmsi(&self) -> Option<u32> {
        self.picked.as_ref().map(|s| s.mmsi)
    }

    /// The card as last asked for (`shipCard`).
    pub fn card(&self) -> Option<&ShipCard> {
        self.card.as_ref()
    }

    /// The selected ship's details for its panel (`shipCard?.mmsi === selectedShip.mmsi ?
    /// shipCard.details : null`): `None` while they load or when they could not be had.
    pub fn card_details(&self) -> Option<&ShipDetails> {
        self.selected_card().and_then(|c| c.details.as_ref())
    }

    /// Why the selected ship's details could not be had (the panel's error and Retry).
    pub fn card_error(&self) -> Option<&str> {
        self.selected_card().and_then(|c| c.error.as_deref())
    }

    fn selected_card(&self) -> Option<&ShipCard> {
        let mmsi = self.selected_mmsi()?;
        self.card.as_ref().filter(|c| c.mmsi == mmsi)
    }

    /// Whether the camera follows the selected ship (`followingVehicle`).
    pub fn following(&self) -> bool {
        self.following
    }

    // ---------------------------------------------------------------- what the others ask

    /// The panel's Retry (`loadShipCard(selectedShip.mmsi)`): the card is asked for again now,
    /// with the skeletons back if the last one failed; the 60 s refresh keeps its time.
    pub fn retry_card(&mut self) {
        if let Some(mmsi) = self.selected_mmsi() {
            self.load_card(mmsi);
        }
    }

    /// The key C or the panel's Follow with a ship selected (`followingVehicle =
    /// !followingVehicle`).
    pub fn toggle_following(&mut self) {
        self.following = !self.following;
    }

    /// Locks or lets go of the camera on the selected ship.
    pub fn set_following(&mut self, following: bool) {
        self.following = following;
    }

    /// Whether a flight is selected, a train is selected or an airport's board is open, as the
    /// state's poll hands it over before this part's step: one panel at a time, so the ship
    /// closes (`select(id)`, `selectTrain(train)` and `openAirport(code)` → `selectShip(null)`).
    pub fn set_other_panel_open(&mut self, open: bool) {
        self.other_open = open;
    }

    // ---------------------------------------------------------------- the page's functions

    /// The source, once: the fixture folder, else the backend, else none.
    fn resolve(&mut self, startup: &Startup, backend: Option<&Backend>, requests: Option<&Requests>) {
        if !matches!(self.source, Source::Auto) {
            return;
        }
        self.source = if let Some(dir) = &startup.fixture {
            Source::On(Box::new(FixtureFeeds { dir: dir.clone() }))
        } else if let (Some(_), Some(requests)) = (backend, requests) {
            Source::On(Box::new(BackendFeeds { requests: requests.clone() }))
        } else {
            Source::Off
        };
    }

    /// `stopShips()`.
    fn stop(&mut self) {
        self.settle_due = None;
        self.refresh_due = None;
    }

    /// The view effect: ships off the map stop everything and drop what they brought (the list
    /// only when there is one to clear); on, the ships are asked for once the camera has stood
    /// [`SHIPS_SETTLE_MS`]. Returns whether the map's list changed.
    fn view_effect(&mut self, on: bool, now: f64) -> bool {
        if !on {
            self.stop();
            // `shipsRequest++`: an answer on its way is dropped
            self.request_counter += 1;
            self.request = None;
            let had = !self.ships.is_empty();
            if had {
                self.set_ships(Vec::new());
            }
            self.ships_at = 0.0;
            self.failed = false;
            return had;
        }
        self.settle_due = Some(now + SHIPS_SETTLE_MS);
        false
    }

    /// `loadShips()`.
    fn load_ships(&mut self, view: ViewState, size: [f64; 2], on: bool, now: f64) {
        self.stop();
        let Source::On(feeds) = &mut self.source else { return };
        if !on {
            return;
        }
        self.refresh_due = Some(now + SHIPS_REFRESH_MS);
        let area = view_area(geo::View { longitude: view.longitude, latitude: view.latitude, zoom: view.zoom, pitch: view.pitch }, size[0], size[1])
            .unwrap_or(WORLD);
        self.request_counter += 1;
        let answer = feeds.ships_in_view(area, view.zoom);
        // a request out before this one is dropped: its number is no longer the latest
        self.request = Some(InFlight { request: self.request_counter, answer, asked: view });
    }

    /// The answer to `loadShips`'s request, if it has arrived: kept unless a newer request was
    /// made, the ships left the map, the feed's zoom step changed or the camera has gone elsewhere
    /// since it asked. A failure keeps the ships on show. Returns whether the list changed.
    fn take_ships(&mut self, view: ViewState, size: [f64; 2], on: bool, now: f64) -> bool {
        let Some(answer) = self.request.as_ref().and_then(|r| r.answer.poll()) else { return false };
        let Some(InFlight { request, asked, .. }) = self.request.take() else { return false };
        match answer {
            Ok(list) => {
                // dropped if newer was asked meanwhile, or the camera has gone elsewhere
                if request != self.request_counter || !on || ship_zoom(view.zoom) != ship_zoom(asked.zoom) || left_behind(&asked, &view, size) {
                    return false;
                }
                self.set_ships(list);
                self.ships_at = now;
                self.failed = false;
                true
            }
            // resting after a failure, or unreachable: the ships on show stay
            Err(e) => {
                log::debug!("ships in view: {e}");
                if request == self.request_counter {
                    self.failed = true;
                }
                false
            }
        }
    }

    fn set_ships(&mut self, ships: Vec<Ship>) {
        self.ships = ships;
        self.ships_generation += 1;
    }

    /// `loadShipCard(mmsi)`: a new number; the skeletons unless the card on show is this ship's
    /// and did not fail.
    fn load_card(&mut self, mmsi: u32) {
        let Source::On(feeds) = &mut self.source else { return };
        self.card_counter += 1;
        let keep = self.card.as_ref().is_some_and(|c| c.mmsi == mmsi && c.error.is_none());
        if !keep {
            self.card = Some(ShipCard { mmsi, details: None, error: None });
        }
        let answer = feeds.ship_details(mmsi);
        self.card_request = Some(CardInFlight { request: self.card_counter, mmsi, answer });
    }

    /// The card's answer, if it has arrived: taken when it is the latest request's; a failure
    /// keeps a card of the same ship that has details.
    fn take_card(&mut self) {
        let Some(answer) = self.card_request.as_ref().and_then(|r| r.answer.poll()) else { return };
        let Some(CardInFlight { request, mmsi, .. }) = self.card_request.take() else { return };
        if request != self.card_counter {
            return;
        }
        match answer {
            Ok(details) => self.card = Some(ShipCard { mmsi, details: Some(details), error: None }),
            // a failed refresh keeps the card it had
            Err(e) => {
                log::debug!("ship {mmsi}'s card: {e}");
                let had = self.card.as_ref().is_some_and(|c| c.mmsi == mmsi && c.details.is_some());
                if !had {
                    self.card = Some(ShipCard { mmsi, details: None, error: Some(e) });
                }
            }
        }
    }

    /// The card's `setInterval`: asked again every [`SHIP_DETAILS_REFRESH_MS`] while the ship is
    /// selected.
    fn card_tick(&mut self, now: f64) {
        let (Some(due), Some(mmsi)) = (self.card_due, self.selected_mmsi()) else { return };
        if now < due {
            return;
        }
        let next = due + SHIP_DETAILS_REFRESH_MS;
        self.card_due = Some(if next > now { next } else { now + SHIP_DETAILS_REFRESH_MS });
        self.load_card(mmsi);
    }

    /// `selectShip(ship)`'s own part: the same ship again does nothing; otherwise the selection,
    /// the card cleared and its counter bumped, and the effects of a new MMSI (the camera lets go;
    /// the card asked for now and every minute). Returns whether the selection changed.
    fn pick(&mut self, ship: Option<Ship>, now: f64) -> bool {
        if let Some(s) = &ship
            && self.selected_mmsi() == Some(s.mmsi)
        {
            return false;
        }
        let before = self.selected_mmsi();
        let changed = self.picked.is_some() || ship.is_some();
        self.picked = ship;
        if changed {
            self.picked_generation += 1;
        }
        self.card_counter += 1;
        self.card_request = None;
        self.card = None;
        let mmsi = self.selected_mmsi();
        if mmsi != before {
            // another ship (or none): the camera lets go
            self.following = false;
            // the card's effect: the old interval cleared, the new ship's card asked for
            self.card_due = None;
            if let (Some(mmsi), Source::On(_)) = (mmsi, &self.source) {
                self.load_card(mmsi);
                self.card_due = Some(now + SHIP_DETAILS_REFRESH_MS);
            }
        }
        changed
    }

    /// `selectedShip`, and the effect that keeps `pickedShip` at it: the list's row for its MMSI,
    /// else the last seen.
    fn refresh_selected(&mut self) -> bool {
        let key = (self.ships_generation, self.picked_generation);
        if key == self.selected_for {
            return false;
        }
        self.selected_for = key;
        let Some(picked) = &self.picked else { return false };
        let Some(listed) = self.ships.iter().find(|s| s.mmsi == picked.mmsi) else { return false };
        if listed == picked {
            return false;
        }
        self.picked = Some(listed.clone());
        self.picked_generation += 1;
        self.selected_for.1 = self.picked_generation;
        true
    }

    /// `shipsFiltered` and `shipsOnMap`, made again when the ships, the filters (but the sort) or
    /// the selection changed. The list counts as a new one where the page's `shipsOnMap` is a new
    /// array: when the filtered ships are made again (a new answer, the filters), and when the
    /// selection changes while it is drawn after them (or no longer is); a selection that is one
    /// of the filtered ships leaves it as it was.
    fn refilter(&mut self, filters: &ShipFilters) -> bool {
        let filters = filter_part(filters);
        let filtered_new = self.on_map_for.ships != self.ships_generation || self.on_map_for.filters.as_ref() != Some(&filters);
        let selected_new = self.on_map_for.picked != self.picked_generation;
        if !filtered_new && !selected_new {
            return false;
        }
        if filtered_new {
            let keep = ship_filter(&filters);
            self.filtered = self.ships.iter().enumerate().filter(|(_, s)| keep(s)).map(|(i, _)| i).collect();
        }
        self.on_map_for = OnMapKey { ships: self.ships_generation, picked: self.picked_generation, filters: Some(filters) };
        let appended =
            self.picked.as_ref().is_some_and(|p| !self.filtered.iter().any(|&i| self.ships[i].mmsi == p.mmsi));
        let new_list = filtered_new || appended || self.appended;
        self.appended = appended;
        // no ships before and none now: nothing for the map to take in
        if !new_list || (self.on_map.is_empty() && self.filtered.is_empty() && !appended) {
            return false;
        }
        self.on_map = self.filtered.iter().map(|&i| self.ships[i].clone()).collect();
        if let (true, Some(selected)) = (appended, &self.picked) {
            self.on_map.push(selected.clone());
        }
        self.on_map_generation += 1;
        true
    }

    /// This frame's step (see [`super::AppState::poll`] for the order, and the module's
    /// documentation for the steps).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = cx.clocks.local_ms;
        let view = cx.controller.view();
        let size = cx.controller.size();
        let settings = &cx.persisted.settings;
        let on = shown_kinds(settings.mode, &settings.also_show).ships;
        let mut changed = false;

        if !self.started {
            self.start(cx, view, size, on, now);
        } else {
            if self.last_view != Some(view) || on != self.ships_were {
                self.last_view = Some(view);
                changed |= self.view_effect(on, now);
            }
            // the ships off the map: nothing left to show
            if on != self.ships_were {
                self.ships_were = on;
                if !on {
                    changed |= self.pick(None, now);
                }
            }
        }
        // one panel at a time: a flight, a train or a board opened closes the ship
        if self.other_open && self.picked.is_some() {
            changed |= self.pick(None, now);
        }

        // the timers
        if self.settle_due.is_some_and(|at| now >= at) || self.refresh_due.is_some_and(|at| now >= at) {
            self.load_ships(view, size, on, now);
        }
        self.card_tick(now);

        // the answers
        changed |= self.take_ships(view, size, on, now);
        self.take_card();

        // the selected ship and the list the map draws
        changed |= self.refresh_selected();
        changed |= self.refilter(&cx.persisted.ship_filters);
        // the selection is drawn (halo and ring): a new one, from wherever it came, is a change
        changed |= std::mem::replace(&mut self.polled_picked, self.picked_generation) != self.picked_generation;
        if changed {
            cx.wants.scene = true;
        }
        for due in [self.settle_due, self.refresh_due, self.card_due].into_iter().flatten() {
            cx.wants.wake_at(due);
        }
    }

    /// The page's mount: the source and the effects' first runs.
    fn start(&mut self, cx: &Cx<'_>, view: ViewState, size: [f64; 2], on: bool, now: f64) {
        self.started = true;
        self.resolve(cx.startup, cx.backend, cx.requests);
        self.last_view = Some(view);
        self.ships_were = on;
        if on && cx.startup.fixture.is_some() && matches!(self.source, Source::On(_)) {
            // canned answers come at once: asked at the first frame (see the deviations)
            self.load_ships(view, size, on, now);
        } else {
            self.view_effect(on, now);
        }
    }
}

impl AppState {
    /// `selectShip(ship)` at local time `now_ms` (a click on the map, the list, Escape with
    /// `None`): the same ship again does nothing; another one closes the flight, the train and
    /// the airport board, and asks for its card.
    pub fn select_ship(&mut self, ship: Option<Ship>, now_ms: f64) {
        self.ships.resolve(&self.startup, self.backend.as_ref(), self.requests.as_ref());
        let opening = ship.is_some();
        if !self.ships.pick(ship, now_ms) || !opening {
            return;
        }
        // one panel at a time; the focus (and the sidebar) stays where it is
        if self.flight.selected_id().is_some() {
            self.select_flight(None, SelectOptions::default(), now_ms);
        }
        self.select_train(None, now_ms);
        self.flight.set_focus_airport(None);
    }

    /// `pickShip(ship)`: picked from the list or the search, so the place marker goes, the
    /// aircraft lock lets go, the ship is selected and the camera flies to it, at zoom 11 or
    /// closer.
    pub fn pick_ship(&mut self, ship: Ship, now_ms: f64) {
        self.flight.set_place(None);
        self.flight.set_following(false);
        let (lon, lat) = (ship.lon, ship.lat);
        self.select_ship(Some(ship), now_ms);
        self.sync_flight(now_ms);
        let zoom = js_max(self.controller.view().zoom, 11.0);
        self.controller.fly_to(lon, lat, Some(zoom), now_ms);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex};

    use rustradar_core::model::NavStatus;
    use rustradar_core::settings::Mode;
    use rustradar_core::ship_list::{DEFAULT_SHIP_FILTERS, ShipMotion};

    use super::*;
    use crate::dev::demo_trains;
    use crate::state::flight::MarkedPlace;

    /// The local clock at the first frame.
    const T0: f64 = 1_782_036_000_000.0;
    /// Gothenburg harbour (V6).
    const V6: (f64, f64, f64) = (57.70, 11.93, 13.0);
    const SIZE: [f64; 2] = [1440.0, 900.0];

    type Listed = Result<Vec<Ship>, String>;
    type Carded = Result<ShipDetails, String>;

    /// The requests the ships state made, with the senders that answer them.
    #[derive(Default)]
    struct Log {
        asked: Vec<(Area, f64, Sender<Listed>)>,
        cards: Vec<(u32, Sender<Carded>)>,
    }

    /// Canned feeds: every request waits until the test answers it.
    #[derive(Clone, Default)]
    struct Canned(Arc<Mutex<Log>>);

    impl ShipFeeds for Canned {
        fn ships_in_view(&mut self, area: Area, zoom: f64) -> Answer<Listed> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().asked.push((area, zoom, tx));
            rx.into()
        }

        fn ship_details(&mut self, mmsi: u32) -> Answer<Carded> {
            let (tx, rx) = mpsc::channel();
            self.0.lock().unwrap().cards.push((mmsi, tx));
            rx.into()
        }
    }

    impl Canned {
        fn log(&self) -> std::sync::MutexGuard<'_, Log> {
            self.0.lock().unwrap()
        }

        fn requests(&self) -> usize {
            self.log().asked.len()
        }

        /// Answers the latest ships request.
        fn answer(&self, list: Listed) {
            self.log().asked.last().expect("a request").2.send(list).unwrap();
        }

        fn cards(&self) -> Vec<u32> {
            self.log().cards.iter().map(|(mmsi, _)| *mmsi).collect()
        }

        /// Answers the latest card request.
        fn answer_card(&self, card: Carded) {
            self.log().cards.last().expect("a card request").1.send(card).unwrap();
        }
    }

    fn view((latitude, longitude, zoom): (f64, f64, f64)) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing: 0.0, pitch: 0.0, position: None }
    }

    /// The whole app's state on canned ship feeds at `at`, ships the focus.
    fn app(canned: &Canned, at: (f64, f64, f64)) -> AppState {
        let mut s = AppState::new(Startup::default(), None, None, None, view(at), SIZE);
        s.persisted.settings.mode = Mode::Ships;
        s.ships = Ships::with_feeds(Some(Box::new(canned.clone())));
        s
    }

    /// The first poll at V6: nothing asked yet (the camera has to settle).
    fn started(canned: &Canned) -> AppState {
        let mut s = app(canned, V6);
        s.poll(T0);
        s
    }

    /// The first poll, the settle and an answer of `list`: the ships on show at `T0 + 700`.
    fn loaded(canned: &Canned, list: &[Ship]) -> AppState {
        let mut s = started(canned);
        s.poll(T0 + SHIPS_SETTLE_MS);
        canned.answer(Ok(list.to_vec()));
        s.poll(T0 + 700.0);
        assert_eq!(s.ships.ships(), list);
        s
    }

    /// A ship in Gothenburg's harbour.
    fn ship(mmsi: u32, kind: u8) -> Ship {
        Ship {
            mmsi,
            lat: 57.70 + f64::from(mmsi % 100) * 0.001,
            lon: 11.93,
            name: format!("SHIP {mmsi}"),
            kind,
            size: 1,
            heading: Some(90.0),
            old: false,
            dims: Some([100, 20, 10, 10]),
            frames: Vec::new(),
        }
    }

    fn details(mmsi: u32, name: &str) -> ShipDetails {
        ShipDetails {
            name: Some(name.into()),
            mmsi,
            nav_status: Some(NavStatus { code: 0, label: "Under way using engine".into() }),
            ..Default::default()
        }
    }

    #[test]
    fn ship_zoom_steps_and_sizes_known() {
        // `Math.min(16, Math.max(2, Math.floor(zoom + 1)))`
        assert_eq!(ship_zoom(0.5), 2.0);
        assert_eq!(ship_zoom(1.9), 2.0);
        assert_eq!(ship_zoom(2.0), 3.0);
        assert_eq!(ship_zoom(12.99), 13.0);
        assert_eq!(ship_zoom(13.0), 14.0);
        assert_eq!(ship_zoom(15.5), 16.0);
        assert_eq!(ship_zoom(22.0), 16.0);
        assert_eq!(ship_zoom(-3.0), 2.0);
        assert!(ship_zoom(f64::NAN).is_nan(), "as JavaScript's Math.min and Math.max");
        // the backend's feed zoom agrees wherever the zoom is a number
        for z in [0.0, 1.0, 4.4, 5.0, 9.99, 13.0, 16.0, 30.0] {
            assert_eq!(ship_zoom(z), f64::from(backend_ships::feed_zoom(z)), "{z}");
        }
        assert!(!sizes_known(5.99), "step 6");
        assert!(sizes_known(6.0), "step 7");
    }

    #[test]
    fn the_camera_settles_600_ms_then_every_60_s() {
        let canned = Canned::default();
        let mut s = started(&canned);
        assert_eq!(canned.requests(), 0, "nothing before the camera has stood 600 ms");
        let wants = s.poll(T0 + 599.0);
        assert_eq!(wants.wake_at, Some(T0 + SHIPS_SETTLE_MS));
        assert_eq!(canned.requests(), 0);
        s.poll(T0 + 600.0);
        assert_eq!(canned.requests(), 1);
        assert!(s.ships.loading());
        {
            let log = canned.log();
            let (area, zoom, _) = &log.asked[0];
            assert_eq!(*zoom, 13.0);
            let expected = view_area(geo::View { longitude: 11.93, latitude: 57.70, zoom: 13.0, pitch: 0.0 }, 1440.0, 900.0).unwrap();
            assert_eq!(*area, expected);
        }
        // the refresh is set again as each request starts, whenever its answer comes
        assert_eq!(s.poll(T0 + 700.0).wake_at, Some(T0 + 600.0 + SHIPS_REFRESH_MS));
        s.poll(T0 + 600.0 + SHIPS_REFRESH_MS - 1.0);
        assert_eq!(canned.requests(), 1);
        s.poll(T0 + 600.0 + SHIPS_REFRESH_MS);
        assert_eq!(canned.requests(), 2, "a second request without an answer to the first");
        s.poll(T0 + 600.0 + 2.0 * SHIPS_REFRESH_MS);
        assert_eq!(canned.requests(), 3);
    }

    #[test]
    fn the_whole_world_is_asked_for_when_most_of_the_globe_is_in_sight() {
        let canned = Canned::default();
        let mut s = app(&canned, (35.0, 10.0, 1.9));
        s.poll(T0);
        s.poll(T0 + 600.0);
        let log = canned.log();
        let (area, zoom, _) = &log.asked[0];
        assert_eq!(*area, WORLD);
        assert_eq!(*zoom, 1.9);
    }

    #[test]
    fn a_moving_camera_waits_and_the_refresh_still_comes() {
        let canned = Canned::default();
        let mut s = started(&canned);
        s.poll(T0 + 600.0);
        assert_eq!(canned.requests(), 1);
        // the camera moves every 300 ms (following an aircraft): the settle never ends, the 60 s
        // refresh still comes
        let mut t = T0 + 600.0;
        let mut lon = 11.93;
        while t < T0 + 600.0 + SHIPS_REFRESH_MS + 1000.0 {
            t += 300.0;
            lon += 0.00001;
            s.controller.set_view(view((57.70, lon, 13.0)));
            assert!(!s.poll(t).scene, "a move alone changes nothing on the map");
        }
        assert_eq!(canned.requests(), 2, "only the refresh asked");
        // the refresh cleared the settle: it waits for the next move
        let refreshed = T0 + 600.0 + SHIPS_REFRESH_MS;
        assert!(t > refreshed);
        // it stands: 600 ms after the last move it is asked for
        let stood = t;
        s.poll(stood + 599.0);
        assert_eq!(canned.requests(), 2);
        s.poll(stood + 600.0);
        assert_eq!(canned.requests(), 3);
        // the same view reported again is no move
        s.controller.set_view(view((57.70, lon, 13.0)));
        s.poll(stood + 700.0);
        s.poll(stood + 1300.0);
        assert_eq!(canned.requests(), 3);
    }

    #[test]
    fn a_refresh_that_comes_during_the_settle_cancels_it() {
        let canned = Canned::default();
        let mut s = started(&canned);
        s.poll(T0 + 600.0);
        // a move 300 ms before the refresh is due: the refresh asks, and the settle it cleared does
        // not ask again
        let refresh = T0 + 600.0 + SHIPS_REFRESH_MS;
        s.controller.set_view(view((57.70, 11.931, 13.0)));
        s.poll(refresh - 300.0);
        s.poll(refresh);
        assert_eq!(canned.requests(), 2);
        s.poll(refresh + 300.0);
        s.poll(refresh + 1000.0);
        assert_eq!(canned.requests(), 2);
        assert_eq!(s.poll(refresh + 1100.0).wake_at, Some(refresh + SHIPS_REFRESH_MS));
    }

    #[test]
    fn an_answer_is_kept_and_a_failure_keeps_the_ships() {
        let canned = Canned::default();
        let mut s = started(&canned);
        s.poll(T0 + 600.0);
        let list = vec![ship(1, 4), ship(2, 3)];
        canned.answer(Ok(list.clone()));
        let wants = s.poll(T0 + 800.0);
        assert!(wants.scene);
        assert_eq!(s.ships.ships(), &list[..]);
        assert_eq!(s.ships.ships_at(), T0 + 800.0);
        assert!(s.ships.loaded() && !s.ships.failed() && !s.ships.loading());
        assert_eq!(s.ships.on_map(), &list[..]);
        // the next request fails: the ships on show stay, the list says so
        s.poll(T0 + 600.0 + SHIPS_REFRESH_MS);
        canned.answer(Err("VesselFinder is resting".into()));
        let wants = s.poll(T0 + 600.0 + SHIPS_REFRESH_MS + 100.0);
        assert!(!wants.scene, "nothing new to draw");
        assert!(s.ships.failed());
        assert_eq!(s.ships.ships(), &list[..]);
        assert_eq!(s.ships.ships_at(), T0 + 800.0);
        // the next answer clears it, and replaces the set even when it is shorter
        s.poll(T0 + 600.0 + 2.0 * SHIPS_REFRESH_MS);
        canned.answer(Ok(vec![ship(2, 3)]));
        s.poll(T0 + 600.0 + 2.0 * SHIPS_REFRESH_MS + 100.0);
        assert!(!s.ships.failed());
        assert_eq!(s.ships.ships(), &[ship(2, 3)][..]);
    }

    #[test]
    fn the_request_counter_drops_an_older_requests_answer() {
        let canned = Canned::default();
        let mut s = started(&canned);
        s.poll(T0 + 600.0);
        // a small move and a settle: a second request before the first is answered
        s.controller.set_view(view((57.7001, 11.93, 13.0)));
        s.poll(T0 + 700.0);
        s.poll(T0 + 1300.0);
        assert_eq!(canned.requests(), 2);
        // the first one's answer, and its failure, find nobody waiting
        assert!(canned.log().asked[0].2.send(Ok(vec![ship(9, 4)])).is_err());
        assert!(canned.log().asked[0].2.send(Err("late".into())).is_err());
        s.poll(T0 + 1400.0);
        assert!(s.ships.ships().is_empty() && !s.ships.failed());
        assert!(s.ships.loading(), "the second one is still out");
        canned.answer(Ok(vec![ship(1, 4)]));
        s.poll(T0 + 1500.0);
        assert_eq!(s.ships.ships(), &[ship(1, 4)][..]);
    }

    #[test]
    fn an_answer_for_another_zoom_step_or_a_view_left_behind_is_dropped() {
        let canned = Canned::default();
        let mut s = app(&canned, (57.70, 11.93, 12.9));
        s.poll(T0);
        s.poll(T0 + 600.0);
        assert_eq!(canned.log().asked[0].1, 12.9);
        // zoomed in past 13 before the answer: the feed's step is 14 now, not 13
        s.controller.set_view(view((57.70, 11.93, 13.05)));
        canned.answer(Ok(vec![ship(1, 4)]));
        s.poll(T0 + 700.0);
        assert!(s.ships.ships().is_empty(), "the wrong density for this zoom");
        assert!(!s.ships.loaded() && !s.ships.failed());
        // asked at 13.05 once stood; an answer at 13.9 (the same step) is kept
        s.poll(T0 + 1300.0);
        assert_eq!(canned.requests(), 2);
        s.controller.set_view(view((57.70, 11.93, 13.9)));
        canned.answer(Ok(vec![ship(1, 4)]));
        s.poll(T0 + 1400.0);
        assert_eq!(s.ships.ships(), &[ship(1, 4)][..]);
        // asked at 13.9, then the camera is in Stockholm: dropped
        s.poll(T0 + 2000.0);
        assert_eq!(canned.requests(), 3);
        s.controller.set_view(view((59.33, 18.07, 13.9)));
        canned.answer(Ok(vec![ship(2, 4)]));
        s.poll(T0 + 2100.0);
        assert_eq!(s.ships.ships(), &[ship(1, 4)][..], "Gothenburg's ships are not shown over Stockholm");
        assert_eq!(s.ships.ships_at(), T0 + 1400.0);
    }

    #[test]
    fn ships_off_the_map_stop_clear_and_deselect() {
        let canned = Canned::default();
        let list = vec![ship(1, 4), ship(2, 3)];
        let mut s = loaded(&canned, &list);
        s.select_ship(Some(list[0].clone()), T0 + 800.0);
        assert_eq!(s.ships.selected_mmsi(), Some(1));
        // a request out when they go off
        s.poll(T0 + 600.0 + SHIPS_REFRESH_MS);
        assert_eq!(canned.requests(), 2);
        s.persisted.settings.mode = Mode::Aircraft;
        let wants = s.poll(T0 + 61_000.0);
        assert!(wants.scene);
        assert!(s.ships.ships().is_empty() && !s.ships.loaded() && !s.ships.failed());
        assert!(s.ships.on_map().is_empty());
        assert_eq!(s.ships.selected(), None, "the selected ship closes");
        assert_eq!(s.ships.card(), None);
        // the answer on its way is dropped (nobody waits for it any more); no timer is left
        assert!(canned.log().asked.last().unwrap().2.send(Ok(list.clone())).is_err());
        let wants = s.poll(T0 + 61_100.0);
        assert!(s.ships.ships().is_empty());
        assert_eq!(wants.wake_at, None);
        s.poll(T0 + 200_000.0);
        assert_eq!(canned.requests(), 2);
        // off again with nothing to clear: no scene refresh asked
        s.controller.set_view(view((57.71, 11.93, 13.0)));
        assert!(!s.poll(T0 + 200_100.0).scene);
        // back on, as a kind also shown: asked once the camera has stood 600 ms
        s.persisted.settings.also_show = vec![Mode::Ships];
        s.poll(T0 + 300_000.0);
        assert_eq!(canned.requests(), 2);
        s.poll(T0 + 300_600.0);
        assert_eq!(canned.requests(), 3);
    }

    #[test]
    fn the_card_is_asked_on_selection_and_every_minute_and_a_refresh_keeps_it() {
        let canned = Canned::default();
        let list = vec![ship(1, 4), ship(2, 3)];
        let mut s = loaded(&canned, &list);
        let t = T0 + 1000.0;
        s.select_ship(Some(list[0].clone()), t);
        assert_eq!(canned.cards(), [1], "asked at once");
        // the skeletons: the card is this ship's, with nothing in it yet
        assert_eq!(s.ships.card(), Some(&ShipCard { mmsi: 1, details: None, error: None }));
        assert_eq!(s.ships.card_details(), None);
        assert_eq!(s.ships.card_error(), None);
        canned.answer_card(Ok(details(1, "STENA DANICA")));
        s.poll(t + 100.0);
        assert_eq!(s.ships.card_details(), Some(&details(1, "STENA DANICA")));
        // every minute while selected; the card stays on show while it is asked again
        s.poll(t + 200.0);
        assert_eq!(s.ships.card_due, Some(t + SHIP_DETAILS_REFRESH_MS));
        s.poll(t + SHIP_DETAILS_REFRESH_MS - 1.0);
        assert_eq!(canned.cards().len(), 1);
        s.poll(t + SHIP_DETAILS_REFRESH_MS);
        assert_eq!(canned.cards(), [1, 1]);
        assert_eq!(s.ships.card_details(), Some(&details(1, "STENA DANICA")), "a refresh keeps the card");
        // a failed refresh keeps it too
        canned.answer_card(Err("resting".into()));
        s.poll(t + SHIP_DETAILS_REFRESH_MS + 100.0);
        assert_eq!(s.ships.card_details(), Some(&details(1, "STENA DANICA")));
        assert_eq!(s.ships.card_error(), None);
        // a frame late: the next one keeps its minute
        s.poll(t + 2.0 * SHIP_DETAILS_REFRESH_MS + 40.0);
        assert_eq!(canned.cards().len(), 3);
        s.poll(t + 2.0 * SHIP_DETAILS_REFRESH_MS + 50.0);
        assert_eq!(s.ships.card_due, Some(t + 3.0 * SHIP_DETAILS_REFRESH_MS));
        canned.answer_card(Ok(details(1, "STENA DANICA II")));
        s.poll(t + 2.0 * SHIP_DETAILS_REFRESH_MS + 100.0);
        assert_eq!(s.ships.card_details().and_then(|d| d.name.as_deref()), Some("STENA DANICA II"));

        // another ship: the skeletons, then a failure, which the panel shows
        let t = t + 150_000.0;
        s.select_ship(Some(list[1].clone()), t);
        assert_eq!(s.ships.card(), Some(&ShipCard { mmsi: 2, details: None, error: None }));
        canned.answer_card(Err("VesselFinder is resting".into()));
        s.poll(t + 100.0);
        assert_eq!(s.ships.card_error(), Some("VesselFinder is resting"));
        assert_eq!(s.ships.card_details(), None);
        // Retry: the skeletons again, at once; the minute keeps its time
        s.ships.retry_card();
        assert_eq!(s.ships.card(), Some(&ShipCard { mmsi: 2, details: None, error: None }));
        assert_eq!(*canned.cards().last().unwrap(), 2);
        let asked = canned.cards().len();
        s.poll(t + 5000.0);
        assert_eq!(s.ships.card_due, Some(t + SHIP_DETAILS_REFRESH_MS));
        canned.answer_card(Ok(details(2, "GERD")));
        s.poll(t + 5100.0);
        assert_eq!(s.ships.card_details(), Some(&details(2, "GERD")));
        // a retry while a card is on show keeps it while it asks
        s.ships.retry_card();
        assert_eq!(s.ships.card_details(), Some(&details(2, "GERD")));
        assert_eq!(canned.cards().len(), asked + 1);
        // deselected: no card, no timer
        s.select_ship(None, t + 6000.0);
        assert_eq!(s.ships.card(), None);
        s.persisted.settings.mode = Mode::Aircraft;
        assert_eq!(s.poll(t + 7000.0).wake_at, None);
    }

    #[test]
    fn a_card_answer_for_an_earlier_ship_or_request_is_dropped() {
        let canned = Canned::default();
        let list = vec![ship(1, 4), ship(2, 3)];
        let mut s = loaded(&canned, &list);
        s.select_ship(Some(list[0].clone()), T0 + 1000.0);
        s.select_ship(Some(list[1].clone()), T0 + 1100.0);
        assert_eq!(canned.cards(), [1, 2]);
        // the first ship's card finds nobody waiting
        assert!(canned.log().cards[0].1.send(Ok(details(1, "A"))).is_err());
        s.poll(T0 + 1200.0);
        assert_eq!(s.ships.card(), Some(&ShipCard { mmsi: 2, details: None, error: None }));
        // a retry while the second is out: only the retry's answer counts
        s.ships.retry_card();
        assert_eq!(canned.cards(), [1, 2, 2]);
        assert!(canned.log().cards[1].1.send(Err("first".into())).is_err());
        canned.answer_card(Ok(details(2, "B")));
        s.poll(T0 + 1300.0);
        assert_eq!(s.ships.card_details(), Some(&details(2, "B")));
        // the same ship again does nothing: no new request, the card stays
        s.select_ship(Some(list[1].clone()), T0 + 1400.0);
        assert_eq!(canned.cards().len(), 3);
        assert_eq!(s.ships.card_details(), Some(&details(2, "B")));
    }

    #[test]
    fn the_selected_ship_follows_its_row_and_stays_when_the_list_drops_it() {
        let canned = Canned::default();
        let mut s = loaded(&canned, &[ship(1, 4), ship(2, 3)]);
        s.select_ship(Some(ship(1, 4)), T0 + 1000.0);
        s.poll(T0 + 1100.0);
        // the next answer has it moved on
        let mut moved = ship(1, 4);
        moved.lat += 0.01;
        moved.heading = Some(95.0);
        s.poll(T0 + 600.0 + SHIPS_REFRESH_MS);
        canned.answer(Ok(vec![moved.clone(), ship(2, 3)]));
        let wants = s.poll(T0 + 600.0 + SHIPS_REFRESH_MS + 100.0);
        assert!(wants.scene);
        assert_eq!(s.ships.selected(), Some(&moved));
        // then the list drops it: it stays where it was last seen, drawn after the others
        s.poll(T0 + 600.0 + 2.0 * SHIPS_REFRESH_MS);
        canned.answer(Ok(vec![ship(2, 3)]));
        s.poll(T0 + 600.0 + 2.0 * SHIPS_REFRESH_MS + 100.0);
        assert_eq!(s.ships.selected(), Some(&moved));
        assert_eq!(s.ships.on_map(), &[ship(2, 3), moved.clone()][..]);
        assert_eq!(s.ships.filtered_len(), 1);
    }

    #[test]
    fn the_map_draws_the_filtered_ships_and_the_selection() {
        let canned = Canned::default();
        let list = vec![ship(1, 4), ship(2, 3), ship(3, 6)];
        let mut s = loaded(&canned, &list);
        assert_eq!(s.ships.on_map(), &list[..]);
        let generation = s.ships.on_map_generation();
        // passenger ships left out: the map draws the rest
        s.persisted.ship_filters.hidden_kinds = vec![3];
        assert!(s.poll(T0 + 800.0).scene);
        assert_eq!(s.ships.on_map(), &[ship(1, 4), ship(3, 6)][..]);
        assert_eq!(s.ships.filtered().map(|s| s.mmsi).collect::<Vec<_>>(), [1, 3]);
        assert!(s.ships.on_map_generation() > generation);
        // the order of the list is not the map's business
        let generation = s.ships.on_map_generation();
        s.persisted.ship_filters.sort = ShipSort::Name;
        assert!(!s.poll(T0 + 900.0).scene);
        assert_eq!(s.ships.on_map_generation(), generation);
        // a ship the filters keep, selected: the same list (today's same array)
        s.select_ship(Some(ship(3, 6)), T0 + 1000.0);
        assert!(s.poll(T0 + 1100.0).scene, "the selection is drawn");
        assert_eq!(s.ships.on_map_generation(), generation);
        assert_eq!(s.ships.on_map(), &[ship(1, 4), ship(3, 6)][..]);
        // one they leave out: drawn after them, a new list
        s.select_ship(Some(ship(2, 3)), T0 + 1200.0);
        s.poll(T0 + 1300.0);
        assert_eq!(s.ships.on_map(), &[ship(1, 4), ship(3, 6), ship(2, 3)][..]);
        assert!(s.ships.on_map_generation() > generation);
        // deselected: back to the filtered ships, a new list again
        let generation = s.ships.on_map_generation();
        s.select_ship(None, T0 + 1400.0);
        s.poll(T0 + 1500.0);
        assert_eq!(s.ships.on_map(), &[ship(1, 4), ship(3, 6)][..]);
        assert!(s.ships.on_map_generation() > generation);
        // the filters back to their defaults
        s.persisted.ship_filters = DEFAULT_SHIP_FILTERS;
        s.poll(T0 + 1600.0);
        assert_eq!(s.ships.on_map(), &list[..]);
        // moored ships only: none of these
        s.persisted.ship_filters.motion = ShipMotion::Moored;
        s.poll(T0 + 1700.0);
        assert!(s.ships.on_map().is_empty());
    }

    #[test]
    fn the_list_searches_filters_and_sorts() {
        let canned = Canned::default();
        let mut big = ship(5, 4);
        big.size = 2;
        big.name = "ÄLVSBORG".into();
        let mut unnamed = ship(7, 4);
        unnamed.name = "7".into();
        let list = vec![ship(1, 4), ship(2, 3), big.clone(), unnamed.clone()];
        let s = loaded(&canned, &list);
        let mmsis = |ships: Vec<&Ship>| ships.iter().map(|s| s.mmsi).collect::<Vec<_>>();
        let mut filters = DEFAULT_SHIP_FILTERS;
        let here = LatLon { lat: 57.70, lon: 11.93 };
        // by size: the large one first, then by name, the unnamed one last
        assert_eq!(mmsis(s.ships.listed(&filters, "", here)), [5, 1, 2, 7]);
        assert_eq!(mmsis(s.ships.listed(&filters, "  ", here)), [5, 1, 2, 7]);
        assert_eq!(mmsis(s.ships.listed(&filters, "alvs", here)), [5], "folded");
        assert_eq!(mmsis(s.ships.listed(&filters, "7", here)), [7], "an MMSI from the start");
        filters.hidden_kinds = vec![3];
        assert_eq!(mmsis(s.ships.listed(&filters, "", here)), [5, 1, 7]);
        // nearest first from the list's centre (ship n lies n·0.001° north of the harbour)
        filters.sort = ShipSort::Distance;
        assert_eq!(mmsis(s.ships.listed(&filters, "", here)), [1, 5, 7]);
        assert_eq!(mmsis(s.ships.listed(&filters, "", LatLon { lat: 57.71, lon: 11.93 })), [7, 5, 1]);
    }

    #[test]
    fn a_ship_closes_the_flight_the_train_and_the_board_and_they_close_the_ship() {
        let canned = Canned::default();
        let mut s = loaded(&canned, &[ship(1, 4), ship(2, 3)]);
        let train = demo_trains(57.70, 11.93, T0 as i64)[0].clone();
        s.flight.set_focus_airport(Some("got".into()));
        s.select_flight(Some(42), SelectOptions::default(), T0 + 100.0);
        assert_eq!(s.flight.selected_id(), Some(42));
        s.select_train(Some(train.clone()), T0 + 150.0);
        assert!(s.trains.selected().is_some());
        // a ship: the flight, the train and the board close at once
        s.select_ship(Some(ship(1, 4)), T0 + 200.0);
        assert_eq!(s.flight.selected_id(), None);
        assert_eq!(s.trains.selected(), None);
        assert_eq!(s.flight.focus_airport(), None);
        s.poll(T0 + 300.0);
        assert_eq!(s.ships.selected_mmsi(), Some(1), "nothing else is open");
        // a flight selected afterwards closes the ship at the next poll
        s.select_flight(Some(43), SelectOptions::default(), T0 + 400.0);
        let wants = s.poll(T0 + 500.0);
        assert!(wants.scene);
        assert_eq!(s.ships.selected(), None);
        assert_eq!(s.ships.card(), None);
        assert_eq!(s.flight.selected_id(), Some(43));
        // so does a train
        s.select_ship(Some(ship(2, 3)), T0 + 600.0);
        assert_eq!(s.flight.selected_id(), None);
        s.select_train(Some(train), T0 + 700.0);
        s.poll(T0 + 800.0);
        assert_eq!(s.ships.selected(), None);
        assert!(s.trains.selected().is_some());
        // and an airport's board
        s.select_ship(Some(ship(1, 4)), T0 + 900.0);
        assert_eq!(s.trains.selected(), None);
        s.poll(T0 + 950.0);
        assert_eq!(s.ships.selected_mmsi(), Some(1));
        s.flight.set_focus_airport(Some("GOT".into()));
        s.poll(T0 + 1000.0);
        assert_eq!(s.ships.selected(), None);
    }

    #[test]
    fn a_pick_flies_to_the_ship_and_lets_go_of_the_locks() {
        let canned = Canned::default();
        let mut s = loaded(&canned, &[ship(1, 4)]);
        s.controller.set_view(view((57.70, 11.93, 9.0)));
        s.flight.set_place(Some(MarkedPlace { lon: 12.0, lat: 57.0, name: "Kungsbacka".into() }));
        s.flight.set_following(true);
        s.ships.set_following(true);
        let mut far = ship(3, 6);
        far.lat = 57.65;
        far.lon = 11.80;
        s.pick_ship(far.clone(), T0 + 1000.0);
        assert!(s.flight.place().is_none());
        assert!(!s.flight.following());
        assert!(!s.ships.following(), "a new ship: the camera lets go");
        assert_eq!(s.ships.selected(), Some(&far));
        s.advance_camera(T0 + 1000.0 + 10_000.0);
        let v = s.controller.view();
        assert!((v.longitude - 11.80).abs() < 1e-9 && (v.latitude - 57.65).abs() < 1e-9, "{v:?}");
        assert_eq!(v.zoom, 11.0, "zoom 11 from 9");
        // already closer in: the zoom stays
        s.controller.set_view(view((57.65, 11.80, 14.0)));
        s.pick_ship(ship(1, 4), T0 + 20_000.0);
        s.advance_camera(T0 + 40_000.0);
        assert_eq!(s.controller.view().zoom, 14.0);
        // the vehicle lock: C toggles it, the same ship again keeps it, another ship lets go
        s.ships.toggle_following();
        assert!(s.ships.following());
        s.select_ship(Some(ship(1, 4)), T0 + 41_000.0);
        assert!(s.ships.following());
        s.poll(T0 + 41_100.0);
        assert!(s.ships.following(), "a refresh of the same ship keeps it");
        s.select_ship(Some(ship(3, 6)), T0 + 42_000.0);
        assert!(!s.ships.following());
    }

    #[test]
    fn a_fixture_folder_answers_the_ships_and_their_cards_at_the_first_frame() {
        let dir = std::env::temp_dir().join(format!("rustradar-ships-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(FIXTURE_SHIP_DETAILS)).unwrap();
        let list = vec![ship(1, 4), ship(2, 3)];
        std::fs::write(dir.join(FIXTURE_SHIPS), serde_json::to_vec(&list).unwrap()).unwrap();
        std::fs::write(dir.join(FIXTURE_SHIP_DETAILS).join("1.json"), serde_json::to_vec(&details(1, "STENA DANICA")).unwrap()).unwrap();
        let startup = Startup { fixture: Some(dir.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, view(V6), SIZE);
        s.persisted.settings.also_show = vec![Mode::Ships];
        let wants = s.poll(T0);
        assert!(wants.scene);
        assert_eq!(s.ships.ships(), &list[..], "at the first frame: a shot's clock stands still");
        assert_eq!(s.ships.on_map(), &list[..]);
        assert_eq!(s.ships.refresh_due, Some(T0 + SHIPS_REFRESH_MS));
        s.select_ship(Some(list[0].clone()), T0 + 100.0);
        s.poll(T0 + 200.0);
        assert_eq!(s.ships.card_details(), Some(&details(1, "STENA DANICA")));
        // a ship without a card in the folder fails as a request does
        s.select_ship(Some(list[1].clone()), T0 + 300.0);
        s.poll(T0 + 400.0);
        assert!(s.ships.card_error().is_some_and(|e| e.contains("2.json")), "{:?}", s.ships.card_error());
        // the list is read again at each request: without it, a failure that keeps the ships
        std::fs::remove_file(dir.join(FIXTURE_SHIPS)).unwrap();
        s.poll(T0 + SHIPS_REFRESH_MS);
        assert!(s.ships.failed());
        assert_eq!(s.ships.ships(), &list[..]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_backend_there_are_no_ships() {
        let mut s = AppState::new(Startup::default(), None, None, None, view(V6), SIZE);
        s.persisted.settings.mode = Mode::Ships;
        s.poll(T0);
        assert!(s.ships.failed(), "the list says so");
        s.poll(T0 + 600.0);
        assert!(!s.ships.loading() && !s.ships.loaded());
        assert_eq!(s.poll(T0 + 700.0).wake_at, None, "no refresh without a source");
        // a ship can still be selected (from nowhere), but no card is asked
        s.select_ship(Some(ship(1, 4)), T0 + 800.0);
        assert_eq!(s.ships.selected_mmsi(), Some(1));
        assert_eq!(s.ships.card(), None);
        s.ships.retry_card();
        assert_eq!(s.ships.card(), None);
        assert_eq!(s.poll(T0 + 900.0).wake_at, None);
    }

    #[test]
    fn the_backends_card_refuses_mmsi_0_without_the_network() {
        let rt = crate::requests::runtime().expect("runtime");
        rustradar_backend::set_rt(rt.handle().clone());
        let mut feeds = BackendFeeds { requests: Requests::new(rt.handle().clone(), crate::requests::no_wake()) };
        let answer = feeds.ship_details(0);
        let started = std::time::Instant::now();
        let got = loop {
            if let Some(v) = answer.poll() {
                break v;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(10), "no answer");
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(got, Err("not a ship".to_owned()));
    }

    #[test]
    fn freshness_once_the_ships_have_come() {
        let canned = Canned::default();
        let mut s = started(&canned);
        assert_eq!(s.ships.freshness(T0 + 1000.0), None);
        s.poll(T0 + 600.0);
        canned.answer(Ok(vec![]));
        s.poll(T0 + 1000.0);
        assert!(s.ships.loaded(), "an empty answer is an answer");
        assert_eq!(s.ships.freshness(T0 + 30_000.0).map(|f| f.stale), Some(false));
        assert_eq!(s.ships.freshness(T0 + 1000.0 + 10.0 * SHIPS_REFRESH_MS).map(|f| f.stale), Some(true));
        assert_eq!(s.ships.freshness(T0).map(|f| f.label), s.ships.freshness(T0 + 1000.0).map(|f| f.label), "never a negative age");
    }
}
