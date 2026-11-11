//! TrainPanel (doc 06 §4, §8.1; `TrainPanel.svelte`, `trains.ts`): the selected train in the
//! shell's detail panel slot ([`super::shell::Slots::train_panel`]), built on [`super::panel`].
//!
//! Top to bottom, as today's markup has it:
//! 1. **Strip** (`minmax(0,1fr) auto 40px`, `.bare` without the middle cell while the train
//!    neither moves nor has a direction): the name (22 px at 700, line height 1.1, cut with `…`;
//!    longer than 16 characters 17 px at line height 1.25, at most two rows; tooltip the name)
//!    over the 8 × 8 swatch in the train's map colour (radius 2, 6 px before the text, on the
//!    baseline) and `{kind} train` (12 px `--text-2`); then `{n} km/h` (or `—`) in `--amber` over
//!    the direction of travel (`007°`, Mono), tooltip `Speed and direction of travel`; the close
//!    button `Close train details` ([`Action::CloseTrain`]).
//! 2. **Photo** (when Wikimedia has one for the train's product or operator and it loads): the
//!    2:1 block, a click opening its article (`{title} on Wikipedia`), the credit `Photo:
//!    Wikipedia, {title}`.
//! 3. **Journey** (with a destination or a known delay; padding 16, a rule under it, `minmax(0,
//!    1fr) auto` 12 apart): `Destination` (12 px `--text-3`) over the destination 4 px under it (22
//!    px at 700, line height 1.1, cut; longer than 14 characters 14 px at line height 1.5) or `Not
//!    given` (12 px `--text-2`); on the right `Running` over the delay badge at 12 px
//!    (`delayText` with a capital, in its tone), right-aligned, tooltip `At the last or next stop,
//!    as the operator reports it`.
//! 4. **Stops** (Trafikverket's trains; padding `12px 16px 0`): `Stops` (13 px at 650) and their
//!    count (12 px `--text-3`, 8 px on, on its baseline), 8 px over the rows. The stops are the
//!    live stream's ([`crate::state::trains::Trains::live_stops`]) or else the panel's own ask
//!    ([`Loads`]). Each row (`14px 1fr auto`, 8 apart, padding `5px 0`, 13 px): the line through
//!    the stops (2 px at 6 px in, from the first row's middle to the last's, `--text-3` on the
//!    rows passed, else `--line-control`), the dot (10 px inside a 2 px border, 2 px in: `--text-3`
//!    passed, `--amber` the next stop, else a `--line-control` ring on `--surface`), the name
//!    (wrapping; `--text-3` passed, 650 the next, struck through when cancelled, then the bad
//!    badge `Cancelled`), each note on its own line (12 px `--warn`); on the right in Mono 12 px 8
//!    apart, at the row's top: the planned time struck through in `--text-3` and the time now
//!    expected in `--warn` (late) or `--live` (early) with `{n} min late|early` as its tooltip, or
//!    the time alone; `Tr {track}` in `--text-3` (tooltip `Track`). Without rows: `No timetable for
//!    this train: it isn't announced to travellers.`, `Couldn't load the stops. Trying again in a
//!    minute.` or `Loading stops…` (12 px `--text-2`).
//! 5. **Body** (padding 16, 12 apart): the facts in two columns, `Operator` (the logo on its light
//!    tile, at most 56 × 22 with `2px 4px` of padding, radius 3, 8 px before the name; `Not given`
//!    without one), `Source`, `Position` (the whole row, Mono, where the train is drawn now;
//!    tooltip `Moving along the source's expected positions` or `As last reported`); then the
//!    source's credit as a link (11 px `--text-3`, underlined in `--line` 2 px under the baseline,
//!    `--text-1` under the pointer).
//! 6. **Footer**, pinned to the panel's bottom (padding `12px 16px`, a rule on top): the report's
//!    age (`Position 40 s ago`, 13 px `--text-2`, or past five minutes the warn badge `Position 12
//!    min old` at 12 px; tooltip `Reported at {clock} {zone}`) taking what the buttons leave (an
//!    age wider than that runs under them, as `flex: 1` lets it), then Follow ([`Action::ToggleFollow`],
//!    pressed while the camera follows the train) and `{source}` with `ExternalLink` opening the
//!    source's page (tooltip its address).
//!
//! **The panel's own asks** ([`Loads`], the component's `$effect`s): the stops of a Trafikverket
//! train when it is selected and every minute while the panel is open (an answer replaces the
//! stops; a failure shows only while none are known); the pictures for its product and operator
//! when they change (an answer for an earlier train is dropped). They are asked of the `--fixture`
//! folder ([`FIXTURE_STOPS`]: `{ train id: [TrainStop] }`, a train it does not name having no
//! timetable; no pictures), else of the demo (no timetable, no pictures), else of the backend
//! (`trains::stops`, `train_media::lookup`), else of nothing (the page without Tauri: `Loading
//! stops…`). A panel not drawn in the pass before (closed, or another panel over it) starts again,
//! as the component mounts again.
//!
//! Deviations:
//! - The panel's asks run from the UI pass (as the component's effects do today), not from a part
//!   of `state`: no state file holds them.
//! - The fixture folder's stops serve `--demo` too (`--demo --fixture DIR` shows stops); the demo
//!   alone answers no timetable at once, where today's `#demo` draws no train at all.
//! - Times are in UTC (the app has no viewer's time zone yet, U6's note); digits are Overpass's own
//!   (no `tabular-nums`); text takes egui's baseline (U1).
//! - The photo and the logo load through egui's image loaders ([`super::panel::remote`]): until
//!   item U18's loader for `https:` lands they read as failed and are left out, as today when they
//!   do not load.
//! - The phone order (photo and footer last, the source button hidden) is item U19's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::text::LayoutJob;
use egui::{Color32, Context, CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use rustradar_backend::{train_media, trains as backend_trains};
use rustradar_core::model::{Train, TrainMedia, TrainSource, TrainStop, f32_as_js};
use rustradar_core::num::js_round;
use rustradar_core::readings::degrees;
use rustradar_core::ship_details::lat_lon;
use rustradar_core::time::Clocks;
use rustradar_core::trains::{
    delay_text, delay_tone, heading_at, next_stop, position_at, stop_view, train_color_of, train_credit, train_label, train_position_age,
    train_product, train_source_name,
};

use super::fonts::Face;
use super::icons::{self, Icon};
use super::lists::trains::{capital, color};
use super::panel::{self, Cell, Fact, Fit, Part, Photo, Piece, Remote};
use super::shell::Pass;
use super::text::{self, Style};
use super::widgets::badge::{Badge, Tone};
use super::widgets::button::Button;
use super::widgets::tooltip;
use super::theme;
use crate::requests::Requests;
use crate::state::aircraft::Answer;
use crate::state::{Action, AppState};

/// The panel's accessible name (`aria-label`).
pub const LABEL: &str = "Selected train";
/// The close button's accessible name.
pub const CLOSE_LABEL: &str = "Close train details";
/// The moving cell's tooltip.
pub const MOVING_TOOLTIP: &str = "Speed and direction of travel";
/// The journey's labels and the delay's tooltip.
pub const DESTINATION: &str = "Destination";
pub const RUNNING: &str = "Running";
pub const RUNNING_TOOLTIP: &str = "At the last or next stop, as the operator reports it";
/// What a value not given reads.
pub const NOT_GIVEN: &str = "Not given";
/// The stops' heading and messages.
pub const STOPS: &str = "Stops";
pub const NO_TIMETABLE: &str = "No timetable for this train: it isn't announced to travellers.";
pub const STOPS_FAILED: &str = "Couldn't load the stops. Trying again in a minute.";
pub const STOPS_LOADING: &str = "Loading stops…";
/// A cancelled stop's badge, a track's prefix and tooltip.
pub const CANCELLED: &str = "Cancelled";
pub const TRACK_TOOLTIP: &str = "Track";
/// The position's tooltips.
pub const MOVING_ALONG: &str = "Moving along the source's expected positions";
pub const AS_LAST_REPORTED: &str = "As last reported";
/// The stops are asked again this often while the panel is open (`STOPS_REFRESH_MS`).
pub const STOPS_REFRESH_MS: f64 = 60_000.0;
/// The `--fixture` folder's stops: `{ train id: [TrainStop] }`.
pub const FIXTURE_STOPS: &str = "train_stops.json";

/// A name longer than this (UTF-16 units, `name.length`) is set smaller and may wrap.
pub const LONG_NAME: usize = 16;
/// A destination longer than this is set at 14 px (`.code.free`).
pub const LONG_DESTINATION: usize = 14;
/// The swatch: side, gap to the text.
pub const SWATCH: f32 = 8.0;
pub const SWATCH_GAP: f32 = 6.0;
/// The journey: padding, the columns' gap, a column's line gap.
pub const JOURNEY_PAD: f32 = theme::SPACE_4;
pub const JOURNEY_GAP: f32 = theme::SPACE_3;
pub const JOURNEY_LINE_GAP: f32 = theme::SPACE_1;
/// The stops: padding above and at the sides, the heading's gap to its count and to the rows,
/// how far the count's line box starts below the heading's (their baselines lined up).
pub const STOPS_PAD: Vec2 = Vec2::new(theme::SPACE_4, theme::SPACE_3);
pub const STOPS_HEAD_GAP: f32 = theme::SPACE_2;
pub const COUNT_DROP: f32 = 1.0;
/// A stop's row: padding above and below, the grid's columns and gap.
pub const STOP_PAD: f32 = 5.0;
pub const STOP_DOT_COLUMN: f32 = 14.0;
pub const STOP_GAP: f32 = theme::SPACE_2;
/// The line through the stops: its left and width.
pub const LINE_X: f32 = 6.0;
pub const LINE_WIDTH: f32 = 2.0;
/// The dot: its inside, its border, its margin on the left (a content box: 14 px in all).
pub const DOT: f32 = 10.0;
pub const DOT_BORDER: f32 = 2.0;
pub const DOT_LEFT: f32 = 2.0;
/// Between a stop's name, badge and notes: across and down.
pub const WHERE_GAP: Vec2 = Vec2::new(theme::SPACE_2, 2.0);
/// Between the times and the track.
pub const WHEN_GAP: f32 = theme::SPACE_2;
/// The body's padding and gap.
pub const BODY_PAD: f32 = theme::SPACE_4;
pub const BODY_GAP: f32 = theme::SPACE_3;
/// The operator's logo: its largest size, its tile's padding, the gap to the name.
pub const LOGO_MAX: Vec2 = Vec2::new(56.0, 22.0);
pub const LOGO_PAD: Vec2 = Vec2::new(4.0, 2.0);
pub const LOGO_GAP: f32 = theme::SPACE_2;
/// The credit's underline: under the baseline, thick.
pub const UNDERLINE_OFFSET: f32 = 2.0;
pub const UNDERLINE_WIDTH: f32 = 1.0;

/// `name.length`: UTF-16 code units, as JavaScript counts a string.
pub fn js_length(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The strip's title (`h2`): 22 px at 700, or 17 px at line height 1.25 for a long name.
pub fn title_style(long: bool) -> Style {
    let style = panel::title_style(Face::Ui);
    if long { style.size(theme::TEXT_LG).line_height(1.25) } else { style }
}

/// The destination (`.code`): 22 px at 700, line height 1.1, or 14 px at 1.5 when long (`.free`).
pub fn destination_style(long: bool) -> Style {
    let style = Style::ui(theme::TEXT_XL, 700, theme::TEXT_1).line_height(panel::TITLE_LINE_HEIGHT);
    if long { style.size(theme::TEXT_BASE).line_height(1.5) } else { style }
}

/// A label (`.label`, `dt`): 12 px `--text-3`.
pub const fn label_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A message or a value not given (`.unknown`): 12 px `--text-2`.
pub const fn unknown_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The stops' heading (`h3`: 13 px at 650) and its count (12 px `--text-3`).
pub const fn stops_title_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_BADGE, theme::TEXT_1)
}

