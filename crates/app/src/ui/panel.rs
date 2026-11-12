//! The detail panels' shared anatomy (doc 06 §4; `FlightPanel.svelte`, `TrainPanel.svelte`,
//! `ShipPanel.svelte`, `AirportPanel.svelte`): the parts all four are built from, laid out as
//! Chromium lays out today's CSS.
//!
//! - **Body** ([`body`]): the panel's content scrolls inside the area the shell places
//!   ([`super::shell::panel`]: 360 px wide, at most the window's height less 44), its footer
//!   pinned to the bottom (`position: sticky; bottom: 0`): the footer stands after the content
//!   when all of it fits, else at the bottom of the panel over the content, which scrolls under it
//!   up to the footer's own place. The scroll bar floats over the content's right edge, as
//!   Chromium's overlay bar does in the reference pictures, so the content keeps its 360 px.
//! - **Strip** ([`strip`], "ATC flight-progress strip"): a grid of cells, `1fr` ([`Cell::fill`])
//!   or `auto` ([`Cell::auto`]) wide, then the 40 px close column; 1 px `--line` rules between
//!   the cells and under the row. A cell is a column of [`Piece`]s 2 px apart, centred on the row,
//!   padded 12 (the first 16 on the left, with no rule). The close button is `.icon-button`'s
//!   `X` 18/1.75, `Close (Esc)`, centred in its column.
//! - **Facts** ([`facts`]): `dl.facts`, a grid of equal columns (3; 2 in the train panel) 12 px
//!   apart (`.compact`: rows 8 apart), items placed as CSS's sparse auto-placement places them
//!   ([`Span`]: one column, two, or the whole row); `dt` 12 px `--text-3`, `dd` 2 px under it at
//!   13 px, wrapping. Each item's `title` is its tooltip.
//! - **Photo** ([`photo`], [`photo_skeleton`]): the full-width block at 2:1 (5:2 short) on
//!   `--surface-raised` (while loading, that block alone, rounded 4), the image covering it
//!   (`object-fit: cover`), a click opening it; the credit at right 8, bottom 4 (11 px
//!   `--text-1` over its shadow); with more than one picture, the round 28 px previous and next
//!   buttons (`ChevronLeft`/`ChevronRight` 18/2 on `oklch(0.15 0 0 / 0.7)`) at left and right 8,
//!   shown over 150 ms while the block is pointed at or a button holds the keyboard, and the count
//!   `n/total` (Mono 11 px) at left 8.
//! - **Footer** ([`Footer`]): a wrapping row, padding `12px 16px`, a 1 px rule on top, the status
//!   on the left and the buttons on the right 8 apart; [`follow_button`] is the vehicle panels'
//!   Follow.
//! - **Text**: [`append_clock`] is `<Clock>` (the time in Mono, a word space, the zone's name in
//!   Overpass `--text-3`, or a dash); [`title_style`], [`value_style`] and [`sub_style`] are the
//!   strip's `h2`, `.value` and `p`/`.sub`.
//! - **Remote images** ([`remote`]): through egui's image loaders (`Context::try_load_texture`):
//!   `crate::images` (U18) for `http:` and `https:` URIs, from the network, or from the
//!   `--fixture` folder's files (F2; a shot never asks the network). One that fails (or that no
//!   loader takes) reads as failed: a photo shows its `--surface-raised` block, an airline logo is
//!   left out, as today when they do not load.
//!
//! Boxes, borders and rules are put on whole device pixels, as Chromium snaps a box's edges;
//! heights add up CSS's line boxes (egui rounds a text row to whole pixels; the panel uses the
//! exact line heights so the parts land where Chromium puts them).
//!
//! Deviations: the credit's and count's `text-shadow: 0 1px 2px` is drawn unblurred, 1 px down,
//! at half its alpha; the scroll bar is egui's floating bar (Chromium's overlay bar in today's
//! pictures; a classic one on Windows' WebView2 would take 11 px of the width).

use std::sync::Arc;

use egui::epaint::Galley;
use egui::load::{SizedTexture, TexturePoll};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, Context, CornerRadius, CursorIcon, Id, Layout, Painter, Pos2, Rangef, Rect, Response, ScrollArea, Sense, SizeHint,
    TextureOptions, Ui, UiBuilder, Vec2, Widget,
};
use rustradar_core::time::{Clocks, UnixTime};

use super::fonts::Face;
use super::icons::{self, Icon};
use super::text::{self, Style};
use super::widgets::button::Button;
use super::widgets::icon_button::IconButton;
use super::widgets::{skeleton, tooltip};
use super::{anim, theme};

