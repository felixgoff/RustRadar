//! TokenInput (doc 06 §11.1; `TokenInput.svelte`): chosen values as chips in a field you type
//! into, with suggestions listed under it (the aircraft filters' airlines, types and airports).
//!
//! - **Label**: 12 px at 600 in `--text-2`; the parts 4 px apart in a column (label, field,
//!   suggestions, hint).
//! - **Field**: a wrapping row of the chips and the input, 4 px apart both ways, centred in each
//!   line; padding `3px 8px`, a 1 px `--line-control` border (`--amber` while the input is
//!   focused, 150 ms), radius 4, `--ink`. CSS's `min-height: 34px` is the content box, so an
//!   empty field is 42 px tall. The input grows into the rest of its line and wraps onto a line
//!   of its own when fewer than 84 px are left (its `min-width: 80px` plus the 2 px side padding
//!   Chromium gives an input); it is 28 px tall (26 and 1 px above and below), 13 px text.
//! - **Chip**: 24 px tall, padding-left 8, radius 4, `--amber-soft`, 12 px at 600 in `--amber`,
//!   the text cut at 150 px with an ellipsis (its tooltip the whole text), then 2 px and a 22 × 22
//!   remove button with `X` 12/2 (`--amber-soft` under the pointer).
//! - **Suggestions** (while the input is focused and `suggest` gives any): in the flow under the
//!   field, `.surface` on `--surface-raised`, padding 4; rows padding `6px 8px`, radius 4, 13 px,
//!   the label on the left cut with an ellipsis and the detail on the right (Overpass Mono 11 px
//!   in `--text-3`) on its baseline, 12 px apart; the active row on `--surface-hover`.
//! - **Hint**: 11 px `--text-3`.
//!
//! Behaviour ([`State`], tested on its own): `suggest(query, values)` is asked while the list
//! is open, even for an empty query. Focus or typing opens the list (typing makes the first row
//! active); losing the focus closes it. Down/Up cycle the active row; Enter adds the active
//! suggestion, else `free(trim(query))` when that is valid; Backspace in an empty input removes
//! the last chip; Escape closes the list and keeps the focus. Pressing a row (or the pointer
//! entering it, which makes it active) adds it without taking the focus from the input. Adding
//! clears the query and makes the first row active; a value already chosen is not added twice.
//! [`three_letters`] and [`type_code`] are the filters' free-entry rules.

use std::hash::Hash;
use std::sync::Arc;

use egui::{Color32, CornerRadius, CursorIcon, Galley, Id, Key, Modifiers, Pos2, Rect, Response, Sense, Stroke, StrokeKind, SurrenderFocusOn, Ui, Vec2};

use super::button::transition_color;
use super::text_field;
use super::tooltip;
use crate::ui::anim;
use crate::ui::icons::{self, Icon, STROKE_REGULAR};
use crate::ui::text::{self, Style};
use crate::ui::theme;

/// Between the label, the field, the suggestions and the hint; and between items in the field.
pub const GAP: f32 = theme::SPACE_1;
/// The field's padding.
pub const PAD: Vec2 = Vec2::new(theme::SPACE_2, 3.0);
/// The field's minimum content height.
pub const MIN_CONTENT: f32 = 34.0;
/// The input's outer minimum width and height (Chromium's `padding: 1px 2px` round it).
pub const INPUT_MIN_WIDTH: f32 = 80.0 + 2.0 * INPUT_PAD.x;
pub const INPUT_HEIGHT: f32 = 26.0 + 2.0 * INPUT_PAD.y;
pub const INPUT_PAD: Vec2 = text_field::NATIVE_PADDING;
/// A chip: height, left padding, text's maximum width, gap to the button, the button's side.
pub const CHIP_HEIGHT: f32 = 24.0;
pub const CHIP_PAD: f32 = theme::SPACE_2;
pub const CHIP_TEXT_MAX: f32 = 150.0;
pub const CHIP_GAP: f32 = 2.0;
pub const REMOVE: f32 = 22.0;
/// The suggestions' padding, a row's padding and the gap between label and detail.
pub const LIST_PAD: f32 = theme::SPACE_1;
pub const ROW_PAD: Vec2 = Vec2::new(theme::SPACE_2, 6.0);
pub const ROW_GAP: f32 = theme::SPACE_3;

