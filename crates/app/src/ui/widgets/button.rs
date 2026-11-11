//! `.button` (app.css:177-199; doc 06 §1.4): the bordered text button with an optional icon, the
//! toggle that is on (`aria-pressed="true"`), and the variants components build on it.
//!
//! **Look** ([`Button`]): `display: inline-flex; align-items: center; gap: 8px`, 32 px high
//! (border box), `padding: 0 12px` inside a 1 px `--line-control` border, radius 4, transparent;
//! 13 px Overpass at weight 550 in the colour it inherits (`--text-1` everywhere today,
//! [`Button::color`] otherwise). Its parts ([`Part`]) are laid out left to right 8 px apart, each
//! centred on the button's middle: a Lucide icon (14 px, stroke 1.75 in every use), a text (one
//! row, its line box 13 × 1.4 px), the spinner (`LoaderCircle` 14/2, [`super::spinner`]) and the
//! Filters button's active-count pill (`.active-count`, +page:1836-1847: at least 18 px wide, 18
//! high, `0 4px` padding, radius 9, `--amber` with 11 px weight 700 `--amber-ink` text).
//!
//! **States**, in the CSS's order (a later rule wins):
//! - under the pointer (`:hover`): fill `--surface-hover`;
//! - pressed toggle ([`Button::pressed`], `[aria-pressed="true"]`): border `--amber`, fill
//!   `--amber-soft`, text and icon `--amber`, under the pointer too;
//! - `.primary` ([`Button::primary`]: KeysDialog:311-320, UpdateToast:84-95): filled `--amber`
//!   to its edge (a transparent or `--amber` border), `--amber-ink` text, the content centred; the
//!   update toast's lightens to [`theme::AMBER_HOVER`] under the pointer when enabled
//!   ([`Button::primary_hover`]), the keys dialog's stays;
//! - disabled ([`Button::enabled`]): app.css gives `.button:disabled` no look of its own (the
//!   author's `color: inherit` beats the browser's grey, `:hover` still matches and the cursor
//!   stays a pointing hand); the components that dim theirs (the keys dialog's primary at 0.5,
//!   the update toast's buttons at 0.7) also take the default cursor: [`Button::dim_when_disabled`].
//! - compact ([`Button::compact`], FlightPanel:1310-1314 and the ship's and train's): 32 px wide,
//!   no padding, the content centred (the Follow button while the status needs the room);
//! - keyboard focus (`:focus-visible`, app.css:91-94): a 2 px `--amber` outline 2 px outside.
//!
//! Only the fill eases (`transition: background-color 150ms var(--ease-out)`); the border, text
//! and icon change at once. [`transition_color`] runs that transition as CSS does (from where the
//! colour stands, premultiplied sRGB, the reversing shortening), for the other widgets too.

use egui::{
    Color32, Context, CornerRadius, CursorIcon, Id, Painter, Pos2, Rect, Response, Role, Sense, Stroke, StrokeKind, Ui, Vec2,
    WidgetInfo,
};

use super::super::icons::{self, Icon};
use super::super::text::{self, Style};
use super::super::{anim, theme};
use super::spinner;

/// The button's height (border box), px.
pub const HEIGHT: f32 = 32.0;
/// `padding: 0 var(--space-3)`, inside the border.
pub const PADDING_X: f32 = theme::SPACE_3;
/// `gap: var(--space-2)` between the parts.
pub const GAP: f32 = theme::SPACE_2;
/// The text's size (`--text-md`).
pub const TEXT_SIZE: f32 = theme::TEXT_MD;
/// The size every button icon is drawn at today.
pub const ICON_SIZE: f32 = 14.0;
/// `.button.compact`'s width.
pub const COMPACT_WIDTH: f32 = 32.0;
/// `.active-count`: height and least width, px.
pub const COUNT_SIZE: f32 = 18.0;
/// `.active-count`: padding either side of its number, px.
pub const COUNT_PADDING_X: f32 = theme::SPACE_1;
/// `.active-count`'s weight.
pub const COUNT_WEIGHT: u16 = 700;
/// `.active-count`'s radius.
pub const COUNT_RADIUS: u8 = theme::RADIUS_PILL;

