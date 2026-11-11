//! Range sliders: the aircraft filters' two-thumb altitude slider (doc 06 §11 item 3,
//! `FilterPanel.svelte`) and the settings' native slider (doc 06 §19.3, `SettingsMenu.svelte`).
//!
//! **[`DualRange`]**: two range inputs on one rail, only their thumbs taking the pointer.
//! - 20 px tall; the rail 2 px at top 9, 8 px in from either side, radius 1, `--line-control`;
//!   the span between the values in `--amber`; thumbs 16 px circles, a 2 px `--amber` border
//!   round `--ink`, 2 px from the top.
//! - A thumb's centre runs from 8 px in to 8 px from the end (the input's width less the
//!   thumb's), so the span ends at the thumbs' centres.
//! - Dragging puts the thumb's centre under the pointer, snapped to the step, and keeps the
//!   thumbs a step apart (`setLow`: at most `high − step`; `setHigh`: at least `low + step`).
//!   Where they overlap the high thumb is on top, unless the low one is within three steps of
//!   the end (`z-index: 2`), so a pair pushed to the top can still be pulled apart.
//! - Keyboard (Chromium's range input): each thumb takes the focus in turn; Left/Down a step
//!   down, Right/Up a step up, Page Down/Up a tenth of the range (at least a step), Home/End
//!   the ends; then the same clamps. Focused: a 2 px `--amber` outline 2 px outside the thumb.
//!
//! **[`Range`]**: `<input type="range">` as Chromium draws it with `color-scheme: dark` and
//! `accent-color: --amber` (measured on today's settings menu): 16 px tall; an 8 px track of
//! radius 4 across the whole width with a 1 px `(133,133,133)` border, `(59,59,59)` inside and
//! `--amber` inside up to the thumb; a 16 px `--amber` thumb whose centre runs as above. Same
//! dragging (a click on the track jumps there too) and keys.
//!
//! Deviation: Chromium tints a native thumb a little under the pointer and while pressed; the
//! [`Range`] here keeps one colour.

use std::hash::Hash;

use egui::{Color32, CornerRadius, CursorIcon, Id, Key, Modifiers, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::segmented::paint_focus;
use crate::ui::theme;

/// The dual slider's height.
pub const HEIGHT: f32 = 20.0;
/// A thumb's diameter (both sliders).
pub const THUMB: f32 = 16.0;
/// The dual slider's thumb border and rail.
pub const THUMB_BORDER: f32 = 2.0;
pub const RAIL: f32 = 2.0;
pub const RAIL_TOP: f32 = 9.0;
/// The native slider's height, track height and colours.
pub const NATIVE_HEIGHT: f32 = 16.0;
pub const TRACK: f32 = 8.0;
pub const TRACK_BORDER: Color32 = Color32::from_rgb(133, 133, 133);
pub const TRACK_FILL: Color32 = Color32::from_rgb(59, 59, 59);

/// Rounds `v` to the nearest step from `min` and clamps it to the steps within `[min, max]`
/// (Blink's `StepRange::ClampAndRoundToStep` for a range input).
pub fn snap(v: f64, min: f64, max: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return v.clamp(min, max);
    }
    let top = min + ((max - min) / step).floor() * step;
    (min + ((v - min) / step).round() * step).clamp(min, top)
}

/// The value under `x` on a slider in `rect`: the thumb's centre at `x`, its travel inset by
/// half a thumb at either end, snapped.
pub fn value_at(x: f32, rect: Rect, min: f64, max: f64, step: f64) -> f64 {
    let travel = (rect.width() - THUMB).max(1.0);
    let t = f64::from(((x - rect.left() - THUMB / 2.0) / travel).clamp(0.0, 1.0));
    snap(min + t * (max - min), min, max, step)
}

