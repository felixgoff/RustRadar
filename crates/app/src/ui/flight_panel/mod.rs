//! The flight panel (items U10 and U11; doc 06 §4, §7; `FlightPanel.svelte`): the selected
//! flight's details in the shell's detail panel slot ([`super::shell::Slots::flight_panel`]).
//!
//! Top to bottom, as today's markup has it (built on [`super::panel`]):
//! 1. **Back row** (only when the flight was opened from an airport board that is still open):
//!    `ArrowLeft` 14/2 and `{code} departures and arrivals`, 12 px `--text-2` (`--text-1` on
//!    `--surface-hover` under the pointer), padding `4px 16px`, a rule under it; it closes the
//!    flight, so the board shows again ([`Action::CloseFlight`]).
//! 2. **Strip** (`1fr auto auto 40px`): the callsign (`callsign || flight || "No callsign"`, Mono
//!    22/700) over the flight number (`flight || schedule.flight_number || "No flight number"`);
//!    the type code over the registration; the altitude (`altitudeLabel`, in `--amber`) over the
//!    ground speed (`{n} kt`), dimmed to `--text-3` with the tooltip `As last reported` once the
//!    stream ended; the close button (`Close flight details`: [`Action::CloseFlight`]).
//! 3. **Photo**: while loading (no details and not ended) a skeleton block; else the aircraft's
//!    picture (`medium || thumbnail`) when it has any, its credit `© {copyright}`, a click opening
//!    its JetPhotos page; with more than one, the previous and next buttons (wrapping) and the
//!    count. The block is 2:1, 5:2 under a back row or for a diverted flight. Which picture is
//!    shown goes back to the first when the flight changes.
//! 4. **Airline row** (when the callsign's ICAO prefix names a known airline or the schedule has a
//!    logo id): the logo on a 26 px `--text-1` tile (`2px 6px` padding, the picture at most 110 ×
//!    22, left out when it fails), 12 px from the name (13/600); padding `12px 16px 0`.
//! 5. **Route** (padding 16, gaps 8, a rule under it): the two airports in two columns 12 apart,
//!    the destination's right-aligned: the code (Mono 22/700, a text button underlined in
//!    `--amber` 3 px under its baseline under the pointer, `Departures and arrivals at {name}`:
//!    [`Action::OpenAirportOf`]; `—` without one), the city (12 px, cut with `…`, the airport's
//!    name as its tooltip), `{label} <Clock>` in the airport's zone (12 px `--text-2`), the delay
//!    badge (`punctuality`) 4 px under it. Once ended, an arrival that is still an estimate dims
//!    and its badge turns neutral. Then the progress track (2 px `--line-control`, 4 px above and
//!    under, 7 px in): the `--amber` fill to `progress_pct` and the `Plane` 14/2 turned 45° at its
//!    end, both easing 600 ms, `--text-3` once ended; a `--line` rule without progress data. Then
//!    `{km} flown · {km} to go · {duration} left` (12 px `--text-2`), or while loading a 12 px
//!    skeleton 70 % wide; `Diverted to {name}` (bad badge) when diverted.
//! 6. **Tabs** `Overview`, `Route`, `Aircraft`, `History` ([`super::widgets::tabs`]): the tab is
//!    kept while the panel stays open (another flight picked from the history keeps it) and back
//!    to Overview when the panel opens again.
//! 7. **Tab panel** (padding 16, gaps 16): [`overview`] (U10); the Route, Aircraft and History
//!    tabs are item U11's, the altitude chart in the overview item U12's ([`Parts`]).
//! 8. **Footer** ([`footer`]), pinned to the bottom of the panel.
//!
//! What the panel shows is read from the state each frame ([`View::of`]): the row shown for the
//! selection, the details with OpenSky's points, the route, the follow stream's state and last
//! report, sign-in, adsb.lol's measurements while recent, the camera's lock. What it asks goes
//! back as [`Action`]s; links open through egui (`OpenUrl`).
//!
//! **Checked** against Chromium's layout of today's component, server-rendered by Svelte with the
//! same props and laid out with app.css at 1440 × 900 (the loading, live, ended, diverted and
//! from-board states of `tests/flight_panel.rs`): every part's box, rule, badge, button and icon
//! lands on Chromium's pixels, or within one; text sits up to a pixel lower (egui's baseline, as
//! everywhere in the UI).
//!
//! **For other items**: [`Parts`] is where U11 puts the Route, Aircraft and History tabs and U12
//! the altitude chart with its export menu (each a [`Slot`] drawn from the [`View`] in the tab
//! panel's width); [`super::panel`] holds the anatomy the train, ship and airport panels share.
//! The photo and the airline logo load through egui's image loaders ([`super::panel::remote`]):
//! item U18 installs the loader for `https:` URIs; until then the photo shows its
//! `--surface-raised` block and the logo is left out, as when they fail today.
//!
//! Deviations:
//! - Times without a zone are in UTC: the app has no viewer time zone yet (U6's note).
//! - Digits are Overpass's own proportional figures (`font-variant-numeric: tabular-nums` on the
//!   distances needs OpenType features egui does not have; U6's note).
//! - The tooltips are the app's own (`ui::widgets::tooltip`), not the system's.
//! - The phone order (doc 06 §7.6) is item U19's.

pub mod aircraft;
pub mod footer;
pub mod history;
pub mod overview;
pub mod route;

use egui::text::LayoutJob;
use egui::{Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, Ui, Vec2};
use rustradar_core::labels::altitude_label;
use rustradar_core::model::{AdsbAircraft, AircraftInfo, Airline, Airport, FlightDetails, FlightInfo, FlightProgress, LiveFlight, ScheduleInfo, airline_logo_url};
use rustradar_core::num::js_round;
use rustradar_core::reference::airline_icao;
use rustradar_core::routes::Route;
use rustradar_core::text::group_thousands;
use rustradar_core::time::{Clocks, Punctuality, Tone as TimeTone, delay_minutes, duration, punctuality};

