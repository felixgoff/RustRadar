//! Item U4's acceptance (plan stage 7; doc 06 §6.1, §11, §11.1, §12.1–12.2, §7 item 6): the
//! second widget gallery drawn by egui through `ui::widgets`, as a window frame draws the UI, on
//! SwiftShader.
//!
//! - `the_widget_gallery`: segmented groups, toggle rows, chips, token inputs, tabs, text fields
//!   and both sliders in four `.surface` cards (an aircraft filter panel's body, a layers and
//!   settings popover, the train and ship filters, a detail panel's tabs and fields) at the
//!   places a Chromium rendering of today's CSS for the same markup puts them
//!   (`target/shots/u4-widget-gallery.png`). Sizes, places and colours are checked against what
//!   Chromium laid out (the numbers in [`TODAY`]): segmented 34.19 px a row, toggle rows 34.98
//!   and 32, the switch 34 × 20, chips 26, the token field 92 and 42, tabs 39.19 with the 2 px
//!   underline over the rule, fields 36 and the search box 38, the altitude rail at 9 px and its
//!   thumbs at 95 and 203.
//! - `todays_ship_sort_and_search`: the ships list's sort control and the search box rebuilt at
//!   their places in `docs/port/reference/satellite-v6-1440x900.png`: the same borders, the
//!   chosen option's fill, and the labels and placeholder as wide and in the same places.
//!
//! Needs a wgpu adapter (SwiftShader or lavapipe through `VK_ICD_FILENAMES`); without one it prints
//! `skipped: …` and passes.

mod common;

use std::path::Path;

use common::shots::{Rect, bounds, gpu, pixel, save};
use egui::{Color32, CornerRadius, Id, Pos2, RawInput, Sense, Stroke, StrokeKind, Vec2};
use rustradar_app::ui::icons::{Icon, STROKE_THIN};
use rustradar_app::ui::text::{self, Style};
use rustradar_app::ui::theme;
use rustradar_app::ui::widgets::chips::{self, Chip, ChipOption, FilterChips, Kind};
use rustradar_app::ui::widgets::range_slider::{DualRange, Range};
use rustradar_app::ui::widgets::segmented::{Choice, Segmented};
use rustradar_app::ui::widgets::tabs::{Tab, Tabs};
use rustradar_app::ui::widgets::text_field::{self, TextField};
use rustradar_app::ui::widgets::toggle_row::ToggleRow;
use rustradar_app::ui::widgets::token_input::{self, Suggestion, TokenInput};
use rustradar_map::Gpu;
use rustradar_map::headless::{FORMAT, Image, Offscreen};

/// The canvas, CSS px, at scale 1: the desktop reference pictures' size.
const SIZE: [u32; 2] = [1440, 900];

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

/// A `.surface` card at `pos`, `width` wide (border box), with `padding` and `gap` between its
/// parts (a flex column), holding `add`.
fn card<R>(ui: &mut egui::Ui, name: &str, pos: Pos2, width: f32, padding: f32, gap: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> (egui::Rect, R) {
    let shown = egui::Area::new(Id::NULL.with(name)).fixed_pos(pos).fade_in(false).show(ui.ctx(), |ui| {
        theme::surface()
            .inner_margin(padding)
            .show(ui, |ui| {
                ui.set_width(width - 2.0 * (padding + theme::BORDER));
                ui.spacing_mut().item_spacing = Vec2::new(0.0, gap);
                add(ui)
            })
            .inner
    });
    (shown.response.rect, shown.inner)
}

/// A filter group's label: 12 px at 600 in `--text-2`.
fn group_label(ui: &mut egui::Ui, label: &str) -> egui::Rect {
    text::label(ui, text::job(label, &Style::ui(theme::TEXT_SM, 600, theme::TEXT_2)), Sense::hover()).rect
}

/// A quiet line: 12 px `--text-3`, wrapped.
fn hint(ui: &mut egui::Ui, hint: &str) -> egui::Rect {
    let w = ui.available_width();
    text::label(ui, text::wrapped(hint, &Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3), w), Sense::hover()).rect
}

/// Runs `add` with the vertical gap `gap` between its parts.
fn column<R>(ui: &mut egui::Ui, gap: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = gap;
        add(ui)
    })
    .inner
}

