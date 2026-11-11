//! ToggleRow (doc 06 §12.2; `ToggleRow.svelte`): a whole row that toggles a switch, as across
//! the app's popovers and filter panels.
//!
//! - **Row**: a grid `1fr auto auto` with gap 12, centred, at least 32 px tall, 13 px text; a
//!   click anywhere on it toggles. The switch always sits in the third column, so a row without
//!   a key keeps both gaps (24 px between the text and the switch).
//! - **Text**: the label (13 px `--text-1`) and an optional hint under it (12 px `--text-3`, one
//!   row, cut with an ellipsis).
//! - **Key cap** (optional): at least 20 × 20 inside, padding `0 4px`, a 1 px `--line` border
//!   with a 2 px bottom, radius 4, Overpass Mono 11 px at 600 in `--text-3`. CSS sizes it as a
//!   content box: 30 × 23 for one character.
//! - **Switch** ([`switch`]): a 34 × 20 track of radius 10 in `--line-control`, `--amber` when on
//!   (150 ms); a 14 px knob 3 px in, `--text-1`, `--amber-ink` when on, sliding 14 px in 150 ms.
//! - **Keyboard**: the switch takes the focus (Tab), Space toggles it, as a checkbox does (not
//!   Enter); focused, a 2 px `--amber` outline 2 px outside the track.

use std::hash::Hash;

use egui::{Color32, CornerRadius, CursorIcon, Id, Key, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::button::transition_color;
use super::segmented::paint_focus;
use crate::ui::anim;
use crate::ui::text::{self, Style};
use crate::ui::theme;

/// The row's minimum height.
pub const MIN_HEIGHT: f32 = 32.0;
/// The grid's gap.
pub const GAP: f32 = theme::SPACE_3;
/// The track.
pub const SWITCH: Vec2 = Vec2::new(34.0, 20.0);
/// The knob's diameter and inset.
pub const KNOB: f32 = 14.0;
pub const KNOB_INSET: f32 = 3.0;
/// The key cap's minimum content size, side padding and borders (top and sides 1, bottom 2).
pub const KEY_MIN: f32 = 20.0;
pub const KEY_PAD_X: f32 = 4.0;
pub const KEY_BORDER_BOTTOM: f32 = 2.0;
/// The switch's focus outline offset (the app's `:focus-visible`).
pub const FOCUS_OFFSET: f32 = theme::FOCUS_RING_OFFSET;

/// A labelled switch row.
pub struct ToggleRow<'a> {
    id_salt: Id,
    label: &'a str,
    hint: Option<&'a str>,
    key: Option<&'a str>,
}

impl<'a> ToggleRow<'a> {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, label: &'a str) -> Self {
        ToggleRow { id_salt: Id::NULL.with(id_salt), label, hint: None, key: None }
    }

    /// One quiet line under the label.
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }

    /// Its single-key shortcut, as the keyboard help shows it.
    pub fn key(mut self, key: &'a str) -> Self {
        self.key = Some(key);
        self
    }

    /// Lays the row out across the available width; the response is `changed()` when a click or
    /// Space flipped `checked`.
    pub fn show(self, ui: &mut Ui, checked: &mut bool) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let width = ui.available_width();
        let label_style = Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1);
        let hint_style = Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3);
        let key_style = Style::data(theme::TEXT_XS, 600, theme::TEXT_3).letter_spacing(0.0);
        let key = self.key.map(|k| ui.painter().layout_job(text::job(k, &key_style)));
        let key_w = key.as_ref().map_or(0.0, |k| k.size().x.max(KEY_MIN) + 2.0 * KEY_PAD_X + 2.0 * theme::BORDER);
        let text_w = (width - SWITCH.x - 2.0 * GAP - key_w).max(0.0);
        let label = ui.painter().layout_job(text::truncated(self.label, &label_style, text_w));
        let hint = self.hint.map(|h| ui.painter().layout_job(text::truncated(h, &hint_style, text_w)));
        let text_h = label.size().y + hint.as_ref().map_or(0.0, |h| h.size().y);
        let height = text_h.max(MIN_HEIGHT);
        let (rect, mut response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        response = response.on_hover_cursor(CursorIcon::PointingHand);

        let painter = ui.painter();
        let top = rect.center().y - text_h / 2.0;
        let label_h = label.size().y;
        painter.galley(Pos2::new(rect.left(), top), label, Color32::PLACEHOLDER);
        if let Some(hint) = hint {
            painter.galley(Pos2::new(rect.left(), top + label_h), hint, Color32::PLACEHOLDER);
        }
        if let Some(key) = key {
            let cap_h = KEY_MIN + theme::BORDER + KEY_BORDER_BOTTOM;
            let cap = Rect::from_min_size(Pos2::new(rect.right() - SWITCH.x - GAP - key_w, rect.center().y - cap_h / 2.0), Vec2::new(key_w, cap_h));
            paint_key_cap(painter, cap);
            let content = Rect::from_min_max(cap.min + Vec2::splat(theme::BORDER), cap.max - Vec2::new(theme::BORDER, KEY_BORDER_BOTTOM));
            painter.galley(content.center() - key.size() / 2.0, key, Color32::PLACEHOLDER);
        }
        let track = Rect::from_min_size(Pos2::new(rect.right() - SWITCH.x, rect.center().y - SWITCH.y / 2.0), SWITCH);

        // a checkbox: Space flips it, Enter does not (egui clicks a focused widget on both)
        let enter = response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.key_pressed(Key::Space));
        if response.clicked() && !enter {
            *checked = !*checked;
            response.mark_changed();
        }
        switch(ui, id, track, *checked);
        if response.has_focus() {
            paint_focus(ui.painter(), track, SWITCH.y / 2.0, FOCUS_OFFSET);
        }
        response
    }
}

