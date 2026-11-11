//! Item U1's acceptance (plan stage 7; doc 06 §1): a type specimen drawn by egui with the app's
//! theme, as a window frame draws the UI, on SwiftShader.
//!
//! - every weight of Overpass (400 … 750) and Overpass Mono (400 … 700) draws visibly thicker
//!   strokes than the one before (ink coverage rises with weight);
//! - `"A · B"` has a gap on both sides of the dot;
//! - the `.surface` panel is filled `(50,50,50)` with a `(68,68,68)` border;
//! - the sidebar's texts of today's app (the wordmark, the Filters button, the "plain browser"
//!   alert) come out as wide and as inky as in `docs/port/reference/dark-v1-1440x900.png`.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. The shot goes to `target/shots/u1-type-specimen.png`.

mod common;

use std::path::Path;

use common::shots::{Rect, bounds, components, gpu, pixel, save};
use egui::{Color32, CornerRadius, Pos2, RawInput, Sense, Vec2};
use rustradar_app::ui::fonts::{Face, MONO_WEIGHTS, UI_WEIGHTS};
use rustradar_app::ui::text::{self, Style};
use rustradar_app::ui::theme;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};

/// The canvas, CSS px, at scale 1: the desktop reference pictures' size.
const SIZE: [u32; 2] = [1440, 900];

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
}

impl Canvas {
    fn new(gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx, egui, target: Offscreen::new(gpu, SIZE) }
    }

    /// A frame at egui time `time` with `draw` as the UI.
    fn frame(&mut self, gpu: &Gpu, time: f64, draw: impl FnMut(&mut egui::Ui)) {
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            time: Some(time),
            ..Default::default()
        };
        let output = self.ctx.run_ui(raw, draw);
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("specimen") });
        let mut commands = self.egui.update_buffers(device, queue, &mut encoder, &primitives, &screen);
        {
            let [r, g, b, _] = theme::INK.to_array().map(|c| f64::from(c) / 255.0);
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: self.target.view(),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui.render(&mut pass, &primitives, &screen);
        }
        commands.push(encoder.finish());
        queue.submit(commands);
        for id in textures.free.drain() {
            self.egui.free_texture(&id);
        }
    }

    fn read(&self, gpu: &Gpu) -> Image {
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        self.target.read(gpu).expect("read back")
    }
}

/// Where the specimen put its parts (screen points).
#[derive(Debug, Clone)]
struct Parts {
    panel: egui::Rect,
    ui_weights: Vec<(u16, egui::Rect)>,
    mono_weights: Vec<(u16, egui::Rect)>,
    dot: egui::Rect,
    wordmark: egui::Rect,
    filters: egui::Rect,
    alert_title: egui::Rect,
    alert_detail: egui::Rect,
    truncated: egui::Rect,
}

impl Default for Parts {
    fn default() -> Self {
        let nothing = egui::Rect::NOTHING;
        Parts {
            panel: nothing,
            ui_weights: Vec::new(),
            mono_weights: Vec::new(),
            dot: nothing,
            wordmark: nothing,
            filters: nothing,
            alert_title: nothing,
            alert_detail: nothing,
            truncated: nothing,
        }
    }
}

/// The sample of each weight: the same letters, so ink compares weight with weight.
const UI_SAMPLE: &str = "Hamburgefonstiv RustRadar 0123";
const MONO_SAMPLE: &str = "FL350 12,345 ft 0123";

