//! The flight panel's History tab (item U11; doc 06 §7.4; `FlightPanel.svelte`'s
//! `tab === "history"`), drawn in the tab panel's width ([`super::Parts::history`]).
//!
//! **The tab's own ask** ([`Loads`], the component's `history` and its `$effect`): the aircraft's
//! other flights are asked for when the tab shows a registration (`aircraft.reg || flight.reg`)
//! they were not asked for yet, once per registration while the panel stays open (another flight
//! of the same aircraft picked from the list asks nothing); an answer for an earlier registration
//! is dropped; `Try again` asks again. The panel opened again starts afresh, as the component
//! mounts again ([`super::mounted_pass`]). Asked of the `--fixture` folder ([`FIXTURE_HISTORY`]:
//! `{ registration: [HistoryRow] }`, a registration it does not name having no flights), else of
//! the backend (`aircraft_history`), else of nothing: then it fails at once, as `invoke` does
//! without Tauri.
//!
//! What the tab shows:
//! - no registration: `This aircraft's registration isn't known, so its other flights can't be
//!   looked up.` (13 px `--text-2`);
//! - failed: `Couldn't load the flights of {reg}.` and 12 px under it `Try again` (`RefreshCw`
//!   14/1.75);
//! - loading: five skeleton lines 12 px high, `150 + (i·37 mod 90)` px wide, 8 px in, 30 px apart
//!   (a `.row` grid with the skeleton in its first cell and an empty second row);
//! - none: `No recent flights recorded for {reg}.`;
//! - the rows ([`row_view`]): each a button (padding 8, radius 4, 12 px; columns `74px 1fr auto`
//!   8 apart, rows 2 apart): the day (`dayAt` in the origin's zone, `--text-2`), the number (Mono
//!   650), the legs (`ARN – CPH`, Mono) in the last column; under them the times (`<Clock>` from
//!   and to, each in its airport's zone, `--text-3`) across the first two columns and the status
//!   right-aligned (`This flight` for the one shown) in its tone's colour (`--live`, `--warn`,
//!   `--danger`, else `--text-2`). The flight shown sits on `--amber-soft`; a live flight with an
//!   id is a button (`--surface-hover` under the pointer) that selects it and flies there
//!   ([`Action::PickFlight`]); the others ask nothing. Then `Live flights open on the map.` (11 px
//!   `--text-3`) right under the list.
//!
//! **Checked** against Chromium's layout of today's component, server-rendered by Svelte with the
//! same props and laid out with app.css at 1440 × 900 (`tests/flight_panel_tabs.rs`: the rows, the
//! loading lines, the failure): rows, columns, fills and skeletons on Chromium's pixels or within
//! one; text up to a pixel lower (egui's baseline).
//!
//! Deviations:
//! - The tab's ask runs from the UI pass, as the component's effect does today (as the train
//!   panel's, U14): no state file holds it.
//! - Times without a known zone are in UTC (the app has no viewer time zone yet, U6's note).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::text::LayoutJob;
use egui::{Color32, Context, CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, Ui, Vec2};
use rustradar_backend::Backend;
use rustradar_backend::convert;
use rustradar_core::model::HistoryRow;
use rustradar_core::reference::Reference;
use rustradar_core::time::{Clocks, Tone, tone_of};

use super::super::icons::{self, Icon};
use super::super::panel;
use super::super::shell::Pass;
use super::super::text::{self, Style};
use super::super::widgets::button::{Button, focus_ring};
use super::super::widgets::skeleton;
use super::super::theme;
use super::{DASH, View};
use crate::requests::Requests;
use crate::state::aircraft::Answer;
use crate::state::{Action, AppState};

/// The `--fixture` folder's flights of each aircraft: `{ registration: [HistoryRow] }`.
pub const FIXTURE_HISTORY: &str = "aircraft_history.json";
/// What the tab says without a registration, and its other messages.
pub const NO_REGISTRATION: &str = "This aircraft's registration isn't known, so its other flights can't be looked up.";
pub const TRY_AGAIN: &str = "Try again";
pub const LIVE_NOTE: &str = "Live flights open on the map.";
pub const THIS_FLIGHT: &str = "This flight";

