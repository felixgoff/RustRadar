//! ShipFilterPanel, the ship filters (doc 06 §13; `ShipFilterPanel.svelte`, `FilterChips.svelte`,
//! `Segmented.svelte`, `ToggleRow.svelte`, `ship-list.ts`): the sidebar's filter panel while ships
//! are the focus and the filters are open ([`super::super::shell::Slots::ship_filters`]).
//!
//! **Header** (the three filter panels', [`super::aircraft`]'s constants: 49 px): `Filters`,
//! `{shown} of {total} shown` (the ships the filters keep, of everything loaded), `Reset`
//! (disabled while no group narrows the ships: [`ship_filter_count`]) and the close button
//! (`Close filters (F)`).
//!
//! **Body** (padding 16, its groups 16 apart, scrolling within the height the sidebar gives):
//! 1. FilterChips `Type` ([`kind_options`]): one chip per colour key of `SHIP_COLOR_KEYS` but the
//!    unknown, then the unknown (key 0, which also holds every unlisted type), each with its short
//!    name (`shipKindShort`), its map colour as the swatch and how many loaded ships are of it
//!    (none shown for 0);
//! 2. `Movement` (12 px at 600 `--text-2`), 8 px over the segmented `All`, `Under way`, `Moored`
//!    without its legend, 8 px over the hint `Moored includes ships reporting no heading.` (12 px
//!    `--text-3`);
//! 3. FilterChips `Size` ([`size_options`]): `Small`, `Medium`, `Large` with their counts where the
//!    feed tells sizes apart ([`sizes_known`]); else disabled, without counts, with the hint `Zoom
//!    in to tell sizes apart: zoomed out, VesselFinder lists every ship as medium.`;
//! 4. ToggleRow `Hide old positions` with the hint `Ships that haven't reported lately`.
//!
//! **Changes** reach the state as one [`Action::SetShipFilters`]: the panel edits a copy of
//! `persisted.ship_filters` as `bind:filters` did and hands it over
//! ([`AppState::ship_filters_handed`]) when it differs at the end of the pass. Reset restores
//! `DEFAULT_SHIP_FILTERS` keeping the sort; the close button is `Action::Open(Open::Filters,
//! false)`. The ship state filters the list the map draws again at its next poll.
//!
//! **Checked** against Chromium's layout of today's component with the same props
//! (`tests/ship_ui.rs`): the header, the chips' rows, the movement group, the disabled size group
//! and its hint, the switch land on Chromium's pixels or within one.
//!
//! The count's `tabular-nums` is Overpass's tabular figures (`ui::text::tabular`, F1).

use egui::{Color32, Frame, Id, Margin, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, UiBuilder, Vec2};
use rustradar_core::model::Ship;
use rustradar_core::ship_list::{DEFAULT_SHIP_FILTERS, SHIP_SIZES, ShipFilters, ShipMotion, ShipSize, ship_filter, ship_filter_count, ship_kind_short};
use rustradar_core::ships::{SHIP_COLOR_KEYS, ship_color, ship_kind_key};

use super::aircraft::{CLOSE, CLOSE_TOOLTIP, GROUP_GAP, HEADER_GAP, HEADER_HEIGHT, HEADER_PAD, HEADER_PAD_LEFT, LEGEND_GAP, TITLE, label_style, shown_style, shown_text, title_style};
use crate::state::shortcuts::Open;
use crate::state::ships::sizes_known;
use crate::state::{Action, AppState};
use crate::ui::icons::Icon;
use crate::ui::lists::ships::{color, paragraph};
use crate::ui::shell::Pass;
use crate::ui::text::{self, Style};
use crate::ui::theme;
use crate::ui::widgets::chips::{self, ChipOption, FilterChips};
use crate::ui::widgets::icon_button::{self, IconButton};
use crate::ui::widgets::segmented::{Choice, Segmented};
use crate::ui::widgets::toggle_row::ToggleRow;