/// The specimen: a `.surface` card with every weight of both faces, the middle dot, the sizes,
/// the sidebar's texts of today's app, a truncated row and the colour tokens.
fn specimen(ui: &mut egui::Ui, parts: &mut Parts) {
    let card = egui::Area::new(egui::Id::unique("specimen")).fixed_pos(Pos2::new(16.0, 16.0)).fade_in(false).show(ui.ctx(), |ui| {
        theme::surface().inner_margin(theme::SPACE_4).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(theme::SPACE_3, 2.0);
            ui.set_width(1100.0 - 32.0 - 34.0);
            ui.set_max_height(SIZE[1] as f32 - 64.0);
            let caption = Style::data(theme::TEXT_SM, 400, theme::TEXT_3);
            let row = |ui: &mut egui::Ui, name: &str, job: egui::text::LayoutJob| -> egui::Rect {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(90.0, 30.0), Sense::hover());
                    ui.painter().galley(rect.left_center() - Vec2::new(0.0, 8.0), ui.painter().layout_job(text::job(name, &caption)), Color32::PLACEHOLDER);
                    text::label(ui, job, Sense::hover()).rect
                })
                .inner
            };
            for &w in &UI_WEIGHTS {
                let rect = row(ui, &format!("ui-{w}"), text::job(UI_SAMPLE, &Style::ui(22.0, w, theme::TEXT_1)));
                parts.ui_weights.push((w, rect));
            }
            for &w in &MONO_WEIGHTS {
                let rect = row(ui, &format!("mono-{w}"), text::job(MONO_SAMPLE, &Style::data(22.0, w, theme::TEXT_1)));
                parts.mono_weights.push((w, rect));
            }
            ui.add_space(theme::SPACE_2);
            ui.horizontal(|ui| {
                parts.dot = text::label(ui, text::job("A · B", &Style::ui(40.0, 400, theme::TEXT_1)), Sense::hover()).rect;
                ui.add_space(theme::SPACE_6);
                ui.vertical(|ui| {
                    text::label(ui, text::job("KLM1234 · AMS → LHR · 12,000 ft", &Style::ui(theme::TEXT_MD, 400, theme::TEXT_2)), Sense::hover());
                    let mut mixed = text::job("FL350 · ", &Style::data(theme::TEXT_MD, 400, theme::TEXT_1));
                    text::append(&mut mixed, "climbing · 1,200 fpm", &Style::ui(theme::TEXT_MD, 650, theme::LIVE));
                    text::label(ui, mixed, Sense::hover());
                    parts.truncated =
                        text::label(ui, text::truncated("Scandinavian Airlines System · Stockholm Arlanda · Terminal 5", &Style::body(), 220.0), Sense::hover()).rect;
                });
                ui.add_space(theme::SPACE_6);
                ui.vertical(|ui| {
                    for (name, size, color) in [
                        ("--text-xs 11", theme::TEXT_XS, theme::TEXT_3),
                        ("--text-sm 12", theme::TEXT_SM, theme::TEXT_2),
                        ("--text-md 13", theme::TEXT_MD, theme::TEXT_1),
                        ("--text-base 14", theme::TEXT_BASE, theme::TEXT_1),
                        ("--text-lg 17", theme::TEXT_LG, theme::TEXT_1),
                        ("--text-xl 22", theme::TEXT_XL, theme::TEXT_1),
                    ] {
                        text::label(ui, text::job(name, &Style::ui(size, 400, color)), Sense::hover());
                    }
                });
            });
            ui.add_space(theme::SPACE_3);
            // the sidebar's texts in today's styles (+page:1804-1811, app.css:178-189, +page:1863-1892)
            ui.horizontal(|ui| {
                parts.wordmark = text::label(ui, text::job("RustRadar", &Style::ui(theme::TEXT_LG, theme::WEIGHT_WORDMARK, theme::TEXT_1).letter_spacing(theme::LETTER_SPACING_TIGHT)), Sense::hover()).rect;
                ui.add_space(theme::SPACE_6);
                parts.filters = text::label(ui, text::job("Filters", &Style::ui(theme::TEXT_MD, theme::WEIGHT_BUTTON, theme::TEXT_1)), Sense::hover()).rect;
                ui.add_space(theme::SPACE_6);
                egui::Frame::new().fill(theme::DANGER_SOFT).corner_radius(CornerRadius::same(theme::RADIUS_SM)).inner_margin(theme::SPACE_3).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, theme::SPACE_2);
                    ui.vertical(|ui| {
                        parts.alert_title = text::label(ui, text::job("No live flight data in a plain browser.", &Style::ui(theme::TEXT_MD, 400, theme::TEXT_1)), Sense::hover()).rect;
                        parts.alert_detail =
                            text::label(ui, text::job("RustRadar needs its Rust backend. Start it", &Style::ui(theme::TEXT_SM, 400, theme::TEXT_2)), Sense::hover()).rect;
                    });
                });
            });
            ui.add_space(theme::SPACE_3);
            // the colour tokens
            ui.horizontal(|ui| {
                for c in [
                    theme::INK,
                    theme::SURFACE,
                    theme::SURFACE_RAISED,
                    theme::SURFACE_HOVER,
                    theme::LINE,
                    theme::LINE_CONTROL,
                    theme::TEXT_3,
                    theme::TEXT_2,
                    theme::TEXT_1,
                    theme::AMBER,
                    theme::AMBER_SOFT,
                    theme::LIVE,
                    theme::LIVE_SOFT,
                    theme::DANGER,
                    theme::DANGER_SOFT,
                    theme::WARN,
                    theme::WARN_SOFT,
                    theme::SERIES_1,
                    theme::LOGO_TILE,
                    theme::LIVE_PULSE,
                ] {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(36.0, 24.0), Sense::hover());
                    ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS), c);
                }
            });
        });
    });
    parts.panel = card.response.rect;
}

