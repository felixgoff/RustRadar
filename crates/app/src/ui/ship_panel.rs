//! ShipPanel (doc 06 §4, §8.2; `ShipPanel.svelte`, `ship-details.ts`): the selected ship in the
//! shell's detail panel slot ([`super::shell::Slots::ship_panel`]), built on [`super::panel`].
//!
//! What it shows is read from the state each frame ([`View`]): the ship as the map last showed it
//! ([`Ships::selected`](crate::state::ships::Ships::selected)), VesselFinder's card for it
//! ([`Ships::card_details`](crate::state::ships::Ships::card_details), `None` while it loads or
//! after a failure) and why it failed ([`Ships::card_error`](crate::state::ships::Ships::card_error)).
//! It is loading while it has neither.
//!
//! Top to bottom, as today's markup has it:
//! 1. **Strip** (`minmax(0,1fr) auto 40px`; `.bare`, without the middle cell, while the ship
//!    neither moves nor loads): the name ([`View::name`]: the card's, else the feed's unless it is
//!    the MMSI, else `Unnamed ship`; Overpass Mono 22 px at 700, line height 1.1, cut with `…`;
//!    longer than 13 characters 17 px at line height 1.3; tooltip the name) over the subtitle
//!    ([`View::subtitle`]: VesselFinder's type or the map's colour class, and the flag, joined by
//!    ` · `; 12 px `--text-2`, cut, tooltip itself) or while loading a 140 × 12 skeleton; then
//!    when a speed or course is known `{n.n} kn` (or `—`) in `--amber` over the course (`271°`,
//!    Mono), tooltip `Speed and course over ground`, or while loading two skeletons (56 × 14, 36 ×
//!    12); the close button `Close ship details` ([`Action::CloseShip`]).
//! 2. **Photo**: while loading the 2:1 block alone; else VesselFinder's picture when the card has
//!    one and it does not fail (`onerror` takes the block away), a click opening the ship's page
//!    (`View this ship on VesselFinder`), the credit `Photo: VesselFinder`.
//! 3. **Voyage** (padding 16, 12 apart, a rule under it): while loading two 84 × 22 skeletons at
//!    the sides and a 12 px one 70 % wide; else, with a destination, an ETA, a status or SAR:
//!    - the legs (`minmax(0,1fr) auto`, 12 apart): `Destination` (12 px `--text-3`) over the
//!      destination (Mono at 700: 22 px, line height 1.1, for a UN/LOCODE, else 14 px at 1.5; cut;
//!      tooltip `UN/LOCODE {dest}, as entered by the crew` or `As entered by the crew`) or `Not
//!      given` (12 px `--text-2`); on the right, with an ETA, `ETA` or `ETA, passed` over the time
//!      in UTC with its zone (Mono 14 px at 700, line height 1.5) and the day (`Today`, `Mon 6
//!      Oct`; 12 px `--text-2`), right-aligned, both `--text-3` once passed; tooltip `This ETA has
//!      passed; the crew hasn't updated it` or `As entered by the crew; the port's time zone isn't
//!      known, so in UTC`;
//!    - the badges, 8 apart: the navigational status in its tone (`navTone`; tooltip `AIS
//!      navigational status, as set by the crew`), `Search and rescue aircraft` (neutral).
//! 4. **Body** (padding 16, 12 apart):
//!    - after a failure: `Couldn't load this ship's details from VesselFinder.` (13 px
//!      `--text-2`), the error (11 px `--text-3`) and the `.button` `Try again` with `RefreshCw`,
//!      8 apart ([`Action::RetryShipCard`]);
//!    - the facts in three columns ([`View::facts`]): while loading six skeleton pairs (52 × 10,
//!      4 px over `44 + (i·23 mod 40)` × 12); else `IMO`, `MMSI`, `Flag` (tooltip its code),
//!      `Length × beam`, `Draught / max` or `Draught` (tooltip `The current draught as entered by
//!      the crew, and the ship's maximum`), `Built`, `Gross tonnage`, `Deadweight`, each only
//!      when known (the MMSI always), and `Position` across the row (tooltip `As the map last
//!      showed it`); all but the flag in Mono;
//!    - once the card is in, the notes (11 px `--text-3`, 2 apart): which of the status,
//!      destination and ETA shown the crew entered ([`crew_entered`]), and `Data: VesselFinder`.
//! 5. **Footer**, pinned to the panel's bottom (padding `12px 16px`, a rule on top): on the left
//!    (`flex: 1`: what the buttons leave, its text wrapping there) the report's age (`Position 2
//!    min ago`, 13 px `--text-2`, or past 15 minutes the warn badge `Position 47 min old` at 12
//!    px, which runs under the buttons when wider than its room; tooltip `Reported at {clock}
//!    {zone}`) and `via satellite AIS` (11 px `--text-3`) under it; while loading a 120 × 12
//!    skeleton; else for an old position the warn badge `Old position`. On the right Follow
//!    ([`Action::ToggleFollow`], pressed while the camera follows the ship) and `VesselFinder`
//!    with `ExternalLink` after it, opening the ship's page.
//!
//! **Checked** against Chromium's layout of today's component, server-rendered by Svelte with the
//! same props and laid out with app.css at 1440 × 900 (`tests/ship_ui.rs`: the full card with its
//! photo, a free-text destination with a passed ETA, an alarming status, SAR, satellite AIS and a
//! stale report, loading, the error): every part's box, rule, badge, skeleton and button lands on
//! Chromium's pixels or within one; text stands on Chromium's baselines (`ui::text`'s hook, F1).
//!
//! Deviations:
//! - The photo loads through egui's image loaders ([`super::panel::remote`]): `crate::images`
//!   (U18) from the network, or from the `--fixture` folder's files (F2; a shot never asks the
//!   network); one that fails is left out, as today when it does not load.
//! - Closed and opened again, the panel keeps how far its body was scrolled (egui keeps a scroll
//!   area's state by its id); today's component mounts afresh at the top.
//! - In the phone's bottom sheet ([`super::sheet::in_sheet`], U19): the strip, the voyage and the
//!   footer are marked for its peek detent, the photo follows the voyage (doc 06 §8.2).