use super::fonts::Face;
use super::icons::{self, Icon};
use super::panel::{self, Cell, Fit, Part, Photo, Piece, Remote};
use super::shell::Pass;
use super::text::{self, Style};
use super::widgets::badge::{Badge, Tone};
use super::widgets::button::transition_color;
use super::widgets::tabs::{Tab as TabItem, Tabs};
use super::widgets::{skeleton, tooltip};
use super::{anim, theme};
use crate::state::flight::FollowState;
use crate::state::{Action, AppState};

/// The panel's accessible name (`aria-label`).
pub const LABEL: &str = "Selected flight";
/// The close button's accessible name.
pub const CLOSE_LABEL: &str = "Close flight details";
/// The strip's callsign and flight number when there are none.
pub const NO_CALLSIGN: &str = "No callsign";
pub const NO_FLIGHT_NUMBER: &str = "No flight number";
/// The dash of a value not known.
pub const DASH: &str = "—";
/// The altitude cell's tooltip once ended.
pub const AS_LAST_REPORTED: &str = "As last reported";
/// The photo's tooltip.
pub const PHOTO_TOOLTIP: &str = "View this photo on JetPhotos";
/// The back row's text after the board's code.
pub const BACK_SUFFIX: &str = " departures and arrivals";

/// The back row's padding (`var(--space-1) var(--space-4)`) and icon.
const BACK_PAD: Vec2 = Vec2::new(theme::SPACE_4, theme::SPACE_1);
const BACK_ICON: f32 = 14.0;
/// The airline row: padding, gap, the logo tile's height and padding, the logo's largest size.
const AIRLINE_PAD: Vec2 = Vec2::new(theme::SPACE_4, theme::SPACE_3);
const AIRLINE_GAP: f32 = theme::SPACE_3;
const LOGO_TILE: f32 = 26.0;
const LOGO_PAD: Vec2 = Vec2::new(6.0, 2.0);
const LOGO_MAX: Vec2 = Vec2::new(110.0, 22.0);
/// The route section: padding, gap, the airports' column gap, a column's line gap, the badge's
/// margin, the code's underline offset.
const ROUTE_PAD: f32 = theme::SPACE_4;
const ROUTE_GAP: f32 = theme::SPACE_2;
const AIRPORTS_GAP: f32 = theme::SPACE_3;
const LINE_GAP: f32 = 2.0;
const BADGE_MARGIN: f32 = 2.0;
const UNDERLINE_OFFSET: f32 = 3.0;
/// The progress track: height, vertical margin, inset, marker.
const TRACK: f32 = 2.0;
const TRACK_MARGIN: Vec2 = Vec2::new(7.0, theme::SPACE_1);
const MARKER: f32 = 14.0;
/// The loading skeleton under the track: 12 px high, 70 % wide.
const LINE_SKELETON: Vec2 = Vec2::new(0.7, 12.0);
/// The tab panel's padding and gap.
pub const TABPANEL_PAD: f32 = theme::SPACE_4;
pub const TABPANEL_GAP: f32 = theme::SPACE_4;

/// A tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Overview,
    Route,
    Aircraft,
    History,
}

/// The tabs in order (`TABS`).
pub const TABS: [TabItem<'static, Tab>; 4] =
    [TabItem::new(Tab::Overview, "Overview"), TabItem::new(Tab::Route, "Route"), TabItem::new(Tab::Aircraft, "Aircraft"), TabItem::new(Tab::History, "History")];

/// A part of the panel another item draws: into `ui` (the tab panel's width, its padding round
/// it) from the panel's [`View`].
pub type Slot = fn(&mut Ui, &View<'_>, &mut Pass<'_>);

/// The parts other items draw: the Route, Aircraft and History tabs (U11) and the overview's
/// altitude chart with its export menu (U12). A part no item fills is not drawn.
#[derive(Debug, Clone, Copy, Default)]
pub struct Parts {
    pub route: Option<Slot>,
    pub aircraft: Option<Slot>,
    pub history: Option<Slot>,
    pub chart: Option<Slot>,
}

impl Parts {
    /// The app's parts: each item puts its function here.
    pub const APP: Parts = Parts { route: Some(route::show), aircraft: Some(aircraft::show), history: Some(history::show), chart: None };

    fn tab(&self, tab: Tab) -> Option<Slot> {
        match tab {
            Tab::Overview => None,
            Tab::Route => self.route,
            Tab::Aircraft => self.aircraft,
            Tab::History => self.history,
        }
    }
}

/// A departure or an arrival as the route shows it: its label, its time (Unix s, 0 none) and its
/// punctuality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct When {
    pub label: &'static str,
    pub time: u32,
    pub delay: Option<Punctuality>,
}

/// What the panel shows, read from the state (the component's props and deriveds).
pub struct View<'a> {
    /// The row shown for the selection (`flight`).
    pub flight: &'a LiveFlight,
    /// The details with OpenSky's points in front of the trail (`details`).
    pub details: Option<&'a FlightDetails>,
    pub route: Option<&'a Route>,
    pub status: FollowState,
    /// The newest report when the stream ended (feed clock ms; 0 otherwise).
    pub last_seen_ms: f64,
    pub authenticated: bool,
    /// How many track points came from the OpenSky Network.
    pub opensky_added: usize,
    /// What the transponder reports, from adsb.lol, while recent.
    pub measured: Option<&'a AdsbAircraft>,
    /// The camera follows the aircraft.
    pub following: bool,
    /// The back row's label, when the flight was opened from a board that is still open.
    pub back: Option<String>,
    /// The airline the callsign names (`reference.airline(airlineIcao(callsign))`).
    pub airline: Option<&'a Airline>,
    /// Local time, Unix ms.
    pub now_ms: f64,
    /// The viewer's clocks.
    pub clocks: Clocks,
}

/// `a || b`.
fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

