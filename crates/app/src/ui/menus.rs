//! The map controls' two menus (doc 06 §19.2–19.3; `LayersMenu.svelte`, `SettingsMenu.svelte`):
//! what is drawn, and how the app behaves. [`super::map_controls`] puts one at a time in its
//! popover (300 px of content, padding 16, a flex column with gap 12) and calls [`layers`] or
//! [`settings`] for the content.
//!
//! **Layers** ([`layers`]), sections a flex column with gap 6:
//! - *Map*: the basemap (segmented, legend hidden, `Dark 1`, `Satellite 2`, `Realistic 3`); on the
//!   realistic map the weather (`Weather`, 3 columns: Live, Sun, Clouds, Rain, Fog, Snow) and the
//!   Tilt-shift row; the Terrain (E), 3D buildings (B), Place names (L) and Rail map (R) rows; with
//!   the rail map on, what it shows (`Speed limits` / `Signal blocks`, legend hidden) and a quiet
//!   line saying what that is (`.rail-shows`: margins 2 above and 4 below, gap 4). A segmented
//!   group that is the section's own child keeps 4 px under it.
//! - *Aircraft*, only while the aircraft are on the map: the Aircraft labels row (K).
//! - *Sky*: the Weather radar (W) and Day and night (D) rows.
//!
//! **Settings** ([`settings`]), sections a flex column with gap 12, each heading pulled 4 px
//! closer to what follows:
//! - *Display*: `Place name size` (2 columns: Small 0.85, Medium 1, Large 1.25, Extra large 1.5),
//!   at opacity 0.4 and inert while place names are off, with `Place names are off in Layers` 4 px
//!   under it; then `Altitude exaggeration` with `×{n}` (Overpass Mono, `--amber`) at the right,
//!   8 px over the native slider (1–40, step 1), and `Applies when zoomed out; true scale up close`
//!   4 px under the slider (8 of gap, −4 of margin).
//! - *Data*: the Aircraft-reported data row; on phones also the `.button` with `KeyRound` 14/1.75,
//!   `API keys and Flightradar24 login`, which closes the menu and opens the keys dialog.
//!
//! Each section after the first has a 1 px `--line` rule on top and 12 px under it; a heading is
//! 12 px at weight 600 in `--text-3` (a 16.8 px line), 4 px over the section's gap in Layers.
//!
//! **Changes** become actions, applied after the pass: the basemap
//! [`Action::SetBasemap`]; the rows that have a key [`Action::Toggle`] (as the keys do); the
//! weather, Tilt-shift, the rail map's choice, the name size, the exaggeration and the
//! aircraft-reported data [`Action::Set`] with a [`Setting`]. The settings are saved by the
//! settings store's next poll (U5).
//!
//! The rail map's choice shows whenever the rail map is on (`railMap && isTauri()`: the native app
//! is the backend's own build).
//!
//! **Checked** against Chromium's layout of today's components, server-rendered by Svelte inside
//! `MapControls.svelte` with the same props and laid out with app.css at 1440 × 900
//! (`tests/map_controls.rs`): every heading, rule, segmented box, row, switch, key cap and slider
//! lands on Chromium's pixels or within one. Text sits up to a pixel lower (egui's baseline, U1).

use egui::emath::GuiRounding;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Ui, Vec2};
use rustradar_core::geo::Basemap;
use rustradar_core::num::js_string;
use rustradar_core::settings::{RailShows, shown_kinds};
use rustradar_core::weather::Sky;

use super::icons::{self, Icon};
use super::text::{self, Style};
use super::theme;
use super::widgets::button::Button;
use super::widgets::range_slider::Range;
use super::widgets::segmented::{Choice, Segmented};
use super::widgets::toggle_row::ToggleRow;
use crate::state::shortcuts::{Open, Toggle};
use crate::state::{Action, AppState, Setting};