/// One suggestion: the value it adds, what the row says, and a detail on the right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub value: String,
    pub label: String,
    pub detail: Option<String>,
}

impl Suggestion {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Suggestion { value: value.into(), label: label.into(), detail: None }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The keys the input handles itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKey {
    Down,
    Up,
    Enter,
    Backspace,
    Escape,
}

impl TokenKey {
    const ALL: [(Key, TokenKey); 5] = [(Key::ArrowDown, TokenKey::Down), (Key::ArrowUp, TokenKey::Up), (Key::Enter, TokenKey::Enter), (Key::Backspace, TokenKey::Backspace), (Key::Escape, TokenKey::Escape)];
}

/// The input's own state (`query`, `open`, `active` of `TokenInput.svelte`), kept per input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub query: String,
    pub open: bool,
    pub active: usize,
    /// The row under the pointer (a row becomes active when the pointer enters it).
    hovered: Option<usize>,
    /// Where the list was last drawn: the pointer over it does not take the focus from the input.
    list: Option<Rect>,
    /// Whether the input had the focus when last drawn (`onfocus` / `onblur` are the changes).
    focused: bool,
}

impl State {
    /// Adds `value` unless chosen already; clears the query, the first row active.
    pub fn add(&mut self, values: &mut Vec<String>, value: String) -> bool {
        let added = !values.contains(&value);
        if added {
            values.push(value);
        }
        self.query.clear();
        self.active = 0;
        added
    }

    /// The input's text changed (`oninput`).
    pub fn input(&mut self) {
        self.open = true;
        self.active = 0;
    }

    pub fn focus(&mut self) {
        self.open = true;
    }

    pub fn blur(&mut self) {
        self.open = false;
    }

    /// A key in the input (`onkeydown`), with the suggestions as they stand. Returns whether the
    /// key was the input's (its default action and propagation stopped): Down, Up and Enter when
    /// they act, Escape while the list is open.
    pub fn key(&mut self, key: TokenKey, suggestions: &[Suggestion], values: &mut Vec<String>, free: Option<FreeFn<'_>>) -> bool {
        let n = suggestions.len();
        match key {
            TokenKey::Down if n > 0 => {
                self.active = (self.active + 1) % n;
                true
            }
            TokenKey::Up if n > 0 => {
                self.active = (self.active + n - 1) % n;
                true
            }
            TokenKey::Enter => {
                let typed = free.and_then(|f| f(js_trim(&self.query)));
                if let Some(s) = suggestions.get(self.active) {
                    self.add(values, s.value.clone());
                } else if let Some(typed) = typed.filter(|t| !t.is_empty()) {
                    self.add(values, typed);
                }
                true
            }
            TokenKey::Backspace => {
                if self.query.is_empty()
                    && let Some(last) = values.last().cloned()
                {
                    remove(values, &last);
                }
                false
            }
            TokenKey::Escape if self.open => {
                self.open = false;
                true
            }
            _ => false,
        }
    }
}

/// Takes `value` out of `values`.
pub fn remove(values: &mut Vec<String>, value: &str) {
    values.retain(|v| v != value);
}

/// JavaScript's `String.prototype.trim`: Unicode's white space without U+0085, and U+FEFF.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}

/// The airlines' and airports' free entry, `/^[A-Z]{3}$/i` upper-cased.
pub fn three_letters(q: &str) -> Option<String> {
    (q.len() == 3 && q.bytes().all(|b| b.is_ascii_alphabetic())).then(|| q.to_ascii_uppercase())
}

