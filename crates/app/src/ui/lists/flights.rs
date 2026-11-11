//! TopFlights, the aircraft list (doc 06 §14; `TopFlights.svelte`): the most tracked flights in the
//! sidebar's list card while aircraft are the focus ([`super::super::shell::Slots::aircraft_list`]).
//!
//! - **Header** (`header`: a baseline row, space between, padding `0 8px 8px`): `Most tracked`
//!   (14 px at 650) on the left; on the right the hint, the `Eye` 13/1.75 and `Watching` 4 px
//!   apart (12 px `--text-3`, centred on its 16.8 px line; tooltip `People watching each flight on
//!   Flightradar24 right now`). The hint's baseline is its eye's bottom (an inline-flex box's
//!   first item, a replaced element: 14.9 px down), Overpass's at 14 px is 13.01 px down its line
//!   box, so the title's line box starts [`TITLE_DROP`] below the hint's (Chromium's layout of
//!   today's markup): the header is 29.5 px high.
//! - **Failed with nothing to show** (`failed && !flights?.length`): `Couldn't load the most
//!   tracked flights.` (13 px `--text-2`) and 12 px under it the `.button` `Try again` with
//!   `RefreshCw`, padding 8 round them: [`Action::RetryTop`].
//! - **Loading** (`flights === null`): six placeholder rows, each 40 px (padding 8, a 2 px row
//!   gap): a skeleton `58 + (i·17 mod 30)` × 12 on the left and 34 × 12 on the right, a
//!   `120 + (i·29 mod 70)` × 10 one under them ([`skeleton_widths`]).
//! - **Rows** (buttons: a `1fr auto` grid, gaps `2px 8px`, padding 8, radius 4, 53 px high):
//!   - the callsign (`callsign || flight_number || "No callsign"`, Overpass Mono 13 px at 650,
//!     `--amber` on the selected flight) and the watchers (`live_clicks` grouped only from five
//!     digits, Mono 13 px `--text-2`, right-aligned) on the first line;
//!   - the second line (12 px `--text-2`, 4 px apart, centred): the origin's and destination's
//!     IATA codes in Mono (`?` for one not known) with an `ArrowRight` 11/2 in `--text-3`
//!     between, or `No route filed` when neither is known; then 4 px further the aircraft type
//!     (`full_description || type`, `--text-3`, cut with an ellipsis);
//!   - `--surface-hover` under the pointer (150 ms), the selected flight on `--amber-soft`
//!     (under the pointer too: the later rule);
//!   - a click selects the flight and flies to it ([`Action::PickFlight`]: `select(flight_id,
//!     {fly: true})`).
//!
//! The list's refresh (on becoming the focus and every 60 s) is `state::aircraft`'s; an empty
//! answer shows the header alone, as today.
//!
//! Deviations: the rows' digits are Overpass Mono's own (no `tabular-nums` in egui; Mono's are
//! tabular anyway); text takes egui's baseline, up to a pixel lower than Chromium's (U1).

use egui::text::LayoutJob;
use egui::{Align2, Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Sense, Ui, Vec2};
use rustradar_core::model::TopFlight;
use rustradar_core::text::group_thousands;

use crate::state::{Action, AppState};
use crate::ui::icons::{self, Icon, STROKE_REGULAR, STROKE_THIN};
use crate::ui::shell::Pass;
use crate::ui::text::{self, Style};
use crate::ui::widgets::button::{self, Button, focus_ring, transition_color};
use crate::ui::widgets::{skeleton, tooltip};
use crate::ui::{anim, theme};

/// The header's title.
pub const TITLE: &str = "Most tracked";
/// The hint on the header's right, and its tooltip.
pub const HINT: &str = "Watching";
pub const HINT_TOOLTIP: &str = "People watching each flight on Flightradar24 right now";
/// What the list says when the most tracked flights did not load.
pub const FAILED: &str = "Couldn't load the most tracked flights.";
/// The retry button's text.
pub const RETRY: &str = "Try again";
/// A row's callsign when the flight has neither a callsign nor a flight number.
pub const NO_CALLSIGN: &str = "No callsign";
/// A row's second line when neither airport is known.
pub const NO_ROUTE: &str = "No route filed";

