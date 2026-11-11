//! Item U17's acceptance (plan stage 7; doc 06 §20–21): the keys dialog and the shortcuts' help as
//! the window's UI pass draws them, on SwiftShader, beside Chromium's layout of today's components
//! (server-rendered by Svelte, opened with `showModal()`, laid out with app.css; the numbers in
//! the tests are Chromium's).
//!
//! - `the_help`: `target/shots/u17-help.png` (1440 × 900) and `u17-help-phone.png` (390 × 844):
//!   the backdrop, the card centred 362 × 868 (as tall as the window allows: the list scrolls),
//!   the header, the rules every 36 px, the caps and the descriptions; no web inspector's row.
//! - `the_keys_dialog`: `u17-keys.png` (signed out), `u17-keys-signed.png` (signed in, the fields
//!   filled), `u17-keys-changed.png` (`Save and sign in`, the status, the box ticked),
//!   `u17-keys-error.png`, `u17-keys-busy.png` and `u17-keys-phone.png`: the card, the legends'
//!   rules, the fields, the badge, the buttons and the checkbox where Chromium puts them.
//! - `the_dialogs_through_the_window`: through the window's engine (`u17-help-window.png`): `?`
//!   opens the help, whose backdrop takes the pointer over the map, the map's keys off; the keys
//!   dialog takes the page's keys while a field has the keyboard; Escape closes either.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::{Path, PathBuf};

use common::shots::{gpu, pixel, save};
use egui::{Event, Key, Pos2, RawInput, Vec2};
use rustradar_app::app::Engine;
use rustradar_app::input::{KeyName, MapEvent};
use rustradar_app::state::aircraft::Answer;
use rustradar_app::state::shortcuts::Open;
use rustradar_app::state::{Action, AppState, Startup};
use rustradar_app::ui::keys_dialog::{self, KeysFeeds, Picked, Status};
use rustradar_app::ui::{shortcuts, theme};
use rustradar_core::geo::Basemap;
use rustradar_core::model::{Keys, SessionInfo};
use rustradar_map::Gpu;
use rustradar_map::io;
use rustradar_map::headless::{FORMAT, Image, Offscreen};
use rustradar_scene::camera::ViewState;

/// The desktop canvas, CSS px, at scale 1, and the phone's.
const DESKTOP: [u32; 2] = [1440, 900];
const PHONE: [u32; 2] = [390, 844];

/// egui alone onto a target cleared to `--ink`, as the window's UI pass draws it.
struct Canvas {
    ctx: egui::Context,
    egui: egui_wgpu::Renderer,
    target: Offscreen,
    size: [u32; 2],
}

impl Canvas {
    fn new(gpu: &Gpu, size: [u32; 2]) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_wgpu::Renderer::new(&gpu.device, FORMAT, egui_wgpu::RendererOptions::default());
        Canvas { ctx, egui, target: Offscreen::new(gpu, size), size }
    }

    /// A frame of both dialogs for `state` at egui time `time`: the actions they asked for.
    fn frame(&mut self, gpu: &Gpu, time: f64, state: &AppState) -> Vec<Action> {
        let raw = RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(self.size[0] as f32, self.size[1] as f32))),
            time: Some(time),
            ..Default::default()
        };
        let mut actions = Vec::new();
        let output = self.ctx.run_ui(raw, |ui| {
            shortcuts::ui(ui.ctx(), state, &mut actions);
            keys_dialog::ui(ui.ctx(), state, &mut actions);
        });
        let (device, queue) = (&gpu.device, &gpu.queue);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: self.target.size(), pixels_per_point: output.pixels_per_point };
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.egui.update_texture(device, queue, id, delta);
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("dialogs") });
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
        actions
    }

    /// Frames at egui times 0–3 s (the card is laid out unseen on its first, enters over 180 ms),
    /// then the picture.
    fn settle(&mut self, gpu: &Gpu, state: &AppState) -> Image {
        for time in [0.0, 1.0, 2.0, 3.0] {
            self.frame(gpu, time, state);
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        self.target.read(gpu).expect("read back")
    }
}

