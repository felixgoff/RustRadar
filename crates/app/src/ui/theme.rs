//! Design tokens (doc 06 §1; plan 1.5): every colour, size, radius and spacing of `src/app.css`'s
//! `:root` (app.css:18-66), the literal colours components write in `oklch()`, and [`apply`],
//! which puts them, the fonts of [`super::fonts`] and the look of `.surface` into an egui context.
//!
//! Units: CSS px, which are egui points.
//!
//! Colours are `Color32`, which egui keeps premultiplied: a translucent token is built from its
//! CSS value (`rgb(55 142 240 / 0.18)`) with its alpha rounded as Skia rounds a float alpha
//! (`round(a·255)`, halves up), so over a panel it blends to what the browser drew.
//!
//! The `oklch()` literals (doc 06 §1.1's second table) are converted exactly, as plan section 7
//! decides: CSS Color 4's OKLCH → OKLab → XYZ (D65) → linear sRGB → sRGB, each channel rounded to
//! 8 bits ([`oklch`], checked against every constant in a test). Doc 06's approximations differ in
//! two: the live pulse is `(105,217,141)`, not `(108,214,140)`, and the legend's warm white
//! `(241,238,231)`, not `(241,238,230)`.

use egui::{Color32, CornerRadius, FontId, Frame, Margin, Shadow, Stroke, TextStyle};

use super::fonts::{self, Face};

// ---- colours (app.css:18-42) ----

/// `--ink` `#1e1e1e`: the window's background, the canvas behind the globe, the "filed route"
/// code box, the switch knob when on.
pub const INK: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
/// `--surface` `#323232`: every floating panel.
pub const SURFACE: Color32 = Color32::from_rgb(0x32, 0x32, 0x32);
/// `--surface-raised` `#3e3e3e`: inputs, the neutral badge, the skeleton's base, photo
/// placeholders, `kbd`.
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(0x3e, 0x3e, 0x3e);
/// `--surface-hover` `#4a4a4a`: rows and buttons under the pointer, the skeleton's highlight.
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x4a, 0x4a, 0x4a);
/// `--line` `#444444`: panel borders, dividers.
pub const LINE: Color32 = Color32::from_rgb(0x44, 0x44, 0x44);
/// `--line-control` `#6e6e6e`: the borders of controls (button, input, the switch's track when
/// off), the scrollbar's thumb.
pub const LINE_CONTROL: Color32 = Color32::from_rgb(0x6e, 0x6e, 0x6e);
/// `--text-1` `#e3e3e3`: primary text.
pub const TEXT_1: Color32 = Color32::from_rgb(0xe3, 0xe3, 0xe3);
/// `--text-2` `#c4c4c4`: secondary text, the default colour of icons.
pub const TEXT_2: Color32 = Color32::from_rgb(0xc4, 0xc4, 0xc4);
/// `--text-3` `#a8a8a8`: tertiary text, labels (`dt`), placeholders.
pub const TEXT_3: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
/// `--amber` `#378ef0`: **the accent, Spectrum blue despite its name**: selection, the focus ring,
/// primary buttons, the caret.
pub const AMBER: Color32 = Color32::from_rgb(0x37, 0x8e, 0xf0);
/// `--amber-ink` `#ffffff`: text on the accent.
pub const AMBER_INK: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
/// `--amber-soft` `rgb(55 142 240 / 0.18)`: the background of what is pressed or selected.
pub const AMBER_SOFT: Color32 = Color32::from_rgba_unmultiplied_const(55, 142, 240, alpha(0.18));
/// `--live` `#33ab84`: "Live", early, good.
pub const LIVE: Color32 = Color32::from_rgb(0x33, 0xab, 0x84);
/// `--live-soft` `rgb(51 171 132 / 0.16)`: the good badge's background.
pub const LIVE_SOFT: Color32 = Color32::from_rgba_unmultiplied_const(51, 171, 132, alpha(0.16));
/// `--danger` `#ec5b62`: failures.
pub const DANGER: Color32 = Color32::from_rgb(0xec, 0x5b, 0x62);
/// `--danger-soft` `rgb(236 91 98 / 0.14)`: the error alert's and the bad badge's background.
pub const DANGER_SOFT: Color32 = Color32::from_rgba_unmultiplied_const(236, 91, 98, alpha(0.14));
/// `--warn` `#e68619`: delays, stale data (always with words, never colour alone).
pub const WARN: Color32 = Color32::from_rgb(0xe6, 0x86, 0x19);
/// `--warn-soft` `rgb(230 134 25 / 0.16)`: the warn alert's and badge's background.
pub const WARN_SOFT: Color32 = Color32::from_rgba_unmultiplied_const(230, 134, 25, alpha(0.16));
/// `--series-1` `#2680eb`: the altitude chart's line.
pub const SERIES_1: Color32 = Color32::from_rgb(0x26, 0x80, 0xeb);

