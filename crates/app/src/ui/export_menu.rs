//! ExportMenu (item U12; doc 06 §10; `ExportMenu.svelte`, `export.ts`): the flight's recorded
//! path saved as a KML, GPX, CSV or GeoJSON file, in the altitude chart's header
//! ([`super::chart`] draws the trigger where the header's tools stand).
//!
//! - **The trigger**: a text button, 12 px `--text-3` (`--text-1` under the pointer or while the
//!   menu is open; `--danger` after a failure, under the pointer `--text-1`), at half opacity while
//!   disabled: fewer than two positions to export, or a save running. It reads `Export`,
//!   `Saving…` while a save runs, then `Saved` or `Not saved` for 5 s (8 s after a failure). Its
//!   tooltip is the last outcome's text, else `Nothing to export until the aircraft has reported a
//!   few positions` or `Save this flight's path as a file`. A click opens or closes the menu.
//! - **The menu** (an egui area over the panel, `z-index: 5`: over the panel's pinned footer and
//!   cut by the panel's edge as the panel's `overflow` cuts it today): 240 px of content, padding
//!   4, 1 px `--line` border, radius 4, on `--surface-raised`, its right edge on the trigger's
//!   and its top 8 px under the trigger's line box (the `.export` block, 19.6 px: its 14 px strut);
//!   the four formats of [`EXPORT_FORMATS`] as items (padding 8, radius 2, `--surface-hover`
//!   under the pointer or with the keyboard on them: the label 13 px at 600, 2 px over the hint
//!   12 px `--text-3`); then the note `{count} positions, saved to Downloads` (12 px `--text-3`,
//!   a rule over it, margins `4px 8px`, padding-top 8).
//! - **Behaviour** (ported from the component's script): opening puts the keyboard on the first
//!   item (at the next pass, as `queueMicrotask`); Up and Down cycle through the items; Escape
//!   closes it, goes no further (the page's Escape would close the panel: [`escape`], called
//!   before the shell's keys) and gives the trigger the keyboard; a click outside the trigger and
//!   the menu, or the keyboard leaving them, closes it. Choosing a format builds the file
//!   ([`export_path`], [`export_file_name`] of the [`ExportMeta`]) and saves it
//!   ([`ExportFeeds::save`]: the backend writes it to Downloads, numbered when the name is taken,
//!   and the file manager shows it, `saveExport`); then `Saved {file}` or `Couldn't save the path:
//!   {error}`, the menu closes and the trigger takes the keyboard.
//! - **Focus rings** follow Chromium's `:focus-visible`: shown after the keyboard was used, not
//!   when a click put the keyboard there (the first item after a click on the trigger).
//!
//! **Checked** against Chromium's layout of today's component (opened by a click and with Enter
//! then Down, and `Saved`; `tests/altitude_chart.rs`): the menu's border, items, highlight and
//! focus ring on Chromium's pixels, the texts on its ink or within a pixel. In the flight panel
//! (`tests/altitude_chart_panel.rs`) it opens over the pinned footer, and Escape closes it before
//! the panel.
//!
//! The menu's state is the component's: it starts afresh when the chart is drawn again after a
//! pass without it (the panel reopened, the Overview tab chosen again); a save still running
//! then completes and shows its file, unseen, as the old component's promise did.
//!
//! Deviations:
//! - Phones do not show the menu in the first native release (plan section 6, item 9; the chart
//!   leaves it out where [`crate::platform::PHONE`]).
//! - The menu opening near the panel's bottom makes the panel's content taller by what of it
//!   hangs below (`Ui::expand_to_include_rect`), so it can be scrolled into view, as Chromium
//!   lets the panel scroll to an absolutely placed box; a panel shorter than the window grows
//!   instead of scrolling.
//! - Without a backend (it did not start) a save answers
//!   [`NO_BACKEND`] (today's plain-browser page downloaded the file).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use egui::emath::GuiRounding as _;
use egui::{Area, Context, CornerRadius, CursorIcon, Event, FocusDirection, Id, Key, Order, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use rustradar_backend::Backend;
use rustradar_core::export::{EXPORT_FORMATS, ExportFormat, ExportMeta, export_file_name, export_path, export_points};
use rustradar_core::model::TrailPoint;

use super::keys_dialog::{NO_BACKEND, message};
use super::panel;
use super::text::{self, Style};
use super::theme;
use super::widgets::button::focus_ring;
use super::widgets::tooltip;
use crate::requests::Requests;
use crate::state::AppState;
use crate::state::aircraft::Answer;

/// The trigger's labels.
pub const EXPORT: &str = "Export";
pub const SAVING: &str = "Saving…";
pub const SAVED: &str = "Saved";
pub const NOT_SAVED: &str = "Not saved";
/// The trigger's tooltips without an outcome.
pub const NOTHING_TO_EXPORT: &str = "Nothing to export until the aircraft has reported a few positions";
pub const SAVE_PATH: &str = "Save this flight's path as a file";
/// The menu's accessible name.
pub const MENU_LABEL: &str = "Export the flight path";

/// The menu's content width (`width: 240px`, content-box), padding, border, radius.
pub const MENU_WIDTH: f32 = 240.0;
pub const MENU_PAD: f32 = theme::SPACE_1;
/// How far under the trigger's line box the menu starts (`top: calc(100% + var(--space-2))`).
pub const MENU_GAP: f32 = theme::SPACE_2;
/// An item's padding, the gap between its label and hint, its radius (`--radius-sm` less 2).
pub const ITEM_PAD: f32 = theme::SPACE_2;
pub const ITEM_GAP: f32 = 2.0;
pub const ITEM_RADIUS: u8 = theme::RADIUS_SM - 2;
/// The note's margins (`var(--space-1) var(--space-2)`) and its padding over the text.
pub const NOTE_MARGIN: Vec2 = Vec2::new(theme::SPACE_2, theme::SPACE_1);
pub const NOTE_PAD: f32 = theme::SPACE_2;
/// The trigger's line box (12 px at 1.4) and the `.export` block round it (its 14 px strut).
pub const TRIGGER_LINE: f32 = theme::TEXT_SM * theme::LINE_HEIGHT;
pub const BLOCK_LINE: f32 = theme::TEXT_BASE * theme::LINE_HEIGHT;
/// The trigger's opacity while disabled.
pub const DISABLED_OPACITY: f32 = 0.5;
/// How long an outcome stays on the trigger, s.
pub const SAVED_SECONDS: f64 = 5.0;
pub const NOT_SAVED_SECONDS: f64 = 8.0;

/// The trigger's text (`.toggle`: 12 px).
pub const fn trigger_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// An item's label (13 px at 600, `--text-1`).
pub const fn label_style() -> Style {
    Style::ui(theme::TEXT_MD, 600, theme::TEXT_1)
}

/// An item's hint and the note (12 px `--text-3`).
pub const fn hint_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

/// The note under the items.
pub fn note(count: usize) -> String {
    format!("{count} positions, saved to Downloads")
}

/// `Saved {file}`: the last part of `path` (`\` or `/` apart), or `name` when that is empty.
pub fn saved(path: &Path, name: &str) -> String {
    let path = path.to_string_lossy();
    let file = path.rsplit(['/', '\\']).next().filter(|f| !f.is_empty()).unwrap_or(name);
    format!("Saved {file}")
}

/// `Couldn't save the path: {error}`, the error without a leading `Error:`.
pub fn not_saved(e: &str) -> String {
    format!("Couldn't save the path: {}", message(e))
}

// ------------------------------------------------------------------------------------------------
// where the files go

/// Where a save goes: the backend, nowhere, or a test's.
pub trait ExportFeeds: Send {
    /// `saveExport(fileName, contents, format)`: the file written to Downloads (numbered when the
    /// name is taken) and shown in the file manager; where it went.
    fn save(&mut self, file_name: &str, contents: String) -> Answer<Result<PathBuf, String>>;
}

/// The backend's Downloads, on its runtime.
struct BackendFeeds {
    backend: Backend,
    requests: Requests,
}

impl ExportFeeds for BackendFeeds {
    fn save(&mut self, file_name: &str, contents: String) -> Answer<Result<PathBuf, String>> {
        let (backend, file_name) = (self.backend.clone(), file_name.to_owned());
        self.requests
            .request(async move {
                // blocking file I/O (doc 07: `save_export` does it on the blocking pool)
                let path = tokio::task::spawn_blocking(move || backend.save_export(&file_name, &contents)).await.unwrap_or_else(|e| Err(e.to_string()))?;
                // showing it is a convenience; the file is saved either way
                crate::platform::reveal(&path);
                Ok(path)
            })
            .into()
    }
}

/// Without a backend: every save answers [`NO_BACKEND`].
struct NoBackend;

impl ExportFeeds for NoBackend {
    fn save(&mut self, _file_name: &str, _contents: String) -> Answer<Result<PathBuf, String>> {
        Answer::ready(Err(NO_BACKEND.into()))
    }
}

/// The feeds for `state`: the backend's, else none.
pub fn feeds_for(state: &AppState) -> Box<dyn ExportFeeds> {
    match (&state.backend, &state.requests) {
        (Some(backend), Some(requests)) => Box::new(BackendFeeds { backend: backend.clone(), requests: requests.clone() }),
        _ => Box::new(NoBackend),
    }
}

// ------------------------------------------------------------------------------------------------
// the component's state

/// The last outcome (`status`), until egui time `until`.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub text: String,
    pub error: bool,
    pub until: f64,
}