use egui::text::LayoutJob;
use egui::{Color32, Context, CornerRadius, Id, Pos2, Rect, Response, Sense, Ui, Vec2};
use rustradar_core::freshness::Freshness;
use rustradar_core::model::{NavStatus, Ship, ShipDetails, ship_page_url, ship_photo_url};
use rustradar_core::num::to_fixed;
use rustradar_core::readings::degrees;
use rustradar_core::ship_details::{dimensions, draught, is_locode, lat_lon, nav_tone, position_age};
use rustradar_core::ship_list::ship_name;
use rustradar_core::ships::ship_kind_label;
use rustradar_core::text::group_thousands;
use rustradar_core::time::Clocks;

use super::fonts::Face;
use super::lists::ships::paragraph;
use super::icons::{self, Icon};
use super::panel::{self, Cell, Fact, Fit, Part, Photo, Piece, Remote};
use super::shell::Pass;
use super::text::{self, Style};
use super::theme;
use super::widgets::badge::{Badge, Tone};
use super::widgets::button::Button;
use super::widgets::{skeleton, tooltip};
use crate::state::Action;

/// The panel's accessible name (`aria-label`).
pub const LABEL: &str = "Selected ship";
/// The close button's accessible name.
pub const CLOSE_LABEL: &str = "Close ship details";
/// A ship with no name anywhere.
pub const UNNAMED: &str = "Unnamed ship";
/// The moving cell's tooltip, and its dash for a speed not known.
pub const MOVING_TOOLTIP: &str = "Speed and course over ground";
pub const DASH: &str = "—";
/// The photo's tooltip and credit.
pub const PHOTO_TOOLTIP: &str = "View this ship on VesselFinder";
pub const PHOTO_CREDIT: &str = "Photo: VesselFinder";
/// The voyage's labels and tooltips.
pub const DESTINATION: &str = "Destination";
pub const NOT_GIVEN: &str = "Not given";
pub const FREE_TEXT_TOOLTIP: &str = "As entered by the crew";
pub const ETA: &str = "ETA";
pub const ETA_PASSED: &str = "ETA, passed";
pub const ETA_TOOLTIP: &str = "As entered by the crew; the port's time zone isn't known, so in UTC";
pub const ETA_PASSED_TOOLTIP: &str = "This ETA has passed; the crew hasn't updated it";
pub const STATUS_TOOLTIP: &str = "AIS navigational status, as set by the crew";
pub const SAR: &str = "Search and rescue aircraft";
/// The failure's words and button.
pub const FAILED: &str = "Couldn't load this ship's details from VesselFinder.";
pub const RETRY: &str = "Try again";
/// The facts' tooltips.
pub const DRAUGHT_TOOLTIP: &str = "The current draught as entered by the crew, and the ship's maximum";
pub const POSITION_TOOLTIP: &str = "As the map last showed it";
/// The data's credit.
pub const DATA_CREDIT: &str = "Data: VesselFinder";
/// The footer's texts.
pub const VIA_SATELLITE: &str = "via satellite AIS";
pub const OLD_POSITION: &str = "Old position";
pub const VESSELFINDER: &str = "VesselFinder";

/// Longer names than this (UTF-16 units, `name.length`) take the strip's smaller title.
pub const LONG_NAME: usize = 13;
/// The strip's skeletons: the subtitle's, the speed's and the course's.
pub const SUBTITLE_SKELETON: Vec2 = Vec2::new(140.0, 12.0);
pub const SPEED_SKELETON: Vec2 = Vec2::new(56.0, 14.0);
pub const COURSE_SKELETON: Vec2 = Vec2::new(36.0, 12.0);
/// The voyage's padding, gaps, a leg's line gap and the badges' gap.
pub const VOYAGE_PAD: f32 = theme::SPACE_4;
pub const VOYAGE_GAP: f32 = theme::SPACE_3;
pub const LEGS_GAP: f32 = theme::SPACE_3;
pub const LEG_LINE_GAP: f32 = 2.0;
pub const BADGES_GAP: f32 = theme::SPACE_2;
/// The voyage's skeletons while loading: the codes', and the line's height and share of the width.
pub const CODE_SKELETON: Vec2 = Vec2::new(84.0, 22.0);
pub const LINE_SKELETON_HEIGHT: f32 = 12.0;
pub const LINE_SKELETON_SHARE: f32 = 0.7;
/// The body's padding and gap, the failure's gap, the notes' gap.
pub const BODY_PAD: f32 = theme::SPACE_4;
pub const BODY_GAP: f32 = theme::SPACE_3;
pub const STACK_GAP: f32 = theme::SPACE_2;
pub const NOTES_GAP: f32 = 2.0;
/// The facts' skeletons while loading: how many, the label's size and its margin under it.
pub const FACT_SKELETONS: usize = 6;
pub const FACT_LABEL_SKELETON: Vec2 = Vec2::new(52.0, 10.0);
pub const FACT_LABEL_MARGIN: f32 = 4.0;
pub const FACT_VALUE_SKELETON_HEIGHT: f32 = 12.0;
/// The footer's report: its parts' gap, the skeleton while loading.
pub const REPORT_GAP: f32 = 2.0;
pub const REPORT_SKELETON: Vec2 = Vec2::new(120.0, 12.0);

/// The strip's title: Mono 22 px at 700 (17 px at line height 1.3 when `long`).
pub fn title_style(long: bool) -> Style {
    let style = panel::title_style(Face::Mono);
    if long { style.size(theme::TEXT_LG).line_height(1.3) } else { style }
}

/// A leg's label (`.label`: 12 px `--text-3`).
pub const fn label_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The destination (`.code.data`: Mono at 700; 22 px at 1.1 for a UN/LOCODE, else `.free` 14 px
/// at 1.5).
pub fn destination_style(locode: bool) -> Style {
    let style = Style::data(theme::TEXT_XL, 700, theme::TEXT_1).line_height(panel::TITLE_LINE_HEIGHT);
    if locode { style } else { style.size(theme::TEXT_BASE).line_height(1.5) }
}

/// The ETA's time (`.end .code.data`: Mono 14 px at 700, line height 1.5), `--text-3` once
/// passed.
pub const fn eta_style(passed: bool) -> Style {
    Style::data(theme::TEXT_BASE, 700, if passed { theme::TEXT_3 } else { theme::TEXT_1 }).line_height(1.5)
}

/// The ETA's day and `Not given` (`.when`, `.unknown`: 12 px `--text-2`), the day `--text-3`
/// once passed.
pub const fn when_style(passed: bool) -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, if passed { theme::TEXT_3 } else { theme::TEXT_2 })
}

