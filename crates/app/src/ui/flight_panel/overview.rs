//! The flight panel's Overview tab (item U10; doc 06 §7.1; `FlightPanel.svelte`'s
//! `tab === "overview"`).
//!
//! - **Facts** (3 columns): `Vertical speed` (the aircraft's own from adsb.lol with an `adsb.lol`
//!   tag and the tooltip `Reported by the aircraft, via adsb.lol` when Flightradar24 sends none,
//!   else Flightradar24's while signed in), `Track`, `Squawk` (adsb.lol's, or Flightradar24's
//!   four octal digits), `Phase` (the flight stage as a sentence), `Airspace` (two columns, only
//!   when known). Flightradar24's values dim to `--text-3` once the stream ended; a sign-in-only
//!   value with nothing to show is left out.
//! - **Sign-in note** (when some are left out): `LockKeyhole` 12/2, `signInNote`, and the inline
//!   `Sign in` (600, `--text-1`, underlined in `--line-control` 3 px under its baseline,
//!   `--amber` under the pointer): it opens the keys dialog ([`Action::Open`] `Keys`). 8 px under
//!   the facts, 12 px `--text-3`.
//! - **Altitude chart** with its export menu: item U12's ([`super::Parts::chart`]).
//! - **Reported by the aircraft** (with adsb.lol's measurements): a rule 8 px further down, 16 px
//!   over the header (`Reported by the aircraft`, 12/600 `--text-2`, and the `adsb.lol`
//!   `ExternalLink` 11/2 link, 11 px `--text-3`, `--text-1` under the pointer, opening adsb.lol,
//!   its baseline on the heading's); the readings always on show; the disclosure `More from the
//!   aircraft ({n})` (`ChevronRight` 14/2 turning 90° over 150 ms; 12/550 `--text-2`, `--text-1`
//!   under the pointer; open or closed for the whole session) with the other readings and `Not
//!   reported by this aircraft: …`; opened, the panel scrolls it clear of the pinned footer and
//!   12 px more; `Position by multilateration: …` for a multilaterated position. Notes 11 px
//!   `--text-3`; the parts 8 px apart.

use egui::text::LayoutJob;
use egui::{CornerRadius, CursorIcon, Id, Pos2, Rect, Response, Sense, Ui, Vec2};
use rustradar_core::adsb::vertical_rate;
use rustradar_core::model::ADSB_LOL_URL;
use rustradar_core::readings::{Reading, aircraft_readings, degrees, listing};
use rustradar_core::signin::{Have, SignInValue, missing_without_sign_in, sign_in_note};
use rustradar_core::text::group_thousands;

use super::super::fonts::Face;
use super::super::icons::{self, Icon};
use super::super::panel::{self, Fact, Fit, Piece, Span};
use super::super::shell::Pass;
use super::super::text::{self, Style};
use super::super::widgets::button::{focus_ring, transition_color};
use super::super::widgets::tooltip;
use super::super::{anim, theme};
use super::{DASH, Parts, TABPANEL_GAP, View};
use crate::state::Action;
use crate::state::shortcuts::Open;

/// The tag after a value the aircraft reported (`.via`), and its tooltip.
pub const VIA: &str = "adsb.lol";
pub const VIA_TOOLTIP: &str = "Reported by the aircraft, via adsb.lol";
/// The sign-in note's button.
pub const SIGN_IN: &str = "Sign in";
/// The measured section's heading, its link's tooltip, the disclosure, the notes.
pub const MEASURED: &str = "Reported by the aircraft";
pub const ADSB_TOOLTIP: &str = "Community ADS-B data from adsb.lol, under the Open Database Licence";
pub const MLAT_NOTE: &str = "Position by multilateration: less precise than the aircraft's own.";

