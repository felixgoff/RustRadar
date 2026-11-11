//! TrainFilterPanel, the train filters (doc 06 §12; `TrainFilterPanel.svelte`, `FilterChips.svelte`,
//! `ToggleRow.svelte`, `train-list.ts`): the sidebar's filter panel while trains are the focus and
//! the filters are open ([`super::super::shell::Slots::train_filters`]).
//!
//! **Header** (the three filter panels', [`super::aircraft`]'s constants: 49 px): `Filters`,
//! `{shown} of {total} shown` (the filtered trains that still run, of everything loaded), `Reset`
//! (disabled while no group narrows the trains: [`train_filter_count`]) and the close button
//! (`Close filters (F)`).
//!
//! **Body** (padding 16, its groups 16 apart, scrolling within the height the sidebar gives):
//! 1. FilterChips `Kind` ([`kind_options`]): one chip per kind of `TRAIN_KINDS` with its label,
//!    its map colour as the swatch and how many loaded trains are of it (none shown for 0);
//! 2. FilterChips `Operator` ([`operator_options`]): the operators of the loaded trains, most
//!    trains first and the unknown one (`""`, `Unknown`, no swatch) last, then the ones left out
//!    that run no train now (count 0, so they can be put back); each operator's swatch is the
//!    colour a train of the kind `other` gets from it; the hint `Each operator's trains come live
//!    on their own stream; one left out isn't streamed.`;
//! 3. ToggleRow `Delayed only` with the hint `5 min or more late, as the operator reports`.
//!
//! **Changes** reach the state as one [`Action::SetTrainFilters`]: the panel edits a copy of
//! `persisted.train_filters` as `bind:filters` did and hands it over
//! ([`AppState::train_filters_handed`]) when it differs at the end of the pass. Reset empties the
//! kinds and operators left out and turns Delayed only back to its default, keeping the sort; the
//! close button is `Action::Open(Open::Filters, false)`. The train state sends the operators left
//! out to the backend (`hideTrainOperators`) at its next poll.
//!
//! **Checked** against Chromium's layout of today's component, server-rendered by Svelte with the
//! same props and laid out with app.css at 1440 × 900 (a kind and an operator left out, one no
//! longer running, an unknown operator; `tests/train_ui.rs`): every box, rule, chip, swatch,
//! switch and button lands on Chromium's pixels or within one; text sits up to a pixel lower
//! (egui's baseline).
//!
//! The card's content ends on a whole pixel below the height the sidebar solves (at most a pixel
//! further): egui's debug builds mark a fractional edge in orange.
//!
//! Deviations: `tabular-nums` as in the shell (U6); text takes egui's baseline (U1).

use egui::{Color32, Frame, Id, Margin, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, UiBuilder, Vec2};
use rustradar_core::model::{Train, TrainKind, TrainSource};
use rustradar_core::train_list::{DEFAULT_TRAIN_FILTERS, TrainFilters, train_filter, train_filter_count, train_operators};
use rustradar_core::trains::{TRAIN_KINDS, train_color, train_color_of, train_label, train_running};

use super::aircraft::{CLOSE, CLOSE_TOOLTIP, GROUP_GAP, HEADER_GAP, HEADER_HEIGHT, HEADER_PAD, HEADER_PAD_LEFT, TITLE, shown_text, shown_style, title_style};
use crate::state::shortcuts::Open;
use crate::state::{Action, AppState};
use crate::ui::icons::Icon;
use crate::ui::lists::trains::color;
use crate::ui::shell::Pass;
use crate::ui::text::{self, Style};
use crate::ui::theme;
use crate::ui::widgets::chips::{self, ChipOption, FilterChips};
use crate::ui::widgets::icon_button::{self, IconButton};
use crate::ui::widgets::toggle_row::ToggleRow;

/// The body's padding.
pub const BODY_PAD: f32 = theme::SPACE_4;
/// The operators' hint.
pub const OPERATOR_HINT: &str = "Each operator's trains come live on their own stream; one left out isn't streamed.";
/// The delayed trains' row and its hint.
pub const DELAYED_ONLY: &str = "Delayed only";
pub const DELAYED_HINT: &str = "5 min or more late, as the operator reports";
/// The unknown operator's chip.
pub const UNKNOWN: &str = "Unknown";

/// A kind's chip: the kind, its label, its colour on the map, how many loaded trains are of it.
#[derive(Debug, Clone, PartialEq)]
pub struct KindOption {
    pub kind: TrainKind,
    pub label: &'static str,
    pub color: Color32,
    pub count: usize,
}