/// The text style of a button's label in `color`.
pub const fn label_style(color: Color32) -> Style {
    Style::ui(TEXT_SIZE, theme::WEIGHT_BUTTON, color)
}

/// The text style of the active-count pill's number.
pub const fn count_style() -> Style {
    Style::ui(theme::TEXT_XS, COUNT_WEIGHT, theme::AMBER_INK)
}

/// One flex item of a button.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// A Lucide icon in the button's colour.
    Icon(icons::Spec),
    /// A text on one row.
    Text(String),
    /// The 900 ms spinner (`LoaderCircle` at this size, stroke 2) in the button's colour.
    Spinner(f32),
    /// The active-count pill with this number.
    Count(u32),
}

/// The filled variants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// `.button`: bordered, transparent.
    Plain,
    /// `.primary`: filled `--amber`, `--amber-ink` text, content centred; `hover` is the fill
    /// under the pointer while enabled (none: it stays `--amber`).
    Primary { hover: Option<Color32> },
}

/// A `.button`. Build it, then [`Button::show`] it (or `ui.add(button)`).
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    parts: Vec<Part>,
    kind: Kind,
    pressed: Option<bool>,
    enabled: bool,
    disabled_opacity: Option<f32>,
    width: Option<f32>,
    compact: bool,
    color: Color32,
    tooltip: Option<String>,
    label: Option<String>,
}

impl Default for Button {
    fn default() -> Self {
        Button {
            parts: Vec::new(),
            kind: Kind::Plain,
            pressed: None,
            enabled: true,
            disabled_opacity: None,
            width: None,
            compact: false,
            color: theme::TEXT_1,
            tooltip: None,
            label: None,
        }
    }
}

impl Button {
    /// An empty button: add its parts in order.
    pub fn new() -> Button {
        Button::default()
    }

    /// A button with one text.
    pub fn text_only(text: impl Into<String>) -> Button {
        Button::new().text(text)
    }

    /// Adds an icon (`<Icon size={14} strokeWidth={1.75} />` in every use today).
    pub fn icon(mut self, spec: icons::Spec) -> Button {
        self.parts.push(Part::Icon(spec));
        self
    }

    /// Adds a text. Leading and trailing white space is dropped, as a flex item's text loses it.
    pub fn text(mut self, text: impl Into<String>) -> Button {
        let text: String = text.into();
        self.parts.push(Part::Text(text.trim().to_string()));
        self
    }

    /// Adds the spinner at `size` px (14 in the keys dialog's Save).
    pub fn spinner(mut self, size: f32) -> Button {
        self.parts.push(Part::Spinner(size));
        self
    }

    /// Adds the active-count pill with `n`, when `n` is not 0 (`{#if n}`).
    pub fn count(mut self, n: u32) -> Button {
        if n != 0 {
            self.parts.push(Part::Count(n));
        }
        self
    }

    /// A toggle (`aria-pressed`): on shows the pressed look.
    pub fn pressed(mut self, on: bool) -> Button {
        self.pressed = Some(on);
        self
    }

    /// The keys dialog's `.primary`: filled `--amber`, the same under the pointer.
    pub fn primary(mut self) -> Button {
        self.kind = Kind::Primary { hover: None };
        self
    }

    /// The update toast's `.button.primary`: filled `--amber`, `hover` under the pointer while
    /// enabled ([`theme::AMBER_HOVER`]).
    pub fn primary_hover(mut self, hover: Color32) -> Button {
        self.kind = Kind::Primary { hover: Some(hover) };
        self
    }

    /// Whether it can be clicked (`disabled` when false).
    pub fn enabled(mut self, enabled: bool) -> Button {
        self.enabled = enabled;
        self
    }

    /// While disabled, drawn at `opacity` with the default cursor (the keys dialog's primary 0.5,
    /// the update toast's buttons 0.7). Without it a disabled button looks enabled, as app.css
    /// has it.
    pub fn dim_when_disabled(mut self, opacity: f32) -> Button {
        self.disabled_opacity = Some(opacity);
        self
    }