/// The column's gap under the failure (`.stack`).
const STACK_GAP: f32 = theme::SPACE_3;
/// A row: its padding, the first column, the gaps between columns and rows.
const ROW_PAD: f32 = theme::SPACE_2;
const DAY_COLUMN: f32 = 74.0;
const COLUMN_GAP: f32 = theme::SPACE_2;
const ROW_GAP: f32 = 2.0;
/// The loading lines: how many, their height.
pub const PLACEHOLDERS: usize = 5;
const PLACEHOLDER_HEIGHT: f32 = 12.0;

/// `Couldn't load the flights of {reg}.`
pub fn failed_text(reg: &str) -> String {
    format!("Couldn't load the flights of {reg}.")
}

/// `No recent flights recorded for {reg}.`
pub fn empty_text(reg: &str) -> String {
    format!("No recent flights recorded for {reg}.")
}

/// The width of loading line `i` (`150 + ((i * 37) % 90)` px).
pub fn placeholder_width(i: usize) -> f32 {
    (150 + (i * 37) % 90) as f32
}

// ------------------------------------------------------------------------------------------------
// the tab's own ask

/// The aircraft's flights, or why not.
pub type Flights = Result<Vec<HistoryRow>, String>;

/// Where the tab's ask goes: the backend, the `--fixture` folder, or a test's.
pub trait HistoryFeeds: Send {
    /// `aircraftHistory(registration)`.
    fn aircraft_history(&mut self, registration: &str) -> Answer<Flights>;
}

/// The backend's ask, on its runtime.
struct BackendFeeds {
    backend: Backend,
    requests: Requests,
}

impl HistoryFeeds for BackendFeeds {
    fn aircraft_history(&mut self, registration: &str) -> Answer<Flights> {
        let backend = self.backend.clone();
        let registration = registration.to_owned();
        self.requests
            .request(async move { backend.aircraft_history(&registration).await.map(|rows| rows.into_iter().map(convert::history_row).collect()) })
            .into()
    }
}

/// The `--fixture` folder's flights ([`FIXTURE_HISTORY`]), read when asked for.
struct FixtureFeeds {
    dir: PathBuf,
}

impl HistoryFeeds for FixtureFeeds {
    fn aircraft_history(&mut self, registration: &str) -> Answer<Flights> {
        let path = self.dir.join(FIXTURE_HISTORY);
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, Vec<HistoryRow>>>(&bytes).map_err(|e| e.to_string()))
            .map_err(|e| format!("fixture {}: {e}", path.display()));
        Answer::ready(read.map(|mut all| all.remove(registration).unwrap_or_default()))
    }
}

/// The feeds for `state`: the fixture folder, else the backend, else none.
fn feeds_for(state: &AppState) -> Option<Box<dyn HistoryFeeds>> {
    if let Some(dir) = &state.startup.fixture {
        return Some(Box::new(FixtureFeeds { dir: dir.clone() }));
    }
    match (&state.backend, &state.requests) {
        (Some(backend), Some(requests)) => Some(Box::new(BackendFeeds { backend: backend.clone(), requests: requests.clone() })),
        _ => None,
    }
}

/// The aircraft's flights as the tab holds them (`history`): the registration asked for, its rows
/// once they came, whether the ask failed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct History {
    pub reg: String,
    pub rows: Option<Vec<HistoryRow>>,
    pub failed: bool,
}

/// The tab's ask and what it brought (see the module documentation).
#[derive(Default)]
pub struct Loads {
    /// Chosen at the first draw ([`feeds_for`]) unless a test installed some.
    feeds: Option<Box<dyn HistoryFeeds>>,
    resolved: bool,
    /// The pass the panel was mounted in ([`super::mounted_pass`]) when this was last drawn.
    mounted: Option<u64>,
    history: Option<History>,
    /// The ask out: the registration it is for, the answer to come.
    ask: Option<(String, Answer<Flights>)>,
}

impl std::fmt::Debug for Loads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loads").field("history", &self.history).field("asking", &self.ask.as_ref().map(|a| &a.0)).finish_non_exhaustive()
    }
}

