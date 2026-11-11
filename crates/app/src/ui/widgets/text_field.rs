//! A single-line text field (doc 06 §11 item 1, §20 item 2, §5.1): the filter panel's
//! `Callsign, flight or registration`, the keys dialog's fields, and the frame the search box
//! (U8) and the token input build on.
//!
//! - **Frame**: `--ink` fill, a 1 px `--line-control` border, `--amber` while focused (150 ms),
//!   radius 4, side padding 12. CSS gives `height: 34px` to the content box, so the field is
//!   36 px tall with its border ([`HEIGHT`]); the search box's `height: 36px` makes 38
//!   ([`SEARCH_HEIGHT`]).
//! - **Text**: 13 px (the search box 14) in `--text-1`, set through [`text::job`] like every UI
//!   text; the placeholder in `--text-3`, cut with an ellipsis; the caret and the selection in
//!   `--amber` (`theme::apply`). A password shows a bullet (U+2022) per character, as Chromium
//!   draws `-webkit-text-security: disc`.
//! - **Adornments**: an optional leading icon 8 px before the text (the search box's `Search`
//!   16/1.75, `--text-3`, `--text-2` while focused, at once: only the border has a transition),
//!   and room kept at the right end for one the caller draws ([`Field::trailing`]: a key cap, a
//!   spinner, a clear button). The search box's input keeps Chromium's `padding: 1px 2px`
//!   ([`TextField::native_padding`]); the filter and keys fields set their own.
//! - **Behaviour**: egui's `TextEdit` (so the platform's text input and IME work as for any egui
//!   field), single line. A click anywhere in the frame focuses it (the input's padding, or the
//!   search box's `<label>`). Enter does nothing here (no form): the caller reads it. Escape
//!   keeps the focus, as in a browser's text input; the search box blurs itself.

use std::hash::Hash;
use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Align, Align2, Color32, CornerRadius, CursorIcon, EventFilter, Frame, Galley, Id, Layout, Pos2, Rect, Response, Sense, Stroke, StrokeKind, TextBuffer, TextEdit, Ui, UiBuilder, Vec2};

use super::button::transition_color;
use crate::ui::anim;
use crate::ui::icons::{self, Spec};
use crate::ui::text::{self, Style};
use crate::ui::theme;

/// A field's outer height: `height: 34px` on the content box and a 1 px border.
pub const HEIGHT: f32 = 34.0 + 2.0 * theme::BORDER;
/// The search box's outer height (`height: 36px`).
pub const SEARCH_HEIGHT: f32 = 36.0 + 2.0 * theme::BORDER;
/// Side padding.
pub const PAD_X: f32 = theme::SPACE_3;
/// Between the icon, the text and the trailing adornment.
pub const GAP: f32 = theme::SPACE_2;
/// Chromium's own padding round an `<input>` that does not set one.
pub const NATIVE_PADDING: Vec2 = Vec2::new(2.0, 1.0);
/// The character a password shows.
pub const BULLET: char = '\u{2022}';

/// What a shown field gives back.
#[derive(Debug, Clone)]
pub struct Field {
    /// The text edit's response: `changed()`, `has_focus()`, `lost_focus()`.
    pub response: Response,
    /// The whole field, border included.
    pub frame: Rect,
    /// The room kept at the right end for an adornment (zero wide when none was asked for).
    pub trailing: Rect,
    pub focused: bool,
}

/// A text field over a `String`.
pub struct TextField<'a> {
    id_salt: Id,
    text: &'a mut String,
    placeholder: &'a str,
    size: f32,
    height: f32,
    password: bool,
    icon: Option<Spec>,
    trailing: f32,
    width: Option<f32>,
    native_padding: bool,
}

impl<'a> TextField<'a> {
    /// The filter panel's field: 13 px, 36 px tall.
    pub fn new(id_salt: impl Hash + std::fmt::Debug, text: &'a mut String) -> Self {
        TextField { id_salt: Id::NULL.with(id_salt), text, placeholder: "", size: theme::TEXT_MD, height: HEIGHT, password: false, icon: None, trailing: 0.0, width: None, native_padding: false }
    }

    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = placeholder;
        self
    }

    /// The text's size (the search box: 14).
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// The outer height, border included.
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
    }

    /// A leading icon (`--text-3`, `--text-2` while focused).
    pub fn icon(mut self, icon: Spec) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Room for an adornment at the right end, `width` wide, [`GAP`] after the text.
    pub fn trailing(mut self, width: f32) -> Self {
        self.trailing = width;
        self
    }

    /// Keep Chromium's own `padding: 1px 2px` round the input inside the frame (the search box's
    /// `<input>` does not reset it, so its text starts 2 px after the icon's gap).
    pub fn native_padding(mut self) -> Self {
        self.native_padding = true;
        self
    }

    /// The outer width (default: the available width).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Field {
        let id = ui.make_persistent_id(self.id_salt);
        let edit_id = id.with("edit");
        let width = self.width.unwrap_or_else(|| ui.available_width());
        let (frame, _) = ui.allocate_exact_size(Vec2::new(width, self.height), Sense::hover());
        // the padding and the label focus the input too; the edit, drawn after, wins where it is
        let around = ui.interact(frame, id.with("frame"), Sense::CLICK).on_hover_cursor(CursorIcon::Text);
        let focused = ui.memory(|m| m.has_focus(edit_id));
        let border = transition_color(ui.ctx(), id.with("border"), if focused { theme::AMBER } else { theme::LINE_CONTROL }, anim::HOVER);
        ui.painter().rect(frame, CornerRadius::same(theme::RADIUS_SM), theme::INK, Stroke::new(theme::BORDER, border), StrokeKind::Inside);

        let inner = frame.shrink2(Vec2::new(theme::BORDER + PAD_X, theme::BORDER));
        let mut left = inner.left();
        if let Some(icon) = self.icon {
            let color = if focused { theme::TEXT_2 } else { theme::TEXT_3 };
            icons::paint(ui.painter(), Rect::from_center_size(Pos2::new(left + icon.size / 2.0, inner.center().y), icon.size_vec()), icon, color);
            left += icon.size + GAP;
        }
        let right = if self.trailing > 0.0 { inner.right() - self.trailing - GAP } else { inner.right() };
        let trailing = Rect::from_min_max(Pos2::new(inner.right() - self.trailing, inner.top()), inner.max);
        let mut text_rect = Rect::from_min_max(Pos2::new(left, inner.top()), Pos2::new(right.max(left), inner.bottom()));
        if self.native_padding {
            text_rect = text_rect.shrink2(NATIVE_PADDING);
        }

        let style = Style::ui(self.size, theme::WEIGHT_REGULAR, theme::TEXT_1);
        let response = edit(ui, edit_id, self.text, text_rect, &style, self.placeholder, self.password);
        // after the edit, which lets go of the focus on a click outside itself
        if around.clicked() {
            ui.memory_mut(|m| m.request_focus(edit_id));
        }
        Field { focused: response.has_focus(), response, frame, trailing }
    }
}

