//! Item U2's acceptance (plan stage 7; doc 06 §26): the icon sheet drawn by egui through
//! `ui::icons`, as a window frame draws the UI, on SwiftShader.
//!
//! - every icon of doc 06 §26 has ink at 14, 16, 18 and 20 px, at scale 1 and 2;
//! - stroke 2.5 covers more ink than 1.75 (and 2 lies between), icon by icon and size by size;
//! - the icons today's sidebar, mode bar and map controls show in
//!   `docs/port/reference/dark-v1-1440x900.png` come out where they are, in their colours, with
//!   their ink within a few percent of today's and their pixels close to today's.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes. Shots go to `target/shots/u2-*.png`.

mod common;

use std::path::Path;

use common::shots::{Rect, area, gpu, save};
use egui::{Color32, CornerRadius, Pos2, RawInput, Vec2};
use rustradar_app::ui::icons::{self, Icon, STROKE_BOLD, STROKE_REGULAR, STROKE_THIN, Spec};
use rustradar_app::ui::theme;
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};

/// The canvas, points: the desktop reference pictures' size.
const SIZE: [u32; 2] = [1440, 900];

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
    scale: f32,
}

impl Canvas {
    fn new(gpu: &Gpu, scale: f32) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        let size = SIZE.map(|n| (n as f32 * scale) as u32);
        Canvas { ctx, egui, target: Offscreen::new(gpu, size), scale }
    }

    /// A frame at egui time `time` with `draw` as the UI.
    fn frame(&mut self, gpu: &Gpu, time: f64, draw: impl FnMut(&mut egui::Ui)) {
        let mut raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(SIZE[0] as f32, SIZE[1] as f32))),
            time: Some(time),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(self.scale);
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
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("icon sheet") });
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

    /// `rect` (points) as the pixels it covers.
    fn px(&self, rect: egui::Rect) -> Rect {
        let s = self.scale;
        Rect { x0: (rect.min.x * s).floor() as u32, y0: (rect.min.y * s).floor() as u32, x1: (rect.max.x * s).ceil() as u32, y1: (rect.max.y * s).ceil() as u32 }
    }
}

fn rgba(c: Color32) -> [u8; 4] {
    [c.r(), c.g(), c.b(), 255]
}

/// The sizes the acceptance asks for, and the strokes in use.
const SIZES: [f32; 4] = [14.0, 16.0, 18.0, 20.0];
const STROKES: [f32; 3] = [STROKE_THIN, STROKE_REGULAR, STROKE_BOLD];
/// The sheet's cell pitch, points.
const PITCH: f32 = 34.0;
/// The sheet's corner, points.
const ORIGIN: Pos2 = Pos2::new(40.0, 40.0);

