//! AltitudeChart (item U12; doc 06 §9; `AltitudeChart.svelte`): the flight's altitude over time in
//! the flight panel's Overview, between the facts and "Reported by the aircraft"
//! ([`super::flight_panel::Parts::chart`]: [`show`]), with the [`super::export_menu`] in its
//! header.
//!
//! - **Header** (a row on one baseline, space between): `Altitude` (12 px at 600 `--text-2`) and
//!   ` feet` (400, `--text-3`); while the profile can be drawn, at the right, 12 apart: the zone
//!   note (11 px `--text-3`: `{place} time`, or `Your time, {zone}` when the departure airport's
//!   zone is not known, its tooltip saying which), the export trigger and the text toggle `Table`
//!   / `Chart` (12 px `--text-3`, `--text-1` under the pointer). The export's block (`.export`, a
//!   flex item whose line box is the panel's 14 px strut) makes the row 19.6 px high, its
//!   baseline the strut's; without it the row is 16.8.
//! - **Data** ([`Profile`]): the points with a time, oldest first. Fewer than two, or no time
//!   between the first and the last: [`EMPTY`] (12 px `--text-3`) under the header.
//! - **Plot** (8 px under the header, the container's width × 132; [`Plot`]): `x(t) = 46 + (t −
//!   t0)/(t1 − t0)·(width − 54)`, `y(alt) = 114 − max(0, alt)/top·96`, `top` from [`nice_scale`]
//!   of `max(1000, highest)·1.12`. A 1 px `--line` grid line at each step with its label (Mono
//!   11 px `--text-3`, thousands grouped) right-aligned 6 px left of the plot, its baseline 4 px
//!   under the line; times ([`x_ticks`]: every 5, 10, 15, 30, 60, 120, 180, 360 or 720 minutes,
//!   whichever gives at most four intervals, on round minutes of the zone's clock; one that would
//!   hang off either end left out) centred under the plot, their baseline at 127. The area under
//!   the line in `--series-1` at 10 %; the line in `--series-1`, 2 px, round caps, through the
//!   points at a tenth of a pixel (`toFixed(1)`). The end dot (radius 5, `--series-1`, `--text-3`
//!   once ended, a 2 px `--surface` ring) and its label `{n} ft` (Mono 11 px `--text-2`, `--text-3`
//!   once ended) right-aligned 9 px left of it, 8 px over it when the line comes in level or
//!   climbing ([`end_label`]), 16 under it descending when there is room, else none. The plot's
//!   origin stands on a whole pixel, as Chromium puts an `<svg>`.
//! - **Reading a point**: the pointer over the plot picks the point nearest in time ([`nearest`]),
//!   when it moves; leaving clears it. The plot takes the keyboard (Tab): it then reads the last
//!   point; Left and Right step one point (with Shift a tenth of them), Home and End go to the
//!   ends; leaving it clears. A point read shows a 1 px `--line-control` line down the plot, the
//!   dot on the point, and the box at the plot's top (10 px right of the line, or left of it past
//!   the plot's middle; padding `4px 8px`, 1 px `--line`, radius 4, `--surface-raised`): a 10 × 2
//!   `--series-1` key and `{n} ft` (Mono 13 px at 650), then `<Clock> · {kt} kt` (11 px
//!   `--text-2`); the end dot and its label are hidden meanwhile.
//! - **Table** (the toggle): `Time`, `Altitude`, `Speed` (12 px at 600 `--text-3`, the last two
//!   right-aligned, pinned at the top while the rows scroll) over about 30 rows ([`table_rows`]:
//!   every `ceil(n/30)`th point and the last), Mono 12 px, a 1 px `--line` rule over each, padded
//!   `2px 0`, in columns sized as Chromium's automatic table layout sizes them (each its widest
//!   text, the rest shared in proportion); at most 180 px high, scrolling.
//!
//! The component's state (the table shown, the point read) is kept while the chart is drawn
//! from pass to pass, and starts afresh when it is drawn again after a pass without it (the
//! panel reopened, the Overview tab chosen again), as the component mounts again today.
//!
//! **Checked** against Chromium's layout of today's component (mounted by Svelte in a 360 px
//! panel with app.css, 328 px of chart; the states of `tests/altitude_chart.rs`: climbing, the
//! point read by the pointer either side of the middle and by the keys, descending, low, five
//! hours, ended without a zone, the table, the menu by pointer and by keys, saved, empty): the
//! grid rows, the area, the line, the dot and its ring, the box of the point read, the table's
//! rules, the menu's border and the focus rings land on Chromium's pixels; the header's texts,
//! the tick and end labels and the table's cells on its ink, or within a pixel (texts stand on
//! Chromium's baselines, `ui::text`'s hook, a wrapped message's rows too).
//!
//! Deviations:
//! - A click on the plot does not give it the keyboard (Chromium focuses it, showing the last
//!   point until the pointer moves); Tab does.
//! - The line's joins are egui's (mitred, cut at sharp corners) rather than round; the caps are
//!   round.