/// The strip's last column, which holds the close button (`grid-template-columns: … 40px`).
pub const CLOSE_COLUMN: f32 = 40.0;
/// The close button's icon size (`X` 18/1.75).
pub const CLOSE_ICON: f32 = 18.0;
/// The close button's tooltip, every panel's (`title="Close (Esc)"`).
pub const CLOSE_TOOLTIP: &str = "Close (Esc)";
/// A strip cell's padding (`padding: var(--space-3)`).
pub const CELL_PAD: f32 = theme::SPACE_3;
/// The first cell's left padding (`.cell.id { padding-left: var(--space-4) }`).
pub const CELL_PAD_FIRST: f32 = theme::SPACE_4;
/// The gap between a cell's lines.
pub const CELL_GAP: f32 = 2.0;
/// `h2`'s line height.
pub const TITLE_LINE_HEIGHT: f32 = 1.1;
/// The facts' gaps (`gap: var(--space-3)`), and `.compact`'s row gap.
pub const FACTS_GAP: f32 = theme::SPACE_3;
pub const FACTS_ROW_GAP_COMPACT: f32 = theme::SPACE_2;
/// `dd`'s `margin-top`.
pub const DD_GAP: f32 = 2.0;
/// The photo's aspect ratio (`2 / 1`), and the short one (`5 / 2`).
pub const PHOTO_ASPECT: f32 = 2.0;
pub const PHOTO_ASPECT_SHORT: f32 = 2.5;
/// The photo's previous and next buttons: side, inset, icon.
pub const PHOTO_NAV: f32 = 28.0;
pub const PHOTO_INSET: f32 = theme::SPACE_2;
pub const PHOTO_NAV_ICON: f32 = 18.0;
/// The credit's and count's distance from the photo's bottom (`bottom: var(--space-1)`).
pub const PHOTO_TEXT_BOTTOM: f32 = theme::SPACE_1;
/// The footer's padding (`padding: var(--space-3) var(--space-4)`) and gap.
pub const FOOTER_PAD: Vec2 = Vec2::new(theme::SPACE_4, theme::SPACE_3);
pub const FOOTER_GAP: f32 = theme::SPACE_2;
/// The footer's height in one row with 32 px buttons: 12 + 32 + 12 and the rule.
pub const FOOTER_HEIGHT: f32 = 57.0;
/// The Follow button's tooltips.
pub const FOLLOW_TOOLTIP: &str = "Follow with the camera (C)";
pub const STOP_FOLLOWING_TOOLTIP: &str = "Stop following (C)";
/// Its text and accessible name.
pub const FOLLOW: &str = "Follow";

/// `h2` in the strip: 22 px at 700, line height 1.1, no letter spacing (also on `.data`).
pub const fn title_style(face: Face) -> Style {
    Style { face, size: theme::TEXT_XL, weight: 700, color: theme::TEXT_1, line_height: TITLE_LINE_HEIGHT, letter_spacing: 0.0 }
}

/// `.data.value`: Mono 14 px at 650.
pub const fn value_style(color: Color32) -> Style {
    Style::data(theme::TEXT_BASE, 650, color)
}

/// `.strip p` (Overpass) and `.sub` (Mono with `.data`): 12 px `--text-2`.
pub const fn sub_style(face: Face) -> Style {
    match face {
        Face::Mono => Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2),
        Face::Ui => Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2),
    }
}

/// `dt`: 12 px `--text-3`.
pub const fn dt_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// `dd`: 13 px in `face` (Mono with `.data`), `--text-1`, or `--text-3` when `dim`.
pub const fn dd_style(face: Face, dim: bool) -> Style {
    let color = if dim { theme::TEXT_3 } else { theme::TEXT_1 };
    match face {
        Face::Mono => Style::data(theme::TEXT_MD, theme::WEIGHT_REGULAR, color),
        Face::Ui => Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, color),
    }
}

/// `.note`: 11 px `--text-3`.
pub const fn note_style() -> Style {
    Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

// ------------------------------------------------------------------------------------------------
// pixels

/// `v` on the device pixel grid at `ppp` pixels per point (Chromium paints a box's edges on whole
/// pixels).
pub fn snap(v: f32, ppp: f32) -> f32 {
    (v * ppp).round() / ppp
}

/// `rect` with its edges on the device pixel grid.
pub fn snap_rect(rect: Rect, ppp: f32) -> Rect {
    Rect::from_min_max(Pos2::new(snap(rect.min.x, ppp), snap(rect.min.y, ppp)), Pos2::new(snap(rect.max.x, ppp), snap(rect.max.y, ppp)))
}

/// A 1 px `--line` rule across `x` whose top is at `y` (a border: `border-top`, or a
/// `border-bottom` whose top is the box's bottom less 1).
pub fn hrule(painter: &Painter, x: impl Into<Rangef>, y: f32) {
    let rect = Rect::from_x_y_ranges(x.into(), y..=y + theme::BORDER);
    painter.rect_filled(snap_rect(rect, painter.pixels_per_point()), CornerRadius::ZERO, theme::LINE);
}

/// A 1 px `--line` rule down `y` whose left is at `x` (`border-left`).
pub fn vrule(painter: &Painter, x: f32, y: impl Into<Rangef>) {
    let rect = Rect::from_x_y_ranges(x..=x + theme::BORDER, y.into());
    painter.rect_filled(snap_rect(rect, painter.pixels_per_point()), CornerRadius::ZERO, theme::LINE);
}

// ------------------------------------------------------------------------------------------------
// text

/// `style` in the data face (`.data`: Overpass Mono, `letter-spacing: -0.01em`).
pub const fn data(style: &Style) -> Style {
    Style { face: Face::Mono, letter_spacing: theme::LETTER_SPACING_TIGHT, ..*style }
}

/// `<Clock unix timeZone>` (`Clock.svelte`) appended to `job` inside text of `style`: the time
/// (`clockAt`) in the data face, a word space in the text's own face, and the zone's name
/// (`zoneName`) in Overpass `--text-3` with no letter spacing; a dash in the data face without a
/// time. `zone` none or empty is the viewer's zone (`clocks`).
pub fn append_clock(job: &mut LayoutJob, clocks: &Clocks, unix: impl UnixTime, zone: Option<&str>, style: &Style) {
    match unix.unix() {
        Some(unix) => {
            text::append(job, &clocks.clock_at(unix, zone), &data(style));
            text::append(job, " ", style);
            let zone_style = Style { face: Face::Ui, letter_spacing: 0.0, color: theme::TEXT_3, ..*style };
            text::append(job, &clocks.zone_name(unix, zone), &zone_style);
        }
        None => text::append(job, rustradar_core::time::DASH, &data(style)),
    }
}

/// How a text fits the width it is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Wraps at word boundaries (and inside a word wider than the width).
    Wrap,
    /// One row, cut with `…` (`white-space: nowrap; overflow: hidden; text-overflow: ellipsis`).
    Ellipsis,
    /// At most this many rows, the last cut with `…` (`-webkit-line-clamp`).
    Clamp(usize),
}