/// Where a thumb's centre stands for `v`.
pub fn thumb_x(v: f64, rect: Rect, min: f64, max: f64) -> f32 {
    let t = if max > min { ((v - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    rect.left() + THUMB / 2.0 + (rect.width() - THUMB) * t as f32
}

/// The value a key moves `v` to (Blink's `RangeInputType::HandleKeydownEvent`), or `None` for
/// another key.
pub fn key_step(key: Key, v: f64, min: f64, max: f64, step: f64) -> Option<f64> {
    let big = ((max - min) / 10.0).max(step);
    let next = match key {
        Key::ArrowLeft | Key::ArrowDown => v - step,
        Key::ArrowRight | Key::ArrowUp => v + step,
        Key::PageDown => v - big,
        Key::PageUp => v + big,
        Key::Home => min,
        Key::End => max,
        _ => return None,
    };
    Some(snap(next, min, max, step))
}

/// `setLow`: the low thumb's new value, kept a step under `high`.
pub fn set_low(v: f64, high: f64, step: f64) -> f64 {
    v.min(high - step)
}

/// `setHigh`: the high thumb's new value, kept a step over `low`.
pub fn set_high(v: f64, low: f64, step: f64) -> f64 {
    v.max(low + step)
}

/// The keys a focused thumb takes.
const KEYS: [Key; 8] = [Key::ArrowLeft, Key::ArrowDown, Key::ArrowRight, Key::ArrowUp, Key::PageDown, Key::PageUp, Key::Home, Key::End];

/// The keys pressed for a focused thumb, taken from the input; its new value after them.
fn keys(ui: &Ui, response: &Response, mut v: f64, min: f64, max: f64, step: f64) -> Option<f64> {
    if !response.has_focus() {
        return None;
    }
    ui.memory_mut(|m| m.set_focus_lock_filter(response.id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() }));
    let mut moved = false;
    for key in KEYS {
        while ui.input_mut(|i| i.consume_key(Modifiers::NONE, key)) {
            if let Some(next) = key_step(key, v, min, max, step) {
                v = next;
                moved = true;
            }
        }
    }
    moved.then_some(v)
}

/// The aircraft filters' altitude slider: a low and a high value on one rail.
pub struct DualRange {
    id_salt: Id,
    min: f64,
    max: f64,
    step: f64,
}

impl DualRange {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, min: f64, max: f64, step: f64) -> Self {
        DualRange { id_salt: Id::NULL.with(id_salt), min, max, step }
    }

    /// The slider across the available width; `changed()` when a drag or a key moved a value.
    pub fn show(self, ui: &mut Ui, low: &mut f64, high: &mut f64) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let (rect, mut response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), HEIGHT), Sense::hover());
        let (min, max, step) = (self.min, self.max, self.step);
        let thumb = |v: f64| Rect::from_center_size(Pos2::new(thumb_x(v, rect, min, max), rect.top() + THUMB_BORDER + THUMB / 2.0), Vec2::splat(THUMB));
        // a pair at the top keeps the low thumb on top, so it can be pulled down again
        let low_on_top = *low > max - step * 3.0;
        let order: [bool; 2] = if low_on_top { [false, true] } else { [true, false] };
        let mut thumbs = [(Rect::NOTHING, false), (Rect::NOTHING, false)];
        let mut changed = false;
        // later interacts are on top: interact in stacking order, bottom first
        for &is_high in &order {
            let v = if is_high { *high } else { *low };
            let r = thumb(v);
            let tr = ui.interact(r, id.with(is_high), Sense::click_and_drag()).on_hover_cursor(CursorIcon::Grab);
            // only the round thumb takes the pointer
            let inside = tr.hover_pos().is_none_or(|p| p.distance(r.center()) <= THUMB / 2.0) || tr.dragged();
            let mut next = None;
            if (tr.dragged() || (tr.is_pointer_button_down_on() && inside))
                && let Some(p) = tr.interact_pointer_pos()
            {
                next = Some(value_at(p.x, rect, min, max, step));
            }
            if let Some(k) = keys(ui, &tr, v, min, max, step) {
                next = Some(k);
            }
            if let Some(n) = next {
                let n = if is_high { set_high(n, *low, step) } else { set_low(n, *high, step) };
                let slot = if is_high { &mut *high } else { &mut *low };
                if n != *slot {
                    *slot = n;
                    changed = true;
                }
            }
            thumbs[usize::from(is_high)].1 = tr.has_focus();
            response |= tr;
        }
        thumbs[0].0 = thumb(*low);
        thumbs[1].0 = thumb(*high);

        let painter = ui.painter();
        let rail = Rect::from_min_max(Pos2::new(rect.left() + THUMB / 2.0, rect.top() + RAIL_TOP), Pos2::new(rect.right() - THUMB / 2.0, rect.top() + RAIL_TOP + RAIL));
        painter.rect_filled(rail, CornerRadius::same((RAIL / 2.0) as u8), theme::LINE_CONTROL);
        let span = Rect::from_min_max(Pos2::new(thumb_x(*low, rect, min, max), rail.top()), Pos2::new(thumb_x(*high, rect, min, max), rail.bottom()));
        painter.rect_filled(span, CornerRadius::ZERO, theme::AMBER);
        for &is_high in &order {
            let (r, focused) = thumbs[usize::from(is_high)];
            painter.circle(r.center(), THUMB / 2.0 - THUMB_BORDER / 2.0, theme::INK, Stroke::new(THUMB_BORDER, theme::AMBER));
            if focused {
                paint_focus(painter, r, THUMB / 2.0, theme::FOCUS_RING_OFFSET);
            }
        }
        if changed {
            response.mark_changed();
        }
        response
    }
}

