//! The map error card (item B9; doc 01 §9.5): `Globe.svelte`'s `{#if mapError}` card
//! (G:3799-3808, styles G:3859-3883) with `mapErrorDetails` and `retryMap` (G:1260-1277).
//!
//! While the map is stopped by an error ([`MapRenderer::error`]: the first wgpu error, or the
//! device lost, caught once the window has called [`MapRenderer::catch_errors`]), a card in the
//! middle of the map says "The map couldn't draw", the error's message in the data font, and has
//! two buttons: "Copy details" puts the message, the graphics adapter and the system on the
//! clipboard with everything wgpu said ([`details`]), and "Retry" clears the error so the map
//! draws again ([`MapRenderer::retry`]). After a lost device Retry needs a new device: [`ui`] then
//! answers [`Retry::Rebuild`], and the app opens one and calls [`MapRenderer::rebuild`] (today's
//! Retry reloads the page; plan 2.2 has the app rebuild at once when the device is lost, so the
//! card shows only if the rebuilt map fails too).
//!
//! The card, as today's CSS lays it out (content-box, so padding and border come on top of the
//! width; the buttons are `border-box`, as Chromium's `<button>`):
//! - centred in the map (`top: 50%; left: 50%; translate(-50%, -50%)`), `--surface` with a 1 px
//!   `--line` border, radius 4, padding `--space-4` (16), width `min(420px, 100% − 2·16px)`;
//! - the title at 14 px (the body's size), `--text-1`, line height 1.4;
//! - the message 12 px (`--text-sm`) in the data font, `--text-2`, line height 1.4, 8 px above and
//!   12 px below, wrapped anywhere (`overflow-wrap: anywhere`);
//! - the buttons in a row 8 px apart: `.button` (height 32, padding 0 12, 1 px `--line-control`
//!   border, radius 4, transparent, 13 px, `--text-1`; `--surface-hover` under the pointer, eased
//!   in over 150 ms with `--ease-out`).
//!
//! Deviations, until the design system of stage 7 exists (U1: `ui::theme`, `ui::fonts`,
//! `ui::widgets`): the text is in egui's own fonts (Overpass at weights 600 and 550, and Overpass
//! Mono, come with `ui::fonts`), and "Copy details" names the app's version and system where
//! today names the browser (`Browser: <user agent>` → `System: RustRadar <version> (<os> <arch>)`)
//! and the adapter where today names WebGL's vendor, renderer and versions.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use rustradar_map::MapRenderer;
use rustradar_map::renderer::{MapError, Retry};

use crate::theme;

/// The card's title.
pub const TITLE: &str = "The map couldn't draw";
/// The button that copies [`details`].
pub const COPY_DETAILS: &str = "Copy details";
/// The button that clears the error ([`MapRenderer::retry`]).
pub const RETRY: &str = "Retry";

/// The card's widest content, CSS px (`min(420px, …)`).
pub const MAX_WIDTH: f32 = 420.0;
/// `--space-4`: the card's padding, and its distance from the map's sides at the narrowest.
pub const PADDING: f32 = 4.0 * theme::SPACE;
/// The card's border, px.
pub const BORDER: f32 = 1.0;
/// The root's `line-height`.
pub const LINE_HEIGHT: f32 = 1.4;
/// The title's size: the body's `--text-base`.
pub const TITLE_SIZE: f32 = theme::TEXT_BASE;
/// The message's size, `--text-sm`.
pub const MESSAGE_SIZE: f32 = theme::TEXT_SM;
/// Above the message (`--space-2`).
pub const MESSAGE_ABOVE: f32 = 2.0 * theme::SPACE;
/// Below the message (`--space-3`).
pub const MESSAGE_BELOW: f32 = 3.0 * theme::SPACE;
/// Between the buttons (`--space-2`).
pub const BUTTON_GAP: f32 = 2.0 * theme::SPACE;
/// `.button`'s height.
pub const BUTTON_HEIGHT: f32 = 32.0;
/// `.button`'s side padding (`--space-3`).
pub const BUTTON_PADDING: f32 = 3.0 * theme::SPACE;
/// `.button`'s text, `--text-md`.
pub const BUTTON_TEXT_SIZE: f32 = theme::TEXT_MD;
/// `.button`'s background transition.
pub const HOVER_SECONDS: f32 = 0.15;

/// The card's content width on a map `map_width` CSS px wide: `min(420px, 100% − 2·16px)`.
pub fn content_width(map_width: f32) -> f32 {
    MAX_WIDTH.min(map_width - 2.0 * PADDING).max(0.0)
}