/// The failure's words (`.explain`: 13 px `--text-2`).
pub const fn explain_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The report's age while recent (`.status`: 13 px `--text-2`).
pub const fn status_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// `text.length` in UTF-16 units, as JavaScript counts.
fn js_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `Some(text)` unless it is empty (JavaScript's truthiness of a string).
fn non_empty(text: Option<&str>) -> Option<&str> {
    text.filter(|t| !t.is_empty())
}

/// `{Status, destination and ETA} are` (or `is` for one): what of the status, destination and
/// ETA shown the crew typed in, so it isn't read as measured (`crewEntered`); empty for none.
pub fn crew_entered(status: bool, destination: bool, eta: bool) -> String {
    let items: Vec<&str> = [(status, "status"), (destination, "destination"), (eta, "ETA")].into_iter().filter(|(on, _)| *on).map(|(_, w)| w).collect();
    let Some((last, rest)) = items.split_last() else { return String::new() };
    let list = if rest.is_empty() { (*last).to_owned() } else { format!("{} and {last}", rest.join(", ")) };
    let mut chars = list.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    format!("{first}{} {}", chars.as_str(), if items.len() > 1 { "are" } else { "is" })
}

/// One fact: its label, value, whether in Mono, its tooltip, whether across the row.
#[derive(Debug, Clone, PartialEq)]
pub struct FactRow {
    pub label: &'static str,
    pub value: String,
    pub data: bool,
    pub tooltip: Option<String>,
    pub wide: bool,
}

/// The selected ship as the panel shows it.
#[derive(Debug, Clone)]
pub struct View<'a> {
    pub ship: &'a Ship,
    pub details: Option<&'a ShipDetails>,
    pub error: Option<&'a str>,
    /// The ticking clock, local Unix ms.
    pub now: f64,
    pub following: bool,
    pub clocks: Clocks,
}

impl View<'_> {
    /// Neither the card nor a failure yet (`loading`).
    pub fn loading(&self) -> bool {
        self.details.is_none() && self.error.is_none()
    }

    /// `details?.name || (ship.name !== String(ship.mmsi) ? ship.name : "") || "Unnamed ship"`.
    pub fn name(&self) -> &str {
        non_empty(self.details.and_then(|d| d.name.as_deref())).or(non_empty(Some(ship_name(self.ship)))).unwrap_or(UNNAMED)
    }

    /// VesselFinder's own wording, else the map's colour class (none for "unknown", kind 0).
    pub fn type_label(&self) -> Option<&str> {
        non_empty(self.details.and_then(|d| d.type_label.as_deref())).or((self.ship.kind != 0).then(|| ship_kind_label(self.ship.kind)))
    }

    /// `[typeLabel, details?.flag].filter(Boolean).join(" · ")`.
    pub fn subtitle(&self) -> String {
        let flag = non_empty(self.details.and_then(|d| d.flag.as_deref()));
        [self.type_label(), flag].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }

    /// A speed or a course is known (`moving`).
    pub fn moving(&self) -> bool {
        self.details.is_some_and(|d| d.speed_kn.is_some() || d.course_deg.is_some())
    }

    /// `{speedKn.toFixed(1)} kn` or a dash.
    pub fn speed(&self) -> String {
        match self.details.and_then(|d| d.speed_kn) {
            Some(kn) => format!("{} kn", to_fixed(kn, 1)),
            None => DASH.to_owned(),
        }
    }

    /// The course over ground (`271°`), or nothing.
    pub fn course(&self) -> String {
        self.details.and_then(|d| d.course_deg).map(degrees).unwrap_or_default()
    }

    /// The card's photo id, when it names one.
    pub fn photo_id(&self) -> Option<&str> {
        non_empty(self.details.and_then(|d| d.photo_id.as_deref()))
    }

    /// The navigational status as set by the crew.
    pub fn status(&self) -> Option<&NavStatus> {
        self.details.and_then(|d| d.nav_status.as_ref())
    }

    /// The destination as entered by the crew (none when empty).
    pub fn destination(&self) -> Option<&str> {
        non_empty(self.details.and_then(|d| d.destination.as_deref()))
    }

    /// The ETA as entered by the crew, Unix seconds (none for 0).
    pub fn eta(&self) -> Option<i64> {
        self.details.and_then(|d| d.eta_unix).filter(|e| *e != 0)
    }

    /// An ETA the ship has already passed (`etaPassed`).
    pub fn eta_passed(&self) -> bool {
        self.eta().is_some_and(|e| (e as f64) * 1000.0 < self.now)
    }

    /// A search-and-rescue aircraft.
    pub fn sar(&self) -> bool {
        self.details.is_some_and(|d| d.sar)
    }

    /// The report's age, when the card times it (`positionAge(details.positionUnix, now)`).
    pub fn age(&self) -> Option<Freshness> {
        self.details.and_then(|d| d.position_unix).filter(|p| *p != 0).map(|p| position_age(p, self.now as i64))
    }

    /// The report's tooltip: `Reported at {clockWithZone(positionUnix)}`.
    pub fn reported_at(&self) -> String {
        format!("Reported at {}", self.clocks.clock_with_zone(self.details.and_then(|d| d.position_unix), None))
    }

    /// The facts, in order (`facts`): each only when known, the MMSI and the position always.
    pub fn facts(&self) -> Vec<FactRow> {
        let d = self.details;
        let fact = |label, value: String, data| FactRow { label, value, data, tooltip: None, wide: false };
        let count = |n: u32| group_thousands(f64::from(n));
        let mut list = Vec::new();
        if let Some(imo) = d.and_then(|d| d.imo).filter(|n| *n != 0) {
            list.push(fact("IMO", imo.to_string(), true));
        }
        list.push(fact("MMSI", self.ship.mmsi.to_string(), true));
        if let Some(flag) = non_empty(d.and_then(|d| d.flag.as_deref())) {
            list.push(FactRow { tooltip: d.and_then(|d| d.flag_code.clone()), ..fact("Flag", flag.to_owned(), false) });
        }
        if let Some(size) = d.and_then(|d| dimensions(d.length_m, d.beam_m)) {
            list.push(fact("Length × beam", size, true));
        }
        if let Some(d) = d
            && let Some(depth) = draught(d.draught_m, d.max_draught_m)
        {
            let truthy = |v: Option<f64>| v.is_some_and(|v| v != 0.0 && !v.is_nan());
            let label = if truthy(d.draught_m) && truthy(d.max_draught_m) { "Draught / max" } else { "Draught" };
            list.push(FactRow { tooltip: Some(DRAUGHT_TOOLTIP.to_owned()), ..fact(label, depth, true) });
        }
        if let Some(year) = d.and_then(|d| d.year_built).filter(|y| *y != 0) {
            list.push(fact("Built", year.to_string(), true));
        }
        if let Some(gt) = d.and_then(|d| d.gross_tonnage).filter(|n| *n != 0) {
            list.push(fact("Gross tonnage", count(gt), true));
        }
        if let Some(dwt) = d.and_then(|d| d.deadweight).filter(|n| *n != 0) {
            list.push(fact("Deadweight", format!("{} t", count(dwt)), true));
        }
        list.push(FactRow { tooltip: Some(POSITION_TOOLTIP.to_owned()), wide: true, ..fact("Position", lat_lon(self.ship.lat, self.ship.lon), true) });
        list
    }

    /// The crew's note (`{crewEntered} entered by the crew.`), when anything shown was.
    pub fn crew_note(&self) -> Option<String> {
        let said = crew_entered(self.status().is_some(), self.destination().is_some(), self.eta().is_some());
        (!said.is_empty()).then(|| format!("{said} entered by the crew."))
    }
}