use egui::epaint::{Mesh, PathShape};
use egui::{Color32, CornerRadius, CursorIcon, Event, FocusDirection, Id, Key, Pos2, Rect, ScrollArea, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use rustradar_core::export::ExportMeta;
use rustradar_core::model::TrailPoint;
use rustradar_core::num::{fixed_number, js_round};
use rustradar_core::text::group_thousands;
use rustradar_core::time::Clocks;

use super::export_menu::{self, Trigger};
use super::flight_panel::View;
use super::fonts::Face;
use super::panel;
use super::shell::Pass;
use super::text::{self, Style};
use super::theme;
use super::widgets::button::focus_ring;
use super::widgets::tooltip;

/// Room for the y labels on the left, the end dot on the right, the end label above the ceiling
/// and the times under the baseline (px).
pub const LEFT: f64 = 46.0;
pub const RIGHT: f64 = 8.0;
pub const TOP: f64 = 18.0;
pub const PLOT: f64 = 96.0;
pub const AXIS: f64 = 18.0;
/// The plot's height.
pub const HEIGHT: f64 = TOP + PLOT + AXIS;
/// About how many rows the table shows.
pub const TABLE_ROWS: usize = 30;
/// The table's highest (`max-height: 180px`).
pub const TABLE_MAX: f32 = 180.0;

/// The heading and its unit.
pub const HEADING: &str = "Altitude ";
pub const UNIT: &str = "feet";
/// The toggle's labels.
pub const TABLE: &str = "Table";
pub const CHART: &str = "Chart";
/// What shows without enough points.
pub const EMPTY: &str = "The altitude profile appears once the aircraft has reported a few positions.";
/// The table's headers.
pub const COLUMNS: [&str; 3] = ["Time", "Altitude", "Speed"];

/// Between the header and what is under it (`.chart { gap }`), and between the header's tools.
pub const GAP: f32 = theme::SPACE_2;
pub const TOOLS_GAP: f32 = theme::SPACE_3;
/// The read point's box: its offset from the line, padding, gap between its lines, the key.
pub const TIP_OFFSET: f32 = 10.0;
pub const TIP_PAD: Vec2 = Vec2::new(theme::SPACE_2, theme::SPACE_1);
pub const TIP_GAP: f32 = 1.0;
pub const KEY: Vec2 = Vec2::new(10.0, 2.0);
pub const KEY_GAP: f32 = theme::SPACE_2;
/// The table's header padding under its text, a row's padding, a rule.
pub const TH_PAD: f32 = theme::SPACE_1;
pub const TD_PAD: f32 = 2.0;
/// The end dot's radius and ring, the line's width.
pub const DOT: f32 = 5.0;
pub const RING: f32 = 2.0;
pub const LINE: f32 = 2.0;

/// The heading (12 px at 600, `--text-2`) and its unit (400, `--text-3`).
pub const fn heading_style() -> Style {
    Style::ui(theme::TEXT_SM, 600, theme::TEXT_2)
}

/// The zone note (11 px `--text-3`).
pub const fn zone_style() -> Style {
    Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The toggle (12 px, `--text-3`), and the empty message.
pub const fn small_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The plot's labels (`.tick`, `.end`: Overpass Mono 11 px, no letter spacing).
pub const fn tick_style(color: Color32) -> Style {
    Style { face: Face::Mono, size: theme::TEXT_XS, weight: theme::WEIGHT_REGULAR, color, line_height: theme::LINE_HEIGHT, letter_spacing: 0.0 }
}

/// The read point's value (`.value`: Overpass Mono 13 px at 650).
pub const fn value_style() -> Style {
    Style { face: Face::Mono, size: theme::TEXT_MD, weight: theme::WEIGHT_BADGE, color: theme::TEXT_1, line_height: theme::LINE_HEIGHT, letter_spacing: 0.0 }
}

/// The read point's clock and speed (`.label`: 11 px `--text-2`).
pub const fn reading_style() -> Style {
    Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// The panel's strut (14 px at 1.4), which the export's block stands on.
pub const fn strut_style() -> Style {
    Style::ui(theme::TEXT_BASE, theme::WEIGHT_REGULAR, theme::TEXT_1)
}

/// `{n} ft`: the altitude, none below zero, thousands grouped (`feet`).
pub fn feet(alt: i32) -> String {
    format!("{} ft", group_thousands(f64::from(alt.max(0))))
}

/// The zone note and its tooltip: `{place} time` with the zone it names, or the viewer's own.
pub fn zone_note(place: Option<&str>, zone: &str) -> (String, String) {
    match place.filter(|p| !p.is_empty()) {
        Some(place) => (format!("{place} time"), format!("Times are local to {place} ({zone})")),
        None => (format!("Your time, {zone}"), format!("Times are in your time zone ({zone}); the departure airport's isn't known")),
    }
}

/// The y axis: its ceiling and the step between its lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub top: f64,
    pub step: f64,
}

/// A clean ceiling with at most four intervals (`niceScale`): the first step for which the
/// ceiling over `max` is at most four of it.
pub fn nice_scale(max: f64) -> Scale {
    for step in [250.0, 500.0, 1_000.0, 2_000.0, 2_500.0, 5_000.0, 10_000.0, 15_000.0, 20_000.0] {
        let top = f64::max(step, (max / step).ceil() * step);
        if top / step <= 4.0 {
            return Scale { top, step };
        }
    }
    Scale { top: (max / 20_000.0).ceil() * 20_000.0, step: 20_000.0 }
}

/// The trail as the chart reads it (`points`): the points with a time, oldest first (a stable
/// sort, as `Array.prototype.sort`).
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub points: Vec<TrailPoint>,
}

impl Profile {
    pub fn new(trail: &[TrailPoint]) -> Profile {
        let mut points: Vec<TrailPoint> = trail.iter().filter(|p| p.timestamp > 0).copied().collect();
        points.sort_by_key(|p| p.timestamp);
        Profile { points }
    }

    /// The first point's time, 0 without one.
    pub fn t0(&self) -> f64 {
        self.points.first().map_or(0.0, |p| p.timestamp as f64)
    }

    /// The last point's time, 0 without one.
    pub fn t1(&self) -> f64 {
        self.points.last().map_or(0.0, |p| p.timestamp as f64)
    }

    /// Two points or more over some time: the chart, else the empty message.
    pub fn enough(&self) -> bool {
        self.points.len() >= 2 && self.t1() > self.t0()
    }

    /// Enough, in a container wider than the plot's margins (`drawable`).
    pub fn drawable(&self, width: f64) -> bool {
        self.enough() && width > LEFT + RIGHT
    }

    /// `niceScale(max(1000, highest) × 1.12)`: headroom over the highest point keeps the end label
    /// in the chart.
    pub fn scale(&self) -> Scale {
        let highest = self.points.iter().map(|p| f64::from(p.altitude)).fold(1_000.0, f64::max);
        nice_scale(highest * 1.12)
    }

    /// The highest point, the first of equals.
    pub fn highest(&self) -> Option<&TrailPoint> {
        let mut best = self.points.first()?;
        for p in &self.points {
            if p.altitude > best.altitude {
                best = p;
            }
        }
        Some(best)
    }

    /// The plot over a container `width` px wide.
    pub fn plot(&self, width: f64) -> Plot {
        Plot { width, t0: self.t0(), t1: self.t1(), scale: self.scale() }
    }
}

/// The plot's coordinates: `width` px across, the points' time span, the y scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plot {
    pub width: f64,
    pub t0: f64,
    pub t1: f64,
    pub scale: Scale,
}