/// The component's state, kept by egui between passes.
#[derive(Default)]
pub struct Menu {
    /// Chosen at the first pass ([`feeds_for`]) unless a test installed some.
    feeds: Option<Box<dyn ExportFeeds>>,
    pub open: bool,
    pub busy: bool,
    pub status: Option<Status>,
    /// The saves running, with the names they were asked under.
    saves: Vec<(Answer<Result<PathBuf, String>>, String)>,
    /// The first item takes the keyboard at a pass after this one (the pass it opened in).
    focus_first: Option<u64>,
    /// The keyboard was used last (`:focus-visible`), not the pointer.
    focus_visible: bool,
    /// The egui pass it was last drawn in.
    pass: u64,
}

impl std::fmt::Debug for Menu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Menu").field("open", &self.open).field("busy", &self.busy).field("status", &self.status).field("saves", &self.saves.len()).finish_non_exhaustive()
    }
}

impl Menu {
    fn feeds(&mut self) -> &mut dyn ExportFeeds {
        self.feeds.get_or_insert_with(|| Box::new(NoBackend)).as_mut()
    }

    /// The trigger's text.
    pub fn label(&self) -> &'static str {
        match &self.status {
            _ if self.busy => SAVING,
            Some(s) if s.error => NOT_SAVED,
            Some(_) => SAVED,
            None => EXPORT,
        }
    }

    /// The trigger's tooltip with `count` positions to export.
    pub fn title(&self, count: usize) -> String {
        match &self.status {
            Some(s) => s.text.clone(),
            None if count < 2 => NOTHING_TO_EXPORT.to_owned(),
            None => SAVE_PATH.to_owned(),
        }
    }

    /// `save(format)`: the file built and asked to be saved.
    pub fn save(&mut self, trail: &[TrailPoint], meta: &ExportMeta, format: ExportFormat) {
        self.busy = true;
        let contents = export_path(trail, meta, format);
        let name = export_file_name(meta, &export_points(trail), format);
        let ask = self.feeds().save(&name, contents);
        self.saves.push((ask, name));
    }

    /// `show(text, error)` at egui time `now`.
    fn show(&mut self, text: String, error: bool, now: f64) {
        let until = now + if error { NOT_SAVED_SECONDS } else { SAVED_SECONDS };
        self.status = Some(Status { text, error, until });
    }

    /// The saves answered since the last pass, at egui time `now`: each sets the status, ends
    /// `busy`, closes the menu and asks for the trigger to take the keyboard (`finally`). Answers
    /// whether one did.
    fn poll(&mut self, now: f64) -> bool {
        let mut answered = Vec::new();
        self.saves.retain(|(ask, name)| match ask.poll() {
            Some(answer) => {
                answered.push((answer, name.clone()));
                false
            }
            None => true,
        });
        let done = !answered.is_empty();
        for (answer, name) in answered {
            match answer {
                Ok(path) => self.show(saved(&path, &name), false, now),
                Err(e) => self.show(not_saved(&e), true, now),
            }
            self.busy = false;
            self.open = false;
        }
        if self.status.as_ref().is_some_and(|s| s.until <= now) {
            self.status = None;
        }
        done
    }

    /// The component mounted afresh: its state as new, the saves running left to finish unseen.
    fn remount(&mut self) {
        let feeds = self.feeds.take();
        *self = Menu { feeds, ..Menu::default() };
    }
}