/// "Copy details" (`mapErrorDetails`, G:1260-1269): the message, the graphics adapter (`None`:
/// there is none), the app and its system, then everything wgpu said.
pub fn details(error: &MapError, adapter: Option<&wgpu::AdapterInfo>) -> String {
    let renderer = match adapter {
        Some(info) => {
            let device_type = format!("{:?}", info.device_type);
            [info.name.as_str(), info.driver.as_str(), info.driver_info.as_str(), info.backend.to_str(), device_type.as_str()]
                .into_iter()
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" | ")
        }
        None => "no graphics device".to_string(),
    };
    [
        format!("RustRadar map error: {}", error.message),
        format!("Renderer: {renderer}"),
        format!("System: {}", system()),
        String::new(),
        error.details.clone(),
    ]
    .join("\n")
}

/// The app and the system it runs on, where today's details give the browser's user agent.
pub fn system() -> String {
    format!("RustRadar {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH)
}

/// What a frame of the card did, and where its parts are (screen points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Card {
    /// The card, border included.
    pub rect: Rect,
    pub copy_details: Rect,
    pub retry: Rect,
    /// "Copy details" was clicked: the details are on the clipboard.
    pub copied: bool,
    /// "Retry" was clicked.
    pub retried: bool,
}

/// One frame of the card for `error` over the map (the whole of `ctx`'s screen), the details
/// built for `adapter` when they are copied. Like every egui area it is laid out on its first frame
/// and shown from its second.
pub fn show(ctx: &egui::Context, error: &MapError, adapter: Option<&wgpu::AdapterInfo>) -> Card {
    let map = ctx.content_rect();
    let width = content_width(map.width());
    let mut card = Card { rect: Rect::NOTHING, copy_details: Rect::NOTHING, retry: Rect::NOTHING, copied: false, retried: false };
    let response = egui::Area::new(Id::unique("map_error"))
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .fade_in(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(Stroke::new(BORDER, theme::LINE))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(PADDING)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    ui.set_width(width);
                    text(ui, TITLE, TITLE_SIZE, FontFamily::Proportional, theme::TEXT_1, width);
                    ui.add_space(MESSAGE_ABOVE);
                    text(ui, &error.message, MESSAGE_SIZE, FontFamily::Monospace, theme::TEXT_2, width);
                    ui.add_space(MESSAGE_BELOW);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(BUTTON_GAP, 0.0);
                        let copy = button(ui, COPY_DETAILS);
                        let retry = button(ui, RETRY);
                        card.copy_details = copy.rect;
                        card.retry = retry.rect;
                        if copy.clicked() {
                            ui.ctx().copy_text(details(error, adapter));
                            card.copied = true;
                        }
                        card.retried = retry.clicked();
                    });
                });
        });
    card.rect = response.response.rect;
    card
}

/// What the card needs of the map: [`MapRenderer`]'s error and Retry (a stand-in in tests).
pub trait StoppedMap {
    /// The error that stopped the map, if one did ([`MapRenderer::error`]).
    fn error(&self) -> Option<MapError>;
    /// The card's Retry ([`MapRenderer::retry`]).
    fn retry(&mut self) -> Retry;
}

impl StoppedMap for MapRenderer {
    fn error(&self) -> Option<MapError> {
        MapRenderer::error(self)
    }

    fn retry(&mut self) -> Retry {
        MapRenderer::retry(self)
    }
}

/// The card for `map` in the UI pass: shown while the map is stopped by an error, its buttons
/// carried out. Returns what Retry asked for: [`Retry::Redraw`] (the error is cleared, the map
/// draws again) or [`Retry::Rebuild`] (the device is lost: open a new one, then
/// [`MapRenderer::rebuild`]); `None` when there is no card or Retry was not clicked.
pub fn ui(ctx: &egui::Context, map: &mut impl StoppedMap, adapter: Option<&wgpu::AdapterInfo>) -> Option<Retry> {
    let error = map.error()?;
    let card = show(ctx, &error, adapter);
    card.retried.then(|| map.retry())
}

/// A paragraph of `size` px text in `family` and `color`, at line height 1.4, wrapped within
/// `width` (anywhere in a word when a word is wider): as tall as its CSS line boxes.
fn text(ui: &mut egui::Ui, text: &str, size: f32, family: FontFamily, color: Color32, width: f32) {
    let mut job = LayoutJob::single_section(
        text.to_string(),
        TextFormat { font_id: FontId::new(size, family), color, line_height: Some(size * LINE_HEIGHT), ..Default::default() },
    );
    job.wrap.max_width = width;
    let galley = ui.painter().layout_job(job);
    // the rows' CSS height: egui puts each row on a whole pixel, which is no more than half a
    // pixel off its line box
    let height = galley.rows.len() as f32 * size * LINE_HEIGHT;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    ui.painter().galley(rect.min, galley, color);
}