/// The aircraft types' free entry, `/^[A-Z0-9]{2,4}$/i` upper-cased.
pub fn type_code(q: &str) -> Option<String> {
    ((2..=4).contains(&q.len()) && q.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| q.to_ascii_uppercase())
}

/// Where flex-wrap puts items of `sizes` (outer sizes; the last is the input, which grows) in a
/// content box `width` wide, `gap` apart both ways, centred in their lines: each item's rect
/// relative to the content box, and the content height (at least `min_height`).
pub fn flow(sizes: &[Vec2], width: f32, gap: f32, min_height: f32) -> (Vec<Rect>, f32) {
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut used = 0.0;
    for (i, s) in sizes.iter().enumerate() {
        match lines.last_mut() {
            Some(line) if used + gap + s.x <= width => {
                line.push(i);
                used += gap + s.x;
            }
            _ => {
                lines.push(vec![i]);
                used = s.x;
            }
        }
    }
    let mut rects = vec![Rect::NOTHING; sizes.len()];
    let mut y = 0.0;
    for line in &lines {
        let height = line.iter().map(|&i| sizes[i].y).fold(0.0, f32::max);
        let taken: f32 = line.iter().map(|&i| sizes[i].x).sum::<f32>() + gap * (line.len() - 1) as f32;
        let mut x = 0.0;
        for &i in line {
            let mut w = sizes[i].x;
            // the input (flex: 1) takes the line's free space
            if i == sizes.len() - 1 {
                w += (width - taken).max(0.0);
            }
            rects[i] = Rect::from_min_size(Pos2::new(x, y + (height - sizes[i].y) / 2.0), Vec2::new(w, sizes[i].y));
            x += w + gap;
        }
        y += height + gap;
    }
    let content = (y - gap).max(0.0);
    let min = min_height.max(content);
    // align-content: normal (stretch) spreads the extra height over the lines; with one line it
    // centres the items in the whole box
    if lines.len() == 1 && min > content {
        for r in &mut rects {
            *r = r.translate(Vec2::new(0.0, (min - content) / 2.0));
        }
    }
    (rects, min)
}

/// A free-entry rule: the value a trimmed query stands for, if it is valid.
pub type FreeFn<'f> = &'f dyn Fn(&str) -> Option<String>;
type Suggest<'a> = Box<dyn FnMut(&str, &[String]) -> Vec<Suggestion> + 'a>;
type Free<'a> = Box<dyn Fn(&str) -> Option<String> + 'a>;
type ChipText<'a> = Box<dyn Fn(&str) -> String + 'a>;

/// A token input over a list of chosen values.
pub struct TokenInput<'a> {
    id_salt: Id,
    label: &'a str,
    suggest: Suggest<'a>,
    free: Option<Free<'a>>,
    chip: ChipText<'a>,
    placeholder: &'a str,
    hint: Option<&'a str>,
}

impl<'a> TokenInput<'a> {
    /// `suggest(query, values)`: the suggestions for a query (possibly empty), best first, given
    /// the values chosen.
    pub fn new(id_salt: impl Hash + std::fmt::Debug, label: &'a str, suggest: impl FnMut(&str, &[String]) -> Vec<Suggestion> + 'a) -> Self {
        TokenInput { id_salt: Id::NULL.with(id_salt), label, suggest: Box::new(suggest), free: None, chip: Box::new(str::to_owned), placeholder: "", hint: None }
    }

    /// A value typed in full when no suggestion is active (given the trimmed query); `None` if
    /// it is not valid.
    pub fn free(mut self, free: impl Fn(&str) -> Option<String> + 'a) -> Self {
        self.free = Some(Box::new(free));
        self
    }