    /// `.button.compact`: 32 px wide, no padding, the content centred.
    pub fn compact(mut self) -> Button {
        self.compact = true;
        self
    }

    /// A fixed width (border box), the content where the variant puts it: centred for
    /// [`Button::primary`] and [`Button::compact`], else after the padding.
    pub fn width(mut self, width: f32) -> Button {
        self.width = Some(width);
        self
    }

    /// The colour the button inherits for its text and icons (`--text-1` by default).
    pub fn color(mut self, color: Color32) -> Button {
        self.color = color;
        self
    }

    /// The `title` (shown by [`super::tooltip::show`]).
    pub fn tooltip(mut self, text: impl Into<String>) -> Button {
        self.tooltip = Some(text.into());
        self
    }

    /// The accessible name (`aria-label`) when the texts do not say it (a compact Follow).
    pub fn label(mut self, label: impl Into<String>) -> Button {
        self.label = Some(label.into());
        self
    }

    /// The parts in order.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    fn centred(&self) -> bool {
        self.compact || matches!(self.kind, Kind::Primary { .. })
    }

    fn padding(&self) -> f32 {
        if self.compact { 0.0 } else { PADDING_X }
    }

    /// The width of each part in `ctx`, px.
    fn part_widths(&self, ctx: &Context) -> Vec<f32> {
        self.parts
            .iter()
            .map(|part| match part {
                Part::Icon(spec) => spec.size,
                Part::Spinner(size) => *size,
                Part::Text(t) => text::measure(ctx, text::job(t, &label_style(self.color))).x,
                Part::Count(n) => count_width(ctx, *n),
            })
            .collect()
    }

    /// The width its parts take with their gaps, px.
    pub fn content_width(&self, ctx: &Context) -> f32 {
        let widths = self.part_widths(ctx);
        widths.iter().sum::<f32>() + GAP * widths.len().saturating_sub(1) as f32
    }

    /// The button's size (border box): its fixed width, 32 when compact, else its content with
    /// the padding and the border; 32 px high.
    pub fn size(&self, ctx: &Context) -> Vec2 {
        let width = match (self.width, self.compact) {
            (Some(w), _) => w,
            (None, true) => COMPACT_WIDTH,
            (None, false) => self.content_width(ctx) + 2.0 * (self.padding() + theme::BORDER),
        };
        Vec2::new(width, HEIGHT)
    }

    /// What it looks like in a state.
    pub fn look(&self, hovered: bool, enabled: bool) -> Look {
        let pressed = self.pressed == Some(true);
        let (mut fill, mut border, mut color) = (Color32::TRANSPARENT, Some(theme::LINE_CONTROL), self.color);
        if hovered {
            fill = theme::SURFACE_HOVER;
        }
        if pressed {
            border = Some(theme::AMBER);
            fill = theme::AMBER_SOFT;
            color = theme::AMBER;
        }
        if let Kind::Primary { hover } = self.kind {
            border = None;
            fill = match hover {
                Some(h) if hovered && enabled => h,
                _ => theme::AMBER,
            };
            color = theme::AMBER_INK;
        }
        let opacity = if enabled { 1.0 } else { self.disabled_opacity.unwrap_or(1.0) };
        Look { fill, border, color, opacity }
    }

    /// Lays the button out in `ui`, draws it and answers for it (clicks, hover, focus).
    pub fn show(self, ui: &mut Ui) -> Response {
        let enabled = self.enabled && ui.is_enabled();
        let size = self.size(ui.ctx());
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let (rect, mut response) = ui.allocate_exact_size(size, sense);
        let accessible = self.label.clone().unwrap_or_else(|| self.texts());
        response.widget_info(|| match self.pressed {
            Some(on) => WidgetInfo::selected(Role::Button, enabled, on, &accessible),
            None => WidgetInfo::labeled(Role::Button, enabled, &accessible),
        });
        let hovered = response.hovered();
        let look = self.look(hovered, enabled);
        // only the fill eases
        let fill = transition_color(ui.ctx(), response.id.with("fill"), look.fill, anim::HOVER);
        if ui.is_rect_visible(rect) {
            let mut painter = ui.painter().clone();
            painter.multiply_opacity(look.opacity);
            self.paint(&painter, rect, Look { fill, ..look });
            if response.has_focus() {
                focus_ring(ui.painter(), rect, theme::RADIUS);
            }
        }
        if enabled || self.disabled_opacity.is_none() {
            response = response.on_hover_cursor(CursorIcon::PointingHand);
        }
        if let Some(tip) = &self.tooltip {
            response = super::tooltip::show(response, tip);
        }
        response
    }

    /// Draws the button in `rect` as `look` says (the fill as it stands now).
    pub fn paint(&self, painter: &Painter, rect: Rect, look: Look) {
        let radius = CornerRadius::same(theme::RADIUS);
        painter.rect_filled(rect, radius, look.fill);
        if let Some(border) = look.border {
            painter.rect_stroke(rect, radius, Stroke::new(theme::BORDER, border), StrokeKind::Inside);
        }
        let ctx = painter.ctx();
        let widths = self.part_widths(ctx);
        let content = widths.iter().sum::<f32>() + GAP * widths.len().saturating_sub(1) as f32;
        let mut x = if self.centred() { rect.center().x - content / 2.0 } else { rect.min.x + theme::BORDER + self.padding() };
        let middle = rect.center().y;
        for (part, w) in self.parts.iter().zip(widths) {
            let slot = Rect::from_min_max(Pos2::new(x, rect.min.y), Pos2::new(x + w, rect.max.y));
            match part {
                Part::Icon(spec) => icons::paint(painter, slot, *spec, look.color),
                Part::Spinner(size) => spinner::paint(painter, slot, *size, look.color),
                Part::Text(t) => {
                    let galley = painter.layout_job(text::job(t, &label_style(look.color)));
                    painter.galley(Pos2::new(x, middle - galley.size().y / 2.0), galley, look.color);
                }
                Part::Count(n) => paint_count(painter, Pos2::new(x, middle), *n),
            }
            x += w + GAP;
        }
    }

    fn texts(&self) -> String {
        let texts: Vec<&str> = self
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        texts.join(" ")
    }
}

