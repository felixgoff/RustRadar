//! The live dot (FlightPanel:1326-1349; doc 06 §1.1, §27): the flight panel footer's status dot.
//!
//! - [`LiveDot::Live`] (`.dot`): an 8 px `--live` disc pulsing every 2 s: a ring
//!   (`box-shadow: 0 0 0 <spread>`, round with the disc) spreading from 0 to 6 px while its colour,
//!   `oklch(0.8 0.15 152)` ([`theme::LIVE_PULSE`]), fades from alpha 0.6 to 0, both on CSS's
//!   `ease-out` (`animation: pulse 2s ease-out infinite`; [`anim::live_dot_ring`]). The ring lies
//!   under the disc. While one is drawn egui repaints at once; under reduced motion there is no
//!   ring (the 0.01 ms single run ends at once and leaves no shadow).
//! - [`LiveDot::Ended`] (`.status.ended .dot`): not live is a fact, not a failure: a still, hollow
//!   8 px dot, its edge a 1.5 px `--text-3` ring inside (`box-shadow: inset 0 0 0 1.5px`).
//!
//! The ring's colour stays put while its alpha falls: the animation mixes two colours that differ
//! only in alpha, in premultiplied Oklab, so only the alpha moves.

use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use super::super::{anim, theme};

/// The dot's diameter, px.
pub const SIZE: f32 = 8.0;
/// The ended dot's ring, px (inside the dot).
pub const ENDED_RING: f32 = 1.5;
/// The pulse ring's alpha at its start.
pub const PULSE_ALPHA: f32 = 0.6;

/// Which dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDot {
    /// Pulsing `--live`.
    Live,
    /// Still and hollow.
    Ended,
}

/// The pulse ring at `phase` (0–1 of the 2 s cycle): its outer radius about the dot's centre and
/// its colour (premultiplied).
pub fn pulse(phase: f32) -> (f32, Color32) {
    let (spread, fade) = anim::live_dot_ring(phase);
    let [r, g, b] = theme::LIVE_PULSE_RGB;
    let color = Color32::from_rgba_unmultiplied(r, g, b, theme::alpha(PULSE_ALPHA * fade));
    (SIZE / 2.0 + spread, color)
}

impl LiveDot {
    /// Draws the dot centred on `center`.
    pub fn paint(self, painter: &Painter, center: Pos2) {
        let radius = SIZE / 2.0;
        match self {
            LiveDot::Live => {
                if let Some(phase) = anim::cycle(painter.ctx(), anim::LIVE_DOT_PERIOD) {
                    let (outer, color) = pulse(phase);
                    if outer > radius && color.a() > 0 {
                        painter.circle_filled(center, outer, color);
                    }
                }
                painter.circle_filled(center, radius, theme::LIVE);
            }
            LiveDot::Ended => {
                painter.circle_stroke(center, radius - ENDED_RING / 2.0, Stroke::new(ENDED_RING, theme::TEXT_3));
            }
        }
    }

    /// Takes the dot's 8 px square from `ui`'s layout and draws it (the ring spills over it, as a
    /// box shadow does).
    pub fn show(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::hover());
        if ui.is_rect_visible(rect.expand(anim::LIVE_DOT_RING)) {
            self.paint(ui.painter(), rect.center());
        }
        response
    }

    /// The square the dot and its widest ring cover around `center`.
    pub fn bounds(center: Pos2) -> Rect {
        Rect::from_center_size(center, Vec2::splat(SIZE + 2.0 * anim::LIVE_DOT_RING))
    }
}

impl egui::Widget for LiveDot {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::Shape;

    #[test]
    fn the_ring_spreads_6_px_and_fades_out() {
        let (outer, color) = pulse(0.0);
        assert_eq!(outer, 4.0);
        assert_eq!(color, Color32::from_rgba_unmultiplied(105, 217, 141, 153));
        let (outer, color) = pulse(1.0);
        assert_eq!((outer, color.a()), (10.0, 0));
        // CSS's ease-out: faster at first
        let (outer, color) = pulse(0.5);
        let e = anim::CSS_EASE_OUT.at(0.5) as f32;
        assert!((outer - (4.0 + 6.0 * e)).abs() < 1e-5 && outer > 7.0, "{outer}");
        assert_eq!(color.a(), theme::alpha(0.6 * (1.0 - e)));
    }

    fn circles(ctx: &egui::Context, time: f64, dot: LiveDot) -> Vec<egui::epaint::CircleShape> {
        let mut out = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
            dot.paint(ui.painter(), Pos2::new(50.0, 50.0));
        });
        out.textures_delta.clear();
        out.shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                Shape::Circle(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn live_pulses_under_the_dot_and_ended_is_a_hollow_ring() {
        let ctx = egui::Context::default();
        let live = circles(&ctx, 0.5, LiveDot::Live);
        assert_eq!(live.len(), 2);
        assert!(live[0].radius > 4.0 && live[0].fill.a() > 0, "the ring first: {:?}", live[0]);
        assert_eq!((live[1].radius, live[1].fill), (4.0, theme::LIVE));
        let ended = circles(&ctx, 0.5, LiveDot::Ended);
        assert_eq!(ended.len(), 1);
        assert_eq!((ended[0].radius, ended[0].fill, ended[0].stroke), (3.25, Color32::TRANSPARENT, Stroke::new(1.5, theme::TEXT_3)));
        anim::set_reduced_motion(&ctx, true);
        assert_eq!(circles(&ctx, 0.5, LiveDot::Live).len(), 1, "no ring under reduced motion");
    }
}