/// `<input type="range">` in Chromium's dark look, tinted with the accent.
pub struct Range {
    id_salt: Id,
    min: f64,
    max: f64,
    step: f64,
}

impl Range {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, min: f64, max: f64, step: f64) -> Self {
        Range { id_salt: Id::NULL.with(id_salt), min, max, step }
    }

    /// The slider across the available width; `changed()` when a drag, a click or a key moved
    /// `value`.
    pub fn show(self, ui: &mut Ui, value: &mut f64) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), NATIVE_HEIGHT), Sense::hover());
        let (min, max, step) = (self.min, self.max, self.step);
        let mut response = ui.interact(rect, id, Sense::click_and_drag()).on_hover_cursor(CursorIcon::Default);
        let mut next = None;
        if (response.dragged() || response.is_pointer_button_down_on())
            && let Some(p) = response.interact_pointer_pos()
        {
            next = Some(value_at(p.x, rect, min, max, step));
        }
        if let Some(k) = keys(ui, &response, *value, min, max, step) {
            next = Some(k);
        }
        if let Some(n) = next
            && n != *value
        {
            *value = n;
            response.mark_changed();
        }

        let painter = ui.painter();
        let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), TRACK));
        let radius = CornerRadius::same((TRACK / 2.0) as u8);
        painter.rect(track, radius, TRACK_FILL, Stroke::new(theme::BORDER, TRACK_BORDER), StrokeKind::Inside);
        let x = thumb_x(*value, rect, min, max);
        let filled = Rect::from_min_max(track.min + Vec2::splat(theme::BORDER), Pos2::new(x, track.bottom() - theme::BORDER));
        painter.rect_filled(filled, CornerRadius::same((TRACK / 2.0 - theme::BORDER) as u8), theme::AMBER);
        let centre = Pos2::new(x, rect.center().y);
        painter.circle_filled(centre, THUMB / 2.0, theme::AMBER);
        if response.has_focus() {
            paint_focus(painter, Rect::from_center_size(centre, Vec2::splat(THUMB)), THUMB / 2.0, theme::FOCUS_RING_OFFSET);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: f64 = 50_000.0;
    const STEP: f64 = 1_000.0;

    #[test]
    fn values_snap_to_the_step_within_the_range() {
        assert_eq!(snap(10_499.0, 0.0, MAX, STEP), 10_000.0);
        assert_eq!(snap(10_500.0, 0.0, MAX, STEP), 11_000.0);
        assert_eq!(snap(-300.0, 0.0, MAX, STEP), 0.0);
        assert_eq!(snap(51_000.0, 0.0, MAX, STEP), MAX);
        // a top that is not on a step: the last step under it
        assert_eq!(snap(40.0, 1.0, 40.5, 2.0), 39.0);
    }

    #[test]
    fn the_thumbs_travel_inset_by_half_a_thumb() {
        let rect = Rect::from_min_size(Pos2::new(33.0, 351.0), Vec2::new(286.0, HEIGHT));
        // today's span at 10,000 and 30,000 ft: from x 95 to 203 (Chromium)
        assert_eq!(thumb_x(10_000.0, rect, 0.0, MAX), 95.0);
        assert_eq!(thumb_x(30_000.0, rect, 0.0, MAX), 203.0);
        assert_eq!(thumb_x(0.0, rect, 0.0, MAX), 41.0);
        assert_eq!(thumb_x(MAX, rect, 0.0, MAX), 311.0);
        assert_eq!(value_at(95.0, rect, 0.0, MAX, STEP), 10_000.0);
        assert_eq!(value_at(0.0, rect, 0.0, MAX, STEP), 0.0);
        assert_eq!(value_at(400.0, rect, 0.0, MAX, STEP), MAX);
    }

    #[test]
    fn the_thumbs_stay_a_step_apart() {
        assert_eq!(set_low(30_000.0, 30_000.0, STEP), 29_000.0);
        assert_eq!(set_low(5_000.0, 30_000.0, STEP), 5_000.0);
        assert_eq!(set_high(10_000.0, 10_000.0, STEP), 11_000.0);
        assert_eq!(set_high(MAX, 10_000.0, STEP), MAX);
    }

    #[test]
    fn keys_move_as_chromium_moves_a_range_input() {
        assert_eq!(key_step(Key::ArrowRight, 10_000.0, 0.0, MAX, STEP), Some(11_000.0));
        assert_eq!(key_step(Key::ArrowUp, 10_000.0, 0.0, MAX, STEP), Some(11_000.0));
        assert_eq!(key_step(Key::ArrowLeft, 0.0, 0.0, MAX, STEP), Some(0.0));
        assert_eq!(key_step(Key::ArrowDown, 10_000.0, 0.0, MAX, STEP), Some(9_000.0));
        assert_eq!(key_step(Key::PageUp, 10_000.0, 0.0, MAX, STEP), Some(15_000.0));
        assert_eq!(key_step(Key::PageDown, 2_000.0, 0.0, MAX, STEP), Some(0.0));
        assert_eq!(key_step(Key::End, 2_000.0, 0.0, MAX, STEP), Some(MAX));
        assert_eq!(key_step(Key::Home, 2_000.0, 0.0, MAX, STEP), Some(0.0));
        assert_eq!(key_step(Key::Enter, 2_000.0, 0.0, MAX, STEP), None);
        // the exaggeration: 1–40 by 1, a page is 3.9 rounded to the step
        assert_eq!(key_step(Key::PageUp, 4.0, 1.0, 40.0, 1.0), Some(8.0));
    }

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

    #[test]
    fn dragging_the_high_thumb_down_stops_a_step_over_the_low() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let (mut low, mut high) = (10_000.0, 30_000.0);
        let rect = std::cell::Cell::new(Rect::NOTHING);
        let draw = |t: f64, events: Vec<egui::Event>, low: &mut f64, high: &mut f64| {
            pass(&ctx, t, events, |ui| {
                ui.set_width(286.0);
                rect.set(DualRange::new("alt", 0.0, MAX, STEP).show(ui, low, high).rect);
            })
        };
        draw(0.0, vec![], &mut low, &mut high);
        let r = rect.get();
        let at = |v: f64| Pos2::new(thumb_x(v, r, 0.0, MAX), r.top() + 10.0);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        let start = at(30_000.0);
        draw(0.1, vec![egui::Event::PointerMoved(start)], &mut low, &mut high);
        draw(0.2, vec![button(start, true)], &mut low, &mut high);
        for (i, v) in [25_000.0, 15_000.0, 2_000.0].into_iter().enumerate() {
            draw(0.3 + 0.1 * i as f64, vec![egui::Event::PointerMoved(at(v))], &mut low, &mut high);
        }
        draw(0.7, vec![button(at(2_000.0), false)], &mut low, &mut high);
        assert_eq!((low, high), (10_000.0, 11_000.0));
    }
}