impl Plot {
    pub fn x(&self, t: f64) -> f64 {
        LEFT + ((t - self.t0) / (self.t1 - self.t0)) * (self.width - LEFT - RIGHT)
    }

    pub fn y(&self, alt: f64) -> f64 {
        TOP + PLOT - (alt.max(0.0) / self.scale.top) * PLOT
    }

    /// The altitudes of the grid lines: every step from 0 to the ceiling.
    pub fn y_ticks(&self) -> Vec<f64> {
        let n = (self.scale.top / self.scale.step) as usize;
        (0..=n).map(|i| i as f64 * self.scale.step).collect()
    }

    /// The time at `px` from the plot's left (the pointer's), within the points' span.
    pub fn time_at(&self, px: f64) -> f64 {
        let t = self.t0 + ((px - LEFT) / (self.width - LEFT - RIGHT)) * (self.t1 - self.t0);
        self.t1.min(self.t0.max(t))
    }
}

/// The times of the x labels (`xTicks`): on round minutes of the clock in `zone`, every 5, 10, …
/// 720 minutes (the first giving at most four intervals over the span), those whose 36 px label
/// would hang off either end left out.
pub fn x_ticks(plot: &Plot, clocks: &Clocks, zone: Option<&str>) -> Vec<f64> {
    let span = plot.t1 - plot.t0;
    let minutes = [5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 180.0, 360.0, 720.0].into_iter().find(|m| span / (m * 60.0) <= 4.0).unwrap_or(720.0);
    let start = (plot.t0 / 60.0).floor() * 60.0;
    let local = f64::from(clocks.minutes_of_day(start, zone));
    let mut ticks = Vec::new();
    let mut t = start + ((local / minutes).ceil() * minutes - local) * 60.0;
    while t <= plot.t1 {
        let x = plot.x(t);
        if x - 18.0 >= LEFT - 6.0 && x + 18.0 <= plot.width {
            ticks.push(t);
        }
        t += minutes * 60.0;
    }
    ticks
}

/// The line through `points` (`line`): each point at a tenth of a pixel, as `toFixed(1)` writes
/// the path.
pub fn line(points: &[TrailPoint], plot: &Plot) -> Vec<Pos2> {
    points.iter().map(|p| Pos2::new(fixed_number(plot.x(p.timestamp as f64), 1) as f32, fixed_number(plot.y(f64::from(p.altitude)), 1) as f32)).collect()
}

/// Where the end label's right end and baseline stand (`endLabel`), beside the end dot on the side
/// the line does not come from: over it when the line arrives level or climbing (the point about
/// 64 px earlier no more than 300 ft higher), under it descending, and nowhere when descending
/// close to the ground.
pub fn end_label(points: &[TrailPoint], plot: &Plot) -> Option<(f64, f64)> {
    let last = points.last()?;
    let ex = plot.x(last.timestamp as f64);
    let ey = plot.y(f64::from(last.altitude));
    let earlier = &points[points.iter().position(|p| plot.x(p.timestamp as f64) >= ex - 64.0).unwrap_or(0)];
    if i64::from(earlier.altitude) <= i64::from(last.altitude) + 300 {
        return Some((ex - 9.0, ey - 8.0));
    }
    (ey + 18.0 <= TOP + PLOT).then_some((ex - 9.0, ey + 16.0))
}

/// The point nearest in time to `t` (`nearest`, a binary search; the earlier of two as near).
/// `points` holds two or more.
pub fn nearest(points: &[TrailPoint], t: f64) -> usize {
    let (mut lo, mut hi) = (0, points.len() - 1);
    while hi - lo > 1 {
        let mid = (lo + hi) >> 1;
        if (points[mid].timestamp as f64) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if t - points[lo].timestamp as f64 <= points[hi].timestamp as f64 - t { lo } else { hi }
}

/// The point the plot's keys move to from `active` (the last point when none) among `len`:
/// Left and Right one point (with Shift a tenth of them), Home and End the ends; `None` for any
/// other key.
pub fn step(len: usize, active: Option<usize>, key: Key, shift: bool) -> Option<usize> {
    let len = len as i64;
    let step = if shift { js_round(len as f64 / 10.0).max(1.0) as i64 } else { 1 };
    let current = active.map_or(len - 1, |a| a as i64);
    let next = match key {
        Key::ArrowLeft => current - step,
        Key::ArrowRight => current + step,
        Key::Home => 0,
        Key::End => len - 1,
        _ => return None,
    };
    Some(next.max(0).min(len - 1) as usize)
}

/// The rows of the table (`rows`): every `ceil(n/30)`th point, and the last.
pub fn table_rows(len: usize) -> Vec<usize> {
    let stride = len.div_ceil(TABLE_ROWS).max(1);
    let mut rows: Vec<usize> = (0..len).step_by(stride).collect();
    if len > 0 && rows.last() != Some(&(len - 1)) {
        rows.push(len - 1);
    }
    rows
}

/// The plot's accessible description (`summary`).
pub fn summary(profile: &Profile, clocks: &Clocks, zone: Option<&str>, ended: bool) -> String {
    let (Some(last), Some(highest)) = (profile.points.last(), profile.highest()) else { return String::new() };
    let t0 = profile.t0();
    format!(
        "Altitude since {} {}: highest {} at {}, {} {}.",
        clocks.clock_at(t0, zone),
        clocks.zone_name(t0, zone),
        feet(highest.altitude),
        clocks.clock_at(highest.timestamp, zone),
        if ended { "last reported" } else { "now" },
        feet(last.altitude)
    )
}

/// What the chart is given (the component's props).
#[derive(Debug, Clone, Copy)]
pub struct Chart<'a> {
    /// The flight's recorded track.
    pub trail: &'a [TrailPoint],
    /// The departure airport's IANA zone, which the times are read in (the viewer's without one).
    pub time_zone: Option<&'a str>,
    /// Whose time that is (`ARN`); only with `time_zone`.
    pub place: Option<&'a str>,
    /// No longer reporting: the end of the line is the last report.
    pub ended: bool,
    /// The export menu's flight, when the menu is in the header (the `tools` snippet).
    pub export: Option<&'a ExportMeta>,
}

