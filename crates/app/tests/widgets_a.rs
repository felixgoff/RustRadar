//! Item U3's acceptance (plan stage 7; doc 06 §1.4, §2.8): the widget gallery drawn by egui
//! through `ui::widgets`, as a window frame draws the UI, on SwiftShader.
//!
//! - a button is 32 px high with 12 px of padding inside a `(110,110,110)` border; a pressed
//!   toggle has an `(55,142,240)` border on 18 % blue; badges, skeleton, spinner, live dot, alerts
//!   and the tooltip box in their colours (`target/shots/u3-widget-gallery.png`);
//! - today's sidebar and map controls rebuilt from the widgets at today's places
//!   (`target/shots/u3-sidebar-replica.png`) land where `docs/port/reference/dark-v1-1440x900.png`
//!   has them: the Filters button, the "plain browser" alert, the list's first skeletons and the
//!   map controls' icon buttons, in their colours and sizes.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::Path;

use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::{Align, Color32, Event, Layout, Pos2, RawInput, Sense, UiBuilder, Vec2};
use rustradar_app::ui::icons::{self, Icon};
use rustradar_app::ui::text;
use rustradar_app::ui::theme;
use rustradar_app::ui::widgets::alert::Alert;
use rustradar_app::ui::widgets::badge::{Badge, Tone};
use rustradar_app::ui::widgets::button::{self, Button};
use rustradar_app::ui::widgets::icon_button::IconButton;
use rustradar_app::ui::widgets::live_dot::LiveDot;
use rustradar_app::ui::widgets::skeleton::Skeleton;
use rustradar_app::ui::widgets::{spinner, tooltip};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};

/// The canvas, CSS px, at scale 1: the desktop reference pictures' size.
const SIZE: [u32; 2] = [1440, 900];

/// The frames' times: the last at 2.8 s, a whole number of skeleton cycles (1.4 s), so the
/// shimmer stands where the reference pictures' capture stopped it.
const TIMES: [f64; 3] = [0.0, 1.0, 2.8];

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it (U1's canvas).
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

    /// A frame at egui time `time` with the pointer at `pointer` and `draw` as the UI.
    fn frame(&mut self, gpu: &Gpu, time: f64, pointer: Option<Pos2>, draw: impl FnMut(&mut egui::Ui)) {
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            time: Some(time),
            events: pointer.map(Event::PointerMoved).into_iter().collect(),
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
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("widgets") });
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

/// Runs `add` in a child of `ui` whose top left corner is `at` (a left-to-right row).
fn at<R>(ui: &mut egui::Ui, at: Pos2, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let max = egui::Rect::from_min_size(at, Vec2::new(SIZE[0] as f32 - at.x, SIZE[1] as f32 - at.y));
    ui.scope_builder(UiBuilder::new().max_rect(max).layout(Layout::left_to_right(Align::Min)), |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(theme::SPACE_3, theme::SPACE_3);
        add(ui)
    })
    .inner
}