/// The panel in the detail panel slot (the shell's [`Slot`](super::shell::Slot)).
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) {
    let state = pass.state;
    let Some(ship) = state.ships.selected() else { return };
    let ctx = ui.ctx().clone();
    // the report's age ticks with the 1 s clock
    ctx.request_repaint_after_secs(1.0);
    let view = View {
        ship,
        details: state.ships.card_details(),
        error: state.ships.card_error(),
        now: pass.now_ms,
        following: state.ships.following(),
        clocks: state.viewer_clocks.clone(),
    };
    let width = ui.available_width();
    let footer_height = footer_height(&ctx, &view, width);
    // in the phone's sheet the footer is part of the summary (U19)
    super::sheet::peek_part(&ctx, footer_height);
    let id = Id::unique("rustradar::ship_panel");
    panel::body(ui, id, footer_height, |ui, part| match part {
        Part::Content => content(ui, &view, pass.actions),
        Part::Footer(rect) => footer(ui, rect, &view, pass.actions),
    });
}

/// Everything above the footer.
fn content(ui: &mut Ui, view: &View<'_>, actions: &mut Vec<Action>) {
    use super::sheet::{in_sheet, peek};
    // in the phone's sheet: strip and voyage (marked for the peek detent), then the photo and the
    // body (doc 06 §8.2, U19)
    let phone = in_sheet(ui.ctx());
    if peek(ui, |ui| strip(ui, view)).clicked() {
        actions.push(Action::CloseShip);
    }
    if !phone {
        photo(ui, view);
    }
    peek(ui, |ui| voyage(ui, view));
    if phone {
        photo(ui, view);
    }
    if body(ui, view) {
        actions.push(Action::RetryShipCard);
    }
}

/// The strip; answers the close button's response.
fn strip(ui: &mut Ui, view: &View<'_>) -> Response {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let top = ui.cursor().min;
    let name = view.name();
    let title = title_style(js_length(name) > LONG_NAME);
    let subtitle = view.subtitle();
    let sub_style = panel::sub_style(Face::Ui);
    let mut id_pieces = vec![Piece::text(name, &title, Fit::Ellipsis)];
    let sub_h = if !subtitle.is_empty() {
        id_pieces.push(Piece::text(&subtitle, &sub_style, Fit::Ellipsis));
        Some(sub_style.line_px())
    } else if view.loading() {
        id_pieces.push(Piece::Skeleton(SUBTITLE_SKELETON));
        Some(SUBTITLE_SKELETON.y)
    } else {
        None
    };
    let id_h = title.line_px() + sub_h.map_or(0.0, |h| panel::CELL_GAP + h);
    let mut cells = vec![Cell::fill(id_pieces)];
    let mut other_h = 0.0;
    if view.moving() {
        let value = panel::value_style(theme::AMBER);
        let sub = panel::sub_style(Face::Mono);
        other_h = value.line_px() + panel::CELL_GAP + sub.line_px();
        cells.push(Cell::auto(vec![Piece::text(&view.speed(), &value, Fit::Ellipsis), Piece::text(&view.course(), &sub, Fit::Ellipsis)]).tooltip(Some(MOVING_TOOLTIP.to_owned())));
    } else if view.loading() {
        other_h = SPEED_SKELETON.y + panel::CELL_GAP + COURSE_SKELETON.y;
        cells.push(Cell::auto(vec![Piece::Skeleton(SPEED_SKELETON), Piece::Skeleton(COURSE_SKELETON)]));
    }
    let id_w = (panel::strip_widths(&ctx, &cells, width)[0] - panel::CELL_PAD_FIRST - panel::CELL_PAD).max(0.0);
    let closed = panel::strip(ui, cells, CLOSE_LABEL);
    // the name's and the subtitle's own titles, over their whole lines
    let content_h = id_h.max(other_h);
    let mut y = top.y + panel::CELL_PAD + (content_h - id_h) / 2.0;
    let x = top.x + panel::CELL_PAD_FIRST;
    let title_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(id_w, title.line_px()));
    tooltip::show(ui.interact(title_rect, Id::unique("rustradar::ship_panel::name"), Sense::hover()), name);
    y += title.line_px() + panel::CELL_GAP;
    if !subtitle.is_empty() {
        let sub_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(id_w, sub_style.line_px()));
        tooltip::show(ui.interact(sub_rect, Id::unique("rustradar::ship_panel::subtitle"), Sense::hover()), &subtitle);
    }
    closed
}