/// What the chart's export file says about the panel's flight (`exportMeta`).
pub fn export_meta(view: &View<'_>) -> ExportMeta {
    let or = |a: &str, b: &str| if a.is_empty() { b.to_owned() } else { a.to_owned() };
    let schedule_number = view.schedule().map_or("", |s| s.flight_number.as_str());
    ExportMeta {
        callsign: or(view.info().map_or("", |i| i.callsign.as_str()), &view.flight.callsign),
        flight_number: or(&view.flight.flight, schedule_number),
        registration: view.registration().to_owned(),
        aircraft_type: view.typecode().to_owned(),
        origin: view.code(false).to_owned(),
        destination: view.code(true).to_owned(),
    }
}

/// The chart in the flight panel's Overview, from the panel's [`View`].
pub fn show(ui: &mut Ui, view: &View<'_>, pass: &mut Pass<'_>) {
    let trail = view.details.map_or(&[][..], |d| d.trail.as_slice());
    let zone = view.origin().map(|a| a.timezone.as_str()).filter(|z| !z.is_empty());
    let place = zone.map(|_| view.code(false)).filter(|p| !p.is_empty());
    let meta = export_meta(view);
    export_menu::feeds_from(ui.ctx(), pass.state);
    // exports are desktop-only in the first native release (plan section 6)
    let export = (!crate::platform::PHONE).then_some(&meta);
    let props = Chart { trail, time_zone: zone, place, ended: view.ended(), export };
    chart(ui, Id::unique("rustradar::chart"), &props, &view.clocks);
}

/// The component's state between passes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Kept {
    table: bool,
    /// The point read (`active`).
    active: Option<usize>,
    /// The pointer was over the plot at the last pass.
    inside: bool,
    pass: u64,
}

/// Where [`chart`] put its parts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub rect: Rect,
    pub header: Rect,
    /// The plot's box (the `.plot` div), when drawn.
    pub plot: Option<Rect>,
    /// The table's box, when shown.
    pub table: Option<Rect>,
    /// The export trigger, the toggle, the zone note.
    pub export: Option<Rect>,
    pub toggle: Option<Rect>,
    pub zone: Option<Rect>,
    /// The read point's box.
    pub tip: Option<Rect>,
    /// The export menu, while open.
    pub menu: Option<Rect>,
}

/// Where a text stands on its baseline point.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Anchor {
    Middle,
    End,
}

/// `text` in `style` with its baseline at `at`, anchored there as `text-anchor` says.
fn text_at(painter: &egui::Painter, text: &str, style: &Style, at: Pos2, anchor: Anchor) {
    let galley = painter.layout_job(text::job(text, style));
    let w = galley.size().x;
    let x = match anchor {
        Anchor::Middle => at.x - w / 2.0,
        Anchor::End => at.x - w,
    };
    // `ui::text`'s hook puts the baseline `css_baseline` under the top, on the nearest pixel
    let top = at.y - style.css_baseline();
    painter.galley(Pos2::new(x, top), galley, style.color);
}