fn menu_id() -> Id {
    Id::unique("rustradar::export_menu")
}

/// The trigger's id.
pub fn trigger_id() -> Id {
    menu_id().with("trigger")
}

/// The id of the `n`th item.
pub fn item_id(n: usize) -> Id {
    menu_id().with(("item", n))
}

/// The menu's state as `ctx` keeps it between passes.
pub fn menu(ctx: &Context) -> Arc<Mutex<Menu>> {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(menu_id().with("state"), || Arc::new(Mutex::new(Menu::default()))).clone())
}

/// Makes the menu save through `feeds` instead of the backend (tests).
pub fn install_feeds(ctx: &Context, feeds: Box<dyn ExportFeeds>) {
    let menu = menu(ctx);
    menu.lock().unwrap_or_else(PoisonError::into_inner).feeds = Some(feeds);
}

/// Gives the menu `state`'s feeds unless it has some.
pub fn feeds_from(ctx: &Context, state: &AppState) {
    let menu = menu(ctx);
    let mut m = menu.lock().unwrap_or_else(PoisonError::into_inner);
    if m.feeds.is_none() {
        m.feeds = Some(feeds_for(state));
    }
}

/// Escape while the menu is open (its `onkeydown` with `stopPropagation`, before the page's keys:
/// call it before [`super::shell::ui`]): the key is taken out of egui's input, the menu closes and
/// the trigger takes the keyboard.
pub fn escape(ctx: &Context) {
    let menu = menu(ctx);
    let mut m = menu.lock().unwrap_or_else(PoisonError::into_inner);
    // open, and drawn in the last pass (a menu whose panel went away is not there)
    if !m.open || m.pass + 1 < ctx.cumulative_pass_nr() {
        return;
    }
    let pressed = |e: &Event| matches!(e, Event::Key { key: Key::Escape, pressed: true, .. });
    if !ctx.input(|i| i.events.iter().any(pressed)) {
        return;
    }
    ctx.input_mut(|i| i.events.retain(|e| !pressed(e)));
    m.open = false;
    m.focus_visible = true;
    ctx.memory_mut(|mem| mem.request_focus(trigger_id()));
}