/// The facts' and the note's gap (`.stack.overview { gap: var(--space-2) }`).
const STACK_GAP: f32 = theme::SPACE_2;
/// The note's gaps (`gap: var(--space-1) var(--space-2)`), the lock's pull (`margin-right: -4px`).
const NOTE_GAP: Vec2 = Vec2::new(theme::SPACE_2, theme::SPACE_1);
const LOCK: f32 = 12.0;
/// The inline button's padding (`padding: 2px 0`) and its underline's offset.
const ACTION_PAD: f32 = 2.0;
const UNDERLINE_OFFSET: f32 = 3.0;
/// The measured section: its margin above the rule, its padding under it, its gaps.
const MEASURED_MARGIN: f32 = theme::SPACE_2;
const MEASURED_PAD: f32 = theme::SPACE_4;
const MEASURED_GAP: f32 = theme::SPACE_2;
/// The link's baseline on the heading's: its 11 px line box 1 px under the 12 px one (Chromium's
/// layout of today's markup), its icon 3 px after the text.
const LINK_DROP: f32 = 1.0;
const LINK_GAP: f32 = 3.0;
const LINK_ICON: f32 = 11.0;
/// The disclosure: padding, chevron (pulled 3 px left), gap; the opened block's padding and gap.
const SUMMARY_PAD: f32 = theme::SPACE_1;
const CHEVRON: f32 = 14.0;
const CHEVRON_PULL: f32 = 3.0;
const SUMMARY_GAP: f32 = theme::SPACE_1;
const MORE_PAD: f32 = theme::SPACE_2;
const MORE_GAP: f32 = theme::SPACE_3;
/// How far past the pinned footer an opened disclosure scrolls.
const REVEAL_MARGIN: f32 = theme::SPACE_3;

/// `signed(v)`: `—` without a value, else `+1,234 fpm`, `-800 fpm`, `0 fpm`.
pub fn signed(v: Option<i32>) -> String {
    match v {
        None => DASH.to_owned(),
        Some(v) => format!("{}{} fpm", if v > 0 { "+" } else { "" }, group_thousands(f64::from(v))),
    }
}

/// `squawk(code)`: four octal digits (sent in base 10), `—` for 0.
pub fn squawk(code: i32) -> String {
    if code == 0 {
        return DASH.to_owned();
    }
    let digits = if code < 0 { format!("-{:o}", code.unsigned_abs()) } else { format!("{code:o}") };
    format!("{digits:0>4}")
}

/// `sentence(s)`: the first character kept, the rest lower case with `_` as spaces.
pub fn sentence(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => format!("{first}{}", chars.as_str().to_lowercase().replace('_', " ")),
        None => String::new(),
    }
}

/// The values the overview reads (the component's deriveds).
pub struct Values {
    /// Flightradar24's vertical speed, when signed in (`fr24Vs`).
    pub fr24_vs: Option<i32>,
    /// The transponder's (`measuredVs`).
    pub measured_vs: Option<i32>,
    /// Flightradar24's squawk when signed in, else 0 (`fr24Squawk`).
    pub fr24_squawk: i32,
    /// The sign-in values with nothing to show (`gated`).
    pub gated: Vec<SignInValue>,
}

impl Values {
    pub fn of(view: &View<'_>) -> Values {
        let info = view.info();
        let fr24_vs = info.filter(|_| view.authenticated).map(|i| i.vertical_speed);
        let measured_vs = view.measured.and_then(vertical_rate);
        let fr24_squawk = info.filter(|_| view.authenticated).map_or(0, |i| i.squawk);
        let have = Have {
            vertical_speed: measured_vs.is_some(),
            squawk: info.is_some_and(|i| i.squawk != 0) || view.measured.is_some_and(|m| !m.squawk.is_empty()),
            airspace: info.is_some_and(|i| !i.airspace.is_empty()),
        };
        Values { fr24_vs, measured_vs, fr24_squawk, gated: missing_without_sign_in(view.authenticated, have) }
    }
}

/// A value in the data face, `--text-3` when `dim`.
fn data(text: &str, dim: bool) -> Piece<'static> {
    Piece::text(text, &panel::dd_style(Face::Mono, dim), Fit::Wrap)
}

/// A value the aircraft reported: the value in the data face and the `adsb.lol` tag.
fn via(text: &str) -> Piece<'static> {
    let style = panel::dd_style(Face::Mono, false);
    let mut job = LayoutJob::default();
    text::append(&mut job, &format!("{text} "), &style);
    text::append(&mut job, VIA, &Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3));
    Piece::job(job, style.line_px(), Fit::Wrap)
}

