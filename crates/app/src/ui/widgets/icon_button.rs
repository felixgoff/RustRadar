//! `.icon-button` (app.css:148-175; doc 06 §1.4): a 32 × 32 button holding one Lucide icon, no
//! border, radius 4, transparent, the icon in `--text-2` and centred.
//!
//! **States**, in the CSS's order (a later rule wins where both set a property):
//! - under the pointer (`:hover`): fill `--surface-hover`, icon `--text-1`;
//! - pressed ([`IconButton::pressed`], `[aria-pressed="true"]`: the map controls' open popover,
//!   the tilt): fill `--amber-soft`, icon `--amber`, under the pointer too;
//! - disabled ([`IconButton::enabled`]): icon `--text-3`, the whole button at opacity 0.5, the
//!   default cursor; `:hover` still matches, so a disabled button under the pointer shows the
//!   `--surface-hover` fill at half strength (the zoom buttons at their limits);
//! - keyboard focus: the 2 px `--amber` outline 2 px outside ([`super::button::focus_ring`]).
//!
//! The fill and the icon's colour both ease over 150 ms (`transition: background-color 150ms
//! var(--ease-out), color 150ms var(--ease-out)`, [`super::button::transition_color`]).
//!
//! A component's own colour for the icon ([`IconButton::icon_color`]) holds in every state: the
//! keys button signed in (`.signed-in`: `--live`, MapControls:260-262, whose scoped rule comes
//! after app.css at the same specificity) and the compass needle while it can be pressed
//! (`.compass svg`: `--amber`, MapControls:254-259; the caller passes none while disabled, where
//! the needle inherits). Turn the icon itself with [`icons::Spec::rotated`].
//!
//! Deviation: the 0.5 opacity applies to the fill and the icon each, not to the button as one
//! layer; the two only overlap on a disabled button under the pointer, where the icon reads a
//! quarter of the fill darker.

use egui::{Color32, CornerRadius, CursorIcon, Painter, Rect, Response, Role, Sense, Ui, Vec2, WidgetInfo};

use super::super::icons::{self, Icon};
use super::super::{anim, theme};
use super::button::{focus_ring, transition_color};

/// The button's side, px.
pub const SIZE: f32 = 32.0;
/// `.icon-button:disabled`'s opacity.
pub const DISABLED_OPACITY: f32 = 0.5;

/// An `.icon-button`. Build it, then [`IconButton::show`] it (or `ui.add(button)`).
#[derive(Debug, Clone, PartialEq)]
pub struct IconButton {
    spec: icons::Spec,
    pressed: Option<bool>,
    enabled: bool,
    icon_color: Option<Color32>,
    tooltip: Option<String>,
    label: String,
}

impl IconButton {
    /// A button showing `spec` (the map controls' 18 px icons, the panels' 18 px close `X`, the
    /// filters' and dialogs' 16 px `X`; stroke 1.75 in every use), named `label` (`aria-label`).
    pub fn new(spec: icons::Spec, label: impl Into<String>) -> IconButton {
        IconButton { spec, pressed: None, enabled: true, icon_color: None, tooltip: None, label: label.into() }
    }

    /// `icon` at `size` with stroke 1.75.
    pub fn of(icon: Icon, size: f32, label: impl Into<String>) -> IconButton {
        IconButton::new(icon.at(size, icons::STROKE_THIN), label)
    }

    /// A toggle (`aria-pressed`): on shows the pressed look.
    pub fn pressed(mut self, on: bool) -> IconButton {
        self.pressed = Some(on);
        self
    }

    /// Whether it can be clicked (`disabled` when false).
    pub fn enabled(mut self, enabled: bool) -> IconButton {
        self.enabled = enabled;
        self
    }

    /// The icon's colour in every state (a component's own rule).
    pub fn icon_color(mut self, color: Color32) -> IconButton {
        self.icon_color = Some(color);
        self
    }

    /// The `title` (shown by [`super::tooltip::show`]).
    pub fn tooltip(mut self, text: impl Into<String>) -> IconButton {
        self.tooltip = Some(text.into());
        self
    }

    /// What it looks like in a state: the fill, the icon's colour and the opacity.
    pub fn look(&self, hovered: bool, enabled: bool) -> (Color32, Color32, f32) {
        let (mut fill, mut color) = (Color32::TRANSPARENT, theme::TEXT_2);
        if hovered {
            fill = theme::SURFACE_HOVER;
            color = theme::TEXT_1;
        }
        if self.pressed == Some(true) {
            fill = theme::AMBER_SOFT;
            color = theme::AMBER;
        }
        let mut opacity = 1.0;
        if !enabled {
            color = theme::TEXT_3;
            opacity = DISABLED_OPACITY;
        }
        (fill, color, opacity)
    }

    /// Lays the button out in `ui`, draws it and answers for it.
    pub fn show(self, ui: &mut Ui) -> Response {
        let enabled = self.enabled && ui.is_enabled();
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(SIZE), sense);
        response.widget_info(|| match self.pressed {
            Some(on) => WidgetInfo::selected(Role::Button, enabled, on, &self.label),
            None => WidgetInfo::labeled(Role::Button, enabled, &self.label),
        });
        let (fill, color, opacity) = self.look(response.hovered(), enabled);
        let fill = transition_color(ui.ctx(), response.id.with("fill"), fill, anim::HOVER);
        let color = transition_color(ui.ctx(), response.id.with("color"), color, anim::HOVER);
        if ui.is_rect_visible(rect) {
            let mut painter = ui.painter().clone();
            painter.multiply_opacity(opacity);
            paint(&painter, rect, self.spec, fill, self.icon_color.unwrap_or(color));
            if response.has_focus() {
                focus_ring(ui.painter(), rect, theme::RADIUS);
            }
        }
        if enabled {
            response = response.on_hover_cursor(CursorIcon::PointingHand);
        }
        if let Some(tip) = &self.tooltip {
            response = super::tooltip::show(response, tip);
        }
        response
    }
}

impl egui::Widget for IconButton {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui)
    }
}

/// Draws an icon button in `rect`: `fill` with radius 4, `spec` centred in `color`.
pub fn paint(painter: &Painter, rect: Rect, spec: icons::Spec, fill: Color32, color: Color32) {
    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
    icons::paint(painter, rect, spec, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_follow_the_css_order() {
        let b = IconButton::of(Icon::Layers, 18.0, "Map layers");
        assert_eq!(b.look(false, true), (Color32::TRANSPARENT, theme::TEXT_2, 1.0));
        assert_eq!(b.look(true, true), (theme::SURFACE_HOVER, theme::TEXT_1, 1.0));
        let open = b.clone().pressed(true);
        assert_eq!(open.look(false, true), (theme::AMBER_SOFT, theme::AMBER, 1.0));
        assert_eq!(open.look(true, true), (theme::AMBER_SOFT, theme::AMBER, 1.0), "pressed wins over hover");
        // disabled: --text-3 at half opacity; :hover's fill still applies
        assert_eq!(b.look(false, false), (Color32::TRANSPARENT, theme::TEXT_3, 0.5));
        assert_eq!(b.look(true, false), (theme::SURFACE_HOVER, theme::TEXT_3, 0.5));
    }

    #[test]
    fn it_is_32_square() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut rect = Rect::NOTHING;
        ctx.run_ui(egui::RawInput::default(), |ui| rect = IconButton::of(Icon::Plus, 18.0, "Zoom in").enabled(false).show(ui).rect)
            .textures_delta
            .clear();
        assert_eq!(rect.size(), Vec2::splat(32.0));
    }
}