impl<'a> View<'a> {
    /// The panel's view of `state` at local time `now_ms`, when a flight is selected.
    pub fn of(state: &'a AppState, now_ms: f64) -> Option<View<'a>> {
        let flight = state.flight.selected()?;
        let back = (state.flight.from_board()).then(|| state.airport.code()).flatten().map(|code| format!("{code}{BACK_SUFFIX}"));
        let airline = state.aircraft.reference().and_then(|r| r.airline(airline_icao(&flight.callsign)));
        Some(View {
            flight,
            details: state.flight.details(),
            route: state.flight.route(),
            status: state.flight.follow_state(),
            last_seen_ms: state.flight.last_seen_ms(),
            authenticated: state.aircraft.authenticated(),
            opensky_added: state.flight.opensky_added(),
            measured: state.flight.measured(now_ms, state.persisted.settings.adsb),
            following: state.flight.following(),
            back,
            airline,
            now_ms,
            clocks: Clocks::default(),
        })
    }

    /// `details.flight`.
    pub fn info(&self) -> Option<&'a FlightInfo> {
        self.details.and_then(|d| d.flight.as_ref())
    }

    /// `details.aircraft`.
    pub fn aircraft(&self) -> Option<&'a AircraftInfo> {
        self.details.and_then(|d| d.aircraft.as_ref())
    }

    /// `details.schedule`.
    pub fn schedule(&self) -> Option<&'a ScheduleInfo> {
        self.details.and_then(|d| d.schedule.as_ref())
    }

    /// `details.progress`.
    pub fn progress(&self) -> Option<&'a FlightProgress> {
        self.details.and_then(|d| d.progress.as_ref())
    }

    /// No details yet and the stream not ended (`loading`).
    pub fn loading(&self) -> bool {
        self.details.is_none() && self.status != FollowState::Ended
    }

    /// The stream ended (`ended`): the live values stay on show as the last ones, dimmed.
    pub fn ended(&self) -> bool {
        self.status == FollowState::Ended
    }

    /// When the last live data was timed, Unix s, 0 if none came (`seenAt`).
    pub fn seen_at(&self) -> f64 {
        let ms = if self.last_seen_ms != 0.0 && !self.last_seen_ms.is_nan() {
            self.last_seen_ms
        } else {
            (self.flight.timestamp_ms as f64).max(self.info().map_or(0.0, |i| i.timestamp_ms as f64))
        };
        (ms / 1000.0).floor()
    }

    /// The departure airport's zone, which "Last seen" reads in (`seenZone`).
    pub fn seen_zone(&self) -> Option<&'a str> {
        self.origin().map(|a| a.timezone.as_str())
    }

    /// Whose time "Last seen" is in (`seenPlace`).
    pub fn seen_place(&self) -> String {
        match self.seen_zone() {
            Some(zone) if !zone.is_empty() => format!("{} time", or(self.origin().map_or("", |a| a.iata.as_str()), &self.flight.origin)),
            _ => "your time zone".to_owned(),
        }
    }

    /// `info?.altitude ?? flight.alt`.
    pub fn altitude(&self) -> i32 {
        self.info().map_or(self.flight.alt, |i| i.altitude)
    }

    /// `info?.on_ground ?? flight.onGround`.
    pub fn on_ground(&self) -> bool {
        self.info().map_or(self.flight.on_ground, |i| i.on_ground)
    }

    /// The altitude as aviation writes it (`level`).
    pub fn level(&self) -> String {
        altitude_label(f64::from(self.altitude()), self.on_ground())
    }

    /// `{info?.ground_speed ?? flight.speed} kt`.
    pub fn speed(&self) -> String {
        format!("{} kt", self.info().map_or(self.flight.speed, |i| i.ground_speed))
    }

    /// `schedule?.painted_as_id || schedule?.operated_by_id || 0`.
    pub fn logo_id(&self) -> u32 {
        self.schedule().map_or(0, |s| if s.painted_as_id != 0 { s.painted_as_id } else { s.operated_by_id })
    }

    /// `route?.origin`.
    pub fn origin(&self) -> Option<&'a Airport> {
        self.route.and_then(|r| r.origin.as_ref())
    }

    /// `route?.destination`.
    pub fn destination(&self) -> Option<&'a Airport> {
        self.route.and_then(|r| r.destination.as_ref())
    }

    /// The code shown for the origin or the destination (`origin?.iata || flight.origin`).
    pub fn code(&self, destination: bool) -> &'a str {
        let (airport, fallback) = if destination { (self.destination(), &self.flight.destination) } else { (self.origin(), &self.flight.origin) };
        or(airport.map_or("", |a| a.iata.as_str()), fallback)
    }

    /// `departure`: `Departure` without a schedule, `Departed` with the actual time and its
    /// punctuality, else `Scheduled`.
    pub fn departure(&self) -> When {
        let Some(s) = self.schedule() else { return When { label: "Departure", time: 0, delay: None } };
        let late = punctuality(delay_minutes(s.scheduled_departure, s.actual_departure));
        if s.actual_departure != 0 {
            return When { label: "Departed", time: s.actual_departure, delay: late };
        }
        When { label: "Scheduled", time: s.scheduled_departure, delay: None }
    }

    /// `arrival`: `Arrival` without a schedule, `Arrived`, `Expected` with the estimate, else
    /// `Scheduled`; the punctuality against the actual time or the estimate.
    pub fn arrival(&self) -> When {
        let Some(s) = self.schedule() else { return When { label: "Arrival", time: 0, delay: None } };
        let eta = self.progress().map_or(0, |p| p.eta);
        let actual = if s.actual_arrival != 0 { s.actual_arrival } else { eta };
        let late = punctuality(delay_minutes(s.scheduled_arrival, actual));
        if s.actual_arrival != 0 {
            return When { label: "Arrived", time: s.actual_arrival, delay: late };
        }
        if eta != 0 {
            return When { label: "Expected", time: eta, delay: late };
        }
        When { label: "Scheduled", time: s.scheduled_arrival, delay: None }
    }

    /// `clamp(progress_pct, 0, 100)` (`pct`).
    pub fn pct(&self) -> f64 {
        f64::from(self.progress().map_or(0, |p| p.progress_pct)).clamp(0.0, 100.0)
    }

    /// Diverted to another airport than the scheduled one (`diverted`).
    pub fn diverted(&self) -> bool {
        self.schedule().is_some_and(|s| s.diverted_id != 0 && s.diverted_id != s.destination_id)
    }

    /// The flight on Flightradar24's site (`fr24Url`).
    pub fn fr24_url(&self) -> String {
        format!("https://www.flightradar24.com/{}/{:x}", encode_uri_component(or(&self.flight.callsign, "flight")), self.flight.id)
    }

    /// The aircraft's registration (`aircraft?.reg || flight.reg`).
    pub fn registration(&self) -> &'a str {
        or(self.aircraft().map_or("", |a| a.reg.as_str()), &self.flight.reg)
    }

    /// The type code (`aircraft?.typecode || flight.typecode`).
    pub fn typecode(&self) -> &'a str {
        or(self.aircraft().map_or("", |a| a.typecode.as_str()), &self.flight.typecode)
    }
}