/// The facts of the overview, in order.
pub fn facts(view: &View<'_>, values: &Values) -> Vec<Fact<'static>> {
    let ended = view.ended();
    let info = view.info();
    let mut items = Vec::new();
    if values.fr24_vs.is_none()
        && let Some(vs) = values.measured_vs
    {
        items.push(Fact::new("Vertical speed", via(&signed(Some(vs)))).tooltip(Some(VIA_TOOLTIP)));
    } else if !values.gated.contains(&SignInValue::VerticalSpeed) {
        items.push(Fact::new("Vertical speed", data(&signed(values.fr24_vs), ended)));
    }
    let track = info.map_or(view.flight.track, |i| i.track);
    items.push(Fact::new("Track", data(&degrees(f64::from(track)), ended)));
    let measured_squawk = view.measured.map_or("", |m| m.squawk.as_str());
    if values.fr24_squawk == 0 && !measured_squawk.is_empty() {
        items.push(Fact::new("Squawk", via(measured_squawk)).tooltip(Some(VIA_TOOLTIP)));
    } else if !values.gated.contains(&SignInValue::Squawk) {
        items.push(Fact::new("Squawk", data(&squawk(info.map_or(0, |i| i.squawk)), ended)));
    }
    let phase = view.progress().map_or_else(|| DASH.to_owned(), |p| sentence(&p.flight_stage));
    items.push(Fact::new("Phase", Piece::text(&phase, &panel::dd_style(Face::Ui, ended), Fit::Wrap)));
    if let Some(airspace) = info.map(|i| i.airspace.as_str()).filter(|a| !a.is_empty()) {
        items.push(Fact::new("Airspace", Piece::text(airspace, &panel::dd_style(Face::Ui, ended), Fit::Wrap)).span(Span::Two));
    }
    items
}

/// A reading as a fact: the value (`—` when not reported) in the data face unless it is words.
fn reading(r: &Reading) -> Fact<'static> {
    let value = r.value.as_deref().unwrap_or(DASH);
    let face = if r.words { Face::Ui } else { Face::Mono };
    Fact::new(r.label, Piece::text(value, &panel::dd_style(face, false), Fit::Wrap)).wide(r.wide).tooltip(r.title)
}

/// Whether "More from the aircraft" is open: one value for the session, from flight to flight.
fn more_id() -> Id {
    Id::unique("rustradar::flight_panel::more")
}

/// "More from the aircraft" opened in a pass: the next scrolls it clear of the footer.
fn reveal_id() -> Id {
    Id::unique("rustradar::flight_panel::reveal")
}

/// Whether "More from the aircraft" is open.
pub fn more_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(more_id())).unwrap_or(false)
}

/// The tab's content in `ui` (the tab panel's inner width).
pub fn show(ui: &mut Ui, id: Id, view: &View<'_>, pass: &mut Pass<'_>, parts: &Parts) {
    let values = Values::of(view);
    panel::facts(ui, 3, panel::FACTS_GAP, facts(view, &values));
    if !values.gated.is_empty() {
        ui.add_space(STACK_GAP);
        let note = gated_note(ui, id, &values.gated);
        if note.clicked() {
            // `promptSignIn`: the email field takes the keyboard, the note gets it back (U17)
            crate::ui::keys_dialog::prompt_sign_in(ui.ctx(), Some(note.id));
            pass.actions.push(Action::Open(Open::Keys, true));
        }
    }
    if let Some(chart) = parts.chart {
        ui.add_space(TABPANEL_GAP);
        chart(ui, view, pass);
    }
    if let Some(measured) = view.measured {
        ui.add_space(TABPANEL_GAP + MEASURED_MARGIN);
        measured_section(ui, id, measured);
    }
}