impl Loads {
    /// The tab drawn for `registration` (empty: none) in the panel mounted in pass `mounted`:
    /// a new mount starts afresh, answers are taken, and the registration asked for when it was
    /// not yet (`$effect`).
    pub fn update(&mut self, state: &AppState, registration: &str, mounted: u64) {
        if self.mounted != Some(mounted) {
            self.mounted = Some(mounted);
            self.history = None;
            self.ask = None;
        }
        if !self.resolved {
            self.feeds = feeds_for(state);
            self.resolved = true;
        }
        self.poll();
        if !registration.is_empty() && self.history.as_ref().is_none_or(|h| h.reg != registration) {
            self.load(registration);
        }
    }

    /// Takes the answer when it has come; one for another registration than the one shown now
    /// is dropped.
    fn poll(&mut self) {
        let Some(result) = self.ask.as_ref().and_then(|(_, answer)| answer.poll()) else { return };
        let Some((reg, _)) = self.ask.take() else { return };
        if self.history.as_ref().is_some_and(|h| h.reg == reg) {
            self.history = Some(match result {
                Ok(rows) => History { reg, rows: Some(rows), failed: false },
                Err(e) => {
                    log::debug!("aircraft history: {e}");
                    History { reg, rows: None, failed: true }
                }
            });
        }
    }

    /// `loadHistory(reg)`: loading, and the ask made (failed at once with nothing to ask).
    pub fn load(&mut self, reg: &str) {
        self.history = Some(History { reg: reg.to_owned(), rows: None, failed: false });
        match self.feeds.as_mut() {
            Some(feeds) => {
                self.ask = Some((reg.to_owned(), feeds.aircraft_history(reg)));
                // a fixture's answer is there at once: shown in this pass
                self.poll();
            }
            None => {
                self.ask = None;
                self.history = Some(History { reg: reg.to_owned(), rows: None, failed: true });
            }
        }
    }

    /// What the tab holds now.
    pub fn history(&self) -> Option<&History> {
        self.history.as_ref()
    }
}

fn loads_id() -> Id {
    Id::unique("rustradar::flight_panel::history::loads")
}

/// The tab's ask as `ctx` keeps it between frames.
pub fn loads(ctx: &Context) -> Arc<Mutex<Loads>> {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(loads_id(), || Arc::new(Mutex::new(Loads::default()))).clone())
}

/// Makes the tab ask `feeds` instead of the fixture folder or the backend (tests).
pub fn install_feeds(ctx: &Context, feeds: Box<dyn HistoryFeeds>) {
    let loads = loads(ctx);
    let mut loads = loads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loads.feeds = Some(feeds);
    loads.resolved = true;
}

// ------------------------------------------------------------------------------------------------
// rows