/// JavaScript's `encodeURIComponent`: UTF-8, every byte but `A–Z a–z 0–9 - _ . ! ~ * ' ( )`
/// percent-encoded.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `km(m)`: whole kilometres with thousands separators.
pub fn km(metres: u32) -> String {
    format!("{} km", group_thousands(js_round(f64::from(metres) / 1000.0)))
}

/// The line under the track: `{km} flown · {km} to go`, and ` · {duration} left` while time
/// remains.
pub fn distances(p: &FlightProgress) -> String {
    let left = if p.remaining_time > 0 { format!(" · {} left", duration(f64::from(p.remaining_time))) } else { String::new() };
    format!("{} flown · {} to go{left}", km(p.traversed_distance), km(p.remaining_distance))
}

/// The tone of a badge for a punctuality's tone.
pub fn badge_tone(tone: TimeTone) -> Tone {
    match tone {
        TimeTone::Good => Tone::Good,
        TimeTone::Warn => Tone::Warn,
        TimeTone::Bad => Tone::Bad,
    }
}

/// What the panel keeps between frames: the tab, which picture, whether it was drawn last pass,
/// how far it is scrolled.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Kept {
    tab: Tab,
    photo: (u32, usize),
    pass: u64,
    offset: f32,
}

fn kept_id() -> Id {
    Id::unique("rustradar::flight_panel::kept")
}

/// The tab shown in `ctx`'s flight panel (Overview when it is not open).
pub fn tab(ctx: &egui::Context) -> Tab {
    ctx.data(|d| d.get_temp::<Kept>(kept_id())).map_or(Tab::Overview, |k| k.tab)
}

fn mounted_id() -> Id {
    Id::unique("rustradar::flight_panel::mounted")
}

/// The egui pass `ctx`'s flight panel was last mounted in (drawn after a pass without it), for the
/// parts that keep component state of their own (U11's history).
pub fn mounted_pass(ctx: &egui::Context) -> u64 {
    ctx.data(|d| d.get_temp::<u64>(mounted_id())).unwrap_or(0)
}

/// How far `ctx`'s flight panel was scrolled at its last pass, px.
pub fn scroll_offset(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp::<Kept>(kept_id())).map_or(0.0, |k| k.offset)
}

/// The panel in the shell's slot, with the app's [`Parts`].
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) {
    show_with(ui, pass, &Parts::APP);
}

/// The panel with `parts` in the other items' places.
pub fn show_with(ui: &mut Ui, pass: &mut Pass<'_>, parts: &Parts) {
    let state = pass.state;
    let Some(view) = View::of(state, pass.now_ms) else { return };
    let ctx = ui.ctx().clone();
    let id = Id::unique("rustradar::flight_panel");
    let now_pass = ctx.cumulative_pass_nr();
    let mut kept = ctx.data(|d| d.get_temp::<Kept>(kept_id())).unwrap_or_default();
    // mounted again after a pass without the panel: the component's state starts afresh
    if kept.pass + 1 < now_pass {
        kept = Kept::default();
        ctx.data_mut(|d| d.insert_temp(mounted_id(), now_pass));
    }
    if kept.photo.0 != view.flight.id {
        kept.photo = (view.flight.id, 0);
    }
    kept.pass = now_pass;
    let footer = footer::footer(&ctx, &view);
    let footer_h = footer.height(&ctx, ui.available_width());
    let mut footer = Some(footer);
    let scrolled = panel::body(ui, id, footer_h, |ui, part| match part {
        Part::Content => content(ui, id, &view, pass, parts, &mut kept),
        Part::Footer(rect) => {
            if let Some(footer) = footer.take() {
                let responses = footer.show(ui, rect);
                footer::act(ui.ctx(), &view, &responses, pass.actions);
            }
        }
    });
    kept.offset = scrolled.offset;
    ctx.data_mut(|d| d.insert_temp(kept_id(), kept));
}

fn content(ui: &mut Ui, id: Id, view: &View<'_>, pass: &mut Pass<'_>, parts: &Parts, kept: &mut Kept) {
    if let Some(label) = &view.back
        && back_row(ui, id, label)
    {
        pass.actions.push(Action::CloseFlight);
    }
    if strip(ui, view).clicked() {
        pass.actions.push(Action::CloseFlight);
    }
    photo(ui, id, view, kept);
    airline(ui, view);
    if let Some(destination) = route(ui, id, view) {
        pass.actions.push(Action::OpenAirportOf { destination });
    }
    Tabs::new("rustradar::flight_panel::tabs", &TABS).show(ui, &mut kept.tab);
    let width = ui.available_width();
    let inner = Rect::from_min_size(ui.cursor().min + Vec2::splat(TABPANEL_PAD), Vec2::new(width - 2.0 * TABPANEL_PAD, f32::INFINITY));
    let used = panel::region(ui, "tabpanel", inner, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.set_width(inner.width());
        match kept.tab {
            Tab::Overview => overview::show(ui, id, view, pass, parts),
            tab => {
                if let Some(slot) = parts.tab(tab) {
                    slot(ui, view, pass);
                }
            }
        }
        ui.min_rect().height().max(0.0)
    });
    ui.allocate_exact_size(Vec2::new(width, used + 2.0 * TABPANEL_PAD), Sense::hover());
}