/// The header's padding below (`padding: 0 var(--space-2) var(--space-2)`) and at the sides.
pub const HEADER_PAD: f32 = theme::SPACE_2;
/// The hint's eye (`<Eye size={13} strokeWidth={1.75} />`).
pub const EYE: f32 = 13.0;
/// How far the title's line box starts below the hint's (see the module documentation).
pub const TITLE_DROP: f32 = 1.89;
/// A row's padding (`padding: var(--space-2)`).
pub const ROW_PAD: f32 = theme::SPACE_2;
/// The rows' grid gaps: across, and between the two lines (`gap: 2px var(--space-2)`).
pub const ROW_GAP: Vec2 = Vec2::new(theme::SPACE_2, 2.0);
/// Between the parts of a row's second line (`gap: var(--space-1)`), and the type's own margin.
pub const SUB_GAP: f32 = theme::SPACE_1;
/// The arrow between the airports (`<ArrowRight size={11} strokeWidth={2} />`).
pub const ARROW: f32 = 11.0;
/// How many placeholder rows show while loading.
pub const SKELETON_ROWS: usize = 6;
/// The placeholders' heights: the first line's two and the second line's.
pub const SKELETON_HEIGHT: f32 = 12.0;
pub const SKELETON_WIDE_HEIGHT: f32 = 10.0;
/// The first line's right-hand placeholder width.
pub const SKELETON_RIGHT: f32 = 34.0;
/// The failure message's padding and the gap under its text (`.message`).
pub const MESSAGE_PAD: f32 = theme::SPACE_2;
pub const MESSAGE_GAP: f32 = theme::SPACE_3;

/// The title (`h2`: 14 px at 650).
pub const fn title_style() -> Style {
    Style::ui(theme::TEXT_BASE, theme::WEIGHT_BADGE, theme::TEXT_1)
}

/// The hint (`.hint`: 12 px `--text-3`).
pub const fn hint_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A row's callsign (`.callsign.data`: Mono 13 px at 650), `--amber` when selected.
pub const fn callsign_style(selected: bool) -> Style {
    Style::data(theme::TEXT_MD, theme::WEIGHT_BADGE, if selected { theme::AMBER } else { theme::TEXT_1 })
}

