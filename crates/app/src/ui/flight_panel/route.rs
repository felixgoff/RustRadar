//! The flight panel's Route tab (item U11; doc 06 §7.2; `FlightPanel.svelte`'s
//! `tab === "route"`), drawn in the tab panel's width ([`super::Parts::route`]).
//!
//! A column, its parts 12 px apart (`.stack`):
//! - **Kind**: the badge (`Filed flight plan` good for a filed route, `Estimated` neutral for the
//!   app's own estimate, else `No route known` neutral) 2 px under the row's top (the component's
//!   `.badge { margin-top: 2px }` reaches it too), and on the right `AIRAC {ident}` (12 px
//!   `--text-3`, the ident in Mono `--text-1`) centred on the row, its tooltip `Effective {d MMM}
//!   to {d MMM}, 00:00 UTC`.
//! - **Filed** (a filed route and Flightradar24's flight plan): the route string (`Direct` when
//!   empty) in Mono 12 px at line height 1.5, `word-spacing: 0.2em`, on an `--ink` box rounded 4
//!   with `8px 12px` of padding, scrolling past 132 px of text. The box is `width: 100%` with its
//!   padding added (content-box sizing), so it runs 24 px wider than the column and the panel's
//!   edge cuts its right side, as Chromium draws today's. Then the compact facts (`Waypoints`, the
//!   whole row's `Alternates` when there are any) and the note `Flight plans are filed against the
//!   current AIRAC cycle, …` (11 px `--text-3`).
//! - **Otherwise** the explanation ([`explanation`]; 13 px `--text-2`, the runway in Mono) and,
//!   when not signed in, the button `Sign in to Flightradar24` (it opens the keys dialog,
//!   [`Action::Open`] `Keys`).
//! - **Distances** (with a great-circle distance): compact facts `Great circle`, `Flown`, `Typical
//!   time` (`—` without one).
//! - **Map key** ([`legend`]; 12 px `--text-2`, rows 8 apart, each a 28 × 3 swatch rounded 2 then
//!   the text 12 px on, the swatch centred on the row): the track's altitude gradient ([`stops`]:
//!   `ALTITUDE_STOPS` at 0, 8.9, 17.8, 33.3, 55.6, 77.8 and 100 %) with `0 – 45,000 ft` (Mono
//!   `--text-3`, 4 px on) and, when OpenSky's points were put in front, `Start of the track from
//!   the OpenSky Network` (11 px `--text-3`) under it; the filed route's solid swatch; or the
//!   estimate ahead (`--amber` dashes 6 on, 4 off) and the estimated departure (dashes 4 on, 4 off).
//! - **Runways from OurAirports** (when a runway was used): the component's `li.credit` also takes
//!   the photo credit's `.credit` rule (`position: absolute; bottom: 4px; right: 8px`, the text
//!   shadow), so today it stands at the bottom right of the panel as first laid out, under the
//!   pinned footer, and moves up with the content when the panel scrolls: drawn so here (11 px
//!   `--text-3` over the shadow, [`credit_place`]).
//!
//! Paragraphs break where CSS breaks them ([`wrap`]: a row's trailing space hangs; egui's own
//! wrapping would move the last word of a full row down). The history tab's paragraphs use it too.
//!
//! **Checked** against Chromium's layout of today's component, server-rendered by Svelte with the
//! same props and laid out with app.css at 1440 × 900 (`tests/flight_panel_tabs.rs`: estimated
//! with and without runways, scrolled, filed): rows, rules, badge, boxes, swatches and buttons land
//! on Chromium's pixels or within one (sub-pixel line boxes add up a little differently), every
//! paragraph breaks into the same rows; text sits up to a pixel lower (egui's baseline).
//!
//! Deviations:
//! - The dates are written `11 Jun` (`en-GB`, as the app's other dates; plan 1.5): today's
//!   `toLocaleDateString([])` follows the system's locale.
//! - The credit's text shadow is drawn unblurred, as the photo's (U10).

use std::sync::Arc;

use egui::epaint::{RectShape, TextureHandle};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, ColorImage, Context, CornerRadius, Galley, Id, Painter, Pos2, Rect, ScrollArea, Sense, TextureOptions, Ui, Vec2};
use rustradar_core::airac::{AiracCycle, airac_cycle};
use rustradar_core::flightpath::RunwayReason;
use rustradar_core::geo::ALTITUDE_STOPS;
use rustradar_core::model::FlightProgress;
use rustradar_core::routes::{Route, RouteKind};
use rustradar_core::time::duration;

use super::super::fonts::Face;
use super::super::panel::{self, Fact, Fit, Piece};
use super::super::shell::Pass;
use super::super::text::{self, Style};
use super::super::widgets::badge::{Badge, Tone};
use super::super::widgets::button::Button;
use super::super::widgets::tooltip;
use super::super::theme;
use super::{DASH, TABPANEL_PAD, View, km};
use crate::state::Action;
use crate::state::shortcuts::Open;

/// The kind's badges.
pub const FILED: &str = "Filed flight plan";
pub const ESTIMATED: &str = "Estimated";
pub const NO_ROUTE: &str = "No route known";
/// The button under the explanation when not signed in.
pub const SIGN_IN: &str = "Sign in to Flightradar24";
/// A filed route with no route string.
pub const DIRECT: &str = "Direct";
/// The map key's texts.
pub const TRACK_FLOWN: &str = "Track flown, coloured by altitude";
pub const SCALE: &str = "0 – 45,000 ft";
pub const OPENSKY: &str = "Start of the track from the OpenSky Network";
pub const FILED_ROUTE: &str = "Filed route";
pub const RUNWAYS_CREDIT: &str = "Runways from OurAirports";

