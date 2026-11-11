//! Segmented (doc 06 §6.1; `Segmented.svelte`): a radio group drawn as a grid of options in one
//! bordered box, the chosen one filled in the accent.
//!
//! - **Legend** (unless hidden): 13 px `--text-1`, 8 px above the box.
//! - **Box**: a grid of `columns` equal columns (default: every option on one row), gap 2,
//!   padding 2, a 1 px `--line-control` border, radius 4.
//! - **Option**: content centred (the label, then the key hint 6 px after it), padding `5px 0`,
//!   radius 4, 13 px `--text-2`; under the pointer `--surface-hover` (150 ms on the background);
//!   chosen: `--amber` with `--amber-ink` text at weight 650. A key hint is Overpass Mono 11 px
//!   at 600 and opacity 0.7, and the option's tooltip is `{label} ({key})`.
//! - **Keyboard** (a native radio group): Tab reaches the chosen option only; the arrows choose
//!   the next (Right, Down) or previous (Left, Up) option, wrapping, and move the focus with it.
//!   A focused option has a 2 px `--amber` outline 1 px outside it.
//! - **Disabled**: the whole group at opacity 0.4, nothing reacts. Today's CSS also takes the
//!   background off any option under the pointer then, the chosen one included
//!   (`.segmented:disabled label:hover span` outranks `input:checked + span`): kept.
//!
//! The mode bar (doc 06 §6.2, U7) lays its options out as a row of natural widths with its own
//! padding; it builds on [`Look`] and [`paint_option`] here.
//!
//! Deviation: a disabled group's opacity applies to each shape, not to the group as one layer
//! (a chosen option's text over its fill comes out a little fainter than the browser's).

use std::hash::Hash;

use egui::{Color32, CornerRadius, CursorIcon, EventFilter, Id, Key, Modifiers, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::button::transition_color;
use super::tooltip;
use crate::ui::anim;
use crate::ui::text::{self, Style};
use crate::ui::theme;

/// Gap between options, both ways (`gap: 2px`).
pub const GAP: f32 = 2.0;
/// Padding inside the border (`padding: 2px`).
pub const PADDING: f32 = 2.0;
/// An option's padding above and below its line (`padding: 5px 0`).
pub const OPTION_PAD_Y: f32 = 5.0;
/// Between an option's label and its key hint (`gap: 6px`).
pub const KEY_GAP: f32 = 6.0;
/// The key hint's opacity.
pub const KEY_OPACITY: f32 = 0.7;
/// A disabled group's opacity.
pub const DISABLED_OPACITY: f32 = 0.4;
/// The legend's distance to the box (`margin-bottom: var(--space-2)`).
pub const LEGEND_GAP: f32 = theme::SPACE_2;
/// The focus outline's width and offset (`outline: 2px solid; outline-offset: 1px`).
pub const FOCUS_WIDTH: f32 = 2.0;
pub const FOCUS_OFFSET: f32 = 1.0;

/// One option: its value, label and optional single-key shortcut (`[value, label, key?]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Choice<'a, T> {
    pub value: T,
    pub label: &'a str,
    pub key: Option<&'a str>,
}

impl<'a, T: Copy> Choice<'a, T> {
    pub const fn new(value: T, label: &'a str) -> Self {
        Choice { value, label, key: None }
    }

    /// With a key hint after the label and in the tooltip.
    pub const fn key(self, key: &'a str) -> Self {
        Choice { key: Some(key), ..self }
    }
}

/// How an option is drawn in its state: fill, text colour and weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub fill: Color32,
    pub text: Color32,
    pub weight: u16,
}

/// An option's look: chosen → `--amber` fill, `--amber-ink` text at 650; under the pointer →
/// `--surface-hover`; else no fill and `--text-2`. The fill moves over 150 ms; text colour and
/// weight change at once (only `background-color` has a transition).
pub fn look(ui: &Ui, id: Id, chosen: bool, hovered: bool, disabled: bool) -> Look {
    let fill = if disabled && hovered {
        Color32::TRANSPARENT
    } else if chosen {
        theme::AMBER
    } else if hovered {
        theme::SURFACE_HOVER
    } else {
        Color32::TRANSPARENT
    };
    let fill = transition_color(ui.ctx(), id.with("fill"), fill, anim::HOVER);
    if chosen {
        Look { fill, text: theme::AMBER_INK, weight: 650 }
    } else {
        Look { fill, text: theme::TEXT_2, weight: theme::WEIGHT_REGULAR }
    }
}

/// An option's fill, radius 4.
pub fn paint_option(painter: &egui::Painter, rect: Rect, look: &Look) {
    if look.fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), look.fill);
    }
}

/// The 2 px `--amber` outline 1 px outside `rect` (`:focus-visible` with `outline-offset: 1px`),
/// following its radius.
pub fn paint_focus(painter: &egui::Painter, rect: Rect, radius: f32, offset: f32) {
    let r = rect.expand(offset + FOCUS_WIDTH / 2.0);
    let radius = (radius + offset + FOCUS_WIDTH / 2.0).round() as u8;
    painter.rect_stroke(r, CornerRadius::same(radius), Stroke::new(FOCUS_WIDTH, theme::AMBER), StrokeKind::Middle);
}