impl egui::Widget for Button {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui)
    }
}

/// A button's colours in one state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub fill: Color32,
    /// The 1 px border, none for the filled variants.
    pub border: Option<Color32>,
    /// Text and icons.
    pub color: Color32,
    /// The whole button's opacity.
    pub opacity: f32,
}

/// The active-count pill's width for `n`.
pub fn count_width(ctx: &Context, n: u32) -> f32 {
    let number = text::measure(ctx, text::job(&n.to_string(), &count_style())).x;
    (number + 2.0 * COUNT_PADDING_X).max(COUNT_SIZE)
}

/// Draws the active-count pill with `n`, its left edge at `left`'s x, centred on its y.
pub fn paint_count(painter: &Painter, left: Pos2, n: u32) {
    let width = count_width(painter.ctx(), n);
    let pill = Rect::from_min_size(Pos2::new(left.x, left.y - COUNT_SIZE / 2.0), Vec2::new(width, COUNT_SIZE));
    painter.rect_filled(pill, CornerRadius::same(COUNT_RADIUS), theme::AMBER);
    let galley = painter.layout_job(text::job(&n.to_string(), &count_style()));
    painter.galley(pill.center() - galley.size() / 2.0, galley, theme::AMBER_INK);
}

/// `:focus-visible`: a 2 px `--amber` outline, 2 px outside `rect`, following its `radius`.
pub fn focus_ring(painter: &Painter, rect: Rect, radius: u8) {
    let offset = theme::FOCUS_RING_OFFSET + theme::FOCUS_RING / 2.0;
    let r = f32::from(radius) + offset;
    painter.rect_stroke(rect.expand(offset), CornerRadius::same(r.round() as u8), Stroke::new(theme::FOCUS_RING, theme::AMBER), StrokeKind::Middle);
}

/// An icon as the buttons draw it: `icon` at 14 px, stroke 1.75.
pub const fn icon(icon: Icon) -> icons::Spec {
    icon.at(ICON_SIZE, icons::STROKE_THIN)
}