/// The back row; answers whether it was clicked.
fn back_row(ui: &mut Ui, id: Id, label: &str) -> bool {
    let style = Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2);
    let width = ui.available_width();
    let height = BACK_PAD.y * 2.0 + style.line_px();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height + theme::BORDER), Sense::hover());
    let button = Rect::from_min_size(rect.min, Vec2::new(width, height));
    let response = ui.interact(button, id.with("back"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(panel::snap_rect(button, ui.ctx().pixels_per_point()), CornerRadius::ZERO, theme::SURFACE_HOVER);
    }
    let color = if response.hovered() { theme::TEXT_1 } else { theme::TEXT_2 };
    let mid = button.center().y;
    icons::paint(painter, Rect::from_center_size(Pos2::new(button.left() + BACK_PAD.x + BACK_ICON / 2.0, mid), Vec2::splat(BACK_ICON)), Icon::ArrowLeft.at(BACK_ICON, icons::STROKE_REGULAR), color);
    let galley = painter.layout_job(text::job(label, &style.color(color)));
    painter.galley(Pos2::new(button.left() + BACK_PAD.x + BACK_ICON + theme::SPACE_1, mid - galley.size().y / 2.0), galley, color);
    panel::hrule(painter, rect.x_range(), button.bottom());
    if response.has_focus() {
        super::widgets::button::focus_ring(painter, button, 0);
    }
    response.clicked()
}

/// The strip; answers the close button's response.
fn strip(ui: &mut Ui, view: &View<'_>) -> egui::Response {
    let schedule_number = view.schedule().map_or("", |s| s.flight_number.as_str());
    let title = or(or(&view.flight.callsign, &view.flight.flight), NO_CALLSIGN);
    let number = or(or(&view.flight.flight, schedule_number), NO_FLIGHT_NUMBER);
    let ended = view.ended();
    let (level_color, sub_color) = if ended { (theme::TEXT_3, theme::TEXT_3) } else { (theme::AMBER, theme::TEXT_2) };
    let cells = vec![
        Cell::fill(vec![
            Piece::text(title, &panel::title_style(Face::Mono), Fit::Wrap),
            Piece::text(number, &panel::sub_style(Face::Ui), Fit::Ellipsis),
        ]),
        Cell::auto(vec![
            Piece::text(or(view.typecode(), DASH), &panel::value_style(theme::TEXT_1), Fit::Ellipsis),
            Piece::text(or(view.registration(), DASH), &panel::sub_style(Face::Mono), Fit::Ellipsis),
        ]),
        Cell::auto(vec![
            Piece::text(&view.level(), &panel::value_style(level_color), Fit::Ellipsis),
            Piece::text(&view.speed(), &panel::sub_style(Face::Mono).color(sub_color), Fit::Ellipsis),
        ])
        .tooltip(ended.then(|| AS_LAST_REPORTED.to_owned())),
    ];
    panel::strip(ui, cells, CLOSE_LABEL)
}

/// The photo block: the skeleton while loading, else the aircraft's pictures.
fn photo(ui: &mut Ui, id: Id, view: &View<'_>, kept: &mut Kept) {
    let aspect = if view.back.is_some() || view.diverted() { panel::PHOTO_ASPECT_SHORT } else { panel::PHOTO_ASPECT };
    if view.loading() {
        panel::photo_skeleton(ui, aspect);
        return;
    }
    let images = view.aircraft().map_or(&[][..], |a| a.images.as_slice());
    if images.is_empty() {
        return;
    }
    let n = images.len();
    let index = kept.photo.1 % n;
    let image = &images[index];
    let uri = or(&image.medium, &image.thumbnail);
    let response = panel::photo(
        ui,
        id.with("photo"),
        aspect,
        Photo {
            uri,
            credit: (!image.copyright.is_empty()).then(|| format!("© {}", image.copyright)),
            tooltip: PHOTO_TOOLTIP,
            count: (n > 1).then_some((index, n)),
        },
    );
    if response.opened {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&image.url));
    }
    match response.step {
        -1 => kept.photo.1 = (kept.photo.1 + n - 1) % n,
        1 => kept.photo.1 = (kept.photo.1 + 1) % n,
        _ => {}
    }
}