/// `job` made to fit `width` as `fit` says.
pub fn fit(mut job: LayoutJob, fit: Fit, width: f32) -> LayoutJob {
    let width = width.max(0.0);
    job.wrap = match fit {
        Fit::Wrap => TextWrapping::wrap_at_width(width),
        Fit::Ellipsis => TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some(text::ELLIPSIS) },
        Fit::Clamp(rows) => TextWrapping { max_width: width, max_rows: rows, break_anywhere: false, overflow_character: Some(text::ELLIPSIS) },
    };
    job
}

/// What paints a [`Piece::Custom`] in its rectangle.
pub type PaintFn<'a> = Box<dyn FnOnce(&Painter, Rect) + 'a>;

/// A line of a strip cell, a fact's label or value, a footer's status.
pub enum Piece<'a> {
    /// A text: its job (faces and colours in its sections), its CSS line height (px) and how it
    /// fits the width.
    Text { job: LayoutJob, line: f32, fit: Fit },
    /// A `.skeleton` of this size.
    Skeleton(Vec2),
    /// Anything else of this size, painted by the function in its rectangle.
    Custom(Vec2, PaintFn<'a>),
}

impl std::fmt::Debug for Piece<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Piece::Text { job, fit, .. } => write!(f, "Text({:?}, {fit:?})", job.text),
            Piece::Skeleton(size) => write!(f, "Skeleton({size:?})"),
            Piece::Custom(size, _) => write!(f, "Custom({size:?})"),
        }
    }
}

impl<'a> Piece<'a> {
    /// `text` in `style`, fitted as `fit` says.
    pub fn text(text: &str, style: &Style, fit: Fit) -> Piece<'a> {
        Piece::Text { job: text::job(text, style), line: style.line_px(), fit }
    }

    /// A built job whose line box is `line` px.
    pub fn job(job: LayoutJob, line: f32, fit: Fit) -> Piece<'a> {
        Piece::Text { job, line, fit }
    }

    /// The text of a text piece.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Piece::Text { job, .. } => Some(&job.text),
            _ => None,
        }
    }

    /// Its width without any limit (CSS's max-content), px.
    pub fn natural_width(&self, ctx: &Context) -> f32 {
        match self {
            Piece::Text { job, .. } => text::measure(ctx, job.clone()).x,
            Piece::Skeleton(size) | Piece::Custom(size, _) => size.x,
        }
    }

    /// Laid out `width` px wide.
    pub fn lay_out(self, ctx: &Context, width: f32) -> Laid<'a> {
        match self {
            Piece::Text { job, line, fit: f } => {
                let galley = ctx.fonts_mut(|fonts| fonts.layout_job(fit(job, f, width)));
                let size = Vec2::new(galley.size().x, line * galley.rows.len().max(1) as f32);
                Laid { size, kind: LaidKind::Text(galley) }
            }
            Piece::Skeleton(size) => Laid { size, kind: LaidKind::Skeleton },
            Piece::Custom(size, paint) => Laid { size, kind: LaidKind::Custom(paint) },
        }
    }
}

/// A [`Piece`] laid out: its size and what to paint.
pub struct Laid<'a> {
    pub size: Vec2,
    kind: LaidKind<'a>,
}

enum LaidKind<'a> {
    Text(Arc<Galley>),
    Skeleton,
    Custom(PaintFn<'a>),
}

impl Laid<'_> {
    /// Paints it with its top left corner at `at`; its text rows `line` px apart where egui would
    /// round them.
    pub fn paint(self, painter: &Painter, at: Pos2) {
        let rect = Rect::from_min_size(at, self.size);
        match self.kind {
            LaidKind::Text(galley) => {
                let rows = galley.rows.len().max(1) as f32;
                // centre egui's whole-pixel rows on the CSS line boxes
                let top = at.y + (self.size.y - galley.size().y) / 2.0 * (1.0 / rows).min(1.0);
                painter.galley(Pos2::new(at.x, top), galley, theme::TEXT_1);
            }
            LaidKind::Skeleton => skeleton::paint(painter, snap_rect(rect, painter.pixels_per_point()), theme::RADIUS),
            LaidKind::Custom(paint) => paint(painter, rect),
        }
    }
}

// ------------------------------------------------------------------------------------------------
// body

/// Which part of the panel [`body`] asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Part {
    /// Everything above the footer, laid out top-down from the content's top.
    Content,
    /// The footer, in this rectangle (where it stands now: after the content, or pinned to the
    /// bottom of the panel).
    Footer(Rect),
}

/// Floating scroll bars: over the content's right edge, the content keeping its width; no fade
/// at the edges (egui's hint of more content, which CSS does not draw).
fn floating_scroll(ui: &mut Ui) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating = true;
    scroll.fade.strength = 0.0;
}

/// Where [`body`] stands: the panel's visible rectangle and how far its content is scrolled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scrolled {
    pub rect: Rect,
    /// px from the content's top.
    pub offset: f32,
}

/// The panel's scrolling body (see the module documentation): `add` is asked for the
/// [`Part::Content`], then for the [`Part::Footer`], `footer_height` px high, in its place.
pub fn body(ui: &mut Ui, id: Id, footer_height: f32, mut add: impl FnMut(&mut Ui, Part)) -> Scrolled {
    let width = ui.available_width();
    let max_height = ui.max_rect().height().min(ui.available_height());
    ui.scope(|ui| {
        floating_scroll(ui);
        let output = ScrollArea::vertical()
            .id_salt(id.with("scroll"))
            .auto_shrink([false, true])
            .max_height(max_height)
            .show_viewport(ui, |ui, viewport| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                add(ui, Part::Content);
                let top = ui.max_rect().top();
                let (natural, _) = ui.allocate_exact_size(Vec2::new(width, footer_height), Sense::hover());
                // sticky: no lower than the visible bottom, never above its own place
                let y = natural.top().min(top + viewport.max.y - footer_height).max(top);
                add(ui, Part::Footer(Rect::from_min_size(Pos2::new(natural.left(), y), natural.size())));
            });
        Scrolled { rect: output.inner_rect, offset: output.state.clamped_offset().y }
    })
    .inner
}