/// The gap between the popover's children, its sections (`.menu { gap: var(--space-3) }`).
pub const MENU_GAP: f32 = theme::SPACE_3;
/// A section's padding over its content after the rule (`padding-top: var(--space-3)`).
pub const SECTION_PAD: f32 = theme::SPACE_3;
/// The gap between a layers section's parts (`gap: 6px`).
pub const LAYERS_GAP: f32 = 6.0;
/// The gap between a settings section's parts (`gap: var(--space-3)`).
pub const SETTINGS_GAP: f32 = theme::SPACE_3;
/// A heading's margin below it: 4 in Layers, −4 in Settings.
pub const LAYERS_HEADING_MARGIN: f32 = theme::SPACE_1;
pub const SETTINGS_HEADING_MARGIN: f32 = -theme::SPACE_1;
/// Under a segmented group that is a layers section's own child (`margin-bottom: var(--space-1)`).
pub const SEGMENTED_MARGIN: f32 = theme::SPACE_1;
/// `.rail-shows`: margins above and below, gap.
pub const RAIL_SHOWS_MARGIN_TOP: f32 = 2.0;
pub const RAIL_SHOWS_MARGIN_BOTTOM: f32 = theme::SPACE_1;
pub const RAIL_SHOWS_GAP: f32 = theme::SPACE_1;
/// The name size's `.field` gap.
pub const FIELD_GAP: f32 = theme::SPACE_1;
/// `.range`: its gap, and the note's margin above (`margin-top: calc(var(--space-1) * -1)`).
pub const RANGE_GAP: f32 = theme::SPACE_2;
pub const RANGE_NOTE_MARGIN: f32 = -theme::SPACE_1;
/// The altitude exaggeration's range (`min="1" max="40" step="1"`).
pub const EXAGGERATION: (f64, f64, f64) = (1.0, 40.0, 1.0);

/// The basemaps on offer (`BASEMAPS`): value, label, key.
pub const BASEMAPS: [(Basemap, &str, &str); 3] = [(Basemap::Dark, "Dark", "1"), (Basemap::Satellite, "Satellite", "2"), (Basemap::Realistic, "Realistic", "3")];
/// What the rail map can show (`RAIL_SHOWS`).
pub const RAIL_SHOWS: [(RailShows, &str); 2] = [(RailShows::Speeds, "Speed limits"), (RailShows::Blocks, "Signal blocks")];
/// The realistic map's weather (`SKIES`).
pub const SKIES: [(Sky, &str); 6] = [(Sky::Live, "Live"), (Sky::Sunny, "Sun"), (Sky::Cloudy, "Clouds"), (Sky::Rain, "Rain"), (Sky::Fog, "Fog"), (Sky::Snow, "Snow")];
/// Place-name sizes on offer, as multiples of the default (`LABEL_SIZES`).
pub const LABEL_SIZES: [(f64, &str); 4] = [(0.85, "Small"), (1.0, "Medium"), (1.25, "Large"), (1.5, "Extra large")];

/// The quiet line under the rail map's choice.
pub fn rail_shows_note(shows: RailShows) -> &'static str {
    match shows {
        RailShows::Speeds => "Each stretch coloured by its limit",
        RailShows::Blocks => "Free or taken, and by which train; at street level",
    }
}

/// The exaggeration's value as the label shows it (`×{exaggeration}`).
pub fn exaggeration_text(exaggeration: f64) -> String {
    format!("×{}", js_string(exaggeration))
}

/// A section heading's style (`h3`: 12 px, 600, `--text-3`).
const fn heading_style() -> Style {
    Style::ui(theme::TEXT_SM, 600, theme::TEXT_3)
}

/// A quiet line's style (`small`: 12 px `--text-3`).
const fn note_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// Parts one under another with `gap` between them (a flex column).
struct Column<'u> {
    ui: &'u mut Ui,
    gap: f32,
    started: bool,
}

impl<'u> Column<'u> {
    fn new(ui: &'u mut Ui, gap: f32) -> Self {
        Column { ui, gap, started: false }
    }

    /// The next part's place: the gap when one came before.
    fn next(&mut self) -> &mut Ui {
        if self.started {
            self.ui.add_space(self.gap);
        }
        self.started = true;
        self.ui
    }
}

/// The popover's sections: the gap and the rule before each after the first.
struct Sections {
    first: bool,
}

impl Sections {
    fn new() -> Self {
        Sections { first: true }
    }

    /// A section headed `heading` (its margin below `heading_margin`), its parts `gap` apart.
    fn section(&mut self, ui: &mut Ui, heading: &str, heading_margin: f32, gap: f32, body: impl FnOnce(&mut Column<'_>)) {
        if !std::mem::replace(&mut self.first, false) {
            ui.add_space(MENU_GAP);
            rule(ui);
            ui.add_space(SECTION_PAD);
        }
        let mut column = Column::new(ui, gap);
        let ui = column.next();
        line(ui, heading, &heading_style());
        ui.add_space(heading_margin);
        body(&mut column);
    }
}

/// The 1 px `--line` rule across the width (`border-top: 1px solid var(--line)`).
fn rule(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::BORDER), Sense::hover());
    let ppp = ui.pixels_per_point();
    ui.painter().rect_filled(rect.round_to_pixels(ppp), CornerRadius::ZERO, theme::LINE);
}