fn state() -> AppState {
    AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0])
}

fn rgb(image: &Image, x: u32, y: u32) -> [u8; 3] {
    let [r, g, b, _] = pixel(image, x, y);
    [r, g, b]
}

#[track_caller]
fn near(image: &Image, x: u32, y: u32, expected: [u8; 3], tolerance: u8) {
    let got = rgb(image, x, y);
    assert!(got.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= tolerance), "({x}, {y}): {got:?}, Chromium {expected:?}");
}

/// The rows where the colour at column `x` changes between `y0` and `y1` (what `edges.py` printed
/// for Chromium's picture).
fn edges(image: &Image, x: u32, y0: u32, y1: u32) -> Vec<(u32, [u8; 3])> {
    let mut out = Vec::new();
    let mut last = None;
    for y in y0..y1 {
        let p = rgb(image, x, y);
        if last != Some(p) {
            out.push((y, p));
            last = Some(p);
        }
    }
    out
}

/// The rows of `colour` (within 2) at column `x` between `y0` and `y1`.
fn rows_of(image: &Image, x: u32, y0: u32, y1: u32, colour: [u8; 3]) -> Vec<u32> {
    (y0..y1).filter(|&y| rgb(image, x, y).iter().zip(colour).all(|(a, b)| a.abs_diff(b) <= 2)).collect()
}

const BACKDROP_OVER_INK: [u8; 3] = [17, 17, 17];
const SURFACE: [u8; 3] = [50, 50, 50];
const LINE: [u8; 3] = [68, 68, 68];
const LINE_CONTROL: [u8; 3] = [110, 110, 110];
const INK: [u8; 3] = [30, 30, 30];

#[test]
fn the_help() {
    let Some(gpu) = gpu() else { return };
    let mut s = state();
    s.set_open(Open::Help, true);
    let mut canvas = Canvas::new(&gpu, DESKTOP);
    let image = canvas.settle(&gpu, &s);
    save(&image, "u17-help.png");

    // the backdrop over the map's ink, the card 362 wide at 539, 16 to 884
    near(&image, 100, 450, BACKDROP_OVER_INK, 1);
    near(&image, 700, 15, BACKDROP_OVER_INK, 1);
    near(&image, 700, 16, LINE, 1);
    near(&image, 700, 17, SURFACE, 1);
    near(&image, 538, 450, BACKDROP_OVER_INK, 1);
    near(&image, 539, 450, LINE, 1);
    near(&image, 900, 450, LINE, 1);
    near(&image, 901, 450, BACKDROP_OVER_INK, 1);
    near(&image, 700, 883, LINE, 1);
    near(&image, 700, 884, BACKDROP_OVER_INK, 1);
    // the rules: the first at 69 (12 + 32 + 8 under the card's content), then every 36 px but
    // where a description wraps (C: 47.38, K: 47.38)
    let rules = rows_of(&image, 560, 60, 883, LINE);
    let chromium = [69, 105, 141, 177, 213, 249, 285, 321, 357, 404, 440, 476, 512, 548, 584, 620, 656, 692, 728, 776, 812, 848];
    assert_eq!(rules.len(), chromium.len(), "{rules:?}");
    for (got, want) in rules.iter().zip(chromium) {
        assert!(got.abs_diff(want) <= 1, "rule at {got}, Chromium {want}: {rules:?}");
    }
    // the first cap: 556..590 × 75..100, its 2 px bottom border
    let cap = edges(&image, 572, 70, 104);
    assert!(cap.iter().any(|&(y, c)| y == 75 && c == LINE_CONTROL), "{cap:?}");
    assert_eq!(rgb(&image, 572, 98), LINE_CONTROL);
    assert_eq!(rgb(&image, 572, 99), LINE_CONTROL);
    assert_eq!(rgb(&image, 572, 100), SURFACE);
    let left: Vec<u32> = (550..600).filter(|&x| rgb(&image, x, 90) == LINE_CONTROL).collect();
    assert_eq!(left, [556, 589], "the cap's sides");
    // the descriptions start at 640 (556 + 72 + 12)
    let ink = (630..660).find(|&x| (78..96).any(|y| rgb(&image, x, y)[0] > 120)).expect("the first description's ink");
    assert!((640..=642).contains(&ink), "the description starts at {ink}");

    let mut phone = Canvas::new(&gpu, PHONE);
    let image = phone.settle(&gpu, &s);
    save(&image, "u17-help-phone.png");
    // 360 wide at 15, 16 to 828
    near(&image, 15, 400, LINE, 1);
    near(&image, 374, 400, LINE, 1);
    near(&image, 14, 400, BACKDROP_OVER_INK, 1);
    near(&image, 200, 16, LINE, 1);
    near(&image, 200, 827, LINE, 1);
    near(&image, 200, 828, BACKDROP_OVER_INK, 1);
}