// ------------------------------------------------------------------------------------------------
// strip

/// A cell of the [`strip`].
pub struct Cell<'a> {
    pub pieces: Vec<Piece<'a>>,
    /// `1fr` (`min-width: 0`), else `auto`.
    pub fill: bool,
    /// Its `title`.
    pub tooltip: Option<String>,
}

impl<'a> Cell<'a> {
    /// A `1fr` cell (it takes what the `auto` cells and the close column leave).
    pub fn fill(pieces: Vec<Piece<'a>>) -> Cell<'a> {
        Cell { pieces, fill: true, tooltip: None }
    }

    /// An `auto` cell (as wide as its widest piece).
    pub fn auto(pieces: Vec<Piece<'a>>) -> Cell<'a> {
        Cell { pieces, fill: false, tooltip: None }
    }

    /// Its tooltip.
    pub fn tooltip(mut self, text: Option<String>) -> Cell<'a> {
        self.tooltip = text;
        self
    }
}

/// The horizontal padding and border of cell `i`.
fn cell_frame(i: usize) -> f32 {
    if i == 0 { CELL_PAD_FIRST + CELL_PAD } else { 2.0 * CELL_PAD + theme::BORDER }
}

/// The strip's cells' widths across `width` (border boxes): the `auto` cells as wide as their
/// widest piece, the `1fr` cells sharing what is left after them and the close column.
pub fn strip_widths(ctx: &Context, cells: &[Cell<'_>], width: f32) -> Vec<f32> {
    let autos: Vec<Option<f32>> = cells
        .iter()
        .enumerate()
        .map(|(i, c)| (!c.fill).then(|| c.pieces.iter().map(|p| p.natural_width(ctx)).fold(0.0, f32::max) + cell_frame(i)))
        .collect();
    let fixed: f32 = autos.iter().flatten().sum::<f32>() + CLOSE_COLUMN;
    let fills = autos.iter().filter(|a| a.is_none()).count().max(1) as f32;
    let share = ((width - fixed) / fills).max(0.0);
    autos.into_iter().map(|a| a.unwrap_or(share)).collect()
}

/// The strip (see the module documentation) across the available width, the close button named
/// `close_label`; answers the close button's response.
pub fn strip(ui: &mut Ui, cells: Vec<Cell<'_>>, close_label: &str) -> Response {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let widths = strip_widths(&ctx, &cells, width);
    let mut laid = Vec::new();
    for (i, (cell, w)) in cells.into_iter().zip(&widths).enumerate() {
        let content = (w - cell_frame(i)).max(0.0);
        let pieces: Vec<Laid<'_>> = cell.pieces.into_iter().map(|p| p.lay_out(&ctx, content)).collect();
        let height = pieces.iter().map(|p| p.size.y).sum::<f32>() + CELL_GAP * pieces.len().saturating_sub(1) as f32;
        laid.push((pieces, height, cell.tooltip));
    }
    let content_h = laid.iter().map(|(_, h, _)| *h).fold(0.0, f32::max);
    let row = content_h + 2.0 * CELL_PAD;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, row + theme::BORDER), Sense::hover());
    let painter = ui.painter().clone();
    let mut x = rect.left();
    for (i, ((pieces, height, tip), w)) in laid.into_iter().zip(&widths).enumerate() {
        let cell = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(*w, row));
        if i > 0 {
            vrule(&painter, x, rect.top()..=rect.top() + row);
        }
        let left = x + if i == 0 { CELL_PAD_FIRST } else { theme::BORDER + CELL_PAD };
        let mut y = rect.top() + CELL_PAD + (content_h - height) / 2.0;
        for piece in pieces {
            let h = piece.size.y;
            piece.paint(&painter, Pos2::new(left, y));
            y += h + CELL_GAP;
        }
        if let Some(tip) = tip {
            tooltip::show(ui.interact(cell, ui.scope_id().with(("strip-cell", i)), Sense::hover()), &tip);
        }
        x += w;
    }
    hrule(&painter, rect.x_range(), rect.top() + row);
    let close = Rect::from_center_size(Pos2::new(rect.right() - CLOSE_COLUMN / 2.0, rect.top() + row / 2.0), Vec2::splat(32.0));
    put(ui, "strip-close", close, IconButton::of(Icon::X, CLOSE_ICON, close_label).tooltip(CLOSE_TOOLTIP))
}

// ------------------------------------------------------------------------------------------------
// facts

/// How many columns a fact takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    One,
    /// `grid-column: span 2`.
    Two,
    /// `.wide`: `grid-column: 1 / -1`.
    All,
}

/// An item of the [`facts`] grid: `dt` and `dd`.
pub struct Fact<'a> {
    pub label: Piece<'a>,
    pub value: Piece<'a>,
    pub span: Span,
    /// The item's `title`.
    pub tooltip: Option<String>,
}

impl<'a> Fact<'a> {
    /// `label` (12 px `--text-3`) over `value`.
    pub fn new(label: &str, value: Piece<'a>) -> Fact<'a> {
        Fact { label: Piece::text(label, &dt_style(), Fit::Wrap), value, span: Span::One, tooltip: None }
    }

    /// Spanning `span` columns.
    pub fn span(mut self, span: Span) -> Fact<'a> {
        self.span = span;
        self
    }