// ---- literal colours of components (doc 06 §1.1, second table), converted exactly ----

/// `oklch(0.15 0 0 / 0.75)`: the dev readout's background (+page:1905).
pub const DEV_READOUT_BG: Color32 = Color32::from_rgba_unmultiplied_const(11, 11, 11, alpha(0.75));
/// `oklch(0.15 0 0 / 0.6)`: the attribution strip's background (+page:1918).
pub const ATTRIBUTION_BG: Color32 = Color32::from_rgba_unmultiplied_const(11, 11, 11, alpha(0.6));
/// `oklch(1 0 0 / 0.25)`: the attribution links' underline (+page:1929).
pub const ATTRIBUTION_UNDERLINE: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, alpha(0.25));
/// `oklch(0.15 0 0 / 0.7)`: the photo carousel's round previous and next buttons
/// (FlightPanel:826).
pub const PHOTO_BUTTON_BG: Color32 = Color32::from_rgba_unmultiplied_const(11, 11, 11, alpha(0.7));
/// `oklch(0 0 0 / 0.9)`: `text-shadow: 0 1px 2px` under a photo's credit and count
/// (FlightPanel:807, HoverCard:167, ShipPanel:340, TrainPanel:411).
pub const PHOTO_TEXT_SHADOW: Color32 = Color32::from_rgba_unmultiplied_const(0, 0, 0, alpha(0.9));
/// `oklch(0.95 0.01 85 / 0.6)`: the route legend's "Filed route" swatch (FlightPanel:1203).
pub const LEGEND_FILED: Color32 = Color32::from_rgba_unmultiplied_const(241, 238, 231, alpha(0.6));
/// `oklch(0.95 0.01 85 / 0.45)`: the route legend's "estimated departure" dashes
/// (FlightPanel:1209).
pub const LEGEND_ESTIMATE: Color32 = Color32::from_rgba_unmultiplied_const(241, 238, 231, alpha(0.45));
/// `oklch(0.96 0 0)`: the light tile behind train operators' logos (TrainPanel:425).
pub const LOGO_TILE: Color32 = Color32::from_rgb(242, 242, 242);
/// `oklch(0.8 0.15 152 / 0.6)`: the "Live" dot's pulse ring at its start (FlightPanel:1344); it
/// fades to `/ 0` (FlightPanel:1347).
pub const LIVE_PULSE: Color32 = Color32::from_rgba_unmultiplied_const(105, 217, 141, alpha(0.6));
/// The pulse ring's colour without its alpha, `oklch(0.8 0.15 152)`.
pub const LIVE_PULSE_RGB: [u8; 3] = [105, 217, 141];
/// `oklch(0.1 0 0 / 0.5)`: the backdrop behind the keys dialog and the shortcuts help
/// (KeysDialog:207, ShortcutsHelp:78).
pub const BACKDROP: Color32 = Color32::from_rgba_unmultiplied_const(3, 3, 3, alpha(0.5));
/// `oklch(0.12 0 0)`: the dev panel's code blocks (DevPanel:280).
pub const DEV_PANEL_CODE_BG: Color32 = Color32::from_rgb(6, 6, 6);
/// `oklch(0.15 0 0 / 0.8)`: the rail speed legend's background (G:3841).
pub const SPEED_LEGEND_BG: Color32 = Color32::from_rgba_unmultiplied_const(11, 11, 11, alpha(0.8));
/// `oklch(0.16 0.01 260)`: the text on the rail speed legend's coloured bands (G:3856).
pub const SPEED_LEGEND_INK: Color32 = Color32::from_rgb(11, 13, 18);
/// `oklch(from var(--amber) calc(l + 0.05) c h)`: the update toast's primary button under the
/// pointer (UpdateToast:90). The lightened accent leaves sRGB in blue (257.3); clipped to 255,
/// as Chromium draws it.
pub const AMBER_HOVER: Color32 = Color32::from_rgb(72, 158, 255);