// ------------------------------------------------------------------------------------------------
// drawing

/// The trigger for one pass: the menu's state polled, its label known before the header is laid
/// out (see [`Trigger::show`]).
pub struct Trigger {
    menu: Arc<Mutex<Menu>>,
    label: &'static str,
}

impl Trigger {
    /// The menu's state in `ctx` at this pass: the saves' answers taken, a state left from a pass
    /// without the menu cleared (the component mounted again).
    pub fn new(ctx: &Context) -> Trigger {
        let menu = menu(ctx);
        let label = {
            let mut m = menu.lock().unwrap_or_else(PoisonError::into_inner);
            let now_pass = ctx.cumulative_pass_nr();
            if m.pass + 1 < now_pass {
                m.remount();
            }
            m.pass = now_pass;
            let now = ctx.input(|i| i.time);
            if m.poll(now) {
                ctx.memory_mut(|mem| mem.request_focus(trigger_id()));
            }
            if let Some(status) = &m.status {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64((status.until - now).max(0.0)));
            }
            if m.busy {
                ctx.request_repaint();
            }
            m.label()
        };
        Trigger { menu, label }
    }

    /// The trigger's text.
    pub fn label(&self) -> &'static str {
        self.label
    }

    /// The trigger's width: its text's.
    pub fn width(&self, ctx: &Context) -> f32 {
        ctx.fonts_mut(|f| f.layout_job(text::job(self.label, &trigger_style()))).size().x
    }

    /// The trigger in `button` (its text's line box, its baseline `baseline` px under the box's
    /// top), the `.export` block round it `block`, with the menu under the block while open; for
    /// the path `trail` of the flight `meta` describes. `ui` is the chart's: its content is
    /// extended to the open menu's bottom, so the panel can be scrolled to it. Answers the menu's
    /// rectangle while it is open.
    pub fn show(self, ui: &mut Ui, block: Rect, button: Rect, trail: &[TrailPoint], meta: &ExportMeta) -> Option<Rect> {
        let ctx = ui.ctx().clone();
        let mut m = self.menu.lock().unwrap_or_else(PoisonError::into_inner);
        // `:focus-visible`: the keyboard used last, not the pointer
        let (keyed, pressed) = ctx.input(|i| (i.events.iter().any(|e| matches!(e, Event::Key { pressed: true, .. })), i.pointer.any_pressed()));
        if pressed {
            m.focus_visible = false;
        } else if keyed {
            m.focus_visible = true;
        }
        let count = export_points(trail).len();
        let enabled = count >= 2 && !m.busy;
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let response = ui.interact(button, trigger_id(), sense);
        let response = if enabled { response.on_hover_cursor(CursorIcon::PointingHand) } else { response };
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, m.open, self.label));
        if response.clicked() {
            // a pressed button takes the keyboard (without a ring after a click)
            ctx.memory_mut(|mem| mem.request_focus(trigger_id()));
            m.open = !m.open;
            if m.open {
                m.focus_first = Some(ctx.cumulative_pass_nr());
                m.focus_visible = !response.clicked_by(PointerButton::Primary);
            }
        }
        // the colour: under the pointer, then a failure, then open
        let error = m.status.as_ref().is_some_and(|s| s.error);
        let color = if response.hovered() && enabled {
            theme::TEXT_1
        } else if error {
            theme::DANGER
        } else if m.open {
            theme::TEXT_1
        } else {
            theme::TEXT_3
        };
        let painter = ui.painter();
        let style = trigger_style().color(color);
        let galley = painter.layout_job(text::job(self.label, &style));
        let color = if enabled { color } else { color.gamma_multiply(DISABLED_OPACITY) };
        painter.galley_with_override_text_color(button.min, galley, color);
        if response.has_focus() && m.focus_visible {
            focus_ring(painter, button, 0);
        }
        let title = m.title(count);
        tooltip::show(response, &title);
        if !m.open {
            return None;
        }
        let menu_rect = menu_rect(&ctx, block, count);
        // (on egui's 1/32 px grid, as every rectangle a `Ui` holds)
        ui.expand_to_include_rect(menu_rect.round_ui());
        let clip = ui.clip_rect();
        self.menu_area(&ctx, &mut m, menu_rect, clip, block, count, trail, meta);
        Some(menu_rect)
    }

    /// The open menu at `rect`, cut by `clip`.
    #[allow(clippy::too_many_arguments)]
    fn menu_area(&self, ctx: &Context, m: &mut Menu, rect: Rect, clip: Rect, block: Rect, count: usize, trail: &[TrailPoint], meta: &ExportMeta) {
        let n = EXPORT_FORMATS.len();
        // the first item takes the keyboard a pass after opening (`queueMicrotask`)
        if m.focus_first.is_some_and(|opened| opened < ctx.cumulative_pass_nr()) {
            m.focus_first = None;
            ctx.memory_mut(|mem| mem.request_focus(item_id(0)));
        }
        let focused = ctx.memory(|mem| mem.focused());
        let at = focused.and_then(|f| (0..n).find(|&i| item_id(i) == f));
        // Up and Down cycle while the keyboard is in the menu (an item, or none: the menu itself)
        if at.is_some() || focused.is_none() {
            let down = take_presses(ctx, Key::ArrowDown);
            let up = take_presses(ctx, Key::ArrowUp);
            if down + up > 0 {
                let mut i = at.map_or(-1, |a| a as i64);
                for _ in 0..down {
                    i = (i + 1).rem_euclid(n as i64);
                }
                for _ in 0..up {
                    i = (i - 1 + n as i64).rem_euclid(n as i64);
                }
                ctx.memory_mut(|mem| {
                    mem.request_focus(item_id(i as usize));
                    mem.move_focus(FocusDirection::None);
                });
                m.focus_visible = true;
            }
        }
        let mut chosen = None;
        Area::new(menu_id().with("area")).order(Order::Foreground).fixed_pos(rect.min).constrain(false).fade_in(false).show(ctx, |ui| {
            ui.set_clip_rect(clip);
            let painter = ui.painter().clone();
            let ppp = ctx.pixels_per_point();
            let r = CornerRadius::same(theme::RADIUS_SM);
            painter.rect(panel::snap_rect(rect, ppp), r, theme::SURFACE_RAISED, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
            let width = MENU_WIDTH - 2.0 * ITEM_PAD;
            let left = rect.left() + theme::BORDER + MENU_PAD;
            let mut y = rect.top() + theme::BORDER + MENU_PAD;
            for (i, f) in EXPORT_FORMATS.iter().enumerate() {
                let label = painter.layout_job(text::job(f.label, &label_style()));
                let hint = painter.layout_job(text::wrapped(f.hint, &hint_style(), width));
                let hint_h = hint_style().line_px() * hint.rows.len().max(1) as f32;
                let item = Rect::from_min_size(Pos2::new(left, y), Vec2::new(MENU_WIDTH, ITEM_PAD + label_style().line_px() + ITEM_GAP + hint_h + ITEM_PAD));
                let response = ui.interact(item, item_id(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, f.label));
                let keyed = response.has_focus() && m.focus_visible;
                if response.hovered() || keyed {
                    painter.rect_filled(panel::snap_rect(item, ppp), CornerRadius::same(ITEM_RADIUS), theme::SURFACE_HOVER);
                }
                let text_left = item.left() + ITEM_PAD;
                painter.galley(Pos2::new(text_left, item.top() + ITEM_PAD), label, theme::TEXT_1);
                painter.galley(Pos2::new(text_left, item.top() + ITEM_PAD + label_style().line_px() + ITEM_GAP), hint, theme::TEXT_3);
                if keyed {
                    focus_ring(&painter, item, ITEM_RADIUS);
                }
                if response.clicked() {
                    chosen = Some(f.format);
                }
                y = item.bottom();
            }
            // the note, its rule on top
            let top = y + NOTE_MARGIN.y;
            let x = left + NOTE_MARGIN.x;
            panel::hrule(&painter, x..=x + MENU_WIDTH - 2.0 * NOTE_MARGIN.x, top);
            let note = painter.layout_job(text::wrapped(&note(count), &hint_style(), MENU_WIDTH - 2.0 * NOTE_MARGIN.x));
            painter.galley(Pos2::new(x, top + theme::BORDER + NOTE_PAD), note, theme::TEXT_3);
        });
        if let Some(format) = chosen {
            m.save(trail, meta, format);
            ctx.request_repaint();
        }
        // a click outside the trigger's block and the menu closes it (the window's `onclick`)
        let outside = ctx.input(|i| i.pointer.primary_clicked() && i.pointer.interact_pos().is_some_and(|p| !block.contains(p) && !rect.contains(p)));
        // the keyboard leaving them closes it (`onfocusout`)
        let ours = |id: Id| id == trigger_id() || (0..n).any(|i| item_id(i) == id);
        let left = ctx.memory(|mem| mem.focused()).is_some_and(|f| !ours(f));
        if outside || left {
            m.open = false;
        }
    }
}