/// The sheet: a `.surface` card, a column per icon, a row per (stroke, size), each icon centred in
/// its cell in `--text-2`; under it the rotated uses (the 45° progress plane, the compass at 30°,
/// the spinner at 120.5°). Returns each cell's rect by (icon, stroke, size).
fn sheet(ui: &mut egui::Ui) -> Vec<(Icon, f32, f32, egui::Rect)> {
    let painter = ui.painter();
    let rows = (STROKES.len() * SIZES.len()) as f32;
    let card = egui::Rect::from_min_size(ORIGIN - Vec2::splat(16.0), Vec2::new(Icon::ALL.len() as f32 * PITCH + 32.0, (rows + 1.0) * PITCH + 32.0));
    painter.rect(card, CornerRadius::same(theme::RADIUS), theme::SURFACE, egui::Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
    let mut cells = Vec::new();
    for (row, (stroke, size)) in STROKES.iter().flat_map(|&s| SIZES.iter().map(move |&z| (s, z))).enumerate() {
        for (column, icon) in Icon::ALL.into_iter().enumerate() {
            let cell = egui::Rect::from_min_size(ORIGIN + Vec2::new(column as f32 * PITCH, row as f32 * PITCH), Vec2::splat(PITCH));
            icons::paint(painter, cell, icon.at(size, stroke), theme::TEXT_2);
            cells.push((icon, stroke, size, cell));
        }
    }
    let y = ORIGIN.y + rows * PITCH;
    for (i, (spec, color)) in [
        (Icon::Plane.at(14.0, STROKE_REGULAR).rotated(45f32.to_radians()), theme::AMBER),
        (Icon::Navigation2.at(18.0, STROKE_THIN).rotated(-30f32.to_radians()), theme::AMBER),
        (Icon::LoaderCircle.at(14.0, STROKE_REGULAR).rotated(120.5f32.to_radians()), theme::TEXT_2),
        (Icon::ChevronRight.at(14.0, STROKE_REGULAR).rotated(90f32.to_radians()), theme::TEXT_2),
        (Icon::Check.at(12.0, STROKE_BOLD), theme::AMBER),
        (Icon::ArrowRight.at(11.0, STROKE_REGULAR), theme::TEXT_3),
        (Icon::Eye.at(13.0, STROKE_THIN), theme::TEXT_3),
        (Icon::Plane.at(15.0, STROKE_THIN), theme::AMBER_INK),
    ]
    .into_iter()
    .enumerate()
    {
        let cell = egui::Rect::from_min_size(Pos2::new(ORIGIN.x + i as f32 * PITCH, y), Vec2::splat(PITCH));
        if color == theme::AMBER_INK {
            painter.rect_filled(cell.shrink(4.0), CornerRadius::same(theme::RADIUS), theme::AMBER);
        }
        icons::paint(painter, cell, spec, color);
    }
    cells
}

/// Ink (pixels of full `--text-2` over `--surface`) in each cell.
fn inks(canvas: &Canvas, image: &Image, cells: &[(Icon, f32, f32, egui::Rect)]) -> Vec<(Icon, f32, f32, f64)> {
    cells.iter().map(|&(icon, stroke, size, cell)| (icon, stroke, size, area(image, canvas.px(cell), rgba(theme::SURFACE), rgba(theme::TEXT_2)))).collect()
}

fn check_sheet(canvas: &Canvas, image: &Image, cells: &[(Icon, f32, f32, egui::Rect)]) {
    let inks = inks(canvas, image, cells);
    let ink = |icon: Icon, stroke: f32, size: f32| inks.iter().find(|i| (i.0, i.1, i.2) == (icon, stroke, size)).expect("a cell").3;
    let scale = f64::from(canvas.scale);
    for icon in Icon::ALL {
        for size in SIZES {
            let (thin, regular, bold) = (ink(icon, STROKE_THIN, size), ink(icon, STROKE_REGULAR, size), ink(icon, STROKE_BOLD, size));
            // the least inky icon (`minus`: one 14-unit line) holds 14 × 1.75 units² of ink
            let least = 14.0 * 1.75 * (f64::from(size) * scale / 24.0).powi(2) * 0.8;
            assert!(thin > least, "×{scale}: {icon:?} at {size} px has {thin:.1} px of ink");
            assert!(thin < regular && regular < bold, "×{scale}: {icon:?} at {size} px: ink {thin:.1}, {regular:.1}, {bold:.1} at 1.75, 2, 2.5");
            // ink grows about as the stroke (2.5 / 1.75 = 1.43) where lines dominate
            assert!(bold / thin > 1.2 && bold / thin < 1.6, "×{scale}: {icon:?} at {size} px: 2.5 against 1.75 is {:.2}", bold / thin);
        }
    }
    // a cell holds nothing but its icon: the corners of each cell stay --surface
    for &(icon, stroke, size, cell) in cells {
        let r = canvas.px(cell);
        for (x, y) in [(r.x0, r.y0), (r.x1 - 1, r.y0), (r.x0, r.y1 - 1), (r.x1 - 1, r.y1 - 1)] {
            assert_eq!(image.pixel(x, y), rgba(theme::SURFACE), "×{scale}: {icon:?} {size}/{stroke}: corner ({x}, {y})");
        }
    }
}

#[test]
fn the_icon_sheet() {
    let Some(gpu) = gpu() else { return };
    for scale in [1.0, 2.0] {
        let mut canvas = Canvas::new(&gpu, scale);
        let mut cells = Vec::new();
        canvas.frame(&gpu, 0.0, |ui| cells = sheet(ui));
        let image = canvas.read(&gpu);
        save(&image, &format!("u2-icon-sheet-x{scale}.png"));
        check_sheet(&canvas, &image, &cells);
        // every (icon, size, stroke) is one texture, the rotated uses four more (the chevron's
        // 90° and the arrow, eye, check and plane uses are at sizes of their own)
        assert_eq!(icons::cached(&canvas.ctx), Icon::ALL.len() * SIZES.len() * STROKES.len() + 8);
        // drawn again: nothing new is rasterised
        canvas.frame(&gpu, 1.0, |ui| cells = sheet(ui));
        assert_eq!(icons::cached(&canvas.ctx), Icon::ALL.len() * SIZES.len() * STROKES.len() + 8);
        assert_eq!(canvas.read(&gpu).rgba, image.rgba, "×{scale}: the same picture from the cache");
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

/// An icon of today's desktop shell in `dark-v1-1440x900.png`: what it is, the colour it is drawn
/// in, the colour under it, the centre of its square (points) and how far its ink may stray from
/// today's.
struct Use {
    what: &'static str,
    spec: Spec,
    color: Color32,
    under: Color32,
    centre: Pos2,
    /// The share by which the ink may differ from today's.
    ink: f64,
}

/// The ink tolerance of most uses. The reference was drawn by Chromium's GPU rasteriser on
/// SwiftShader, with 4× MSAA for these paths: its edge pixels are covered 0, ¼, ½, ¾ or 1 (an
/// 18 px `minus` comes out 1.25 px thick, the geometry's 1.31 here), so ink differs by a few
/// percent either way.
const INK: f64 = 0.08;
/// Lucide draws dots as zero-length segments (`M10 8h.01`) whose round caps are discs one stroke
/// wide (1.35 px² at 18/1.75); Chromium's MSAA raster covers 0.5 px² of each. `Keyboard` has six:
/// 13 % more ink here.
const INK_DOTS: f64 = 0.15;

/// The centre of button `button` of map-control stack `stack` (0 at the top), from the layout of
/// doc 06 §2.3 and §19.1: right 16, bottom 28; stacks of 4, 2 and 2 buttons 32 px square, each
/// stack with a 1 px border and 2 px padding, 8 apart.
fn control(stack: usize, button: usize) -> Pos2 {
    let buttons = [4.0, 2.0, 2.0];
    let height = |n: f32| n * 32.0 + 2.0 * (1.0 + 2.0);
    let below: f32 = buttons[stack + 1..].iter().map(|&n| height(n) + 8.0).sum();
    let top = SIZE[1] as f32 - 28.0 - below - height(buttons[stack]);
    Pos2::new(SIZE[0] as f32 - 16.0 - 3.0 - 16.0, top + 3.0 + button as f32 * 32.0 + 16.0)
}

/// The icons of today's desktop shell at V1 (none of them rotated). The sidebar's and the mode
/// bar's centres are measured on the picture; the map controls' come from the layout.
fn uses() -> Vec<Use> {
    let alert = Color32::from_rgb(76, 56, 57);
    let disabled = theme::TEXT_3.gamma_multiply(0.5);
    let button = |what, icon: Icon, stack, i, ink| Use { what, spec: icon.at(18.0, STROKE_THIN), color: theme::TEXT_2, under: theme::SURFACE, centre: control(stack, i), ink };
    let ui = |what, spec, color, under, centre: [f32; 2]| Use { what, spec, color, under, centre: Pos2::from(centre), ink: INK };
    vec![
        ui("wordmark Radar 20/1.75", Icon::Radar.at(20.0, STROKE_THIN), theme::AMBER, theme::SURFACE, [43.0, 45.0]),
        ui("search field Search 16/1.75", Icon::Search.at(16.0, STROKE_THIN), theme::TEXT_3, theme::INK, [54.0, 88.0]),
        ui("Filters Funnel 14/1.75", Icon::Funnel.at(14.0, STROKE_THIN), theme::TEXT_1, theme::SURFACE, [53.0, 135.0]),
        ui("alert TriangleAlert 16/1.75", Icon::TriangleAlert.at(16.0, STROKE_THIN), theme::DANGER, alert, [53.0, 184.0]),
        ui("Watching Eye 13/1.75", Icon::Eye.at(13.0, STROKE_THIN), theme::TEXT_3, theme::SURFACE, [258.5, 296.5]),
        ui("mode bar Plane 15/1.75", Icon::Plane.at(15.0, STROKE_THIN), theme::AMBER_INK, theme::AMBER, [653.5, 35.5]),
        ui("mode bar Ship 15/1.75", Icon::Ship.at(15.0, STROKE_THIN), theme::TEXT_2, theme::SURFACE, [748.5, 35.0]),
        button("Layers 18/1.75", Icon::Layers, 0, 0, INK),
        button("Settings 18/1.75", Icon::Settings, 0, 1, INK),
        button("KeyRound 18/1.75", Icon::KeyRound, 0, 2, INK),
        button("Keyboard 18/1.75", Icon::Keyboard, 0, 3, INK_DOTS),
        button("Plus 18/1.75", Icon::Plus, 1, 0, INK),
        button("Minus 18/1.75", Icon::Minus, 1, 1, INK),
        button("Rotate3d 18/1.75", Icon::Rotate3d, 2, 0, INK),
        Use { what: "compass Navigation2 18/1.75, disabled", spec: Icon::Navigation2.at(18.0, STROKE_THIN), color: disabled, under: theme::SURFACE, centre: control(2, 1), ink: INK },
    ]
}

/// The ink of `rect` in `image` shifted by `d`: each pixel's distance from `under` toward
/// `color`, summed.
fn ink_at(image: &Image, rect: Rect, d: [i32; 2], under: Color32, color: Color32) -> f64 {
    let shifted = Rect { x0: rect.x0.saturating_add_signed(d[0]), y0: rect.y0.saturating_add_signed(d[1]), x1: rect.x1.saturating_add_signed(d[0]), y1: rect.y1.saturating_add_signed(d[1]) };
    // a disabled colour is premultiplied: its opaque look over `under`
    let seen = |c: Color32| {
        let a = f32::from(c.a()) / 255.0;
        let mix = |fg: u8, bg: u8| (f32::from(fg) + f32::from(bg) * (1.0 - a)).round() as u8;
        [mix(c.r(), under.r()), mix(c.g(), under.g()), mix(c.b(), under.b()), 255]
    };
    area(image, shifted, rgba(under), seen(color))
}

/// The sum of the channels' differences between `a`'s `rect` and `b`'s `rect` shifted by `d`.
fn sad(a: &Image, b: &Image, rect: Rect, d: [i32; 2]) -> u64 {
    rect.pixels()
        .map(|(x, y)| {
            let (p, q) = (a.pixel(x, y), b.pixel(x.saturating_add_signed(d[0]), y.saturating_add_signed(d[1])));
            (0..3).map(|c| u64::from(p[c].abs_diff(q[c]))).sum::<u64>()
        })
        .sum()
}

/// Side by side, 8 times larger: for each use, ours left and today's right.
fn side_by_side(ours: &Image, theirs: &Image, windows: &[(Rect, [i32; 2])]) -> Image {
    const K: u32 = 8;
    let (w, h) = windows.iter().fold((0, 0), |(w, h), (r, _)| (w.max(r.width()), h.max(r.height())));
    let (cell_w, cell_h) = (w * K * 2 + 3 * K, h * K + 2 * K);
    let columns = 3;
    let rows = (windows.len() as u32).div_ceil(columns);
    let (width, height) = (cell_w * columns, cell_h * rows);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for (i, &(r, d)) in windows.iter().enumerate() {
        let (cx, cy) = ((i as u32 % columns) * cell_w + K, (i as u32 / columns) * cell_h + K);
        for (side, (image, shift)) in [(ours, [0, 0]), (theirs, d)].into_iter().enumerate() {
            for (x, y) in r.pixels() {
                let p = image.pixel(x.saturating_add_signed(shift[0]), y.saturating_add_signed(shift[1]));
                let (ox, oy) = (cx + side as u32 * (w * K + K) + (x - r.x0) * K, cy + (y - r.y0) * K);
                for (dx, dy) in (0..K).flat_map(|dy| (0..K).map(move |dx| (dx, dy))) {
                    let at = (((oy + dy) * width + ox + dx) * 4) as usize;
                    rgba[at..at + 4].copy_from_slice(&p);
                }
            }
        }
    }
    Image { width, height, rgba }
}

#[test]
fn todays_shell_icons_look_as_today() {
    let Some(gpu) = gpu() else { return };
    let theirs = reference("dark-v1-1440x900.png");
    let uses = uses();
    let mut canvas = Canvas::new(&gpu, 1.0);
    let mut squares = Vec::new();
    canvas.frame(&gpu, 0.0, |ui| {
        squares.clear();
        for u in &uses {
            let tile = egui::Rect::from_center_size(u.centre, Vec2::splat(30.0));
            ui.painter().rect_filled(tile, CornerRadius::ZERO, u.under);
            icons::paint(ui.painter(), tile, u.spec, u.color);
            squares.push(icons::square(tile, u.spec, 1.0));
        }
    });
    let ours = canvas.read(&gpu);
    save(&ours, "u2-shell-icons.png");

    let mut windows = Vec::new();
    for (u, square) in uses.iter().zip(&squares) {
        // the icon's square and 3 px round it
        let window = canvas.px(square.expand(3.0));
        // today's icon is where the shell puts it, give or take the rounding of its layout
        let d = (-3..=3).flat_map(|dy| (-3..=3).map(move |dx| [dx, dy])).min_by_key(|&d| sad(&ours, &theirs, window, d)).expect("a shift");
        let (ink, today) = (ink_at(&ours, window, [0, 0], u.under, u.color), ink_at(&theirs, window, d, u.under, u.color));
        let mean = sad(&ours, &theirs, window, d) as f64 / (window.len() * 3) as f64;
        eprintln!("{}: today's {:?} px off; ink {ink:.2}, today {today:.2} ({:+.1} %); mean difference {mean:.2} levels", u.what, d, (ink / today - 1.0) * 100.0);
        assert_eq!(d, [0, 0], "{}: today's icon is {d:?} px from where ours is", u.what);
        assert!((ink / today - 1.0).abs() < u.ink, "{}: ink {ink:.2}, today's {today:.2}", u.what);
        // over the window (mostly background) a few levels of MSAA against exact coverage, more
        // at the dots
        assert!(mean < 5.0 + 20.0 * (u.ink - INK), "{}: pixels differ by {mean:.2} levels on average", u.what);
        windows.push((window, d));
    }
    save(&side_by_side(&ours, &theirs, &windows), "u2-shell-icons-vs-reference.png");
}