/// Keys from memory, answered at once.
#[derive(Default)]
struct Stored(Keys);

impl KeysFeeds for Stored {
    fn load(&mut self) -> Answer<Keys> {
        Answer::ready(self.0.clone())
    }

    fn save(&mut self, keys: Keys) -> Answer<Result<Keys, String>> {
        Answer::ready(Ok(keys))
    }

    fn sign_in(&mut self, _email: String, _password: String) -> Answer<Result<SessionInfo, String>> {
        Answer::ready(Ok(SessionInfo { authenticated: true, expires: None }))
    }

    fn sign_out(&mut self) -> Answer<Result<SessionInfo, String>> {
        Answer::ready(Ok(SessionInfo::default()))
    }

    fn export(&mut self, file_name: &str, _contents: String) -> Answer<Result<PathBuf, String>> {
        Answer::ready(Ok(PathBuf::from("/tmp").join(file_name)))
    }

    fn reveal(&mut self, _path: &Path) {}

    fn pick(&mut self) -> Option<Answer<Picked>> {
        None
    }
}

fn keys_canvas(gpu: &Gpu, size: [u32; 2], stored: Keys) -> Canvas {
    let canvas = Canvas::new(gpu, size);
    keys_dialog::install_feeds(&canvas.ctx, Box::new(Stored(stored)));
    canvas
}

