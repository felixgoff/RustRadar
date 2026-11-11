//! The tooltip style (plan 1.5; doc 06 §29): every `title` attribute of the web build is a
//! tooltip here, and they carry real information (units, time zones, shortcut hints).
//!
//! Today the webview draws a `title` as the system's own tooltip (Windows', macOS's, GTK's), so it
//! has no one look. The port draws them in the app's own tooltip style, the one its in-page
//! tooltips share (the altitude chart's `.tooltip`, AltitudeChart:340-353, and the map's
//! `.aircraft-tooltip`, [`crate::ui::tooltip`]): `--surface-raised` with a 1 px `--line` border,
//! radius 4, no shadow, `4px 8px` of padding ([`frame`]), and 12 px Overpass in `--text-1` at line
//! height 1.4 ([`style`]), wrapped at [`MAX_WIDTH`]. Like a system tooltip it shows near the
//! pointer, below the cursor ([`GAP`]), after the pointer has rested on the widget for egui's
//! `tooltip_delay` (0.5 s), and on disabled widgets too (Chromium shows `title` on a disabled
//! button). Deviation: the system's look is not reproduced.

use egui::containers::Tooltip;
use egui::{CornerRadius, Frame, Margin, Response, Sense, Shadow, Stroke, Ui};

use super::super::text::{self, Style};
use super::super::theme;

/// The padding: `--space-1` above and below, `--space-2` either side.
pub const PADDING: Margin = Margin::symmetric(theme::SPACE_2 as i8, theme::SPACE_1 as i8);
/// The widest a tooltip's text runs before it wraps, px.
pub const MAX_WIDTH: f32 = 320.0;
/// How far below the pointer's tip the tooltip starts (under a standard arrow cursor), px.
pub const GAP: f32 = 16.0;

/// The tooltip's box.
pub fn frame() -> Frame {
    Frame::new()
        .fill(theme::SURFACE_RAISED)
        .stroke(Stroke::new(theme::BORDER, theme::LINE))
        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
        .inner_margin(PADDING)
        .shadow(Shadow::NONE)
}

/// The tooltip's text style.
pub const fn style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_1)
}

/// Shows `text` as `response`'s tooltip while the pointer rests on it (enabled or not); answers
/// `response`.
pub fn show(response: Response, text: &str) -> Response {
    show_ui(response, |ui| {
        text::label(ui, text::wrapped(text, &style(), MAX_WIDTH), Sense::hover());
    })
}

/// Shows `add`'s contents in the tooltip's box while the pointer rests on `response`.
pub fn show_ui(response: Response, add: impl FnOnce(&mut Ui)) -> Response {
    let mut tooltip = if response.enabled() { Tooltip::for_enabled(&response) } else { Tooltip::for_disabled(&response) };
    tooltip.popup = tooltip.popup.frame(frame());
    tooltip.at_pointer().gap(GAP).show(add);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::Shape;
    use egui::{Event, Pos2, RawInput, Rect, Vec2};

    fn input(time: f64, pointer: Pos2) -> RawInput {
        RawInput {
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events: vec![Event::PointerMoved(pointer)],
            ..Default::default()
        }
    }

    #[test]
    fn a_resting_pointer_shows_the_box() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let at = Pos2::new(110.0, 110.0);
        let mut boxes = Vec::new();
        for (i, time) in [0.0, 0.1, 0.7, 0.8, 0.9].into_iter().enumerate() {
            let mut raw = input(time, at);
            if i > 0 {
                raw.events.clear(); // the pointer rests
            }
            let mut out = ctx.run_ui(raw, |ui| {
                egui::Area::new(egui::Id::unique("w")).fixed_pos(Pos2::new(100.0, 100.0)).show(ui.ctx(), |ui| {
                    let (_, r) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
                    show(r, "Zoom in (+)");
                });
            });
            out.textures_delta.clear();
            boxes = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    Shape::Rect(r) if r.fill == theme::SURFACE_RAISED => Some(r.clone()),
                    _ => None,
                })
                .collect();
        }
        assert_eq!(boxes.len(), 1, "one tooltip box");
        let b = &boxes[0];
        assert_eq!((b.stroke.width, b.stroke.color, b.corner_radius), (1.0, theme::LINE, CornerRadius::same(4)));
        assert!(b.rect.min.y >= at.y + GAP - 1.0, "below the pointer: {:?}", b.rect);
        let text_w = text::measure(&ctx, text::job("Zoom in (+)", &style())).x;
        // the padding and the border either side
        assert!((b.rect.width() - (text_w + 18.0)).abs() <= 0.1, "{} vs {}", b.rect.width(), text_w + 18.0);
        assert_eq!(b.rect.height(), 17.0 + 8.0 + 2.0, "one 12 × 1.4 px row on a whole pixel, 4 + 4 and the border");
    }
}