/// One line of `style` across the width: its CSS line box.
fn line(ui: &mut Ui, text: &str, style: &Style) -> Rect {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, style.line_px()), Sense::hover());
    let galley = ui.painter().layout_job(text::truncated(text, style, width));
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    rect
}

/// A quiet line (`small`), wrapped across the width: its CSS line boxes.
fn note(ui: &mut Ui, text: &str) -> Rect {
    let style = note_style();
    let width = ui.available_width();
    let galley = ui.painter().layout_job(text::wrapped(text, &style, width));
    let rows = galley.rows.len().max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, rows * style.line_px()), Sense::hover());
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    rect
}

/// A switch row for a setting a key also toggles: `on` now, [`Action::Toggle`] when flipped.
fn toggle_row(ui: &mut Ui, row: ToggleRow<'_>, on: bool, what: Toggle, actions: &mut Vec<Action>) {
    let mut checked = on;
    if row.show(ui, &mut checked).changed() {
        actions.push(Action::Toggle(what));
    }
}

/// A switch row for a setting of the menus alone: `on` now, [`Action::Set`] with `set(checked)`
/// when flipped.
fn set_row(ui: &mut Ui, row: ToggleRow<'_>, on: bool, set: fn(bool) -> Setting, actions: &mut Vec<Action>) {
    let mut checked = on;
    if row.show(ui, &mut checked).changed() {
        actions.push(Action::Set(set(checked)));
    }
}

/// LayersMenu: what is drawn (see the module documentation).
pub fn layers(ui: &mut Ui, state: &AppState, actions: &mut Vec<Action>) {
    let s = &state.persisted.settings;
    let mut sections = Sections::new();
    sections.section(ui, "Map", LAYERS_HEADING_MARGIN, LAYERS_GAP, |c| {
        let basemaps = BASEMAPS.map(|(v, label, key)| Choice::new(v, label).key(key));
        let mut basemap = s.basemap;
        let ui = c.next();
        if Segmented::new("basemap", "Basemap", &basemaps).hide_legend().show(ui, &mut basemap).changed() {
            actions.push(Action::SetBasemap(basemap));
        }
        ui.add_space(SEGMENTED_MARGIN);
        if s.basemap == Basemap::Realistic {
            // live: the weather where the map is looking
            let skies = SKIES.map(|(v, label)| Choice::new(v, label));
            let mut sky = s.sky;
            let ui = c.next();
            if Segmented::new("sky", "Weather", &skies).columns(3).show(ui, &mut sky).changed() {
                actions.push(Action::Set(Setting::Sky(sky)));
            }
            ui.add_space(SEGMENTED_MARGIN);
            set_row(c.next(), ToggleRow::new("tilt-shift", "Tilt-shift").hint("Top and bottom blurred, like a miniature"), s.tilt_shift, Setting::TiltShift, actions);
        }
        toggle_row(c.next(), ToggleRow::new("terrain", "Terrain").hint("Mountains and valleys in relief").key("E"), s.terrain, Toggle::Terrain, actions);
        toggle_row(c.next(), ToggleRow::new("buildings", "3D buildings").hint("Appear at street level").key("B"), s.buildings, Toggle::Buildings, actions);
        toggle_row(c.next(), ToggleRow::new("labels", "Place names").key("L"), s.labels, Toggle::Labels, actions);
        toggle_row(c.next(), ToggleRow::new("rail-map", "Rail map").hint("Railway lines from OpenStreetMap").key("R"), s.rail_map, Toggle::RailMap, actions);
        // the speeds and the blocks both come through the app's backend
        if s.rail_map {
            let ui = c.next();
            ui.add_space(RAIL_SHOWS_MARGIN_TOP);
            let shows = RAIL_SHOWS.map(|(v, label)| Choice::new(v, label));
            let mut rail_shows = s.rail_shows;
            if Segmented::new("rail-shows", "The rail map shows", &shows).hide_legend().show(ui, &mut rail_shows).changed() {
                actions.push(Action::Set(Setting::RailShows(rail_shows)));
            }
            ui.add_space(RAIL_SHOWS_GAP);
            note(ui, rail_shows_note(s.rail_shows));
            ui.add_space(RAIL_SHOWS_MARGIN_BOTTOM);
        }
    });
    // which traffic is on the map is the mode bar's: only the aircraft have a layer of their own,
    // offered while they are on it
    if shown_kinds(s.mode, &s.also_show).aircraft {
        sections.section(ui, "Aircraft", LAYERS_HEADING_MARGIN, LAYERS_GAP, |c| {
            let row = ToggleRow::new("aircraft-labels", "Aircraft labels").hint("Callsign next to each aircraft").key("K");
            toggle_row(c.next(), row, s.aircraft_labels, Toggle::AircraftLabels, actions);
        });
    }
    sections.section(ui, "Sky", LAYERS_HEADING_MARGIN, LAYERS_GAP, |c| {
        toggle_row(c.next(), ToggleRow::new("weather", "Weather radar").hint("Rain and snow, every 10 min").key("W"), s.weather, Toggle::Weather, actions);
        toggle_row(c.next(), ToggleRow::new("daylight", "Day and night").hint("Shades where the sun has set").key("D"), s.daylight, Toggle::Daylight, actions);
    });
}