/// The kinds' chips over the loaded `trains` (`kinds`), in `TRAIN_KINDS`' order.
pub fn kind_options(trains: &[Train]) -> Vec<KindOption> {
    TRAIN_KINDS
        .iter()
        .map(|&kind| KindOption { kind, label: train_label(kind), color: color(train_color(kind)), count: trains.iter().filter(|t| t.kind == kind).count() })
        .collect()
}

/// An operator's chip: its name as trains give it (`""` for unknown), its label, its colour (none
/// for unknown), how many loaded trains it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct OperatorOption {
    pub name: String,
    pub label: String,
    pub color: Option<Color32>,
    pub count: usize,
}

/// The colour a train of the kind `other` run by `operator` is drawn in
/// (`trainColorOf({ kind: "other", operator })`).
pub fn operator_color(operator: &str) -> Color32 {
    let train = Train {
        id: String::new(),
        source: TrainSource::Trafikverket,
        name: String::new(),
        kind: TrainKind::Other,
        lat: 0.0,
        lon: 0.0,
        heading: None,
        speed_kmh: None,
        destination: None,
        operator: Some(operator.to_owned()),
        delay_minutes: None,
        position_unix: 0,
        frames: Vec::new(),
        reports: Vec::new(),
        predicted: Vec::new(),
        cars: None,
        tail: Vec::new(),
        structures: Vec::new(),
        ended_unix: None,
        shape: None,
        model: None,
    };
    color(train_color_of(&train))
}

/// The operators' chips (`operators`): those of the loaded `trains` (`trainOperators`), then those
/// left out in `hidden` that run none of them.
pub fn operator_options(trains: &[Train], hidden: &[String]) -> Vec<OperatorOption> {
    let running = train_operators(trains);
    let gone: Vec<&String> = hidden.iter().filter(|o| !running.iter().any(|r| &r.name == *o)).collect();
    running
        .into_iter()
        .map(|o| (o.name, o.count))
        .chain(gone.into_iter().map(|name| (name.clone(), 0)))
        .map(|(name, count)| OperatorOption {
            label: if name.is_empty() { UNKNOWN.to_owned() } else { name.clone() },
            color: (!name.is_empty()).then(|| operator_color(&name)),
            name,
            count,
        })
        .collect()
}

/// Reset (`reset`): no kind or operator left out, Delayed only as by default, the sort kept.
pub fn reset(filters: &TrainFilters) -> TrainFilters {
    TrainFilters { hidden_kinds: Vec::new(), hidden_operators: Vec::new(), delayed_only: DEFAULT_TRAIN_FILTERS.delayed_only, sort: filters.sort }
}

/// The panel in the sidebar (the shell's [`FilterSlot`](super::super::shell::FilterSlot)): draws
/// within the height `ui` allows and answers its natural height, header and whole body.
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) -> f32 {
    let state = pass.state;
    let mut filters = state.persisted.train_filters.clone();
    let available = ui.available_height();
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let header = header(ui, pass, &mut filters);
    let body_max = (available - header).max(0.0);
    let out = ScrollArea::vertical().id_salt("rustradar::filters::trains::body").max_height(body_max).auto_shrink([false, true]).show(ui, |ui| {
        Frame::NONE.inner_margin(Margin::same(BODY_PAD as i8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            body(ui, state, &mut filters);
        });
    });
    if filters != state.persisted.train_filters {
        state.train_filters_handed.set(Some(filters));
        pass.actions.push(Action::SetTrainFilters);
    }
    // the card ends on a whole pixel (the height the sidebar solves may not: egui's debug builds
    // flag a fractional edge), as Chromium snaps a box's edges
    let bottom = ui.max_rect().bottom().ceil();
    ui.expand_to_include_y(bottom);
    header + out.content_size.y
}