pub const fn stops_count_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A stop's name: 13 px, `--text-3` once passed, 650 for the next stop.
pub const fn stop_name_style(passed: bool, next: bool) -> Style {
    Style::ui(theme::TEXT_MD, if next { theme::WEIGHT_BADGE } else { theme::WEIGHT_REGULAR }, if passed { theme::TEXT_3 } else { theme::TEXT_1 })
}

/// A stop's note (`.stop-note`): 12 px `--warn`.
pub const fn stop_note_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::WARN)
}

/// A stop's times (`.when.data`): Mono 12 px in `color`.
pub const fn when_style(color: Color32) -> Style {
    Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, color)
}

/// The footer's age (`.status`): 13 px `--text-2`.
pub const fn status_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The moving cell's speed, `{n} km/h` (`Math.round`), or a dash.
pub fn speed_text(train: &Train) -> String {
    match train.speed_kmh {
        Some(s) => format!("{} km/h", js_round(f32_as_js(s))),
        None => rustradar_core::time::DASH.to_owned(),
    }
}

/// Whether the strip has its moving cell (`speedKmh !== null || heading !== null`).
pub fn moving(train: &Train, now_ms: i64) -> bool {
    train.speed_kmh.is_some() || heading_at(train, now_ms).is_some()
}

/// The journey's delay words (`delayText` with a capital) and tone (`delayTone ?? neutral`).
pub fn delay_badge(train: &Train) -> Option<(String, Tone)> {
    let delay = delay_text(train.delay_minutes);
    (!delay.is_empty()).then(|| (capital(&delay), delay_tone(train.delay_minutes).map_or(Tone::Neutral, Tone::from)))
}

/// Whether the source has the train's timetable (`timetabled`: Trafikverket's).
pub fn timetabled(train: &Train) -> bool {
    train.source == TrainSource::Trafikverket
}

/// The pictures asked for a train: its source, product and operator (`{ source, product,
/// operator }`).
type MediaKey = (TrainSource, Option<String>, Option<String>);

// ------------------------------------------------------------------------------------------------
// the panel's own asks

/// Where the panel's asks go: the backend, the `--fixture` folder, the demo, or a test's.
pub trait PanelFeeds: Send {
    /// `trainStops(id)`.
    fn stops(&mut self, id: &str) -> Answer<Result<Vec<TrainStop>, String>>;
    /// `trainMedia(source, product, operator)`.
    fn media(&mut self, source: TrainSource, product: Option<String>, operator: Option<String>) -> Answer<TrainMedia>;
}

/// The backend's asks, on its runtime.
struct BackendFeeds {
    requests: Requests,
}

impl PanelFeeds for BackendFeeds {
    fn stops(&mut self, id: &str) -> Answer<Result<Vec<TrainStop>, String>> {
        let id = id.to_owned();
        self.requests.request(async move { backend_trains::stops(&id).await }).into()
    }