/// SettingsMenu: how the app behaves (see the module documentation). `phone`: the keys entry is
/// listed here, where the controls have no room for their own button.
pub fn settings(ui: &mut Ui, state: &AppState, actions: &mut Vec<Action>, phone: bool) {
    let s = &state.persisted.settings;
    let mut sections = Sections::new();
    sections.section(ui, "Display", SETTINGS_HEADING_MARGIN, SETTINGS_GAP, |c| {
        let ui = c.next();
        let sizes = LABEL_SIZES.map(|(v, label)| Choice::new(v, label));
        let mut size = s.label_size;
        if Segmented::new("label-size", "Place name size", &sizes).columns(2).disabled(!s.labels).show(ui, &mut size).changed() && s.labels {
            actions.push(Action::Set(Setting::LabelSize(size)));
        }
        if !s.labels {
            ui.add_space(FIELD_GAP);
            note(ui, "Place names are off in Layers");
        }
        let ui = c.next();
        // the label: the name, and the value at the right on the same line
        let row = line(ui, "Altitude exaggeration", &Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1));
        let value = ui.painter().layout_job(text::job(&exaggeration_text(s.exaggeration), &Style::data(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::AMBER)));
        ui.painter().galley(Pos2::new(row.right() - value.size().x, row.top()), value, Color32::PLACEHOLDER);
        ui.add_space(RANGE_GAP);
        let (min, max, step) = EXAGGERATION;
        let mut exaggeration = s.exaggeration;
        if Range::new("exaggeration", min, max, step).show(ui, &mut exaggeration).changed() {
            actions.push(Action::Set(Setting::Exaggeration(exaggeration)));
        }
        ui.add_space(RANGE_GAP + RANGE_NOTE_MARGIN);
        note(ui, "Applies when zoomed out; true scale up close");
    });
    sections.section(ui, "Data", SETTINGS_HEADING_MARGIN, SETTINGS_GAP, |c| {
        let row = ToggleRow::new("adsb", "Aircraft-reported data").hint("Heading, bank and autopilot, from adsb.lol");
        set_row(c.next(), row, s.adsb, Setting::Adsb, actions);
        if phone {
            let ui = c.next();
            let keys = Button::new().icon(Icon::KeyRound.at(14.0, icons::STROKE_THIN)).text("API keys and Flightradar24 login");
            if ui.horizontal(|ui| keys.show(ui)).inner.clicked() {
                actions.push(Action::SetMenu(None));
                actions.push(Action::Open(Open::Keys, true));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use egui::{Context, Event, PointerButton, RawInput};
    use rustradar_core::settings::Mode;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::state::Startup;

    fn state() -> AppState {
        AppState::new(Startup::default(), None, None, None, ViewState::INITIAL, [1440.0, 900.0])
    }

    /// One pass of `menu` in a 300 px column at (0, 0) with `events`: the actions it asked for.
    fn pass(ctx: &Context, state: &AppState, events: Vec<Event>, menu: fn(&mut Ui, &AppState, &mut Vec<Action>)) -> Vec<Action> {
        let mut actions = Vec::new();
        let raw = RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            egui::Area::new(egui::Id::unique("menu")).fixed_pos(Pos2::ZERO).show(ui.ctx(), |ui| {
                ui.set_width(300.0);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                menu(ui, state, &mut actions);
            });
        });
        out.textures_delta.clear();
        actions
    }

    /// Clicks `at` in `menu` (after a pass that lays it out: the pointer moves there, presses and
    /// lets go, each in a pass of its own) and applies what it asked for.
    fn click_and_apply(ctx: &Context, s: &mut AppState, at: Pos2, menu: fn(&mut Ui, &AppState, &mut Vec<Action>)) -> Vec<Action> {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        pass(ctx, s, Vec::new(), menu);
        let mut actions = pass(ctx, s, vec![Event::PointerMoved(at)], menu);
        actions.extend(pass(ctx, s, vec![button(true)], menu));
        actions.extend(pass(ctx, s, vec![button(false)], menu));
        // the pointer leaves, so the next click starts afresh
        actions.extend(pass(ctx, s, vec![Event::PointerGone], menu));
        for &a in &actions {
            s.apply(a, 0.0);
        }
        actions
    }

    fn layers_menu(ui: &mut Ui, s: &AppState, actions: &mut Vec<Action>) {
        layers(ui, s, actions);
    }

    fn settings_menu(ui: &mut Ui, s: &AppState, actions: &mut Vec<Action>) {
        settings(ui, s, actions, false);
    }

    fn settings_phone(ui: &mut Ui, s: &AppState, actions: &mut Vec<Action>) {
        settings(ui, s, actions, true);
    }

    // Chromium's layout of today's LayersMenu, relative to the popover's content (left 1061, top
    // 414.52 at 1440 × 900): the basemap at 26.79, rows of 34.98 (32 without a hint) 6 apart from
    // 70.98; with the realistic map and the rail map on (top 205.98) the weather's last row from
    // 130.36 and the rail map's choice at 375.49.
    const LAYERS_TOP: f32 = 414.52;

    #[test]
    fn the_layers_menu_s_toggles_reach_the_settings() {
        let ctx = Context::default();
        super::theme::apply(&ctx);
        let mut s = state();
        assert!(!s.persisted.settings.terrain && s.persisted.settings.buildings);
        // Terrain's row (Chromium: 485.5 − 414.52), anywhere on it
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(40.0, 485.5 - LAYERS_TOP + 17.0), layers_menu);
        assert_eq!(actions, [Action::Toggle(Toggle::Terrain)]);
        assert!(s.persisted.settings.terrain);
        // 3D buildings' switch
        click_and_apply(&ctx, &mut s, Pos2::new(283.0, 526.48 - LAYERS_TOP + 17.0), layers_menu);
        assert!(!s.persisted.settings.buildings);
        // Place names, Rail map
        click_and_apply(&ctx, &mut s, Pos2::new(40.0, 567.47 - LAYERS_TOP + 16.0), layers_menu);
        assert!(!s.persisted.settings.labels);
        click_and_apply(&ctx, &mut s, Pos2::new(40.0, 605.47 - LAYERS_TOP + 17.0), layers_menu);
        assert!(s.persisted.settings.rail_map);
        // the rail map's choice shows under it, 6 + 2 below its row: Signal blocks (Chromium:
        // from 1212 − 1061 across)
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(1212.0 - 1061.0 + 40.0, 605.47 + 34.98 + 6.0 + 2.0 - LAYERS_TOP + 14.0), layers_menu);
        assert_eq!(actions, [Action::Set(Setting::RailShows(RailShows::Blocks))]);
        assert_eq!(s.persisted.settings.rail_shows, RailShows::Blocks);
    }

    #[test]
    fn the_basemap_and_the_realistic_map_s_own_rows() {
        let ctx = Context::default();
        super::theme::apply(&ctx);
        let mut s = state();
        // Satellite (Chromium: 1162.66 − 1061, 441.31 − 414.52)
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(150.0, 441.31 - LAYERS_TOP + 17.0), layers_menu);
        assert_eq!(actions, [Action::SetBasemap(Basemap::Satellite)]);
        assert_eq!(s.persisted.settings.basemap, Basemap::Satellite);
        // Realistic: the weather and Tilt-shift come in
        click_and_apply(&ctx, &mut s, Pos2::new(250.0, 441.31 - LAYERS_TOP + 17.0), layers_menu);
        assert_eq!(s.persisted.settings.basemap, Basemap::Realistic);
        // Snow: the weather's last option (Chromium: 1261.33 − 1061, 336.34 − 205.98)
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(250.0, 336.34 - 205.98 + 14.0), layers_menu);
        assert_eq!(actions, [Action::Set(Setting::Sky(Sky::Snow))]);
        assert_eq!(s.persisted.settings.sky, Sky::Snow);
        // Tilt-shift (Chromium: 377.53 − 205.98)
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(40.0, 377.53 - 205.98 + 17.0), layers_menu);
        assert_eq!(actions, [Action::Set(Setting::TiltShift(true))]);
        assert!(s.persisted.settings.tilt_shift);
    }

    #[test]
    fn the_aircraft_section_only_while_aircraft_are_on_the_map() {
        let ctx = Context::default();
        super::theme::apply(&ctx);
        let mut s = state();
        // Aircraft labels (Chromium: 692.25 − 414.52)
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(40.0, 692.25 - LAYERS_TOP + 17.0), layers_menu);
        assert_eq!(actions, [Action::Toggle(Toggle::AircraftLabels)]);
        assert!(s.persisted.settings.aircraft_labels);
        // ships the focus without the aircraft: the Sky section's first row moves up into its place
        // (Chromium: Weather radar at 692.25 there)
        s.persisted.settings.mode = Mode::Ships;
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(40.0, 692.25 - LAYERS_TOP + 17.0), layers_menu);
        assert_eq!(actions, [Action::Toggle(Toggle::Weather)]);
        assert!(s.persisted.settings.weather);
        // Day and night under it
        click_and_apply(&ctx, &mut s, Pos2::new(40.0, 692.25 + 40.98 - LAYERS_TOP + 17.0), layers_menu);
        assert!(!s.persisted.settings.daylight);
    }

    // Chromium's layout of today's SettingsMenu relative to its content (top 579.88): the name
    // size's options from 54 (2 rows 30.19 apart), the slider at 153.54, the Data row at 240.14.
    const SETTINGS_TOP: f32 = 579.88;

    #[test]
    fn the_settings_menu_s_choices_reach_the_settings() {
        let ctx = Context::default();
        super::theme::apply(&ctx);
        let mut s = state();
        // Large (Chromium: 1064 − 1061, 664.05 − 579.88)
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(60.0, 664.05 - SETTINGS_TOP + 14.0), settings_menu);
        assert_eq!(actions, [Action::Set(Setting::LabelSize(1.25))]);
        assert_eq!(s.persisted.settings.label_size, 1.25);
        // the slider's right end: 40
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(299.0, 733.42 - SETTINGS_TOP + 8.0), settings_menu);
        assert_eq!(actions, [Action::Set(Setting::Exaggeration(40.0))]);
        assert_eq!(s.persisted.settings.exaggeration, 40.0);
        // and its left end: 1
        click_and_apply(&ctx, &mut s, Pos2::new(1.0, 733.42 - SETTINGS_TOP + 8.0), settings_menu);
        assert_eq!(s.persisted.settings.exaggeration, 1.0);
        // Aircraft-reported data (Chromium: 820.02 − 579.88)
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(40.0, 820.02 - SETTINGS_TOP + 17.0), settings_menu);
        assert_eq!(actions, [Action::Set(Setting::Adsb(false))]);
        assert!(!s.persisted.settings.adsb);
    }

    #[test]
    fn the_name_size_is_inert_while_place_names_are_off() {
        let ctx = Context::default();
        super::theme::apply(&ctx);
        let mut s = state();
        s.persisted.settings.labels = false;
        // Large stays where it was (Chromium: 643.25 − 559.08 with the note)
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(60.0, 643.25 - 559.08 + 14.0), settings_menu);
        assert!(actions.is_empty(), "{actions:?}");
        assert_eq!(s.persisted.settings.label_size, 1.0);
    }

    #[test]
    fn the_keys_entry_on_phones() {
        let ctx = Context::default();
        super::theme::apply(&ctx);
        let mut s = state();
        s.set_menu(Some(crate::state::shortcuts::Menu::Settings));
        // under the Data row: 240.14 + 34.98 + 12
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(40.0, 240.14 + 34.98 + 12.0 + 16.0), settings_phone);
        assert_eq!(actions, [Action::SetMenu(None), Action::Open(Open::Keys, true)]);
        assert!(s.shortcuts.keys_open());
        assert_eq!(s.shortcuts.menu(), None);
        // not on the desktop
        let actions = click_and_apply(&ctx, &mut s, Pos2::new(40.0, 240.14 + 34.98 + 12.0 + 16.0), settings_menu);
        assert!(actions.is_empty());
    }

    #[test]
    fn texts_as_today() {
        assert_eq!(exaggeration_text(10.0), "×10");
        assert_eq!(exaggeration_text(1.0), "×1");
        assert_eq!(rail_shows_note(RailShows::Speeds), "Each stretch coloured by its limit");
        assert_eq!(rail_shows_note(RailShows::Blocks), "Free or taken, and by which train; at street level");
    }
}