    /// What a chip says for a value (default: the value).
    pub fn chip(mut self, chip: impl Fn(&str) -> String + 'a) -> Self {
        self.chip = Box::new(chip);
        self
    }

    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = placeholder;
        self
    }

    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }

    /// The input across the available width; `changed()` when a value was added or removed. The
    /// response's rect is the whole input, label to hint.
    pub fn show(mut self, ui: &mut Ui, values: &mut Vec<String>) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let edit_id = id.with("edit");
        let mut state: State = ui.data(|d| d.get_temp(id)).unwrap_or_default();
        let before = values.clone();
        let width = ui.available_width();
        let focused = ui.memory(|m| m.has_focus(edit_id));

        // the keys go to the input before its text edit sees them
        if focused {
            let suggestions = if state.open { (self.suggest)(&state.query, values) } else { Vec::new() };
            let pressed: Vec<Key> = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key { key, pressed: true, modifiers, .. } if modifiers.is_none() => Some(*key),
                        _ => None,
                    })
                    .collect()
            });
            let mut suggestions = suggestions;
            for key in pressed {
                let Some(&(_, k)) = TokenKey::ALL.iter().find(|(egui_key, _)| *egui_key == key) else { continue };
                let before_key = (state.query.clone(), values.len());
                if state.key(k, &suggestions, values, self.free.as_deref()) {
                    ui.input_mut(|i| i.consume_key(Modifiers::NONE, key));
                }
                if before_key != (state.query.clone(), values.len()) {
                    suggestions = if state.open { (self.suggest)(&state.query, values) } else { Vec::new() };
                }
            }
        }

        let label = text::label(ui, text::job(self.label, &Style::ui(theme::TEXT_SM, 600, theme::TEXT_2)), Sense::hover());
        let mut response = label;
        ui.add_space(GAP - ui.spacing().item_spacing.y);

        // the field: chips, then the input
        let chip_style = Style::ui(theme::TEXT_SM, 600, theme::AMBER);
        // the text's box is as wide as the text, at most 150 px (the ellipsis may end short of it)
        let chips: Vec<(String, Arc<Galley>, f32)> = values
            .iter()
            .map(|v| {
                let t = (self.chip)(v);
                let w = text::measure(ui.ctx(), text::job(&t, &chip_style)).x.min(CHIP_TEXT_MAX);
                let g = ui.painter().layout_job(text::truncated(&t, &chip_style, CHIP_TEXT_MAX));
                (t, g, w)
            })
            .collect();
        let mut sizes: Vec<Vec2> = chips.iter().map(|(_, _, w)| Vec2::new(CHIP_PAD + w + CHIP_GAP + REMOVE, CHIP_HEIGHT)).collect();
        sizes.push(Vec2::new(INPUT_MIN_WIDTH, INPUT_HEIGHT));
        let content_w = width - 2.0 * (theme::BORDER + PAD.x);
        let (items, content_h) = flow(&sizes, content_w, GAP, MIN_CONTENT);
        let field_h = content_h + 2.0 * (theme::BORDER + PAD.y);
        let (field, _) = ui.allocate_exact_size(Vec2::new(width, field_h), Sense::hover());
        let mut whole = response.rect.union(field);
        let border = transition_color(ui.ctx(), id.with("border"), if focused { theme::AMBER } else { theme::LINE_CONTROL }, anim::HOVER);
        ui.painter().rect(field, CornerRadius::same(theme::RADIUS_SM), theme::INK, Stroke::new(theme::BORDER, border), StrokeKind::Inside);
        let origin = field.min + Vec2::splat(theme::BORDER) + PAD;
        let mut removed = None;
        for (i, (full, galley, text_w)) in chips.iter().enumerate() {
            let r = items[i].translate(origin.to_vec2());
            ui.painter().rect_filled(r, CornerRadius::same(theme::RADIUS), theme::AMBER_SOFT);
            let text_w = *text_w;
            let text_rect = Rect::from_min_size(Pos2::new(r.left() + CHIP_PAD, r.center().y - galley.size().y / 2.0), galley.size());
            ui.painter().galley(text_rect.min, galley.clone(), Color32::PLACEHOLDER);
            tooltip::show(ui.interact(r, id.with(("chip", i)), Sense::hover()), full);
            let button = Rect::from_min_size(Pos2::new(r.left() + CHIP_PAD + text_w + CHIP_GAP, r.center().y - REMOVE / 2.0), Vec2::splat(REMOVE));
            let b = ui.interact(button, id.with(("remove", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            if b.hovered() {
                ui.painter().rect_filled(button, CornerRadius::same(theme::RADIUS), theme::AMBER_SOFT);
            }
            icons::paint(ui.painter(), button, Icon::X.at(12.0, STROKE_REGULAR), theme::AMBER);
            if b.clicked() {
                removed = Some(values[i].clone());
            }
            response |= b;
        }
        let input = items[items.len() - 1].translate(origin.to_vec2()).shrink2(INPUT_PAD);
        // the pointer over the list keeps the focus in the input (`onmousedown` prevents the blur)
        let over_list = state.list.zip(ui.ctx().pointer_interact_pos()).is_some_and(|(r, p)| r.contains(p));
        let surrender = ui.ctx().options(|o| o.input_options.surrender_focus_on);
        if over_list {
            ui.ctx().options_mut(|o| o.input_options.surrender_focus_on = SurrenderFocusOn::Never);
        }
        let query_before = state.query.clone();
        let edit = text_field::edit(ui, edit_id, &mut state.query, input, &Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1), self.placeholder, false);
        ui.ctx().options_mut(|o| o.input_options.surrender_focus_on = surrender);
        // the focus may have come between passes (a request, Tab) as well as in this one
        let has_focus = edit.has_focus();
        if has_focus && !state.focused {
            state.focus();
        }
        if state.query != query_before {
            state.input();
        }
        if !has_focus && state.focused {
            state.blur();
        }
        state.focused = has_focus;
        // a click in the field's padding focuses the input
        let around = ui.interact(field, id.with("field"), Sense::CLICK);
        if around.clicked() && !edit.has_focus() {
            ui.memory_mut(|m| m.request_focus(edit_id));
        }
        if let Some(v) = removed {
            remove(values, &v);
        }

        let suggestions = if state.open && edit.has_focus() { (self.suggest)(&state.query, values) } else { Vec::new() };
        state.list = None;
        if !suggestions.is_empty() {
            ui.add_space(GAP - ui.spacing().item_spacing.y);
            let row_line = Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1).line_px();
            let row_h = 2.0 * ROW_PAD.y + row_line;
            let list_h = 2.0 * (theme::BORDER + LIST_PAD) + suggestions.len() as f32 * row_h;
            let (list, _) = ui.allocate_exact_size(Vec2::new(width, list_h), Sense::hover());
            whole = whole.union(list);
            state.list = Some(list);
            ui.painter().rect(list, CornerRadius::same(theme::RADIUS), theme::SURFACE_RAISED, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
            let mut hovered = None;
            let mut picked = None;
            for (i, s) in suggestions.iter().enumerate() {
                let row = Rect::from_min_size(list.min + Vec2::splat(theme::BORDER + LIST_PAD) + Vec2::new(0.0, i as f32 * row_h), Vec2::new(width - 2.0 * (theme::BORDER + LIST_PAD), row_h));
                let r = ui.interact(row, id.with(("row", i)), Sense::hover()).on_hover_cursor(CursorIcon::PointingHand);
                if r.hovered() {
                    hovered = Some(i);
                    if ui.input(|inp| inp.pointer.primary_pressed()) {
                        picked = Some(s.value.clone());
                    }
                }
                if i == state.active {
                    ui.painter().rect_filled(row, CornerRadius::same(theme::RADIUS), theme::SURFACE_HOVER);
                }
                paint_row(ui, row, s);
            }
            // `onmouseenter`: the row the pointer has just entered becomes active
            if hovered.is_some() && hovered != state.hovered {
                state.active = hovered.unwrap_or(0);
            }
            state.hovered = hovered;
            if let Some(v) = picked {
                state.add(values, v);
                ui.memory_mut(|m| m.request_focus(edit_id));
            }
        } else {
            state.hovered = None;
        }
        if let Some(hint) = self.hint {
            ui.add_space(GAP - ui.spacing().item_spacing.y);
            whole = whole.union(text::label(ui, text::wrapped(hint, &Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3), width), Sense::hover()).rect);
        }
        ui.data_mut(|d| d.insert_temp(id, state));
        response |= edit;
        // the response covers the whole input: label, field, suggestions and hint
        response.rect = whole;
        if *values != before {
            response.mark_changed();
        }
        response
    }
}