    fn media(&mut self, source: TrainSource, product: Option<String>, operator: Option<String>) -> Answer<TrainMedia> {
        self.requests.request(train_media::lookup(source.as_str(), product, operator)).into()
    }
}

/// The `--fixture` folder's stops ([`FIXTURE_STOPS`]), read when asked for; no pictures.
struct FixtureFeeds {
    dir: PathBuf,
}

impl PanelFeeds for FixtureFeeds {
    fn stops(&mut self, id: &str) -> Answer<Result<Vec<TrainStop>, String>> {
        let path = self.dir.join(FIXTURE_STOPS);
        let read = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, Vec<TrainStop>>>(&bytes).map_err(|e| e.to_string()))
            .map_err(|e| format!("fixture {}: {e}", path.display()));
        Answer::ready(read.map(|mut stops| stops.remove(id).unwrap_or_default()))
    }

    fn media(&mut self, _source: TrainSource, _product: Option<String>, _operator: Option<String>) -> Answer<TrainMedia> {
        Answer::ready(TrainMedia::default())
    }
}

/// The demo's: no timetable, no pictures.
struct DemoFeeds;

impl PanelFeeds for DemoFeeds {
    fn stops(&mut self, _id: &str) -> Answer<Result<Vec<TrainStop>, String>> {
        Answer::ready(Ok(Vec::new()))
    }

    fn media(&mut self, _source: TrainSource, _product: Option<String>, _operator: Option<String>) -> Answer<TrainMedia> {
        Answer::ready(TrainMedia::default())
    }
}

/// The feeds for `state`: the fixture folder, else the demo, else the backend, else none.
fn feeds_for(state: &AppState) -> Option<Box<dyn PanelFeeds>> {
    if let Some(dir) = &state.startup.fixture {
        return Some(Box::new(FixtureFeeds { dir: dir.clone() }));
    }
    if state.startup.demo {
        return Some(Box::new(DemoFeeds));
    }
    match (&state.backend, &state.requests) {
        (Some(_), Some(requests)) => Some(Box::new(BackendFeeds { requests: requests.clone() })),
        _ => None,
    }
}

/// The panel's own asks and what they brought (see the module documentation).
#[derive(Default)]
pub struct Loads {
    /// Chosen at the first draw ([`feeds_for`]) unless a test installed some.
    feeds: Option<Box<dyn PanelFeeds>>,
    resolved: bool,
    /// The egui pass the panel was last drawn in.
    drawn_pass: Option<u64>,
    /// The train the stops are for (`id`).
    id: Option<String>,
    /// `fetchedStops`, `stopsFailed`.
    stops: Option<Vec<TrainStop>>,
    stops_failed: bool,
    /// The stops' asks out, in the order made.
    stops_asks: Vec<Answer<Result<Vec<TrainStop>, String>>>,
    /// When the stops are asked for next (local Unix ms; the minute's interval).
    stops_due: Option<f64>,
    /// The pictures: what they were asked for, the answer, the ask out.
    media_key: Option<MediaKey>,
    media: Option<TrainMedia>,
    media_ask: Option<Answer<TrainMedia>>,
}

impl std::fmt::Debug for Loads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loads").field("id", &self.id).field("stops", &self.stops.as_ref().map(Vec::len)).field("stops_failed", &self.stops_failed).finish_non_exhaustive()
    }
}

impl Loads {
    /// The asks for `train` in the panel drawn in egui pass `pass_nr` at local time `now_ms`:
    /// answers taken, asks made when due; answers when to come back for the next one.
    pub fn update(&mut self, state: &AppState, train: &Train, now_ms: f64, pass_nr: u64) -> Option<f64> {
        // a panel not drawn in the pass before is mounted anew
        if self.drawn_pass.is_none_or(|p| p + 1 != pass_nr) {
            self.id = None;
            self.media_key = None;
        }
        self.drawn_pass = Some(pass_nr);
        if !self.resolved {
            self.feeds = feeds_for(state);
            self.resolved = true;
        }
        // the stops' effect: on the id alone, so the map's refreshes of the same train ask nothing
        if self.id.as_deref() != Some(train.id.as_str()) {
            self.id = Some(train.id.clone());
            self.stops = None;
            self.stops_failed = false;
            self.stops_asks.clear();
            self.stops_due = (timetabled(train) && self.feeds.is_some()).then_some(now_ms);
        }
        let mut answers = Vec::new();
        self.stops_asks.retain(|ask| match ask.poll() {
            Some(answer) => {
                answers.push(answer);
                false
            }
            None => true,
        });
        for answer in answers {
            match answer {
                Ok(stops) => {
                    self.stops = Some(stops);
                    self.stops_failed = false;
                }
                Err(e) => {
                    log::debug!("train stops: {e}");
                    self.stops_failed = self.stops.is_none();
                }
            }
        }
        if let (Some(due), Some(feeds)) = (self.stops_due, self.feeds.as_mut())
            && now_ms >= due
        {
            self.stops_asks.push(feeds.stops(&train.id));
            self.stops_due = Some(due + STOPS_REFRESH_MS);
        }
        // the pictures' effect: on the source, the product and the operator
        let key = (train.source, train_product(train), train.operator.clone());
        if self.media_key.as_ref() != Some(&key) {
            self.media = None;
            self.media_ask = None;
            if let Some(feeds) = self.feeds.as_mut()
                && (key.1.is_some() || key.2.as_deref().is_some_and(|o| !o.is_empty()))
            {
                self.media_ask = Some(feeds.media(key.0, key.1.clone(), key.2.clone()));
            }
            self.media_key = Some(key);
        }
        if let Some(media) = self.media_ask.as_ref().and_then(Answer::poll) {
            self.media = Some(media);
            self.media_ask = None;
        }
        self.stops_due
    }

    /// The stops fetched (`fetchedStops`).
    pub fn stops(&self) -> Option<&[TrainStop]> {
        self.stops.as_deref()
    }

    /// Whether the stops failed to load and none are known (`stopsFailed`).
    pub fn stops_failed(&self) -> bool {
        self.stops_failed
    }

    /// The pictures found (`media`).
    pub fn media(&self) -> Option<&TrainMedia> {
        self.media.as_ref()
    }
}

fn loads_id() -> Id {
    Id::unique("rustradar::train_panel::loads")
}

/// The panel's asks as `ctx` keeps them between frames.
pub fn loads(ctx: &Context) -> Arc<Mutex<Loads>> {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(loads_id(), || Arc::new(Mutex::new(Loads::default()))).clone())
}

/// Makes the panel ask `feeds` instead of the fixture folder, the demo or the backend (tests).
pub fn install_feeds(ctx: &Context, feeds: Box<dyn PanelFeeds>) {
    let loads = loads(ctx);
    let mut loads = loads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loads.feeds = Some(feeds);
    loads.resolved = true;
}

// ------------------------------------------------------------------------------------------------
// drawing

/// What the panel shows, read from the state and the panel's asks.
struct View<'a> {
    train: &'a Train,
    now: i64,
    following: bool,
    /// `liveStops ?? fetchedStops`.
    stops: Option<Vec<TrainStop>>,
    stops_failed: bool,
    media: Option<TrainMedia>,
    clocks: Clocks,
}