/// The photo: the block while loading, else VesselFinder's picture unless it failed.
fn photo(ui: &mut Ui, view: &View<'_>) {
    if view.loading() {
        panel::photo_skeleton(ui, panel::PHOTO_ASPECT);
        return;
    }
    let Some(id) = view.photo_id() else { return };
    let uri = ship_photo_url(id, false);
    if panel::remote(ui.ctx(), &uri) == Remote::Failed {
        return;
    }
    let shown = Photo { uri: &uri, credit: Some(PHOTO_CREDIT.to_owned()), tooltip: PHOTO_TOOLTIP, count: None };
    if panel::photo(ui, Id::unique("rustradar::ship_panel::photo"), panel::PHOTO_ASPECT, shown).opened {
        ui.ctx().open_url(egui::OpenUrl::new_tab(ship_page_url(view.ship.mmsi)));
    }
}

/// `job` laid out in `painter`'s fonts, its galley centred on a `line` px box at `top`, from
/// `left` (or ending at `left` when `right`); answers its box.
fn put(painter: &egui::Painter, job: LayoutJob, line: f32, left: f32, top: f32, right: bool) -> Rect {
    let galley = painter.layout_job(job);
    let w = galley.size().x;
    let x = if right { left - w } else { left };
    let rect = Rect::from_min_size(Pos2::new(x, top), Vec2::new(w, line));
    painter.galley(Pos2::new(x, top + (line - galley.size().y) / 2.0), galley, Color32::PLACEHOLDER);
    rect
}

/// The badges of the voyage: the status in its tone, SAR.
fn voyage_badges(view: &View<'_>) -> Vec<(Badge, Option<&'static str>)> {
    let mut badges = Vec::new();
    if let Some(status) = view.status() {
        badges.push((Badge::new(status.label.clone(), Tone::from(nav_tone(status))), Some(STATUS_TOOLTIP)));
    }
    if view.sar() {
        badges.push((Badge::new(SAR, Tone::Neutral), None));
    }
    badges
}

/// The badges laid out in rows across `width`, 8 apart (`flex-wrap: wrap`): each one's place
/// from the row's top left, and the rows' height.
fn wrap(sizes: &[Vec2], width: f32) -> (Vec<Vec2>, f32) {
    let (mut x, mut y, mut row_h) = (0.0f32, 0.0f32, 0.0f32);
    let mut places = Vec::new();
    for size in sizes {
        if x > 0.0 && x + size.x > width {
            y += row_h + BADGES_GAP;
            x = 0.0;
            row_h = 0.0;
        }
        places.push(Vec2::new(x, y));
        x += size.x + BADGES_GAP;
        row_h = row_h.max(size.y);
    }
    (places, if sizes.is_empty() { 0.0 } else { y + row_h })
}

/// The voyage: where to and when, the status.
fn voyage(ui: &mut Ui, view: &View<'_>) {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let inner = width - 2.0 * VOYAGE_PAD;
    if view.loading() {
        let height = 2.0 * VOYAGE_PAD + CODE_SKELETON.y + VOYAGE_GAP + LINE_SKELETON_HEIGHT;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height + theme::BORDER), Sense::hover());
        let painter = ui.painter();
        let (x, y) = (rect.left() + VOYAGE_PAD, rect.top() + VOYAGE_PAD);
        skeleton::paint(painter, Rect::from_min_size(Pos2::new(x, y), CODE_SKELETON), theme::RADIUS);
        skeleton::paint(painter, Rect::from_min_size(Pos2::new(x + inner - CODE_SKELETON.x, y), CODE_SKELETON), theme::RADIUS);
        let line = Rect::from_min_size(Pos2::new(x, y + CODE_SKELETON.y + VOYAGE_GAP), Vec2::new(inner * LINE_SKELETON_SHARE, LINE_SKELETON_HEIGHT));
        skeleton::paint(painter, line, theme::RADIUS);
        panel::hrule(painter, rect.x_range(), rect.bottom() - theme::BORDER);
        return;
    }
    let destination = view.destination();
    let eta = view.eta();
    let badges = voyage_badges(view);
    if destination.is_none() && eta.is_none() && badges.is_empty() {
        return;
    }
    let label = label_style();
    let passed = view.eta_passed();
    // the legs: the right column as wide as its widest line
    let legs = (destination.is_some() || eta.is_some()).then(|| {
        let right = eta.map(|eta| {
            let clock = view.clocks.clock_with_zone(eta, Some("UTC"));
            let day = view.clocks.day_at(eta as f64, Some("UTC"), view.now as i64);
            let label_text = if passed { ETA_PASSED } else { ETA };
            let w = [text::measure(&ctx, text::job(label_text, &label)).x, text::measure(&ctx, text::job(&clock, &eta_style(passed))).x, text::measure(&ctx, text::job(&day, &when_style(passed))).x]
                .into_iter()
                .fold(0.0, f32::max);
            (label_text, clock, day, w)
        });
        let right_w = right.as_ref().map_or(0.0, |r| r.3);
        let value = match destination {
            Some(d) => destination_style(is_locode(d)),
            None => when_style(false),
        };
        let left_h = label.line_px() + LEG_LINE_GAP + value.line_px();
        let right_h = right.as_ref().map_or(0.0, |_| label.line_px() + LEG_LINE_GAP + eta_style(passed).line_px() + LEG_LINE_GAP + when_style(passed).line_px());
        (right, right_w, value, left_h.max(right_h))
    });
    let badge_sizes: Vec<Vec2> = badges.iter().map(|(b, _)| b.measure(&ctx)).collect();
    let (places, badges_h) = wrap(&badge_sizes, inner);
    let legs_h = legs.as_ref().map_or(0.0, |l| l.3);
    let gap = if legs.is_some() && !badges.is_empty() { VOYAGE_GAP } else { 0.0 };
    let height = 2.0 * VOYAGE_PAD + legs_h + gap + badges_h;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height + theme::BORDER), Sense::hover());
    let painter = ui.painter().clone();
    let (x, y) = (rect.left() + VOYAGE_PAD, rect.top() + VOYAGE_PAD);
    if let Some((right, right_w, value, _)) = legs {
        // the grid's gap stands between the columns even without an ETA
        let left_w = (inner - right_w - LEGS_GAP).max(0.0);
        put(&painter, text::job(DESTINATION, &label), label.line_px(), x, y, false);
        let value_top = y + label.line_px() + LEG_LINE_GAP;
        match destination {
            Some(d) => {
                let r = put(&painter, text::truncated(d, &value, left_w), value.line_px(), x, value_top, false);
                let tip = if is_locode(d) { format!("UN/LOCODE {d}, as entered by the crew") } else { FREE_TEXT_TOOLTIP.to_owned() };
                tooltip::show(ui.interact(r, Id::unique("rustradar::ship_panel::destination"), Sense::hover()), &tip);
            }
            None => {
                put(&painter, text::job(NOT_GIVEN, &value), value.line_px(), x, value_top, false);
            }
        }
        if let Some((label_text, clock, day, w)) = right {
            let end = x + inner;
            put(&painter, text::job(label_text, &label), label.line_px(), end, y, true);
            let clock_top = y + label.line_px() + LEG_LINE_GAP;
            put(&painter, text::job(&clock, &eta_style(passed)), eta_style(passed).line_px(), end, clock_top, true);
            let day_top = clock_top + eta_style(passed).line_px() + LEG_LINE_GAP;
            put(&painter, text::job(&day, &when_style(passed)), when_style(passed).line_px(), end, day_top, true);
            let column = Rect::from_min_max(Pos2::new(end - w, y), Pos2::new(end, day_top + when_style(passed).line_px()));
            tooltip::show(ui.interact(column, Id::unique("rustradar::ship_panel::eta"), Sense::hover()), if passed { ETA_PASSED_TOOLTIP } else { ETA_TOOLTIP });
        }
    }
    let badges_top = y + legs_h + gap;
    for (n, ((badge, tip), place)) in badges.into_iter().zip(places).enumerate() {
        let r = badge.paint(&painter, Pos2::new(x, badges_top) + place);
        if let Some(tip) = tip {
            tooltip::show(ui.interact(r, Id::unique(("rustradar::ship_panel::badge", n)), Sense::hover()), tip);
        }
    }
    panel::hrule(&painter, rect.x_range(), rect.bottom() - theme::BORDER);
}