/// A running CSS transition of one colour.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ColorTransition {
    from: Color32,
    to: Color32,
    reversing_adjusted_from: Color32,
    shortening: f32,
    start: f64,
    seconds: f32,
}

impl ColorTransition {
    fn progress(&self, now: f64) -> f32 {
        if self.seconds <= 0.0 {
            return 1.0;
        }
        (((now - self.start) / f64::from(self.seconds)) as f32).clamp(0.0, 1.0)
    }

    fn at(&self, now: f64) -> Color32 {
        self.from.lerp_to_gamma(self.to, anim::ease_out(self.progress(now)))
    }
}

/// The colour of `id`'s transition towards `target` over `seconds` with `--ease-out`, as CSS runs
/// `transition: <colour property> <seconds> var(--ease-out)`: the first call stands at `target`;
/// a new target starts from where the colour stands; going back to where a running transition
/// began takes only as long as the way come (CSS Transitions §3); colours mix in premultiplied
/// sRGB (CSS Color 4 for legacy colours). Keeps egui repainting while it runs; reduced motion
/// ([`anim::reduced_motion`]) makes it instant.
pub fn transition_color(ctx: &Context, id: Id, target: Color32, seconds: f32) -> Color32 {
    let seconds = anim::duration(ctx, seconds);
    let id = id.with("rustradar::widgets::transition_color");
    let now = ctx.input(|i| i.time);
    let previous: Option<ColorTransition> = ctx.data(|d| d.get_temp(id));
    let standing = ColorTransition { from: target, to: target, reversing_adjusted_from: target, shortening: 1.0, start: now, seconds: 0.0 };
    let next = match previous {
        None => standing,
        Some(t) if t.to == target => t,
        // reduced motion's 0.01 ms is over before the frame is shown
        Some(_) if seconds <= anim::REDUCED => standing,
        Some(t) => {
            let current = t.at(now);
            let running = t.progress(now) < 1.0;
            if running && target == t.reversing_adjusted_from {
                let eased = anim::ease_out(t.progress(now));
                let shortening = (eased * t.shortening + (1.0 - t.shortening)).abs().clamp(0.0, 1.0);
                ColorTransition { from: current, to: target, reversing_adjusted_from: t.to, shortening, start: now, seconds: seconds * shortening }
            } else {
                ColorTransition { from: current, to: target, reversing_adjusted_from: current, shortening: 1.0, start: now, seconds }
            }
        }
    };
    if previous != Some(next) {
        ctx.data_mut(|d| d.insert_temp(id, next));
    }
    if next.progress(now) < 1.0 {
        ctx.request_repaint();
    }
    next.at(now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{RectShape, Shape};
    use egui::{Event, PointerButton, RawInput};

    /// A themed context that has run once (its fonts are loaded from the first pass).
    fn ctx() -> Context {
        let ctx = Context::default();
        theme::apply(&ctx);
        ctx.run_ui(RawInput::default(), |_| {}).textures_delta.clear();
        ctx
    }

    fn input(time: f64, pointer: Option<Pos2>) -> RawInput {
        let mut raw = RawInput {
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            ..Default::default()
        };
        if let Some(p) = pointer {
            raw.events.push(Event::PointerMoved(p));
        }
        raw
    }

    /// Runs one frame with `f` in an area at (100, 100); the rect it returned and the rect shapes drawn.
    fn frame(ctx: &Context, raw: RawInput, f: impl FnOnce(&mut Ui) -> Response) -> (Response, Vec<RectShape>) {
        let mut f = Some(f);
        let mut out = None;
        let mut output = ctx.run_ui(raw, |ui| {
            egui::Area::new(Id::unique("area")).fixed_pos(Pos2::new(100.0, 100.0)).show(ui.ctx(), |ui| {
                if let Some(f) = f.take() {
                    out = Some(f(ui));
                }
            });
        });
        output.textures_delta.clear();
        let rects = output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                Shape::Rect(r) => Some(r),
                _ => None,
            })
            .collect();
        (out.expect("drawn"), rects)
    }

    #[test]
    fn a_text_button_is_32_high_with_12_px_padding_inside_its_border() {
        let ctx = ctx();
        let (r, _) = frame(&ctx, input(0.0, None), |ui| Button::text_only("Show all trains").show(ui));
        let text_w = text::measure(&ctx, text::job("Show all trains", &label_style(theme::TEXT_1))).x;
        assert_eq!(r.rect.height(), 32.0);
        assert!((r.rect.width() - (text_w + 26.0)).abs() < 1e-3, "{} vs {}", r.rect.width(), text_w + 26.0);
    }

    #[test]
    fn parts_are_8_px_apart() {
        let ctx = ctx();
        let b = Button::new().icon(icon(Icon::Funnel)).text("Filters").count(3);
        let text_w = text::measure(&ctx, text::job("Filters", &label_style(theme::TEXT_1))).x;
        let pill = count_width(&ctx, 3);
        assert_eq!(pill, 18.0, "one digit fits the 18 px minimum");
        assert!((b.content_width(&ctx) - (14.0 + 8.0 + text_w + 8.0 + 18.0)).abs() < 1e-3);
        assert_eq!(Button::new().count(0).parts().len(), 0, "no pill for 0");
        // a wide number grows the pill: its text plus 4 px each side
        let wide = text::measure(&ctx, text::job("128", &count_style())).x;
        assert!((count_width(&ctx, 128) - (wide + 8.0)).abs() < 1e-3 && count_width(&ctx, 128) > 18.0);
        assert_eq!(Button::new().icon(icon(Icon::LocateFixed)).compact().size(&ctx), Vec2::new(32.0, 32.0));
        assert_eq!(Button::text_only("  Follow ").parts(), &[Part::Text("Follow".into())]);
    }

    #[test]
    fn looks_follow_the_css_order() {
        let plain = Button::text_only("Filters");
        assert_eq!(plain.look(false, true), Look { fill: Color32::TRANSPARENT, border: Some(theme::LINE_CONTROL), color: theme::TEXT_1, opacity: 1.0 });
        assert_eq!(plain.look(true, true).fill, theme::SURFACE_HOVER);
        // disabled looks the same, hover included
        assert_eq!(plain.look(true, false), plain.look(true, true));
        let on = Button::text_only("Filters").pressed(true);
        let pressed = Look { fill: theme::AMBER_SOFT, border: Some(theme::AMBER), color: theme::AMBER, opacity: 1.0 };
        assert_eq!(on.look(false, true), pressed);
        assert_eq!(on.look(true, true), pressed, "pressed wins over hover");
        assert_eq!(Button::text_only("Filters").pressed(false).look(false, true), plain.look(false, true));
        let keys = Button::text_only("Save").primary().dim_when_disabled(0.5);
        assert_eq!(keys.look(true, true), Look { fill: theme::AMBER, border: None, color: theme::AMBER_INK, opacity: 1.0 });
        assert_eq!(keys.look(false, false).opacity, 0.5);
        let toast = Button::text_only("Restart").primary_hover(theme::AMBER_HOVER).dim_when_disabled(0.7);
        assert_eq!(toast.look(true, true).fill, theme::AMBER_HOVER);
        assert_eq!(toast.look(true, false).fill, theme::AMBER, ":hover:not(:disabled)");
    }

    #[test]
    fn the_fill_eases_in_and_out_under_the_pointer() {
        let ctx = ctx();
        let draw = |ui: &mut Ui| Button::text_only("Retry now").show(ui);
        frame(&ctx, input(0.0, None), draw); // an area's first pass only sizes it
        let (r, _) = frame(&ctx, input(0.0, None), draw);
        let inside = r.rect.center();
        let fill_of = |rects: &[RectShape]| rects.iter().find(|s| s.rect == r.rect && s.stroke.is_empty()).map(|s| s.fill);
        let (_, rects) = frame(&ctx, input(1.0, Some(inside)), draw);
        assert_eq!(fill_of(&rects), Some(Color32::TRANSPARENT), "starts where it stood");
        let (_, rects) = frame(&ctx, input(1.075, Some(inside)), draw);
        let mid = Color32::TRANSPARENT.lerp_to_gamma(theme::SURFACE_HOVER, anim::ease_out(0.5));
        assert_eq!(fill_of(&rects), Some(mid));
        let (_, rects) = frame(&ctx, input(1.2, Some(inside)), draw);
        assert_eq!(fill_of(&rects), Some(theme::SURFACE_HOVER));
        // the border never eases
        let border = rects.iter().find(|s| !s.stroke.is_empty()).expect("a border");
        assert_eq!((border.stroke.width, border.stroke.color, border.stroke_kind), (1.0, theme::LINE_CONTROL, StrokeKind::Inside));
    }

    #[test]
    fn a_click_or_the_keyboard_presses_it() {
        let ctx = ctx();
        frame(&ctx, input(0.0, None), |ui| Button::text_only("Retry now").show(ui)); // sizing pass
        let (r, _) = frame(&ctx, input(0.0, None), |ui| Button::text_only("Retry now").show(ui));
        let p = r.rect.center();
        let mut raw = input(0.1, Some(p));
        raw.events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
        frame(&ctx, raw, |ui| Button::text_only("Retry now").show(ui));
        let mut raw = input(0.2, Some(p));
        raw.events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
        let (r, _) = frame(&ctx, raw, |ui| Button::text_only("Retry now").show(ui));
        assert!(r.clicked());
        // disabled: no click
        let mut raw = input(1.1, Some(p));
        raw.events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
        frame(&ctx, raw, |ui| Button::text_only("Retry now").enabled(false).show(ui));
        let mut raw = input(1.2, Some(p));
        raw.events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
        let (r, _) = frame(&ctx, raw, |ui| Button::text_only("Retry now").enabled(false).show(ui));
        assert!(!r.clicked() && r.hovered());
    }

    #[test]
    fn a_colour_transition_runs_as_css_runs_it() {
        let ctx = ctx();
        let id = Id::unique("t");
        let at = |time: f64, target: Color32| {
            let mut c = Color32::BLACK;
            ctx.run_ui(input(time, None), |ui| c = transition_color(ui.ctx(), id, target, anim::HOVER)).textures_delta.clear();
            c
        };
        assert_eq!(at(0.0, theme::TEXT_2), theme::TEXT_2, "the first call stands");
        assert_eq!(at(1.0, theme::TEXT_1), theme::TEXT_2);
        let half = theme::TEXT_2.lerp_to_gamma(theme::TEXT_1, anim::ease_out(0.5));
        assert_eq!(at(1.075, theme::TEXT_1), half);
        // back half-way: shortened to the way come
        assert_eq!(at(1.075, theme::TEXT_2), half);
        let shortening = anim::ease_out(0.5);
        let t = 0.5 * f64::from(anim::HOVER * shortening);
        let expected = half.lerp_to_gamma(theme::TEXT_2, anim::ease_out(0.5));
        assert_eq!(at(1.075 + t, theme::TEXT_2), expected);
        assert_eq!(at(1.075 + 2.0 * t + 1e-6, theme::TEXT_2), theme::TEXT_2);
        // reduced motion: at once (0.01 ms)
        anim::set_reduced_motion(&ctx, true);
        assert_eq!(at(3.0, theme::AMBER), theme::AMBER);
    }

    #[test]
    fn the_focus_ring_sits_2_px_outside() {
        let ctx = ctx();
        let mut rect_drawn = None;
        let mut output = ctx.run_ui(input(0.0, None), |ui| {
            let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(80.0, 32.0));
            focus_ring(ui.painter(), rect, theme::RADIUS);
            rect_drawn = Some(rect);
        });
        output.textures_delta.clear();
        output.shapes.iter().for_each(|s| {
            if let Shape::Rect(r) = &s.shape {
                // its inner edge 2 px out, its outer edge 4 px out
                let rect = rect_drawn.expect("drawn");
                assert_eq!(r.rect, rect.expand(3.0));
                assert_eq!((r.stroke.width, r.stroke.color), (2.0, theme::AMBER));
                assert_eq!(r.corner_radius, CornerRadius::same(7));
            }
        });
    }
}