/// A `.surface` card from `min` to `max`.
fn card(ui: &egui::Ui, min: Pos2, max: Pos2) {
    let rect = egui::Rect::from_min_max(min, max);
    ui.painter().rect(rect, theme::RADIUS, theme::SURFACE, egui::Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
}

/// A screen rectangle as the pixels it covers.
fn px(rect: egui::Rect) -> Rect {
    let x0 = rect.min.x.floor().max(0.0) as u32;
    let y0 = rect.min.y.floor().max(0.0) as u32;
    Rect { x0, y0, x1: rect.max.x.ceil() as u32, y1: rect.max.y.ceil() as u32 }
}

fn rgb(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

/// `top` (premultiplied) over the opaque `under`, as egui blends it, each channel ±0.5.
fn over(top: Color32, under: Color32) -> [f64; 3] {
    let a = f64::from(top.a()) / 255.0;
    [0, 1, 2].map(|i| f64::from(top.to_array()[i]) + f64::from(under.to_array()[i]) * (1.0 - a))
}

#[track_caller]
fn assert_near(p: [u8; 4], want: [f64; 3], tolerance: f64, what: &str) {
    for c in 0..3 {
        assert!((f64::from(p[c]) - want[c]).abs() <= tolerance, "{what}: {p:?} for {want:?}");
    }
}

fn lum(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0
}

/// Where the gallery put its parts.
#[derive(Debug, Clone)]
struct Gallery {
    card: egui::Rect,
    plain: egui::Rect,
    filters: egui::Rect,
    pressed: egui::Rect,
    hovered: egui::Rect,
    primary: egui::Rect,
    disabled: egui::Rect,
    compact: egui::Rect,
    icon_buttons: Vec<egui::Rect>,
    badges: Vec<(Tone, egui::Rect)>,
    skeleton: egui::Rect,
    spinners: Vec<(f32, egui::Rect)>,
    live: egui::Rect,
    ended: egui::Rect,
    error: egui::Rect,
    warn: egui::Rect,
    tip: egui::Rect,
}

impl Default for Gallery {
    fn default() -> Self {
        let n = egui::Rect::NOTHING;
        Gallery {
            card: n,
            plain: n,
            filters: n,
            pressed: n,
            hovered: n,
            primary: n,
            disabled: n,
            compact: n,
            icon_buttons: Vec::new(),
            badges: Vec::new(),
            skeleton: n,
            spinners: Vec::new(),
            live: n,
            ended: n,
            error: n,
            warn: n,
            tip: n,
        }
    }
}

/// The gallery: every widget of U3 in its states on a `.surface` card.
fn gallery(ui: &mut egui::Ui, g: &mut Gallery) {
    g.card = egui::Rect::from_min_max(Pos2::new(16.0, 16.0), Pos2::new(1100.0, 600.0));
    card(ui, g.card.min, g.card.max);
    let x = 40.0;
    // each button from a whole pixel, as the sidebar's layout puts them
    let mut left = x;
    let mut next = |ui: &mut egui::Ui, b: Button| {
        let rect = at(ui, Pos2::new(left, 40.0), |ui| b.show(ui).rect);
        left = rect.max.x.ceil() + theme::SPACE_3;
        rect
    };
    g.plain = next(ui, Button::text_only("Show all trains"));
    g.filters = next(ui, Button::new().icon(button::icon(Icon::Funnel)).text("Filters"));
    g.pressed = next(ui, Button::new().icon(button::icon(Icon::Funnel)).text("Filters").count(3).pressed(true));
    g.hovered = next(ui, Button::new().icon(button::icon(Icon::RefreshCw)).text("Try again"));
    g.disabled = next(ui, Button::new().icon(button::icon(Icon::RefreshCw)).text("Retry now").enabled(false));
    g.compact = next(ui, Button::new().icon(button::icon(Icon::LocateFixed)).compact().pressed(true).label("Follow"));
    next(ui, Button::new().text("Flightradar24").icon(button::icon(Icon::ExternalLink)));
    g.primary = next(ui, Button::new().spinner(spinner::SIZE).text("Save and sign in").primary());
    next(ui, Button::text_only("Restart").primary_hover(theme::AMBER_HOVER).enabled(false).dim_when_disabled(0.7));
    at(ui, Pos2::new(x, 96.0), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for b in [
            IconButton::of(Icon::Layers, 18.0, "Map layers"),
            IconButton::of(Icon::Settings, 18.0, "Settings").pressed(true),
            IconButton::of(Icon::KeyRound, 18.0, "API keys").icon_color(theme::LIVE),
            IconButton::of(Icon::Keyboard, 18.0, "Keyboard shortcuts"),
            IconButton::of(Icon::Plus, 18.0, "Zoom in").enabled(false),
            IconButton::new(Icon::Navigation2.at(18.0, icons::STROKE_THIN).rotated(-30f32.to_radians()), "Reset to north up").icon_color(theme::AMBER),
            IconButton::of(Icon::X, 18.0, "Close"),
        ] {
            g.icon_buttons.push(b.show(ui).rect);
        }
    });
    at(ui, Pos2::new(x, 148.0), |ui| {
        for (text, tone) in [("Filed flight plan", Tone::Good), ("Old position", Tone::Warn), ("Cancelled", Tone::Bad), ("Estimated", Tone::Neutral)] {
            g.badges.push((tone, Badge::new(text, tone).show(ui).rect));
        }
        Badge::new("Updated 3 min ago", Tone::Warn).size(theme::TEXT_SM).show(ui);
    });
    at(ui, Pos2::new(x, 186.0), |ui| {
        g.skeleton = Skeleton::new(240.0, 12.0).show(ui).rect;
        Skeleton::new(90.0, 12.0).show(ui);
        Skeleton::new(120.0, 64.0).show(ui);
    });
    at(ui, Pos2::new(x, 270.0), |ui| {
        // the spinner at four turns of its cycle, drawn as it is drawn at each
        for phase in [0.0f32, 0.25, 0.5, 0.75] {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
            icons::paint(ui.painter(), rect, spinner::spec(spinner::SIZE, phase), theme::TEXT_2);
            g.spinners.push((phase, rect));
        }
        spinner::show(ui, spinner::SIZE, theme::TEXT_2);
        spinner::show(ui, spinner::SIZE_SMALL, theme::TEXT_3);
        ui.add_space(24.0);
        g.live = LiveDot::Live.show(ui).rect;
        text::label(ui, text::job("Live", &text::Style::ui(theme::TEXT_MD, 600, theme::LIVE)), Sense::hover());
        ui.add_space(24.0);
        g.ended = LiveDot::Ended.show(ui).rect;
        text::label(ui, text::job("Last seen 14:02 CEST", &text::Style::ui(theme::TEXT_MD, 400, theme::TEXT_2)), Sense::hover());
    });
    at(ui, Pos2::new(x, 320.0), |ui| {
        ui.set_width(286.0);
        g.error = Alert::error("Can't reach Flightradar24. Trying again in 12 s.")
            .detail("error sending request for url (https://data-cloud.flightradar24.com/zones/fcgi/feed.js)")
            .show_with(ui, |ui| {
                Button::new().icon(button::icon(Icon::RefreshCw)).text("Retry now").show(ui);
            })
            .rect;
    });
    at(ui, Pos2::new(x + 320.0, 320.0), |ui| {
        ui.set_width(286.0);
        g.warn = Alert::warn("Flightradar24 is limiting requests. Slowing down; next update in 21 s.").show(ui).rect;
    });
    at(ui, Pos2::new(x + 640.0, 320.0), |ui| {
        g.tip = tooltip::frame()
            .show(ui, |ui| text::label(ui, text::wrapped("Filters (F)", &tooltip::style(), tooltip::MAX_WIDTH), Sense::hover()))
            .response
            .rect;
    });
}

/// Where the replica put its parts.
#[derive(Debug, Clone)]
struct Replica {
    filters: egui::Rect,
    alert: egui::Rect,
    skeletons: Vec<egui::Rect>,
    controls: Vec<egui::Rect>,
}

impl Default for Replica {
    fn default() -> Self {
        Replica { filters: egui::Rect::NOTHING, alert: egui::Rect::NOTHING, skeletons: Vec::new(), controls: Vec::new() }
    }
}

/// Today's sidebar and map controls at today's places (doc 06 §2.3, §2.8, §14, §19.1).
fn replica(ui: &mut egui::Ui, r: &mut Replica) {
    // the primary card: 320 wide at 16, 16; its content 17 in
    card(ui, Pos2::new(16.0, 16.0), Pos2::new(336.0, 264.0));
    r.filters = at(ui, Pos2::new(33.0, 119.0), |ui| Button::new().icon(button::icon(Icon::Funnel)).text("Filters").tooltip("Filters (F)").show(ui).rect);
    r.alert = at(ui, Pos2::new(33.0, 163.0), |ui| {
        ui.set_width(286.0);
        Alert::error("No live flight data in a plain browser.").detail("RustRadar needs its Rust backend. Start it with `bun tauri dev`.").show(ui).rect
    });
    // the list card and the first two rows of TopFlights' skeletons (TopFlights:37-39)
    card(ui, Pos2::new(16.0, 276.0), Pos2::new(336.0, 571.0));
    for (i, y) in [(0u32, 326.0f32), (1, 366.0)] {
        let first = 58.0 + f32::from((i * 17 % 30) as u16);
        let wide = 120.0 + f32::from((i * 29 % 70) as u16);
        r.skeletons.push(at(ui, Pos2::new(33.0, y), |ui| Skeleton::new(first, 12.0).show(ui).rect));
        at(ui, Pos2::new(285.0, y), |ui| Skeleton::new(34.0, 12.0).show(ui));
        r.skeletons.push(at(ui, Pos2::new(33.0, y + 14.0), |ui| Skeleton::new(wide, 10.0).show(ui).rect));
    }
    // the map controls: right 16, bottom 28; stacks of 4, 2 and 2 with 2 px padding, 8 apart
    let stacks: [&[IconButton]; 3] = [
        &[
            IconButton::of(Icon::Layers, 18.0, "Map layers"),
            IconButton::of(Icon::Settings, 18.0, "Settings"),
            IconButton::of(Icon::KeyRound, 18.0, "API keys"),
            IconButton::of(Icon::Keyboard, 18.0, "Keyboard shortcuts"),
        ],
        &[IconButton::of(Icon::Plus, 18.0, "Zoom in"), IconButton::of(Icon::Minus, 18.0, "Zoom out")],
        &[IconButton::of(Icon::Rotate3d, 18.0, "Tilt to 3D"), IconButton::of(Icon::Navigation2, 18.0, "Reset to north up").enabled(false)],
    ];
    let mut bottom = SIZE[1] as f32 - 28.0;
    let mut placed = Vec::new();
    for stack in stacks.iter().rev() {
        let height = stack.len() as f32 * 32.0 + 6.0;
        let top = bottom - height;
        let left = SIZE[0] as f32 - 16.0 - 38.0;
        card(ui, Pos2::new(left, top), Pos2::new(left + 38.0, bottom));
        for (i, b) in stack.iter().enumerate() {
            placed.push((top + 3.0 + 32.0 * i as f32, left + 3.0, b.clone()));
        }
        bottom = top - 8.0;
    }
    placed.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (y, x, b) in placed {
        r.controls.push(at(ui, Pos2::new(x, y), |ui| b.show(ui).rect));
    }
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

/// The bounds of the pixels in `rect` that differ from `under` by more than `by` (mean of the
/// channels).
fn ink_bounds(image: &Image, rect: Rect, under: [u8; 4], by: f64) -> Option<Rect> {
    bounds(image, rect, |p| (lum(p) - lum(under)).abs() > by)
}

/// The ink-weighted centre of the pixels in `rect` brighter than `under`.
fn ink_centre(image: &Image, rect: Rect, under: [u8; 4]) -> [f64; 2] {
    let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
    for (x, y) in rect.pixels() {
        let w = (lum(pixel(image, x, y)) - lum(under)).max(0.0);
        sx += w * (f64::from(x) + 0.5);
        sy += w * (f64::from(y) + 0.5);
        sw += w;
    }
    [sx / sw, sy / sw]
}

#[test]
fn the_widget_gallery() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let mut g = Gallery::default();
    let mut pointer = None;
    for time in TIMES {
        g = Gallery::default();
        canvas.frame(&gpu, time, pointer, |ui| gallery(ui, &mut g));
        pointer = Some(g.hovered.center());
    }
    let image = canvas.read(&gpu);
    save(&image, "u3-widget-gallery.png");
    eprintln!("{g:?}");
    let surface = rgb(theme::SURFACE);

    // a button: 32 px high, its 1 px border (110,110,110), 12 px of padding inside it
    let plain = px(g.plain);
    assert_eq!(plain.height(), 32, "{plain:?}");
    let [cx, cy] = plain.centre().map(|v| v as u32);
    for (x, y, edge) in [(plain.x0, cy, "left"), (plain.x1 - 1, cy, "right"), (cx, plain.y0, "top"), (cx, plain.y1 - 1, "bottom")] {
        assert_eq!(pixel(&image, x, y), [110, 110, 110, 255], "the {edge} border");
    }
    assert_eq!(pixel(&image, plain.x0 + 1, cy), surface, "transparent inside the border");
    assert_eq!(pixel(&image, cx, plain.y0 + 1), surface);
    // inside the border, clear of its rounded corners
    let inner = Rect { x0: plain.x0 + 1, y0: plain.y0 + 5, x1: plain.x1 - 1, y1: plain.y1 - 5 };
    let ink = ink_bounds(&image, inner, surface, 20.0).expect("the label's ink");
    let (left, right) = (ink.x0 - inner.x0, inner.x1 - ink.x1);
    eprintln!("Show all trains: ink {ink:?}, {left} px from the left border, {right} from the right");
    assert!((12..=14).contains(&left) && (12..=14).contains(&right), "12 px of padding: {left} and {right}");
    // the label's line box centred on the middle: its capitals' ink 9 px under the top and 12
    // over the bottom (room for descenders), as today's "Filters" (y 128–139 in 119–151)
    let (above, below) = (ink.y0 - plain.y0, plain.y1 - ink.y1);
    assert_eq!((above, below), (9, 12), "the label's ink in the button");

    // the icon after the padding, the text 8 px after the icon
    let filters = px(g.filters);
    // (laid out after a fractional width, its edges fall between pixels: look past them)
    let icon_ink = ink_bounds(&image, Rect { x0: filters.x0 + 3, y0: filters.y0 + 5, x1: filters.x0 + 13 + 14, y1: filters.y1 - 5 }, surface, 20.0).expect("the funnel");
    assert!((filters.x0 + 13..=filters.x0 + 15).contains(&icon_ink.x0), "the icon after the padding: {icon_ink:?} in {filters:?}");
    let text_ink = ink_bounds(&image, Rect { x0: icon_ink.x1 + 1, y0: filters.y0 + 5, x1: filters.x1 - 3, y1: filters.y1 - 5 }, surface, 20.0).expect("the text");
    eprintln!("Filters: icon ink {icon_ink:?}, text ink {text_ink:?} in {filters:?}");
    // the text's slot starts 13 + 14 + 8 px in (its first glyph's ink a pixel or two later)
    let slot = g.filters.min.x + 35.0;
    assert!((slot..=slot + 2.5).contains(&(text_ink.x0 as f32)), "8 px after the icon: {text_ink:?} in {filters:?}");

    // the pressed toggle: an --amber border on 18 % --amber over --surface, the label in --amber
    let pressed = px(g.pressed);
    let [px_, py] = pressed.centre().map(|v| v as u32);
    assert_eq!(pixel(&image, pressed.x0, py), [55, 142, 240, 255], "pressed border");
    assert_eq!(pixel(&image, px_, pressed.y0), [55, 142, 240, 255], "pressed border");
    let soft = over(theme::AMBER_SOFT, theme::SURFACE);
    assert_near(pixel(&image, pressed.x0 + 3, py), soft, 0.5, "pressed fill");
    assert_eq!(pixel(&image, pressed.x0 + 3, py), [51, 67, 84, 255], "(55,142,240) at 18 % on (50,50,50)");
    // its count pill: --amber with the number in white
    let pill = Rect { x0: pressed.x1 - 13 - 18, y0: pressed.y0 + 7, x1: pressed.x1 - 13, y1: pressed.y1 - 7 };
    assert_eq!(pixel(&image, pill.x0 + 2, py), rgb(theme::AMBER), "pill");
    assert!(ink_bounds(&image, pill, rgb(theme::AMBER), 40.0).is_some(), "the pill's number");

    // hovered (the pointer rests on it): --surface-hover inside
    let hovered = px(g.hovered);
    assert_eq!(pixel(&image, hovered.x0 + 3, hovered.centre()[1] as u32), rgb(theme::SURFACE_HOVER), "hover fill");
    // compact: 32 × 32 with the icon centred
    let compact = px(g.compact);
    assert_eq!((compact.width(), compact.height()), (32, 32));
    let c = ink_centre(&image, Rect { x0: compact.x0 + 2, y0: compact.y0 + 2, x1: compact.x1 - 2, y1: compact.y1 - 2 }, rgb(Color32::from_rgb(51, 67, 84)));
    assert!((c[0] - compact.centre()[0]).abs() < 1.0 && (c[1] - compact.centre()[1]).abs() < 1.0, "centred icon {c:?} in {compact:?}");
    // primary: --amber to its edge, white label
    let primary = px(g.primary);
    assert_eq!(pixel(&image, primary.x0, primary.centre()[1] as u32), rgb(theme::AMBER), "primary to its edge");
    assert_eq!(pixel(&image, primary.x0 + 3, primary.centre()[1] as u32), rgb(theme::AMBER));

    // icon buttons: 32 square, transparent; pressed --amber-soft; disabled dimmed
    let ib = &g.icon_buttons;
    assert!(ib.iter().all(|r| r.size() == Vec2::splat(32.0)));
    assert_eq!(pixel(&image, px(ib[0]).x0 + 2, px(ib[0]).y0 + 2), surface);
    assert_eq!(pixel(&image, px(ib[1]).x0 + 2, px(ib[1]).y0 + 2), [51, 67, 84, 255], "pressed icon button");
    let brightest = |r: egui::Rect| px(r).pixels().map(|(x, y)| lum(pixel(&image, x, y))).fold(0.0, f64::max);
    let (normal, disabled) = (brightest(ib[0]), brightest(ib[4]));
    // --text-2 at full strength against --text-3 at half
    assert!(normal > 150.0 && disabled < 50.0 + (168.0 - 50.0) * 0.5 + 6.0, "{normal} {disabled}");
    let green = px(ib[2]).pixels().map(|(x, y)| pixel(&image, x, y)).filter(|p| p[1] > p[0] + 40).count();
    assert!(green > 20, "the signed-in key in --live: {green}");

    // badges: their tones' fills over --surface, 18.5 px high
    for (tone, rect) in &g.badges {
        let (fill, _) = tone.colors();
        let r = px(*rect);
        assert!((rect.height() - 18.5).abs() < 1e-3, "{tone:?} {rect:?}");
        assert_near(pixel(&image, r.x0 + 2, r.centre()[1] as u32), over(fill, theme::SURFACE), 1.0, &format!("{tone:?} badge"));
        assert!(ink_bounds(&image, Rect { x0: r.x0 + 4, y0: r.y0 + 1, x1: r.x1 - 4, y1: r.y1 - 1 }, pixel(&image, r.x0 + 2, r.centre()[1] as u32), 30.0).is_some(), "{tone:?} text");
    }

    // the skeleton at rest: --surface-raised on the left rising to --surface-hover on the right
    let sk = px(g.skeleton);
    let y = sk.centre()[1] as u32;
    let (l, m, r) = (pixel(&image, sk.x0 + 4, y), pixel(&image, (sk.x0 + sk.x1) / 2, y), pixel(&image, sk.x1 - 5, y));
    eprintln!("skeleton {l:?} {m:?} {r:?}");
    assert!((f64::from(l[0]) - 62.4).abs() <= 1.5 && (f64::from(m[0]) - 68.0).abs() <= 1.5 && (f64::from(r[0]) - 73.6).abs() <= 1.5, "{l:?} {m:?} {r:?}");

    // the spinner turns clockwise: its ink's centre a quarter turn on at each step
    let angles: Vec<f64> = g
        .spinners
        .iter()
        .map(|(_, rect)| {
            let r = px(*rect);
            let c = ink_centre(&image, r, surface);
            let [ox, oy] = r.centre();
            (c[1] - oy).atan2(c[0] - ox).to_degrees()
        })
        .collect();
    eprintln!("spinner ink angles {angles:?}");
    for pair in angles.windows(2) {
        let turn = (pair[1] - pair[0]).rem_euclid(360.0);
        assert!((turn - 90.0).abs() < 6.0, "a quarter turn clockwise: {turn}");
    }

    // the live dot: --live inside, its pulse spilling past the 8 px; the ended dot hollow
    let live = px(g.live);
    let [lx, ly] = live.centre().map(|v| v as u32);
    assert_eq!(pixel(&image, lx, ly), rgb(theme::LIVE));
    let ring = pixel(&image, live.x1 + 1, ly);
    assert!(ring[1] > ring[0] + 10 && lum(ring) > lum(surface) + 5.0, "the pulse ring past the dot: {ring:?}");
    let ended = px(g.ended);
    let [ex, ey] = ended.centre().map(|v| v as u32);
    assert_eq!(pixel(&image, ex, ey), surface, "hollow");
    assert!(lum(pixel(&image, ended.x0, ey)) > lum(surface) + 30.0, "its ring");

    // the alerts: their fills, the icon in the tone's colour
    let error = px(g.error);
    assert_near(pixel(&image, error.x0 + 2, error.y0 + 2), over(theme::DANGER_SOFT, theme::SURFACE), 1.0, "error fill");
    let warn = px(g.warn);
    assert_near(pixel(&image, warn.x0 + 2, warn.y0 + 2), over(theme::WARN_SOFT, theme::SURFACE), 1.0, "warn fill");
    let red = Rect { x0: error.x0 + 12, y0: error.y0 + 12, x1: error.x0 + 29, y1: error.y0 + 30 }.pixels().filter(|&(x, y)| {
        let p = pixel(&image, x, y);
        p[0] > p[1] + 60
    });
    assert!(red.count() > 15, "the danger triangle");
    assert!((g.warn.height() - (24.0 + 2.0 * 18.0)).abs() < 0.01, "two rows of warning: {:?}", g.warn);

    // the tooltip box: --surface-raised in a --line border
    let tip = px(g.tip);
    assert_eq!(pixel(&image, tip.x0 + 2, tip.y0 + 2), rgb(theme::SURFACE_RAISED));
    assert_eq!(pixel(&image, tip.x0, tip.centre()[1] as u32), rgb(theme::LINE));
}

#[test]
fn todays_sidebar_and_map_controls() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let mut r = Replica::default();
    for time in TIMES {
        r = Replica::default();
        canvas.frame(&gpu, time, None, |ui| replica(ui, &mut r));
    }
    let image = canvas.read(&gpu);
    save(&image, "u3-sidebar-replica.png");
    eprintln!("{r:?}");
    let today = reference("dark-v1-1440x900.png");
    let surface = rgb(theme::SURFACE);

    // the Filters button: today's border box is x 33–119, y 119–151 (its edges 0.05 px off the
    // pixel grid there, so its border reads 107 against our 110)
    let ours = px(r.filters);
    eprintln!("Filters at {ours:?}");
    assert_eq!((ours.x0, ours.y0, ours.y1), (33, 119, 151));
    assert!(ours.x1.abs_diff(119) <= 1, "as wide as today's: {ours:?}");
    let border = |img: &Image, x: u32| lum(pixel(img, x, 135));
    assert!(border(&today, 33) > 100.0 && border(&image, 33) > 100.0, "left border column");
    assert!(border(&today, 118) > 100.0 && border(&image, ours.x1 - 1) > 100.0, "right border column");
    let inside = Rect { x0: 35, y0: 121, x1: 117, y1: 149 };
    let (a, b) = (ink_bounds(&image, inside, surface, 25.0).expect("ours"), ink_bounds(&today, inside, surface, 25.0).expect("today's"));
    eprintln!("Filters' icon and text: ours {a:?}, today's {b:?}");
    for (x, y, what) in [(a.x0, b.x0, "left"), (a.x1, b.x1, "right"), (a.y0, b.y0, "top"), (a.y1, b.y1, "bottom")] {
        assert!(x.abs_diff(y) <= 1, "the content's {what} edge: {x} against today's {y}");
    }

    // the alert: x 33–319, y 163–247 on --danger-soft over --surface
    let alert = px(r.alert);
    eprintln!("alert at {alert:?}");
    assert_eq!((alert.x0, alert.x1, alert.y0, alert.y1), (33, 319, 163, 247));
    assert_eq!(pixel(&image, 200, 240), pixel(&today, 200, 240), "the alert's fill");
    let red = |img: &Image| bounds(img, Rect { x0: 36, y0: 166, x1: 66, y1: 200 }, |p| p[0] > p[1] + 40).expect("the triangle");
    let (a, b) = (red(&image), red(&today));
    eprintln!("triangle: ours {a:?}, today's {b:?}");
    assert!(a.x0.abs_diff(b.x0) <= 1 && a.x1.abs_diff(b.x1) <= 1 && a.y0.abs_diff(b.y0) <= 1 && a.y1.abs_diff(b.y1) <= 1);
    // its three rows of text where today's are
    let rows = |img: &Image| -> Vec<(u32, u32)> {
        let lit: Vec<bool> = (165..246).map(|y| (66..318).any(|x| lum(pixel(img, x, y)) > 120.0)).collect();
        let mut out = Vec::new();
        let mut start = None;
        for (i, &on) in lit.iter().enumerate() {
            match (on, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    out.push((165 + s as u32, 165 + i as u32));
                    start = None;
                }
                _ => {}
            }
        }
        out
    };
    let (a, b) = (rows(&image), rows(&today));
    eprintln!("alert text rows: ours {a:?}, today's {b:?}");
    let main_rows = |v: &[(u32, u32)]| v.iter().filter(|(y0, y1)| y1 - y0 >= 5).copied().collect::<Vec<_>>();
    let (a, b) = (main_rows(&a), main_rows(&b));
    assert_eq!(a.len(), 3, "three rows: {a:?}");
    assert_eq!(a.len(), b.len());
    for ((a0, _), (b0, _)) in a.iter().zip(&b) {
        assert!(a0.abs_diff(*b0) <= 1, "a row at {a0}, today's at {b0}");
    }

    // TopFlights' first skeletons: where today's are, the same shimmer at rest
    for rect in &r.skeletons[..2] {
        let s = px(*rect);
        for x in [s.x0 + 2, (s.x0 + s.x1) / 2, s.x1 - 3] {
            let y = s.centre()[1] as u32;
            let (ours, theirs) = (pixel(&image, x, y), pixel(&today, x, y));
            assert!(ours[0].abs_diff(theirs[0]) <= 2, "skeleton {s:?} at {x}: {ours:?} against today's {theirs:?}");
        }
        assert_eq!(pixel(&today, s.x0 - 1, s.centre()[1] as u32), surface, "today's skeleton starts at {}", s.x0);
    }

    // the map controls: each icon where today's is, with as much ink (the compass disabled: --text-3
    // at half strength); U2 found today's raster 1–15 % lighter on these icons (its MSAA edges)
    let ink = |img: &Image, rect: Rect| rect.pixels().map(|(x, y)| (lum(pixel(img, x, y)) - lum(surface)).max(0.0)).sum::<f64>();
    for rect in &r.controls {
        let box_ = px(rect.shrink(3.0));
        let (a, b) = (ink_centre(&image, box_, surface), ink_centre(&today, box_, surface));
        let (ia, ib) = (ink(&image, box_), ink(&today, box_));
        eprintln!("control {box_:?}: ink centre {a:?} today's {b:?}, ink {ia:.0} today's {ib:.0}");
        assert!((a[0] - b[0]).abs() < 0.75 && (a[1] - b[1]).abs() < 0.75, "{box_:?}: {a:?} against {b:?}");
        assert!((ia / ib - 1.0).abs() < 0.16, "{box_:?}: ink {ia:.0} against today's {ib:.0}");
    }
    // the disabled compass has about a quarter of the enabled tilt button's ink, as today
    let (tilt, compass) = (px(r.controls[6].shrink(3.0)), px(r.controls[7].shrink(3.0)));
    let (ours, theirs) = (ink(&image, compass) / ink(&image, tilt), ink(&today, compass) / ink(&today, tilt));
    eprintln!("compass / tilt ink: ours {ours:.3}, today's {theirs:.3}");
    assert!((ours - theirs).abs() < 0.05, "{ours} against {theirs}");
}