    /// The whole row when `wide`.
    pub fn wide(self, wide: bool) -> Fact<'a> {
        if wide { self.span(Span::All) } else { self }
    }

    /// Its tooltip.
    pub fn tooltip(mut self, text: Option<impl Into<String>>) -> Fact<'a> {
        self.tooltip = text.map(Into::into);
        self
    }
}

/// Where CSS's sparse auto-placement puts items of these spans in `columns` columns: `(row,
/// column, span)` each.
pub fn grid_places(spans: &[Span], columns: usize) -> Vec<(usize, usize, usize)> {
    let columns = columns.max(1);
    let (mut row, mut col) = (0, 0);
    spans
        .iter()
        .map(|span| {
            let n = match span {
                Span::One => 1,
                Span::Two => 2.min(columns),
                Span::All => columns,
            };
            if col + n > columns {
                row += 1;
                col = 0;
            }
            let at = (row, col, n);
            col += n;
            at
        })
        .collect()
}

/// `dl.facts` in `columns` columns across the available width, rows `row_gap` apart (12, or 8
/// when `.compact`); answers its rectangle.
pub fn facts(ui: &mut Ui, columns: usize, row_gap: f32, items: Vec<Fact<'_>>) -> Rect {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let columns = columns.max(1);
    let col_w = (width - FACTS_GAP * (columns - 1) as f32) / columns as f32;
    let spans: Vec<Span> = items.iter().map(|f| f.span).collect();
    let places = grid_places(&spans, columns);
    let mut laid = Vec::new();
    for (item, &(row, col, n)) in items.into_iter().zip(&places) {
        let w = col_w * n as f32 + FACTS_GAP * (n - 1) as f32;
        let label = item.label.lay_out(&ctx, w);
        let value = item.value.lay_out(&ctx, w);
        let h = label.size.y + DD_GAP + value.size.y;
        laid.push((row, col, w, label, value, h, item.tooltip));
    }
    let rows = places.last().map_or(0, |p| p.0 + 1);
    let mut heights = vec![0.0f32; rows];
    for (row, _, _, _, _, h, _) in &laid {
        heights[*row] = heights[*row].max(*h);
    }
    let total = heights.iter().sum::<f32>() + row_gap * rows.saturating_sub(1) as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, total), Sense::hover());
    let tops: Vec<f32> = heights.iter().scan(rect.top(), |y, h| {
        let top = *y;
        *y += h + row_gap;
        Some(top)
    }).collect();
    let painter = ui.painter().clone();
    for (i, (row, col, w, label, value, _, tip)) in laid.into_iter().enumerate() {
        let left = rect.left() + (col_w + FACTS_GAP) * col as f32;
        let top = tops[row];
        let label_h = label.size.y;
        label.paint(&painter, Pos2::new(left, top));
        value.paint(&painter, Pos2::new(left, top + label_h + DD_GAP));
        if let Some(tip) = tip {
            let item = Rect::from_min_size(Pos2::new(left, top), Vec2::new(w, heights[row]));
            tooltip::show(ui.interact(item, ui.scope_id().with(("fact", i)), Sense::hover()), &tip);
        }
    }
    rect
}

// ------------------------------------------------------------------------------------------------
// images

/// A remote image as egui's loaders have it now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Remote {
    /// Still loading.
    Loading,
    /// Loaded: its texture and its size in texels (CSS px at scale 1, as `<img>`'s natural size).
    Ready(SizedTexture),
    /// It did not load, or nothing loads `uri`s of its kind (`onerror`).
    Failed,
}

/// `uri` through egui's image loaders (see the module documentation).
pub fn remote(ctx: &Context, uri: &str) -> Remote {
    if uri.is_empty() {
        return Remote::Failed;
    }
    match ctx.try_load_texture(uri, TextureOptions::LINEAR, SizeHint::default()) {
        Ok(TexturePoll::Ready { texture }) => Remote::Ready(texture),
        Ok(TexturePoll::Pending { .. }) => Remote::Loading,
        Err(_) => Remote::Failed,
    }
}

/// The texture coordinates that make an image of `image` size cover a box of `size`, centred
/// (`object-fit: cover`, `object-position: 50% 50%`).
pub fn cover_uv(image: Vec2, size: Vec2) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 || size.x <= 0.0 || size.y <= 0.0 {
        return Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    }
    let (image_aspect, box_aspect) = (image.x / image.y, size.x / size.y);
    if image_aspect > box_aspect {
        let u = box_aspect / image_aspect;
        Rect::from_min_max(Pos2::new((1.0 - u) / 2.0, 0.0), Pos2::new((1.0 + u) / 2.0, 1.0))
    } else {
        let v = image_aspect / box_aspect;
        Rect::from_min_max(Pos2::new(0.0, (1.0 - v) / 2.0), Pos2::new(1.0, (1.0 + v) / 2.0))
    }
}

/// The size an image of natural size `image` is drawn at under `max-width`/`max-height`, its
/// aspect kept.
pub fn contain(image: Vec2, max: Vec2) -> Vec2 {
    if image.x <= 0.0 || image.y <= 0.0 {
        return Vec2::ZERO;
    }
    image * (max.x / image.x).min(max.y / image.y).min(1.0)
}

// ------------------------------------------------------------------------------------------------
// photo

/// A photo block's picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Photo<'a> {
    /// The image shown.
    pub uri: &'a str,
    /// The credit at the bottom right (none or empty: none).
    pub credit: Option<String>,
    /// The image button's tooltip.
    pub tooltip: &'a str,
    /// `(index, total)` with more than one picture: the previous and next buttons and the count.
    pub count: Option<(usize, usize)>,
}