/// The panel in the detail panel slot (the shell's [`Slot`](super::shell::Slot)).
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) {
    let state = pass.state;
    let Some(train) = state.trains.selected() else { return };
    let ctx = ui.ctx().clone();
    let loads = loads(&ctx);
    let (stops, stops_failed, media) = {
        let mut loads = loads.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(due) = loads.update(state, train, pass.now_ms, ctx.cumulative_pass_nr()) {
            ctx.request_repaint_after_secs((((due - pass.now_ms) / 1000.0).max(0.0)) as f32);
        }
        (loads.stops.clone(), loads.stops_failed, loads.media.clone())
    };
    // the ages tick with the 1 s clock
    ctx.request_repaint_after_secs(1.0);
    let view = View {
        train,
        now: pass.now_ms as i64,
        following: state.trains.following(),
        stops: state.trains.live_stops().map(<[TrainStop]>::to_vec).or(stops),
        stops_failed,
        media,
        clocks: Clocks::default(),
    };
    let footer_height = footer_height(&ctx, &view);
    let id = Id::unique("rustradar::train_panel");
    panel::body(ui, id, footer_height, |ui, part| match part {
        Part::Content => content(ui, &view, pass.actions),
        Part::Footer(rect) => footer(ui, rect, &view, pass.actions),
    });
}

/// Everything above the footer.
fn content(ui: &mut Ui, view: &View<'_>, actions: &mut Vec<Action>) {
    strip(ui, view, actions);
    photo(ui, view);
    journey(ui, view);
    if timetabled(view.train) {
        stops(ui, view);
    }
    body(ui, view);
}

/// The strip: the name and kind, the speed and direction, the close button.
fn strip(ui: &mut Ui, view: &View<'_>, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let train = view.train;
    let width = ui.available_width();
    let top = ui.cursor().min;
    // the moving cell first: the name's cell takes what it leaves
    let heading = heading_at(train, view.now);
    let moving_cell = moving(train, view.now).then(|| {
        let value = Piece::text(&speed_text(train), &panel::value_style(theme::AMBER), Fit::Ellipsis);
        let sub = Piece::text(&heading.map(degrees).unwrap_or_default(), &panel::sub_style(Face::Mono), Fit::Ellipsis);
        let w = value.natural_width(&ctx).max(sub.natural_width(&ctx)) + 2.0 * panel::CELL_PAD + theme::BORDER;
        (Cell::auto(vec![value, sub]).tooltip(Some(MOVING_TOOLTIP.to_owned())), w)
    });
    let moving_w = moving_cell.as_ref().map_or(0.0, |(_, w)| *w);
    let id_w = (width - moving_w - panel::CLOSE_COLUMN - panel::CELL_PAD_FIRST - panel::CELL_PAD).max(0.0);
    let long = js_length(&train.name) > LONG_NAME;
    let title_style = title_style(long);
    let fit = if long { Fit::Clamp(2) } else { Fit::Ellipsis };
    let title_rows = ctx.fonts_mut(|f| f.layout_job(panel::fit(text::job(&train.name, &title_style), fit, id_w))).rows.len().max(1);
    let title_h = title_style.line_px() * title_rows as f32;
    let kind = format!("{} train", train_label(train.kind));
    let swatch = color(train_color_of(train));
    let sub_line = panel::sub_style(Face::Ui).line_px();
    let kind_piece = Piece::Custom(
        Vec2::new(id_w, sub_line),
        Box::new(move |painter: &egui::Painter, rect: Rect| {
            let style = panel::sub_style(Face::Ui);
            let galley = painter.layout_job(text::truncated(&kind, &style, (rect.width() - SWATCH - SWATCH_GAP).max(0.0)));
            let top = rect.top() + (rect.height() - galley.size().y) / 2.0;
            let baseline = top + galley.rows.first().and_then(|r| r.glyphs.first().map(|g| r.pos.y + g.pos.y)).unwrap_or(galley.size().y * 0.75);
            // `vertical-align: 0`: the swatch stands on the baseline
            let at = Pos2::new(rect.left(), baseline - SWATCH);
            painter.rect_filled(panel::snap_rect(Rect::from_min_size(at, Vec2::splat(SWATCH)), painter.pixels_per_point()), CornerRadius::same(theme::RADIUS_SWATCH), swatch);
            painter.galley(Pos2::new(rect.left() + SWATCH + SWATCH_GAP, top), galley, Color32::PLACEHOLDER);
        }),
    );
    let id_cell = Cell::fill(vec![Piece::text(&train.name, &title_style, fit), kind_piece]);
    let id_h = title_h + panel::CELL_GAP + sub_line;
    let moving_h = panel::value_style(theme::AMBER).line_px() + panel::CELL_GAP + panel::sub_style(Face::Mono).line_px();
    let mut cells = vec![id_cell];
    let has_moving = moving_cell.is_some();
    if let Some((cell, _)) = moving_cell {
        cells.push(cell);
    }
    let closed = panel::strip(ui, cells, CLOSE_LABEL).clicked();
    // the name's own title
    let content_h = if has_moving { id_h.max(moving_h) } else { id_h };
    let title = Rect::from_min_size(
        Pos2::new(top.x + panel::CELL_PAD_FIRST, top.y + panel::CELL_PAD + (content_h - id_h) / 2.0),
        Vec2::new(id_w, title_h),
    );
    tooltip::show(ui.interact(title, Id::unique("rustradar::train_panel::name"), Sense::hover()), &train.name);
    if closed {
        actions.push(Action::CloseTrain);
    }
}

/// The photo, when Wikimedia has one and it loads.
fn photo(ui: &mut Ui, view: &View<'_>) {
    let Some(photo) = view.media.as_ref().and_then(|m| m.photo.as_ref()) else { return };
    if panel::remote(ui.ctx(), &photo.url) == Remote::Failed {
        return;
    }
    let tip = format!("{} on Wikipedia", photo.title);
    let shown = Photo { uri: &photo.url, credit: Some(format!("Photo: Wikipedia, {}", photo.title)), tooltip: &tip, count: None };
    if panel::photo(ui, Id::unique("rustradar::train_panel::photo"), panel::PHOTO_ASPECT, shown).opened {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&photo.page_url));
    }
}

/// `text` in `style` with its galley centred on the style's line box at `top`, `left`; answers
/// the galley's width.
fn put_line(painter: &egui::Painter, job: LayoutJob, style: &Style, left: f32, top: f32) -> f32 {
    let galley = painter.layout_job(job);
    let w = galley.size().x;
    painter.galley(Pos2::new(left, top + (style.line_px() - galley.size().y) / 2.0), galley, Color32::PLACEHOLDER);
    w
}