/// The facts as the grid lays them out: skeleton pairs while loading, else the ship's.
fn facts(view: &View<'_>) -> Vec<Fact<'static>> {
    if view.loading() {
        return (0..FACT_SKELETONS)
            .map(|i| {
                // the label's skeleton, its 4 px margin taking the place of `dd`'s 2
                let label = Piece::Custom(
                    Vec2::new(FACT_LABEL_SKELETON.x, FACT_LABEL_SKELETON.y + FACT_LABEL_MARGIN - panel::DD_GAP),
                    Box::new(|painter: &egui::Painter, rect: Rect| {
                        skeleton::paint(painter, panel::snap_rect(Rect::from_min_size(rect.min, FACT_LABEL_SKELETON), painter.pixels_per_point()), theme::RADIUS);
                    }),
                );
                let value = Piece::Skeleton(Vec2::new(44.0 + ((i * 23) % 40) as f32, FACT_VALUE_SKELETON_HEIGHT));
                Fact { label, value, span: panel::Span::One, tooltip: None }
            })
            .collect();
    }
    view.facts()
        .into_iter()
        .map(|f| {
            let style = panel::dd_style(if f.data { Face::Mono } else { Face::Ui }, false);
            Fact::new(f.label, Piece::text(&f.value, &style, Fit::Wrap)).wide(f.wide).tooltip(f.tooltip)
        })
        .collect()
}

/// The body: the failure, the facts, the notes; answers whether Try again was clicked.
fn body(ui: &mut Ui, view: &View<'_>) -> bool {
    let width = ui.available_width();
    let inner = width - 2.0 * BODY_PAD;
    let mut retry = false;
    ui.add_space(BODY_PAD);
    ui.horizontal(|ui| {
        ui.add_space(BODY_PAD);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.set_width(inner);
            if let Some(error) = view.error {
                paragraph(ui, FAILED, &explain_style(), inner);
                ui.add_space(STACK_GAP);
                paragraph(ui, error, &panel::note_style(), inner);
                ui.add_space(STACK_GAP);
                retry = Button::new().icon(Icon::RefreshCw.at(14.0, icons::STROKE_THIN)).text(RETRY).show(ui).clicked();
                ui.add_space(BODY_GAP);
            }
            panel::facts(ui, 3, panel::FACTS_GAP, facts(view));
            if view.details.is_some() {
                ui.add_space(BODY_GAP);
                let notes: Vec<String> = view.crew_note().into_iter().chain([DATA_CREDIT.to_owned()]).collect();
                for (i, note) in notes.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(NOTES_GAP);
                    }
                    paragraph(ui, note, &panel::note_style(), inner);
                }
            }
        });
    });
    ui.add_space(BODY_PAD);
    retry
}

/// The footer's report, top-down.
enum Report {
    /// The age while recent: plain text, wrapped across the report's width.
    Text(String),
    /// A warn badge at 12 px (a stale age, an old position).
    Badge(Badge),
    /// The skeleton while loading.
    Skeleton,
}

/// The report's first part, whether it says `via satellite AIS` under it, and its tooltip.
fn report(view: &View<'_>) -> Option<(Report, bool, Option<String>)> {
    if let Some(age) = view.age() {
        let satellite = view.details.is_some_and(|d| d.satellite);
        let first = if age.stale { Report::Badge(Badge::new(age.label, Tone::Warn).size(theme::TEXT_SM)) } else { Report::Text(age.label) };
        Some((first, satellite, Some(view.reported_at())))
    } else if view.loading() {
        Some((Report::Skeleton, false, None))
    } else if view.ship.old {
        Some((Report::Badge(Badge::new(OLD_POSITION, Tone::Warn).size(theme::TEXT_SM)), false, None))
    } else {
        None
    }
}

/// The footer's buttons: Follow, and the ship's page.
fn buttons(view: &View<'_>) -> Vec<Button> {
    let page = Button::new().text(VESSELFINDER).icon(Icon::ExternalLink.at(14.0, icons::STROKE_THIN));
    vec![panel::follow_button(view.following, false), page]
}

/// The report's width: what the buttons leave in a footer `width` px wide (`flex: 1`).
fn report_width(ctx: &Context, view: &View<'_>, width: f32) -> f32 {
    let sizes: Vec<Vec2> = buttons(view).iter().map(|b| b.size(ctx)).collect();
    let actions = sizes.iter().map(|s| s.x).sum::<f32>() + panel::FOOTER_GAP * sizes.len().saturating_sub(1) as f32;
    (width - 2.0 * panel::FOOTER_PAD.x - actions - panel::FOOTER_GAP).max(0.0)
}