#[test]
fn the_keys_dialog() {
    let Some(gpu) = gpu() else { return };
    let mut s = state();
    s.set_open(Open::Keys, true);

    // signed out, nothing saved
    let mut canvas = keys_canvas(&gpu, DESKTOP, Keys::default());
    let image = canvas.settle(&gpu, &s);
    save(&image, "u17-keys.png");
    near(&image, 100, 100, BACKDROP_OVER_INK, 1);
    // Chromium's column at x 800: the card's top 174, the Flightradar24 rule 240, the email field
    // 324–359, the password 388–423, the Trafikverket rule 445, the API key 529–564, Save's top
    // 603 (it reaches x 570 only, so the column shows the spacer), the card's bottom 725
    let column = edges(&image, 800, 170, 730);
    let at = |y: u32| rgb(&image, 800, y);
    for (y, colour) in [(174, LINE), (240, LINE), (324, LINE_CONTROL), (325, INK), (359, LINE_CONTROL), (388, LINE_CONTROL), (423, LINE_CONTROL), (445, LINE), (529, LINE_CONTROL), (564, LINE_CONTROL), (725, LINE)] {
        assert!(at(y).iter().zip(colour).all(|(a, b)| a.abs_diff(b) <= 2), "row {y} at x 800: {:?}, Chromium {colour:?}; column {column:?}", at(y));
    }
    near(&image, 800, 726, BACKDROP_OVER_INK, 1);
    near(&image, 800, 173, BACKDROP_OVER_INK, 1);
    // the card's sides at 499 and 940
    near(&image, 499, 450, LINE, 1);
    near(&image, 940, 450, LINE, 1);
    near(&image, 498, 450, BACKDROP_OVER_INK, 1);
    // the rule starts 8 px after the legend: 607 (Chromium 606.98)
    let rule: Vec<u32> = (590..620).filter(|&x| rgb(&image, x, 240) == LINE).collect();
    assert!(rule.first().is_some_and(|&x| (606..=608).contains(&x)), "{rule:?}");
    // Save disabled: --amber at half over --surface (52, 96, 145) at 520, 610
    near(&image, 520, 610, [52, 96, 145], 2);
    // Import… and Export: --line-control borders at 771/849 and 858/923 (770.5–849.72, 857.72–924)
    for x in [771, 849, 858, 923] {
        near(&image, x, 620, LINE_CONTROL, 2);
    }
    // the checkbox: 520..533 × 650..663, border (133, 133, 133) round (59, 59, 59)
    near(&image, 520, 656, [133, 133, 133], 2);
    near(&image, 526, 656, [59, 59, 59], 2);
    near(&image, 532, 656, [133, 133, 133], 2);
    near(&image, 526, 650, [133, 133, 133], 2);
    near(&image, 526, 662, [133, 133, 133], 2);
    // the link's underline at 495, in --amber
    // (egui sets the text up to 2 px lower than Chromium on a paragraph's second row)
    let underline: Vec<u32> = (480..500).filter(|&y| rgb(&image, 600, y) == [55, 142, 240]).collect();
    assert!(underline.len() == 1 && underline[0].abs_diff(495) <= 2, "the underline's rows {underline:?}");

    // signed in, filled: the badge after the legend, Sign out, the card 592 high
    s.aircraft.set_session(SessionInfo { authenticated: true, expires: None });
    let mut canvas = keys_canvas(&gpu, DESKTOP, Keys { fr24_email: "pilot@example.com".into(), fr24_password: "hunter22".into(), trafikverket: "abcdef0123456789".into(), ..Default::default() });
    let image = canvas.settle(&gpu, &s);
    save(&image, "u17-keys-signed.png");
    near(&image, 800, 154, LINE, 1);
    assert!([745, 746].iter().any(|&y| rgb(&image, 800, y) == LINE), "the card's bottom");
    near(&image, 800, 220, LINE, 2);
    // the badge: --live-soft over --surface from 607 to 666, 211 to 229
    let badge = rgb(&image, 612, 220);
    assert!(badge[1] > badge[0] + 8, "the badge's green fill at 612, 220: {badge:?}");
    // Sign out: 516..591 × 412.47..444.47 (Chromium's border rows 412 and 443; within a pixel:
    // the lines' heights here are 13 × 1.4 where Chromium floors them to 1/64 px)
    assert_eq!(rgb(&image, 516, 428), LINE_CONTROL);
    assert_eq!(rgb(&image, 590, 428), LINE_CONTROL);
    assert!([412, 413].iter().any(|&y| rgb(&image, 530, y) == LINE_CONTROL), "Sign out's top");
    assert!([443, 444].iter().any(|&y| rgb(&image, 530, y) == LINE_CONTROL), "Sign out's bottom");
    // eight bullets in the password, white ink in the email
    assert!((530..640).any(|x| rgb(&image, x, 386)[0] > 150), "the password's bullets");
    assert!((530..640).any(|x| (315..330).any(|y| rgb(&image, x, y)[0] > 150)), "the email's text");

    // changed: Save and sign in enabled, the status, the box ticked
    let mut s = state();
    s.set_open(Open::Keys, true);
    let mut canvas = keys_canvas(&gpu, DESKTOP, Keys::default());
    canvas.settle(&gpu, &s);
    {
        let dialog = keys_dialog::dialog(&canvas.ctx);
        let mut d = dialog.lock().unwrap();
        d.form.fr24_email = "pilot@example.com".into();
        d.form.fr24_password = "hunter22".into();
        d.status = Some(Status { text: "Saved, signed in to Flightradar24".into(), error: false });
        d.with_login = true;
        assert_eq!(d.primary_label(), keys_dialog::SAVE_AND_SIGN_IN);
    }
    let image = canvas.settle(&gpu, &s);
    save(&image, "u17-keys-changed.png");
    // the card grows by the status line's 2.4 px: 172.73 to 727.26 in Chromium
    near(&image, 800, 171, BACKDROP_OVER_INK, 2);
    assert!([172, 173].iter().any(|&y| rgb(&image, 800, y) == LINE), "the card's top");
    // Save and sign in: --amber from 516 to 637 (Chromium 516 + 122.19), rows 604.48–636.48
    let blue: Vec<u32> = (500..700).filter(|&x| rgb(&image, x, 620) == [55, 142, 240]).collect();
    assert_eq!((blue.first(), blue.last()), (Some(&516), Some(&637)), "Save and sign in's columns");
    // the status in --live (Chromium's ink rows 579–588), the ticked box in --amber
    let green: Vec<u32> = (570..600).filter(|&y| (516..700).any(|x| { let p = rgb(&image, x, y); p[1] > 120 && p[0] < 90 })).collect();
    assert!(green.first().is_some_and(|&y| y.abs_diff(579) <= 1) && green.last().is_some_and(|&y| y.abs_diff(588) <= 1), "the status's rows {green:?}");
    near(&image, 522, 653, [55, 142, 240], 2);

    // an error in --danger
    {
        let dialog = keys_dialog::dialog(&canvas.ctx);
        dialog.lock().unwrap().status = Some(Status { text: "That file isn't a RustRadar keys file.".into(), error: true });
    }
    let image = canvas.settle(&gpu, &s);
    save(&image, "u17-keys-error.png");
    assert!((516..700).any(|x| (575..596).any(|y| { let p = rgb(&image, x, y); p[0] > 150 && p[1] < 110 })), "the status in red");

    // busy: the spinner in Save, the buttons still drawn
    {
        let dialog = keys_dialog::dialog(&canvas.ctx);
        let mut d = dialog.lock().unwrap();
        d.busy = true;
        d.status = None;
    }
    let image = canvas.settle(&gpu, &s);
    save(&image, "u17-keys-busy.png");

    // a phone: 360 wide at 15, the actions still on one line
    let mut phone = keys_canvas(&gpu, PHONE, Keys::default());
    let image = phone.settle(&gpu, &s);
    save(&image, "u17-keys-phone.png");
    near(&image, 15, 400, LINE, 1);
    near(&image, 374, 400, LINE, 1);
    near(&image, 14, 400, BACKDROP_OVER_INK, 1);
    near(&image, 200, 146, LINE, 1);
    near(&image, 200, 697, LINE, 1);
}