/// A row of the list as the component reads it (`historyTimes`, `toneOf`, `current`).
#[derive(Debug, Clone, PartialEq)]
pub struct RowView<'a> {
    /// `dayAt(stod, originZone)`, or empty without a scheduled departure.
    pub day: String,
    /// `number || callsign || "—"`.
    pub number: &'a str,
    /// `{origin_iata || "?"} – {destination_iata || "?"}`.
    pub legs: String,
    /// The departure (`atod || etod || stod`) and arrival (`atoa || etoa || stoa`), Unix s, 0
    /// for none, with their airports' zones.
    pub dep: (i64, Option<&'a str>),
    pub arr: (i64, Option<&'a str>),
    /// `This flight`, else the status.
    pub status: &'a str,
    pub tone: Option<Tone>,
    /// The flight the panel shows.
    pub current: bool,
    /// A live flight with an id, not the one shown: a click selects it.
    pub enabled: bool,
}

/// `a || b || …` over optional times: the first not 0.
fn first_time(times: [Option<i64>; 3]) -> i64 {
    times.into_iter().flatten().find(|&t| t != 0).unwrap_or(0)
}

/// `a || b` over optional strings.
fn either<'a>(a: &'a Option<String>, b: &'a Option<String>) -> Option<&'a str> {
    a.as_deref().filter(|s| !s.is_empty()).or_else(|| b.as_deref().filter(|s| !s.is_empty()))
}

/// Row `r` of the list for the flight `shown` (its id), the zones from `reference`, the days
/// against local time `now_ms`.
pub fn row_view<'a>(r: &'a HistoryRow, shown: u32, reference: Option<&'a Reference>, clocks: &Clocks, now_ms: f64) -> RowView<'a> {
    let zone = |code: &Option<String>| reference.and_then(|rf| rf.airport(code.as_deref())).map(|a| a.timezone.as_str());
    let (from, to) = (zone(&r.origin_iata), zone(&r.destination_iata));
    let day = match r.stod {
        Some(stod) if stod != 0 => clocks.day_at(stod as f64, from, now_ms as i64),
        _ => String::new(),
    };
    let current = r.flight_id == Some(shown);
    let iata = |code: &'a Option<String>| code.as_deref().filter(|c| !c.is_empty()).unwrap_or("?");
    RowView {
        day,
        number: either(&r.number, &r.callsign).unwrap_or(DASH),
        legs: format!("{} – {}", iata(&r.origin_iata), iata(&r.destination_iata)),
        dep: (first_time([r.atod, r.etod, r.stod]), from),
        arr: (first_time([r.atoa, r.etoa, r.stoa]), to),
        status: if current { THIS_FLIGHT } else { r.status.as_deref().unwrap_or("") },
        tone: tone_of(r.status_color.as_deref()),
        current,
        enabled: r.live && r.flight_id.is_some_and(|id| id != 0) && !current,
    }
}

/// The colour of a status of `tone`.
pub fn tone_color(tone: Option<Tone>) -> Color32 {
    match tone {
        Some(Tone::Good) => theme::LIVE,
        Some(Tone::Warn) => theme::WARN,
        Some(Tone::Bad) => theme::DANGER,
        None => theme::TEXT_2,
    }
}

/// A row's height: two 12 px lines 2 apart, padded 8.
pub fn row_height() -> f32 {
    2.0 * ROW_PAD + 2.0 * row_style().line_px() + ROW_GAP
}

/// `.history .row`: 12 px `--text-1`.
const fn row_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_1)
}

/// Row `i` of the list; answers whether it was clicked (when it can be).
fn row(ui: &mut Ui, id: Id, i: usize, view: &RowView<'_>, clocks: &Clocks) -> bool {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let style = row_style();
    let line = style.line_px();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, row_height()), Sense::hover());
    let sense = if view.enabled { Sense::click() } else { Sense::hover() };
    let mut response = ui.interact(rect, id.with(("row", i)), sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, view.enabled, format!("{} {} {}", view.day, view.number, view.legs)));
    if view.enabled {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    let fill = if view.current {
        Some(theme::AMBER_SOFT)
    } else if view.enabled && response.hovered() {
        Some(theme::SURFACE_HOVER)
    } else {
        None
    };
    if let Some(fill) = fill {
        painter.rect_filled(panel::snap_rect(rect, ctx.pixels_per_point()), CornerRadius::same(theme::RADIUS), fill);
    }
    let layout = |job: LayoutJob| ctx.fonts_mut(|f| f.layout_job(job));
    let day = layout(text::job(&view.day, &style.color(theme::TEXT_2)));
    let number = layout(text::job(view.number, &panel::data(&style).weight(650)));
    let legs = layout(text::job(&view.legs, &panel::data(&style)));
    let times_style = style.color(theme::TEXT_3);
    let mut times = LayoutJob::default();
    panel::append_clock(&mut times, clocks, view.dep.0, view.dep.1, &times_style);
    text::append(&mut times, " – ", &times_style);
    panel::append_clock(&mut times, clocks, view.arr.0, view.arr.1, &times_style);
    let times = layout(times);
    let status = layout(text::job(view.status, &style.color(tone_color(view.tone))));
    // `74px 1fr auto`: the last column as wide as its widest text
    let (left, right) = (rect.left() + ROW_PAD, rect.right() - ROW_PAD);
    let last = right - legs.size().x.max(status.size().x);
    let first = rect.top() + ROW_PAD;
    let second = first + line + ROW_GAP;
    let put = |x: f32, top: f32, galley: std::sync::Arc<egui::Galley>| {
        painter.galley(Pos2::new(x, top + (line - galley.size().y) / 2.0), galley, theme::TEXT_1);
    };
    put(left, first, day);
    put(left + DAY_COLUMN + COLUMN_GAP, first, number);
    put(last, first, legs);
    put(left, second, times);
    put(right - status.size().x, second, status);
    if response.has_focus() {
        focus_ring(painter, rect, theme::RADIUS);
    }
    response.clicked()
}