/// The menu's rectangle under the `.export` block `block`, with `count` positions in its note.
pub fn menu_rect(ctx: &Context, block: Rect, count: usize) -> Rect {
    let width = MENU_WIDTH - 2.0 * ITEM_PAD;
    let items: f32 = EXPORT_FORMATS
        .iter()
        .map(|f| {
            let rows = ctx.fonts_mut(|fonts| fonts.layout_job(text::wrapped(f.hint, &hint_style(), width))).rows.len().max(1);
            ITEM_PAD + label_style().line_px() + ITEM_GAP + hint_style().line_px() * rows as f32 + ITEM_PAD
        })
        .sum();
    let note_rows = ctx.fonts_mut(|fonts| fonts.layout_job(text::wrapped(&note(count), &hint_style(), MENU_WIDTH - 2.0 * NOTE_MARGIN.x))).rows.len().max(1);
    let note_h = theme::BORDER + NOTE_PAD + hint_style().line_px() * note_rows as f32;
    let height = 2.0 * (theme::BORDER + MENU_PAD) + items + 2.0 * NOTE_MARGIN.y + note_h;
    let outer = MENU_WIDTH + 2.0 * (theme::BORDER + MENU_PAD);
    Rect::from_min_size(Pos2::new(block.right() - outer, block.bottom() + MENU_GAP), Vec2::new(outer, height))
}