// ---- typography (app.css:44-53, 68-70, 124-128) ----

/// `--text-xs`, 11 px: attribution, captions, badges, notes.
pub const TEXT_XS: f32 = 11.0;
/// `--text-sm`, 12 px: labels, secondary text.
pub const TEXT_SM: f32 = 12.0;
/// `--text-md`, 13 px: list rows, data, buttons.
pub const TEXT_MD: f32 = 13.0;
/// `--text-base`, 14 px: inputs, the body.
pub const TEXT_BASE: f32 = 14.0;
/// `--text-lg`, 17 px: the wordmark.
pub const TEXT_LG: f32 = 17.0;
/// `--text-xl`, 22 px: callsigns, airport codes.
pub const TEXT_XL: f32 = 22.0;
/// The root's font size (`:root { font-size: 16px }`): text that sets none.
pub const TEXT_ROOT: f32 = 16.0;
/// The attribution strip's text (+page:1920); [`TEXT_ATTRIBUTION_PHONE`] on phones.
pub const TEXT_ATTRIBUTION: f32 = 8.0;
/// The attribution strip's text on phones (+page:1983).
pub const TEXT_ATTRIBUTION_PHONE: f32 = 7.0;
/// The dev readout's monospace text (+page:1907), at line height [`LINE_HEIGHT_DEV_READOUT`].
pub const TEXT_DEV_READOUT: f32 = 10.0;
/// The dev readout's line height.
pub const LINE_HEIGHT_DEV_READOUT: f32 = 1.35;
/// The root's `line-height: 1.4`, inherited by all text unless a component sets its own.
pub const LINE_HEIGHT: f32 = 1.4;
/// `.badge`'s line height.
pub const LINE_HEIGHT_BADGE: f32 = 1.5;
/// `letter-spacing: -0.01em` of `.data` and of the wordmark, as a fraction of the font size.
pub const LETTER_SPACING_TIGHT: f32 = -0.01;

/// The weights in use (`font-weight`): 400 is the default.
pub const WEIGHT_REGULAR: u16 = 400;
/// The weight of `.button`.
pub const WEIGHT_BUTTON: u16 = 550;
/// The weight of `.badge`.
pub const WEIGHT_BADGE: u16 = 650;
/// The wordmark's weight (`h1`, +page:1809).
pub const WEIGHT_WORDMARK: u16 = 750;

// ---- spacing, radii, borders (app.css:55-66, 91-99, 140-145) ----

/// `--space-1`: every spacing is a multiple of it.
pub const SPACE: f32 = 4.0;
/// `--space-1`, 4 px.
pub const SPACE_1: f32 = 4.0;
/// `--space-2`, 8 px.
pub const SPACE_2: f32 = 8.0;
/// `--space-3`, 12 px.
pub const SPACE_3: f32 = 12.0;
/// `--space-4`, 16 px.
pub const SPACE_4: f32 = 16.0;
/// `--space-5`, 20 px.
pub const SPACE_5: f32 = 20.0;
/// `--space-6`, 24 px.
pub const SPACE_6: f32 = 24.0;

/// `--radius` (= `--radius-sm`), 4 px: every corner unless a component says otherwise.
pub const RADIUS: u8 = 4;
/// `--radius-sm`, the same 4 px.
pub const RADIUS_SM: u8 = 4;
/// The active-filter count pill's radius.
pub const RADIUS_PILL: u8 = 9;
/// The train operator logo tile's radius.
pub const RADIUS_LOGO_TILE: u8 = 3;
/// The legend and train colour swatches' radius.
pub const RADIUS_SWATCH: u8 = 2;
/// The tab underline's radius.
pub const RADIUS_TAB_UNDERLINE: u8 = 1;