/// The arrow keys a focused member of a roving group takes for itself (so egui does not move the
/// focus by direction): +1 for `next` keys, −1 for `previous` ones, else 0.
pub fn roving(ui: &Ui, response: &Response, vertical: bool) -> i32 {
    if !response.has_focus() {
        return 0;
    }
    ui.memory_mut(|m| m.set_focus_lock_filter(response.id, EventFilter { horizontal_arrows: true, vertical_arrows: vertical, ..Default::default() }));
    ui.input_mut(|i| {
        let mut step = 0;
        if i.consume_key(Modifiers::NONE, Key::ArrowRight) || (vertical && i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            step += 1;
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowLeft) || (vertical && i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            step -= 1;
        }
        step
    })
}

/// A radio group of [`Choice`]s.
pub struct Segmented<'a, T> {
    id_salt: Id,
    legend: &'a str,
    hide_legend: bool,
    choices: &'a [Choice<'a, T>],
    columns: Option<usize>,
    disabled: bool,
}

impl<'a, T: PartialEq + Copy> Segmented<'a, T> {
    /// A group named `legend` (shown unless [`Self::hide_legend`]) of `choices`.
    pub fn new(id_salt: impl Hash + std::fmt::Debug, legend: &'a str, choices: &'a [Choice<'a, T>]) -> Self {
        Segmented { id_salt: Id::NULL.with(id_salt), legend, hide_legend: false, choices, columns: None, disabled: false }
    }

    /// The legend names the group (for assistive technology) but is not drawn.
    pub fn hide_legend(mut self) -> Self {
        self.hide_legend = true;
        self
    }

    /// `columns` options a row (default: all on one row).
    pub fn columns(mut self, columns: usize) -> Self {
        self.columns = Some(columns.max(1));
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Lays the group out across the available width, `value` the chosen option's. The response
    /// covers the group and is `changed()` when a click or an arrow key chose another option.
    pub fn show(self, ui: &mut Ui, value: &mut T) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let width = ui.available_width();
        let line = Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1).line_px();
        let legend_h = if self.hide_legend { 0.0 } else { line + LEGEND_GAP };
        let n = self.choices.len().max(1);
        let columns = self.columns.unwrap_or(n).min(n);
        let rows = n.div_ceil(columns);
        let option_h = OPTION_PAD_Y * 2.0 + line;
        let inset = theme::BORDER + PADDING;
        let box_h = inset * 2.0 + rows as f32 * option_h + (rows - 1) as f32 * GAP;
        let (rect, mut response) = ui.allocate_exact_size(Vec2::new(width, legend_h + box_h), Sense::hover());

        let mut painter = ui.painter().clone();
        if self.disabled {
            painter.multiply_opacity(DISABLED_OPACITY);
        }
        if !self.hide_legend {
            let galley = painter.layout_job(text::job(self.legend, &Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1)));
            painter.galley(rect.min, galley, Color32::PLACEHOLDER);
        }
        let frame = Rect::from_min_size(rect.min + Vec2::new(0.0, legend_h), Vec2::new(width, box_h));
        painter.rect_stroke(frame, CornerRadius::same(theme::RADIUS_SM), Stroke::new(theme::BORDER, theme::LINE_CONTROL), StrokeKind::Inside);

        let column_w = (width - inset * 2.0 - (columns - 1) as f32 * GAP) / columns as f32;
        let mut chosen_now = None;
        let mut step = 0;
        let mut focused = None;
        for (i, choice) in self.choices.iter().enumerate() {
            let (row, col) = (i / columns, i % columns);
            let min = frame.min + Vec2::new(inset + col as f32 * (column_w + GAP), inset + row as f32 * (option_h + GAP));
            let option = Rect::from_min_size(min, Vec2::new(column_w, option_h));
            let chosen = choice.value == *value;
            let option_id = id.with(i);
            // a radio group: only the chosen option is in the tab order
            let sense = if self.disabled {
                Sense::hover()
            } else if chosen {
                Sense::click()
            } else {
                Sense::CLICK
            };
            let mut r = ui.interact(option, option_id, sense);
            if !self.disabled {
                r = r.on_hover_cursor(CursorIcon::PointingHand);
            }
            if let Some(key) = choice.key {
                r = tooltip::show(r, &format!("{} ({key})", choice.label));
            }
            let look = look(ui, option_id, chosen, r.hovered(), self.disabled);
            paint_option(&painter, option, &look);
            paint_content(&painter, option, choice.label, choice.key, &look);
            if !self.disabled && r.clicked() && !chosen {
                chosen_now = Some(choice.value);
            }
            if chosen && !self.disabled {
                step = roving(ui, &r, true);
                if r.has_focus() {
                    focused = Some(i);
                    paint_focus(&painter, option, f32::from(theme::RADIUS), FOCUS_OFFSET);
                }
            }
            response |= r;
        }
        if let (Some(i), true) = (focused, step != 0) {
            let next = (i as i64 + i64::from(step)).rem_euclid(self.choices.len() as i64) as usize;
            chosen_now = Some(self.choices[next].value);
            ui.memory_mut(|m| m.request_focus(id.with(next)));
        }
        if let Some(v) = chosen_now
            && v != *value
        {
            *value = v;
            response.mark_changed();
        }
        response
    }
}