/// The column's gap (`.stack { gap: var(--space-3) }`).
const STACK_GAP: f32 = theme::SPACE_3;
/// The badge's `margin-top`.
const BADGE_MARGIN: f32 = 2.0;
/// The filed route's box: padding, largest text height, line height, word spacing (em).
const FILED_PAD: Vec2 = Vec2::new(theme::SPACE_3, theme::SPACE_2);
const FILED_MAX: f32 = 132.0;
const FILED_LINE_HEIGHT: f32 = 1.5;
const WORD_SPACING: f32 = 0.2;
/// The map key: a swatch, its gap to the text, the rows' gap, the scale's margin.
pub const SWATCH: Vec2 = Vec2::new(28.0, 3.0);
const SWATCH_GAP: f32 = theme::SPACE_3;
const LEGEND_GAP: f32 = theme::SPACE_2;
const SCALE_MARGIN: f32 = theme::SPACE_1;
/// The dashes ahead (`--amber` 0 6px, transparent 6px 10px) and before (0 4px, 4px 8px).
pub const AHEAD_DASHES: (f32, f32) = (6.0, 4.0);
pub const BEFORE_DASHES: (f32, f32) = (4.0, 4.0);
/// The credit's place (`.credit`: `right: var(--space-2)`, `bottom: var(--space-1)`).
const CREDIT_INSET: Vec2 = Vec2::new(theme::SPACE_2, theme::SPACE_1);
/// Texels across the altitude gradient's texture.
pub const GRADIENT_TEXELS: usize = 256;

/// A run of a paragraph: its text, and whether it is set in the data face (`<span class="data">`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub data: bool,
}

fn plain(text: impl Into<String>) -> Run {
    Run { text: text.into(), data: false }
}

fn data(text: impl Into<String>) -> Run {
    Run { text: text.into(), data: true }
}

/// The runs' text as one string (what the page's `innerText` reads).
pub fn text_of(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// `a || b`.
fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

/// The kind's badge: its text and tone.
pub fn badge(route: Option<&Route>) -> (&'static str, Tone) {
    match route.map(|r| r.kind) {
        Some(RouteKind::Filed) => (FILED, Tone::Good),
        Some(RouteKind::Estimated) => (ESTIMATED, Tone::Neutral),
        _ => (NO_ROUTE, Tone::Neutral),
    }
}

/// `day(d)`: `11 Jun`, the UTC calendar day of Unix ms `ms` as `en-GB` writes it (`Sept` for
/// September, as [`rustradar_core::time::Clocks::day_at`]).
pub fn day(ms: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sept", "Oct", "Nov", "Dec"];
    // days since 1970-01-01 to the civil date (Howard Hinnant's `civil_from_days`)
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{d} {}", MONTHS[(m - 1) as usize])
}

/// The AIRAC row's tooltip: `Effective {d MMM} to {d MMM}, 00:00 UTC`.
pub fn airac_tooltip(airac: &AiracCycle) -> String {
    format!("Effective {} to {}, 00:00 UTC", day(airac.effective_ms), day(airac.expires_ms))
}

/// The filed plan's note.
pub fn filed_note(airac: &AiracCycle) -> String {
    format!("Flight plans are filed against the current AIRAC cycle, {}, effective {} to {}.", airac.ident, day(airac.effective_ms), day(airac.expires_ms))
}

/// The explanation under an estimate or no route (`p.explain`), as the component composes it: the
/// path ahead to a runway and why that runway, or to the airport, or the track alone; then the
/// filed routes' cycle and who sees them. The destination is its IATA code, else its ICAO code,
/// else `the destination`.
pub fn explanation(route: Option<&Route>, ident: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let dest = route.and_then(|r| r.destination.as_ref()).map_or_else(|| "the destination".to_owned(), |d| or(&d.iata, &d.icao).to_owned());
    match route {
        Some(r) if r.ahead.len() > 1 => match &r.arrival {
            Some(arrival) => {
                runs.push(plain(format!("The path ahead is an estimate: the great circle to {dest}, then a turn onto a straight final to runway ")));
                runs.push(data(arrival.runway.as_str()));
                let reason = match arrival.reason {
                    RunwayReason::Aligned => "The aircraft is lined up with that runway.",
                    RunwayReason::Traffic => "Other aircraft are using that runway now.",
                    RunwayReason::Guessed => "The runway in use depends on the wind, which isn't known here, so it's a guess.",
                };
                runs.push(plain(format!(" down a 3° glide path. {reason} Controllers vector arrivals, so the turns are a guess too. ")));
            }
            None => runs.push(plain(format!(
                "The path ahead is an estimate: the great circle to {dest}, descending at 3°. Its runways aren't known, so the path ends at the airport. "
            ))),
        },
        _ => runs.push(plain("The map shows the track flown so far. ")),
    }
    runs.push(plain(format!("The filed route follows airways and waypoints of AIRAC cycle {ident}; Flightradar24 shares flight plans only with signed-in subscribers.")));
    runs
}

/// A map key's swatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swatch {
    /// The track flown, coloured by altitude.
    Gradient,
    /// The filed route.
    Filed,
    /// The estimated path ahead.
    Ahead,
    /// The estimated departure.
    Before,
}

