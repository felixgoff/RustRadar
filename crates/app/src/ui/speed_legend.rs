//! The speed map's legend (G:3790-3797, styles G:3830-3859), from item T4's legend data
//! ([`rustradar_scene::rail::speed_legend`]): while the rail map shows the speed limits and the
//! page's shown zoom is 8 or more, a strip of the speed colours centred along the bottom of the
//! map, `bottom: calc(var(--sheet-h, 0px) + 26px)`.
//!
//! `.speed-legend`: padding 3 × 6 px, chips 2 px apart, radius `--radius-sm`, `oklch(0.15 0 0 /
//! 0.8)`, `font: 600 10px / 1 var(--font-data)` in `--text-2`; the unit `km/h` in `--text-3`
//! with 4 px after it; each `.band` at least 26 px wide, padded 3 × 4 px, radius 2, in its band's
//! colour (opaque) with `oklch(0.16 0.01 260)` text, centred. It is inside `.globe` at z-index 2:
//! over the map and the map's tooltip, under every panel; it takes no pointer
//! (`pointer-events: none`), so the `?` chip's title never shows, as today.
//!
//! The shown zoom is the page's `shownZoom` ([`next_shown_zoom`]: the view's zoom taken only when
//! it crosses a half step), kept in egui's memory.

use egui::{Align2, Color32, Context, CornerRadius, Id, LayerId, Order, Pos2, Rect, Vec2};
use rustradar_scene::rail::{SPEED_LEGEND_UNIT, next_shown_zoom, rail_speeds, speed_legend, speed_legend_shown};

use super::text::{self, Style};
use super::theme;
use crate::state::AppState;

/// From the bottom of the map (on phones, from the sheet's top), px.
pub const BOTTOM: f32 = 26.0;
/// `.speed-legend`'s padding, px (x, y).
pub const PADDING: Vec2 = Vec2::new(6.0, 3.0);
/// The flex gap between the unit and the chips, px.
pub const GAP: f32 = 2.0;
/// The unit's `margin-right`, px.
pub const UNIT_MARGIN: f32 = 4.0;
/// A chip's `min-width`, its padding (x, y) and its radius, px.
pub const BAND_MIN_WIDTH: f32 = 26.0;
pub const BAND_PADDING: Vec2 = Vec2::new(4.0, 3.0);
pub const BAND_RADIUS: u8 = 2;
/// `font: 600 10px / 1 var(--font-data)` (no `.data` letter spacing).
pub const FONT_SIZE: f32 = 10.0;
pub const FONT_WEIGHT: u16 = 600;

/// The layer the legend is painted on: over the map and its tooltip, under every panel.
pub fn layer() -> LayerId {
    LayerId::new(Order::Background, Id::unique("rustradar::speed_legend"))
}

fn style(color: Color32) -> Style {
    Style::data(FONT_SIZE, FONT_WEIGHT, color).line_height(1.0).letter_spacing(0.0)
}

/// The page's shown zoom after a frame at `zoom` ([`next_shown_zoom`], starting at 0).
fn shown_zoom(ctx: &Context, zoom: f64) -> f64 {
    let id = Id::unique("rustradar::speed_legend::shown_zoom");
    ctx.data_mut(|d| {
        let shown = d.get_temp_mut_or(id, 0.0_f64);
        *shown = next_shown_zoom(*shown, zoom);
        *shown
    })
}

/// Paints the legend over the whole of `ctx`'s screen with its bottom `sheet_height + BOTTOM`
/// above the screen's, and returns its box and each chip's.
pub fn paint(ctx: &Context, sheet_height: f32) -> (Rect, Vec<Rect>) {
    let screen = ctx.content_rect();
    let unit = ctx.fonts_mut(|f| f.layout_job(text::job(SPEED_LEGEND_UNIT, &style(theme::TEXT_3))));
    let chips = speed_legend();
    let galleys: Vec<_> = chips.iter().map(|c| ctx.fonts_mut(|f| f.layout_job(text::job(&c.text, &style(theme::SPEED_LEGEND_INK))))).collect();
    let band_width = |w: f32| (w + 2.0 * BAND_PADDING.x).max(BAND_MIN_WIDTH);
    let band_height = FONT_SIZE + 2.0 * BAND_PADDING.y;
    let content = Vec2::new(
        unit.size().x + UNIT_MARGIN + galleys.iter().map(|g| GAP + band_width(g.size().x)).sum::<f32>(),
        band_height.max(FONT_SIZE),
    );
    let size = content + 2.0 * PADDING;
    // left 50%, translateX(-50%); the box's edges on whole pixels, as the browser paints them
    let ppp = ctx.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let bottom = screen.bottom() - sheet_height - BOTTOM;
    let min = Pos2::new(snap(screen.center().x - size.x / 2.0), snap(bottom - size.y));
    let rect = Rect::from_min_size(min, size);
    let painter = ctx.layer_painter(layer()).with_clip_rect(screen);
    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_SM), theme::SPEED_LEGEND_BG);
    // align-items: center
    let middle = rect.center().y;
    let mut x = rect.min.x + PADDING.x;
    painter.galley(Pos2::new(x, middle - unit.size().y / 2.0), unit.clone(), theme::TEXT_3);
    x += unit.size().x + UNIT_MARGIN;
    let mut bands = Vec::with_capacity(chips.len());
    for (chip, galley) in chips.iter().zip(galleys) {
        x += GAP;
        let band = Rect::from_min_size(Pos2::new(x, middle - band_height / 2.0), Vec2::new(band_width(galley.size().x), band_height));
        let [r, g, b] = chip.rgb;
        painter.rect_filled(band, CornerRadius::same(BAND_RADIUS), Color32::from_rgb(r, g, b));
        let at = Align2::CENTER_CENTER.anchor_size(band.center(), galley.size()).min;
        painter.galley(at, galley, theme::SPEED_LEGEND_INK);
        bands.push(band);
        x = band.max.x;
    }
    (rect, bands)
}