/// The airline row.
fn airline(ui: &mut Ui, view: &View<'_>) {
    let logo_id = view.logo_id();
    if view.airline.is_none() && logo_id == 0 {
        return;
    }
    let ctx = ui.ctx().clone();
    let style = Style::ui(theme::TEXT_MD, 600, theme::TEXT_1);
    // the logo: its tile, the picture in it (none while it loads), or left out when it failed
    let logo = (logo_id != 0).then(|| panel::remote(&ctx, &airline_logo_url(logo_id))).filter(|r| *r != Remote::Failed);
    let picture = match logo {
        Some(Remote::Ready(texture)) => Some((texture.id, panel::contain(texture.size, LOGO_MAX))),
        _ => None,
    };
    let tile = logo.map(|_| Vec2::new(picture.map_or(0.0, |p| p.1.x) + 2.0 * LOGO_PAD.x, LOGO_TILE + 2.0 * LOGO_PAD.y));
    let width = ui.available_width();
    let name_left = AIRLINE_PAD.x + tile.map_or(0.0, |t| t.x + AIRLINE_GAP);
    let name = view.airline.map_or("", |a| a.name.as_str());
    let galley = ctx.fonts_mut(|f| f.layout_job(panel::fit(text::job(name, &style), Fit::Wrap, width - AIRLINE_PAD.x - name_left)));
    let name_h = style.line_px() * galley.rows.len().max(1) as f32;
    let row = tile.map_or(0.0, |t| t.y).max(name_h);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, AIRLINE_PAD.y + row), Sense::hover());
    let painter = ui.painter();
    let ppp = ctx.pixels_per_point();
    let mid = rect.top() + AIRLINE_PAD.y + row / 2.0;
    if let Some(tile) = tile {
        let tile_rect = Rect::from_min_size(Pos2::new(rect.left() + AIRLINE_PAD.x, mid - tile.y / 2.0), tile);
        painter.rect_filled(panel::snap_rect(tile_rect, ppp), CornerRadius::same(theme::RADIUS), theme::TEXT_1);
        if let Some((texture, size)) = picture {
            let at = Rect::from_center_size(tile_rect.center(), size);
            painter.image(texture, at, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
    }
    let rows = galley.rows.len().max(1) as f32;
    painter.galley(Pos2::new(rect.left() + name_left, mid - name_h / 2.0 + (name_h - galley.size().y) / 2.0 / rows), galley, theme::TEXT_1);
}

/// One side of the route: the code, its airport, its time.
struct Side<'a> {
    code: &'a str,
    airport: Option<&'a Airport>,
    when: When,
    end: bool,
    stale: bool,
}