/// The sign-in note; answers the `Sign in` button's response.
fn gated_note(ui: &mut Ui, id: Id, gated: &[SignInValue]) -> Response {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let style = Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3);
    let note = ctx.fonts_mut(|f| f.layout_job(panel::fit(text::job(&sign_in_note(gated), &style), Fit::Wrap, width - (LOCK - theme::SPACE_1) - NOTE_GAP.x)));
    let action_style = Style::ui(theme::TEXT_SM, 600, theme::TEXT_1);
    let action = ctx.fonts_mut(|f| f.layout_job(text::job(SIGN_IN, &action_style)));
    let line = style.line_px();
    let note_h = line * note.rows.len().max(1) as f32;
    let action_size = Vec2::new(action.size().x, action_style.line_px() + 2.0 * ACTION_PAD);
    // flex-wrap: the lock and the text, then the button on the same row while it fits
    let first = LOCK - theme::SPACE_1 + NOTE_GAP.x + note.size().x;
    let one_row = first + NOTE_GAP.x + action_size.x <= width;
    let row = note_h.max(LOCK);
    let height = if one_row { row.max(action_size.y) } else { row + NOTE_GAP.y + action_size.y };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter().clone();
    let mid = rect.top() + if one_row { height / 2.0 } else { row / 2.0 };
    icons::paint(&painter, Rect::from_center_size(Pos2::new(rect.left() + LOCK / 2.0, mid), Vec2::splat(LOCK)), Icon::LockKeyhole.at(LOCK, icons::STROKE_REGULAR), theme::TEXT_3);
    let note_x = rect.left() + LOCK - theme::SPACE_1 + NOTE_GAP.x;
    painter.galley(Pos2::new(note_x, mid - note_h / 2.0 + (line - note.size().y / note.rows.len().max(1) as f32) / 2.0), note, theme::TEXT_3);
    let action_rect = if one_row {
        Rect::from_min_size(Pos2::new(rect.left() + first + NOTE_GAP.x, mid - action_size.y / 2.0), action_size)
    } else {
        Rect::from_min_size(Pos2::new(rect.left(), rect.top() + row + NOTE_GAP.y), action_size)
    };
    inline_action(ui, id.with("sign-in"), action_rect, action, &action_style)
}

/// `.inline-action`: the text with its underline (`--line-control`, `--amber` under the pointer
/// over 150 ms).
fn inline_action(ui: &mut Ui, id: Id, rect: Rect, galley: std::sync::Arc<egui::Galley>, style: &Style) -> Response {
    let ctx = ui.ctx().clone();
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, galley.text()));
    let painter = ui.painter();
    let text_top = rect.top() + ACTION_PAD + (style.line_px() - galley.size().y) / 2.0;
    let baseline = text_top + text::row_baseline(&galley, 0);
    let underline = transition_color(&ctx, response.id.with("underline"), if response.hovered() { theme::AMBER } else { theme::LINE_CONTROL }, anim::HOVER);
    let line = Rect::from_x_y_ranges(rect.x_range(), baseline + UNDERLINE_OFFSET..=baseline + UNDERLINE_OFFSET + 1.0);
    painter.rect_filled(panel::snap_rect(line, ctx.pixels_per_point()), CornerRadius::ZERO, underline);
    painter.galley(Pos2::new(rect.left(), text_top), galley, style.color);
    if response.has_focus() {
        focus_ring(painter, rect, 0);
    }
    response
}