/// The journey: the destination, and how late the train runs.
fn journey(ui: &mut Ui, view: &View<'_>) {
    let train = view.train;
    let destination = train.destination.as_deref().filter(|d| !d.is_empty());
    let badge = delay_badge(train);
    if destination.is_none() && badge.is_none() {
        return;
    }
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let inner = width - 2.0 * JOURNEY_PAD;
    let label = label_style();
    // the right column: `Running` over the badge
    let right = badge.map(|(words, tone)| {
        let badge = Badge::new(words, tone).size(theme::TEXT_SM);
        let size = badge.measure(&ctx);
        let label_w = text::measure(&ctx, text::job(RUNNING, &label)).x;
        (badge, size, label_w.max(size.x))
    });
    let right_w = right.as_ref().map_or(0.0, |r| r.2);
    let left_w = if right.is_some() { (inner - right_w - JOURNEY_GAP).max(0.0) } else { inner };
    let value_style = match destination {
        Some(d) => destination_style(js_length(d) > LONG_DESTINATION),
        None => unknown_style(),
    };
    let left_h = label.line_px() + JOURNEY_LINE_GAP + value_style.line_px();
    let right_h = right.as_ref().map_or(0.0, |r| label.line_px() + JOURNEY_LINE_GAP + r.1.y);
    let h = left_h.max(right_h);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 2.0 * JOURNEY_PAD + h + theme::BORDER), Sense::hover());
    let painter = ui.painter().clone();
    let (x, y) = (rect.left() + JOURNEY_PAD, rect.top() + JOURNEY_PAD);
    put_line(&painter, text::job(DESTINATION, &label), &label, x, y);
    let value_top = y + label.line_px() + JOURNEY_LINE_GAP;
    match destination {
        Some(d) => {
            let w = put_line(&painter, text::truncated(d, &value_style, left_w), &value_style, x, value_top);
            let r = Rect::from_min_size(Pos2::new(x, value_top), Vec2::new(w, value_style.line_px()));
            tooltip::show(ui.interact(r, Id::unique("rustradar::train_panel::destination"), Sense::hover()), d);
        }
        None => {
            put_line(&painter, text::job(NOT_GIVEN, &value_style), &value_style, x, value_top);
        }
    }
    if let Some((badge, size, column)) = right {
        let right_edge = rect.right() - JOURNEY_PAD;
        let label_w = text::measure(&ctx, text::job(RUNNING, &label)).x;
        put_line(&painter, text::job(RUNNING, &label), &label, right_edge - label_w, y);
        badge.paint(&painter, Pos2::new(right_edge - size.x, value_top));
        let column = Rect::from_min_size(Pos2::new(right_edge - column, y), Vec2::new(column, h));
        tooltip::show(ui.interact(column, Id::unique("rustradar::train_panel::running"), Sense::hover()), RUNNING_TOOLTIP);
    }
    panel::hrule(&painter, rect.x_range(), rect.bottom() - theme::BORDER);
}

/// A stop's row as laid out across `width`.
struct StopRow {
    name: std::sync::Arc<egui::Galley>,
    /// The badge's size, and whether it shares the name's line.
    badge: Option<(Vec2, bool)>,
    notes: Vec<std::sync::Arc<egui::Galley>>,
    when: Vec<(std::sync::Arc<egui::Galley>, Option<String>)>,
    when_w: f32,
    /// The name's line (its rows, or the badge's height when it is taller and beside it).
    first_line: f32,
    /// The content's height (the row less its padding).
    height: f32,
    passed: bool,
    next: bool,
}

/// `job` with every section struck through in its own colour (`text-decoration: line-through`).
fn struck(mut job: LayoutJob) -> LayoutJob {
    for section in &mut job.sections {
        section.format.strikethrough = Stroke::new(1.0, section.format.color);
    }
    job
}

/// Stop `stop` (the next one when `next`) laid out across `width`.
fn stop_row(ctx: &Context, clocks: &Clocks, stop: &TrainStop, next: bool, width: f32) -> StopRow {
    let view = stop_view(stop);
    let passed = view.passed;
    // the times on the right
    let base = if passed { theme::TEXT_3 } else { theme::TEXT_1 };
    let mut when: Vec<(LayoutJob, Option<String>)> = Vec::new();
    match view.late.filter(|l| *l != 0) {
        Some(late) => {
            when.push((struck(text::job(&clocks.clock_at(view.planned, None), &when_style(theme::TEXT_3))), None));
            let (color, word) = if late > 0 { (theme::WARN, "late") } else { (theme::LIVE, "early") };
            when.push((text::job(&clocks.clock_at(view.time, None), &when_style(color)), Some(format!("{} min {word}", late.abs()))));
        }
        None => when.push((text::job(&clocks.clock_at(view.time, None), &when_style(base)), None)),
    }
    if let Some(track) = stop.track.as_deref().filter(|t| !t.is_empty()) {
        when.push((text::job(&format!("Tr {track}"), &when_style(theme::TEXT_3)), Some(TRACK_TOOLTIP.to_owned())));
    }
    let when: Vec<_> = when.into_iter().map(|(job, tip)| (ctx.fonts_mut(|f| f.layout_job(job)), tip)).collect();
    let when_w = when.iter().map(|(g, _)| g.size().x).sum::<f32>() + WHEN_GAP * when.len().saturating_sub(1) as f32;
    let where_w = (width - STOP_DOT_COLUMN - 2.0 * STOP_GAP - when_w).max(0.0);
    // the name, the badge, the notes
    let name_style = stop_name_style(passed, next);
    let mut job = text::wrapped(&stop.name, &name_style, where_w);
    if stop.canceled {
        job = struck(job);
    }
    let name = ctx.fonts_mut(|f| f.layout_job(job));
    let name_rows = name.rows.len().max(1) as f32;
    let name_h = name_rows * name_style.line_px();
    let badge = stop.canceled.then(|| {
        let size = Badge::new(CANCELLED, Tone::Bad).measure(ctx);
        let natural = text::measure(ctx, text::job(&stop.name, &name_style)).x;
        (size, natural + WHERE_GAP.x + size.x <= where_w)
    });
    let first_line = match badge {
        Some((size, true)) => name_h.max(size.y),
        _ => name_h,
    };
    let mut height = first_line;
    if let Some((size, false)) = badge {
        height += WHERE_GAP.y + size.y;
    }
    let note_style = stop_note_style();
    let notes: Vec<_> = stop
        .notes
        .iter()
        .map(|n| {
            let g = ctx.fonts_mut(|f| f.layout_job(text::wrapped(n, &note_style, where_w)));
            height += WHERE_GAP.y + g.rows.len().max(1) as f32 * note_style.line_px();
            g
        })
        .collect();
    let height = height.max(when_style(base).line_px()).max(DOT + 2.0 * DOT_BORDER);
    StopRow { name, badge, notes, when, when_w, first_line, height, passed, next }
}

/// The stops: the heading, the rows or why there are none.
fn stops(ui: &mut Ui, view: &View<'_>) {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let inner = width - 2.0 * STOPS_PAD.x;
    let title = stops_title_style();
    let list = view.stops.as_deref();
    let message = match list {
        Some([]) => Some(NO_TIMETABLE),
        Some(_) => None,
        None if view.stops_failed => Some(STOPS_FAILED),
        None => Some(STOPS_LOADING),
    };
    let next = list.and_then(next_stop);
    let rows: Vec<StopRow> = list.unwrap_or_default().iter().enumerate().map(|(i, s)| stop_row(&ctx, &view.clocks, s, next == Some(i), inner)).collect();
    let message_job = message.map(|m| text::wrapped(m, &unknown_style(), inner));
    let message_h = message_job.as_ref().map_or(0.0, |job| ctx.fonts_mut(|f| f.layout_job(job.clone())).rows.len().max(1) as f32 * unknown_style().line_px());
    let rows_h: f32 = rows.iter().map(|r| r.height + 2.0 * STOP_PAD).sum();
    let height = STOPS_PAD.y + title.line_px() + STOPS_HEAD_GAP + rows_h + message_h;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter().clone();
    let x = rect.left() + STOPS_PAD.x;
    let mut y = rect.top() + STOPS_PAD.y;
    // the heading and the count on its baseline
    let title_w = put_line(&painter, text::job(STOPS, &title), &title, x, y);
    if let Some(list) = list.filter(|l| !l.is_empty()) {
        let count = stops_count_style();
        put_line(&painter, text::job(&list.len().to_string(), &count), &count, x + title_w + STOPS_HEAD_GAP, y + COUNT_DROP);
    }
    y += title.line_px() + STOPS_HEAD_GAP;
    if let Some(job) = message_job {
        let galley = painter.layout_job(job);
        let rows = galley.rows.len().max(1) as f32;
        painter.galley(Pos2::new(x, y + (rows * unknown_style().line_px() - galley.size().y) / 2.0), galley, Color32::PLACEHOLDER);
        return;
    }
    let last = rows.len().saturating_sub(1);
    for (i, row) in rows.into_iter().enumerate() {
        let h = row.height + 2.0 * STOP_PAD;
        let li = Rect::from_min_size(Pos2::new(x, y), Vec2::new(inner, h));
        paint_stop(ui, &painter, li, row, i == 0, i == last, i);
        y += h;
    }
}