/// A row's watchers (`.watching.data`: Mono 13 px `--text-2`).
pub const fn watching_style() -> Style {
    Style::data(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The second line's airport codes (`.data` in `.sub`: Mono 12 px `--text-2`).
pub const fn code_style() -> Style {
    Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The second line's words (`.sub`: 12 px `--text-2`).
pub const fn sub_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The aircraft type (`.type`: `--text-3`).
pub const fn type_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The failure message (`.message`: 13 px `--text-2`).
pub const fn message_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// A count of watchers as the list writes it (`Intl.NumberFormat(undefined, {useGrouping:
/// "min2"})`): digits grouped only from five up (`1443`, `12,345`).
pub fn watchers_text(n: u32) -> String {
    if n >= 10_000 { group_thousands(f64::from(n)) } else { n.to_string() }
}

/// A row's callsign: `callsign || flight_number || "No callsign"`.
pub fn callsign_text(f: &TopFlight) -> &str {
    if !f.callsign.is_empty() {
        &f.callsign
    } else if !f.flight_number.is_empty() {
        &f.flight_number
    } else {
        NO_CALLSIGN
    }
}

/// A row's route: the two IATA codes (`?` for one not known), or `None` for `No route filed`.
pub fn route(f: &TopFlight) -> Option<(&str, &str)> {
    fn code(c: &str) -> &str {
        if c.is_empty() { "?" } else { c }
    }
    (!f.from_iata.is_empty() || !f.to_iata.is_empty()).then(|| (code(&f.from_iata), code(&f.to_iata)))
}

/// A row's aircraft type: `full_description || type`.
pub fn type_text(f: &TopFlight) -> &str {
    if f.full_description.is_empty() { &f.typecode } else { &f.full_description }
}

/// Placeholder row `i`'s widths: the first line's left skeleton and the second line's.
pub fn skeleton_widths(i: usize) -> (f32, f32) {
    (58.0 + ((i * 17) % 30) as f32, 120.0 + ((i * 29) % 70) as f32)
}

/// What the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// The last ask failed and there is nothing to show: the message and Try again.
    Failed,
    /// The first answer has not come: the placeholder rows.
    Loading,
    /// The rows (none for an empty answer).
    Rows,
}

/// What the list shows for `state` (`{#if failed && !flights?.length} … {:else if flights ===
/// null} … {:else}`).
pub fn body(state: &AppState) -> Body {
    let top = state.aircraft.top();
    if state.aircraft.top_failed() && top.is_none_or(<[TopFlight]>::is_empty) {
        Body::Failed
    } else if top.is_none() {
        Body::Loading
    } else {
        Body::Rows
    }
}

/// A row's height: two lines, the gap and the padding.
pub fn row_height() -> f32 {
    2.0 * ROW_PAD + callsign_style(false).line_px() + ROW_GAP.y + sub_style().line_px()
}

/// A placeholder row's height.
pub const fn skeleton_row_height() -> f32 {
    2.0 * ROW_PAD + SKELETON_HEIGHT + ROW_GAP.y + SKELETON_WIDE_HEIGHT
}

/// The header's height: the title's line box below the drop, and the padding under it.
pub fn header_height() -> f32 {
    (TITLE_DROP + title_style().line_px()).max(hint_style().line_px()) + HEADER_PAD
}

/// The list in the list card (the shell's [`Slot`](super::super::shell::Slot)).
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) {
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    header(ui);
    match body(pass.state) {
        Body::Failed => failed(ui, pass),
        Body::Loading => loading(ui),
        Body::Rows => {
            let selected = pass.state.flight.selected_id();
            for f in pass.state.aircraft.top().unwrap_or_default() {
                if row(ui, f, selected == Some(f.flight_id)) {
                    pass.actions.push(Action::PickFlight(f.flight_id));
                }
            }
        }
    }
}

/// The header: the title, and the hint on the right with its tooltip.
fn header(ui: &mut Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, header_height()), Sense::hover());
    let painter = ui.painter();
    let title = painter.layout_job(text::job(TITLE, &title_style()));
    painter.galley(Pos2::new(rect.left() + HEADER_PAD, rect.top() + TITLE_DROP), title, Color32::PLACEHOLDER);
    // the hint: the eye and the word, centred on the hint's line
    let style = hint_style();
    let word = painter.layout_job(text::job(HINT, &style));
    let hint = Rect::from_min_size(Pos2::new(rect.right() - HEADER_PAD - word.size().x - SUB_GAP - EYE, rect.top()), Vec2::new(EYE + SUB_GAP + word.size().x, style.line_px()));
    let eye = Rect::from_center_size(Pos2::new(hint.left() + EYE / 2.0, hint.center().y), Vec2::splat(EYE));
    icons::paint(painter, eye, Icon::Eye.at(EYE, STROKE_THIN), theme::TEXT_3);
    painter.galley(Pos2::new(eye.right() + SUB_GAP, hint.top()), word, Color32::PLACEHOLDER);
    tooltip::show(ui.interact(hint, Id::unique("rustradar::top_flights::hint"), Sense::hover()), HINT_TOOLTIP);
}

/// The failure message and Try again.
fn failed(ui: &mut Ui, pass: &mut Pass<'_>) {
    let width = ui.available_width();
    ui.add_space(MESSAGE_PAD);
    ui.horizontal(|ui| {
        ui.add_space(MESSAGE_PAD);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            text::label(ui, text::wrapped(FAILED, &message_style(), width - 2.0 * MESSAGE_PAD), Sense::hover());
            ui.add_space(MESSAGE_GAP);
            if Button::new().icon(button::icon(Icon::RefreshCw)).text(RETRY).show(ui).clicked() {
                pass.actions.push(Action::RetryTop);
            }
        });
    });
    ui.add_space(MESSAGE_PAD);
}

