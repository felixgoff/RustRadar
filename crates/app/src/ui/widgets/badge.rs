//! `.badge` (app.css:201-228; doc 06 §1.4): a status label whose tone backs up its words, never
//! replaces them. `display: inline-flex; padding: 1px 6px`, radius 4, 11 px Overpass at weight 650,
//! line height 1.5, one row (`white-space: nowrap`), as wide as its text:
//!
//! | Tone | Fill | Text |
//! |---|---|---|
//! | [`Tone::Good`] | `--live-soft` | `--live` |
//! | [`Tone::Warn`] | `--warn-soft` | `--warn` |
//! | [`Tone::Bad`] | `--danger-soft` | `--danger` |
//! | [`Tone::Neutral`] | `--surface-raised` | `--text-2` |
//!
//! Some places set the badge at 12 px ([`Badge::size`]): the sidebar's stale status
//! (+page:1822-1824), the train's and ship's stale report (`.report .badge`) and the train's delay
//! (`.voyage .badge`). The flight panel's delay badges sit 2 px lower (`margin-top: 2px`), which is
//! the caller's layout.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Ui, Vec2};
use rustradar_core::ship_details::NavTone;
use rustradar_core::time;
use rustradar_core::trains::DelayTone;

use super::super::text::{self, Style};
use super::super::theme;

/// `padding: 1px 6px`.
pub const PADDING: Vec2 = Vec2::new(6.0, 1.0);
/// The badge's size (`--text-xs`).
pub const TEXT_SIZE: f32 = theme::TEXT_XS;

/// A badge's tone (its class).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    Good,
    Warn,
    Bad,
    Neutral,
}

impl Tone {
    /// The fill and the text colour.
    pub const fn colors(self) -> (Color32, Color32) {
        match self {
            Tone::Good => (theme::LIVE_SOFT, theme::LIVE),
            Tone::Warn => (theme::WARN_SOFT, theme::WARN),
            Tone::Bad => (theme::DANGER_SOFT, theme::DANGER),
            Tone::Neutral => (theme::SURFACE_RAISED, theme::TEXT_2),
        }
    }

    /// The tone the TypeScript names (`good`, `warn`, `bad`, `neutral`).
    pub fn from_name(name: &str) -> Option<Tone> {
        match name {
            "good" => Some(Tone::Good),
            "warn" => Some(Tone::Warn),
            "bad" => Some(Tone::Bad),
            "neutral" => Some(Tone::Neutral),
            _ => None,
        }
    }
}

impl From<time::Tone> for Tone {
    fn from(tone: time::Tone) -> Tone {
        match tone {
            time::Tone::Good => Tone::Good,
            time::Tone::Warn => Tone::Warn,
            time::Tone::Bad => Tone::Bad,
        }
    }
}

impl From<DelayTone> for Tone {
    fn from(tone: DelayTone) -> Tone {
        match tone {
            DelayTone::Good => Tone::Good,
            DelayTone::Warn => Tone::Warn,
            DelayTone::Bad => Tone::Bad,
        }
    }
}

impl From<NavTone> for Tone {
    fn from(tone: NavTone) -> Tone {
        match tone {
            NavTone::Neutral => Tone::Neutral,
            NavTone::Warn => Tone::Warn,
        }
    }
}

/// A `.badge`.
#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    text: String,
    tone: Tone,
    size: f32,
    tooltip: Option<String>,
}

impl Badge {
    /// `text` in `tone` at 11 px.
    pub fn new(text: impl Into<String>, tone: Tone) -> Badge {
        Badge { text: text.into(), tone, size: TEXT_SIZE, tooltip: None }
    }

    /// The text's size, px (12 where a component sets `font-size: var(--text-sm)`).
    pub fn size(mut self, size: f32) -> Badge {
        self.size = size;
        self
    }

    /// The `title` (shown by [`super::tooltip::show`]).
    pub fn tooltip(mut self, text: impl Into<String>) -> Badge {
        self.tooltip = Some(text.into());
        self
    }

    /// The text's style.
    pub fn style(&self) -> Style {
        Style::ui(self.size, theme::WEIGHT_BADGE, self.tone.colors().1).line_height(theme::LINE_HEIGHT_BADGE)
    }

    /// The badge's size: its text's width and its line box (`size × 1.5`, not egui's row on a
    /// whole pixel) with the padding: 18.5 px high at 11 px, 20 at 12.
    pub fn measure(&self, ctx: &egui::Context) -> Vec2 {
        let style = self.style();
        Vec2::new(text::measure(ctx, text::job(&self.text, &style)).x, style.line_px()) + 2.0 * PADDING
    }

    /// Draws the badge with its top left corner at `at`, the text centred on its line box;
    /// answers its rectangle (the fill is snapped to whole pixels as it is drawn).
    pub fn paint(&self, painter: &Painter, at: Pos2) -> Rect {
        let style = self.style();
        let galley = painter.layout_job(text::job(&self.text, &style));
        let rect = Rect::from_min_size(at, Vec2::new(galley.size().x, style.line_px()) + 2.0 * PADDING);
        let (fill, color) = self.tone.colors();
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
        let top = rect.center().y - galley.size().y / 2.0;
        painter.galley(Pos2::new(at.x + PADDING.x, top), galley, color);
        rect
    }

    /// Takes the badge's space from `ui`'s layout and draws it.
    pub fn show(self, ui: &mut Ui) -> Response {
        let size = self.measure(ui.ctx());
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::hover());
        if ui.is_rect_visible(rect) {
            self.paint(ui.painter(), rect.min);
        }
        if let Some(tip) = &self.tooltip {
            response = super::tooltip::show(response, tip);
        }
        response
    }
}

impl egui::Widget for Badge {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_tones() {
        assert_eq!(Tone::Good.colors(), (Color32::from_rgba_unmultiplied(51, 171, 132, 41), theme::LIVE));
        assert_eq!(Tone::Warn.colors(), (Color32::from_rgba_unmultiplied(230, 134, 25, 41), theme::WARN));
        assert_eq!(Tone::Bad.colors(), (Color32::from_rgba_unmultiplied(236, 91, 98, 36), theme::DANGER));
        assert_eq!(Tone::Neutral.colors(), (theme::SURFACE_RAISED, theme::TEXT_2));
        for t in [Tone::Good, Tone::Warn, Tone::Bad, Tone::Neutral] {
            let name = format!("{t:?}").to_lowercase();
            assert_eq!(Tone::from_name(&name), Some(t));
        }
        assert_eq!(Tone::from(DelayTone::Warn), Tone::Warn);
        assert_eq!(Tone::from(time::Tone::Bad), Tone::Bad);
        assert_eq!(Tone::from(NavTone::Neutral), Tone::Neutral);
    }

    #[test]
    fn line_height_one_and_a_half_plus_padding() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let b = Badge::new("Old position", Tone::Warn);
        let text_w = text::measure(&ctx, text::job("Old position", &b.style())).x;
        let size = b.measure(&ctx);
        assert!((size.y - 18.5).abs() < 1e-4, "{size:?}");
        assert!((size.x - (text_w + 12.0)).abs() < 1e-4);
        assert!((Badge::new("Updated 2 min ago", Tone::Warn).size(12.0).measure(&ctx).y - 20.0).abs() < 1e-4);
    }
}