/// Stop row `row` in `li` (`first`, `last` of the list).
fn paint_stop(ui: &mut Ui, painter: &egui::Painter, li: Rect, row: StopRow, first: bool, last: bool, i: usize) {
    let ppp = painter.pixels_per_point();
    let middle = li.center().y;
    // the line through the stops, this row's piece of it
    let line_color = if row.passed { theme::TEXT_3 } else { theme::LINE_CONTROL };
    let line = Rect::from_x_y_ranges(li.left() + LINE_X..=li.left() + LINE_X + LINE_WIDTH, if first { middle } else { li.top() }..=if last { middle } else { li.bottom() });
    painter.rect_filled(panel::snap_rect(line, ppp), CornerRadius::ZERO, line_color);
    // the dot over it
    let (ring, fill) = if row.next {
        (theme::AMBER, theme::AMBER)
    } else if row.passed {
        (theme::TEXT_3, theme::TEXT_3)
    } else {
        (theme::LINE_CONTROL, theme::SURFACE)
    };
    let centre = Pos2::new(li.left() + DOT_LEFT + DOT / 2.0 + DOT_BORDER, middle);
    painter.circle_filled(centre, DOT / 2.0 + DOT_BORDER, ring);
    painter.circle_filled(centre, DOT / 2.0, fill);
    // the name, the badge beside it or under it, the notes
    let top = li.top() + STOP_PAD;
    let x = li.left() + STOP_DOT_COLUMN + STOP_GAP;
    let name_style = stop_name_style(row.passed, row.next);
    let name_h = row.name.rows.len().max(1) as f32 * name_style.line_px();
    let name_w = row.name.size().x;
    let name_top = top + (row.first_line - name_h) / 2.0;
    painter.galley(Pos2::new(x, name_top + (name_h - row.name.size().y) / 2.0), row.name.clone(), Color32::PLACEHOLDER);
    let mut y = top + row.first_line;
    if let Some((size, beside)) = row.badge {
        let at = if beside {
            Pos2::new(x + name_w + WHERE_GAP.x, top + (row.first_line - size.y) / 2.0)
        } else {
            let at = Pos2::new(x, y + WHERE_GAP.y);
            y += WHERE_GAP.y + size.y;
            at
        };
        Badge::new(CANCELLED, Tone::Bad).paint(painter, at);
    }
    let note = stop_note_style();
    for galley in row.notes {
        y += WHERE_GAP.y;
        let rows = galley.rows.len().max(1) as f32;
        painter.galley(Pos2::new(x, y + (rows * note.line_px() - galley.size().y) / 2.0), galley.clone(), Color32::PLACEHOLDER);
        y += rows * note.line_px();
    }
    // the times and the track, at the row's top, flush right
    let line = when_style(theme::TEXT_1).line_px();
    let mut wx = li.right() - row.when_w;
    for (n, (galley, tip)) in row.when.into_iter().enumerate() {
        let w = galley.size().x;
        painter.galley(Pos2::new(wx, top + (line - galley.size().y) / 2.0), galley, Color32::PLACEHOLDER);
        if let Some(tip) = tip {
            let r = Rect::from_min_size(Pos2::new(wx, top), Vec2::new(w, line));
            tooltip::show(ui.interact(r, Id::unique(("rustradar::train_panel::when", i, n)), Sense::hover()), &tip);
        }
        wx += w + WHEN_GAP;
    }
}

/// The body: the facts and the source's credit.
fn body(ui: &mut Ui, view: &View<'_>) {
    let ctx = ui.ctx().clone();
    let train = view.train;
    let width = ui.available_width();
    ui.add_space(BODY_PAD);
    let inner = width - 2.0 * BODY_PAD;
    let logo = view.media.as_ref().and_then(|m| m.logo.clone()).and_then(|uri| match panel::remote(&ctx, &uri) {
        Remote::Ready(texture) => Some(texture),
        _ => None,
    });
    // `train.operator ?? "Not given"`: an empty name stays empty
    let operator = train.operator.clone().unwrap_or_else(|| NOT_GIVEN.to_owned());
    let dd = panel::dd_style(Face::Ui, false);
    let operator_piece = match logo {
        Some(texture) => {
            let image = panel::contain(texture.size, LOGO_MAX);
            let tile = image + 2.0 * LOGO_PAD;
            let height = tile.y.max(dd.line_px());
            let col_w = (inner - panel::FACTS_GAP) / 2.0;
            Piece::Custom(
                Vec2::new(col_w, height),
                Box::new(move |painter: &egui::Painter, rect: Rect| {
                    let tile_rect = Rect::from_min_size(Pos2::new(rect.left(), rect.center().y - tile.y / 2.0), tile);
                    painter.rect_filled(tile_rect, CornerRadius::same(theme::RADIUS_LOGO_TILE), theme::LOGO_TILE);
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                    painter.image(texture.id, Rect::from_min_size(tile_rect.min + LOGO_PAD, image), uv, Color32::WHITE);
                    let x = tile_rect.right() + LOGO_GAP;
                    let galley = painter.layout_job(text::wrapped(&operator, &panel::dd_style(Face::Ui, false), (rect.right() - x).max(0.0)));
                    painter.galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
                }),
            )
        }
        None => Piece::text(&operator, &dd, Fit::Wrap),
    };
    let position = position_at(train, view.now);
    let position_tip = if train.frames.len() > 1 { MOVING_ALONG } else { AS_LAST_REPORTED };
    let facts = vec![
        Fact::new("Operator", operator_piece),
        Fact::new("Source", Piece::text(train_source_name(train.source), &dd, Fit::Wrap)),
        Fact::new("Position", Piece::text(&lat_lon(position.lat, position.lon), &panel::dd_style(Face::Mono, false), Fit::Wrap)).wide(true).tooltip(Some(position_tip)),
    ];
    ui.horizontal(|ui| {
        ui.add_space(BODY_PAD);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.set_width(inner);
            panel::facts(ui, 2, panel::FACTS_GAP, facts);
            ui.add_space(BODY_GAP);
            credit(ui, train.source);
        });
    });
    ui.add_space(BODY_PAD);
}

