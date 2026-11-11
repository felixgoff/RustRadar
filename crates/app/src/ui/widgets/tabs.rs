//! Tabs (doc 06 §7 item 6, §8.3 item 2; `FlightPanel.svelte`, `AirportPanel.svelte`): a row of
//! text tabs over a 1 px rule, the chosen one underlined in the accent.
//!
//! - **Row**: tabs from 12 px in, 4 px apart, over a 1 px `--line` bottom border across the
//!   whole width.
//! - **Tab**: padding `12px 8px 8px`, 13 px at weight 550 in `--text-3`; under the pointer or
//!   chosen `--text-1` (no transition). With an icon (the airport board's `PlaneTakeoff` and
//!   `PlaneLanding`, 15/1.75), the icon then the label, 8 px apart, centred.
//! - **Underline**: the chosen tab's, 2 px `--amber`, radius 1, 8 px in from either side, over
//!   the row's border (`bottom: -1px`).
//! - **Keyboard**: only the chosen tab is in the tab order; Left and Right choose the previous
//!   or next tab, wrapping, and move the focus with it (FlightPanel's `tabKeys`; the board's two
//!   tabs toggle the same way). Focused: the app's 2 px `--amber` outline 2 px outside.

use std::hash::Hash;

use egui::{Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::segmented::roving;
use crate::ui::icons::{self, Icon, STROKE_THIN};
use crate::ui::text::{self, Style};
use crate::ui::theme;

/// The row's side padding.
pub const ROW_PAD_X: f32 = theme::SPACE_3;
/// Between tabs.
pub const GAP: f32 = theme::SPACE_1;
/// A tab's padding: top, sides, bottom.
pub const PAD_TOP: f32 = theme::SPACE_3;
pub const PAD_X: f32 = theme::SPACE_2;
pub const PAD_BOTTOM: f32 = theme::SPACE_2;
/// The label's weight.
pub const WEIGHT: u16 = 550;
/// The icon's size and its gap to the label.
pub const ICON: f32 = 15.0;
pub const ICON_GAP: f32 = theme::SPACE_2;
/// The underline: height and inset from the tab's sides.
pub const UNDERLINE: f32 = 2.0;
pub const UNDERLINE_INSET: f32 = theme::SPACE_2;

/// One tab: its value, label and optional icon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tab<'a, T> {
    pub value: T,
    pub label: &'a str,
    pub icon: Option<Icon>,
}

impl<'a, T: Copy> Tab<'a, T> {
    pub const fn new(value: T, label: &'a str) -> Self {
        Tab { value, label, icon: None }
    }

    pub const fn icon(self, icon: Icon) -> Self {
        Tab { icon: Some(icon), ..self }
    }
}

/// A tab list.
pub struct Tabs<'a, T> {
    id_salt: Id,
    tabs: &'a [Tab<'a, T>],
}