/// A row of the map key: its swatch, its text, the scale after it (the gradient's), the small line
/// under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegendRow {
    pub swatch: Swatch,
    pub runs: Vec<Run>,
    pub scale: bool,
    pub source: Option<&'static str>,
}

/// The map key's rows, and whether the runways' credit follows them.
pub fn legend(route: Option<&Route>, opensky_added: usize) -> (Vec<LegendRow>, bool) {
    let mut rows = vec![LegendRow { swatch: Swatch::Gradient, runs: vec![plain(TRACK_FLOWN)], scale: true, source: (opensky_added > 0).then_some(OPENSKY) }];
    let row = |swatch, runs| LegendRow { swatch, runs, scale: false, source: None };
    if route.is_some_and(|r| r.kind == RouteKind::Filed) {
        rows.push(row(Swatch::Filed, vec![plain(FILED_ROUTE)]));
        return (rows, false);
    }
    let Some(route) = route else { return (rows, false) };
    if route.ahead.len() > 1 {
        let runs = match &route.arrival {
            Some(arrival) => vec![plain("Estimated path to runway "), data(arrival.runway.as_str())],
            None => vec![plain("Estimated path to the airport")],
        };
        rows.push(row(Swatch::Ahead, runs));
    }
    if route.before.len() > 1 {
        let runs = match &route.departure {
            Some(departure) => vec![plain("Estimated take-off from runway "), data(departure.runway.as_str()), plain(" to the first tracked position")],
            None => vec![plain("Estimated departure to the first tracked position")],
        };
        rows.push(row(Swatch::Before, runs));
    }
    (rows, route.arrival.is_some() || route.departure.is_some())
}

/// The gradient's stops as the component writes them: each altitude stop's colour at `alt /
/// maxAlt` of the swatch's width (0, 8.9, 17.8, 33.3, 55.6, 77.8 and 100 %).
pub fn stops() -> [(f64, [u8; 3]); 7] {
    let max = ALTITUDE_STOPS[ALTITUDE_STOPS.len() - 1].0;
    ALTITUDE_STOPS.map(|(alt, c)| (alt / max, c))
}

/// The gradient's colour `t` (0–1) of the way across, as CSS mixes `rgb()` stops (in sRGB, before
/// rounding).
pub fn gradient_at(t: f64) -> [f64; 3] {
    let stops = stops();
    let channel = |c: [u8; 3]| c.map(f64::from);
    if t <= stops[0].0 {
        return channel(stops[0].1);
    }
    for pair in stops.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 1.0 };
            let (c0, c1) = (channel(c0), channel(c1));
            return [0, 1, 2].map(|k| c0[k] + (c1[k] - c0[k]) * f);
        }
    }
    channel(stops[stops.len() - 1].1)
}

/// The gradient as a row of texels: texel `i` is its colour `i / (n − 1)` of the way across.
pub fn gradient_image() -> ColorImage {
    let n = GRADIENT_TEXELS;
    let pixels = (0..n)
        .map(|i| {
            let [r, g, b] = gradient_at(i as f64 / (n - 1) as f64).map(|v| v.round() as u8);
            Color32::from_rgb(r, g, b)
        })
        .collect();
    ColorImage::new([n, 1], pixels)
}

/// The texture coordinates across a swatch: a texel centre at either edge, so a pixel `f` of the
/// way across reads the gradient at `f`.
pub fn gradient_uv() -> Rect {
    let half = 0.5 / GRADIENT_TEXELS as f32;
    Rect::from_min_max(Pos2::new(half, 0.0), Pos2::new(1.0 - half, 1.0))
}

fn gradient_texture(ctx: &Context) -> TextureHandle {
    let id = Id::unique("rustradar::flight_panel::route::gradient");
    if let Some(handle) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return handle;
    }
    let handle = ctx.load_texture("altitude gradient", gradient_image(), TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
}

/// The dashes of a swatch `width` px wide, `on` px drawn then `off` px left out, from its left
/// edge: each dash's start and end (the last cut at the edge).
pub fn dashes(width: f32, (on, off): (f32, f32)) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut x = 0.0;
    while x < width {
        out.push((x, (x + on).min(width)));
        x += on + off;
    }
    out
}

/// Draws a swatch in `rect` (snapped to whole pixels, rounded 2 where it ends, as the box clips its
/// background).
fn paint_swatch(painter: &Painter, rect: Rect, swatch: Swatch) {
    let rect = panel::snap_rect(rect, painter.pixels_per_point());
    let r = theme::RADIUS_SWATCH;
    let (pattern, color) = match swatch {
        Swatch::Gradient => {
            let texture = gradient_texture(painter.ctx());
            painter.add(RectShape::filled(rect, CornerRadius::same(r), Color32::WHITE).with_texture(texture.id(), gradient_uv()));
            return;
        }
        Swatch::Filed => {
            painter.rect_filled(rect, CornerRadius::same(r), theme::LEGEND_FILED);
            return;
        }
        Swatch::Ahead => (AHEAD_DASHES, theme::AMBER),
        Swatch::Before => (BEFORE_DASHES, theme::LEGEND_ESTIMATE),
    };
    for (x0, x1) in dashes(rect.width(), pattern) {
        let (west, east) = (if x0 <= 0.0 { r } else { 0 }, if x1 >= rect.width() { r } else { 0 });
        let dash = Rect::from_x_y_ranges(rect.left() + x0..=rect.left() + x1, rect.y_range());
        painter.rect_filled(dash, CornerRadius { nw: west, sw: west, ne: east, se: east }, color);
    }
}

