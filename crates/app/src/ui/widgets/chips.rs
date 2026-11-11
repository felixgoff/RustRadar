//! Chips (doc 06 §11 item 2, §12.1; `FilterChips.svelte` and the aircraft filters' categories in
//! `FilterPanel.svelte`): pills that each turn a value on or off, under a legend with `All` and
//! `None`.
//!
//! - **Legend**: the label (12 px at 600 in `--text-2`) on the left, the links on the right as
//!   [`text_button`]s 4 px apart, on one baseline; 8 px above the chips.
//! - **Chip** ([`chip`]): 26 px tall (a border box), padding `0 8px`, a 1 px `--line-control`
//!   border, radius 13, 12 px. Off: no fill, `--text-3`; under the pointer `--surface-hover`
//!   (150 ms on fill and border); on: `--surface-raised` and `--text-1` with a leading `Check`
//!   12/2.5 in `--amber`. FilterChips' chip ([`Kind::Filter`]) spaces its parts 5 px and pulls
//!   the check 2 px closer; then an 8 × 8 colour swatch (radius 2, opacity 0.35 when off), the
//!   label, and the count (Overpass Mono 11 px in `--text-3`, en-GB digit grouping; none when 0).
//!   The categories' chip ([`Kind::Category`]) is the check and the label 3 px apart.
//! - **Chips row**: wrapping, 4 px apart both ways.
//! - **FilterChips** ([`FilterChips`]): the model is the *hidden* values; a click takes a value
//!   out of `hidden` or appends it; `All` (disabled while nothing is hidden) empties `hidden`,
//!   `None` (disabled while everything is) hides every option. An optional hint (12 px
//!   `--text-3`) follows 8 px under the chips. Disabled: legend and chips at opacity 0.45, the
//!   chips inert; the hint stays readable. The links keep working, as today: HTML leaves the
//!   buttons in a disabled fieldset's first legend enabled (the ships' disabled Size group shows
//!   an amber `None`).
//! - **Text button** ([`text_button`], `.text-button`): padding `2px 4px`, radius 4, 12 px at 600
//!   in `--amber`; under the pointer `--amber-soft` behind it; disabled `--text-3`.

use std::hash::Hash;

use egui::{Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use rustradar_core::text::group_thousands;

use super::button::transition_color;
use super::segmented::paint_focus;
use crate::ui::anim;
use crate::ui::icons::{self, Icon, STROKE_BOLD};
use crate::ui::text::{self, Style};
use crate::ui::theme;

/// A chip's height (a border box).
pub const HEIGHT: f32 = 26.0;
/// Its side padding.
pub const PAD_X: f32 = theme::SPACE_2;
/// Its radius (`border-radius: 13px`).
pub const RADIUS: u8 = 13;
/// The check's size; FilterChips pulls it 2 px closer to what follows (`margin-right: -2px`).
pub const CHECK: f32 = 12.0;
pub const CHECK_PULL: f32 = 2.0;
/// The swatch's side, radius and opacity when off.
pub const SWATCH: f32 = 8.0;
pub const SWATCH_OFF_OPACITY: f32 = 0.35;
/// Between chips, both ways.
pub const ROW_GAP: f32 = theme::SPACE_1;
/// Legend to chips, chips to hint.
pub const LEGEND_GAP: f32 = theme::SPACE_2;
pub const HINT_GAP: f32 = theme::SPACE_2;
/// A disabled group's legend and chips.
pub const DISABLED_OPACITY: f32 = 0.45;
/// The text button's padding.
pub const LINK_PAD: Vec2 = Vec2::new(4.0, 2.0);

/// Which chip: FilterChips' or the aircraft categories'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `FilterChips.svelte`'s `.chip`: parts 5 px apart, the check pulled in 2 px, swatch and count.
    Filter,
    /// `FilterPanel.svelte`'s `.category`: the check and the label 3 px apart.
    Category,
}

impl Kind {
    /// The gap between a chip's parts.
    pub const fn gap(self) -> f32 {
        match self {
            Kind::Filter => 5.0,
            Kind::Category => 3.0,
        }
    }
}

/// One chip as drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chip<'a> {
    pub kind: Kind,
    pub label: &'a str,
    pub on: bool,
    /// Its colour on the map (a swatch; [`Kind::Filter`] only).
    pub color: Option<Color32>,
    /// How many are loaded (shown unless 0 or none; [`Kind::Filter`] only).
    pub count: Option<u64>,
}

impl<'a> Chip<'a> {
    pub const fn new(kind: Kind, label: &'a str, on: bool) -> Self {
        Chip { kind, label, on, color: None, count: None }
    }

    pub const fn color(self, color: Color32) -> Self {
        Chip { color: Some(color), ..self }
    }

    pub const fn count(self, count: u64) -> Self {
        Chip { count: Some(count), ..self }
    }