/// The body's padding.
pub const BODY_PAD: f32 = theme::SPACE_4;
/// The groups' labels.
pub const TYPE: &str = "Type";
pub const MOVEMENT: &str = "Movement";
pub const SIZE: &str = "Size";
/// The movements, as the segmented shows them.
pub const MOTIONS: [(ShipMotion, &str); 3] = [(ShipMotion::All, "All"), (ShipMotion::Moving, "Under way"), (ShipMotion::Moored, "Moored")];
/// The movement's hint.
pub const MOVEMENT_HINT: &str = "Moored includes ships reporting no heading.";
/// The sizes' hint while the feed does not tell them apart.
pub const SIZES_HINT: &str = "Zoom in to tell sizes apart: zoomed out, VesselFinder lists every ship as medium.";
/// The old positions' row and its hint.
pub const HIDE_OLD: &str = "Hide old positions";
pub const HIDE_OLD_HINT: &str = "Ships that haven't reported lately";

/// The movement's hint (`.hint`: 12 px `--text-3`).
pub const fn hint_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// A type's chip: its colour key, its short name, its colour on the map, how many loaded ships
/// are of it.
#[derive(Debug, Clone, PartialEq)]
pub struct KindOption {
    pub key: u8,
    pub label: &'static str,
    pub color: Color32,
    pub count: usize,
}

/// The types' chips over the loaded `ships` (`kinds`): the known types in `SHIP_COLOR_KEYS`'
/// order, the unknown last; a ship counts under its colour key (`shipKindKey`).
pub fn kind_options(ships: &[Ship]) -> Vec<KindOption> {
    SHIP_COLOR_KEYS
        .iter()
        .copied()
        .filter(|&k| k != 0)
        .chain([0])
        .map(|key| KindOption {
            key,
            label: ship_kind_short(key),
            color: color(ship_color(key)),
            count: ships.iter().filter(|s| ship_kind_key(s.kind) == key).count(),
        })
        .collect()
}

/// The sizes' chips over the loaded `ships` (`sizes`): each class with its count where the feed
/// tells sizes apart (`count: sizesKnown ? n.get(size) : undefined`), else none.
pub fn size_options(ships: &[Ship], sizes_known: bool) -> Vec<(ShipSize, &'static str, Option<usize>)> {
    SHIP_SIZES.iter().map(|&(size, label)| (size, label, sizes_known.then(|| ships.iter().filter(|s| s.size == size).count()))).collect()
}

/// Reset (`reset`): the defaults, the sort kept.
pub fn reset(filters: &ShipFilters) -> ShipFilters {
    ShipFilters { sort: filters.sort, ..DEFAULT_SHIP_FILTERS }
}

/// How many loaded ships `filters` keep (`shown`: `shipsFiltered.length`).
pub fn shown(ships: &[Ship], filters: &ShipFilters) -> usize {
    let keep = ship_filter(filters);
    ships.iter().filter(|s| keep(s)).count()
}