/// The report's parts laid out across `room`: each one's height.
fn report_heights(ctx: &Context, view: &View<'_>, room: f32) -> Vec<f32> {
    let Some((first, satellite, _)) = report(view) else { return Vec::new() };
    let first = match first {
        Report::Text(label) => {
            let rows = ctx.fonts_mut(|f| f.layout_job(text::tabular(text::wrapped(&label, &status_style(), room)))).rows.len().max(1);
            status_style().line_px() * rows as f32
        }
        Report::Badge(badge) => badge.measure(ctx).y,
        Report::Skeleton => REPORT_SKELETON.y,
    };
    let mut heights = vec![first];
    if satellite {
        let rows = ctx.fonts_mut(|f| f.layout_job(text::wrapped(VIA_SATELLITE, &panel::note_style(), room))).rows.len().max(1);
        heights.push(panel::note_style().line_px() * rows as f32);
    }
    heights
}

/// The footer's height across `width`: its rule, padding and one row, as tall as the report or
/// the buttons.
fn footer_height(ctx: &Context, view: &View<'_>, width: f32) -> f32 {
    let heights = report_heights(ctx, view, report_width(ctx, view, width));
    let report_h = heights.iter().sum::<f32>() + REPORT_GAP * heights.len().saturating_sub(1) as f32;
    let tallest = buttons(view).iter().map(|b| b.size(ctx).y).fold(report_h, f32::max);
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
    // the report first: a badge wider than its room runs under the buttons
    let room = report_width(&ctx, view, rect.width());
    let heights = report_heights(&ctx, view, room);
    let report_h = heights.iter().sum::<f32>() + REPORT_GAP * heights.len().saturating_sub(1) as f32;
    if let Some((first, satellite, tip)) = report(view) {
        let mut y = mid - report_h / 2.0;
        let first_rect = match first {
            Report::Text(label) => {
                let style = status_style();
                let galley = painter.layout_job(text::tabular(text::wrapped(&label, &style, room)));
                let rows = galley.rows.len().max(1) as f32;
                let at = Pos2::new(row.left(), y + (style.line_px() * rows - galley.size().y) / 2.0 / rows);
                let r = Rect::from_min_size(Pos2::new(row.left(), y), Vec2::new(galley.size().x, style.line_px() * rows));
                painter.galley(at, galley, Color32::PLACEHOLDER);
                r
            }
            Report::Badge(badge) => badge.paint(&painter, Pos2::new(row.left(), y)),
            Report::Skeleton => {
                let r = Rect::from_min_size(Pos2::new(row.left(), y), REPORT_SKELETON);
                skeleton::paint(&painter, panel::snap_rect(r, ppp), theme::RADIUS);
                r
            }
        };
        if let Some(tip) = tip {
            tooltip::show(ui.interact(first_rect, Id::unique("rustradar::ship_panel::report"), Sense::hover()), &tip);
        }
        y += heights[0] + REPORT_GAP;
        if satellite {
            let style = panel::note_style();
            let galley = painter.layout_job(text::wrapped(VIA_SATELLITE, &style, room));
            let rows = galley.rows.len().max(1) as f32;
            painter.galley(Pos2::new(row.left(), y + (style.line_px() * rows - galley.size().y) / 2.0 / rows), galley, Color32::PLACEHOLDER);
        }
    }
    let buttons = buttons(view);
    let sizes: Vec<Vec2> = buttons.iter().map(|b| b.size(&ctx)).collect();
    let total = sizes.iter().map(|s| s.x).sum::<f32>() + panel::FOOTER_GAP * sizes.len().saturating_sub(1) as f32;
    let mut x = row.right() - total;
    for (n, (button, size)) in buttons.into_iter().zip(sizes).enumerate() {
        let r = panel::snap_rect(Rect::from_min_size(Pos2::new(x, mid - size.y / 2.0), size), ppp);
        if panel::put(ui, ("rustradar::ship_panel::button", n), r, button).clicked() {
            match n {
                0 => actions.push(Action::ToggleFollow),
                _ => ctx.open_url(egui::OpenUrl::new_tab(ship_page_url(view.ship.mmsi))),
            }
        }
        x += size.x + panel::FOOTER_GAP;
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::NavStatus;

    use super::*;

    /// 2026-06-21 10:00 UTC.
    const NOW: f64 = 1_782_036_000_000.0;

    fn ship() -> Ship {
        Ship { mmsi: 265_177_000, lat: 57.7072, lon: 11.9441, name: "KATTEGAT EXPRESS".into(), kind: 3, size: 2, heading: Some(271.0), ..Ship::default() }
    }

    fn details() -> ShipDetails {
        ShipDetails {
            name: Some("KATTEGAT EXPRESS".into()),
            imo: Some(9_235_517),
            mmsi: 265_177_000,
            type_label: Some("Passenger/Ro-Ro Cargo Ship".into()),
            flag: Some("Sweden".into()),
            flag_code: Some("SE".into()),
            length_m: Some(184.0),
            beam_m: Some(30.0),
            draught_m: Some(6.3),
            max_draught_m: Some(6.6),
            gross_tonnage: Some(29_289),
            deadweight: Some(5_500),
            year_built: Some(1983),
            destination: Some("DKFDH".into()),
            eta_unix: Some(1_782_048_300),
            speed_kn: Some(14.2),
            course_deg: Some(271.0),
            nav_status: Some(NavStatus { code: 0, label: "Under way using engine".into() }),
            position_unix: Some(1_782_035_925),
            satellite: false,
            photo_id: Some("kattegat-express".into()),
            sar: false,
        }
    }

    fn view<'a>(ship: &'a Ship, details: Option<&'a ShipDetails>, error: Option<&'a str>) -> View<'a> {
        View { ship, details, error, now: NOW, following: false, clocks: Clocks::default() }
    }

    #[test]
    fn what_the_crew_entered_reads_as_today() {
        assert_eq!(crew_entered(false, false, false), "");
        assert_eq!(crew_entered(true, false, false), "Status is");
        assert_eq!(crew_entered(false, true, false), "Destination is");
        assert_eq!(crew_entered(false, false, true), "ETA is");
        assert_eq!(crew_entered(true, true, false), "Status and destination are");
        assert_eq!(crew_entered(false, true, true), "Destination and ETA are");
        assert_eq!(crew_entered(true, true, true), "Status, destination and ETA are");
    }

    #[test]
    fn the_name_the_subtitle_and_the_strip_values() {
        let s = ship();
        let d = details();
        let full = view(&s, Some(&d), None);
        assert!(!full.loading());
        assert_eq!(full.name(), "KATTEGAT EXPRESS");
        assert_eq!(full.subtitle(), "Passenger/Ro-Ro Cargo Ship · Sweden");
        assert!(full.moving());
        assert_eq!((full.speed(), full.course()), ("14.2 kn".to_owned(), "271°".to_owned()));
        // loading: the feed's name and the map's class
        let loading = view(&s, None, None);
        assert!(loading.loading() && !loading.moving());
        assert_eq!((loading.name(), loading.subtitle().as_str()), ("KATTEGAT EXPRESS", "Passenger"));
        // the MMSI as the name, an unknown type: unnamed, no subtitle
        let unnamed = Ship { name: "265177000".into(), kind: 0, ..ship() };
        let v = view(&unnamed, None, Some("503"));
        assert_eq!((v.name(), v.subtitle().as_str()), (UNNAMED, ""));
        // a card without a name or type: the feed's, and an empty name in the card is no name
        let bare = ShipDetails { name: Some(String::new()), type_label: None, flag: None, speed_kn: None, course_deg: Some(118.0), ..details() };
        let v = view(&s, Some(&bare), None);
        assert_eq!((v.name(), v.subtitle().as_str()), ("KATTEGAT EXPRESS", "Passenger"));
        assert!(v.moving());
        assert_eq!((v.speed(), v.course()), (DASH.to_owned(), "118°".to_owned()));
    }

    #[test]
    fn the_facts_in_order_each_only_when_known() {
        let s = ship();
        let d = details();
        let labels = |v: &View<'_>| v.facts().into_iter().map(|f| (f.label, f.value)).collect::<Vec<_>>();
        assert_eq!(
            labels(&view(&s, Some(&d), None)),
            [
                ("IMO", "9235517".to_owned()),
                ("MMSI", "265177000".to_owned()),
                ("Flag", "Sweden".to_owned()),
                ("Length × beam", "184 × 30 m".to_owned()),
                ("Draught / max", "6.3 / 6.6 m".to_owned()),
                ("Built", "1983".to_owned()),
                ("Gross tonnage", "29,289".to_owned()),
                ("Deadweight", "5,500 t".to_owned()),
                ("Position", "57.7072° N, 11.9441° E".to_owned()),
            ]
        );
        let facts = view(&s, Some(&d), None).facts();
        assert_eq!(facts[2].tooltip.as_deref(), Some("SE"));
        assert!(!facts[2].data && facts[0].data && facts[8].wide);
        // a sparse card: the draught alone is the maximum's
        let sparse = ShipDetails { imo: None, beam_m: None, draught_m: None, year_built: Some(0), gross_tonnage: None, flag: None, ..details() };
        assert_eq!(
            labels(&view(&s, Some(&sparse), None)),
            [
                ("MMSI", "265177000".to_owned()),
                ("Length × beam", "184 m".to_owned()),
                ("Draught", "max 6.6 m".to_owned()),
                ("Deadweight", "5,500 t".to_owned()),
                ("Position", "57.7072° N, 11.9441° E".to_owned()),
            ]
        );
        // after a failure: the MMSI and the position
        assert_eq!(view(&s, None, Some("503")).facts().len(), 2);
    }

    #[test]
    fn the_voyage_the_age_and_the_notes() {
        let s = ship();
        let d = details();
        let v = view(&s, Some(&d), None);
        assert_eq!((v.destination(), v.eta(), v.eta_passed()), (Some("DKFDH"), Some(1_782_048_300), false));
        assert_eq!(v.clocks.clock_with_zone(1_782_048_300i64, Some("UTC")), "13:25 UTC");
        assert_eq!(v.clocks.day_at(1_782_048_300.0, Some("UTC"), NOW as i64), "Today");
        assert_eq!(v.age().map(|a| (a.stale, a.label)), Some((false, "Position 1 min ago".to_owned())));
        assert_eq!(v.reported_at(), "Reported at 09:58 UTC");
        assert_eq!(v.crew_note().as_deref(), Some("Status, destination and ETA are entered by the crew."));
        let old = ShipDetails { destination: Some(String::new()), eta_unix: Some(1_781_990_000), nav_status: None, position_unix: Some(1_782_033_180), ..details() };
        let v = view(&s, Some(&old), None);
        assert_eq!((v.destination(), v.eta_passed()), (None, true));
        assert_eq!(v.age().map(|a| (a.stale, a.label)), Some((true, "Position 47 min old".to_owned())));
        assert_eq!(v.crew_note().as_deref(), Some("ETA is entered by the crew."));
        let none = ShipDetails { destination: None, eta_unix: Some(0), nav_status: None, position_unix: Some(0), ..details() };
        let v = view(&s, Some(&none), None);
        assert_eq!((v.eta(), v.age(), v.crew_note()), (None, None, None));
    }

    #[test]
    fn styles_as_today_s_css() {
        assert_eq!(title_style(false).size, 22.0);
        assert!((title_style(true).line_px() - 22.1).abs() < 1e-4);
        assert!((destination_style(true).line_px() - 24.2).abs() < 1e-4);
        assert_eq!(destination_style(false).line_px(), 21.0);
        assert_eq!((destination_style(false).weight, destination_style(false).face), (700, Face::Mono));
        assert_eq!(eta_style(true).color, theme::TEXT_3);
        assert_eq!(js_length("KATTEGAT EXPRESS"), 16);
        assert_eq!(wrap(&[Vec2::new(56.0, 18.5), Vec2::new(146.0, 18.5)], 328.0), (vec![Vec2::ZERO, Vec2::new(64.0, 0.0)], 18.5));
        assert_eq!(wrap(&[Vec2::new(200.0, 18.5), Vec2::new(146.0, 18.5)], 328.0), (vec![Vec2::ZERO, Vec2::new(0.0, 26.5)], 45.0));
    }
}