/// The switch in `track` (34 × 20): track and knob as they stand in their 150 ms transitions
/// towards `on`.
pub fn switch(ui: &Ui, id: Id, track: Rect, on: bool) {
    let painter = ui.painter();
    let fill = transition_color(ui.ctx(), id.with("track"), if on { theme::AMBER } else { theme::LINE_CONTROL }, anim::HOVER);
    painter.rect_filled(track, CornerRadius::same((track.height() / 2.0) as u8), fill);
    let t = anim::toggle(ui.ctx(), id.with("knob"), on, anim::SWITCH_KNOB);
    // the knob's colour has no transition
    let knob = if on { theme::AMBER_INK } else { theme::TEXT_1 };
    let centre = track.min + Vec2::splat(KNOB_INSET + KNOB / 2.0) + Vec2::new(anim::SWITCH_KNOB_TRAVEL * t, 0.0);
    painter.circle_filled(centre, KNOB / 2.0, knob);
}

/// A key cap's frame: a 1 px `--line` border, 2 px at the bottom, radius 4, nothing inside (the
/// background shows through).
pub fn paint_key_cap(painter: &egui::Painter, cap: Rect) {
    let line = Stroke::new(theme::BORDER, theme::LINE);
    painter.rect_stroke(cap, CornerRadius::same(theme::RADIUS), line, StrokeKind::Inside);
    // the bottom border's second pixel, inside the first, between the corners' curves
    let inner = f32::from(theme::RADIUS) - theme::BORDER;
    painter.hline((cap.left() + inner)..=(cap.right() - inner), cap.bottom() - theme::BORDER - 0.5, line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

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
    fn heights_as_today() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let (mut with_hint, mut without) = (Rect::NOTHING, Rect::NOTHING);
        pass(&ctx, 0.0, vec![], |ui| {
            ui.set_width(272.0);
            let mut on = false;
            with_hint = ToggleRow::new("a", "Terrain").hint("Mountains and valleys in relief").key("E").show(ui, &mut on).rect;
            without = ToggleRow::new("b", "Place names").key("L").show(ui, &mut on).rect;
        });
        // label 13 × 1.4 and hint 12 × 1.4 (Chromium: 34.98); a single line keeps the 32 px minimum
        assert!((with_hint.height() - 35.0).abs() < 0.01, "{with_hint:?}");
        assert_eq!(without.height(), 32.0);
    }

    #[test]
    fn a_click_and_space_toggle_but_enter_does_not() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut on = false;
        let rect = std::cell::Cell::new(Rect::NOTHING);
        let id = std::cell::Cell::new(Id::NULL);
        let draw = |t: f64, events: Vec<egui::Event>, on: &mut bool| {
            pass(&ctx, t, events, |ui| {
                ui.set_width(272.0);
                let r = ToggleRow::new("row", "Delayed only").show(ui, on);
                rect.set(r.rect);
                id.set(r.id);
            })
        };
        draw(0.0, vec![], &mut on);
        let at = Pos2::new(rect.get().left() + 20.0, rect.get().center().y);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        draw(0.1, vec![egui::Event::PointerMoved(at)], &mut on);
        draw(0.2, vec![press(true)], &mut on);
        draw(0.3, vec![press(false)], &mut on);
        assert!(on, "a click on the label toggles");
        ctx.memory_mut(|m| m.request_focus(id.get()));
        draw(0.4, vec![], &mut on);
        draw(0.5, vec![key(Key::Enter)], &mut on);
        assert!(on, "Enter leaves a checkbox alone");
        draw(0.6, vec![key(Key::Space)], &mut on);
        assert!(!on, "Space toggles");
    }

    #[test]
    fn the_knob_slides_fourteen_px_in_150_ms() {
        let ctx = egui::Context::default();
        let id = Id::NULL.with("k");
        let at = |t: f64, on: bool| {
            let mut v = 0.0;
            pass(&ctx, t, vec![], |ui| v = anim::toggle(ui.ctx(), id.with("knob"), on, anim::SWITCH_KNOB) * anim::SWITCH_KNOB_TRAVEL);
            v
        };
        assert_eq!(at(0.0, false), 0.0);
        assert_eq!(at(1.0, true), 0.0);
        let mid = at(1.05, true);
        assert!(mid > 7.0 && mid < 14.0, "{mid}");
        assert_eq!(at(1.16, true), 14.0);
    }
}