/// What a photo block was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhotoResponse {
    /// The picture was clicked (it opens its page).
    pub opened: bool,
    /// −1 for the previous picture, 1 for the next.
    pub step: i32,
}

/// The photo block's height across `width` at `aspect`.
pub fn photo_height(width: f32, aspect: f32) -> f32 {
    width / aspect
}

/// `.photo.skeleton`: the block at `aspect` while loading. The panels' own `.photo { background:
/// var(--surface-raised) }` outranks `.skeleton`'s gradient, so it is a flat `--surface-raised`
/// block with the skeleton's radius 4, and does not shimmer (Chromium's rendering of the flight
/// and ship panels).
pub fn photo_skeleton(ui: &mut Ui, aspect: f32) -> Rect {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, photo_height(width, aspect)), Sense::hover());
    ui.painter().rect_filled(snap_rect(rect, ui.ctx().pixels_per_point()), CornerRadius::same(theme::RADIUS), theme::SURFACE_RAISED);
    rect
}

/// Paints `galley` at `at` over `text-shadow: 0 1px 2px oklch(0 0 0 / 0.9)` (drawn unblurred at
/// half its alpha).
fn shadowed(painter: &Painter, at: Pos2, galley: Arc<Galley>) {
    let shadow = theme::PHOTO_TEXT_SHADOW.gamma_multiply(0.5);
    painter.galley_with_override_text_color(at + Vec2::new(0.0, 1.0), galley.clone(), shadow);
    painter.galley(at, galley, theme::TEXT_1);
}