/// The header (as the aircraft filters'); answers its height.
fn header(ui: &mut Ui, pass: &mut Pass<'_>, filters: &mut TrainFilters) -> f32 {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, HEADER_HEIGHT), Sense::hover());
    let row = Rect::from_min_max(
        Pos2::new(rect.left() + HEADER_PAD_LEFT, rect.top() + HEADER_PAD),
        Pos2::new(rect.right() - HEADER_PAD, rect.top() + HEADER_PAD + icon_button::SIZE),
    );
    let middle = row.center().y;
    let close = Rect::from_min_size(Pos2::new(row.right() - icon_button::SIZE, row.top()), Vec2::splat(icon_button::SIZE));
    let closed = IconButton::of(Icon::X, 16.0, CLOSE).tooltip(CLOSE_TOOLTIP).show(&mut ui.new_child(UiBuilder::new().max_rect(close))).clicked();
    let reset_label = "Reset";
    let link = Style::ui(theme::TEXT_SM, 600, theme::AMBER);
    let reset_w = text::measure(ui.ctx(), text::job(reset_label, &link)).x + 2.0 * chips::LINK_PAD.x;
    let reset_h = link.line_px() + 2.0 * chips::LINK_PAD.y;
    let reset_rect = Rect::from_min_size(Pos2::new(close.left() - HEADER_GAP - reset_w, middle - reset_h / 2.0), Vec2::new(reset_w, reset_h));
    let active = train_filter_count(filters) > 0;
    let reset_clicked = chips::text_button(&mut ui.new_child(UiBuilder::new().max_rect(reset_rect)), Id::unique("rustradar::filters::trains::reset"), reset_label, active).clicked();

    let state = pass.state;
    let painter = ui.painter();
    let title = painter.layout_job(text::job(TITLE, &title_style()));
    let title_w = title.size().x;
    painter.galley(Pos2::new(row.left(), middle - title.size().y / 2.0), title, Color32::PLACEHOLDER);
    let keep = train_filter(filters);
    let now = pass.now_ms as i64;
    let trains = state.trains.trains();
    let shown = trains.iter().filter(|t| keep(t) && train_running(t, now)).count();
    let shown = painter.layout_job(text::job(&shown_text(shown, trains.len()), &shown_style()));
    painter.galley(Pos2::new(row.left() + title_w + HEADER_GAP, middle - shown.size().y / 2.0), shown, Color32::PLACEHOLDER);
    painter.hline(rect.x_range(), rect.bottom() - theme::BORDER / 2.0, Stroke::new(theme::BORDER, theme::LINE));

    if reset_clicked {
        *filters = reset(filters);
    }
    if closed {
        pass.actions.push(Action::Open(Open::Filters, false));
    }
    HEADER_HEIGHT
}

/// The body's groups.
fn body(ui: &mut Ui, state: &AppState, filters: &mut TrainFilters) {
    let trains = state.trains.trains();
    // 1. the kinds
    let kinds = kind_options(trains);
    let options: Vec<ChipOption<'_, TrainKind>> = kinds
        .iter()
        .map(|k| {
            let o = ChipOption::new(k.kind, k.label).color(k.color);
            if k.count > 0 { o.count(k.count as u64) } else { o }
        })
        .collect();
    FilterChips::new("rustradar::filters::trains::kinds", "Kind", &options).show(ui, &mut filters.hidden_kinds);
    ui.add_space(GROUP_GAP);

    // 2. the operators, by the names trains give them
    let operators = operator_options(trains, &filters.hidden_operators);
    let options: Vec<ChipOption<'_, &str>> = operators
        .iter()
        .map(|o| {
            let mut c = ChipOption::new(o.name.as_str(), o.label.as_str());
            if let Some(color) = o.color {
                c = c.color(color);
            }
            if o.count > 0 { c.count(o.count as u64) } else { c }
        })
        .collect();
    let before = filters.hidden_operators.clone();
    let mut hidden: Vec<&str> = before.iter().map(String::as_str).collect();
    let changed = FilterChips::new("rustradar::filters::trains::operators", "Operator", &options).hint(OPERATOR_HINT).show(ui, &mut hidden).changed();
    if changed {
        filters.hidden_operators = hidden.into_iter().map(str::to_owned).collect();
    }
    ui.add_space(GROUP_GAP);

    // 3. the delayed ones only
    ToggleRow::new("rustradar::filters::trains::delayed", DELAYED_ONLY).hint(DELAYED_HINT).show(ui, &mut filters.delayed_only);
}

#[cfg(test)]
mod tests {
    use egui::{Context, Event, PointerButton, RawInput};
    use rustradar_core::settings::Mode;
    use rustradar_core::train_list::TrainSort;
    use rustradar_scene::camera::ViewState;

    use super::*;
    use crate::dev::demo_trains;
    use crate::persist::tests::TempDir;
    use crate::state::Startup;
    use crate::state::trains::FIXTURE_TRAINS;
    use crate::ui::shell::{self, Slots};

    const T0: f64 = 1_782_036_000_000.0;
    const V3: ViewState = ViewState { longitude: 18.07, latitude: 59.33, zoom: 9.0, bearing: 0.0, pitch: 0.0, position: None };

    fn trains() -> Vec<Train> {
        let mut trains = demo_trains(V3.latitude, V3.longitude, T0 as i64);
        trains[5].operator = None;
        trains
    }