/// The airlines in the gallery's token input.
const AIRLINES: [(&str, &str, u32); 5] = [("DLH", "Lufthansa", 41), ("KLM", "KLM Royal Dutch Airlines", 33), ("RYR", "Ryanair", 30), ("BAW", "British Airways", 12), ("SAS", "Scandinavian Airlines System", 9)];

/// What the gallery puts where.
#[derive(Debug, Clone)]
struct Parts {
    cards: Vec<egui::Rect>,
    filter_field: egui::Rect,
    categories: Vec<egui::Rect>,
    dual: egui::Rect,
    airlines: egui::Rect,
    types: egui::Rect,
    basemap: egui::Rect,
    sky: egui::Rect,
    toggles: Vec<egui::Rect>,
    size: egui::Rect,
    range: egui::Rect,
    kind: egui::Rect,
    delayed: egui::Rect,
    motion: egui::Rect,
    sizes: egui::Rect,
    flight_tabs: egui::Rect,
    airport_tabs: egui::Rect,
    search: egui::Rect,
    filled: egui::Rect,
    password: egui::Rect,
    sort: egui::Rect,
}

impl Default for Parts {
    fn default() -> Self {
        let n = egui::Rect::NOTHING;
        Parts {
            cards: Vec::new(),
            filter_field: n,
            categories: Vec::new(),
            dual: n,
            airlines: n,
            types: n,
            basemap: n,
            sky: n,
            toggles: Vec::new(),
            size: n,
            range: n,
            kind: n,
            delayed: n,
            motion: n,
            sizes: n,
            flight_tabs: n,
            airport_tabs: n,
            search: n,
            filled: n,
            password: n,
            sort: n,
        }
    }
}

/// The values the gallery's widgets show.
struct Values {
    filter_text: String,
    low: f64,
    high: f64,
    airlines: Vec<String>,
    types: Vec<String>,
    basemap: u8,
    sky: u8,
    toggles: [bool; 4],
    size: u8,
    exaggeration: f64,
    kinds_hidden: Vec<u8>,
    operators_hidden: Vec<u8>,
    delayed: bool,
    motion: u8,
    sizes_hidden: Vec<u8>,
    flight_tab: u8,
    airport_tab: u8,
    search: String,
    filled: String,
    password: String,
    sort: u8,
}

impl Default for Values {
    fn default() -> Self {
        Values {
            filter_text: String::new(),
            low: 10_000.0,
            high: 30_000.0,
            airlines: vec!["BAW".into(), "SAS".into()],
            types: vec![],
            basemap: 0,
            sky: 1,
            toggles: [true, false, true, false],
            size: 1,
            exaggeration: 4.0,
            kinds_hidden: vec![2],
            operators_hidden: vec![],
            delayed: false,
            motion: 1,
            sizes_hidden: vec![],
            flight_tab: 1,
            airport_tab: 0,
            search: String::new(),
            filled: "LH400".into(),
            password: "hunter22".into(),
            sort: 0,
        }
    }
}