/// The bare text edit in `rect` (no frame): `text` in `style` centred on the rect's height, the
/// `placeholder` in `--text-3` (cut with an ellipsis) while it is empty, a bullet per character
/// for a password; Enter left to the caller, Escape keeping the focus. The token input's field
/// uses it too.
pub fn edit(ui: &mut Ui, id: Id, text: &mut String, rect: Rect, style: &Style, placeholder: &str, password: bool) -> Response {
    if text.is_empty() && !placeholder.is_empty() {
        let galley = ui.painter().layout_job(text::truncated(placeholder, &style.color(theme::TEXT_3), rect.width()));
        ui.painter().galley(Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.0), galley, Color32::PLACEHOLDER);
    }
    let style = *style;
    let mut layouter = move |ui: &Ui, buffer: &dyn TextBuffer, _wrap: f32| -> Arc<Galley> { ui.fonts_mut(|f| f.layout_job(job(buffer.as_str(), &style, password))) };
    let edit = TextEdit::singleline(text)
        .id(id)
        .frame(Frame::NONE)
        .desired_width(rect.width())
        .min_size(rect.size())
        .align(Align2::LEFT_CENTER)
        .layouter(&mut layouter)
        .return_key(None)
        .event_filter(EventFilter { horizontal_arrows: true, vertical_arrows: true, escape: true, tab: false });
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    child.add(edit)
}

/// The text as laid out: `style`, or a bullet per character for a password.
fn job(text: &str, style: &Style, password: bool) -> LayoutJob {
    if password {
        text::job(&BULLET.to_string().repeat(text.chars().count()), style)
    } else {
        text::job(text, style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    fn pass(ctx: &egui::Context, time: f64, events: Vec<egui::Event>, mut f: impl FnMut(&mut Ui)) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
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

    #[test]
    fn a_click_focuses_typing_edits_escape_keeps_the_focus() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut text = String::new();
        let field = std::cell::RefCell::new(None);
        let draw = |t: f64, events: Vec<egui::Event>, text: &mut String| {
            pass(&ctx, t, events, |ui| {
                ui.set_width(286.0);
                *field.borrow_mut() = Some(TextField::new("f", text).placeholder("e.g. BAW, LH400, D-AIM").show(ui));
            })
        };
        draw(0.0, vec![], &mut text);
        let frame = field.borrow().as_ref().expect("shown").frame;
        assert_eq!(frame.height(), 36.0, "34 px of content and the border, as Chromium sizes it");
        // a click on the padding, left of the text
        let at = Pos2::new(frame.left() + 4.0, frame.center().y);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        draw(0.1, vec![egui::Event::PointerMoved(at)], &mut text);
        draw(0.2, vec![press(true)], &mut text);
        draw(0.3, vec![press(false)], &mut text);
        draw(0.4, vec![], &mut text);
        assert!(field.borrow().as_ref().expect("shown").focused, "a click in the padding focuses the field");
        draw(0.5, vec![egui::Event::Text("lh400".into())], &mut text);
        assert_eq!(text, "lh400");
        draw(0.6, vec![key(Key::Escape)], &mut text);
        draw(0.7, vec![], &mut text);
        assert!(field.borrow().as_ref().expect("shown").focused, "Escape keeps the focus");
        draw(0.8, vec![key(Key::Enter)], &mut text);
        draw(0.9, vec![], &mut text);
        assert!(field.borrow().as_ref().expect("shown").focused, "Enter keeps the focus");
        assert_eq!(text, "lh400", "and adds no line");
    }

    #[test]
    fn a_password_is_bullets() {
        let style = Style::body();
        assert_eq!(job("hunter22", &style, true).text, "••••••••");
        assert_eq!(job("åäö", &style, true).text, "•••");
        assert_eq!(job("åäö", &style, false).text, "åäö");
    }
}