    fn label_style(&self) -> Style {
        Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, if self.on { theme::TEXT_1 } else { theme::TEXT_3 })
    }

    fn count_text(&self) -> Option<String> {
        self.count.filter(|n| *n > 0).map(|n| group_thousands(n as f64))
    }

    /// The chip's outer size.
    pub fn size(&self, ctx: &egui::Context) -> Vec2 {
        let gap = self.kind.gap();
        let mut parts: Vec<f32> = Vec::new();
        if self.on {
            parts.push(CHECK - if self.kind == Kind::Filter { CHECK_PULL } else { 0.0 });
        }
        if self.color.is_some() {
            parts.push(SWATCH);
        }
        parts.push(text::measure(ctx, text::job(self.label, &self.label_style())).x);
        if let Some(count) = self.count_text() {
            parts.push(text::measure(ctx, text::job(&count, &count_style())).x);
        }
        let content: f32 = parts.iter().sum::<f32>() + gap * (parts.len() - 1) as f32;
        Vec2::new(2.0 * (theme::BORDER + PAD_X) + content, HEIGHT)
    }
}

/// The count's text: Overpass Mono 11 px in `--text-3` (`.count.data`).
fn count_style() -> Style {
    Style::data(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// Draws `chip` at the layout's next place; clickable unless `enabled` is false.
pub fn chip(ui: &mut Ui, id: Id, chip: &Chip<'_>, enabled: bool) -> Response {
    let size = chip.size(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let mut response = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    if enabled {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hovered = enabled && response.hovered();
    let fill = if chip.on {
        theme::SURFACE_RAISED
    } else if hovered {
        theme::SURFACE_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let fill = transition_color(ui.ctx(), id.with("fill"), fill, anim::HOVER);
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(RADIUS), fill, Stroke::new(theme::BORDER, theme::LINE_CONTROL), StrokeKind::Inside);
    let gap = chip.kind.gap();
    let mut x = rect.left() + theme::BORDER + PAD_X;
    let y = rect.center().y;
    if chip.on {
        icons::paint(painter, Rect::from_center_size(Pos2::new(x + CHECK / 2.0, y), Vec2::splat(CHECK)), Icon::Check.at(CHECK, STROKE_BOLD), theme::AMBER);
        x += CHECK + gap - if chip.kind == Kind::Filter { CHECK_PULL } else { 0.0 };
    }
    if let Some(color) = chip.color {
        let swatch = Rect::from_min_size(Pos2::new(x, y - SWATCH / 2.0), Vec2::splat(SWATCH));
        let color = if chip.on { color } else { color.gamma_multiply(SWATCH_OFF_OPACITY) };
        painter.rect_filled(swatch, CornerRadius::same(theme::RADIUS_SWATCH), color);
        x += SWATCH + gap;
    }
    let label = painter.layout_job(text::job(chip.label, &chip.label_style()));
    let label_w = label.size().x;
    painter.galley(Pos2::new(x, y - label.size().y / 2.0), label, Color32::PLACEHOLDER);
    x += label_w + gap;
    if let Some(count) = chip.count_text() {
        let count = painter.layout_job(text::job(&count, &count_style()));
        painter.galley(Pos2::new(x, y - count.size().y / 2.0), count, Color32::PLACEHOLDER);
    }
    if response.has_focus() {
        paint_focus(painter, rect, f32::from(RADIUS), theme::FOCUS_RING_OFFSET);
    }
    response
}

/// Lays `chips` out in wrapping rows 4 px apart across the available width; one response each.
pub fn chips(ui: &mut Ui, id: Id, chips: &[Chip<'_>], enabled: bool) -> Vec<Response> {
    let width = ui.available_width();
    ui.allocate_ui_with_layout(Vec2::new(width, 0.0), egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true), |ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(ROW_GAP);
        ui.set_width(width);
        chips.iter().enumerate().map(|(i, c)| chip(ui, id.with(i), c, enabled)).collect()
    })
    .inner
}

/// `.text-button` (`Reset`, `All`, `None`): the label in `--amber` with `2px 4px` padding, on
/// `--amber-soft` under the pointer; `--text-3` and inert when disabled.
pub fn text_button(ui: &mut Ui, id: Id, label: &str, enabled: bool) -> Response {
    let style = Style::ui(theme::TEXT_SM, 600, if enabled { theme::AMBER } else { theme::TEXT_3 });
    let galley = ui.painter().layout_job(text::job(label, &style));
    let size = galley.size() + 2.0 * LINK_PAD;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let mut response = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    if enabled {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::AMBER_SOFT);
        }
    }
    ui.painter().galley(rect.min + LINK_PAD, galley, Color32::PLACEHOLDER);
    if response.has_focus() {
        paint_focus(ui.painter(), rect, f32::from(theme::RADIUS), theme::FOCUS_RING_OFFSET);
    }
    response
}

/// A legend row: `label` on the left and the `links` (text, enabled) as [`text_button`]s on the
/// right, 4 px apart, the label on the links' baseline. Returns each link's response.
pub fn legend(ui: &mut Ui, id: Id, label: &str, links: &[(&str, bool)]) -> Vec<Response> {
    let width = ui.available_width();
    let label_style = Style::ui(theme::TEXT_SM, 600, theme::TEXT_2);
    let height = label_style.line_px() + 2.0 * LINK_PAD.y;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    // baseline alignment: the links' text sits 2 px down their padding, so the label does too
    let galley = ui.painter().layout_job(text::job(label, &label_style));
    ui.painter().galley(rect.min + Vec2::new(0.0, LINK_PAD.y), galley, Color32::PLACEHOLDER);
    let widths: Vec<f32> = links.iter().map(|(t, _)| text::measure(ui.ctx(), text::job(t, &Style::ui(theme::TEXT_SM, 600, theme::AMBER))).x + 2.0 * LINK_PAD.x).collect();
    let total = widths.iter().sum::<f32>() + theme::SPACE_1 * links.len().saturating_sub(1) as f32;
    let mut x = rect.right() - total;
    let mut out = Vec::with_capacity(links.len());
    for (i, ((t, enabled), w)) in links.iter().zip(widths).enumerate() {
        let r = ui.put(Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w, height)), |ui: &mut Ui| text_button(ui, id.with(("link", i)), t, *enabled));
        out.push(r);
        x += w + theme::SPACE_1;
    }
    out
}