/// Panel and control borders: 1 px.
pub const BORDER: f32 = 1.0;
/// `:focus-visible`: a 2 px solid `--amber` outline…
pub const FOCUS_RING: f32 = 2.0;
/// …2 px outside the element.
pub const FOCUS_RING_OFFSET: f32 = 2.0;
/// `scrollbar-width: thin` (app.css:96-99): the scroll thumb's width; the bar is always shown,
/// `--line-control` on a transparent track, with [`SCROLLBAR_MARGIN`] beside it.
pub const SCROLLBAR_WIDTH: f32 = 6.0;
/// Room beside the scroll thumb, inside the scrolled area's edge.
pub const SCROLLBAR_MARGIN: f32 = 2.0;

/// The gamma egui applies to glyph coverage (`alpha = coverage^0.8`): measured against today's
/// sidebar texts in `docs/port/reference/dark-v1-1440x900.png` (the wordmark, Filters, the
/// alert), whose ink it matches within 5 % (`tests/type_specimen.rs`).
pub const GLYPH_GAMMA: f32 = 0.8;

/// `.surface` (app.css:140-145): the opaque `--surface` panel with a 1 px `--line` border and
/// radius 4, no shadow, no blur (`--shadow: none`), and no padding of its own.
pub fn surface() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(BORDER, LINE))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::ZERO)
        .shadow(Shadow::NONE)
}

/// Puts the tokens into `ctx`: the fonts of [`fonts::install`] with egui's text styles in them,
/// the root's colours (text `--text-1`, selection `--amber` with `--amber-ink` text, the caret
/// `--amber`), panels as `.surface` (no shadows), radius 4, thin `--line-control` scrollbars, and
/// the shared controls' spacing. Call once after creating a context (and again for a new one).
pub fn apply(ctx: &egui::Context) {
    fonts::install(ctx);

    let mut style = (*ctx.global_style()).clone();
    style.visuals = egui::Visuals::dark();
    let v = &mut style.visuals;
    v.dark_mode = true; // color-scheme: dark
    v.panel_fill = SURFACE;
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(BORDER, LINE);
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.faint_bg_color = SURFACE_RAISED;
    // the scroll track is transparent (egui paints it in this colour); inputs are --surface-raised
    v.extreme_bg_color = Color32::TRANSPARENT;
    v.text_edit_bg_color = Some(SURFACE_RAISED);
    v.code_bg_color = SURFACE_RAISED; // kbd
    v.override_text_color = Some(TEXT_1);
    // glyph coverage → alpha: Chromium's light-on-dark text in the reference pictures holds as
    // much ink as a gamma of 0.8 gives (egui's dark default, 2c − c², draws it about 20 % bolder)
    v.text_options.color_transfer_function = egui::epaint::FontColorTransferFunction::Gamma(GLYPH_GAMMA);
    v.hyperlink_color = AMBER;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = AMBER; // ::selection
    v.selection.stroke = Stroke::new(1.0, AMBER_INK);
    v.text_cursor.stroke = Stroke::new(1.0, AMBER); // caret-color
    // egui's own widgets, which the app's widgets (ui::widgets) draw over: `.button`'s
    // transparent fill and --line-control border, --surface-hover under the pointer, pressed in
    // --amber-soft and --amber; their `bg_fill` is the scroll thumb's colour in every state
    let radius = CornerRadius::same(RADIUS);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = radius;
        w.expansion = 0.0;
        w.bg_fill = LINE_CONTROL;
        w.bg_stroke = Stroke::new(BORDER, LINE_CONTROL);
    }
    v.widgets.noninteractive.weak_bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = Stroke::new(BORDER, LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_2);
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_2);
    v.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_1);
    v.widgets.active.weak_bg_fill = AMBER_SOFT;
    v.widgets.active.bg_stroke = Stroke::new(BORDER, AMBER);
    v.widgets.active.fg_stroke = Stroke::new(1.0, AMBER);
    v.widgets.open = v.widgets.active;

    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(SPACE_2, SPACE_2);
    s.button_padding = egui::vec2(SPACE_3, 0.0);
    s.interact_size.y = 32.0;
    s.window_margin = Margin::same(SPACE_4 as i8);
    s.menu_margin = Margin::same(SPACE_2 as i8);
    s.scroll = egui::style::ScrollStyle {
        floating: false,
        bar_width: SCROLLBAR_WIDTH,
        bar_inner_margin: SCROLLBAR_MARGIN,
        bar_outer_margin: 0.0,
        foreground_color: false,
        ..egui::style::ScrollStyle::solid()
    };

    style.text_styles = [
        (TextStyle::Small, FontId::new(TEXT_SM, fonts::family(Face::Ui, WEIGHT_REGULAR))),
        (TextStyle::Body, FontId::new(TEXT_BASE, fonts::family(Face::Ui, WEIGHT_REGULAR))),
        (TextStyle::Button, FontId::new(TEXT_MD, fonts::family(Face::Ui, WEIGHT_BUTTON))),
        (TextStyle::Heading, FontId::new(TEXT_LG, fonts::family(Face::Ui, WEIGHT_WORDMARK))),
        (TextStyle::Monospace, FontId::new(TEXT_MD, fonts::family(Face::Mono, WEIGHT_REGULAR))),
    ]
    .into();
    ctx.set_global_style(style);
}