// ------------------------------------------------------------------------------------------------
// through the window's engine

/// The window's frames, drawn offscreen, with egui's events (U6's harness).
struct Window {
    engine: Engine,
    target: Offscreen,
    egui_s: f64,
}

impl Window {
    fn new(gpu: &Gpu, state: AppState) -> Window {
        let mut engine = Engine::new(gpu, FORMAT, state);
        engine.map.set_tiles(io::offline(), None);
        engine.shell = true;
        Window { engine, target: Offscreen::new(gpu, DESKTOP), egui_s: 0.0 }
    }

    fn frame(&mut self, gpu: &Gpu, events: Vec<Event>, now: f64) {
        let mut input = RawInput {
            time: Some(self.egui_s),
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(DESKTOP[0] as f32, DESKTOP[1] as f32))),
            events,
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
        self.egui_s += 0.25;
        self.engine.frame(gpu, self.target.view(), DESKTOP, 1.0, input, now);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    }

    fn key(&mut self, gpu: &Gpu, key: Key, text: Option<&str>, shift: bool, now: f64) {
        let modifiers = egui::Modifiers { shift, ..Default::default() };
        let mut events = vec![Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }];
        events.extend(text.map(|t| Event::Text(t.into())));
        self.frame(gpu, events, now);
    }
}