/// The chart across `ui`'s width (see the module documentation), its state kept under `id`, its
/// times read with `clocks`. Answers where it put its parts.
pub fn chart(ui: &mut Ui, id: Id, chart: &Chart<'_>, clocks: &Clocks) -> Placed {
    let ctx = ui.ctx().clone();
    let now_pass = ctx.cumulative_pass_nr();
    let kept_id = id.with("kept");
    let mut kept = ctx.data(|d| d.get_temp::<Kept>(kept_id)).unwrap_or_default();
    // drawn again after a pass without it: the component mounts afresh
    if kept.pass + 1 < now_pass {
        kept = Kept::default();
    }
    kept.pass = now_pass;

    let width = ui.available_width();
    // the plot's `clientWidth`, a whole number
    let client = f64::from(width.round());
    let profile = Profile::new(chart.trail);
    let drawable = profile.drawable(client);
    let zone = chart.time_zone;
    let trigger = (drawable && chart.export.is_some()).then(|| Trigger::new(&ctx));

    // the header's height and baseline
    let small = small_style();
    let line12 = small.line_px();
    let header_h = if trigger.is_some() { export_menu::BLOCK_LINE } else { line12 };
    // CSS's baselines (each box's top is the row's baseline less its own)
    let b12 = small.css_baseline();
    let b11 = zone_style().css_baseline();
    let baseline_off = if trigger.is_some() { strut_style().css_baseline() } else { b12 };

    // what is under the header, and its height
    let table_rows = if drawable && kept.table { table_rows(profile.points.len()) } else { Vec::new() };
    let empty = (!profile.enough()).then(|| ctx.fonts_mut(|f| f.layout_job(text::wrapped(EMPTY, &small, width))));
    let body_h = if let Some(empty) = &empty {
        line12 * empty.rows.len().max(1) as f32
    } else if !drawable {
        0.0
    } else if kept.table {
        table_height(table_rows.len()).min(TABLE_MAX)
    } else {
        HEIGHT as f32
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, header_h + GAP + body_h), Sense::hover());
    let header = Rect::from_min_size(rect.min, Vec2::new(width, header_h));
    let baseline = header.top() + baseline_off;
    let mut placed = Placed { rect, header, plot: None, table: None, export: None, toggle: None, zone: None, tip: None, menu: None };
    let painter = ui.painter().clone();

    // the heading
    let mut job = egui::text::LayoutJob::default();
    text::append(&mut job, HEADING, &heading_style());
    text::append(&mut job, UNIT, &heading_style().weight(theme::WEIGHT_REGULAR).color(theme::TEXT_3));
    let heading = painter.layout_job(job);
    painter.galley(Pos2::new(header.left(), baseline - b12), heading, theme::TEXT_2);

    // the tools, placed from the right (the toggle, the export, the zone note), then made in the
    // page's order (the zone note, the export, the toggle: the order Tab takes them in)
    if drawable {
        let toggle_text = if kept.table { CHART } else { TABLE };
        let toggle_w = ctx.fonts_mut(|f| f.layout_job(text::job(toggle_text, &small))).size().x;
        let toggle = Rect::from_min_size(Pos2::new(header.right() - toggle_w, baseline - b12), Vec2::new(toggle_w, line12));
        placed.toggle = Some(toggle);
        let mut right = toggle.left() - TOOLS_GAP;
        let mut export_at = None;
        if let Some(trigger) = &trigger {
            let w = trigger.width(&ctx);
            let block = Rect::from_min_size(Pos2::new(right - w, header.top()), Vec2::new(w, header_h));
            let button = Rect::from_min_size(Pos2::new(block.left(), baseline - b12), Vec2::new(w, line12));
            export_at = Some((block, button));
            placed.export = Some(button);
            right = block.left() - TOOLS_GAP;
        }
        let t0 = profile.t0();
        let (note, title) = zone_note(chart.place, &clocks.zone_name(t0, zone));
        let zone_galley = painter.layout_job(text::job(&note, &zone_style()));
        let zone_rect = Rect::from_min_size(Pos2::new(right - zone_galley.size().x, baseline - b11), Vec2::new(zone_galley.size().x, zone_style().line_px()));
        painter.galley(zone_rect.min, zone_galley, theme::TEXT_3);
        tooltip::show(ui.interact(zone_rect, id.with("zone"), Sense::hover()), &title);
        placed.zone = Some(zone_rect);
        // the export trigger and its menu
        if let (Some(trigger), Some((block, button)), Some(meta)) = (trigger, export_at, chart.export) {
            placed.menu = trigger.show(ui, block, button, chart.trail, meta);
        }
        let response = ui.interact(toggle, id.with("toggle"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, kept.table, toggle_text));
        let color = if response.hovered() { theme::TEXT_1 } else { theme::TEXT_3 };
        painter.galley(toggle.min, painter.layout_job(text::job(toggle_text, &small.color(color))), color);
        if response.has_focus() {
            focus_ring(&painter, toggle, 0);
        }
        if response.clicked() {
            kept.table = !kept.table;
        }
    }

    // what is under the header
    let body = Rect::from_min_size(Pos2::new(rect.left(), header.bottom() + GAP), Vec2::new(width, body_h));
    if let Some(empty) = empty {
        painter.galley(body.min, empty, theme::TEXT_3);
    } else if drawable && kept.table {
        placed.table = Some(table(ui, id, body, &profile, &table_rows, clocks, zone));
    } else if drawable {
        placed.plot = Some(body);
        placed.tip = plot(ui, id, body, &profile, chart, clocks, &mut kept);
    }
    ctx.data_mut(|d| d.insert_temp(kept_id, kept));
    placed
}

/// The table's natural height with `rows` rows: the header row (16.8 + 4) and the rows (2 + 16.8
/// + 2 each), a 1 px rule over each row (collapsed borders: half in each row they part).
pub fn table_height(rows: usize) -> f32 {
    let th = small_style().line_px() + TH_PAD;
    let td = small_style().line_px() + 2.0 * TD_PAD;
    th + rows as f32 * (td + theme::BORDER)
}

/// The columns' widths across `width`: Chromium's automatic table layout, each column its widest
/// text and the room left over shared in proportion to those.
pub fn column_widths(widest: [f32; 3], width: f32) -> [f32; 3] {
    let sum: f32 = widest.iter().sum();
    if sum <= 0.0 || sum >= width {
        return widest;
    }
    widest.map(|w| w * width / sum)
}

/// The table in `rect` (its height at most [`TABLE_MAX`]); answers its box.
fn table(ui: &mut Ui, id: Id, rect: Rect, profile: &Profile, rows: &[usize], clocks: &Clocks, zone: Option<&str>) -> Rect {
    let ctx = ui.ctx().clone();
    let th = Style::ui(theme::TEXT_SM, 600, theme::TEXT_3);
    let td = Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_1);
    let line = td.line_px();
    let cells: Vec<[String; 3]> = rows
        .iter()
        .map(|&i| {
            let p = &profile.points[i];
            [clocks.clock_at(p.timestamp, zone), feet(p.altitude), format!("{} kt", p.ground_speed)]
        })
        .collect();
    let measure = |s: &str, style: &Style| ctx.fonts_mut(|f| f.layout_job(text::job(s, style))).size().x;
    let mut widest = COLUMNS.map(|c| measure(c, &th));
    for row in &cells {
        for (c, s) in row.iter().enumerate() {
            widest[c] = widest[c].max(measure(s, &td));
        }
    }
    let widths = column_widths(widest, rect.width());
    let natural = table_height(cells.len());
    panel::region(ui, "chart-table", rect, |ui| {
        let scroll = &mut ui.spacing_mut().scroll;
        scroll.floating = true;
        scroll.fade.strength = 0.0;
        ScrollArea::vertical().id_salt(id.with("table")).max_height(TABLE_MAX).auto_shrink([false, true]).show_viewport(ui, |ui, viewport| {
            let (content, _) = ui.allocate_exact_size(Vec2::new(rect.width(), natural), Sense::hover());
            // the rows from the table's own top, as Chromium places them (egui puts the scrolled
            // content on whole pixels)
            let content = Rect::from_min_size(Pos2::new(content.left(), rect.top() - viewport.min.y), content.size());
            let painter = ui.painter();
            let ppp = ctx.pixels_per_point();
            // the x of each column's text end (the first's start)
            let mut edges = [content.left(); 3];
            let mut x = content.left();
            for c in 0..3 {
                x += widths[c];
                edges[c] = if c == 0 { content.left() } else { x };
            }
            let place = |c: usize, w: f32| if c == 0 { edges[0] } else { edges[c] - w };
            let row_top = |k: usize| content.top() + th.line_px() + TH_PAD + theme::BORDER / 2.0 + k as f32 * (line + 2.0 * TD_PAD + theme::BORDER);
            for (k, row) in cells.iter().enumerate() {
                let top = row_top(k);
                let rule = Rect::from_x_y_ranges(content.x_range(), top - theme::BORDER / 2.0..=top + theme::BORDER / 2.0);
                painter.rect_filled(panel::snap_rect(rule, ppp), CornerRadius::ZERO, theme::LINE);
                // the rule's half in the row is rounded down to nothing over the padding, as
                // Chromium lays the collapsed border out
                let text_top = top + TD_PAD;
                for (c, s) in row.iter().enumerate() {
                    let galley = painter.layout_job(text::job(s, &td));
                    painter.galley(Pos2::new(place(c, galley.size().x), text_top), galley, theme::TEXT_1);
                }
            }
            // the header, pinned to the top of what shows
            let top = content.top() + viewport.min.y;
            let bg = Rect::from_x_y_ranges(content.x_range(), top..=top + th.line_px() + TH_PAD);
            painter.rect_filled(panel::snap_rect(bg, ppp), CornerRadius::ZERO, theme::SURFACE);
            for (c, name) in COLUMNS.iter().enumerate() {
                let galley = painter.layout_job(text::job(name, &th));
                painter.galley(Pos2::new(place(c, galley.size().x), top), galley, theme::TEXT_3);
            }
        });
    });
    Rect::from_min_size(rect.min, Vec2::new(rect.width(), natural.min(TABLE_MAX)))
}