/// One FilterChips option: its value, name, colour on the map and how many are loaded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChipOption<'a, T> {
    pub value: T,
    pub label: &'a str,
    pub color: Option<Color32>,
    pub count: Option<u64>,
}

impl<'a, T: Copy> ChipOption<'a, T> {
    pub const fn new(value: T, label: &'a str) -> Self {
        ChipOption { value, label, color: None, count: None }
    }

    pub const fn color(self, color: Color32) -> Self {
        ChipOption { color: Some(color), ..self }
    }

    pub const fn count(self, count: u64) -> Self {
        ChipOption { count: Some(count), ..self }
    }
}

/// `FilterChips.svelte`: a legend with `All` and `None`, a chip per option, an optional hint.
pub struct FilterChips<'a, T> {
    id_salt: Id,
    label: &'a str,
    options: &'a [ChipOption<'a, T>],
    hint: Option<&'a str>,
    disabled: bool,
}

impl<'a, T: PartialEq + Copy> FilterChips<'a, T> {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, label: &'a str, options: &'a [ChipOption<'a, T>]) -> Self {
        FilterChips { id_salt: Id::NULL.with(id_salt), label, options, hint: None, disabled: false }
    }

    /// One quiet line under the chips.
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Lays the group out across the available width; `hidden` holds the values left out. The
    /// response is `changed()` when a chip or a link changed `hidden`.
    pub fn show(self, ui: &mut Ui, hidden: &mut Vec<T>) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let enabled = !self.disabled;
        let inner = ui.scope(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);
            let (links, chip_responses) = ui
                .scope(|ui| {
                    if self.disabled {
                        ui.multiply_opacity(DISABLED_OPACITY);
                    }
                    // a disabled fieldset leaves its first legend's buttons enabled (HTML's rule):
                    // today's links still work, faded with the legend
                    let links = legend(ui, id.with("legend"), self.label, &[("All", !hidden.is_empty()), ("None", hidden.len() != self.options.len())]);
                    ui.add_space(LEGEND_GAP);
                    let shown: Vec<Chip<'_>> = self
                        .options
                        .iter()
                        .map(|o| Chip { kind: Kind::Filter, label: o.label, on: !hidden.contains(&o.value), color: o.color, count: o.count })
                        .collect();
                    (links, chips(ui, id.with("chips"), &shown, enabled))
                })
                .inner;
            if let Some(hint) = self.hint {
                ui.add_space(HINT_GAP);
                text::label(ui, text::wrapped(hint, &Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3), ui.available_width()), Sense::hover());
            }
            let mut changed = false;
            if links[0].clicked() {
                hidden.clear();
                changed = true;
            }
            if links[1].clicked() {
                *hidden = self.options.iter().map(|o| o.value).collect();
                changed = true;
            }
            for (o, r) in self.options.iter().zip(&chip_responses) {
                if r.clicked() {
                    toggle(hidden, o.value);
                    changed = true;
                }
            }
            changed
        });
        let mut response = inner.response;
        if inner.inner {
            response.mark_changed();
        }
        response
    }
}

/// Takes `value` out of `hidden`, or appends it (`FilterChips.svelte`'s `toggle`).
pub fn toggle<T: PartialEq + Copy>(hidden: &mut Vec<T>, value: T) {
    if hidden.contains(&value) {
        hidden.retain(|v| *v != value);
    } else {
        hidden.push(value);
    }
}

/// Puts the aircraft filters' category `id` in or out of `selected`, kept ascending
/// (`FilterPanel.svelte`'s `toggleCategory`).
pub fn toggle_sorted<T: Ord + Copy>(selected: &mut Vec<T>, id: T) {
    if selected.contains(&id) {
        selected.retain(|c| *c != id);
    } else {
        selected.push(id);
        selected.sort();
    }
}