impl<'a, T: PartialEq + Copy> Tabs<'a, T> {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, tabs: &'a [Tab<'a, T>]) -> Self {
        Tabs { id_salt: Id::NULL.with(id_salt), tabs }
    }

    /// The row across the available width, `selected` the chosen tab's value; `changed()` when
    /// a click or an arrow key chose another.
    pub fn show(self, ui: &mut Ui, selected: &mut T) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let width = ui.available_width();
        let line = Style::ui(theme::TEXT_MD, WEIGHT, theme::TEXT_1).line_px();
        let tab_h = PAD_TOP + line + PAD_BOTTOM;
        let (rect, mut response) = ui.allocate_exact_size(Vec2::new(width, tab_h + theme::BORDER), Sense::hover());
        let painter = ui.painter().clone();
        painter.hline(rect.x_range(), rect.top() + tab_h + theme::BORDER / 2.0, Stroke::new(theme::BORDER, theme::LINE));

        let mut x = rect.left() + ROW_PAD_X;
        let mut chosen_now = None;
        let mut step = 0;
        let mut focused = None;
        for (i, tab) in self.tabs.iter().enumerate() {
            let chosen = tab.value == *selected;
            let measure = painter.layout_job(text::job(tab.label, &Style::ui(theme::TEXT_MD, WEIGHT, theme::TEXT_1)));
            let content_w = measure.size().x + tab.icon.map_or(0.0, |_| ICON + ICON_GAP);
            let tab_rect = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(PAD_X * 2.0 + content_w, tab_h));
            x = tab_rect.right() + GAP;
            let tab_id = id.with(i);
            // a roving tab index: only the chosen tab takes the focus from Tab
            let r = ui.interact(tab_rect, tab_id, if chosen { Sense::click() } else { Sense::CLICK }).on_hover_cursor(CursorIcon::PointingHand);
            let color = if chosen || r.hovered() { theme::TEXT_1 } else { theme::TEXT_3 };
            let content = Rect::from_min_size(tab_rect.min + Vec2::new(PAD_X, PAD_TOP), Vec2::new(content_w, line));
            let mut label_x = content.left();
            if let Some(icon) = tab.icon {
                icons::paint(&painter, Rect::from_center_size(Pos2::new(content.left() + ICON / 2.0, content.center().y), Vec2::splat(ICON)), icon.at(ICON, STROKE_THIN), color);
                label_x += ICON + ICON_GAP;
            }
            let galley = painter.layout_job(text::job(tab.label, &Style::ui(theme::TEXT_MD, WEIGHT, color)));
            painter.galley(Pos2::new(label_x, content.center().y - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
            if chosen {
                let bottom = tab_rect.bottom() + theme::BORDER;
                let underline = Rect::from_min_max(Pos2::new(tab_rect.left() + UNDERLINE_INSET, bottom - UNDERLINE), Pos2::new(tab_rect.right() - UNDERLINE_INSET, bottom));
                painter.rect_filled(underline, CornerRadius::same(theme::RADIUS_TAB_UNDERLINE), theme::AMBER);
                step = roving(ui, &r, false);
                if r.has_focus() {
                    focused = Some(i);
                    let ring = tab_rect.expand(theme::FOCUS_RING_OFFSET + theme::FOCUS_RING / 2.0);
                    painter.rect_stroke(ring, CornerRadius::ZERO, Stroke::new(theme::FOCUS_RING, theme::AMBER), StrokeKind::Middle);
                }
            }
            if r.clicked() && !chosen {
                chosen_now = Some(tab.value);
            }
            response |= r;
        }
        if let (Some(i), true) = (focused, step != 0) {
            let next = (i as i64 + i64::from(step)).rem_euclid(self.tabs.len() as i64) as usize;
            chosen_now = Some(self.tabs[next].value);
            ui.memory_mut(|m| m.request_focus(id.with(next)));
        }
        if let Some(v) = chosen_now
            && v != *selected
        {
            *selected = v;
            response.mark_changed();
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    const FLIGHT: [Tab<'static, u8>; 4] = [Tab::new(0, "Overview"), Tab::new(1, "Route"), Tab::new(2, "Aircraft"), Tab::new(3, "History")];

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
    fn the_row_is_as_tall_as_today() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut rect = Rect::NOTHING;
        pass(&ctx, 0.0, vec![], |ui| {
            let mut tab = 0;
            rect = Tabs::new("t", &FLIGHT).show(ui, &mut tab).rect;
        });
        // 12 + 13 × 1.4 + 8, and the 1 px rule (Chromium: 39.19)
        assert!((rect.height() - 39.19).abs() < 0.01, "{}", rect.height());
    }

    #[test]
    fn left_and_right_cycle_and_wrap() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut tab = 0u8;
        let mut first = Id::NULL;
        pass(&ctx, 0.0, vec![], |ui| {
            first = ui.make_persistent_id(Id::NULL.with("t")).with(0);
            Tabs::new("t", &FLIGHT).show(ui, &mut tab);
        });
        ctx.memory_mut(|m| m.request_focus(first));
        let draw = |t: f64, events: Vec<egui::Event>, tab: &mut u8| pass(&ctx, t, events, |ui| {
            Tabs::new("t", &FLIGHT).show(ui, tab);
        });
        draw(0.1, vec![], &mut tab);
        draw(0.2, vec![key(Key::ArrowLeft)], &mut tab);
        assert_eq!(tab, 3, "Left from the first wraps to History");
        draw(0.3, vec![], &mut tab);
        draw(0.4, vec![], &mut tab);
        draw(0.5, vec![key(Key::ArrowRight)], &mut tab);
        assert_eq!(tab, 0, "Right from the last wraps to Overview");
        draw(0.6, vec![], &mut tab);
        draw(0.7, vec![], &mut tab);
        draw(0.8, vec![key(Key::ArrowDown)], &mut tab);
        assert_eq!(tab, 0, "Up and Down are not the tabs' keys");
    }
}