/// The paragraph's text in `style`, its data runs in the data face.
fn runs_job(runs: &[Run], style: &Style) -> LayoutJob {
    let mut job = LayoutJob::default();
    for run in runs {
        let style = if run.data { panel::data(style) } else { *style };
        text::append(&mut job, &run.text, &style);
    }
    job
}

/// A text wrapped across a width as CSS wraps it, ready to paint: its rows `line` px apart.
pub struct Wrapped {
    galley: Arc<Galley>,
    line: f32,
    /// Its width and its rows' line boxes.
    pub size: Vec2,
}

/// `job` wrapped within `width` as CSS wraps it (`white-space: normal`): a row breaks at the last
/// space before the first glyph whose ink runs past the width, the space hanging past the row's
/// end. egui's own wrapping also counts the advance of the space after a row's last word, so a
/// row whose words fit can lose its last word. The breaks are found on the job laid out on one row
/// and made newlines; a word wider than the width overflows, as in CSS.
pub fn wrap(ctx: &Context, job: LayoutJob, line: f32, width: f32) -> Wrapped {
    let mut flat = job.clone();
    flat.wrap = TextWrapping::no_max_width();
    let unwrapped = ctx.fonts_mut(|f| f.layout_job(flat));
    let chars: Vec<(usize, char)> = job.text.char_indices().collect();
    let mut breaks = Vec::new();
    let mut next = 0;
    let mut matched = true;
    'rows: for row in &unwrapped.rows {
        let mut start = row.glyphs.first().map_or(0.0, |g| g.pos.x);
        // the last space a row may break at since the row's start: its byte, its glyph's index
        let mut space: Option<(usize, usize)> = None;
        for (i, glyph) in row.glyphs.iter().enumerate() {
            while chars.get(next).is_some_and(|c| c.1 == '\n') {
                next += 1;
            }
            let Some(&(byte, chr)) = chars.get(next) else {
                matched = false;
                break 'rows;
            };
            next += 1;
            if chr != glyph.chr {
                matched = false;
                break 'rows;
            }
            if chr == ' ' {
                space = Some((byte, i));
                continue;
            }
            // egui's own allowance (`effective_wrap_width`)
            if glyph.max_x() - start > width + 0.5
                && let Some((byte, at)) = space.take()
            {
                breaks.push(byte);
                start = row.glyphs.get(at + 1).map_or(glyph.pos.x, |g| g.pos.x);
            }
        }
    }
    let galley = if matched {
        let mut broken = job;
        let mut text = broken.text.into_bytes();
        for b in breaks {
            text[b] = b'\n';
        }
        broken.text = String::from_utf8(text).unwrap_or_default();
        broken.wrap = TextWrapping::no_max_width();
        broken.break_on_newline = true;
        ctx.fonts_mut(|f| f.layout_job(broken))
    } else {
        ctx.fonts_mut(|f| f.layout_job(panel::fit(job, Fit::Wrap, width)))
    };
    let rows = galley.rows.len().max(1) as f32;
    Wrapped { size: Vec2::new(galley.size().x, line * rows), galley, line }
}

impl Wrapped {
    /// Paints it with its top left corner at `at`, egui's whole-pixel rows centred on the CSS line
    /// boxes (as [`panel::Laid`]).
    pub fn paint(self, painter: &Painter, at: Pos2) {
        let rows = self.galley.rows.len().max(1) as f32;
        let top = at.y + (self.line * rows - self.galley.size().y) / 2.0 / rows;
        painter.galley(Pos2::new(at.x, top), self.galley, theme::TEXT_1);
    }
}

/// A wrapping paragraph of `job` (line boxes `line` px) across the column.
pub fn paragraph(ui: &mut Ui, job: LayoutJob, line: f32) {
    let width = ui.available_width();
    let text = wrap(ui.ctx(), job, line, width);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, text.size.y), Sense::hover());
    text.paint(ui.painter(), rect.min);
}

/// `.explain`: 13 px `--text-2`.
const fn explain_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The kind's row: the badge and the AIRAC cycle.
fn kind_row(ui: &mut Ui, id: Id, route: Option<&Route>, airac: &AiracCycle) {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let (text, tone) = badge(route);
    let badge = Badge::new(text, tone);
    let badge_h = BADGE_MARGIN + badge.measure(&ctx).y;
    let style = Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3);
    let mut job = LayoutJob::default();
    text::append(&mut job, "AIRAC ", &style);
    text::append(&mut job, &airac.ident, &panel::data(&style).color(theme::TEXT_1));
    let galley = ctx.fonts_mut(|f| f.layout_job(job));
    let row = badge_h.max(style.line_px());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, row), Sense::hover());
    let painter = ui.painter();
    badge.paint(painter, Pos2::new(rect.left(), rect.top() + (row - badge_h) / 2.0 + BADGE_MARGIN));
    let span = Rect::from_min_size(Pos2::new(rect.right() - galley.size().x, rect.top() + (row - style.line_px()) / 2.0), Vec2::new(galley.size().x, style.line_px()));
    painter.galley(Pos2::new(span.left(), span.top() + (style.line_px() - galley.size().y) / 2.0), galley, style.color);
    tooltip::show(ui.interact(span, id.with("airac"), Sense::hover()), &airac_tooltip(airac));
}