/// A CSS alpha in 8 bits as Skia turns its float alpha into one: `a·255 + 0.5` in `f32`,
/// truncated (`0.7` is 179: `0.7·255` rounds to 178.5 in `f32`).
pub const fn alpha(a: f32) -> u8 {
    (a * 255.0 + 0.5) as u8
}

/// The sRGB colour of CSS's `oklch(l c h / a)` (lightness 0–1, chroma, hue in degrees), each
/// channel clipped to sRGB and rounded to 8 bits: CSS Color 4's conversion (OKLCH → OKLab → LMS →
/// XYZ D65 → linear sRGB → the sRGB transfer function).
pub fn oklch(l: f64, c: f64, h: f64, a: f64) -> Color32 {
    let [r, g, b] = oklch_srgb(l, c, h).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Color32::from_rgba_unmultiplied(r, g, b, alpha(a.clamp(0.0, 1.0) as f32))
}

/// `oklch(l c h)` as unclipped sRGB channels, 0–1 inside the gamut.
pub fn oklch_srgb(l: f64, c: f64, h: f64) -> [f64; 3] {
    let (sin, cos) = h.to_radians().sin_cos();
    let lab = [l, c * cos, c * sin];
    // CSS Color 4, sample code (§18): OKLab → LMS (cube roots) → LMS → XYZ D65 → linear sRGB
    const OKLAB_TO_LMS: [[f64; 3]; 3] = [
        [1.0, 0.396_337_777_376_174_9, 0.215_803_757_309_913_6],
        [1.0, -0.105_561_345_815_658_6, -0.063_854_172_825_813_3],
        [1.0, -0.089_484_177_529_811_9, -1.291_485_548_019_409_2],
    ];
    const LMS_TO_XYZ: [[f64; 3]; 3] = [
        [1.226_879_875_845_924_3, -0.557_814_994_460_217_1, 0.281_391_045_665_964_7],
        [-0.040_575_745_214_800_8, 1.112_286_803_280_317, -0.071_711_058_065_516_4],
        [-0.076_372_936_674_660_1, -0.421_493_332_402_243_2, 1.586_924_019_836_781_6],
    ];
    const XYZ_TO_LINEAR_SRGB: [[f64; 3]; 3] = [
        [12831.0 / 3959.0, -329.0 / 214.0, -1974.0 / 3959.0],
        [-851781.0 / 878810.0, 1648619.0 / 878810.0, 36519.0 / 878810.0],
        [705.0 / 12673.0, -2585.0 / 12673.0, 705.0 / 667.0],
    ];
    let mul = |m: &[[f64; 3]; 3], v: [f64; 3]| [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]);
    let lms = mul(&OKLAB_TO_LMS, lab).map(|v| v * v * v);
    let linear = mul(&XYZ_TO_LINEAR_SRGB, mul(&LMS_TO_XYZ, lms));
    linear.map(|v| {
        let magnitude = v.abs();
        let encoded = if magnitude > 0.003_130_8 { 1.055 * magnitude.powf(1.0 / 2.4) - 0.055 } else { 12.92 * magnitude };
        encoded.copysign(v)
    })
}