/// The source's credit, a link to its page.
fn credit(ui: &mut Ui, source: TrainSource) {
    let credit = train_credit(source);
    let style = panel::note_style();
    let id = Id::unique("rustradar::train_panel::credit");
    let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
    let job = text::wrapped(credit.text, &style.color(if hovered { theme::TEXT_1 } else { theme::TEXT_3 }), ui.available_width());
    let galley = ui.painter().layout_job(job);
    let rows = galley.rows.len().max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), rows * style.line_px()), Sense::hover());
    let at = Pos2::new(rect.left(), rect.top() + (rect.height() - galley.size().y) / 2.0);
    let mut link = Rect::NOTHING;
    for row in &galley.rows {
        let (Some(first), Some(last)) = (row.glyphs.first(), row.glyphs.last()) else { continue };
        let left = at.x + row.pos.x + first.pos.x;
        let right = at.x + row.pos.x + last.pos.x + last.advance_width;
        let baseline = at.y + row.pos.y + first.pos.y;
        let top = (baseline + UNDERLINE_OFFSET).round();
        ui.painter().rect_filled(Rect::from_x_y_ranges(left..=right, top..=top + UNDERLINE_WIDTH), CornerRadius::ZERO, theme::LINE);
        link = link.union(Rect::from_x_y_ranges(left..=right, at.y + row.pos.y..=at.y + row.pos.y + row.size.y));
    }
    ui.painter().galley(at, galley, Color32::PLACEHOLDER);
    let response = ui.interact(link, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if response.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(credit.url));
    }
}

/// The footer's status: the report's age.
enum Status {
    /// Recent: plain text.
    Text(String),
    /// Stale: a warn badge at 12 px.
    Badge(Badge),
}

/// The footer's status and its tooltip (`Reported at {clock} {zone}`).
fn status(view: &View<'_>) -> (Status, String) {
    let age = train_position_age(view.train.position_unix, view.now);
    let tip = format!("Reported at {}", view.clocks.clock_with_zone(view.train.position_unix, None));
    if age.stale { (Status::Badge(Badge::new(age.label, Tone::Warn).size(theme::TEXT_SM)), tip) } else { (Status::Text(age.label), tip) }
}

/// The footer's buttons: Follow, and the source's page.
fn buttons(view: &View<'_>) -> Vec<Button> {
    let credit = train_credit(view.train.source);
    let source = Button::new().text(train_source_name(view.train.source)).icon(Icon::ExternalLink.at(14.0, icons::STROKE_THIN)).tooltip(credit.url);
    vec![panel::follow_button(view.following, false), source]
}

/// The footer's height: its rule, padding and one row (the status never wraps: `flex: 1`).
fn footer_height(ctx: &Context, view: &View<'_>) -> f32 {
    let status_h = match status(view).0 {
        Status::Text(_) => status_style().line_px(),
        Status::Badge(badge) => badge.measure(ctx).y,
    };
    let tallest = buttons(view).iter().map(|b| b.size(ctx).y).fold(status_h, f32::max);
    theme::BORDER + 2.0 * panel::FOOTER_PAD.y + tallest
}