/// The filed route's text: Mono 12 px at line height 1.5, each space `0.2em` wider.
fn filed_job(route: &str) -> (LayoutJob, f32) {
    let style = panel::data(&Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_1)).line_height(FILED_LINE_HEIGHT);
    let space = style.letter_spacing(style.letter_spacing + WORD_SPACING);
    let mut job = LayoutJob::default();
    // `white-space: normal`: runs of white space read as one space, none at the ends
    for (i, word) in route.split_whitespace().enumerate() {
        if i > 0 {
            text::append(&mut job, " ", &space);
        }
        text::append(&mut job, word, &style);
    }
    (job, style.line_px())
}

/// Floating scroll bars with no fade at the edges, as the panel's own.
fn floating_scroll(ui: &mut Ui) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating = true;
    scroll.fade.strength = 0.0;
}

/// The filed route's box (see the module documentation).
fn filed_box(ui: &mut Ui, id: Id, route: &str) {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let (job, line) = filed_job(or(route, DIRECT));
    let text = wrap(&ctx, job, line, width);
    let text_h = text.size.y;
    let box_h = text_h.min(FILED_MAX) + 2.0 * FILED_PAD.y;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, box_h), Sense::hover());
    // content-box: the padding comes on top of the column's width; the panel's edge cuts it
    let edge = ui.max_rect().right() + TABPANEL_PAD;
    let clip = ui.clip_rect().intersect(Rect::from_x_y_ranges(ui.clip_rect().left()..=edge, ui.clip_rect().y_range()));
    let boxed = Rect::from_min_size(rect.min, Vec2::new(width + 2.0 * FILED_PAD.x, box_h));
    let painter = ui.painter().with_clip_rect(clip);
    painter.rect_filled(panel::snap_rect(boxed, ctx.pixels_per_point()), CornerRadius::same(theme::RADIUS), theme::INK);
    if text_h <= FILED_MAX {
        text.paint(&painter, Pos2::new(boxed.left() + FILED_PAD.x, boxed.top() + FILED_PAD.y));
        return;
    }
    // past 132 px the box scrolls (its padding with the text)
    panel::region(ui, "filed-route", boxed, |ui| {
        ui.set_clip_rect(clip.intersect(boxed));
        floating_scroll(ui);
        ScrollArea::vertical().id_salt(id.with("filed-route")).max_height(box_h).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let (inner, _) = ui.allocate_exact_size(Vec2::new(boxed.width(), text_h + 2.0 * FILED_PAD.y), Sense::hover());
            text.paint(ui.painter(), Pos2::new(boxed.left() + FILED_PAD.x, inner.top() + FILED_PAD.y));
        });
    });
}

/// Compact facts (`.facts.compact`) of Mono values.
fn compact_facts(ui: &mut Ui, items: Vec<Fact<'static>>) {
    panel::facts(ui, 3, panel::FACTS_ROW_GAP_COMPACT, items);
}

fn mono_value(value: &str) -> Piece<'static> {
    Piece::text(value, &panel::dd_style(Face::Mono, false), Fit::Wrap)
}

/// The distances' facts: the great circle, the distance flown, the typical time.
pub fn distance_facts(progress: &FlightProgress) -> [(&'static str, String); 3] {
    let typical = if progress.mean_flight_time != 0 { duration(f64::from(progress.mean_flight_time)) } else { DASH.to_owned() };
    [("Great circle", km(progress.great_circle_distance)), ("Flown", km(progress.traversed_distance)), ("Typical time", typical)]
}

/// The map key in the column.
fn paint_legend(ui: &mut Ui, rows: &[LegendRow]) {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let text_w = width - SWATCH.x - SWATCH_GAP;
    let style = Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2);
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            ui.add_space(LEGEND_GAP);
        }
        let mut job = runs_job(&row.runs, &style);
        if row.scale {
            // ` <span class="data scale">`: a word space, then 4 px of margin
            text::append(&mut job, " ", &style);
            job.append(SCALE, SCALE_MARGIN, panel::data(&style).color(theme::TEXT_3).format());
        }
        let main = wrap(&ctx, job, style.line_px(), text_w);
        let note = panel::note_style();
        let source = row.source.map(|s| wrap(&ctx, text::job(s, &note), note.line_px(), text_w));
        let main_h = main.size.y;
        let height = main_h + source.as_ref().map_or(0.0, |s| s.size.y);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let painter = ui.painter();
        let swatch = Rect::from_min_size(Pos2::new(rect.left(), rect.top() + (height - SWATCH.y) / 2.0), SWATCH);
        paint_swatch(painter, swatch, row.swatch);
        let left = rect.left() + SWATCH.x + SWATCH_GAP;
        main.paint(painter, Pos2::new(left, rect.top()));
        if let Some(source) = source {
            source.paint(painter, Pos2::new(left, rect.top() + main_h));
        }
    }
}

/// Where the runways' credit stands: right 8 and bottom 4 inside the panel's box as first laid
/// out, moving up with the content as the panel scrolls (`viewport`: the panel's visible inner
/// box; `scrolled`: how far its content is scrolled; `size`: the text's line box).
pub fn credit_place(viewport: Rect, scrolled: f32, size: Vec2) -> Rect {
    let bottom = viewport.bottom() - scrolled - CREDIT_INSET.y;
    Rect::from_min_size(Pos2::new(viewport.right() - CREDIT_INSET.x - size.x, bottom - size.y), size)
}