/// The plot in `rect` (the `.plot` div), with the point read and its keys; answers the read
/// point's box when there is one.
fn plot(ui: &mut Ui, id: Id, rect: Rect, profile: &Profile, chart: &Chart<'_>, clocks: &Clocks, kept: &mut Kept) -> Option<Rect> {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let points = &profile.points;
    let plot = profile.plot(f64::from(rect.width().round()));
    let zone = chart.time_zone;

    // the pointer and the keyboard (`onpointermove`, `onpointerleave`, `onfocus`, `onblur`,
    // `onkeydown`)
    let response = ui.interact(rect, id.with("plot"), Sense::click());
    let summary = summary(profile, clocks, zone, chart.ended);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, format!("Altitude profile. {summary}")));
    if response.gained_focus() {
        kept.active = Some(points.len() - 1);
    }
    if response.lost_focus() {
        kept.active = None;
    }
    let inside = response.contains_pointer();
    let moved = ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::PointerMoved(_))));
    if inside && moved
        && let Some(pos) = ctx.input(|i| i.pointer.latest_pos())
    {
        kept.active = Some(nearest(points, plot.time_at(f64::from(pos.x - rect.left()))));
    }
    if kept.inside && !inside {
        kept.active = None;
    }
    kept.inside = inside;
    if response.has_focus() {
        let events = ctx.input(|i| i.events.clone());
        let mut taken = vec![false; events.len()];
        for (n, e) in events.iter().enumerate() {
            if let Event::Key { key, pressed: true, modifiers, .. } = e
                && let Some(next) = step(points.len(), kept.active, *key, modifiers.shift)
            {
                kept.active = Some(next);
                taken[n] = true;
            }
        }
        if taken.contains(&true) {
            ctx.input_mut(|i| {
                let mut n = 0;
                i.events.retain(|_| {
                    n += 1;
                    !taken[n - 1]
                });
            });
            ctx.memory_mut(|m| m.move_focus(FocusDirection::None));
        }
    }
    let hovered = kept.active.and_then(|i| points.get(i));

    // the svg, its origin on a whole pixel
    let painter = ui.painter().clone();
    let o = Pos2::new(panel::snap(rect.left(), ppp), panel::snap(rect.top(), ppp));
    let at = |x: f64, y: f64| Pos2::new(o.x + x as f32, o.y + y as f32);
    let ticks = tick_style(theme::TEXT_3);
    for tick in plot.y_ticks() {
        let y = plot.y(tick);
        let grid = Rect::from_min_max(at(LEFT, y), at(plot.width - RIGHT, y + 1.0));
        painter.rect_filled(panel::snap_rect(grid, ppp), CornerRadius::ZERO, theme::LINE);
        text_at(&painter, &group_thousands(tick), &ticks, at(LEFT - 6.0, y + 4.0), Anchor::End);
    }
    for t in x_ticks(&plot, clocks, zone) {
        text_at(&painter, &clocks.clock_at(t, zone), &ticks, at(plot.x(t), TOP + PLOT + 13.0), Anchor::Middle);
    }
    // the area under the line, then the line
    let line: Vec<Pos2> = line(points, &plot).into_iter().map(|p| at(f64::from(p.x), f64::from(p.y))).collect();
    let base = o.y + (TOP + PLOT) as f32;
    let mut mesh = Mesh::default();
    let fill = Color32::from_rgba_unmultiplied_const(theme::SERIES_1.r(), theme::SERIES_1.g(), theme::SERIES_1.b(), theme::alpha(0.1));
    for p in &line {
        mesh.colored_vertex(*p, fill);
        mesh.colored_vertex(Pos2::new(p.x, base), fill);
    }
    for i in 0..line.len().saturating_sub(1) as u32 {
        let (top, bottom, next_top, next_bottom) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        mesh.add_triangle(top, next_top, next_bottom);
        mesh.add_triangle(top, next_bottom, bottom);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(PathShape::line(line.clone(), Stroke::new(LINE, theme::SERIES_1)));
    // round caps
    for end in [line.first(), line.last()].into_iter().flatten() {
        painter.circle_filled(*end, LINE / 2.0, theme::SERIES_1);
    }

    let mut tip = None;
    if let Some(h) = hovered {
        let x = plot.x(h.timestamp as f64);
        painter.line_segment([at(x, TOP), at(x, TOP + PLOT)], Stroke::new(1.0, theme::LINE_CONTROL));
        dot(&painter, at(x, plot.y(f64::from(h.altitude))), theme::SERIES_1);
        tip = Some(reading(&painter, rect, x as f32, plot.width as f32, h, clocks, zone));
    } else if let Some(last) = points.last() {
        let dot = if chart.ended { theme::TEXT_3 } else { theme::SERIES_1 };
        self::dot(&painter, at(plot.x(last.timestamp as f64), plot.y(f64::from(last.altitude))), dot);
        if let Some((x, y)) = end_label(points, &plot) {
            let color = if chart.ended { theme::TEXT_3 } else { theme::TEXT_2 };
            text_at(&painter, &feet(last.altitude), &tick_style(color), at(x, y), Anchor::End);
        }
    }
    if response.has_focus() {
        focus_ring(&painter, rect, theme::RADIUS_SM);
    }
    tip
}