/// A wrapping paragraph in `style` across the column (wrapped as CSS wraps it).
fn paragraph(ui: &mut Ui, words: &str, style: &Style) {
    super::route::paragraph(ui, text::job(words, style), style.line_px());
}

/// `.explain`: 13 px `--text-2`.
const fn explain_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The five loading lines.
fn placeholders(ui: &mut Ui) {
    let width = ui.available_width();
    // a row's padding round the 12 px skeleton, and the grid's empty second row 2 px under it
    let height = 2.0 * ROW_PAD + PLACEHOLDER_HEIGHT + ROW_GAP;
    for i in 0..PLACEHOLDERS {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let line = Rect::from_min_size(rect.min + Vec2::splat(ROW_PAD), Vec2::new(placeholder_width(i), PLACEHOLDER_HEIGHT));
        skeleton::paint(ui.painter(), panel::snap_rect(line, ui.ctx().pixels_per_point()), theme::RADIUS);
    }
}

/// The tab's content in `ui` (the tab panel's inner width).
pub fn show(ui: &mut Ui, view: &View<'_>, pass: &mut Pass<'_>) {
    let id = Id::unique("rustradar::flight_panel::history");
    let ctx = ui.ctx().clone();
    let registration = view.registration();
    let loads = loads(&ctx);
    let mut loads = loads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loads.update(pass.state, registration, super::mounted_pass(&ctx));
    if registration.is_empty() {
        paragraph(ui, NO_REGISTRATION, &explain_style());
        return;
    }
    let history = loads.history().cloned().unwrap_or_default();
    if history.failed {
        paragraph(ui, &failed_text(registration), &explain_style());
        ui.add_space(STACK_GAP);
        if Button::new().icon(Icon::RefreshCw.at(14.0, icons::STROKE_THIN)).text(TRY_AGAIN).show(ui).clicked() {
            loads.load(registration);
            ctx.request_repaint();
        }
        return;
    }
    let Some(rows) = &history.rows else {
        placeholders(ui);
        return;
    };
    if rows.is_empty() {
        paragraph(ui, &empty_text(registration), &explain_style());
        return;
    }
    drop(loads);
    let reference = pass.state.aircraft.reference().map(|r| r.as_ref());
    for (i, r) in rows.iter().enumerate() {
        let row_view = row_view(r, view.flight.id, reference, &view.clocks, view.now_ms);
        if row(ui, id, i, &row_view, &view.clocks)
            && let Some(flight_id) = r.flight_id
        {
            pass.actions.push(Action::PickFlight(flight_id));
        }
    }
    paragraph(ui, LIVE_NOTE, &panel::note_style());
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};

    use rustradar_core::model::{Airport, ReferenceData};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    const NOW_MS: f64 = 1_782_036_000_000.0;

    fn reference() -> Reference {
        let airport = |iata: &str, tz: &str| Airport { iata: iata.into(), icao: format!("X{iata}"), timezone: tz.into(), ..Default::default() };
        Reference::new(ReferenceData { airports: vec![airport("ARN", "Europe/Stockholm"), airport("CPH", "Europe/Copenhagen")], ..Default::default() })
    }

    fn history_row() -> HistoryRow {
        HistoryRow {
            flight_id: Some(0x3a8e_0001),
            number: Some("SK1416".into()),
            callsign: Some("SAS1416".into()),
            origin_iata: Some("CPH".into()),
            destination_iata: Some("ARN".into()),
            status: Some("Landed 07:51".into()),
            status_color: Some("green".into()),
            live: true,
            stod: Some(1_782_022_800),
            atod: Some(1_782_023_400),
            stoa: Some(1_782_027_300),
            atoa: Some(1_782_027_060),
            ..Default::default()
        }
    }

    #[test]
    fn a_row_reads_as_today() {
        let rf = reference();
        let clocks = Clocks::default();
        let r = history_row();
        let v = row_view(&r, 0x3a8f_21c4, Some(&rf), &clocks, NOW_MS);
        assert_eq!(v.day, "Today");
        assert_eq!((v.number, v.legs.as_str(), v.status), ("SK1416", "CPH – ARN", "Landed 07:51"));
        assert_eq!(v.dep, (1_782_023_400, Some("Europe/Copenhagen")), "the actual departure, in the origin's zone");
        assert_eq!(v.arr, (1_782_027_060, Some("Europe/Stockholm")));
        assert_eq!(v.tone, Some(Tone::Good));
        assert!(v.enabled && !v.current);
        // the times as the row writes them (Chromium: `08:30 CEST – 09:31 CEST`)
        let mut job = LayoutJob::default();
        let style = row_style().color(theme::TEXT_3);
        panel::append_clock(&mut job, &clocks, v.dep.0, v.dep.1, &style);
        text::append(&mut job, " – ", &style);
        panel::append_clock(&mut job, &clocks, v.arr.0, v.arr.1, &style);
        assert_eq!(job.text, "08:30 CEST – 09:31 CEST");

        // the flight shown: `This flight` in its status's tone, not a button
        let v = row_view(&r, 0x3a8e_0001, Some(&rf), &clocks, NOW_MS);
        assert_eq!((v.status, v.current, v.enabled, v.tone), (THIS_FLIGHT, true, false, Some(Tone::Good)));
        // not live, or no id: not a button
        let mut r2 = history_row();
        r2.live = false;
        assert!(!row_view(&r2, 1, Some(&rf), &clocks, NOW_MS).enabled);
        let mut r3 = history_row();
        r3.flight_id = None;
        assert!(!row_view(&r3, 1, Some(&rf), &clocks, NOW_MS).enabled);

        // nothing known: dashes and question marks, the estimate before the schedule, the
        // viewer's zone for an airport not in the reference data
        let bare = HistoryRow { destination_iata: Some("OSL".into()), status_color: Some("gray".into()), etod: Some(1_782_050_100), stod: Some(1_782_050_000), stoa: Some(0), ..Default::default() };
        let v = row_view(&bare, 1, Some(&rf), &clocks, NOW_MS);
        assert_eq!((v.number, v.legs.as_str(), v.status, v.tone), ("—", "? – OSL", "", None));
        assert_eq!(v.dep, (1_782_050_100, None));
        assert_eq!(v.arr, (0, None));
        let mut job = LayoutJob::default();
        panel::append_clock(&mut job, &clocks, v.dep.0, v.dep.1, &style);
        text::append(&mut job, " – ", &style);
        panel::append_clock(&mut job, &clocks, v.arr.0, v.arr.1, &style);
        assert_eq!(job.text, "13:55 UTC – —");
        // the callsign when there is no number; no day without a schedule
        let r4 = HistoryRow { callsign: Some("SAS9".into()), number: Some(String::new()), ..Default::default() };
        let v = row_view(&r4, 1, None, &clocks, NOW_MS);
        assert_eq!((v.number, v.day.as_str()), ("SAS9", ""));
        assert_eq!(tone_color(tone_of(Some("yellow"))), theme::WARN);
        assert_eq!(tone_color(tone_of(Some("red"))), theme::DANGER);
        assert_eq!(tone_color(None), theme::TEXT_2);
    }

    #[test]
    fn heights_and_placeholders_as_chromium_lays_them_out() {
        // a row 51.59 px (8 + 16.8 + 2 + 16.8 + 8); the loading lines 150, 187, 224, 171, 208 wide
        assert!((row_height() - 51.6).abs() < 1e-4);
        assert_eq!((0..PLACEHOLDERS).map(placeholder_width).collect::<Vec<_>>(), [150.0, 187.0, 224.0, 171.0, 208.0]);
        assert_eq!(failed_text("SE-ROR"), "Couldn't load the flights of SE-ROR.");
        assert_eq!(empty_text("SE-ROR"), "No recent flights recorded for SE-ROR.");
    }

    /// Answers kept to be sent by the test, and the registrations asked for.
    #[derive(Clone, Default)]
    struct Fake {
        asked: Arc<Mutex<Vec<String>>>,
        senders: Arc<Mutex<Vec<Sender<Flights>>>>,
    }

    impl HistoryFeeds for Fake {
        fn aircraft_history(&mut self, registration: &str) -> Answer<Flights> {
            self.asked.lock().unwrap().push(registration.to_owned());
            let (tx, rx) = mpsc::channel();
            self.senders.lock().unwrap().push(tx);
            rx.into()
        }
    }

    fn state() -> AppState {
        AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0])
    }

    #[test]
    fn the_tab_asks_once_per_registration_and_drops_late_answers() {
        let s = state();
        let fake = Fake::default();
        let mut loads = Loads { feeds: Some(Box::new(fake.clone())), resolved: true, ..Default::default() };
        loads.update(&s, "SE-ROR", 7);
        assert_eq!(loads.history(), Some(&History { reg: "SE-ROR".into(), rows: None, failed: false }), "loading");
        loads.update(&s, "SE-ROR", 7);
        assert_eq!(*fake.asked.lock().unwrap(), ["SE-ROR"], "asked once");
        // another aircraft: asked for, and the first answer, late, dropped
        loads.update(&s, "SE-RXA", 7);
        fake.senders.lock().unwrap()[0].send(Ok(vec![history_row()])).ok();
        loads.update(&s, "SE-RXA", 7);
        assert_eq!(loads.history().map(|h| (h.reg.as_str(), h.rows.is_none())), Some(("SE-RXA", true)));
        fake.senders.lock().unwrap()[1].send(Err("503".into())).unwrap();
        loads.update(&s, "SE-RXA", 7);
        assert_eq!(loads.history(), Some(&History { reg: "SE-RXA".into(), rows: None, failed: true }), "failed");
        // Try again: loading, asked again, the rows
        loads.load("SE-RXA");
        assert!(loads.history().is_some_and(|h| !h.failed && h.rows.is_none()));
        fake.senders.lock().unwrap()[2].send(Ok(vec![history_row()])).unwrap();
        loads.update(&s, "SE-RXA", 7);
        assert_eq!(loads.history().and_then(|h| h.rows.as_ref()).map(Vec::len), Some(1));
        // no registration asks nothing and keeps what it has
        loads.update(&s, "", 7);
        assert_eq!(fake.asked.lock().unwrap().len(), 3);
        // the panel mounted again: asked afresh
        loads.update(&s, "SE-RXA", 9);
        assert_eq!(*fake.asked.lock().unwrap(), ["SE-ROR", "SE-RXA", "SE-RXA", "SE-RXA"]);
        assert!(loads.history().is_some_and(|h| h.rows.is_none()));
    }

    #[test]
    fn without_feeds_it_fails_and_a_fixture_answers_at_once() {
        // no backend, no fixture: as `invoke` without Tauri
        let s = state();
        let mut loads = Loads::default();
        loads.update(&s, "SE-ROR", 1);
        assert_eq!(loads.history(), Some(&History { reg: "SE-ROR".into(), rows: None, failed: true }));

        let dir = std::env::temp_dir().join(format!("rustradar-u11-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let all: HashMap<String, Vec<HistoryRow>> = [("SE-ROR".to_owned(), vec![history_row(), history_row()])].into();
        std::fs::write(dir.join(FIXTURE_HISTORY), serde_json::to_vec(&all).unwrap()).unwrap();
        let s = AppState::new(Startup { fixture: Some(dir.clone()), ..Default::default() }, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        let mut loads = Loads::default();
        loads.update(&s, "SE-ROR", 1);
        assert_eq!(loads.history().and_then(|h| h.rows.as_ref()).map(Vec::len), Some(2), "in the pass that asked");
        loads.update(&s, "SE-RXA", 1);
        assert_eq!(loads.history().and_then(|h| h.rows.as_ref()).map(Vec::len), Some(0), "an aircraft the file does not name has none");
        std::fs::remove_file(dir.join(FIXTURE_HISTORY)).unwrap();
        loads.update(&s, "SE-ROR", 2);
        assert!(loads.history().is_some_and(|h| h.failed), "no file: failed");
        std::fs::remove_dir_all(&dir).ok();
    }
}