/// The photo block (see the module documentation) across the available width at `aspect`.
pub fn photo(ui: &mut Ui, id: Id, aspect: f32, photo: Photo<'_>) -> PhotoResponse {
    let ctx = ui.ctx().clone();
    let width = ui.available_width();
    let size = Vec2::new(width, photo_height(width, aspect));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let ppp = ctx.pixels_per_point();
    let block = snap_rect(rect, ppp);
    painter.rect_filled(block, CornerRadius::ZERO, theme::SURFACE_RAISED);
    if let Remote::Ready(texture) = remote(&ctx, photo.uri) {
        painter.image(texture.id, block, cover_uv(texture.size, block.size()), Color32::WHITE);
    }
    let mut out = PhotoResponse::default();
    let image = ui.interact(rect, id.with("image"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let image = tooltip::show(image, photo.tooltip);
    out.opened = image.clicked();

    let text_style = Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_1);
    let line = text_style.line_px();
    let text_top = rect.bottom() - PHOTO_TEXT_BOTTOM - line;
    if let Some(credit) = photo.credit.filter(|c| !c.is_empty()) {
        let galley = painter.layout_job(text::job(&credit, &text_style));
        let at = Pos2::new(rect.right() - PHOTO_INSET - galley.size().x, text_top + (line - galley.size().y) / 2.0);
        shadowed(&painter, at, galley);
    }
    if let Some((index, total)) = photo.count.filter(|&(_, total)| total > 1) {
        let mid = rect.center().y;
        let previous = Rect::from_center_size(Pos2::new(rect.left() + PHOTO_INSET + PHOTO_NAV / 2.0, mid), Vec2::splat(PHOTO_NAV));
        let next = Rect::from_center_size(Pos2::new(rect.right() - PHOTO_INSET - PHOTO_NAV / 2.0, mid), Vec2::splat(PHOTO_NAV));
        let buttons = [(previous, Icon::ChevronLeft, -1, "Previous photo"), (next, Icon::ChevronRight, 1, "Next photo")];
        let responses: Vec<Response> = buttons
            .iter()
            .map(|&(r, _, step, label)| {
                let response = ui.interact(r, id.with(("nav", step)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                response
            })
            .collect();
        let shown = ui.rect_contains_pointer(rect) || responses.iter().any(Response::has_focus);
        let opacity = anim::toggle(&ctx, id.with("nav-opacity"), shown, anim::HOVER);
        for ((r, icon, step, _), response) in buttons.iter().zip(&responses) {
            if opacity > 0.0 {
                let mut p = painter.clone();
                p.multiply_opacity(opacity);
                p.circle_filled(r.center(), PHOTO_NAV / 2.0, theme::PHOTO_BUTTON_BG);
                icons::paint(&p, *r, icon.at(PHOTO_NAV_ICON, icons::STROKE_REGULAR), theme::TEXT_1);
            }
            if response.has_focus() {
                super::widgets::button::focus_ring(ui.painter(), *r, 14);
            }
            if response.clicked() {
                out.step = *step;
            }
        }
        let count_style = data(&text_style);
        let galley = painter.layout_job(text::job(&format!("{}/{total}", index + 1), &count_style));
        let at = Pos2::new(rect.left() + PHOTO_INSET, text_top + (line - galley.size().y) / 2.0);
        shadowed(&painter, at, galley);
    }
    out
}

// ------------------------------------------------------------------------------------------------
// footer

/// The footer (see the module documentation): its status and its buttons.
pub struct Footer<'a> {
    pub status: Piece<'a>,
    /// The status's `title`.
    pub status_tooltip: Option<String>,
    pub buttons: Vec<Button>,
}

impl Footer<'_> {
    fn sizes(&self, ctx: &Context) -> (Vec2, Vec<Vec2>) {
        let status = match &self.status {
            Piece::Text { line, .. } => Vec2::new(self.status.natural_width(ctx), *line),
            Piece::Skeleton(size) | Piece::Custom(size, _) => *size,
        };
        (status, self.buttons.iter().map(|b| b.size(ctx)).collect())
    }

    /// Whether the status and the buttons share one row `width` px wide (else the buttons wrap
    /// below, to the right).
    fn one_row(status: Vec2, buttons: &[Vec2], width: f32) -> bool {
        let inner = width - 2.0 * FOOTER_PAD.x;
        let actions = buttons.iter().map(|b| b.x).sum::<f32>() + FOOTER_GAP * buttons.len().saturating_sub(1) as f32;
        buttons.is_empty() || status.x + FOOTER_GAP + actions <= inner
    }

    /// Its height across `width`, its top rule included.
    pub fn height(&self, ctx: &Context, width: f32) -> f32 {
        let (status, buttons) = self.sizes(ctx);
        let tallest = buttons.iter().map(|b| b.y).fold(0.0, f32::max);
        let rows = if Self::one_row(status, &buttons, width) { status.y.max(tallest) } else { status.y + FOOTER_GAP + tallest };
        theme::BORDER + 2.0 * FOOTER_PAD.y + rows
    }

    /// Draws it in `rect` (from [`body`]); answers the buttons' responses in order.
    pub fn show(self, ui: &mut Ui, rect: Rect) -> Vec<Response> {
        let ctx = ui.ctx().clone();
        let (status, sizes) = self.sizes(&ctx);
        let one_row = Self::one_row(status, &sizes, rect.width());
        let painter = ui.painter().clone();
        let ppp = ctx.pixels_per_point();
        painter.rect_filled(snap_rect(rect, ppp), CornerRadius::ZERO, theme::SURFACE);
        hrule(&painter, rect.x_range(), rect.top());
        let inner_top = rect.top() + theme::BORDER + FOOTER_PAD.y;
        let tallest = sizes.iter().map(|b| b.y).fold(0.0, f32::max);
        let row = if one_row { status.y.max(tallest) } else { status.y };
        let status_rect = Rect::from_min_size(Pos2::new(rect.left() + FOOTER_PAD.x, inner_top + (row - status.y) / 2.0), status);
        let laid = self.status.lay_out(&ctx, status.x);
        laid.paint(&painter, status_rect.min);
        if let Some(tip) = &self.status_tooltip {
            tooltip::show(ui.interact(status_rect, ui.scope_id().with("footer-status"), Sense::hover()), tip);
        }
        let actions_mid = if one_row { inner_top + row / 2.0 } else { inner_top + status.y + FOOTER_GAP + tallest / 2.0 };
        let total = sizes.iter().map(|b| b.x).sum::<f32>() + FOOTER_GAP * sizes.len().saturating_sub(1) as f32;
        let mut x = rect.right() - FOOTER_PAD.x - total;
        let mut responses = Vec::new();
        for (button, size) in self.buttons.into_iter().zip(sizes) {
            let r = Rect::from_min_size(Pos2::new(x, actions_mid - size.y / 2.0), size);
            responses.push(put(ui, ("footer-button", responses.len()), snap_rect(r, ppp), button));
            x += size.x + FOOTER_GAP;
        }
        responses
    }
}

/// The vehicle panels' Follow button (doc 06 §4): `LocateFixed` 14/1.75 and ` Follow` (the icon
/// alone in 32 × 32 when `compact`), pressed while `following`, its tooltip saying what C does.
pub fn follow_button(following: bool, compact: bool) -> Button {
    let button = Button::new().icon(Icon::LocateFixed.at(14.0, icons::STROKE_THIN));
    let button = if compact { button.compact().label(FOLLOW) } else { button.text(FOLLOW) };
    button.pressed(following).tooltip(if following { STOP_FOLLOWING_TOOLTIP } else { FOLLOW_TOOLTIP })
}

/// A child of `ui` at `rect` for parts laid out by hand, top-down from its top left, that does
/// not move `ui`'s cursor (`salt` keeps its widgets' ids apart from its siblings').
pub fn region<R>(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, rect: Rect, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut child = ui.new_child(UiBuilder::new().id_salt(salt).max_rect(rect).layout(Layout::top_down(Align::Min)));
    add(&mut child)
}

/// `widget` centred in `rect` (its size, here), without moving `ui`'s cursor.
pub fn put(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, rect: Rect, widget: impl Widget) -> Response {
    let mut child = ui.new_child(UiBuilder::new().id_salt(salt).max_rect(rect).layout(Layout::centered_and_justified(egui::Direction::TopDown)));
    child.add(widget)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(ctx: &Context, f: impl FnOnce(&mut Ui)) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 900.0))),
            time: Some(0.0),
            ..Default::default()
        };
        let mut f = Some(f);
        let mut output = ctx.run_ui(raw, |ui| {
            if let Some(f) = f.take() {
                f(ui);
            }
        });
        output.textures_delta.clear();
    }

    fn ctx() -> Context {
        let ctx = Context::default();
        super::super::theme::apply(&ctx);
        ctx
    }

    #[test]
    fn facts_are_placed_as_css_sparse_auto_placement_places_them() {
        use Span::*;
        // the overview with an airspace: three, then Phase and the airspace's two columns
        assert_eq!(grid_places(&[One, One, One, One, Two], 3), [(0, 0, 1), (0, 1, 1), (0, 2, 1), (1, 0, 1), (1, 1, 2)]);
        // a wide item after one in a row starts a row of its own; the next goes under it
        assert_eq!(grid_places(&[One, All, One], 3), [(0, 0, 1), (1, 0, 3), (2, 0, 1)]);
        // two columns: a span that does not fit what is left of the row moves down
        assert_eq!(grid_places(&[One, Two, One], 3), [(0, 0, 1), (0, 1, 2), (1, 0, 1)]);
        assert_eq!(grid_places(&[One, One, Two], 3), [(0, 0, 1), (0, 1, 1), (1, 0, 2)]);
        assert_eq!(grid_places(&[One, One, One, All], 2), [(0, 0, 1), (0, 1, 1), (1, 0, 1), (2, 0, 2)]);
    }

    #[test]
    fn images_cover_and_fit_as_css_draws_them() {
        // a 3:2 picture in the 2:1 block: its middle two thirds of its height... of its width kept
        let uv = cover_uv(Vec2::new(600.0, 400.0), Vec2::new(360.0, 180.0));
        assert!((uv.min.x - 0.0).abs() < 1e-6 && (uv.min.y - 0.125).abs() < 1e-6 && (uv.max.y - 0.875).abs() < 1e-6, "{uv:?}");
        let uv = cover_uv(Vec2::new(1000.0, 300.0), Vec2::new(360.0, 180.0));
        assert!((uv.min.x - 0.2).abs() < 1e-6 && (uv.max.x - 0.8).abs() < 1e-6 && uv.min.y == 0.0, "{uv:?}");
        // a logo of 200 × 60 under max 110 × 22: 73.33 × 22 (Chromium's layout of today's markup)
        let s = contain(Vec2::new(200.0, 60.0), Vec2::new(110.0, 22.0));
        assert!((s.x - 73.333).abs() < 0.01 && s.y == 22.0, "{s:?}");
        // a small one stays at its own size
        assert_eq!(contain(Vec2::new(40.0, 10.0), Vec2::new(110.0, 22.0)), Vec2::new(40.0, 10.0));
        assert_eq!(contain(Vec2::ZERO, Vec2::new(110.0, 22.0)), Vec2::ZERO);
    }

    #[test]
    fn a_strip_lays_out_as_chromium_lays_out_the_flight_panel() {
        let ctx = ctx();
        let mut out = (Vec::new(), Rect::NOTHING);
        pass(&ctx, |ui| {
            ui.set_width(360.0);
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let cells = || {
                vec![
                    Cell::fill(vec![Piece::text("SAS1417", &title_style(Face::Mono), Fit::Wrap), Piece::text("SK1417", &sub_style(Face::Ui), Fit::Ellipsis)]),
                    Cell::auto(vec![Piece::text("A20N", &value_style(theme::TEXT_1), Fit::Ellipsis), Piece::text("SE-ROR", &sub_style(Face::Mono), Fit::Ellipsis)]),
                    Cell::auto(vec![Piece::text("FL360", &value_style(theme::AMBER), Fit::Ellipsis), Piece::text("448 kt", &sub_style(Face::Mono), Fit::Ellipsis)]),
                ]
            };
            let widths = strip_widths(ui.ctx(), &cells(), 360.0);
            let top = ui.cursor().top();
            strip(ui, cells(), "Close flight details");
            out = (widths, Rect::from_min_max(Pos2::new(0.0, top), ui.cursor().min));
        });
        let (widths, rect) = out;
        // Chromium: 182.72, 68.64, 68.64 (the subs' 43.64 px of Mono 12 with its letter spacing,
        // 24 of padding and the rule), and the close column
        assert!((widths[1] - 68.64).abs() < 0.6 && (widths[2] - 68.64).abs() < 0.6, "{widths:?}");
        assert!((widths.iter().sum::<f32>() + CLOSE_COLUMN - 360.0).abs() < 0.01, "{widths:?}");
        // 12 + 24.2 + 2 + 16.8 + 12 and the rule under it (Chromium: 67.98)
        assert!((rect.height() - 67.99).abs() < 0.02, "{}", rect.height());
    }

    #[test]
    fn the_footer_takes_one_row_while_it_fits() {
        let ctx = ctx();
        pass(&ctx, |ui| {
            let footer = Footer {
                status: Piece::text("Connecting…", &Style::ui(13.0, 400, theme::TEXT_2), Fit::Ellipsis),
                status_tooltip: None,
                buttons: vec![follow_button(false, true), Button::new().text("Flightradar24").icon(Icon::ExternalLink.at(14.0, 1.75))],
            };
            assert_eq!(footer.height(ui.ctx(), 362.0 - 2.0), FOOTER_HEIGHT);
            // a narrow panel: the buttons wrap under the status
            assert!((footer.height(ui.ctx(), 220.0) - (1.0 + 24.0 + 18.2 + 8.0 + 32.0)).abs() < 0.01);
        });
    }

    #[test]
    fn the_follow_button_says_what_c_does() {
        let on = follow_button(true, false);
        let off = follow_button(false, true);
        let ctx = ctx();
        pass(&ctx, |ui| {
            assert_eq!(off.size(ui.ctx()), Vec2::new(32.0, 32.0));
            // 12 + 14 + 8 + "Follow" + 12 and the borders (Chromium: 85.58)
            assert!((on.size(ui.ctx()).x - 85.58).abs() < 1.0, "{:?}", on.size(ui.ctx()));
        });
        assert_eq!(on.look(false, true).border, Some(theme::AMBER));
        assert_eq!(off.look(false, true).border, Some(theme::LINE_CONTROL));
    }

    #[test]
    fn a_clock_is_mono_then_the_zone_in_the_ui_face() {
        let clocks = Clocks::default();
        let style = Style::ui(12.0, 400, theme::TEXT_2);
        let mut job = LayoutJob::default();
        text::append(&mut job, "Departed ", &style);
        // 2026-06-21 09:43 UTC in Stockholm
        append_clock(&mut job, &clocks, 1_782_034_980.0, Some("Europe/Stockholm"), &style);
        assert_eq!(job.text, "Departed 11:43 CEST");
        let faces: Vec<_> = job.sections.iter().map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.color)).collect();
        assert_eq!(faces, [("Departed ", theme::TEXT_2), ("11:43", theme::TEXT_2), (" ", theme::TEXT_2), ("CEST", theme::TEXT_3)]);
        let mut none = LayoutJob::default();
        append_clock(&mut none, &clocks, 0.0, None, &style);
        assert_eq!(none.text, "—");
    }
}