    #[test]
    fn the_chips_counts_and_colours() {
        let trains = trains();
        let kinds = kind_options(&trains);
        let shown: Vec<(&str, usize)> = kinds.iter().map(|k| (k.label, k.count)).collect();
        assert_eq!(shown, [("High-speed", 1), ("Intercity", 2), ("Regional", 3), ("Suburban", 0), ("Other", 0)]);
        assert_eq!(kinds[0].color, Color32::from_rgb(226, 92, 196));
        // most trains first, the unknown operator last, then those left out that run nothing
        let operators = operator_options(&trains, &["Arriva".to_owned(), "SJ".to_owned()]);
        let shown: Vec<(&str, &str, usize)> = operators.iter().map(|o| (o.name.as_str(), o.label.as_str(), o.count)).collect();
        assert_eq!(shown, [("SJ", "SJ", 2), ("Öresundståg", "Öresundståg", 1), ("Skånetrafiken", "Skånetrafiken", 1), ("Snälltåget", "Snälltåget", 1), ("", "Unknown", 1), ("Arriva", "Arriva", 0)]);
        // an operator's own colour, or its kind's (other) without one; none for the unknown
        assert_eq!(operators[0].color, Some(Color32::from_rgb(232, 232, 236)));
        assert_eq!(operators[5].color, Some(Color32::from_rgb(158, 150, 140)));
        assert_eq!(operators[4].color, None);
        // Reset keeps the sort
        let set = TrainFilters { hidden_kinds: vec![TrainKind::Regional], hidden_operators: vec!["SJ".into()], delayed_only: true, sort: TrainSort::Speed };
        assert_eq!(reset(&set), TrainFilters { sort: TrainSort::Speed, ..DEFAULT_TRAIN_FILTERS });
    }

    /// One pass of the shell over `events`: the actions.
    fn pass(ctx: &Context, state: &AppState, time: f64, events: Vec<Event>) -> Vec<crate::state::Action> {
        let mut actions = Vec::new();
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1200.0))), time: Some(time), events, ..Default::default() };
        let mut output = ctx.run_ui(raw, |ui| {
            shell::show(ui.ctx(), state, T0, &mut actions, &Slots::APP);
        });
        output.textures_delta.clear();
        actions
    }

    fn click(ctx: &Context, state: &AppState, at: Pos2, time: f64) -> Vec<crate::state::Action> {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
        let mut actions = pass(ctx, state, time, vec![Event::PointerMoved(at)]);
        actions.extend(pass(ctx, state, time + 0.05, vec![button(true)]));
        actions.extend(pass(ctx, state, time + 0.1, vec![button(false)]));
        actions
    }

    #[test]
    fn a_chip_reset_and_close_reach_the_state() {
        let dir = TempDir::new("train-filters");
        std::fs::write(dir.0.join(FIXTURE_TRAINS), serde_json::to_vec(&trains()).unwrap()).unwrap();
        let startup = Startup { fixture: Some(dir.0.clone()), ..Default::default() };
        let mut s = AppState::new(startup, None, None, None, V3, [1440.0, 1200.0]);
        s.persisted.settings.mode = Mode::Trains;
        for t in [T0, T0 + 16.0, T0 + 32.0] {
            s.poll(t);
        }
        assert!(s.apply(Action::Open(Open::Filters, true), T0 + 40.0));
        let ctx = Context::default();
        theme::apply(&ctx);
        for time in [0.0, 1.0, 2.0, 3.0] {
            pass(&ctx, &s, time, Vec::new());
        }
        let card = ctx.memory(|m| m.area_rect(Id::unique(("rustradar::sidebar", "filters")))).expect("the filter panel");
        // Chromium: the first chip (High-speed) at 16 + 1 in, 94.8 under the card's top
        let chip = Pos2::new(card.left() + 17.0 + 40.0, card.top() + 94.8 + 13.0);
        let actions = click(&ctx, &s, chip, 4.0);
        assert_eq!(actions, [Action::SetTrainFilters]);
        assert!(s.apply(Action::SetTrainFilters, T0 + 50.0));
        assert_eq!(s.persisted.train_filters.hidden_kinds, [TrainKind::HighSpeed]);
        // Reset: left of the close button in the header's row
        let reset = Pos2::new(card.right() - 1.0 - 8.0 - 32.0 - 8.0 - 19.0, card.top() + 1.0 + 8.0 + 16.0);
        let actions = click(&ctx, &s, reset, 5.0);
        assert_eq!(actions, [Action::SetTrainFilters]);
        assert!(s.apply(Action::SetTrainFilters, T0 + 60.0));
        assert_eq!(s.persisted.train_filters, DEFAULT_TRAIN_FILTERS);
        // the close button
        let close = Pos2::new(card.right() - 1.0 - 8.0 - 16.0, card.top() + 1.0 + 8.0 + 16.0);
        let actions = click(&ctx, &s, close, 6.0);
        assert_eq!(actions, [Action::Open(Open::Filters, false)]);
    }
}