/// The six placeholder rows.
fn loading(ui: &mut Ui) {
    let width = ui.available_width();
    for i in 0..SKELETON_ROWS {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, skeleton_row_height()), Sense::hover());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let (left, wide) = skeleton_widths(i);
        let top = rect.top() + ROW_PAD;
        let x = rect.left() + ROW_PAD;
        let painter = ui.painter();
        skeleton::paint(painter, Rect::from_min_size(Pos2::new(x, top), Vec2::new(left, SKELETON_HEIGHT)), theme::RADIUS);
        skeleton::paint(painter, Rect::from_min_size(Pos2::new(rect.right() - ROW_PAD - SKELETON_RIGHT, top), Vec2::new(SKELETON_RIGHT, SKELETON_HEIGHT)), theme::RADIUS);
        let second = top + SKELETON_HEIGHT + ROW_GAP.y;
        skeleton::paint(painter, Rect::from_min_size(Pos2::new(x, second), Vec2::new(wide, SKELETON_WIDE_HEIGHT)), theme::RADIUS);
    }
}

/// One flight's row; answers whether it was clicked.
fn row(ui: &mut Ui, f: &TopFlight, selected: bool) -> bool {
    let width = ui.available_width();
    let id = Id::unique(("rustradar::top_flights::row", f.flight_id));
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, row_height()), Sense::hover());
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return response.clicked();
    }
    // `button.row.active` comes after `button.row:hover`: the selection wins under the pointer
    let fill = if selected {
        theme::AMBER_SOFT
    } else if response.hovered() {
        theme::SURFACE_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let fill = transition_color(ui.ctx(), id.with("fill"), fill, anim::HOVER);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), fill);
    let content = rect.shrink(ROW_PAD);

    // the first line: the callsign, the watchers on the right
    let watching = painter.layout_job(text::job(&watchers_text(f.live_clicks), &watching_style()));
    let watching_rect = Align2::RIGHT_TOP.anchor_size(content.right_top(), watching.size());
    let callsign_width = (watching_rect.left() - ROW_GAP.x - content.left()).max(0.0);
    let callsign = painter.layout_job(text::truncated(callsign_text(f), &callsign_style(selected), callsign_width));
    painter.galley(content.left_top(), callsign, Color32::PLACEHOLDER);
    painter.galley(watching_rect.min, watching, Color32::PLACEHOLDER);

    // the second line: the route or its absence, then the type
    let line = sub_style().line_px();
    let top = content.top() + callsign_style(false).line_px() + ROW_GAP.y;
    let middle = top + line / 2.0;
    let mut x = content.left();
    let put = |painter: &egui::Painter, job: LayoutJob, x: &mut f32| {
        let galley = painter.layout_job(job);
        let w = galley.size().x;
        painter.galley(Pos2::new(*x, middle - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
        *x += w + SUB_GAP;
    };
    match route(f) {
        Some((from, to)) => {
            put(painter, text::job(from, &code_style()), &mut x);
            let arrow = Rect::from_center_size(Pos2::new(x + ARROW / 2.0, middle), Vec2::splat(ARROW));
            icons::paint(painter, arrow, Icon::ArrowRight.at(ARROW, STROKE_REGULAR), theme::TEXT_3);
            x += ARROW + SUB_GAP;
            put(painter, text::job(to, &code_style()), &mut x);
        }
        None => put(painter, text::job(NO_ROUTE, &sub_style()), &mut x),
    }
    let kind = type_text(f);
    if !kind.is_empty() {
        x += SUB_GAP;
        let room = (content.right() - x).max(0.0);
        put(painter, text::truncated(kind, &type_style(), room), &mut x);
    }
    if response.has_focus() {
        focus_ring(painter, rect, theme::RADIUS_SM);
    }
    response.clicked()
}

#[cfg(test)]
mod tests {
    use egui::{Context, Event, PointerButton, RawInput};
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::persist::tests::TempDir;
    use crate::state::{Startup, aircraft};
    use crate::ui::shell::{self, Slots};

    const T0: f64 = 1_782_036_000_000.0;

    fn top(id: u32, callsign: &str, flight_number: &str, from: &str, to: &str) -> TopFlight {
        TopFlight {
            flight_id: id,
            live_clicks: 1443,
            callsign: callsign.into(),
            flight_number: flight_number.into(),
            from_iata: from.into(),
            to_iata: to.into(),
            typecode: "A20N".into(),
            full_description: "Airbus A320neo".into(),
            ..Default::default()
        }
    }

    /// A state reading its feeds from `dir` (none of its files: every ask fails), polled once.
    fn state(dir: &TempDir) -> AppState {
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        s.poll(T0);
        s
    }

    fn write_top(dir: &TempDir, flights: &[TopFlight]) {
        std::fs::write(dir.0.join(aircraft::FIXTURE_TOP), serde_json::to_vec(flights).unwrap()).unwrap();
    }

    /// One pass of the shell with the app's slots over `events`: the actions.
    fn pass(ctx: &Context, state: &AppState, time: f64, events: Vec<Event>) -> Vec<Action> {
        let mut actions = Vec::new();
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            shell::show(ui.ctx(), state, T0, &mut actions, &Slots::APP);
        });
        output.textures_delta.clear();
        actions
    }

    /// A click at `at`: the pointer there, pressed, then released.
    fn click(ctx: &Context, state: &AppState, at: Pos2, time: f64) -> Vec<Action> {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
        let mut actions = pass(ctx, state, time, vec![Event::PointerMoved(at)]);
        actions.extend(pass(ctx, state, time + 0.05, vec![button(true)]));
        actions.extend(pass(ctx, state, time + 0.1, vec![button(false)]));
        actions
    }

    #[test]
    fn watchers_are_grouped_from_five_digits() {
        assert_eq!(watchers_text(0), "0");
        assert_eq!(watchers_text(1443), "1443");
        assert_eq!(watchers_text(9999), "9999");
        assert_eq!(watchers_text(10_000), "10,000");
        assert_eq!(watchers_text(12_345), "12,345");
        assert_eq!(watchers_text(1_234_567), "1,234,567");
    }

    #[test]
    fn a_row_s_words() {
        let f = top(1, "SAS1415", "SK1415", "ARN", "CPH");
        assert_eq!(callsign_text(&f), "SAS1415");
        assert_eq!(callsign_text(&top(1, "", "SK1415", "", "")), "SK1415");
        assert_eq!(callsign_text(&top(1, "", "", "", "")), "No callsign");
        assert_eq!(route(&f), Some(("ARN", "CPH")));
        assert_eq!(route(&top(1, "", "", "JFK", "")), Some(("JFK", "?")));
        assert_eq!(route(&top(1, "", "", "", "HNL")), Some(("?", "HNL")));
        assert_eq!(route(&top(1, "", "", "", "")), None, "No route filed");
        assert_eq!(type_text(&f), "Airbus A320neo");
        assert_eq!(type_text(&TopFlight { full_description: String::new(), ..f }), "A20N");
    }

    #[test]
    fn the_placeholders_and_the_heights_are_today_s() {
        let widths: Vec<(f32, f32)> = (0..SKELETON_ROWS).map(skeleton_widths).collect();
        // Chromium's layout of today's markup
        assert_eq!(widths, [(58.0, 120.0), (75.0, 149.0), (62.0, 178.0), (79.0, 137.0), (66.0, 166.0), (83.0, 125.0)]);
        assert_eq!(skeleton_row_height(), 40.0);
        assert!((row_height() - 52.98).abs() < 0.05, "{}", row_height());
        assert!((header_height() - 29.48).abs() < 0.05, "{}", header_height());
        // the loading list in dark-v1: 269.5 px of content in the 295.5 px card
        assert!((header_height() + 6.0 * skeleton_row_height() - 269.48).abs() < 0.05);
    }

    #[test]
    fn loading_failed_and_rows() {
        let dir = TempDir::new("top-flights-body");
        let mut s = state(&dir);
        // no file: the first ask failed, nothing to show
        for t in [T0 + 16.0, T0 + 32.0] {
            s.poll(t);
        }
        assert!(s.aircraft.top_failed());
        assert_eq!(body(&s), Body::Failed);
        // Try again asks once more
        write_top(&dir, &[top(7, "SAS1415", "", "ARN", "CPH"), top(8, "", "BA283", "LHR", "LAX")]);
        assert!(s.apply(Action::RetryTop, T0 + 40.0));
        for t in [T0 + 48.0, T0 + 64.0] {
            s.poll(t);
        }
        assert_eq!(body(&s), Body::Rows);
        assert_eq!(s.aircraft.top().map(<[TopFlight]>::len), Some(2));
        // an empty answer after a failure shows the message again
        let empty = AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0]);
        assert_eq!(body(&empty), Body::Loading, "no feed: the placeholders, as the reference pictures show");
    }

    /// A click on a row asks to select the flight and fly there; Try again asks again.
    #[test]
    fn a_row_picks_its_flight_and_try_again_retries() {
        use rustradar_core::model::{LiveFlight, LiveSnapshot};
        let dir = TempDir::new("top-flights-pick");
        write_top(&dir, &[top(7, "SAS1415", "", "ARN", "CPH"), top(8, "", "BA283", "LHR", "LAX")]);
        let live = LiveFlight { id: 8, callsign: "BAW283".into(), lat: 51.47, lon: -0.45, alt: 30_000, ..Default::default() };
        let snapshot = LiveSnapshot { flights: vec![live], server_time_ms: T0 as u64, ..Default::default() };
        std::fs::write(dir.0.join(aircraft::FIXTURE_LIVE), serde_json::to_vec(&snapshot).unwrap()).unwrap();
        let mut s = state(&dir);
        for t in [T0 + 16.0, T0 + 32.0] {
            s.poll(t);
        }
        assert_eq!(body(&s), Body::Rows);
        let ctx = Context::default();
        crate::ui::theme::apply(&ctx);
        // areas are sized on their first pass, the sidebar's heights solved from the last one's
        for time in [0.0, 1.0, 2.0, 3.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let second = ctx.read_response(Id::unique(("rustradar::top_flights::row", 8u32))).expect("the second row").rect;
        assert!((second.width() - 302.0).abs() < 0.5, "the list card's content width: {second:?}");
        assert!((second.height() - row_height()).abs() < 0.01);
        let actions = click(&ctx, &s, second.center(), 4.0);
        assert_eq!(actions, [Action::PickFlight(8)]);
        assert!(s.apply(Action::PickFlight(8), T0 + 100.0));
        assert_eq!(s.flight.selected_id(), Some(8));
        // known in the feed: the camera flies there at once
        s.controller.update(T0 + 100.0 + 10_000.0);
        let view = s.controller.view();
        assert!((view.longitude + 0.45).abs() < 1e-3 && (view.latitude - 51.47).abs() < 1e-3, "{view:?}");
        // the row is drawn selected
        pass(&ctx, &s, 5.0, Vec::new());

        // failed: the message and its button
        let failing = TempDir::new("top-flights-retry");
        let mut f = state(&failing);
        for t in [T0 + 16.0, T0 + 32.0] {
            f.poll(t);
        }
        assert_eq!(body(&f), Body::Failed);
        let ctx = Context::default();
        crate::ui::theme::apply(&ctx);
        let mut actions = Vec::new();
        for time in [0.0, 1.0, 2.0] {
            actions = pass(&ctx, &f, time, Vec::new());
        }
        assert!(actions.is_empty());
        let card = ctx.memory(|m| m.area_rect(Id::unique(("rustradar::sidebar", "list")))).expect("the list card");
        // the button: under the message's line and 12 px, 8 in from the content's left
        let top = card.top() + theme::BORDER + theme::SPACE_3 + header_height() + MESSAGE_PAD + message_style().line_px() + MESSAGE_GAP;
        let at = Pos2::new(card.left() + theme::BORDER + theme::SPACE_2 + MESSAGE_PAD + 20.0, top + 16.0);
        let actions = click(&ctx, &f, at, 3.0);
        assert_eq!(actions, [Action::RetryTop]);
    }
}