/// The OKLCH lightness, chroma and hue (degrees) of an sRGB colour: the inverse of
/// [`oklch_srgb`], for CSS's relative colours (`oklch(from var(--amber) …)`).
pub fn srgb_oklch(rgb: [u8; 3]) -> [f64; 3] {
    let linear = rgb.map(|c| {
        let c = f64::from(c) / 255.0;
        if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    });
    // Ottosson's matrices (the inverse of the ones above, to 1e-9)
    let l = 0.412_221_470_8 * linear[0] + 0.536_332_536_3 * linear[1] + 0.051_445_992_9 * linear[2];
    let m = 0.211_903_498_2 * linear[0] + 0.680_699_545_1 * linear[1] + 0.107_396_956_6 * linear[2];
    let s = 0.088_302_461_9 * linear[0] + 0.281_718_837_6 * linear[1] + 0.629_978_700_5 * linear[2];
    let [l, m, s] = [l, m, s].map(f64::cbrt);
    let lightness = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
    let b = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
    [lightness, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unmultiplied(c: Color32) -> [u8; 4] {
        c.to_srgba_unmultiplied()
    }

    #[test]
    fn root_colours_are_app_css() {
        let hex = |c: Color32| format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b());
        for (token, css) in [
            (INK, "#1e1e1e"),
            (SURFACE, "#323232"),
            (SURFACE_RAISED, "#3e3e3e"),
            (SURFACE_HOVER, "#4a4a4a"),
            (LINE, "#444444"),
            (LINE_CONTROL, "#6e6e6e"),
            (TEXT_1, "#e3e3e3"),
            (TEXT_2, "#c4c4c4"),
            (TEXT_3, "#a8a8a8"),
            (AMBER, "#378ef0"),
            (AMBER_INK, "#ffffff"),
            (LIVE, "#33ab84"),
            (DANGER, "#ec5b62"),
            (WARN, "#e68619"),
            (SERIES_1, "#2680eb"),
        ] {
            assert_eq!(hex(token), css);
            assert_eq!(token.a(), 255, "{css} is opaque");
        }
        // the acceptance's panel: fill (50,50,50), border (68,68,68)
        assert_eq!(SURFACE.to_array(), [50, 50, 50, 255]);
        assert_eq!(LINE.to_array(), [68, 68, 68, 255]);
        let frame = surface();
        assert_eq!((frame.fill, frame.stroke), (SURFACE, Stroke::new(1.0, LINE)));
        assert_eq!(frame.corner_radius, CornerRadius::same(4));
        assert_eq!(frame.shadow, Shadow::NONE);
    }

    #[test]
    fn soft_colours_keep_their_css_alpha() {
        assert_eq!([alpha(0.18), alpha(0.16), alpha(0.14), alpha(0.7), alpha(0.25), alpha(0.45)], [46, 41, 36, 179, 64, 115]);
        for (token, rgb, a) in [
            (AMBER_SOFT, [55, 142, 240], 0.18),
            (LIVE_SOFT, [51, 171, 132], 0.16),
            (DANGER_SOFT, [236, 91, 98], 0.14),
            (WARN_SOFT, [230, 134, 25], 0.16),
        ] {
            assert_eq!(token.a(), alpha(a));
            let back = unmultiplied(token);
            for i in 0..3 {
                assert!(back[i].abs_diff(rgb[i]) <= 3, "{back:?} for {rgb:?}");
            }
        }
        // over a panel the pressed blue is what the browser blends: 0.18·blue + 0.82·surface
        let blend = |c: Color32, under: Color32| {
            let k = 1.0 - f32::from(c.a()) / 255.0;
            [c.r(), c.g(), c.b()].iter().zip([under.r(), under.g(), under.b()]).map(|(&c, u)| (f32::from(c) + f32::from(u) * k).round() as u8).collect::<Vec<_>>()
        };
        assert_eq!(blend(AMBER_SOFT, SURFACE), [51, 67, 84]);
    }

    #[test]
    fn oklch_literals_are_converted_exactly() {
        // (token, oklch l c h / alpha) as the components write them
        let literals = [
            (DEV_READOUT_BG, [0.15, 0.0, 0.0, 0.75]),
            (ATTRIBUTION_BG, [0.15, 0.0, 0.0, 0.6]),
            (ATTRIBUTION_UNDERLINE, [1.0, 0.0, 0.0, 0.25]),
            (PHOTO_BUTTON_BG, [0.15, 0.0, 0.0, 0.7]),
            (PHOTO_TEXT_SHADOW, [0.0, 0.0, 0.0, 0.9]),
            (LEGEND_FILED, [0.95, 0.01, 85.0, 0.6]),
            (LEGEND_ESTIMATE, [0.95, 0.01, 85.0, 0.45]),
            (LOGO_TILE, [0.96, 0.0, 0.0, 1.0]),
            (LIVE_PULSE, [0.8, 0.15, 152.0, 0.6]),
            (BACKDROP, [0.1, 0.0, 0.0, 0.5]),
            (DEV_PANEL_CODE_BG, [0.12, 0.0, 0.0, 1.0]),
            (SPEED_LEGEND_BG, [0.15, 0.0, 0.0, 0.8]),
            (SPEED_LEGEND_INK, [0.16, 0.01, 260.0, 1.0]),
        ];
        for (token, [l, c, h, a]) in literals {
            assert_eq!(token, oklch(l, c, h, a), "oklch({l} {c} {h} / {a})");
        }
        // the ones whose channels are given separately
        assert_eq!(Color32::from_rgb(LIVE_PULSE_RGB[0], LIVE_PULSE_RGB[1], LIVE_PULSE_RGB[2]), oklch(0.8, 0.15, 152.0, 1.0));
        // white and black are exact; a neutral grey has three equal channels
        assert_eq!(oklch_srgb(1.0, 0.0, 0.0).map(|v| (v * 1e6).round()), [1e6; 3]);
        assert_eq!(oklch_srgb(0.0, 0.0, 0.0), [0.0; 3]);
        let [r, g, b] = oklch_srgb(0.15, 0.0, 0.0);
        assert!((r - g).abs() < 1e-9 && (g - b).abs() < 1e-9 && (r * 255.0 - 11.088).abs() < 0.01, "{r}");
    }

    #[test]
    fn the_lightened_accent_is_clipped_to_srgb() {
        let [l, c, h] = srgb_oklch([0x37, 0x8e, 0xf0]);
        assert!((l - 0.6435).abs() < 1e-3 && (c - 0.1683).abs() < 1e-3 && (h - 254.06).abs() < 0.05, "{l} {c} {h}");
        // the round trip lands on --amber
        assert_eq!(oklch(l, c, h, 1.0), AMBER);
        let lighter = oklch_srgb(l + 0.05, c, h);
        assert!(lighter[2] > 1.0, "blue leaves the gamut: {lighter:?}");
        assert_eq!(oklch(l + 0.05, c, h, 1.0), AMBER_HOVER);
    }

    #[test]
    fn scales_are_app_css() {
        assert_eq!([TEXT_XS, TEXT_SM, TEXT_MD, TEXT_BASE, TEXT_LG, TEXT_XL], [11.0, 12.0, 13.0, 14.0, 17.0, 22.0]);
        assert_eq!([SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6], [4.0, 8.0, 12.0, 16.0, 20.0, 24.0]);
        assert_eq!([SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5, SPACE_6], [1.0, 2.0, 3.0, 4.0, 5.0, 6.0].map(|n| n * SPACE));
        assert_eq!((RADIUS, RADIUS_SM), (4, 4));
    }

    #[test]
    fn apply_sets_the_root_look() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let style = ctx.global_style();
        assert_eq!(style.visuals.panel_fill, SURFACE);
        assert_eq!(style.visuals.window_stroke, Stroke::new(1.0, LINE));
        assert_eq!(style.visuals.window_shadow, Shadow::NONE);
        assert_eq!(style.visuals.override_text_color, Some(TEXT_1));
        assert_eq!(style.visuals.selection.bg_fill, AMBER);
        assert_eq!(style.visuals.text_cursor.stroke.color, AMBER);
        assert_eq!(style.visuals.text_options.color_transfer_function, egui::epaint::FontColorTransferFunction::Gamma(0.8));
        let body = &style.text_styles[&TextStyle::Body];
        assert_eq!((body.size, &body.family), (TEXT_BASE, &fonts::family(Face::Ui, 400)));
        let mono = &style.text_styles[&TextStyle::Monospace];
        assert_eq!(mono.family, fonts::family(Face::Mono, 400));
    }
}