/// The route section; answers which code was clicked (`true`: the destination's).
fn route(ui: &mut Ui, id: Id, view: &View<'_>) -> Option<bool> {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let width = ui.available_width();
    let inner = width - 2.0 * ROUTE_PAD;
    let col_w = (inner - AIRPORTS_GAP) / 2.0;
    let ended = view.ended();
    let actual_arrival = view.schedule().is_some_and(|s| s.actual_arrival != 0);
    let sides = [
        Side { code: view.code(false), airport: view.origin(), when: view.departure(), end: false, stale: false },
        Side { code: view.code(true), airport: view.destination(), when: view.arrival(), end: true, stale: ended && !actual_arrival },
    ];
    let code_style = Style { line_height: panel::TITLE_LINE_HEIGHT, ..panel::data(&Style::ui(theme::TEXT_XL, 700, theme::TEXT_1)) };
    let small = Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_1);
    // each column's parts, laid out at the column's width
    struct Column {
        city: Option<std::sync::Arc<egui::Galley>>,
        when: std::sync::Arc<egui::Galley>,
        when_h: f32,
        badge: Option<(Badge, Vec2)>,
        height: f32,
    }
    let columns: Vec<Column> = sides
        .iter()
        .map(|side| {
            let city = side.airport.map(|a| or(&a.city, &a.name)).filter(|c| !c.is_empty());
            let city = city.map(|c| ctx.fonts_mut(|f| f.layout_job(panel::fit(text::job(c, &small), Fit::Ellipsis, col_w))));
            let when_style = small.color(if side.stale { theme::TEXT_3 } else { theme::TEXT_2 });
            let mut job = LayoutJob::default();
            text::append(&mut job, side.when.label, &when_style);
            text::append(&mut job, " ", &when_style);
            panel::append_clock(&mut job, &view.clocks, side.when.time, side.airport.map(|a| a.timezone.as_str()), &when_style);
            let when = ctx.fonts_mut(|f| f.layout_job(panel::fit(job, Fit::Wrap, col_w)));
            let when_h = when_style.line_px() * when.rows.len().max(1) as f32;
            let badge = side.when.delay.as_ref().map(|d| {
                let badge = Badge::new(&d.label, if side.stale { Tone::Neutral } else { badge_tone(d.tone) });
                let size = badge.measure(&ctx);
                (badge, size)
            });
            let height = code_style.line_px() + LINE_GAP + small.line_px() + LINE_GAP + when_h + badge.as_ref().map_or(0.0, |b| LINE_GAP + BADGE_MARGIN + b.1.y);
            Column { city, when, when_h, badge, height }
        })
        .collect();
    let airports_h = columns.iter().map(|c| c.height).fold(0.0, f32::max);
    let progress = view.progress();
    let loading = view.loading();
    let below = if progress.is_some() {
        ROUTE_GAP + small.line_px()
    } else if loading {
        ROUTE_GAP + LINE_SKELETON.y
    } else {
        0.0
    };
    let diverted = view.diverted().then(|| {
        let name = view.destination().map_or("another airport", |a| a.name.as_str());
        let badge = Badge::new(format!("Diverted to {name}"), Tone::Bad);
        let size = badge.measure(&ctx);
        (badge, size)
    });
    let track_h = 2.0 * TRACK_MARGIN.y + TRACK;
    let height = 2.0 * ROUTE_PAD + airports_h + ROUTE_GAP + track_h + below + diverted.as_ref().map_or(0.0, |d| ROUTE_GAP + BADGE_MARGIN + d.1.y);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height + theme::BORDER), Sense::hover());
    let painter = ui.painter().clone();
    let top = rect.top() + ROUTE_PAD;
    let mut clicked = None;
    for (side, column) in sides.iter().zip(columns) {
        let left = rect.left() + ROUTE_PAD + if side.end { col_w + AIRPORTS_GAP } else { 0.0 };
        let right = left + col_w;
        let place = |w: f32| if side.end { right - w } else { left };
        let mut y = top;
        // the code: a text button, or a dash
        let code = or(side.code, "");
        let shown = if code.is_empty() { DASH } else { code };
        let galley = painter.layout_job(text::job(shown, &code_style));
        let code_rect = Rect::from_min_size(Pos2::new(place(galley.size().x), y), Vec2::new(galley.size().x, code_style.line_px()));
        let text_top = y + (code_style.line_px() - galley.size().y) / 2.0;
        let baseline = text_top + galley.rows.first().map_or(galley.size().y, |r| r.pos.y + r.glyphs.first().map_or(0.0, |g| g.pos.y));
        painter.galley(Pos2::new(code_rect.left(), text_top), galley, theme::TEXT_1);
        if !code.is_empty() {
            let response = ui.interact(code_rect, id.with(("code", side.end)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, code));
            // `text-decoration: underline transparent` turning `--amber` over 150 ms
            let target = if response.hovered() { theme::AMBER } else { Color32::TRANSPARENT };
            let color = transition_color(&ctx, response.id.with("underline"), target, anim::HOVER);
            if color.a() > 0 {
                let line = Rect::from_x_y_ranges(code_rect.x_range(), baseline + UNDERLINE_OFFSET..=baseline + UNDERLINE_OFFSET + 1.0);
                painter.rect_filled(panel::snap_rect(line, ppp), CornerRadius::ZERO, color);
            }
            if response.has_focus() {
                super::widgets::button::focus_ring(&painter, code_rect, 0);
            }
            let name = side.airport.map_or(code, |a| a.name.as_str());
            let response = tooltip::show(response, &format!("Departures and arrivals at {name}"));
            if response.clicked() {
                clicked = Some(side.end);
            }
        }
        y += code_style.line_px() + LINE_GAP;
        if let Some(city) = column.city {
            let city_rect = Rect::from_min_size(Pos2::new(place(city.size().x), y), Vec2::new(city.size().x, small.line_px()));
            painter.galley(Pos2::new(city_rect.left(), y + (small.line_px() - city.size().y) / 2.0), city, theme::TEXT_1);
            if let Some(airport) = side.airport {
                tooltip::show(ui.interact(city_rect, id.with(("city", side.end)), Sense::hover()), &airport.name);
            }
        }
        y += small.line_px() + LINE_GAP;
        let rows = column.when.rows.len().max(1) as f32;
        let when_x = place(column.when.size().x);
        painter.galley(Pos2::new(when_x, y + (column.when_h - column.when.size().y) / 2.0 / rows), column.when, theme::TEXT_2);
        y += column.when_h;
        if let Some((badge, size)) = column.badge {
            y += LINE_GAP + BADGE_MARGIN;
            badge.paint(&painter, Pos2::new(place(size.x), y));
        }
    }
    // the progress track
    let mut y = top + airports_h + ROUTE_GAP + TRACK_MARGIN.y;
    let track = Rect::from_min_size(Pos2::new(rect.left() + ROUTE_PAD + TRACK_MARGIN.x, y), Vec2::new(inner - 2.0 * TRACK_MARGIN.x, TRACK));
    match progress {
        Some(p) => {
            painter.rect_filled(panel::snap_rect(track, ppp), CornerRadius::ZERO, theme::LINE_CONTROL);
            let pct = anim::value(&ctx, id.with("progress"), view.pct() as f32, anim::PROGRESS) / 100.0;
            let color = if ended { theme::TEXT_3 } else { theme::AMBER };
            let fill = Rect::from_min_size(track.min, Vec2::new(track.width() * pct, TRACK));
            painter.rect_filled(panel::snap_rect(fill, ppp), CornerRadius::ZERO, color);
            let at = Pos2::new(track.left() + track.width() * pct, track.center().y);
            icons::paint(&painter, Rect::from_center_size(at, Vec2::splat(MARKER)), Icon::Plane.at(MARKER, icons::STROKE_REGULAR).rotated(45f32.to_radians()), color);
            y += TRACK + TRACK_MARGIN.y + ROUTE_GAP;
            let style = small.color(if ended { theme::TEXT_3 } else { theme::TEXT_2 });
            let galley = painter.layout_job(panel::fit(text::job(&distances(p), &style), Fit::Wrap, inner));
            painter.galley(Pos2::new(rect.left() + ROUTE_PAD, y + (style.line_px() - galley.size().y) / 2.0), galley, style.color);
            y += style.line_px();
        }
        None => {
            painter.rect_filled(panel::snap_rect(track, ppp), CornerRadius::ZERO, theme::LINE);
            y += TRACK + TRACK_MARGIN.y;
            if loading {
                y += ROUTE_GAP;
                let line = Rect::from_min_size(Pos2::new(rect.left() + ROUTE_PAD, y), Vec2::new(inner * LINE_SKELETON.x, LINE_SKELETON.y));
                skeleton::paint(&painter, panel::snap_rect(line, ppp), theme::RADIUS);
                y += LINE_SKELETON.y;
            }
        }
    }
    if let Some((badge, _)) = diverted {
        y += ROUTE_GAP + BADGE_MARGIN;
        badge.paint(&painter, Pos2::new(rect.left() + ROUTE_PAD, y));
    }
    panel::hrule(&painter, rect.x_range(), rect.top() + height);
    clicked
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::{AircraftImage, FlightProgress};

    use super::*;

    fn flight() -> LiveFlight {
        LiveFlight {
            id: 0x3a8f21c4,
            callsign: "SAS1417".into(),
            flight: "SK1417".into(),
            reg: "SE-ROR".into(),
            typecode: "A20N".into(),
            origin: "ARN".into(),
            destination: "CPH".into(),
            alt: 36_000,
            speed: 448,
            ..Default::default()
        }
    }

    fn view<'a>(flight: &'a LiveFlight, details: Option<&'a FlightDetails>, status: FollowState) -> View<'a> {
        View {
            flight,
            details,
            route: None,
            status,
            last_seen_ms: 0.0,
            authenticated: false,
            opensky_added: 0,
            measured: None,
            following: false,
            back: None,
            airline: None,
            now_ms: 1_782_036_000_000.0,
            clocks: Clocks::default(),
        }
    }

    fn schedule() -> ScheduleInfo {
        ScheduleInfo {
            flight_number: "SK1417".into(),
            origin_id: 1,
            destination_id: 2,
            scheduled_departure: 1_782_033_900,
            scheduled_arrival: 1_782_038_100,
            ..Default::default()
        }
    }

    #[test]
    fn departure_and_arrival_read_as_today() {
        let f = flight();
        let none = view(&f, None, FollowState::Connecting);
        assert_eq!(none.departure(), When { label: "Departure", time: 0, delay: None });
        assert_eq!(none.arrival(), When { label: "Arrival", time: 0, delay: None });
        assert!(none.loading() && !none.ended());

        let mut d = FlightDetails { schedule: Some(schedule()), ..Default::default() };
        let v = view(&f, Some(&d), FollowState::Live);
        assert_eq!(v.departure(), When { label: "Scheduled", time: 1_782_033_900, delay: None });
        assert_eq!(v.arrival(), When { label: "Scheduled", time: 1_782_038_100, delay: None });
        assert!(!v.loading());

        // 18 minutes late off the stand, an estimate 17 minutes late
        d.schedule.as_mut().unwrap().actual_departure = 1_782_034_980;
        d.progress = Some(FlightProgress { eta: 1_782_039_120, ..Default::default() });
        let v = view(&f, Some(&d), FollowState::Live);
        let dep = v.departure();
        assert_eq!((dep.label, dep.time), ("Departed", 1_782_034_980));
        assert_eq!(dep.delay.as_ref().map(|p| (p.label.as_str(), p.tone)), Some(("18 min late", TimeTone::Warn)));
        let arr = v.arrival();
        assert_eq!((arr.label, arr.time), ("Expected", 1_782_039_120));
        assert_eq!(arr.delay.as_ref().map(|p| p.label.as_str()), Some("17 min late"));

        // arrived early: the actual time and its verdict, over the estimate
        d.schedule.as_mut().unwrap().actual_arrival = 1_782_037_500;
        let v = view(&f, Some(&d), FollowState::Ended);
        let arr = v.arrival();
        assert_eq!((arr.label, arr.time), ("Arrived", 1_782_037_500));
        assert_eq!(arr.delay.map(|p| (p.label, p.tone)), Some(("10 min early".to_owned(), TimeTone::Good)));
        assert!(v.ended() && !v.loading(), "ended is not loading");
    }

    #[test]
    fn the_strip_and_route_values() {
        let mut f = flight();
        let v = view(&f, None, FollowState::Connecting);
        assert_eq!(v.level(), "FL360");
        assert_eq!(v.speed(), "448 kt");
        assert_eq!(v.code(false), "ARN");
        assert_eq!(v.code(true), "CPH");
        assert_eq!(v.fr24_url(), "https://www.flightradar24.com/SAS1417/3a8f21c4");
        f.callsign = String::new();
        f.on_ground = true;
        let v = view(&f, None, FollowState::Connecting);
        assert_eq!(v.fr24_url(), "https://www.flightradar24.com/flight/3a8f21c4");
        assert_eq!(v.level(), "GND");
        // the stream's values over the feed's
        let d = FlightDetails {
            flight: Some(FlightInfo { altitude: 4_500, ground_speed: 210, on_ground: false, timestamp_ms: 1_782_036_001_000, ..Default::default() }),
            aircraft: Some(AircraftInfo { reg: "SE-RXA".into(), typecode: String::new(), ..Default::default() }),
            ..Default::default()
        };
        let v = view(&f, Some(&d), FollowState::Live);
        assert_eq!(v.level(), "4,500 ft");
        assert_eq!(v.speed(), "210 kt");
        assert_eq!(v.registration(), "SE-RXA");
        assert_eq!(v.typecode(), "A20N", "the feed's when the details have none");
        assert_eq!(v.seen_at(), 1_782_036_001.0);
    }

    #[test]
    fn distances_and_progress() {
        let p = FlightProgress { traversed_distance: 312_400, remaining_distance: 210_600, remaining_time: 3_120, progress_pct: 60, ..Default::default() };
        assert_eq!(distances(&p), "312 km flown · 211 km to go · 52 min left");
        let p = FlightProgress { traversed_distance: 1_234_500, remaining_distance: 0, remaining_time: 0, progress_pct: 140, ..Default::default() };
        assert_eq!(distances(&p), "1,235 km flown · 0 km to go");
        let f = flight();
        let d = FlightDetails { progress: Some(p), ..Default::default() };
        assert_eq!(view(&f, Some(&d), FollowState::Live).pct(), 100.0);
        assert_eq!(view(&f, None, FollowState::Live).pct(), 0.0);
    }

    #[test]
    fn uri_components_are_encoded_as_javascript_encodes_them() {
        assert_eq!(encode_uri_component("SAS1417"), "SAS1417");
        assert_eq!(encode_uri_component("A B/c?é"), "A%20B%2Fc%3F%C3%A9");
        assert_eq!(encode_uri_component("-_.!~*'()"), "-_.!~*'()");
    }

    #[test]
    fn diverted_and_the_logo() {
        let f = flight();
        let mut s = schedule();
        s.operated_by_id = 9;
        let d = FlightDetails { schedule: Some(s.clone()), ..Default::default() };
        let v = view(&f, Some(&d), FollowState::Live);
        assert!(!v.diverted());
        assert_eq!(v.logo_id(), 9);
        s.painted_as_id = 12;
        s.diverted_id = 2;
        let d = FlightDetails { schedule: Some(s.clone()), ..Default::default() };
        let v = view(&f, Some(&d), FollowState::Live);
        assert_eq!(v.logo_id(), 12, "the livery's airline first");
        assert!(!v.diverted(), "diverted to where it was going is not diverted");
        s.diverted_id = 3;
        let d = FlightDetails { schedule: Some(s), aircraft: Some(AircraftInfo { images: vec![AircraftImage::default()], ..Default::default() }), ..Default::default() };
        assert!(view(&f, Some(&d), FollowState::Live).diverted());
    }
}