/// "Reported by the aircraft".
fn measured_section(ui: &mut Ui, id: Id, measured: &rustradar_core::model::AdsbAircraft) {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let readings = aircraft_readings(measured);
    let (left, top) = (ui.cursor().left(), ui.cursor().top());
    panel::hrule(ui.painter(), egui::Rangef::new(left, left + width), top);
    ui.add_space(theme::BORDER + MEASURED_PAD);

    // the header: the heading and the link on one baseline
    let heading_style = Style::ui(theme::TEXT_SM, 600, theme::TEXT_2);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, heading_style.line_px()), Sense::hover());
    let painter = ui.painter().clone();
    let heading = painter.layout_job(text::job(MEASURED, &heading_style));
    painter.galley(Pos2::new(rect.left(), rect.top() + (heading_style.line_px() - heading.size().y) / 2.0), heading, theme::TEXT_2);
    let link_style = Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3);
    let link_text = painter.layout_job(text::job(VIA, &link_style));
    let link_size = Vec2::new(link_text.size().x + LINK_GAP + LINK_ICON, link_style.line_px());
    let link = Rect::from_min_size(Pos2::new(rect.right() - link_size.x, rect.top() + LINK_DROP), link_size);
    let response = ui.interact(link, id.with("adsb-link"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, VIA));
    let color = if response.hovered() { theme::TEXT_1 } else { theme::TEXT_3 };
    painter.galley_with_override_text_color(Pos2::new(link.left(), link.top() + (link_style.line_px() - link_text.size().y) / 2.0), link_text, color);
    icons::paint(&painter, Rect::from_center_size(Pos2::new(link.right() - LINK_ICON / 2.0, link.center().y), Vec2::splat(LINK_ICON)), Icon::ExternalLink.at(LINK_ICON, icons::STROKE_REGULAR), color);
    if response.has_focus() {
        focus_ring(&painter, link, 0);
    }
    if tooltip::show(response, ADSB_TOOLTIP).clicked() {
        ctx.open_url(egui::OpenUrl::new_tab(ADSB_LOL_URL));
    }

    ui.add_space(MEASURED_GAP);
    panel::facts(ui, 3, panel::FACTS_GAP, readings.first.iter().map(reading).collect());

    if !readings.more.is_empty() {
        ui.add_space(MEASURED_GAP);
        let mut open = more_open(&ctx);
        let summary_style = Style::ui(theme::TEXT_SM, 550, theme::TEXT_2);
        let label = format!("More from the aircraft ({})", readings.more.len());
        let galley = painter.layout_job(text::job(&label, &summary_style));
        let row = summary_style.line_px().max(CHEVRON);
        let summary_w = CHEVRON - CHEVRON_PULL + SUMMARY_GAP + galley.size().x;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, row + 2.0 * SUMMARY_PAD), Sense::hover());
        let summary = Rect::from_min_size(rect.min, Vec2::new(summary_w, rect.height()));
        let response = ui.interact(summary, id.with("more"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, &label));
        if response.clicked() {
            open = !open;
            ctx.data_mut(|d| {
                d.insert_temp(more_id(), open);
                d.insert_temp(reveal_id(), open);
            });
        }
        let color = if response.hovered() { theme::TEXT_1 } else { theme::TEXT_2 };
        let mid = rect.center().y;
        let turn = anim::toggle(&ctx, id.with("chevron"), open, anim::CHEVRON);
        let chevron = Rect::from_center_size(Pos2::new(rect.left() - CHEVRON_PULL + CHEVRON / 2.0, mid), Vec2::splat(CHEVRON));
        icons::paint(&painter, chevron, Icon::ChevronRight.at(CHEVRON, icons::STROKE_REGULAR).rotated(turn * std::f32::consts::FRAC_PI_2), color);
        painter.galley_with_override_text_color(Pos2::new(rect.left() - CHEVRON_PULL + CHEVRON + SUMMARY_GAP, mid - galley.size().y / 2.0), galley, color);
        if response.has_focus() {
            focus_ring(&painter, summary, 0);
        }
        if open {
            ui.add_space(MORE_PAD);
            panel::facts(ui, 3, panel::FACTS_GAP, readings.more.iter().map(reading).collect());
            if !readings.unreported.is_empty() {
                ui.add_space(MORE_GAP);
                note(ui, &format!("Not reported by this aircraft: {}.", listing(&readings.unreported)));
            }
            if ctx.data_mut(|d| d.remove_temp::<bool>(reveal_id())).unwrap_or(false) {
                // the opened block clear of the pinned footer, and 12 px more (`scrollBy(hidden +
                // 12)`, only when some of it is hidden)
                let block = Rect::from_x_y_ranges(rect.x_range(), rect.top()..=ui.cursor().top() + panel::FOOTER_HEIGHT + REVEAL_MARGIN);
                ui.scroll_to_rect(block, None);
            }
        }
    }
    if measured.mlat {
        ui.add_space(MEASURED_GAP);
        note(ui, MLAT_NOTE);
    }
}

/// `.note`: 11 px `--text-3`, wrapping.
fn note(ui: &mut Ui, text: &str) {
    let width = ui.available_width();
    let style = panel::note_style();
    let laid = Piece::text(text, &style, Fit::Wrap).lay_out(ui.ctx(), width);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, laid.size.y), Sense::hover());
    laid.paint(ui.painter(), rect.min);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_written_as_today() {
        assert_eq!(signed(None), "—");
        assert_eq!(signed(Some(0)), "0 fpm");
        assert_eq!(signed(Some(-64)), "-64 fpm");
        assert_eq!(signed(Some(1234)), "+1,234 fpm");
        assert_eq!(signed(Some(-2500)), "-2,500 fpm");
        assert_eq!(squawk(0), "—");
        assert_eq!(squawk(0o7700), "7700");
        assert_eq!(squawk(0o1200), "1200");
        assert_eq!(squawk(0o23), "0023");
        assert_eq!(squawk(0o2312), "2312");
        assert_eq!(sentence("AIRBORNE"), "Airborne");
        assert_eq!(sentence("ON_GROUND"), "On ground");
        assert_eq!(sentence("_X_Y"), "_x y", "the first character is kept as it is");
        assert_eq!(sentence(""), "");
    }
}
