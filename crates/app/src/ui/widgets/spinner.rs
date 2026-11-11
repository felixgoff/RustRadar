//! The spinner (`.spin`: KeysDialog:321-330, SearchBox:395-404, FlightPanel:1350-1359; doc 06
//! §27): Lucide's `LoaderCircle` at 14 px (12 in the search box's messages), stroke 2, in the
//! colour around it, turning clockwise once every 900 ms, linearly, forever (`animation: spin
//! 900ms linear infinite`, `transform: rotate(360deg)` about its centre).
//!
//! The turn is [`anim::spinner_angle`] of egui's clock, so every spinner on screen is at the same
//! angle (today each starts from 0 when it appears); while one is drawn egui repaints at once.
//! Under reduced motion it stands still at its start, as a 0.01 ms single run leaves it.

use egui::{Color32, Painter, Rect, Response, Sense, Ui, Vec2};

use super::super::anim;
use super::super::icons::{self, Icon};

/// The spinner's size in buttons, the keys dialog, the flight panel's footer and the search field.
pub const SIZE: f32 = 14.0;
/// The size in the search box's "Searching flights…" messages.
pub const SIZE_SMALL: f32 = 12.0;
/// `strokeWidth={2}`.
pub const STROKE: f32 = icons::STROKE_REGULAR;

/// `LoaderCircle` at `size`, turned to where the spinner stands at `phase` (0–1 of a turn).
pub fn spec(size: f32, phase: f32) -> icons::Spec {
    Icon::LoaderCircle.at(size, STROKE).rotated(anim::spinner_angle(phase))
}

/// Where the spinners stand now (0–1 of a turn); 0 under reduced motion. Keeps egui repainting.
pub fn phase(ctx: &egui::Context) -> f32 {
    anim::cycle(ctx, anim::SPINNER_PERIOD).unwrap_or(0.0)
}

/// Draws a spinner of `size` centred in `rect` in `color`.
pub fn paint(painter: &Painter, rect: Rect, size: f32, color: Color32) {
    icons::paint(painter, rect, spec(size, phase(painter.ctx())), color);
}

/// Takes a `size` square from `ui`'s layout and draws a spinner in it in `color`.
pub fn show(ui: &mut Ui, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, size, color);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    #[test]
    fn one_turn_in_900_ms() {
        let ctx = egui::Context::default();
        let at = |time: f64| {
            let mut p = -1.0;
            let mut out = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| p = phase(ui.ctx()));
            out.textures_delta.clear();
            assert!(out.viewport_output.values().any(|v| v.repaint_delay.is_zero()), "repaints while it turns");
            p
        };
        assert_eq!(at(0.0), 0.0);
        assert!((at(0.45) - 0.5).abs() < 1e-5);
        assert!((at(0.9 * 7.0 + 0.225) - 0.25).abs() < 1e-4);
        let s = spec(SIZE, 0.25);
        assert_eq!((s.icon, s.size, s.stroke), (Icon::LoaderCircle, 14.0, 2.0));
        assert!((s.angle - TAU / 4.0).abs() < 1e-6, "clockwise a quarter turn");
    }

    #[test]
    fn reduced_motion_stands_still() {
        let ctx = egui::Context::default();
        anim::set_reduced_motion(&ctx, true);
        let mut p = -1.0;
        ctx.run_ui(egui::RawInput { time: Some(0.3), ..Default::default() }, |ui| p = phase(ui.ctx())).textures_delta.clear();
        assert_eq!(p, 0.0);
    }
}