/// `.button`: 32 px high, 12 px either side of its 13 px text, a 1 px `--line-control` border,
/// radius 4, transparent; `--surface-hover` under the pointer.
fn button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let format = TextFormat { font_id: FontId::new(BUTTON_TEXT_SIZE, FontFamily::Proportional), color: theme::TEXT_1, ..Default::default() };
    let galley = ui.painter().layout_job(LayoutJob::single_section(label.to_string(), format));
    let size = Vec2::new(galley.size().x + 2.0 * BUTTON_PADDING + 2.0 * BORDER, BUTTON_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool_with_time_and_easing(response.id, response.hovered(), HOVER_SECONDS, ease_out);
    let painter = ui.painter();
    let radius = CornerRadius::same(theme::RADIUS);
    if hover > 0.0 {
        painter.rect_filled(rect, radius, theme::SURFACE_HOVER.gamma_multiply(hover));
    }
    painter.rect_stroke(rect, radius, Stroke::new(BORDER, theme::LINE_CONTROL), StrokeKind::Inside);
    let at = Pos2::new(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0);
    painter.galley(at, Arc::clone(&galley), theme::TEXT_1);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `--ease-out`, `cubic-bezier(0.16, 1, 0.3, 1)`: the curve's y where its x is `t`.
fn ease_out(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let (x1, y1, x2, y2) = (0.16f64, 1.0f64, 0.3f64, 1.0f64);
    let t = f64::from(t.clamp(0.0, 1.0));
    let bezier = |s: f64, p1: f64, p2: f64| 3.0 * (1.0 - s) * (1.0 - s) * s * p1 + 3.0 * (1.0 - s) * s * s * p2 + s * s * s;
    // x(s) rises from 0 to 1: halve the interval until it holds t
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if bezier(mid, x1, x2) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(0.5 * (lo + hi), y1, y2) as f32
}

#[cfg(test)]
mod tests {
    use egui::epaint::{ClippedShape, Shape};
    use egui::{Event, PointerButton, RawInput};

    use super::*;

    const SCREEN: [f32; 2] = [1440.0, 900.0];

    fn error() -> MapError {
        MapError {
            message: "In Device::create_render_pipeline, label = 'path': Error matching the vertex shader".into(),
            details: "Validation Error\n\nCaused by:\n  In Device::create_render_pipeline, label = 'path'\n".into(),
            lost: false,
        }
    }

    fn adapter() -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: "SwiftShader Device (Subzero)".into(),
            vendor: 0x1ae0,
            device: 0xc0de,
            driver: "SwiftShader driver".into(),
            driver_info: "5.0.0".into(),
            ..wgpu::AdapterInfo::new(wgpu::DeviceType::Cpu, wgpu::Backend::Vulkan)
        }
    }

    /// A frame of `ctx` on a `screen`-sized window with `events`, at `time` seconds.
    fn frame(ctx: &egui::Context, screen: [f32; 2], events: Vec<Event>, time: f64, ui: impl FnMut(&egui::Context)) -> egui::FullOutput {
        let mut ui = ui;
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(screen[0], screen[1]))),
            events,
            time: Some(time),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |root| ui(root.ctx()));
        out.textures_delta.clear();
        out
    }

    /// The texts the frame drew, in order.
    fn texts(shapes: &[ClippedShape]) -> Vec<(String, Pos2)> {
        let mut out = Vec::new();
        for clipped in shapes {
            if let Shape::Text(text) = &clipped.shape {
                out.push((text.galley.job.text.clone(), text.pos));
            }
        }
        out
    }

    fn click(at: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(at),
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
            Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ]
    }

    /// Doc 01 §9.5: the title, the message in the data font, "Copy details" and "Retry", on a
    /// `--surface` card centred in the map, `min(420px, 100% − 32px)` wide plus its padding and border.
    #[test]
    fn the_card_says_what_went_wrong_with_two_buttons() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut card = None;
        // laid out on the first frame, shown from the second
        for time in [0.0, 1.0] {
            let out = frame(&ctx, SCREEN, Vec::new(), time, |ctx| card = Some(show(ctx, &error(), Some(&adapter()))));
            if time == 1.0 {
                let texts = texts(&out.shapes);
                let names: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
                assert_eq!(names, [TITLE, error().message.as_str(), COPY_DETAILS, RETRY]);
                // top to bottom, the buttons side by side
                assert!(texts[0].1.y < texts[1].1.y && texts[1].1.y < texts[2].1.y);
                assert_eq!(texts[2].1.y.round(), texts[3].1.y.round());
                assert!(texts[2].1.x < texts[3].1.x);
                // the message in the data font, the title in the UI font
                let fonts: Vec<(FontFamily, f32)> = out
                    .shapes
                    .iter()
                    .filter_map(|c| if let Shape::Text(t) = &c.shape { Some(&t.galley.job.sections[0].format.font_id) } else { None })
                    .map(|f| (f.family.clone(), f.size))
                    .collect();
                assert_eq!(fonts[0], (FontFamily::Proportional, 14.0));
                assert_eq!(fonts[1], (FontFamily::Monospace, 12.0));
                assert_eq!(fonts[2], (FontFamily::Proportional, 13.0));
                // the card's own fill and border
                let surface = out.shapes.iter().any(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == theme::SURFACE && r.stroke.color == theme::LINE));
                assert!(surface, "a --surface card with a --line border");
            }
        }
        let card = card.expect("shown");
        assert!(!card.copied && !card.retried);
        // 420 wide plus 2 × 16 padding and 2 × 1 border, centred
        assert!((card.rect.width() - 454.0).abs() < 0.5, "{:?}", card.rect);
        assert!((card.rect.center().x - 720.0).abs() < 0.5 && (card.rect.center().y - 450.0).abs() < 0.5, "{:?}", card.rect);
        // the title line, 8, one message line, 12, the buttons: 19.6 + 8 + 2 × 16.8 (two lines at
        // 420 px) + 12 + 32 + 2 × 17 around
        let content = card.rect.height() - 2.0 * (PADDING + BORDER);
        let lines = ((content - 19.6 - 8.0 - 12.0 - 32.0) / 16.8).round();
        assert!(lines >= 1.0, "{content}");
        assert!((content - (19.6 + 8.0 + lines * 16.8 + 12.0 + 32.0)).abs() < 0.5, "{content}");
        // `.button`: 32 high, text and 12 px either side, 8 apart, left-aligned under the message
        assert_eq!(card.copy_details.height(), 32.0);
        assert_eq!(card.retry.height(), 32.0);
        assert!((card.retry.min.x - card.copy_details.max.x - 8.0).abs() < 0.01);
        assert!((card.copy_details.min.x - (card.rect.min.x + 17.0)).abs() < 0.5);
        assert!((card.copy_details.max.y - (card.rect.max.y - 17.0)).abs() < 0.5);
    }

    #[test]
    fn the_card_is_at_most_420_wide_and_16_from_the_sides_of_a_narrow_map() {
        assert_eq!(content_width(1440.0), 420.0);
        assert_eq!(content_width(452.0), 420.0);
        assert_eq!(content_width(390.0), 358.0);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut card = None;
        for time in [0.0, 1.0] {
            frame(&ctx, [390.0, 844.0], Vec::new(), time, |ctx| card = Some(show(ctx, &error(), None)));
        }
        let card = card.expect("shown");
        // content-box: 358 + 2 × 17 = 392 for a 390-px phone, as today's CSS gives (a pixel over
        // either side)
        assert!((card.rect.width() - 392.0).abs() < 0.5, "{:?}", card.rect);
        assert!((card.rect.center().x - 195.0).abs() < 0.5);
    }

    /// "Copy details" puts `mapErrorDetails` on the clipboard; "Retry" asks the renderer.
    #[test]
    fn copy_details_copies_and_retry_is_reported() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut card = None;
        for time in [0.0, 1.0] {
            frame(&ctx, SCREEN, Vec::new(), time, |ctx| card = Some(show(ctx, &error(), Some(&adapter()))));
        }
        let at = card.expect("shown").copy_details.center();
        let mut clicked = None;
        let out = frame(&ctx, SCREEN, click(at), 2.0, |ctx| clicked = Some(show(ctx, &error(), Some(&adapter()))));
        let clicked = clicked.expect("shown");
        assert!(clicked.copied && !clicked.retried);
        let copied: Vec<&String> =
            out.platform_output.commands.iter().filter_map(|c| if let egui::OutputCommand::CopyText(t) = c { Some(t) } else { None }).collect();
        assert_eq!(copied, [&details(&error(), Some(&adapter()))]);

        let at = clicked.retry.center();
        let mut clicked = None;
        let out = frame(&ctx, SCREEN, click(at), 3.0, |ctx| clicked = Some(show(ctx, &error(), Some(&adapter()))));
        let clicked = clicked.expect("shown");
        assert!(clicked.retried && !clicked.copied);
        assert!(!out.platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_))));
    }

    /// `mapErrorDetails` (G:1260-1269): the message, the renderer, the system, a blank line, the
    /// details.
    #[test]
    fn the_details_name_the_error_the_adapter_and_the_system() {
        let text = details(&error(), Some(&adapter()));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "RustRadar map error: In Device::create_render_pipeline, label = 'path': Error matching the vertex shader");
        assert_eq!(lines[1], "Renderer: SwiftShader Device (Subzero) | SwiftShader driver | 5.0.0 | vulkan | Cpu");
        assert_eq!(lines[2], format!("System: RustRadar {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH));
        assert_eq!(lines[3], "");
        assert_eq!(lines[4..].join("\n"), error().details.trim_end());
        let none = details(&error(), None);
        assert_eq!(none.lines().nth(1), Some("Renderer: no graphics device"));
        let mut blank = adapter();
        blank.driver = String::new();
        blank.driver_info = " ".into();
        assert_eq!(details(&error(), Some(&blank)).lines().nth(1), Some("Renderer: SwiftShader Device (Subzero) | vulkan | Cpu"));
    }

    #[test]
    fn the_hover_eases_out_as_the_css_curve() {
        assert_eq!(ease_out(0.0), 0.0);
        assert!((ease_out(1.0) - 1.0).abs() < 1e-6);
        // cubic-bezier(0.16, 1, 0.3, 1), solved for x in f64: most of the way there early
        for (t, y) in [(0.1, 0.494_390_6), (0.2, 0.752_126_1), (0.5, 0.971_779_1)] {
            assert!((ease_out(t) - y).abs() < 1e-5, "{t}: {}", ease_out(t));
        }
    }

    /// A map that is stopped or not, with today's Retry: a lost device's error stays.
    #[derive(Default)]
    struct Map {
        error: Option<MapError>,
        retries: usize,
    }

    impl StoppedMap for Map {
        fn error(&self) -> Option<MapError> {
            self.error.clone()
        }

        fn retry(&mut self) -> Retry {
            self.retries += 1;
            if self.error.as_ref().is_some_and(|e| e.lost) {
                return Retry::Rebuild;
            }
            self.error = None;
            Retry::Redraw
        }
    }

    /// The card in the frame loop: nothing while the map draws; the stopped map's card, whose
    /// Retry clears the error, or asks for a new device after a loss.
    #[test]
    fn the_card_follows_the_map() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut map = Map::default();
        let mut answer = None;
        let out = frame(&ctx, SCREEN, Vec::new(), 0.0, |ctx| answer = ui(ctx, &mut map, None));
        assert_eq!(answer, None);
        assert!(texts(&out.shapes).is_empty(), "no card while the map draws");

        map.error = Some(error());
        let mut retry_at = None;
        for time in [1.0, 2.0] {
            let out = frame(&ctx, SCREEN, Vec::new(), time, |ctx| {
                answer = ui(ctx, &mut map, None);
                retry_at = Some(show(ctx, &error(), None).retry.center());
            });
            assert_eq!(answer, None);
            if time == 2.0 {
                assert_eq!(texts(&out.shapes)[0].0, TITLE);
            }
        }
        let at = retry_at.expect("laid out");
        frame(&ctx, SCREEN, click(at), 3.0, |ctx| answer = ui(ctx, &mut map, None));
        assert_eq!(answer, Some(Retry::Redraw));
        assert_eq!((map.error.clone(), map.retries), (None, 1), "Retry cleared it");
        let out = frame(&ctx, SCREEN, Vec::new(), 4.0, |ctx| answer = ui(ctx, &mut map, None));
        assert!(texts(&out.shapes).is_empty(), "the card is gone");

        let lost = MapError::device_lost(wgpu::DeviceLostReason::Unknown, "");
        map.error = Some(lost.clone());
        for time in [5.0, 6.0] {
            frame(&ctx, SCREEN, Vec::new(), time, |ctx| answer = ui(ctx, &mut map, None));
        }
        frame(&ctx, SCREEN, click(at), 7.0, |ctx| answer = ui(ctx, &mut map, None));
        assert_eq!(answer, Some(Retry::Rebuild), "a lost device needs a new one");
        assert_eq!(map.error, Some(lost));
    }
}