/// The label, and the key hint 6 px after it, centred in `option`.
fn paint_content(painter: &egui::Painter, option: Rect, label: &str, key: Option<&str>, look: &Look) {
    let label = painter.layout_job(text::job(label, &Style::ui(theme::TEXT_MD, look.weight, look.text)));
    // `font: 600 var(--text-xs) var(--font-data)`: no `.data`, so no letter spacing
    let key = key.map(|k| painter.layout_job(text::job(k, &Style::data(theme::TEXT_XS, 600, look.text.gamma_multiply(KEY_OPACITY)).letter_spacing(0.0))));
    let width = label.size().x + key.as_ref().map_or(0.0, |k| KEY_GAP + k.size().x);
    let left = option.center().x - width / 2.0;
    let centre_y = option.center().y;
    let label_w = label.size().x;
    painter.galley(Pos2::new(left, centre_y - label.size().y / 2.0), label, Color32::PLACEHOLDER);
    if let Some(key) = key {
        painter.galley(Pos2::new(left + label_w + KEY_GAP, centre_y - key.size().y / 2.0), key, Color32::PLACEHOLDER);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHOICES: [Choice<'static, u8>; 3] = [Choice::new(0, "Size"), Choice::new(1, "Name"), Choice::new(2, "Distance")];

    /// Runs `f` in a pass of a themed context at `time`, with `events` as input.
    fn pass(ctx: &egui::Context, time: f64, events: Vec<egui::Event>, mut f: impl FnMut(&mut Ui)) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| f(ui));
        output.textures_delta.clear();
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    #[test]
    fn a_row_is_as_tall_as_today() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut rect = Rect::NOTHING;
        pass(&ctx, 0.0, vec![], |ui| {
            ui.set_width(326.0);
            let mut v = 0;
            rect = Segmented::new("sort", "Sort ships by", &CHOICES).hide_legend().show(ui, &mut v).rect;
        });
        // 1 + 2 + (5 + 13 × 1.4 + 5) + 2 + 1 (Chromium: 34.19)
        assert!((rect.height() - 34.19).abs() < 0.01, "{}", rect.height());
        assert_eq!(rect.width(), 326.0);
    }

    #[test]
    fn the_arrows_choose_and_wrap() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut v = 0u8;
        let mut changed = vec![];
        let draw = |ctx: &egui::Context, t: f64, events: Vec<egui::Event>, v: &mut u8, changed: &mut Vec<bool>| {
            pass(ctx, t, events, |ui| {
                ui.set_width(300.0);
                changed.push(Segmented::new("g", "Group", &CHOICES).show(ui, v).changed());
            })
        };
        draw(&ctx, 0.0, vec![], &mut v, &mut changed);
        // Tab reaches the chosen option (the only one in the tab order)
        draw(&ctx, 0.1, vec![key(Key::Tab)], &mut v, &mut changed);
        draw(&ctx, 0.2, vec![], &mut v, &mut changed);
        let mut focused = None;
        pass(&ctx, 0.25, vec![], |ui| {
            ui.set_width(300.0);
            Segmented::new("g", "Group", &CHOICES).show(ui, &mut 0);
            focused = ui.memory(|m| m.focused()).map(|f| f == ui.make_persistent_id(Id::NULL.with("g")).with(0));
        });
        assert_eq!(focused, Some(true), "Tab focuses the chosen option");
        draw(&ctx, 0.3, vec![key(Key::ArrowLeft)], &mut v, &mut changed);
        assert_eq!(v, 2, "Left from the first wraps to the last");
        draw(&ctx, 0.4, vec![], &mut v, &mut changed);
        draw(&ctx, 0.5, vec![], &mut v, &mut changed);
        draw(&ctx, 0.6, vec![key(Key::ArrowDown)], &mut v, &mut changed);
        assert_eq!(v, 0, "Down from the last wraps to the first");
        assert_eq!(changed.iter().filter(|c| **c).count(), 2);
    }

    #[test]
    fn a_click_chooses_and_a_disabled_group_ignores_it() {
        for disabled in [false, true] {
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let mut v = 0u8;
            let rect = std::cell::Cell::new(Rect::NOTHING);
            let draw = |t: f64, events: Vec<egui::Event>, v: &mut u8| {
                pass(&ctx, t, events, |ui| {
                    ui.set_width(300.0);
                    rect.set(Segmented::new("g", "Group", &CHOICES).hide_legend().disabled(disabled).show(ui, v).rect);
                })
            };
            draw(0.0, vec![], &mut v);
            // the middle of the third option
            let at = Pos2::new(rect.get().right() - 40.0, rect.get().center().y);
            let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            draw(0.1, vec![egui::Event::PointerMoved(at)], &mut v);
            draw(0.2, vec![press(true)], &mut v);
            draw(0.3, vec![press(false)], &mut v);
            assert_eq!(v, if disabled { 0 } else { 2 }, "disabled {disabled}");
        }
    }
}