/// The gallery: the four cards of today's markup (see the module docs).
fn gallery(ui: &mut egui::Ui, v: &mut Values, p: &mut Parts) {
    let (a, _) = card(ui, "a", Pos2::new(16.0, 16.0), 320.0, 16.0, 16.0, |ui| {
        column(ui, 4.0, |ui| {
            group_label(ui, "Callsign, flight or registration");
            p.filter_field = TextField::new("filter-text", &mut v.filter_text).placeholder("e.g. BAW, LH400, D-AIM").show(ui).frame;
        });
        column(ui, 0.0, |ui| {
            chips::legend(ui, Id::NULL.with("categories"), "Categories", &[("All", true), ("None", true)]);
            ui.add_space(chips::LEGEND_GAP);
            let on = [true, true, false, true, false, false, true, false, false, false, true, true];
            let names = ["Passenger", "Cargo", "Military and government", "Business jets", "General aviation", "Helicopters", "Lighter than air", "Gliders", "Drones", "Ground vehicles", "Other", "Uncategorised"];
            let list: Vec<Chip<'_>> = names.iter().zip(on).map(|(n, on)| Chip::new(Kind::Category, n, on)).collect();
            p.categories = chips::chips(ui, Id::NULL.with("category-chips"), &list, true).iter().map(|r| r.rect).collect();
        });
        column(ui, 0.0, |ui| {
            // FilterPanel's altitude legend: the label, and the value on its baseline at the right
            let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.8), Sense::hover());
            let label = ui.painter().layout_job(text::job("Altitude", &Style::ui(theme::TEXT_SM, 600, theme::TEXT_2)));
            ui.painter().galley(row.min, label, Color32::PLACEHOLDER);
            let value = ui.painter().layout_job(text::job("10,000 – 30,000 ft", &Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_1)));
            ui.painter().galley(Pos2::new(row.right() - value.size().x, row.top()), value, Color32::PLACEHOLDER);
            ui.add_space(8.0);
            p.dual = DualRange::new("altitude", 0.0, 50_000.0, 1_000.0).show(ui, &mut v.low, &mut v.high).rect;
        });
        let suggest = |q: &str, picked: &[String]| -> Vec<Suggestion> {
            let q = token_input::js_trim(q).to_uppercase();
            AIRLINES
                .iter()
                .filter(|(code, name, _)| !picked.iter().any(|v| v == code) && (q.is_empty() || code.starts_with(&q) || name.to_uppercase().contains(&q)))
                .take(6)
                .map(|(code, name, n)| Suggestion::new(*code, *name).detail(format!("{code} · {n} live")))
                .collect()
        };
        let chip = |code: &str| AIRLINES.iter().find(|(c, ..)| *c == code).map_or(code.to_owned(), |(c, name, _)| format!("{c} {name}"));
        p.airlines = TokenInput::new("airlines", "Airlines", suggest).free(token_input::three_letters).chip(chip).placeholder("Name or code").show(ui, &mut v.airlines).rect;
        p.types = TokenInput::new("types", "Aircraft types", |_: &str, _: &[String]| Vec::new())
            .free(token_input::type_code)
            .placeholder("e.g. A388 or B77")
            .hint("A type code, or its start: B77 matches every Boeing 777.")
            .show(ui, &mut v.types)
            .rect;
        // the airlines' input has the focus, so its suggestions show
        let edit = ui.make_persistent_id(Id::NULL.with("airlines")).with("edit");
        if ui.memory(|m| m.focused().is_none()) {
            ui.memory_mut(|m| m.request_focus(edit));
        }
    });
    let (b, _) = card(ui, "b", Pos2::new(352.0, 16.0), 306.0, 16.0, 6.0, |ui| {
        let basemaps = [Choice::new(0u8, "Dark").key("1"), Choice::new(1, "Satellite").key("2"), Choice::new(2, "Realistic").key("3")];
        p.basemap = Segmented::new("basemap", "Basemap", &basemaps).hide_legend().show(ui, &mut v.basemap).rect;
        ui.add_space(4.0);
        let skies = [Choice::new(0u8, "Live"), Choice::new(1, "Sun"), Choice::new(2, "Clouds"), Choice::new(3, "Rain"), Choice::new(4, "Fog"), Choice::new(5, "Snow")];
        p.sky = Segmented::new("sky", "Weather", &skies).columns(3).show(ui, &mut v.sky).rect;
        // LayersMenu's `section > .segmented { margin-bottom: 4px }` holds for both groups
        ui.add_space(4.0);
        let rows = [("Tilt-shift", Some("Top and bottom blurred, like a miniature"), None), ("Terrain", Some("Mountains and valleys in relief"), Some("E")), ("3D buildings", Some("Appear at street level"), Some("B")), ("Place names", None, Some("L"))];
        for (i, (label, h, key)) in rows.into_iter().enumerate() {
            let mut row = ToggleRow::new(("toggle", i), label);
            if let Some(h) = h {
                row = row.hint(h);
            }
            if let Some(k) = key {
                row = row.key(k);
            }
            p.toggles.push(row.show(ui, &mut v.toggles[i]).rect);
        }
        // the settings menu after a rule
        ui.add_space(6.0);
        let (rule, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(rule, CornerRadius::ZERO, theme::LINE);
        ui.add_space(6.0);
        column(ui, 4.0, |ui| {
            let sizes = [Choice::new(0u8, "Small"), Choice::new(1, "Medium"), Choice::new(2, "Large"), Choice::new(3, "Extra large")];
            p.size = Segmented::new("label-size", "Place name size", &sizes).columns(2).disabled(true).show(ui, &mut v.size).rect;
            hint(ui, "Place names are off in Layers");
        });
        column(ui, 8.0, |ui| {
            let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.2), Sense::hover());
            let label = ui.painter().layout_job(text::job("Altitude exaggeration", &Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1)));
            ui.painter().galley(row.min, label, Color32::PLACEHOLDER);
            let value = ui.painter().layout_job(text::job("×4", &Style::data(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::AMBER)));
            ui.painter().galley(Pos2::new(row.right() - value.size().x, row.top()), value, Color32::PLACEHOLDER);
            p.range = Range::new("exaggeration", 1.0, 40.0, 1.0).show(ui, &mut v.exaggeration).rect;
            ui.add_space(-4.0);
            hint(ui, "Applies when zoomed out; true scale up close");
        });
    });
    let (c, _) = card(ui, "c", Pos2::new(674.0, 16.0), 320.0, 16.0, 16.0, |ui| {
        let kinds = [
            ChipOption::new(0u8, "High-speed").color(Color32::from_rgb(226, 92, 196)).count(12),
            ChipOption::new(1, "Intercity").color(Color32::from_rgb(128, 132, 246)).count(87),
            ChipOption::new(2, "Regional").color(Color32::from_rgb(96, 212, 160)).count(1_412),
            ChipOption::new(3, "Suburban").color(Color32::from_rgb(112, 190, 232)).count(230),
            ChipOption::new(4, "Other").color(Color32::from_rgb(158, 150, 140)).count(0),
        ];
        p.kind = FilterChips::new("kind", "Kind", &kinds).show(ui, &mut v.kinds_hidden).rect;
        let operators = [ChipOption::new(0u8, "SJ").color(Color32::from_rgb(232, 232, 236)).count(64), ChipOption::new(1, "SL").color(Color32::from_rgb(70, 146, 236)).count(198), ChipOption::new(2, "Unknown").count(3)];
        FilterChips::new("operator", "Operator", &operators).hint("Each operator's trains come live on their own stream; one left out isn't streamed.").show(ui, &mut v.operators_hidden);
        p.delayed = ToggleRow::new("delayed", "Delayed only").hint("5 min or more late, as the operator reports").show(ui, &mut v.delayed).rect;
        column(ui, 8.0, |ui| {
            group_label(ui, "Movement");
            let motions = [Choice::new(0u8, "All"), Choice::new(1, "Under way"), Choice::new(2, "Moored")];
            p.motion = Segmented::new("motion", "Movement", &motions).hide_legend().show(ui, &mut v.motion).rect;
            hint(ui, "Moored includes ships reporting no heading.");
        });
        let sizes = [ChipOption::new(0u8, "Small"), ChipOption::new(1, "Medium"), ChipOption::new(2, "Large")];
        p.sizes = FilterChips::new("sizes", "Size", &sizes).hint("Zoom in to tell sizes apart: zoomed out, VesselFinder lists every ship as medium.").disabled(true).show(ui, &mut v.sizes_hidden).rect;
    });
    let (d, _) = card(ui, "d", Pos2::new(1010.0, 16.0), 360.0, 0.0, 0.0, |ui| {
        let flight = [Tab::new(0u8, "Overview"), Tab::new(1, "Route"), Tab::new(2, "Aircraft"), Tab::new(3, "History")];
        p.flight_tabs = Tabs::new("flight-tabs", &flight).show(ui, &mut v.flight_tab).rect;
        ui.add_space(16.0);
        let airport = [Tab::new(0u8, "Departures").icon(Icon::PlaneTakeoff), Tab::new(1, "Arrivals").icon(Icon::PlaneLanding)];
        p.airport_tabs = Tabs::new("airport-tabs", &airport).show(ui, &mut v.airport_tab).rect;
        egui::Frame::new().inner_margin(16.0).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 12.0;
            p.search = search_box(ui, &mut v.search);
            column(ui, 4.0, |ui| {
                group_label(ui, "Callsign, flight or registration");
                p.filled = TextField::new("filled", &mut v.filled).show(ui).frame;
            });
            column(ui, 4.0, |ui| {
                group_label(ui, "Password");
                p.password = TextField::new("password", &mut v.password).password(true).show(ui).frame;
            });
            let sorts = [Choice::new(0u8, "Size"), Choice::new(1, "Name"), Choice::new(2, "Distance")];
            p.sort = Segmented::new("sort", "Sort ships by", &sorts).hide_legend().show(ui, &mut v.sort).rect;
        });
    });
    p.cards = vec![a, b, c, d];
}