/// The legend in the UI pass, when the rail map shows the speed limits from zoom 8 (G:3790), the
/// phone's sheet `sheet_height` px high (0 on the desktop). Returns where it was painted.
pub fn ui(ctx: &Context, state: &AppState, sheet_height: f32) -> Option<Rect> {
    let settings = &state.persisted.settings;
    let shown = shown_zoom(ctx, state.controller.view().zoom);
    speed_legend_shown(settings.rail_map, rail_speeds(settings.rail_map, settings.rail_shows), shown).then(|| paint(ctx, sheet_height).0)
}

#[cfg(test)]
mod tests {
    use rustradar_core::settings::RailShows;
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::rail::SPEED_BANDS;

    use super::*;
    use crate::state::Startup;

    fn state(zoom: f64) -> AppState {
        let view = ViewState { longitude: 18.07, latitude: 59.33, zoom, bearing: 0.0, pitch: 0.0, position: None };
        AppState::new(Startup::default(), None, None, None, view, [1440.0, 900.0])
    }

    fn pass(ctx: &Context, state: &AppState, sheet: f32) -> Option<Rect> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))),
            ..Default::default()
        };
        let mut shown = None;
        let mut output = ctx.run_ui(input, |ui| shown = super::ui(ui.ctx(), state, sheet));
        output.textures_delta.clear();
        shown
    }

    /// Shown with the rail map on in its speeds from the shown zoom 8 (a half step's lag), not
    /// while it shows the blocks or is off; centred, 22 px high, 26 px over the bottom (or the
    /// sheet), the chips at least 26 px wide in their order and colours.
    #[test]
    fn the_legend_shows_with_the_speed_map_from_zoom_8() {
        let ctx = Context::default();
        super::super::theme::apply(&ctx);
        let mut s = state(9.0);
        assert_eq!(pass(&ctx, &s, 0.0), None, "the rail map is off");
        s.persisted.settings.rail_map = true;
        s.persisted.settings.rail_shows = RailShows::Speeds;
        // the fonts exist from the first pass on
        pass(&ctx, &s, 0.0);
        let rect = pass(&ctx, &s, 0.0).expect("shown at zoom 9");
        assert_eq!(rect.height(), 22.0);
        assert_eq!(rect.bottom(), 900.0 - 26.0);
        assert!((rect.center().x - 720.0).abs() <= 0.5, "{rect:?}");
        let (_, bands) = paint(&ctx, 0.0);
        assert_eq!(bands.len(), SPEED_BANDS.len() + 1);
        for pair in bands.windows(2) {
            assert_eq!(pair[1].min.x - pair[0].max.x, GAP);
        }
        assert!(bands.iter().all(|b| b.width() >= BAND_MIN_WIDTH && b.height() == 16.0));
        // "250+" is wider than the minimum: 4 cells of Overpass Mono at 10 px and the padding
        assert!((bands[5].width() - (4.0 * 6.16 + 8.0)).abs() < 0.5, "{}", bands[5].width());
        // on a phone, over the sheet
        assert_eq!(pass(&ctx, &s, 300.0).expect("shown").bottom(), 900.0 - 300.0 - 26.0);
        // the blocks: no speeds, no legend
        s.persisted.settings.rail_shows = RailShows::Blocks;
        assert_eq!(pass(&ctx, &s, 0.0), None);
        // below zoom 8, none; the shown zoom follows only across a half step: 7.9 after 9 is taken
        s.persisted.settings.rail_shows = RailShows::Speeds;
        s.controller.set_view(ViewState { zoom: 7.9, ..s.controller.view() });
        assert_eq!(pass(&ctx, &s, 0.0), None);
    }
}