/// How much ink (0–1 per pixel, from `--surface` to `--text-1`) `rect` holds.
fn ink(image: &Image, rect: egui::Rect) -> f64 {
    let lum = |c: Color32| (f64::from(c.r()) + f64::from(c.g()) + f64::from(c.b())) / 3.0;
    ink_between(image, px(rect), lum(theme::SURFACE), lum(theme::TEXT_1))
}

/// How much ink `rect` holds: each pixel's brightness from `bg` (0) to `fg` (1), clamped.
fn ink_between(image: &Image, rect: Rect, bg: f64, fg: f64) -> f64 {
    rect.pixels()
        .map(|(x, y)| {
            let p = pixel(image, x, y);
            let l = (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0;
            ((l - bg) / (fg - bg)).clamp(0.0, 1.0)
        })
        .sum()
}

/// A screen rectangle as the pixels it covers.
fn px(rect: egui::Rect) -> Rect {
    let x0 = rect.min.x.floor().max(0.0) as u32;
    let y0 = rect.min.y.floor().max(0.0) as u32;
    Rect { x0, y0, x1: rect.max.x.ceil() as u32, y1: rect.max.y.ceil() as u32 }
}

/// The ink's horizontal extent in `rect`: pixels brighter than `threshold` (mean of the channels).
fn ink_width(image: &Image, rect: Rect, threshold: f64) -> Option<(u32, u32)> {
    let b = bounds(image, rect, |p| (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0 > threshold)?;
    Some((b.x0, b.x1))
}

/// A reference picture (8-bit RGB, or a palette) as RGBA.
fn reference(name: &str) -> Image {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/port/reference").join(name);
    let file = std::io::BufReader::new(std::fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().expect("a PNG");
    let mut buf = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut buf).expect("a frame");
    let channels = info.color_type.samples();
    let rgba = buf[..info.buffer_size()].chunks_exact(channels).flat_map(|p| [p[0], p[1], p[2], if channels == 4 { p[3] } else { 255 }]).collect();
    Image { width: info.width, height: info.height, rgba }
}

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

#[test]
fn the_type_specimen() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let mut parts = Parts::default();
    // an area is laid out on its first frame and shown from its second
    for time in [0.0, 1.0] {
        parts = Parts::default();
        canvas.frame(&gpu, time, |ui| specimen(ui, &mut parts));
    }
    let image = canvas.read(&gpu);
    save(&image, "u1-type-specimen.png");
    eprintln!("{parts:?}");

    // the panel: --surface inside, a 1 px --line border, 4 px corners
    let panel = px(parts.panel);
    assert!(panel.width() > 1000 && panel.height() > 400, "{panel:?}");
    let [cx, _] = panel.centre();
    let cx = cx as u32;
    assert_eq!(pixel(&image, panel.x0 + 8, panel.y0 + 8), [50, 50, 50, 255], "panel fill");
    assert_eq!(pixel(&image, cx, panel.y0), [68, 68, 68, 255], "top border");
    assert_eq!(pixel(&image, cx, panel.y1 - 1), [68, 68, 68, 255], "bottom border");
    assert_eq!(pixel(&image, panel.x0, panel.y0 + 100), [68, 68, 68, 255], "left border");
    assert_eq!(pixel(&image, panel.x1 - 1, panel.y0 + 100), [68, 68, 68, 255], "right border");
    assert_eq!(pixel(&image, cx, panel.y0 + 1), [50, 50, 50, 255], "the border is 1 px");
    assert_eq!(pixel(&image, cx, panel.y0 - 1), rgb(theme::INK), "--ink outside");
    let corner = pixel(&image, panel.x0, panel.y0);
    assert!(corner[0] < 68, "a rounded corner: {corner:?}");

    // ink rises with weight, for both faces
    for (face, weights) in [(Face::Ui, &parts.ui_weights), (Face::Mono, &parts.mono_weights)] {
        let inks: Vec<(u16, f64)> = weights.iter().map(|&(w, r)| (w, ink(&image, r))).collect();
        eprintln!("{face:?} ink by weight: {inks:?}");
        for pair in inks.windows(2) {
            let ((w0, i0), (w1, i1)) = (pair[0], pair[1]);
            // 50 units of weight add at least 2 % ink
            let step = f64::from(w1 - w0) / 50.0;
            assert!(i1 > i0 * (1.0 + 0.02 * step), "{face:?}: {w1} ({i1:.0}) is not inkier than {w0} ({i0:.0})");
        }
        let (first, last) = (inks[0].1, inks[inks.len() - 1].1);
        assert!(last > first * 1.3, "{face:?}: the heaviest has 30 % more ink than 400: {first:.0} → {last:.0}");
    }

    // "A · B": three blobs of ink, a clear gap either side of the dot
    let dot_rect = px(parts.dot);
    let mut blobs = components(&image, dot_rect, |p| p[0] > 120);
    blobs.sort_by_key(|c| c.bounds.x0);
    assert_eq!(blobs.len(), 3, "A, the dot and B: {blobs:?}");
    let (a, dot, b) = (blobs[0].bounds, blobs[1].bounds, blobs[2].bounds);
    let (left, right) = (i64::from(dot.x0) - i64::from(a.x1), i64::from(b.x0) - i64::from(dot.x1));
    eprintln!("A {a:?} dot {dot:?} B {b:?}: gaps {left} and {right} px");
    assert!(left >= 6 && right >= 6, "a gap on both sides of the dot at 40 px: {left} and {right}");
    assert!(dot.width() >= 3 && dot.width() <= 8 && dot.height() <= 8, "a dot: {dot:?}");
    // in the middle of the x-height: between B's top and its baseline
    let dot_y = dot.centre()[1];
    assert!(dot_y > f64::from(b.y0) + 0.3 * f64::from(b.height()) && dot_y < f64::from(b.y1) - 0.3 * f64::from(b.height()), "dot at {dot_y} on B {b:?}");

    // a truncated row ends within its 220 px
    let truncated = px(parts.truncated);
    assert!(truncated.width() <= 221, "{truncated:?}");

    // today's sidebar: the same texts as wide as in the reference picture (ink extents, ±2 px)
    let reference = reference("dark-v1-1440x900.png");
    let wide = |img: &Image, r: Rect, threshold: f64| ink_width(img, r, threshold).map(|(x0, x1)| x1 - x0).expect("ink");
    // …and as inky: the ink of each text (0–1 per pixel between its background and its colour)
    // within 5 % of today's (the glyph gamma of theme::apply)
    let (alert, surface) = (63.0, 50.0);
    let lum = |c: Color32| (f64::from(c.r()) + f64::from(c.g()) + f64::from(c.b())) / 3.0;
    for (name, ours, theirs, threshold, bg, fg) in [
        ("the wordmark", px(parts.wordmark), Rect { x0: 58, y0: 28, x1: 170, y1: 55 }, 150.0, surface, theme::TEXT_1),
        ("Filters", px(parts.filters), Rect { x0: 65, y0: 125, x1: 110, y1: 145 }, 150.0, surface, theme::TEXT_1),
        ("the alert", px(parts.alert_title), Rect { x0: 66, y0: 172, x1: 300, y1: 194 }, 150.0, alert, theme::TEXT_1),
        ("the alert's detail", px(parts.alert_detail), Rect { x0: 66, y0: 198, x1: 310, y1: 214 }, 120.0, alert, theme::TEXT_2),
    ] {
        let (ours_w, theirs_w) = (wide(&image, ours, threshold), wide(&reference, theirs, threshold));
        let (ours_ink, theirs_ink) = (ink_between(&image, ours, bg, lum(fg)), ink_between(&reference, theirs, bg, lum(fg)));
        eprintln!("{name}: {ours_w} px wide and {ours_ink:.0} of ink, today {theirs_w} px and {theirs_ink:.0}");
        assert!(ours_w.abs_diff(theirs_w) <= 2, "{name}: {ours_w} px wide, today's {theirs_w} px");
        assert!((ours_ink / theirs_ink - 1.0).abs() < 0.05, "{name}: ink {ours_ink:.0}, today's {theirs_ink:.0}");
    }
}