/// A chip's count: shown unless 0 or not known (`{#if count}`).
fn counted<T: Copy>(option: ChipOption<'_, T>, count: Option<usize>) -> ChipOption<'_, T> {
    match count {
        Some(n) if n > 0 => option.count(n as u64),
        _ => option,
    }
}

/// The panel in the sidebar (the shell's [`FilterSlot`](super::super::shell::FilterSlot)): draws
/// within the height `ui` allows and answers its natural height, header and whole body.
pub fn show(ui: &mut Ui, pass: &mut Pass<'_>) -> f32 {
    let state = pass.state;
    let mut filters = state.persisted.ship_filters.clone();
    let available = ui.available_height();
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    let header = header(ui, pass, &mut filters);
    let body_max = (available - header).max(0.0);
    let out = ScrollArea::vertical().id_salt("rustradar::filters::ships::body").max_height(body_max).auto_shrink([false, true]).show(ui, |ui| {
        Frame::NONE.inner_margin(Margin::same(BODY_PAD as i8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            body(ui, state, &mut filters);
        });
    });
    if filters != state.persisted.ship_filters {
        state.ship_filters_handed.set(Some(filters));
        pass.actions.push(Action::SetShipFilters);
    }
    // the card ends on a whole pixel, as Chromium snaps a box's edges (the height the sidebar
    // solves may not; egui's debug builds flag a fractional edge)
    let bottom = ui.max_rect().bottom().ceil();
    ui.expand_to_include_y(bottom);
    header + out.content_size.y
}

/// The header (as the aircraft filters'); answers its height.
fn header(ui: &mut Ui, pass: &mut Pass<'_>, filters: &mut ShipFilters) -> f32 {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, HEADER_HEIGHT), Sense::hover());
    let row = Rect::from_min_max(
        Pos2::new(rect.left() + HEADER_PAD_LEFT, rect.top() + HEADER_PAD),
        Pos2::new(rect.right() - HEADER_PAD, rect.top() + HEADER_PAD + icon_button::SIZE),
    );
    let middle = row.center().y;
    // the close button on the right, Reset before it, in child uis (the header's allocation is
    // the layout's)
    let close = Rect::from_min_size(Pos2::new(row.right() - icon_button::SIZE, row.top()), Vec2::splat(icon_button::SIZE));
    let closed = IconButton::of(Icon::X, 16.0, CLOSE).tooltip(CLOSE_TOOLTIP).show(&mut ui.new_child(UiBuilder::new().max_rect(close))).clicked();
    let reset_label = "Reset";
    let link = Style::ui(theme::TEXT_SM, 600, theme::AMBER);
    let reset_w = text::measure(ui.ctx(), text::job(reset_label, &link)).x + 2.0 * chips::LINK_PAD.x;
    let reset_h = link.line_px() + 2.0 * chips::LINK_PAD.y;
    let reset_rect = Rect::from_min_size(Pos2::new(close.left() - HEADER_GAP - reset_w, middle - reset_h / 2.0), Vec2::new(reset_w, reset_h));
    let active = ship_filter_count(filters) > 0;
    let reset_clicked = chips::text_button(&mut ui.new_child(UiBuilder::new().max_rect(reset_rect)), Id::unique("rustradar::filters::ships::reset"), reset_label, active).clicked();

    let ships = pass.state.ships.ships();
    let painter = ui.painter();
    let title = painter.layout_job(text::job(TITLE, &title_style()));
    let title_w = title.size().x;
    painter.galley(Pos2::new(row.left(), middle - title.size().y / 2.0), title, Color32::PLACEHOLDER);
    let shown = painter.layout_job(text::tabular(text::job(&shown_text(shown(ships, filters), ships.len()), &shown_style())));
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
fn body(ui: &mut Ui, state: &AppState, filters: &mut ShipFilters) {
    let ships = state.ships.ships();
    // 1. the types
    let kinds = kind_options(ships);
    let options: Vec<ChipOption<'_, u8>> = kinds.iter().map(|k| counted(ChipOption::new(k.key, k.label).color(k.color), Some(k.count))).collect();
    FilterChips::new("rustradar::filters::ships::kinds", TYPE, &options).show(ui, &mut filters.hidden_kinds);
    ui.add_space(GROUP_GAP);

    // 2. the movement: a label, the segmented, the hint, 8 apart
    let width = ui.available_width();
    paragraph(ui, MOVEMENT, &label_style(), width);
    ui.add_space(LEGEND_GAP);
    let choices: Vec<Choice<'_, ShipMotion>> = MOTIONS.iter().map(|&(v, label)| Choice::new(v, label)).collect();
    Segmented::new("rustradar::filters::ships::motion", MOVEMENT, &choices).hide_legend().show(ui, &mut filters.motion);
    ui.add_space(LEGEND_GAP);
    paragraph(ui, MOVEMENT_HINT, &hint_style(), width);
    ui.add_space(GROUP_GAP);

    // 3. the sizes, where the feed tells them apart
    let known = sizes_known(state.controller.view().zoom);
    let sizes = size_options(ships, known);
    let options: Vec<ChipOption<'_, ShipSize>> = sizes.iter().map(|&(size, label, count)| counted(ChipOption::new(size, label), count)).collect();
    let chips = FilterChips::new("rustradar::filters::ships::sizes", SIZE, &options).disabled(!known);
    let chips = if known { chips } else { chips.hint(SIZES_HINT) };
    chips.show(ui, &mut filters.hidden_sizes);
    ui.add_space(GROUP_GAP);

    // 4. old positions
    ToggleRow::new("rustradar::filters::ships::old", HIDE_OLD).hint(HIDE_OLD_HINT).show(ui, &mut filters.hide_old);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship(mmsi: u32, kind: u8, size: u8) -> Ship {
        Ship { mmsi, kind, size, ..Ship::default() }
    }

    #[test]
    fn types_in_the_atlas_order_the_unknown_last_unlisted_types_with_it() {
        let ships = [ship(1, 3, 2), ship(2, 3, 1), ship(3, 33, 1), ship(4, 0, 0), ship(5, 5, 0)];
        let kinds = kind_options(&ships);
        let keys: Vec<u8> = kinds.iter().map(|k| k.key).collect();
        assert_eq!(keys, [2, 3, 4, 5, 6, 7, 8, 0]);
        let labels: Vec<&str> = kinds.iter().map(|k| k.label).collect();
        assert_eq!(labels, ["Tug or special", "Passenger", "Cargo", "Fishing", "Tanker", "Rescue or law", "Pleasure or sailing", "Unknown type"]);
        let counts: Vec<usize> = kinds.iter().map(|k| k.count).collect();
        assert_eq!(counts, [0, 2, 0, 1, 0, 0, 0, 2]);
        assert_eq!(kinds[1].color, Color32::from_rgb(98, 152, 240));
        assert_eq!(kinds[7].color, Color32::from_rgb(140, 152, 168));
    }

    #[test]
    fn sizes_are_counted_only_where_the_feed_tells_them_apart() {
        let ships = [ship(1, 3, 2), ship(2, 3, 1), ship(3, 4, 1)];
        assert_eq!(size_options(&ships, true), [(0, "Small", Some(0)), (1, "Medium", Some(2)), (2, "Large", Some(1))]);
        assert_eq!(size_options(&ships, false), [(0, "Small", None), (1, "Medium", None), (2, "Large", None)]);
        // a count of 0 is not shown, as `{#if count}`
        assert_eq!(counted(ChipOption::new(0u8, "Small"), Some(0)).count, None);
        assert_eq!(counted(ChipOption::new(1u8, "Medium"), Some(2)).count, Some(2));
        assert_eq!(counted(ChipOption::new(1u8, "Medium"), None).count, None);
    }

    #[test]
    fn reset_keeps_the_sort_and_shown_counts_what_the_filters_keep() {
        let filters = ShipFilters { hidden_kinds: vec![3], motion: ShipMotion::Moored, hidden_sizes: vec![0], hide_old: true, sort: rustradar_core::ship_list::ShipSort::Name };
        assert_eq!(reset(&filters), ShipFilters { sort: rustradar_core::ship_list::ShipSort::Name, ..DEFAULT_SHIP_FILTERS });
        let mut moving = ship(1, 4, 1);
        moving.heading = Some(10.0);
        let ships = [moving, ship(2, 3, 1), ship(3, 4, 0)];
        assert_eq!(shown(&ships, &DEFAULT_SHIP_FILTERS), 3);
        assert_eq!(shown(&ships, &ShipFilters { motion: ShipMotion::Moored, ..DEFAULT_SHIP_FILTERS }), 2);
        assert_eq!(shown(&ships, &ShipFilters { hidden_kinds: vec![4], ..DEFAULT_SHIP_FILTERS }), 1);
    }
}
