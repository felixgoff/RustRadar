//! `.alert` (+page:1863-1892; doc 06 §2.8): the sidebar's failure and warning box.
//!
//! `display: flex; gap: 8px; padding: 12px`, radius 4, no border; [`Kind::Error`] on
//! `--danger-soft` with its `TriangleAlert` icon (16 px, stroke 1.75, `margin-top: 1px`) in
//! `--danger`, [`Kind::Warn`] (`.alert.warn`) on `--warn-soft` in `--warn`. Beside the icon a
//! column (`gap: 8px`, left-aligned) holds the main text (13 px `--text-1`, line height 1.4), then
//! the detail (`.detail`: 12 px `--text-2`, `word-break: break-word`), then whatever the caller
//! adds ([`Alert::show_with`]: the feed failure's `Retry now` button). The texts wrap at word
//! boundaries within the column (a word wider than it is broken where it must be). The alert
//! takes the whole width it is given.
//!
//! Today's texts (doc 06 §2.8): `Flightradar24 is limiting requests. Slowing down; next update in
//! {retryIn} s.` (warn), `Can't reach Flightradar24. Trying again in {retryIn} s.` with the error
//! as its detail (error).

use egui::{Align, CornerRadius, Frame, Layout, Margin, Pos2, Rect, Response, Sense, Ui, UiBuilder, Vec2};

use super::super::icons::{self, Icon};
use super::super::text::{self, Style};
use super::super::theme;

/// The icon's size, px.
pub const ICON_SIZE: f32 = 16.0;
/// The icon's `margin-top`, px.
pub const ICON_TOP: f32 = 1.0;
/// Padding, px.
pub const PADDING: f32 = theme::SPACE_3;
/// The gap between the icon and the column, and between the column's parts, px.
pub const GAP: f32 = theme::SPACE_2;

/// Error or warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `.alert`: `--danger-soft` and `--danger` (`role="alert"`).
    Error,
    /// `.alert.warn`: `--warn-soft` and `--warn` (`role="status"`).
    Warn,
}

impl Kind {
    /// The fill and the icon's colour.
    pub const fn colors(self) -> (egui::Color32, egui::Color32) {
        match self {
            Kind::Error => (theme::DANGER_SOFT, theme::DANGER),
            Kind::Warn => (theme::WARN_SOFT, theme::WARN),
        }
    }
}

/// The main text's style.
pub const fn text_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1)
}

/// The detail's style.
pub const fn detail_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// An `.alert`.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    kind: Kind,
    text: String,
    detail: Option<String>,
}

impl Alert {
    /// An error alert saying `text`.
    pub fn error(text: impl Into<String>) -> Alert {
        Alert { kind: Kind::Error, text: text.into(), detail: None }
    }

    /// A warning alert saying `text`.
    pub fn warn(text: impl Into<String>) -> Alert {
        Alert { kind: Kind::Warn, text: text.into(), detail: None }
    }

    /// The detail under the text.
    pub fn detail(mut self, detail: impl Into<String>) -> Alert {
        self.detail = Some(detail.into());
        self
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Draws the alert across `ui`'s available width.
    pub fn show(self, ui: &mut Ui) -> Response {
        self.show_with(ui, |_| {})
    }

    /// Draws the alert across `ui`'s available width with `add` putting more in the column under
    /// the texts (8 px below them, left-aligned, as wide as it wants).
    pub fn show_with(self, ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> Response {
        let (fill, tint) = self.kind.colors();
        let role = match self.kind {
            Kind::Error => egui::Role::Alert,
            Kind::Warn => egui::Role::Status,
        };
        let inner = Frame::new().fill(fill).corner_radius(CornerRadius::same(theme::RADIUS_SM)).inner_margin(Margin::same(PADDING as i8)).show(ui, |ui| {
            let width = ui.available_width();
            ui.set_width(width);
            ui.set_min_height(ICON_TOP + ICON_SIZE);
            let top_left = ui.cursor().min;
            let icon = Rect::from_min_size(top_left + Vec2::new(0.0, ICON_TOP), Vec2::splat(ICON_SIZE));
            icons::paint(ui.painter(), icon, Icon::TriangleAlert.at(ICON_SIZE, icons::STROKE_THIN), tint);
            let column_left = ICON_SIZE + GAP;
            let column = Rect::from_min_max(top_left + Vec2::new(column_left, 0.0), Pos2::new(top_left.x + width, f32::MAX / 4.0));
            ui.scope_builder(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)), |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(GAP, GAP);
                let w = column.width();
                text::label(ui, text::wrapped(&self.text, &text_style(), w), Sense::hover());
                if let Some(detail) = &self.detail {
                    text::label(ui, text::wrapped(detail, &detail_style(), w), Sense::hover());
                }
                add(ui);
            });
        });
        let response = inner.response;
        let label = match &self.detail {
            Some(d) => format!("{} {d}", self.text),
            None => self.text.clone(),
        };
        response.widget_info(|| egui::WidgetInfo::labeled(role, true, &label));
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::widgets::button::Button;

    fn laid_out(width: f32, f: impl FnOnce(&mut Ui) -> Response) -> (Response, egui::Context) {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut f = Some(f);
        let mut out = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::Area::new(egui::Id::unique("alert")).fixed_pos(Pos2::new(33.0, 163.0)).show(ui.ctx(), |ui| {
                ui.set_width(width);
                if let Some(f) = f.take() {
                    out = Some(f(ui));
                }
            });
        });
        output.textures_delta.clear();
        (out.expect("drawn"), ctx)
    }

    #[test]
    fn todays_plain_browser_alert_is_84_px_high() {
        // the reference picture's alert: 286 wide at (33, 163), 84 px high (its texts on three
        // rows: 12 + 18.2 + 8 + 2 × 16.8 + 12 = 83.8, drawn over 84 pixels); egui sets each row
        // on whole pixels (18 and 17 px)
        let (r, _) = laid_out(286.0, |ui| {
            Alert::error("No live flight data in a plain browser.").detail("RustRadar needs its Rust backend. Start it with bun tauri dev.").show(ui)
        });
        assert_eq!(r.rect.min, Pos2::new(33.0, 163.0));
        assert_eq!(r.rect.width(), 286.0);
        assert_eq!(r.rect.height(), 12.0 + 18.0 + 8.0 + 2.0 * 17.0 + 12.0, "{:?}", r.rect);
    }

    #[test]
    fn a_warning_alone_is_its_one_row_and_the_padding() {
        let (r, ctx) = laid_out(286.0, |ui| Alert::warn("Flightradar24 is limiting requests.").show(ui));
        let row = text::measure(&ctx, text::job("x", &text_style())).y;
        assert!((r.rect.height() - (row + 24.0)).abs() < 0.01, "{:?}", r.rect);
    }

    #[test]
    fn the_retry_button_comes_8_px_under_the_detail() {
        let mut button = Rect::NOTHING;
        let (r, _) = laid_out(286.0, |ui| {
            Alert::error("Can't reach Flightradar24. Trying again in 12 s.").detail("error sending request").show_with(ui, |ui| {
                button = Button::new().icon(super::super::button::icon(Icon::RefreshCw)).text("Retry now").show(ui).rect;
            })
        });
        assert_eq!(button.min.x, 33.0 + 12.0 + 16.0 + 8.0);
        assert!((r.rect.max.y - 12.0 - button.max.y).abs() < 0.01);
        assert_eq!(button.height(), 32.0);
    }
}