/// The presses of `key` in this pass, taken out of egui's input.
fn take_presses(ctx: &Context, key: Key) -> usize {
    ctx.input_mut(|i| {
        let before = i.events.len();
        i.events.retain(|e| !matches!(e, Event::Key { key: k, pressed: true, .. } if *k == key));
        before - i.events.len()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_read_as_today() {
        assert_eq!(saved(Path::new("/home/me/Downloads/SAS1417 ARN-CPH 2026-06-21 (2).kml"), "x.kml"), "Saved SAS1417 ARN-CPH 2026-06-21 (2).kml");
        assert_eq!(saved(Path::new("C:\\Users\\me\\Downloads\\SAS1417.gpx"), "x.gpx"), "Saved SAS1417.gpx");
        assert_eq!(saved(Path::new("/home/me/Downloads/"), "flight.csv"), "Saved flight.csv", "an empty last part falls back to the name");
        assert_eq!(not_saved("Error: No Downloads folder to save to"), "Couldn't save the path: No Downloads folder to save to");
        assert_eq!(not_saved("disk full"), "Couldn't save the path: disk full");
        assert_eq!(note(60), "60 positions, saved to Downloads");
    }

    #[test]
    fn the_trigger_s_label_and_tooltip() {
        let mut m = Menu::default();
        assert_eq!((m.label(), m.title(1)), (EXPORT, NOTHING_TO_EXPORT.to_owned()));
        assert_eq!(m.title(2), SAVE_PATH);
        m.busy = true;
        assert_eq!(m.label(), SAVING);
        m.busy = false;
        m.show("Saved a.kml".into(), false, 10.0);
        assert_eq!((m.label(), m.title(0)), (SAVED, "Saved a.kml".to_owned()));
        m.show(not_saved("nope"), true, 10.0);
        assert_eq!(m.label(), NOT_SAVED);
        assert_eq!(m.status.as_ref().map(|s| s.until), Some(18.0), "a failure stays 8 s");
        assert!(!m.poll(17.9) && m.status.is_some());
        assert!(!m.poll(18.0) && m.status.is_none(), "then it goes");
    }
}