/// A dot of radius [`DOT`] in `color` with its [`RING`] px `--surface` ring centred on its edge,
/// as SVG strokes a circle: the colour shows to `DOT − RING/2`, the ring to `DOT + RING/2`.
fn dot(painter: &egui::Painter, center: Pos2, color: Color32) {
    painter.circle_filled(center, DOT + RING / 2.0, theme::SURFACE);
    painter.circle_filled(center, DOT - RING / 2.0, color);
}

/// The read point's box at the top of the plot `plot` (the `.plot` div), beside the line at `x`
/// in a plot `width` wide; answers its box.
fn reading(painter: &egui::Painter, plot: Rect, x: f32, width: f32, p: &TrailPoint, clocks: &Clocks, zone: Option<&str>) -> Rect {
    let ppp = painter.pixels_per_point();
    let value = painter.layout_job(text::job(&feet(p.altitude), &value_style()));
    let style = reading_style();
    let mut job = egui::text::LayoutJob::default();
    panel::append_clock(&mut job, clocks, p.timestamp, zone, &style);
    text::append(&mut job, " · ", &style);
    text::append(&mut job, &format!("{} kt", p.ground_speed), &panel::data(&style));
    let label = painter.layout_job(job);
    let content = Vec2::new((KEY.x + KEY_GAP + value.size().x).max(label.size().x), value_style().line_px() + TIP_GAP + style.line_px());
    let size = content + 2.0 * TIP_PAD + Vec2::splat(2.0 * theme::BORDER);
    let left = if x > width / 2.0 { plot.left() + x - TIP_OFFSET - size.x } else { plot.left() + x + TIP_OFFSET };
    let rect = Rect::from_min_size(Pos2::new(left, plot.top()), size);
    painter.rect(panel::snap_rect(rect, ppp), CornerRadius::same(theme::RADIUS_SM), theme::SURFACE_RAISED, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
    let inner = rect.min + TIP_PAD + Vec2::splat(theme::BORDER);
    let line1 = value_style().line_px();
    let key = Rect::from_min_size(Pos2::new(inner.x, inner.y + (line1 - KEY.y) / 2.0), KEY);
    painter.rect_filled(panel::snap_rect(key, ppp), CornerRadius::same(1), theme::SERIES_1);
    painter.galley(Pos2::new(inner.x + KEY.x + KEY_GAP, inner.y), value, theme::TEXT_1);
    painter.galley(Pos2::new(inner.x, inner.y + line1 + TIP_GAP), label, theme::TEXT_2);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(timestamp: u64, altitude: i32) -> TrailPoint {
        TrailPoint { timestamp, altitude, ground_speed: 400, ..Default::default() }
    }

    #[test]
    fn nice_scales_have_at_most_four_steps() {
        assert_eq!(nice_scale(1_120.0), Scale { top: 1_500.0, step: 500.0 });
        assert_eq!(nice_scale(40_320.0), Scale { top: 45_000.0, step: 15_000.0 });
        assert_eq!(nice_scale(42_560.0), Scale { top: 45_000.0, step: 15_000.0 });
        assert_eq!(nice_scale(10_000.0), Scale { top: 10_000.0, step: 2_500.0 });
        assert_eq!(nice_scale(200.0), Scale { top: 250.0, step: 250.0 });
        assert_eq!(nice_scale(90_000.0), Scale { top: 100_000.0, step: 20_000.0 }, "past four of the largest step");
        let profile = Profile::new(&[pt(10, 36_000), pt(20, 0)]);
        assert_eq!(profile.scale(), Scale { top: 45_000.0, step: 15_000.0 });
        assert_eq!(profile.plot(328.0).y_ticks(), [0.0, 15_000.0, 30_000.0, 45_000.0]);
        // a low flight still gets a 1,000 ft ceiling's worth of scale
        assert_eq!(Profile::new(&[pt(1, 50), pt(2, 80)]).scale(), Scale { top: 1_500.0, step: 500.0 });
    }

    #[test]
    fn points_without_a_time_are_dropped_and_the_rest_sorted() {
        let profile = Profile::new(&[pt(30, 3), pt(0, 9), pt(10, 1), pt(20, 2), pt(10, 5)]);
        let order: Vec<(u64, i32)> = profile.points.iter().map(|p| (p.timestamp, p.altitude)).collect();
        assert_eq!(order, [(10, 1), (10, 5), (20, 2), (30, 3)], "a stable sort");
        assert!(profile.enough());
        assert!(profile.drawable(55.0) && !profile.drawable(54.0));
        assert!(!Profile::new(&[pt(10, 1)]).enough());
        assert!(!Profile::new(&[pt(10, 1), pt(10, 2)]).enough(), "no time between them");
        assert_eq!(profile.highest().map(|p| p.timestamp), Some(10), "the first of the highest");
    }

    #[test]
    fn coordinates_ticks_and_the_line() {
        let profile = Profile::new(&(0..60).map(|i| pt(1_782_034_980 + 52 * i, (1_400 * i as i32).min(36_000))).collect::<Vec<_>>());
        let plot = profile.plot(328.0);
        assert_eq!(plot.x(plot.t0), 46.0);
        assert_eq!(plot.x(plot.t1), 320.0);
        assert_eq!(plot.y(0.0), 114.0);
        assert_eq!(plot.y(-500.0), 114.0, "nothing below the ground");
        assert_eq!(plot.y(45_000.0), 18.0);
        // the line at a tenth of a pixel, as Chromium was given it
        let line = line(&profile.points, &plot);
        assert_eq!((line[0], line[1], line[2], line[4]), (Pos2::new(46.0, 114.0), Pos2::new(50.6, 111.0), Pos2::new(55.3, 108.0), Pos2::new(64.6, 102.1)));
        assert_eq!(line[59], Pos2::new(320.0, 37.2));
        // ticks every 15 minutes on Stockholm's round quarters (11:43 to 12:34 CEST)
        let clocks = Clocks::default();
        let ticks = x_ticks(&plot, &clocks, Some("Europe/Stockholm"));
        let labels: Vec<String> = ticks.iter().map(|&t| clocks.clock_at(t, Some("Europe/Stockholm"))).collect();
        assert_eq!(labels, ["12:00", "12:15", "12:30"]);
        assert!((plot.x(ticks[0]) - 137.095_176_010_430_24).abs() < 1e-9, "where Chromium centred 12:00");
        // the same in UTC: two hours earlier on the clock
        let labels: Vec<String> = x_ticks(&plot, &clocks, None).iter().map(|&t| clocks.clock_at(t, None)).collect();
        assert_eq!(labels, ["10:00", "10:15", "10:30"]);
        // five hours: every two hours, the one at 02:00 too close to the start to show
        let long = Profile::new(&(0..300).map(|i| pt(1_782_000_000 + 60 * i, 38_000)).collect::<Vec<_>>());
        let plot = long.plot(328.0);
        let labels: Vec<String> = x_ticks(&plot, &clocks, Some("Europe/Stockholm")).iter().map(|&t| clocks.clock_at(t, Some("Europe/Stockholm"))).collect();
        assert_eq!(labels, ["04:00", "06:00"]);
    }

    #[test]
    fn the_end_label_takes_the_side_the_line_does_not_come_from() {
        let climb: Vec<TrailPoint> = (0..60).map(|i| pt(1_000 + 52 * i, (1_400 * i as i32).min(36_000))).collect();
        let profile = Profile::new(&climb);
        let plot = profile.plot(328.0);
        let (x, y) = end_label(&profile.points, &plot).unwrap();
        assert_eq!(x, 311.0);
        assert!((y - 29.2).abs() < 1e-9, "over the dot: {y}");
        // descending with room under it
        let descend: Vec<TrailPoint> = (0..45).map(|i| pt(1_000 + 40 * i, 36_000 - 350 * i as i32)).collect();
        let profile = Profile::new(&descend);
        let plot = profile.plot(328.0);
        let (_, y) = end_label(&profile.points, &plot).unwrap();
        assert!((y - (plot.y(20_600.0) + 16.0)).abs() < 1e-9);
        // descending near the ground: none
        let low: Vec<TrailPoint> = (0..45).map(|i| pt(1_000 + 40 * i, 36_000 - 800 * i as i32)).collect();
        let profile = Profile::new(&low);
        assert_eq!(end_label(&profile.points, &profile.plot(328.0)), None);
    }

    #[test]
    fn the_nearest_point_and_the_keys() {
        let points: Vec<TrailPoint> = [10, 20, 40, 80].map(|t| pt(t, 0)).to_vec();
        assert_eq!(nearest(&points, 10.0), 0);
        assert_eq!(nearest(&points, 15.0), 0, "the earlier of two as near");
        assert_eq!(nearest(&points, 16.0), 1);
        assert_eq!(nearest(&points, 61.0), 3);
        assert_eq!(nearest(&points, 80.0), 3);
        assert_eq!(step(4, None, Key::ArrowLeft, false), Some(2), "from the last point");
        assert_eq!(step(4, Some(0), Key::ArrowLeft, false), Some(0));
        assert_eq!(step(4, Some(3), Key::ArrowRight, false), Some(3));
        assert_eq!(step(60, Some(10), Key::ArrowRight, true), Some(16), "a tenth of the points");
        assert_eq!(step(4, Some(2), Key::ArrowRight, true), Some(3), "at least one");
        assert_eq!(step(60, Some(10), Key::Home, false), Some(0));
        assert_eq!(step(60, Some(10), Key::End, false), Some(59));
        assert_eq!(step(60, Some(10), Key::ArrowUp, false), None);
        // the pointer's time, held to the span
        let plot = Profile::new(&points).plot(328.0);
        assert_eq!(plot.time_at(0.0), 10.0);
        assert_eq!(plot.time_at(400.0), 80.0);
        assert_eq!(plot.time_at(46.0 + 274.0 / 2.0), 45.0);
    }

    #[test]
    fn table_rows_words_and_columns() {
        assert_eq!(table_rows(5), [0, 1, 2, 3, 4]);
        assert_eq!(table_rows(60).len(), 31, "every second point and the last");
        assert_eq!(table_rows(60)[30], 59);
        assert_eq!(table_rows(300).len(), 31);
        assert_eq!(table_rows(61).len(), 21, "every third point, the last among them");
        assert_eq!(table_rows(61)[20], 60);
        assert_eq!(table_rows(0), Vec::<usize>::new());
        assert!((table_height(31) - 696.6).abs() < 1e-3, "{}", table_height(31));
        assert_eq!(column_widths([36.0, 66.0, 44.0], 292.0), [72.0, 132.0, 88.0]);
        assert_eq!(column_widths([200.0, 200.0, 0.0], 100.0), [200.0, 200.0, 0.0], "no room to share");
        assert_eq!(feet(36_000), "36,000 ft");
        assert_eq!(feet(-120), "0 ft");
        assert_eq!(zone_note(Some("ARN"), "CEST"), ("ARN time".to_owned(), "Times are local to ARN (CEST)".to_owned()));
        assert_eq!(zone_note(None, "UTC").0, "Your time, UTC");
        assert_eq!(zone_note(Some(""), "UTC").1, "Times are in your time zone (UTC); the departure airport's isn't known");
        let profile = Profile::new(&(0..60).map(|i| pt(1_782_034_980 + 52 * i, (1_400 * i as i32).min(36_000))).collect::<Vec<_>>());
        assert_eq!(
            summary(&profile, &Clocks::default(), Some("Europe/Stockholm"), false),
            "Altitude since 11:43 CEST: highest 36,000 ft at 12:05, now 36,000 ft."
        );
        assert!(summary(&profile, &Clocks::default(), None, true).ends_with("last reported 36,000 ft."));
    }
}