/// The dialogs in the window: `?` opens the help over the map (the backdrop takes the pointer, the
/// map's keys are off), Escape closes it; the keys dialog takes the page's keys (`p` types, it
/// does not change the focus), Escape while typing closes it, and the map's keys come back.
#[test]
fn the_dialogs_through_the_window() {
    let Some(gpu) = gpu() else { return };
    let now = common::NOW_MS;
    let mut w = Window::new(&gpu, AppState::new(Startup { basemap: Some(Basemap::Dark), ..Default::default() }, None, None, None, ViewState::INITIAL, DESKTOP.map(f64::from)));
    w.frame(&gpu, Vec::new(), now);
    let tilt = MapEvent::Key { key: KeyName::Char('t'), pressed: true };

    w.key(&gpu, Key::Questionmark, Some("?"), true, now + 100.0);
    assert!(w.engine.state.shortcuts.help_open());
    for t in 0..4 {
        w.frame(&gpu, vec![Event::PointerMoved(Pos2::new(1200.0, 450.0))], now + 200.0 + f64::from(t) * 100.0);
    }
    assert!(w.engine.egui.is_pointer_over_egui(), "the backdrop over the map takes the pointer");
    let e = &mut w.engine;
    assert!(!e.input.handle(tilt, false, &mut e.state.controller, now + 700.0), "t does nothing under the help");
    let image = w.target.read(&gpu).expect("read back");
    save(&image, "u17-help-window.png");
    // the card over the backdrop, the backdrop over the map
    near(&image, 539, 450, LINE, 1);
    assert!(rgb(&image, 1300, 450).iter().all(|&c| c < 40), "the map darkened by the backdrop: {:?}", rgb(&image, 1300, 450));
    w.key(&gpu, Key::Escape, None, false, now + 800.0);
    assert!(!w.engine.state.shortcuts.help_open());

    // the keys dialog from the map controls' button
    w.engine.state.apply(Action::Open(Open::Keys, true), now + 900.0);
    for t in 0..3 {
        w.frame(&gpu, Vec::new(), now + 1000.0 + f64::from(t) * 100.0);
    }
    // a click in the email field, then p: typed, not the aircraft's focus
    let email = Pos2::new(720.0, 341.0);
    let press = |pressed| Event::PointerButton { pos: email, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
    w.frame(&gpu, vec![Event::PointerMoved(email), press(true)], now + 1400.0);
    w.frame(&gpu, vec![press(false)], now + 1500.0);
    w.engine.state.apply(Action::SetMode(rustradar_core::settings::Mode::Ships), now + 1550.0);
    w.key(&gpu, Key::P, Some("p"), false, now + 1600.0);
    assert_eq!(keys_dialog::dialog(&w.engine.egui).lock().unwrap().form.fr24_email, "p");
    assert_eq!(w.engine.state.persisted.settings.mode, rustradar_core::settings::Mode::Ships, "p stayed in the field");
    let e = &mut w.engine;
    assert!(!e.input.handle(tilt, false, &mut e.state.controller, now + 1700.0), "t does nothing under the keys dialog");
    w.key(&gpu, Key::Escape, None, false, now + 1800.0);
    assert!(!w.engine.state.shortcuts.keys_open(), "Escape in a field closes the dialog");
    w.frame(&gpu, Vec::new(), now + 1900.0);
    let e = &mut w.engine;
    assert!(e.input.handle(tilt, false, &mut e.state.controller, now + 2000.0), "t tilts again");
}