/// The footer in `rect` (from [`panel::body`]).
fn footer(ui: &mut Ui, rect: Rect, view: &View<'_>, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let painter = ui.painter().clone();
    painter.rect_filled(panel::snap_rect(rect, ppp), CornerRadius::ZERO, theme::SURFACE);
    panel::hrule(&painter, rect.x_range(), rect.top());
    let row = Rect::from_min_max(
        Pos2::new(rect.left() + panel::FOOTER_PAD.x, rect.top() + theme::BORDER + panel::FOOTER_PAD.y),
        Pos2::new(rect.right() - panel::FOOTER_PAD.x, rect.bottom() - panel::FOOTER_PAD.y),
    );
    let mid = row.center().y;
    // the status first: an age wider than its room runs under the buttons
    let (status, tip) = status(view);
    let status_rect = match status {
        Status::Text(label) => {
            let style = status_style();
            let top = mid - style.line_px() / 2.0;
            let w = put_line(&painter, text::job(&label, &style), &style, row.left(), top);
            Rect::from_min_size(Pos2::new(row.left(), top), Vec2::new(w, style.line_px()))
        }
        Status::Badge(badge) => {
            let size = badge.measure(&ctx);
            badge.paint(&painter, Pos2::new(row.left(), mid - size.y / 2.0))
        }
    };
    tooltip::show(ui.interact(status_rect, Id::unique("rustradar::train_panel::status"), Sense::hover()), &tip);
    let buttons = buttons(view);
    let sizes: Vec<Vec2> = buttons.iter().map(|b| b.size(&ctx)).collect();
    let total = sizes.iter().map(|s| s.x).sum::<f32>() + panel::FOOTER_GAP * sizes.len().saturating_sub(1) as f32;
    let mut x = row.right() - total;
    let credit = train_credit(view.train.source);
    for (n, (button, size)) in buttons.into_iter().zip(sizes).enumerate() {
        let r = panel::snap_rect(Rect::from_min_size(Pos2::new(x, mid - size.y / 2.0), size), ppp);
        if panel::put(ui, ("rustradar::train_panel::button", n), r, button).clicked() {
            match n {
                0 => actions.push(Action::ToggleFollow),
                _ => ctx.open_url(egui::OpenUrl::new_tab(credit.url)),
            }
        }
        x += size.x + panel::FOOTER_GAP;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Sender};

    use egui::{Event, PointerButton, RawInput};
    use rustradar_core::settings::Mode;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::dev::demo_trains;
    use crate::persist::tests::TempDir;
    use crate::state::Startup;
    use crate::state::trains::FIXTURE_TRAINS;
    use crate::ui::shell::{self, Panel, Slots};

    const T0: f64 = 1_782_036_000_000.0;
    const V3: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };

    /// Feeds that log what is asked and answer the stops when the test sends.
    #[derive(Clone, Default)]
    struct Fake {
        asked: Arc<Mutex<Vec<String>>>,
        stops: Arc<Mutex<Vec<Sender<Result<Vec<TrainStop>, String>>>>>,
    }

    impl Fake {
        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }

        /// Answers the latest stops ask.
        fn answer(&self, answer: Result<Vec<TrainStop>, String>) {
            self.stops.lock().unwrap().pop().expect("an ask out").send(answer).unwrap();
        }
    }

    impl PanelFeeds for Fake {
        fn stops(&mut self, id: &str) -> Answer<Result<Vec<TrainStop>, String>> {
            self.asked.lock().unwrap().push(format!("stops {id}"));
            let (tx, rx) = mpsc::channel();
            self.stops.lock().unwrap().push(tx);
            Answer::Channel(rx)
        }

        fn media(&mut self, source: TrainSource, product: Option<String>, operator: Option<String>) -> Answer<TrainMedia> {
            self.asked.lock().unwrap().push(format!("media {} {product:?} {operator:?}", source.as_str()));
            Answer::ready(TrainMedia { photo: None, logo: Some("https://example.invalid/logo.png".into()) })
        }
    }

    fn stop(name: &str) -> TrainStop {
        TrainStop { name: name.into(), ..Default::default() }
    }

    #[test]
    fn words_and_styles() {
        assert_eq!(js_length("Demo SJ 101"), 11);
        assert_eq!(js_length("Arlanda Express 7041"), 20);
        assert_eq!(js_length("Öresundståg 1234"), 16, "one unit each");
        let mut t = demo_trains(59.33, 18.07, T0 as i64).remove(1);
        assert_eq!(speed_text(&t), "79 km/h");
        assert!(moving(&t, T0 as i64));
        t.speed_kmh = None;
        assert_eq!(speed_text(&t), "—");
        assert!(moving(&t, T0 as i64), "a direction is enough");
        t.heading = None;
        t.frames.clear();
        assert!(!moving(&t, T0 as i64), "bare");
        assert!(timetabled(&t));
        assert_eq!(delay_badge(&t), Some(("On time".to_owned(), Tone::Good)));
        assert_eq!((title_style(false).size, title_style(false).line_px()), (22.0, 22.0 * 1.1));
        assert_eq!((title_style(true).size, title_style(true).line_px()), (17.0, 17.0 * 1.25));
        assert_eq!(destination_style(true).line_px(), 21.0);
    }

    #[test]
    fn the_stops_every_minute_and_the_pictures_once() {
        let state = AppState::new(Startup::default(), None, None, None, V3, [1440.0, 900.0]);
        let fake = Fake::default();
        let mut loads = Loads { feeds: Some(Box::new(fake.clone())), resolved: true, ..Default::default() };
        let mut trains = demo_trains(59.33, 18.07, T0 as i64);
        let sj = trains.remove(1);
        // opened: the stops and the pictures asked at once
        assert_eq!(loads.update(&state, &sj, T0, 1), Some(T0 + STOPS_REFRESH_MS));
        assert_eq!(fake.asked(), ["stops trafikverket:demo:1", "media trafikverket Some(\"Demo SJ\") Some(\"SJ\")"]);
        assert_eq!((loads.stops(), loads.stops_failed()), (None, false), "loading");
        // the logo came: kept; a failure with no stops known says so
        loads.update(&state, &sj, T0 + 16.0, 2);
        assert_eq!(loads.media().and_then(|m| m.logo.as_deref()), Some("https://example.invalid/logo.png"));
        fake.answer(Err("unreachable".into()));
        loads.update(&state, &sj, T0 + 32.0, 3);
        assert_eq!((loads.stops(), loads.stops_failed()), (None, true));
        // the same train again (a refresh of the map) asks nothing until the minute is up
        loads.update(&state, &sj, T0 + 59_000.0, 4);
        assert_eq!(fake.asked().len(), 2);
        loads.update(&state, &sj, T0 + 60_000.0, 5);
        assert_eq!(fake.asked().len(), 3);
        fake.answer(Ok(vec![stop("Stockholm Central")]));
        loads.update(&state, &sj, T0 + 60_016.0, 6);
        assert_eq!((loads.stops().map(<[TrainStop]>::len), loads.stops_failed()), (Some(1), false));
        // a later failure keeps the stops on show
        loads.update(&state, &sj, T0 + 120_000.0, 7);
        fake.answer(Err("unreachable".into()));
        loads.update(&state, &sj, T0 + 120_016.0, 8);
        assert_eq!((loads.stops().map(<[TrainStop]>::len), loads.stops_failed()), (Some(1), false));
        // another train: its own stops, at once; another product and operator: new pictures
        let other = trains.remove(0);
        loads.update(&state, &other, T0 + 130_000.0, 9);
        assert_eq!(loads.stops(), None);
        assert_eq!(fake.asked()[4..], ["stops trafikverket:demo:0".to_owned(), "media trafikverket Some(\"Demo Skånetrafiken\") Some(\"Skånetrafiken\")".to_owned()]);
        // closed and opened again (a pass without the panel): asked anew
        loads.update(&state, &other, T0 + 140_000.0, 11);
        assert_eq!(fake.asked().len(), 8);
        // no product and no operator: no pictures asked
        let mut plain = other.clone();
        plain.id = "trafikverket:2026-06-21:1".into();
        plain.name = "Train 1".into();
        plain.operator = None;
        loads.update(&state, &plain, T0 + 150_000.0, 12);
        assert_eq!(fake.asked().last().map(String::as_str), Some("stops trafikverket:2026-06-21:1"));
        assert_eq!(loads.media(), None);
    }

    #[test]
    fn the_fixture_folder_s_stops() {
        let dir = TempDir::new("train-stops");
        let mut feeds = FixtureFeeds { dir: dir.0.clone() };
        let answer = |a: Answer<Result<Vec<TrainStop>, String>>| a.poll().expect("answered at once");
        assert!(answer(feeds.stops("trafikverket:demo:1")).is_err(), "no file: a failure");
        let stops = HashMap::from([("trafikverket:demo:1".to_owned(), vec![stop("Uppsala C")])]);
        std::fs::write(dir.0.join(FIXTURE_STOPS), serde_json::to_vec(&stops).unwrap()).unwrap();
        assert_eq!(answer(feeds.stops("trafikverket:demo:1")).unwrap(), [stop("Uppsala C")]);
        assert_eq!(answer(feeds.stops("trafikverket:demo:2")).unwrap(), [], "a train it does not name has no timetable");
        assert_eq!(feeds.media(TrainSource::Trafikverket, Some("SJ".into()), None).poll(), Some(TrainMedia::default()));
        // the demo: no timetable at once
        assert_eq!(answer(DemoFeeds.stops("trafikverket:demo:1")).unwrap(), []);
    }

    /// One pass of the shell over `events`: the actions.
    fn pass(ctx: &Context, state: &AppState, time: f64, events: Vec<Event>) -> Vec<Action> {
        let mut actions = Vec::new();
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))), time: Some(time), events, ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| {
            shell::show(ui.ctx(), state, T0, &mut actions, &Slots::APP);
        });
        output.textures_delta.clear();
        actions
    }

    fn click(ctx: &Context, state: &AppState, at: Pos2, time: f64) -> Vec<Action> {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
        let mut actions = pass(ctx, state, time, vec![Event::PointerMoved(at)]);
        actions.extend(pass(ctx, state, time + 0.05, vec![button(true)]));
        actions.extend(pass(ctx, state, time + 0.1, vec![button(false)]));
        actions
    }

    #[test]
    fn close_and_follow() {
        let dir = TempDir::new("train-panel");
        std::fs::write(dir.0.join(FIXTURE_TRAINS), serde_json::to_vec(&demo_trains(59.33, 18.07, T0 as i64)).unwrap()).unwrap();
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, V3, [1440.0, 900.0]);
        s.persisted.settings.mode = Mode::Trains;
        for t in [T0, T0 + 16.0, T0 + 32.0] {
            s.poll(t);
        }
        let i = s.trains.trains().iter().position(|t| t.id == "trafikverket:demo:1").unwrap();
        assert!(s.apply(Action::PickTrain(i), T0 + 40.0));
        let ctx = Context::default();
        theme::apply(&ctx);
        for time in [0.0, 1.0, 2.0, 3.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let panel = ctx.memory(|m| m.area_rect(Id::unique(("rustradar::shell::panel", Panel::Train)))).expect("the panel");
        // Chromium: 362 wide, the strip 67.98 high, the footer 57 at the bottom
        assert!((panel.width() - 362.0).abs() < 0.5, "{panel:?}");
        // the close button: centred in the strip's last 40 px
        let close = Pos2::new(panel.right() - 1.0 - 20.0, panel.top() + 1.0 + 33.5);
        assert_eq!(click(&ctx, &s, close, 4.0), [Action::CloseTrain]);
        // Follow: the footer's first button, 16 in from the right past the source's button
        let follow = Pos2::new(panel.right() - 1.0 - 16.0 - 119.67 - 8.0 - 40.0, panel.bottom() - 1.0 - 28.0);
        assert_eq!(click(&ctx, &s, follow, 5.0), [Action::ToggleFollow]);
        assert!(s.apply(Action::ToggleFollow, T0 + 50.0));
        assert!(s.trains.following());
        assert!(s.apply(Action::CloseTrain, T0 + 60.0));
        assert_eq!(s.trains.selected_id(), None);
        assert!(!s.apply(Action::CloseTrain, T0 + 70.0), "nothing left to close");
    }
}