/// The search box's field (doc 06 §5.1) as U8 builds it on the text field: 38 px, 14 px text, the
/// `Search` icon, and the `/` key cap in the room kept at the right.
fn search_box(ui: &mut egui::Ui, query: &mut String) -> egui::Rect {
    let field = TextField::new("search", query).height(text_field::SEARCH_HEIGHT).size(theme::TEXT_BASE).icon(Icon::Search.at(16.0, STROKE_THIN)).trailing(20.0).native_padding().placeholder("Ship name, MMSI or address").show(ui);
    let cap = egui::Rect::from_center_size(field.trailing.center(), Vec2::splat(20.0));
    ui.painter().rect_stroke(cap, CornerRadius::same(theme::RADIUS), Stroke::new(theme::BORDER, theme::LINE_CONTROL), StrokeKind::Inside);
    let slash = ui.painter().layout_job(text::job("/", &Style::data(theme::TEXT_XS, 600, theme::TEXT_3).letter_spacing(0.0)));
    ui.painter().galley(cap.center() - slash.size() / 2.0, slash, Color32::PLACEHOLDER);
    field.frame
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

fn lum(p: [u8; 4]) -> f64 {
    (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0
}

#[track_caller]
fn near(a: f32, b: f32, tolerance: f32, what: &str) {
    assert!((a - b).abs() <= tolerance, "{what}: {a} for {b}");
}

/// What Chromium laid out for the same markup with today's CSS (`getBoundingClientRect`, CSS px):
/// the boxes the gallery is checked against.
mod today {
    /// Segmented: one row; two rows; an option.
    pub const SEGMENTED: f32 = 34.19;
    pub const SEGMENTED_2_ROWS: f32 = 64.38;
    pub const OPTION: f32 = 28.19;
    /// Toggle rows with a hint and without.
    pub const TOGGLE_HINT: f32 = 34.98;
    pub const TOGGLE: f32 = 32.0;
    /// The text field and the search box.
    pub const FIELD: f32 = 36.0;
    pub const SEARCH: f32 = 38.0;
    /// A chip, and today's first category chip's width ("Passenger" with its check).
    pub const CHIP: f32 = 26.0;
    pub const PASSENGER: f32 = 89.23;
    /// Token fields: two chips on two lines and the input on a third; empty.
    pub const TOKENS_3_LINES: f32 = 92.0;
    pub const TOKENS_EMPTY: f32 = 42.0;
    /// The tab row with its rule.
    pub const TABS: f32 = 39.19;
    /// The altitude slider: its box and its thumbs' centres at 10,000 and 30,000 ft (x from the
    /// slider's left).
    pub const DUAL: f32 = 20.0;
    pub const LOW_THUMB: f32 = 62.0;
    pub const HIGH_THUMB: f32 = 170.0;
}

#[test]
fn the_widget_gallery() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let mut values = Values::default();
    let mut parts = Parts::default();
    // areas show from their second frame; the focus asked in the first opens the list in the second
    for time in [0.0, 1.0, 2.0] {
        parts = Parts::default();
        canvas.frame(&gpu, time, |ui| gallery(ui, &mut values, &mut parts));
    }
    let image = canvas.read(&gpu);
    save(&image, "u4-widget-gallery.png");
    eprintln!("{parts:#?}");

    // sizes as Chromium laid today's CSS out
    near(parts.basemap.height(), today::SEGMENTED, 0.02, "a segmented row");
    near(parts.sort.height(), today::SEGMENTED, 0.02, "the sort control");
    near(parts.size.height(), today::SEGMENTED_2_ROWS + 18.2 + 8.0, 0.05, "two rows under a legend");
    near(parts.toggles[0].height(), today::TOGGLE_HINT, 0.03, "a toggle row with a hint");
    near(parts.toggles[3].height(), today::TOGGLE, 0.0, "a toggle row without");
    near(parts.filter_field.height(), today::FIELD, 0.0, "the text field");
    near(parts.filled.height(), today::FIELD, 0.0, "the filled field");
    near(parts.search.height(), today::SEARCH, 0.0, "the search box");
    for r in &parts.categories {
        near(r.height(), today::CHIP, 0.0, "a chip");
    }
    near(parts.categories[0].width(), today::PASSENGER, 1.5, "the Passenger chip");
    near(parts.dual.height(), today::DUAL, 0.0, "the altitude slider");
    near(parts.flight_tabs.height(), today::TABS, 0.02, "the tab row");
    let label_and_gap = 16.8 + 4.0;
    near(parts.types.height(), label_and_gap + today::TOKENS_EMPTY + 4.0 + 15.39, 0.3, "the empty token input, its label and hint");

    // the switch: 34 × 20 at the right end of each row; on: --amber with the knob at the right
    for (i, (r, on)) in parts.toggles.iter().zip([true, false, true, false]).enumerate() {
        let track = Rect { x0: r.max.x as u32 - 34, y0: (r.center().y - 10.0).round() as u32, x1: r.max.x as u32, y1: (r.center().y + 10.0).round() as u32 };
        let mid_y = (track.y0 + track.y1) / 2;
        let track_colour = if on { theme::AMBER } else { theme::LINE_CONTROL };
        let knob_colour = if on { theme::AMBER_INK } else { theme::TEXT_1 };
        let (knob_x, empty_x) = if on { (track.x1 - 10, track.x0 + 6) } else { (track.x0 + 10, track.x1 - 6) };
        assert_eq!(pixel(&image, knob_x, mid_y), rgb(knob_colour), "toggle {i}: the knob");
        assert_eq!(pixel(&image, empty_x, mid_y), rgb(track_colour), "toggle {i}: the track");
        assert_eq!(pixel(&image, track.x0 + 17, track.y0 + 1), rgb(track_colour), "toggle {i}: the track's top");
        let k = bounds(&image, track, |p| lum(p) > 200.0).expect("a knob");
        assert!((13..=15).contains(&k.width()) && (13..=15).contains(&k.height()), "toggle {i}: a 14 px knob: {k:?}");
    }

    // segmented: the box's border, the chosen option filled 3 px in, the rest of the surface
    let sort = px(parts.sort);
    let mid = (sort.y0 + sort.y1) / 2;
    assert_eq!(pixel(&image, sort.x0, mid), rgb(theme::LINE_CONTROL), "the box's left border");
    assert_eq!(pixel(&image, sort.x0 + 1, mid), rgb(theme::SURFACE), "2 px of padding");
    assert_eq!(pixel(&image, sort.x0 + 3, mid), rgb(theme::AMBER), "the chosen option");
    let option_w = (parts.sort.width() - 6.0 - 4.0) / 3.0;
    assert_eq!(pixel(&image, (parts.sort.min.x + 3.0 + option_w + 1.0) as u32, mid), rgb(theme::SURFACE), "the 2 px gap");
    let fill = bounds(&image, sort, |p| p == rgb(theme::AMBER)).expect("a fill");
    near(fill.height() as f32, today::OPTION, 1.0, "the chosen option's height");
    // disabled at 0.4: the chosen fill over the surface
    let size = px(parts.size);
    let faded = bounds(&image, size, |p| p[2] > p[0] + 40).expect("the chosen option, faded");
    let p = pixel(&image, faded.x0 + 4, (faded.y0 + faded.y1) / 2);
    let want = [55.0 * 0.4 + 50.0 * 0.6, 142.0 * 0.4 + 50.0 * 0.6, 240.0 * 0.4 + 50.0 * 0.6];
    for ch in 0..3 {
        assert!((f64::from(p[ch]) - want[ch]).abs() <= 1.5, "the disabled group at 0.4: {p:?} for {want:?}");
    }

    // chips: an off chip has no fill, an on chip --surface-raised, both a --line-control border
    let on = px(parts.categories[0]);
    let off = px(parts.categories[2]);
    assert_eq!(pixel(&image, on.x1 - 6, (on.y0 + on.y1) / 2), rgb(theme::SURFACE_RAISED), "an on chip");
    assert_eq!(pixel(&image, off.x1 - 6, (off.y0 + off.y1) / 2), rgb(theme::SURFACE), "an off chip");
    assert_eq!(pixel(&image, (on.x0 + on.x1) / 2, on.y0), rgb(theme::LINE_CONTROL), "a chip's border");

    // the altitude slider: the rail 2 px at 9 px, the span in --amber between the thumbs' centres
    let dual = parts.dual;
    let rail_y = (dual.min.y + 10.0).floor() as u32;
    assert_eq!(pixel(&image, (dual.min.x + 20.0) as u32, rail_y), rgb(theme::LINE_CONTROL), "the rail");
    assert_eq!(pixel(&image, (dual.min.x + 116.0) as u32, rail_y), rgb(theme::AMBER), "the span");
    assert_eq!(pixel(&image, (dual.min.x + 116.0) as u32, rail_y + 3), rgb(theme::SURFACE), "the rail is 2 px");
    for (name, x) in [("low", today::LOW_THUMB), ("high", today::HIGH_THUMB)] {
        let c = Pos2::new(dual.min.x + x, dual.min.y + 10.0);
        assert_eq!(pixel(&image, c.x as u32, c.y as u32), rgb(theme::INK), "the {name} thumb's middle");
        // the ring's outer side (the span runs into the inner one): 8 px from the centre
        // pixels at least half covered by the ring
        let ring = bounds(&image, px(egui::Rect::from_center_size(c, Vec2::splat(20.0))), |p| p[2] > 145 && p[0] < 90).expect("a ring");
        assert!((15..=17).contains(&ring.height()), "the {name} thumb is 16 px tall: {ring:?}");
        let outer = if name == "low" { c.x - ring.x0 as f32 } else { ring.x1 as f32 - c.x };
        assert!((7.0..=9.0).contains(&outer), "the {name} thumb's radius: {outer} in {ring:?}");
    }

    // tabs: the rule under the row, the chosen tab's 2 px underline over it
    let tabs = px(parts.flight_tabs);
    let underline = bounds(&image, tabs, |p| p == rgb(theme::AMBER)).expect("an underline");
    assert_eq!(underline.height(), 2, "{underline:?}");
    let bottom = parts.flight_tabs.max.y.round() as u32;
    assert_eq!(underline.y1, bottom, "over the rule: {underline:?} in {tabs:?}");
    assert_eq!(pixel(&image, tabs.x0 + 4, bottom - 1), rgb(theme::LINE), "the rule");
    assert_eq!(pixel(&image, tabs.x0 + 4, bottom - 2), rgb(theme::SURFACE), "a 1 px rule");

    // the focused token input: an --amber border and its suggestions under it
    let airlines = px(parts.airlines);
    let field_border = bounds(&image, airlines, |p| p == rgb(theme::AMBER)).expect("a focused border");
    near(field_border.height() as f32, today::TOKENS_3_LINES, 1.0, "the field with two chips");
    assert!(airlines.y1 > field_border.y1 + 90, "the suggestions under the field: {airlines:?} {field_border:?}");

    // a field's border, --ink inside
    let field = px(parts.filter_field);
    assert_eq!(pixel(&image, (field.x0 + field.x1) / 2, field.y0), rgb(theme::LINE_CONTROL), "a field's border");
    assert_eq!(pixel(&image, field.x1 - 4, field.y0 + 4), rgb(theme::INK), "a field's inside");
}

/// The ships list's sort control and the search box at their places in today's
/// `satellite-v6` picture (the sidebar's cards 320 px wide from x 16).
fn ship_sort_and_search(ui: &mut egui::Ui, query: &mut String, sort: &mut u8) -> (egui::Rect, egui::Rect) {
    // the two sidebar cards behind them
    for (min, max) in [(Pos2::new(16.0, 16.0), Pos2::new(336.0, 262.0)), (Pos2::new(16.0, 276.0), Pos2::new(336.0, 450.0))] {
        ui.painter().rect(egui::Rect::from_min_max(min, max), theme::RADIUS, theme::SURFACE, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
    }
    let search = egui::Area::new(Id::NULL.with("search")).fixed_pos(Pos2::new(33.0, 69.0)).fade_in(false).show(ui.ctx(), |ui| {
        ui.set_width(286.0);
        search_box(ui, query)
    });
    let sort_rect = egui::Area::new(Id::NULL.with("sort")).fixed_pos(Pos2::new(33.0, 316.0)).fade_in(false).show(ui.ctx(), |ui| {
        ui.set_width(286.0);
        let sorts = [Choice::new(0u8, "Size"), Choice::new(1, "Name"), Choice::new(2, "Distance")];
        Segmented::new("sort", "Sort ships by", &sorts).hide_legend().show(ui, sort).rect
    });
    (search.inner, sort_rect.inner)
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

#[test]
fn todays_ship_sort_and_search() {
    let Some(gpu) = gpu() else { return };
    let mut canvas = Canvas::new(&gpu);
    let (mut query, mut sort) = (String::new(), 0u8);
    let mut rects = (egui::Rect::NOTHING, egui::Rect::NOTHING);
    for time in [0.0, 1.0] {
        canvas.frame(&gpu, time, |ui| rects = ship_sort_and_search(ui, &mut query, &mut sort));
    }
    let image = canvas.read(&gpu);
    save(&image, "u4-ship-sort-and-search.png");
    let today = reference("satellite-v6-1440x900.png");
    let (search, sort_rect) = rects;
    eprintln!("search {search:?} sort {sort_rect:?}");

    // the same boxes: the search box's border rows at 69 and 106, the sort control's at 316 and
    // 349, from x 33 to 318 (the picture is a dithered 256-colour PNG: borders within 8 levels)
    let border = |img: &Image, x: u32, y: u32| lum(pixel(img, x, y));
    for (what, x, y) in [("search top", 100, 69), ("search bottom", 100, 106), ("search left", 33, 88), ("search right", 318, 88), ("sort top", 200, 316), ("sort bottom", 200, 349), ("sort left", 33, 332), ("sort right", 318, 332)] {
        let (ours, theirs) = (border(&image, x, y), border(&today, x, y));
        assert!((ours - theirs).abs() <= 8.0, "{what}: {ours} here, {theirs} today");
        assert!(ours > 95.0, "{what} is a border: {ours}");
    }
    // the chosen option's fill: x 36–127, y 319–346 in both
    let fill = |img: &Image| bounds(img, Rect { x0: 30, y0: 310, x1: 330, y1: 355 }, |p| p[2] > 200 && p[0] < 80 && p[1] > 120).expect("a fill");
    let (ours, theirs) = (fill(&image), fill(&today));
    assert_eq!((ours.x0, ours.x1, ours.y0, ours.y1), (theirs.x0, theirs.x1, theirs.y0, theirs.y1), "the chosen option: {ours:?}, today {theirs:?}");

    // each label as wide as today's and in the same place, ±1 px: "Size" white on the fill,
    // "Name" and "Distance" in --text-2 on the surface; the search box's icon, placeholder and
    // key cap
    let ink = |img: &Image, r: Rect, under: f64, by: f64| bounds(img, r, |p| lum(p) - under > by).expect("ink");
    for (what, r, under, by) in [
        ("Size", Rect { x0: 37, y0: 320, x1: 127, y1: 345 }, 145.0, 60.0),
        ("Name", Rect { x0: 130, y0: 320, x1: 220, y1: 345 }, 50.0, 40.0),
        ("Distance", Rect { x0: 223, y0: 320, x1: 316, y1: 345 }, 50.0, 40.0),
        ("the icon", Rect { x0: 40, y0: 75, x1: 62, y1: 100 }, 30.0, 40.0),
        ("the placeholder", Rect { x0: 64, y0: 75, x1: 280, y1: 100 }, 30.0, 40.0),
        ("the key cap", Rect { x0: 296, y0: 72, x1: 318, y1: 104 }, 30.0, 40.0),
    ] {
        let (ours, theirs) = (ink(&image, r, under, by), ink(&today, r, under, by));
        eprintln!("{what}: ink {ours:?}, today {theirs:?}");
        for (a, b, edge) in [(ours.x0, theirs.x0, "left"), (ours.x1, theirs.x1, "right"), (ours.y0, theirs.y0, "top"), (ours.y1, theirs.y1, "bottom")] {
            assert!(a.abs_diff(b) <= 1, "{what}'s {edge}: {ours:?}, today {theirs:?}");
        }
    }
}