/// The runways' credit (see the module documentation), drawn under the pinned footer.
fn paint_credit(ui: &Ui) {
    let ctx = ui.ctx();
    let style = panel::note_style();
    let galley = ctx.fonts_mut(|f| f.layout_job(text::job(RUNWAYS_CREDIT, &style)));
    // the panel's inner box: its area as it stood last pass, inside its border (the scroll area's
    // clip runs to the window's bottom while the content fits)
    let Some(area) = ctx.memory(|m| m.area_rect(ui.layer_id().id)) else { return };
    let viewport = area.shrink(theme::BORDER);
    let at = credit_place(viewport, super::scroll_offset(ctx), Vec2::new(galley.size().x, style.line_px()));
    let top = at.top() + (style.line_px() - galley.size().y) / 2.0;
    let painter = ui.painter();
    let shadow = theme::PHOTO_TEXT_SHADOW.gamma_multiply(0.5);
    painter.galley_with_override_text_color(Pos2::new(at.left(), top + 1.0), galley.clone(), shadow);
    painter.galley(Pos2::new(at.left(), top), galley, style.color);
}

/// The tab's content in `ui` (the tab panel's inner width).
pub fn show(ui: &mut Ui, view: &View<'_>, pass: &mut Pass<'_>) {
    let id = Id::unique("rustradar::flight_panel::route");
    let airac = airac_cycle(view.now_ms as i64);
    kind_row(ui, id, view.route, &airac);
    let plan = view.details.and_then(|d| d.flight_plan.as_ref()).filter(|_| view.route.is_some_and(|r| r.kind == RouteKind::Filed));
    if let Some(plan) = plan {
        ui.add_space(STACK_GAP);
        filed_box(ui, id, &plan.route);
        ui.add_space(STACK_GAP);
        let mut facts = vec![Fact::new("Waypoints", mono_value(&plan.waypoints.len().to_string()))];
        if !plan.alternates.is_empty() {
            facts.push(Fact::new("Alternates", mono_value(&plan.alternates.join(", "))).wide(true));
        }
        compact_facts(ui, facts);
        ui.add_space(STACK_GAP);
        let note = panel::note_style();
        paragraph(ui, text::job(&filed_note(&airac), &note), note.line_px());
    } else {
        ui.add_space(STACK_GAP);
        let style = explain_style();
        paragraph(ui, runs_job(&explanation(view.route, &airac.ident), &style), style.line_px());
        if !view.authenticated {
            ui.add_space(STACK_GAP);
            let sign_in = Button::new().text(SIGN_IN).show(ui);
            if sign_in.clicked() {
                // `promptSignIn`: the email field takes the keyboard, the button gets it back (U17)
                crate::ui::keys_dialog::prompt_sign_in(ui.ctx(), Some(sign_in.id));
                pass.actions.push(Action::Open(Open::Keys, true));
            }
        }
    }
    if let Some(progress) = view.progress().filter(|p| p.great_circle_distance != 0) {
        ui.add_space(STACK_GAP);
        compact_facts(ui, distance_facts(progress).into_iter().map(|(label, value)| Fact::new(label, mono_value(&value))).collect());
    }
    ui.add_space(STACK_GAP);
    let (rows, credit) = legend(view.route, view.opensky_added);
    paint_legend(ui, &rows);
    if credit {
        paint_credit(ui);
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::Airport;
    use rustradar_core::routes::{PathPoint, RunwayGuess};

    use super::*;

    fn airport(iata: &str, icao: &str) -> Airport {
        Airport { iata: iata.into(), icao: icao.into(), ..Default::default() }
    }

    fn route(kind: RouteKind, ahead: usize, before: usize) -> Route {
        let point = PathPoint::default();
        Route {
            origin: Some(airport("ARN", "ESSA")),
            destination: Some(airport("CPH", "EKCH")),
            kind,
            filed: Vec::new(),
            before: vec![point; before],
            ahead: vec![point; ahead],
            departure: None,
            arrival: None,
        }
    }

    const TAIL: &str = "The filed route follows airways and waypoints of AIRAC cycle 2606; Flightradar24 shares flight plans only with signed-in subscribers.";

    #[test]
    fn the_explanation_reads_as_today() {
        // to the airport: no runways known (Chromium's text of today's component)
        let r = route(RouteKind::Estimated, 2, 2);
        assert_eq!(
            text_of(&explanation(Some(&r), "2606")),
            format!("The path ahead is an estimate: the great circle to CPH, descending at 3°. Its runways aren't known, so the path ends at the airport. {TAIL}")
        );
        // to a runway, each reason
        let mut r = route(RouteKind::Estimated, 3, 0);
        for (reason, why) in [
            (RunwayReason::Guessed, "The runway in use depends on the wind, which isn't known here, so it's a guess."),
            (RunwayReason::Aligned, "The aircraft is lined up with that runway."),
            (RunwayReason::Traffic, "Other aircraft are using that runway now."),
        ] {
            r.arrival = Some(RunwayGuess { runway: "22L".into(), reason });
            let runs = explanation(Some(&r), "2606");
            assert_eq!(
                text_of(&runs),
                format!("The path ahead is an estimate: the great circle to CPH, then a turn onto a straight final to runway 22L down a 3° glide path. {why} Controllers vector arrivals, so the turns are a guess too. {TAIL}")
            );
            assert_eq!(runs.iter().filter(|r| r.data).map(|r| r.text.as_str()).collect::<Vec<_>>(), ["22L"], "the runway in the data face");
        }
        // the track alone: one point ahead, or no route
        assert_eq!(text_of(&explanation(Some(&route(RouteKind::Estimated, 1, 2)), "2606")), format!("The map shows the track flown so far. {TAIL}"));
        assert_eq!(text_of(&explanation(None, "2606")), format!("The map shows the track flown so far. {TAIL}"));
        // the ICAO code without an IATA one; the destination unknown
        let mut r = route(RouteKind::Estimated, 2, 0);
        r.destination = Some(airport("", "EKCH"));
        assert!(text_of(&explanation(Some(&r), "2606")).starts_with("The path ahead is an estimate: the great circle to EKCH, descending"));
        r.destination = None;
        assert!(text_of(&explanation(Some(&r), "2606")).starts_with("The path ahead is an estimate: the great circle to the destination, descending"));
    }

    #[test]
    fn the_badge_and_the_map_key() {
        assert_eq!(badge(None), (NO_ROUTE, Tone::Neutral));
        assert_eq!(badge(Some(&route(RouteKind::None, 0, 0))), (NO_ROUTE, Tone::Neutral));
        assert_eq!(badge(Some(&route(RouteKind::Estimated, 0, 0))), (ESTIMATED, Tone::Neutral));
        assert_eq!(badge(Some(&route(RouteKind::Filed, 0, 0))), (FILED, Tone::Good));

        let swatches = |rows: &[LegendRow]| rows.iter().map(|r| r.swatch).collect::<Vec<_>>();
        // the track alone, OpenSky's start under it
        let (rows, credit) = legend(None, 12);
        assert_eq!(swatches(&rows), [Swatch::Gradient]);
        assert_eq!((rows[0].scale, rows[0].source), (true, Some(OPENSKY)));
        assert!(!credit);
        assert_eq!(legend(None, 0).0[0].source, None);
        // filed: the solid line, nothing estimated
        let (rows, credit) = legend(Some(&route(RouteKind::Filed, 5, 5)), 0);
        assert_eq!(swatches(&rows), [Swatch::Gradient, Swatch::Filed]);
        assert_eq!(text_of(&rows[1].runs), FILED_ROUTE);
        assert!(!credit);
        // estimated both ways, to the airport and from it
        let mut r = route(RouteKind::Estimated, 2, 2);
        let (rows, credit) = legend(Some(&r), 0);
        assert_eq!(swatches(&rows), [Swatch::Gradient, Swatch::Ahead, Swatch::Before]);
        assert_eq!(text_of(&rows[1].runs), "Estimated path to the airport");
        assert_eq!(text_of(&rows[2].runs), "Estimated departure to the first tracked position");
        assert!(!credit);
        // with runways: named in the data face, and the credit
        r.arrival = Some(RunwayGuess { runway: "22L".into(), reason: RunwayReason::Guessed });
        r.departure = Some(RunwayGuess { runway: "19R".into(), reason: RunwayReason::Aligned });
        let (rows, credit) = legend(Some(&r), 0);
        assert_eq!(rows[1].runs, [plain("Estimated path to runway "), data("22L")]);
        assert_eq!(text_of(&rows[2].runs), "Estimated take-off from runway 19R to the first tracked position");
        assert!(credit);
        // a single point either way draws no estimate, but a runway still earns the credit
        let mut r = route(RouteKind::Estimated, 1, 1);
        r.departure = Some(RunwayGuess { runway: "19R".into(), reason: RunwayReason::Aligned });
        let (rows, credit) = legend(Some(&r), 0);
        assert_eq!(swatches(&rows), [Swatch::Gradient]);
        assert!(credit);
    }

    #[test]
    fn the_gradient_takes_the_altitude_stops_where_the_component_puts_them() {
        // 0, 8.9, 17.8, 33.3, 55.6, 77.8 and 100 %: each stop's altitude over 45,000 ft
        let at: Vec<f64> = stops().iter().map(|s| (s.0 * 1000.0).round() / 10.0).collect();
        assert_eq!(at, [0.0, 8.9, 17.8, 33.3, 55.6, 77.8, 100.0]);
        for (t, c) in stops() {
            assert_eq!(gradient_at(t), c.map(f64::from), "the stop's own colour at {t}");
        }
        // halfway between the first two stops; held past the ends
        let mid = gradient_at(4_000.0 / 45_000.0 / 2.0);
        assert_eq!(mid, [255.0, 147.0, 0.0]);
        assert_eq!(gradient_at(-1.0), [255.0, 98.0, 0.0]);
        assert_eq!(gradient_at(2.0), [205.0, 85.0, 255.0]);
        // the texture: its first and last texels the end stops, and a pixel f of the way across a
        // swatch samples the gradient at f (linear between texel centres)
        let image = gradient_image();
        assert_eq!(image.pixels[0], Color32::from_rgb(255, 98, 0));
        assert_eq!(image.pixels[GRADIENT_TEXELS - 1], Color32::from_rgb(205, 85, 255));
        let uv = gradient_uv();
        for k in 0..28 {
            let f = (k as f32 + 0.5) / 28.0;
            let u = uv.min.x + (uv.max.x - uv.min.x) * f;
            let x = u * GRADIENT_TEXELS as f32 - 0.5;
            let (i, w) = (x.floor() as usize, x - x.floor());
            let sampled = f32::from(image.pixels[i].g()) * (1.0 - w) + f32::from(image.pixels[(i + 1).min(GRADIENT_TEXELS - 1)].g()) * w;
            let want = gradient_at(f64::from(f))[1] as f32;
            assert!((sampled - want).abs() <= 1.5, "pixel {k}: {sampled} for {want}");
        }
    }

    #[test]
    fn dashes_repeat_from_the_left_edge() {
        // Chromium: 1079–1085, 1089–1095, 1099–1105 for the path ahead; four of 4 before
        assert_eq!(dashes(28.0, AHEAD_DASHES), [(0.0, 6.0), (10.0, 16.0), (20.0, 26.0)]);
        assert_eq!(dashes(28.0, BEFORE_DASHES), [(0.0, 4.0), (8.0, 12.0), (16.0, 20.0), (24.0, 28.0)]);
        assert_eq!(dashes(5.0, AHEAD_DASHES), [(0.0, 5.0)]);
    }

    #[test]
    fn dates_and_the_airac_texts() {
        // AIRAC 2606: effective 11 June 2026, the next on 9 July
        let airac = airac_cycle(1_782_036_000_000);
        assert_eq!(airac.ident, "2606");
        assert_eq!(airac_tooltip(&airac), "Effective 11 Jun to 9 Jul, 00:00 UTC");
        assert_eq!(filed_note(&airac), "Flight plans are filed against the current AIRAC cycle, 2606, effective 11 Jun to 9 Jul.");
        assert_eq!(day(0), "1 Jan");
        assert_eq!(day(1_788_652_800_000), "6 Sept");
        assert_eq!(day(-86_400_000), "31 Dec");
        assert_eq!(day(951_782_400_000), "29 Feb");
    }

    #[test]
    fn distances_and_the_credit_place() {
        let p = FlightProgress { great_circle_distance: 523_000, traversed_distance: 312_400, mean_flight_time: 4_200, ..Default::default() };
        assert_eq!(distance_facts(&p).map(|f| f.1), ["523 km", "312 km", "1 h 10 min"]);
        let p = FlightProgress { great_circle_distance: 1_523_400, ..Default::default() };
        assert_eq!(distance_facts(&p).map(|f| f.1), ["1,523 km", "0 km", "—"]);
        // Chromium: the li at 1244.33–1415 × 851.61–867 in a panel whose inner box ends at 1423
        // and 871 (its text 130.67 wide after 40 of padding)
        let viewport = Rect::from_min_max(Pos2::new(1063.0, 147.0), Pos2::new(1423.0, 871.0));
        let at = credit_place(viewport, 0.0, Vec2::new(130.67, 15.4));
        assert!((at.right() - 1415.0).abs() < 0.01 && (at.bottom() - 867.0).abs() < 0.01, "{at:?}");
        assert!((credit_place(viewport, 100.0, Vec2::new(130.67, 15.4)).bottom() - 767.0).abs() < 0.01, "up with the content");
    }

    /// The rows of `runs` in `style` wrapped across `width` in a context with the app's fonts.
    fn rows(runs: &[Run], style: &Style, width: f32) -> Vec<String> {
        let ctx = Context::default();
        theme::apply(&ctx);
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 900.0))), ..Default::default() };
        let mut rows = Vec::new();
        let mut output = ctx.run_ui(raw, |ui| {
            let wrapped = wrap(ui.ctx(), runs_job(runs, style), style.line_px(), width);
            rows = wrapped.galley.rows.iter().map(|r| r.glyphs.iter().map(|g| g.chr).collect::<String>().trim_end().to_owned()).collect();
        });
        output.textures_delta.clear();
        rows
    }

    #[test]
    fn paragraphs_break_where_chromium_breaks_them() {
        // egui alone moves `3°` down: the space after it does not fit (Chromium lets it hang)
        let mut r = route(RouteKind::Estimated, 3, 0);
        r.arrival = Some(RunwayGuess { runway: "22R".into(), reason: RunwayReason::Guessed });
        assert_eq!(
            rows(&explanation(Some(&r), "2606"), &explain_style(), 328.0),
            [
                "The path ahead is an estimate: the great circle to CPH,",
                "then a turn onto a straight final to runway 22R down a 3°",
                "glide path. The runway in use depends on the wind,",
                "which isn't known here, so it's a guess. Controllers",
                "vector arrivals, so the turns are a guess too. The filed",
                "route follows airways and waypoints of AIRAC cycle",
                "2606; Flightradar24 shares flight plans only with",
                "signed-in subscribers.",
            ]
        );
        // a word wider than the width overflows its row; a row with nothing to break at stays
        let style = explain_style();
        assert_eq!(rows(&[plain("Supercalifragilistic expialidocious")], &style, 60.0), ["Supercalifragilistic", "expialidocious"]);
        assert_eq!(rows(&[plain("one"), data(" two"), plain("\nthree")], &style, 300.0), ["one two", "three"]);
    }

    #[test]
    fn the_filed_route_spaces_its_words() {
        let (job, line) = filed_job(" N0450F360 HAPZI2C  DCT ");
        assert_eq!(job.text, "N0450F360 HAPZI2C DCT", "white space collapsed as CSS does");
        assert_eq!(line, 18.0);
        let text_of = |s: &egui::text::LayoutSection| &job.text[s.byte_range.start.0..s.byte_range.end.0];
        let spaces: Vec<f32> = job.sections.iter().filter(|s| text_of(s).trim().is_empty()).map(|s| s.format.extra_letter_spacing).collect();
        let words: Vec<f32> = job.sections.iter().filter(|s| !text_of(s).trim().is_empty()).map(|s| s.format.extra_letter_spacing).collect();
        // −0.01 em of letter spacing and 0.2 em of word spacing at 12 px
        assert_eq!(job.sections.iter().filter(|s| text_of(s).trim().is_empty()).map(|s| text_of(s).len()).sum::<usize>(), 2);
        assert!(words.iter().all(|s| (s + 0.12).abs() < 1e-4), "{words:?}");
        assert!(spaces.iter().all(|s| (s - 2.28).abs() < 1e-4), "{spaces:?}");
    }
}


