//! Design tokens, mirrored from `src/app.css` (`:root`, lines 18-66).

use egui::Color32;

/// Map canvas behind everything: `#1c1c1c`.
pub const CANVAS: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1c);
/// `--ink`
pub const INK: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
/// `--surface`: panels.
pub const SURFACE: Color32 = Color32::from_rgb(0x32, 0x32, 0x32);
/// `--surface-raised`
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(0x3e, 0x3e, 0x3e);
/// `--surface-hover`
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x4a);
/// `--line`
pub const LINE: Color32 = Color32::from_rgb(0x44, 0x44, 0x44);
/// `--line-control`
pub const LINE_CONTROL: Color32 = Color32::from_rgb(0x6e, 0x6e, 0x6e);
/// `--text-1`
pub const TEXT_1: Color32 = Color32::from_rgb(0xe3, 0xe3, 0xe3);
/// `--text-2`
pub const TEXT_2: Color32 = Color32::from_rgb(0xc4, 0xc4, 0xc4);
/// `--text-3`
pub const TEXT_3: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
/// `--amber` (Spectrum blue despite the name): selection, primary actions.
pub const ACCENT: Color32 = Color32::from_rgb(0x37, 0x8e, 0xf0);
/// `--amber-ink`
pub const ACCENT_INK: Color32 = Color32::WHITE;

/// `--text-sm` .. `--text-xl`, in CSS px (= egui points).
pub const TEXT_XS: f32 = 11.0;
pub const TEXT_SM: f32 = 12.0;
pub const TEXT_MD: f32 = 13.0;
pub const TEXT_BASE: f32 = 14.0;
pub const TEXT_LG: f32 = 17.0;
pub const TEXT_XL: f32 = 22.0;

/// `--radius`
pub const RADIUS: u8 = 4;
/// `--space-1`: every spacing is a multiple of this.
pub const SPACE: f32 = 4.0;

/// Applies the tokens to an egui context. Call once after creating it.
pub fn apply(ctx: &egui::Context) {
    use egui::{CornerRadius, FontFamily, FontId, Stroke, TextStyle};

    // Overpass ships in node_modules only as woff2 (@fontsource-variable),
    // which egui's font loader cannot read: it needs TTF/OTF. Until a
    // TTF is vendored, egui's built-in font stands in. To switch:
    // `fonts.font_data.insert("overpass", FontData::from_static(include_bytes!(..)).into())`
    // and put "overpass" first in `fonts.families[Proportional]`.

    let mut style = (*ctx.global_style()).clone();
    style.visuals = egui::Visuals::dark();
    let v = &mut style.visuals;
    v.panel_fill = SURFACE;
    v.window_fill = SURFACE;
    v.extreme_bg_color = INK; // text field background
    v.override_text_color = Some(TEXT_1);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, ACCENT_INK);
    let radius = CornerRadius::same(RADIUS);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    v.widgets.inactive.bg_fill = SURFACE_RAISED;
    v.widgets.inactive.weak_bg_fill = SURFACE_RAISED;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE_CONTROL);
    v.widgets.hovered.bg_fill = SURFACE_HOVER;
    v.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, LINE_CONTROL);
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT;

    style.spacing.item_spacing = egui::vec2(2.0 * SPACE, 2.0 * SPACE);
    style.spacing.button_padding = egui::vec2(3.0 * SPACE, 1.5 * SPACE);
    style.text_styles = [
        (TextStyle::Small, FontId::new(TEXT_SM, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(TEXT_BASE, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(TEXT_BASE, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(TEXT_LG, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(TEXT_MD, FontFamily::Monospace)),
    ]
    .into();
    ctx.set_global_style(style);
}