/// A suggestion row's label (cut with an ellipsis) and its detail on the label's baseline.
fn paint_row(ui: &Ui, row: Rect, s: &Suggestion) {
    let content = row.shrink2(ROW_PAD);
    let detail = s.detail.as_deref().map(|d| ui.painter().layout_job(text::job(d, &Style::data(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3))));
    let detail_w = detail.as_ref().map_or(0.0, |d| d.size().x + ROW_GAP);
    let label = ui.painter().layout_job(text::truncated(&s.label, &Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_1), (content.width() - detail_w).max(0.0)));
    let label_baseline = baseline(&label);
    ui.painter().galley(content.min, label, Color32::PLACEHOLDER);
    if let Some(d) = detail {
        let y = content.top() + label_baseline - baseline(&d);
        ui.painter().galley(Pos2::new(content.right() - d.size().x, y), d, Color32::PLACEHOLDER);
    }
}

/// A galley's first baseline from its top.
fn baseline(galley: &Galley) -> f32 {
    galley.rows.first().map_or(0.0, |r| r.pos.y + r.row.glyphs.first().map_or(0.0, |g| g.pos.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestions(values: &[&str]) -> Vec<Suggestion> {
        values.iter().map(|v| Suggestion::new(*v, *v)).collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn free_entry_rules() {
        assert_eq!(three_letters("baw"), Some("BAW".into()));
        assert_eq!(three_letters("BaW"), Some("BAW".into()));
        assert_eq!(three_letters("BA"), None);
        assert_eq!(three_letters("BAWS"), None);
        assert_eq!(three_letters("B4W"), None);
        assert_eq!(three_letters("ÅÄÖ"), None, "only A–Z");
        assert_eq!(three_letters("ſab"), None, "no case folding outside ASCII");
        assert_eq!(type_code("b77"), Some("B77".into()));
        assert_eq!(type_code("a388"), Some("A388".into()));
        assert_eq!(type_code("A"), None);
        assert_eq!(type_code("A3888"), None);
        assert_eq!(type_code("B7-7"), None);
        assert_eq!(type_code("٣٣"), None, "only ASCII digits");
    }

    #[test]
    fn trims_as_javascript_does() {
        assert_eq!(js_trim("  baw \t\n"), "baw");
        assert_eq!(js_trim("\u{feff}baw\u{a0}"), "baw", "BOM and no-break space");
        assert_eq!(js_trim("\u{3000}baw\u{2028}"), "baw");
        assert_eq!(js_trim("\u{85}baw"), "\u{85}baw", "NEL is not JavaScript white space");
    }

    #[test]
    fn down_and_up_cycle_the_active_row() {
        let mut s = State { open: true, ..Default::default() };
        let list = suggestions(&["DLH", "KLM", "RYR"]);
        let mut values = vec![];
        assert!(s.key(TokenKey::Down, &list, &mut values, None));
        assert_eq!(s.active, 1);
        s.key(TokenKey::Down, &list, &mut values, None);
        s.key(TokenKey::Down, &list, &mut values, None);
        assert_eq!(s.active, 0, "wraps");
        s.key(TokenKey::Up, &list, &mut values, None);
        assert_eq!(s.active, 2, "wraps back");
        assert!(!s.key(TokenKey::Down, &[], &mut values, None), "no list: the key is not the input's");
        assert_eq!(s.active, 2);
    }

    #[test]
    fn enter_adds_the_active_suggestion_or_the_typed_value() {
        let free = |q: &str| three_letters(q);
        let mut s = State { open: true, query: "kl".into(), active: 1, ..Default::default() };
        let mut values = strings(&["BAW"]);
        assert!(s.key(TokenKey::Enter, &suggestions(&["KLC", "KLM"]), &mut values, Some(&free)));
        assert_eq!(values, strings(&["BAW", "KLM"]));
        assert_eq!((s.query.as_str(), s.active, s.open), ("", 0, true), "the query clears, the list stays open");
        // no suggestion: the typed code, trimmed and upper-cased
        s.query = " sas ".into();
        s.key(TokenKey::Enter, &[], &mut values, Some(&free));
        assert_eq!(values, strings(&["BAW", "KLM", "SAS"]));
        // invalid typing adds nothing and keeps the query
        s.query = "scandinavian".into();
        assert!(s.key(TokenKey::Enter, &[], &mut values, Some(&free)), "Enter is always the input's");
        assert_eq!(values.len(), 3);
        assert_eq!(s.query, "scandinavian");
        // an active row past the end of a shorter list falls back to the typed value
        s.query = "dlh".into();
        s.active = 4;
        s.key(TokenKey::Enter, &suggestions(&["DLA"]), &mut values, Some(&free));
        assert_eq!(values.last().map(String::as_str), Some("DLH"));
        // a value chosen already is not added twice, but the query still clears
        s.query = "baw".into();
        s.key(TokenKey::Enter, &[], &mut values, Some(&free));
        assert_eq!(values.iter().filter(|v| *v == "BAW").count(), 1);
        assert_eq!(s.query, "");
    }

    #[test]
    fn backspace_in_an_empty_input_removes_the_last_chip() {
        let mut s = State::default();
        let mut values = strings(&["BAW", "SAS"]);
        assert!(!s.key(TokenKey::Backspace, &[], &mut values, None), "the key goes on to the input");
        assert_eq!(values, strings(&["BAW"]));
        s.query = "k".into();
        s.key(TokenKey::Backspace, &[], &mut values, None);
        assert_eq!(values, strings(&["BAW"]), "not while there is text");
    }

    #[test]
    fn escape_closes_an_open_list_only() {
        let mut s = State { open: true, ..Default::default() };
        let mut values = vec![];
        assert!(s.key(TokenKey::Escape, &[], &mut values, None));
        assert!(!s.open);
        assert!(!s.key(TokenKey::Escape, &[], &mut values, None), "a closed list lets Escape through");
        s.input();
        assert!(s.open, "typing opens it again");
    }

    #[test]
    fn chips_and_the_input_wrap_as_flexbox_does() {
        // today's Airlines field (Chromium): 268 px of content, chips 143.98 and 182 wide
        let sizes = [Vec2::new(143.98, 24.0), Vec2::new(182.0, 24.0), Vec2::new(INPUT_MIN_WIDTH, INPUT_HEIGHT)];
        let (rects, height) = flow(&sizes, 268.0, GAP, MIN_CONTENT);
        assert_eq!(rects[0].min, Pos2::new(0.0, 0.0));
        assert_eq!(rects[1].min, Pos2::new(0.0, 28.0), "the second chip does not fit beside the first");
        // 82 px left beside the second chip, less than the input's 84: a line of its own
        assert_eq!(rects[2], Rect::from_min_size(Pos2::new(0.0, 56.0), Vec2::new(268.0, 28.0)));
        assert_eq!(height, 84.0, "Chromium: a 92 px field");
        // an empty field: the input alone, centred in the 34 px minimum
        let (rects, height) = flow(&[Vec2::new(INPUT_MIN_WIDTH, INPUT_HEIGHT)], 268.0, GAP, MIN_CONTENT);
        assert_eq!(height, 34.0);
        assert_eq!(rects[0], Rect::from_min_size(Pos2::new(0.0, 3.0), Vec2::new(268.0, 28.0)));
        // one short chip: the input grows beside it, the chip centred on the input's line
        let (rects, _) = flow(&[Vec2::new(100.0, 24.0), Vec2::new(INPUT_MIN_WIDTH, INPUT_HEIGHT)], 268.0, GAP, MIN_CONTENT);
        assert_eq!(rects[0].min.y, 5.0);
        assert_eq!(rects[1], Rect::from_min_size(Pos2::new(104.0, 3.0), Vec2::new(164.0, 28.0)));
    }

    fn pass(ctx: &egui::Context, time: f64, events: Vec<egui::Event>, mut f: impl FnMut(&mut Ui)) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 400.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| f(ui));
        output.textures_delta.clear();
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    /// The airlines' input as the filter panel builds it, over a few airlines.
    fn airlines<'a>() -> TokenInput<'a> {
        const AIRLINES: [(&str, &str); 4] = [("BAW", "British Airways"), ("DLH", "Lufthansa"), ("KLM", "KLM Royal Dutch Airlines"), ("SAS", "Scandinavian Airlines System")];
        TokenInput::new("airlines", "Airlines", |q, picked| {
            let q = js_trim(q).to_uppercase();
            AIRLINES.iter().filter(|(code, name)| !picked.iter().any(|p| p == code) && (q.is_empty() || code.starts_with(&q) || name.to_uppercase().contains(&q))).map(|(code, name)| Suggestion::new(*code, *name).detail(*code)).collect()
        })
        .free(three_letters)
        .placeholder("Name or code")
    }

    #[test]
    fn typing_arrows_and_enter_through_egui() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut values: Vec<String> = vec![];
        let (mut edit, mut state_id) = (Id::NULL, Id::NULL);
        pass(&ctx, 0.0, vec![], |ui| {
            ui.set_width(286.0);
            state_id = ui.make_persistent_id(Id::NULL.with("airlines"));
            edit = state_id.with("edit");
            airlines().show(ui, &mut values);
        });
        ctx.memory_mut(|m| m.request_focus(edit));
        let draw = |t: f64, events: Vec<egui::Event>, values: &mut Vec<String>| -> bool {
            let mut changed = false;
            pass(&ctx, t, events, |ui| {
                ui.set_width(286.0);
                changed = airlines().show(ui, values).changed();
            });
            changed
        };
        draw(0.1, vec![], &mut values);
        draw(0.2, vec![egui::Event::Text("a".into())], &mut values);
        // "A": British Airways, Lufthansa, KLM Royal Dutch Airlines, Scandinavian Airlines System
        draw(0.3, vec![key(Key::ArrowDown)], &mut values);
        assert!(draw(0.4, vec![key(Key::Enter)], &mut values));
        assert_eq!(values, strings(&["DLH"]));
        // a code typed in full: its suggestion is the first row, active
        draw(0.5, vec![egui::Event::Text("sas".into())], &mut values);
        draw(0.6, vec![key(Key::Enter)], &mut values);
        assert_eq!(values, strings(&["DLH", "SAS"]));
        draw(0.7, vec![key(Key::Backspace)], &mut values);
        assert_eq!(values, strings(&["DLH"]));
        // Escape closes the list and keeps the focus
        draw(0.8, vec![key(Key::Escape)], &mut values);
        draw(0.9, vec![], &mut values);
        assert!(ctx.memory(|m| m.has_focus(edit)));
        let state: State = ctx.data(|d| d.get_temp(state_id)).unwrap_or_default();
        assert!(!state.open);
    }
}
