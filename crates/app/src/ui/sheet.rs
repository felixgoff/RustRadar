//! BottomSheet (item U19; doc 06 §17, §3.11, §3.12; `BottomSheet.svelte`, `+page.svelte:1117-1131`,
//! `1613-1623`): on a phone, the lists and filters in a sheet over the bottom of the map, and the
//! details of whatever is picked in their place.
//!
//! **The detents** ([`Detent`]): `peek`, `half`, `full`, of the sheet's content box ([`heights`]):
//! - `peek`: the grab handle, the head (if any) and the content's summary when the content marks
//!   parts of itself for it ([`peek_part`]: a panel's back row, strip, key line and footer, today's
//!   `data-peek`), else [`PEEK`] (90 px);
//! - `half`: `max(peek, round(viewport height × 0.5))`;
//! - `full`: `max(peek, viewport height − clear)`, `clear` being what the full sheet leaves free at
//!   the top ([`CLEAR_LIST`] with the search on show, [`CLEAR_DETAIL`] with a detail open).
//!
//! The content box is also held to `100dvh − clear − safe top − safe bottom` (CSS `max-height`), so
//! the full sheet stays clear of the notch and the search. Its height eases over 220 ms
//! (`transition: height 220ms var(--ease-out)`, [`anim::SHEET`]), not while it is dragged.
//!
//! **The box** ([`show`]): fixed to the window's bottom, left and right (a phone on its side: at
//! `left: max(8, safe-left)`, 320 px wide), `.surface` without its left, right and bottom borders
//! (the 1 px `--line` top border tapering into the 12 px top corners), the home bar's padding
//! (`safe-area-inset-bottom`) under the content, content clipped (`overflow: hidden`). Inside, in
//! a column:
//! 1. the **grab handle**: a button the sheet's width and 22 px high with a 36 × 4 pill (radius 2,
//!    `--line-control`, `--amber` with the keyboard on it) in its middle. A drag follows the finger
//!    (`height = clamp(from + dy, peek − 40, full)`; more than 6 px of movement counts as a drag)
//!    and snaps to the nearest detent on release ([`nearest`]); a tap steps `peek → half → full →
//!    peek` ([`next`]); with the keyboard on it, Up raises a detent and Down lowers one;
//! 2. the **head** (the list's toolbar), padding `0 16px 4px` (at least the side safe areas);
//! 3. the **body**, the rest, its sides padded by the side safe areas: the caller lays it out (the
//!    list view scrolls it; a detail panel fills it and scrolls itself).
//!
//! The sheet answers its outer height ([`Shown::height`], the page's `--sheet-h`: content, home bar
//! padding and border, a whole number of pixels as `offsetHeight` gives it): the attribution strip
//! and the map controls sit on it.
//!
//! **The page's detent** ([`Sheet`], in [`crate::state::AppState::sheet`]): what the handle asks
//! goes back as [`Action::Sheet`](crate::state::Action::Sheet). When a detail opens, the list's
//! detent is kept and the sheet lowers to `peek`, or to `half` for an airport board with no flight
//! over it; when the details close the list's detent comes back ([`Sheet::follow`]). On a phone,
//! closing the top thing (Escape, Android's back) lowers a raised sheet last of all
//! ([`Sheet::lower`]).
//!
//! **Peek parts.** Content marks its summary while the sheet draws it: [`in_sheet`] says whether a
//! component is drawn in a sheet (the panels put their summary first there), and [`peek_part`]
//! adds a part's height (rounded, as `offsetHeight` is) to the summary. The sheet measures the
//! summary as it draws and uses it from the next frame on (today's `ResizeObserver`).
//!
//! Deviations:
//! - The top border's taper into the corners is drawn as a second rounded shape 1 px lower with an
//!   11 px radius, where CSS's inner border edge is an ellipse (12 × 11): the two differ by a
//!   fraction of a pixel.
//! - Keyboard focus is egui's: only the keyboard moves it, so it stands for `:focus-visible`.
//! - A drag ends when the primary button or the finger comes up wherever it is (the handle's
//!   pointer capture); a press and release within one frame count as a tap.

use egui::{Area, Context, CornerRadius, CursorIcon, Id, Key, LayerId, Modifiers, Order, Pos2, Rect, Role, Sense, Ui, UiBuilder, Vec2, WidgetInfo};

use super::widgets::button::focus_ring;
use super::{anim, theme};

/// The peek height when the content marks no summary (`PEEK`).
pub const PEEK: f32 = 90.0;
/// The grab handle's height.
pub const GRAB_HEIGHT: f32 = 22.0;
/// Its pill: 36 × 4, radius 2.
pub const PILL: Vec2 = Vec2::new(36.0, 4.0);
pub const PILL_RADIUS: u8 = 2;
/// The sheet's top corners (`border-radius: 12px 12px 0 0`).
pub const RADIUS: u8 = 12;
/// The head's padding: across (at least the side safe area), and below.
pub const HEAD_PADDING: Vec2 = Vec2::new(theme::SPACE_4, theme::SPACE_1);
/// More than this much movement on the handle is a drag, not a tap.
pub const DRAG_SLOP: f32 = 6.0;
/// How far below `peek` a drag may pull the sheet.
pub const BELOW_PEEK: f32 = 40.0;
/// What the full sheet leaves clear at the top with the search and mode buttons on show.
pub const CLEAR_LIST: f32 = 64.0;
/// … and with a detail open (the search steps aside).
pub const CLEAR_DETAIL: f32 = 12.0;
/// A phone on its side: the sheet's distance from the left (at least the safe area) and width.
pub const LANDSCAPE_LEFT: f32 = theme::SPACE_2;
pub const LANDSCAPE_WIDTH: f32 = 320.0;

/// How far up the sheet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Detent {
    #[default]
    Peek,
    Half,
    Full,
}

/// The detents from the lowest up (`ORDER`).
pub const ORDER: [Detent; 3] = [Detent::Peek, Detent::Half, Detent::Full];

impl Detent {
    fn index(self) -> usize {
        ORDER.iter().position(|d| *d == self).unwrap_or(0)
    }

    /// The handle's accessible words for it.
    pub fn words(self) -> &'static str {
        match self {
            Detent::Full => "full height",
            Detent::Half => "half height",
            Detent::Peek => "lowered",
        }
    }
}

/// The detent a tap on the handle steps to: `peek → half → full → peek`.
pub fn next(detent: Detent) -> Detent {
    ORDER[(detent.index() + 1) % ORDER.len()]
}

/// Up on the handle: one detent higher, none above `full`.
pub fn raise(detent: Detent) -> Option<Detent> {
    ORDER.get(detent.index() + 1).copied()
}

/// Down on the handle: one detent lower, none below `peek`.
pub fn lower(detent: Detent) -> Option<Detent> {
    detent.index().checked_sub(1).map(|i| ORDER[i])
}

/// The three heights of the content box, px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heights {
    pub peek: f32,
    pub half: f32,
    pub full: f32,
}

impl Heights {
    /// `detent`'s height.
    pub fn of(&self, detent: Detent) -> f32 {
        match detent {
            Detent::Peek => self.peek,
            Detent::Half => self.half,
            Detent::Full => self.full,
        }
    }
}

/// The heights for a content summary of `summary` px (0: none marked), a handle `grab` high, a
/// head `head` high (`None`: no head), a viewport `viewport_height` high, and `clear` left free at
/// the top.
pub fn heights(summary: f32, grab: f32, head: Option<f32>, viewport_height: f32, clear: f32) -> Heights {
    let peek = if summary != 0.0 { grab + head.unwrap_or(0.0) + summary } else { PEEK };
    Heights { peek, half: peek.max(js_round(viewport_height * 0.5)), full: peek.max(viewport_height - clear) }
}

/// JavaScript's `Math.round` (halves up).
fn js_round(v: f32) -> f32 {
    (v + 0.5).floor()
}

/// Where a drag from a sheet `from` high holds it after the finger moved `dy` up:
/// `clamp(from + dy, peek − 40, full)`.
pub fn dragged(heights: &Heights, from: f32, dy: f32) -> f32 {
    (from + dy).max(heights.peek - BELOW_PEEK).min(heights.full)
}

/// The detent nearest a sheet `height` high, `current` winning a tie (`ORDER.reduce(…, detent)`).
pub fn nearest(heights: &Heights, height: f32, current: Detent) -> Detent {
    ORDER.iter().fold(current, |best, &d| if (heights.of(d) - height).abs() < (heights.of(best) - height).abs() { d } else { best })
}

// ------------------------------------------------------------------------------------------------
// the page's detent

/// The page's `sheetDetent` and the list's detent kept while details show (`listDetent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sheet {
    detent: Detent,
    list_detent: Detent,
    /// Whether a detail was open when [`Self::follow`] last looked.
    details: bool,
}

impl Sheet {
    /// How far up the sheet is.
    pub fn detent(&self) -> Detent {
        self.detent
    }

    /// Sets the detent (the handle's drag, tap and keys); answers whether it changed.
    pub fn set(&mut self, detent: Detent) -> bool {
        std::mem::replace(&mut self.detent, detent) != detent
    }

    /// The detent [`Self::follow`] would leave for `details` open or not (`board_alone`: the open
    /// detail is an airport board with no flight over it): the UI reads it in the frame a detail
    /// opens or closes, before the state follows.
    pub fn detent_with(&self, details: bool, board_alone: bool) -> Detent {
        match (self.details, details) {
            (false, true) => {
                if board_alone {
                    Detent::Half
                } else {
                    Detent::Peek
                }
            }
            (true, false) => self.list_detent,
            _ => self.detent,
        }
    }

    /// The page's rule (§3.12): when a detail opens, the list's detent is kept and the sheet goes
    /// to `half` for an airport board with no flight over it, else `peek`; when every detail has
    /// closed, the list's detent comes back. Answers whether the detent changed.
    pub fn follow(&mut self, details: bool, board_alone: bool) -> bool {
        let detent = self.detent_with(details, board_alone);
        if details && !self.details {
            self.list_detent = self.detent;
        }
        self.details = details;
        self.set(detent)
    }

    /// `closeTop`'s last step on a phone: a sheet above `peek` goes to `peek`. Answers whether it
    /// did.
    pub fn lower(&mut self) -> bool {
        self.detent != Detent::Peek && self.set(Detent::Peek)
    }
}

// ------------------------------------------------------------------------------------------------
// peek parts

fn summary_id() -> Id {
    Id::unique("rustradar::sheet::summary")
}

/// Whether the content being drawn is in a sheet: the panels put their summary first there.
pub fn in_sheet(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<Option<f32>>(summary_id())).flatten().is_some()
}

/// Marks a part `height` px high as part of the summary the sheet shows at `peek` (`data-peek`):
/// nothing outside a sheet.
pub fn peek_part(ctx: &Context, height: f32) {
    ctx.data_mut(|d| {
        let sum = d.get_temp_mut_or_default::<Option<f32>>(summary_id());
        if let Some(sum) = sum {
            *sum += height.round();
        }
    });
}

/// Draws `add` as a peek part (see [`peek_part`]): its height is the cursor's travel (the panels
/// lay their parts out with no spacing between them).
pub fn peek<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let top = ui.cursor().top();
    let r = add(ui);
    peek_part(ui.ctx(), ui.cursor().top() - top);
    r
}

// ------------------------------------------------------------------------------------------------
// drawing

/// What the page gives the sheet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Props<'a> {
    /// The sheet's accessible name: `Details`, `Flights`, `Trains`, `Ships`.
    pub label: &'a str,
    /// The page's detent.
    pub detent: Detent,
    /// What the full sheet leaves clear at the top.
    pub clear: f32,
    /// A phone on its side: the sheet down the left, 320 px wide.
    pub landscape: bool,
}

/// Where the sheet was drawn and what it asks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    /// The box, border and home bar padding included.
    pub rect: Rect,
    /// Its height as `offsetHeight` gives it (whole pixels): `--sheet-h`.
    pub height: f32,
    /// The content box's height now.
    pub content: f32,
    /// The detents' heights.
    pub heights: Heights,
    /// The body's rectangle (inside the side safe areas).
    pub body: Rect,
    /// The handle's ask: a new detent.
    pub ask: Option<Detent>,
    /// The handle is being dragged.
    pub dragging: bool,
}

/// A drag of the handle (`drag`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    /// Where the finger went down.
    y: f32,
    from: f32,
    now: f32,
    moved: bool,
}

/// What the sheet keeps between frames.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Kept {
    drag: Option<Drag>,
    /// The content's summary and the head's height, as last measured.
    summary: f32,
    head: f32,
}

fn sheet_id() -> Id {
    Id::unique("rustradar::sheet")
}

/// The sheet's layer: for tests and for the areas that must stay above or below it.
pub fn layer() -> LayerId {
    LayerId::new(Order::Middle, sheet_id())
}

/// The sheet's box in `viewport` with the content `content` high: the side edges, the bottom on
/// the viewport's, the home bar's padding and the top border on top of the content.
pub fn frame(viewport: Rect, insets: egui::SafeAreaInsets, landscape: bool, content: f32) -> Rect {
    let outer = content + insets.0.bottom + theme::BORDER;
    let (left, right) = if landscape {
        let left = viewport.left() + LANDSCAPE_LEFT.max(insets.0.left);
        (left, left + LANDSCAPE_WIDTH)
    } else {
        (viewport.left(), viewport.right())
    };
    Rect::from_min_max(Pos2::new(left, viewport.bottom() - outer), Pos2::new(right, viewport.bottom()))
}

/// The most the content box may be (`max-height: calc(100dvh - clear - safe-top - safe-bottom)`).
pub fn max_content(viewport_height: f32, insets: egui::SafeAreaInsets, clear: f32) -> f32 {
    (viewport_height - clear - insets.0.top - insets.0.bottom).max(0.0)
}

/// Draws the sheet for `props` (see the module documentation): `head` in its head when given,
/// `body` in the body's rectangle (a child UI as wide and as high as the body, clipped to it).
pub fn show(ctx: &Context, props: &Props<'_>, head: Option<&mut dyn FnMut(&mut Ui)>, body: &mut dyn FnMut(&mut Ui)) -> Shown {
    let id = sheet_id();
    let viewport = ctx.viewport_rect();
    let insets = ctx.input(|i| i.safe_area_insets());
    let mut kept: Kept = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let has_head = head.is_some();
    let heights = heights(kept.summary, GRAB_HEIGHT, has_head.then_some(kept.head), viewport.height(), props.clear);

    // the drag under way: the finger's height, or its end
    let mut ask = None;
    if let Some(mut drag) = kept.drag {
        let (down, at) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
        if down {
            if let Some(at) = at {
                let dy = drag.y - at.y;
                drag.now = dragged(&heights, drag.from, dy);
                drag.moved |= dy.abs() > DRAG_SLOP;
            }
            kept.drag = Some(drag);
        } else {
            ask = Some(if drag.moved { nearest(&heights, drag.now, props.detent) } else { next(props.detent) });
            kept.drag = None;
        }
    }
    let detent = ask.unwrap_or(props.detent);
    let target = kept.drag.map_or(heights.of(detent), |d| d.now);
    let eased = anim::value(ctx, id.with("height"), target, if kept.drag.is_some() { 0.0 } else { anim::SHEET });
    let content = eased.min(max_content(viewport.height(), insets, props.clear));
    let wanted = frame(viewport, insets, props.landscape, content);
    let side = (insets.0.left, insets.0.right);
    let mut rect = wanted;
    let mut body_rect = Rect::NOTHING;
    let mut grab_response = None;

    // (clipped to the whole window, not egui's safe area: the home bar's padding is the sheet's)
    Area::new(id).order(Order::Middle).fixed_pos(wanted.min).fade_in(false).constrain_to(viewport).constrain(false).show(ctx, |ui| {
        // (egui puts the area's corner on the pixel grid: everything from where it stands)
        let (at, _) = ui.allocate_exact_size(wanted.size(), Sense::hover());
        rect = at;
        let inner = Rect::from_min_max(Pos2::new(rect.left(), rect.top() + theme::BORDER), Pos2::new(rect.right(), rect.top() + theme::BORDER + content));
        paint_box(ui.painter(), rect);
        let clip = ui.clip_rect().intersect(rect);
        ui.set_clip_rect(clip);

        // the grab handle
        let grab = Rect::from_min_size(inner.min, Vec2::new(inner.width(), GRAB_HEIGHT));
        let response = ui.interact(grab, id.with("grab"), Sense::click_and_drag()).on_hover_cursor(CursorIcon::Grab);
        response.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("{}: {}. Drag, tap or use the arrow keys to resize", props.label, props.detent.words())));
        let focused = response.has_focus();
        let pill = Rect::from_center_size(grab.center(), PILL);
        ui.painter().rect_filled(pill, CornerRadius::same(PILL_RADIUS), if focused { theme::AMBER } else { theme::LINE_CONTROL });
        if focused {
            focus_ring(ui.painter(), grab, 0);
        }
        grab_response = Some(response);

        // the head, padded, measured for `peek`
        let mut top = grab.bottom();
        if let Some(head) = head {
            let pad_left = HEAD_PADDING.x.max(side.0);
            let pad_right = HEAD_PADDING.x.max(side.1);
            let room = Rect::from_min_max(Pos2::new(inner.left() + pad_left, top), Pos2::new(inner.right() - pad_right, inner.bottom().max(top)));
            let mut child = ui.new_child(UiBuilder::new().id_salt("rustradar::sheet::head").max_rect(Rect::from_min_size(room.min, Vec2::new(room.width(), f32::INFINITY))));
            child.set_clip_rect(clip);
            head(&mut child);
            let used = child.min_rect().height().max(0.0) + HEAD_PADDING.y;
            kept.head = used.round();
            top += used;
        } else {
            kept.head = 0.0;
        }

        // the body, its summary measured as it is drawn
        body_rect = Rect::from_min_max(Pos2::new(inner.left() + side.0, top), Pos2::new(inner.right() - side.1, inner.bottom().max(top)));
        ctx.data_mut(|d| d.insert_temp::<Option<f32>>(summary_id(), Some(0.0)));
        let mut child = ui.new_child(UiBuilder::new().id_salt("rustradar::sheet::body").max_rect(body_rect));
        child.set_clip_rect(clip.intersect(body_rect));
        body(&mut child);
        kept.summary = ctx.data_mut(|d| d.remove_temp::<Option<f32>>(summary_id())).flatten().unwrap_or(0.0);
    });
    // above the top bar where the two meet (z 4 over z 3)
    ctx.memory_mut(|m| m.areas_mut().move_to_top(layer()));

    // the handle: a press starts a drag, a tap steps, Up and Down with the keyboard on it
    if let Some(response) = grab_response {
        if kept.drag.is_none() && ask.is_none() && response.is_pointer_button_down_on() {
            let y = ctx.input(|i| i.pointer.press_origin()).map_or(rect.top(), |p| p.y);
            let from = heights.of(props.detent);
            kept.drag = Some(Drag { y, from, now: from, moved: false });
        } else if kept.drag.is_none() && ask.is_none() && response.clicked() {
            // pressed and let go within one frame
            ask = Some(next(props.detent));
        }
        if response.has_focus() {
            let (up, down) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::ArrowUp), i.consume_key(Modifiers::NONE, Key::ArrowDown)));
            if up {
                ask = raise(props.detent).or(ask);
            } else if down {
                ask = lower(props.detent).or(ask);
            }
        }
    }
    let dragging = kept.drag.is_some();
    ctx.data_mut(|d| d.insert_temp(id, kept));
    Shown { rect, height: rect.height().round(), content, heights, body: body_rect, ask: ask.filter(|d| *d != props.detent), dragging }
}

/// The sheet's fill and its top border, which tapers into the 12 px top corners.
fn paint_box(painter: &egui::Painter, rect: Rect) {
    let top = CornerRadius { nw: RADIUS, ne: RADIUS, sw: 0, se: 0 };
    painter.rect_filled(rect, top, theme::LINE);
    let inner = Rect::from_min_max(Pos2::new(rect.left(), rect.top() + theme::BORDER), rect.max);
    let r = RADIUS - 1;
    painter.rect_filled(inner, CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, theme::SURFACE);
}

#[cfg(test)]
mod tests {
    use egui::{Event, PointerButton, RawInput, SafeAreaInsets};

    use super::*;

    const PHONE: Vec2 = Vec2::new(390.0, 844.0);

    fn insets(top: f32, bottom: f32) -> SafeAreaInsets {
        SafeAreaInsets(egui::epaint::MarginF32 { left: 0.0, right: 0.0, top, bottom })
    }

    #[test]
    fn the_three_heights() {
        // the list: no summary, a head, 844 high, 64 clear
        let h = heights(0.0, GRAB_HEIGHT, Some(36.0), 844.0, CLEAR_LIST);
        assert_eq!(h, Heights { peek: 90.0, half: 422.0, full: 780.0 });
        // a panel's summary: the handle and the parts
        let h = heights(239.0, GRAB_HEIGHT, None, 844.0, CLEAR_DETAIL);
        assert_eq!(h, Heights { peek: 261.0, half: 422.0, full: 832.0 });
        // a summary taller than half the window holds half up to it
        let h = heights(500.0, GRAB_HEIGHT, None, 700.0, CLEAR_DETAIL);
        assert_eq!((h.peek, h.half, h.full), (522.0, 522.0, 688.0));
        // half rounds as Math.round does
        assert_eq!(heights(0.0, GRAB_HEIGHT, None, 845.0, 64.0).half, 423.0);
        // the content box is held clear of the notch and the home bar
        assert_eq!(max_content(844.0, insets(47.0, 34.0), CLEAR_LIST), 699.0);
    }

    #[test]
    fn taps_keys_drags_and_the_nearest_detent() {
        assert_eq!([next(Detent::Peek), next(Detent::Half), next(Detent::Full)], [Detent::Half, Detent::Full, Detent::Peek]);
        assert_eq!((raise(Detent::Peek), raise(Detent::Full)), (Some(Detent::Half), None));
        assert_eq!((lower(Detent::Half), lower(Detent::Peek)), (Some(Detent::Peek), None));
        let h = Heights { peek: 90.0, half: 422.0, full: 780.0 };
        // the finger's travel, no lower than 40 under peek, no higher than full
        assert_eq!(dragged(&h, 90.0, 100.0), 190.0);
        assert_eq!(dragged(&h, 90.0, -200.0), 50.0);
        assert_eq!(dragged(&h, 422.0, 900.0), 780.0);
        assert_eq!(nearest(&h, 300.0, Detent::Peek), Detent::Half);
        assert_eq!(nearest(&h, 200.0, Detent::Half), Detent::Peek);
        assert_eq!(nearest(&h, 700.0, Detent::Peek), Detent::Full);
        // a tie keeps the detent it started from
        assert_eq!(nearest(&h, 256.0, Detent::Half), Detent::Half);
        assert_eq!(nearest(&h, 256.0, Detent::Peek), Detent::Peek);
    }

    #[test]
    fn details_lower_the_sheet_and_the_list_gets_its_detent_back() {
        let mut s = Sheet::default();
        assert!(s.set(Detent::Full));
        // a flight: peek, the list's full kept
        assert_eq!(s.detent_with(true, false), Detent::Peek);
        assert!(s.follow(true, false));
        assert_eq!(s.detent(), Detent::Peek);
        // raised by hand while open, then another detail: no change (detailOpen stays true)
        s.set(Detent::Half);
        assert!(!s.follow(true, false));
        assert_eq!(s.detent(), Detent::Half);
        // all closed: the list's full again
        assert_eq!(s.detent_with(false, false), Detent::Full);
        assert!(s.follow(false, false));
        assert_eq!(s.detent(), Detent::Full);
        // a board alone opens half up
        s.set(Detent::Peek);
        s.follow(true, true);
        assert_eq!(s.detent(), Detent::Half);
        s.follow(false, false);
        assert_eq!(s.detent(), Detent::Peek);
        // closeTop's last step
        assert!(!s.lower());
        s.set(Detent::Full);
        assert!(s.lower());
        assert_eq!(s.detent(), Detent::Peek);
    }

    /// One pass of a sheet at `detent` with `events` at egui time `time`, a body that marks
    /// `summary` px as its peek parts.
    fn pass(ctx: &Context, time: f64, detent: Detent, summary: f32, head: bool, events: Vec<Event>, insets: SafeAreaInsets) -> Shown {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, PHONE)), time: Some(time), events, safe_area_insets: Some(insets), ..Default::default() };
        let mut shown = None;
        let mut out = ctx.run_ui(raw, |ui| {
            let props = Props { label: "Flights", detent, clear: CLEAR_LIST, landscape: false };
            let mut head_fn = |ui: &mut Ui| {
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
            };
            let mut body = |ui: &mut Ui| {
                assert!(in_sheet(ui.ctx()));
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                if summary > 0.0 {
                    peek(ui, |ui| ui.allocate_exact_size(Vec2::new(ui.available_width(), summary), Sense::hover()));
                }
            };
            shown = Some(show(ui.ctx(), &props, if head { Some(&mut head_fn) } else { None }, &mut body));
        });
        out.textures_delta.clear();
        assert!(!in_sheet(ctx), "only while the sheet draws");
        shown.expect("shown")
    }

    #[test]
    fn the_box_its_height_and_the_peek_summary() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let none = insets(0.0, 0.0);
        // the list at peek: 90 of content and the 1 px border
        let s = pass(&ctx, 0.0, Detent::Peek, 0.0, true, vec![], none);
        assert_eq!(s.rect, Rect::from_min_max(Pos2::new(0.0, 753.0), Pos2::new(390.0, 844.0)));
        assert_eq!(s.height, 91.0);
        // the head measured: 32 and its 4 px below
        let s = pass(&ctx, 0.1, Detent::Peek, 0.0, true, vec![], none);
        assert_eq!(s.body.top(), 753.0 + 1.0 + 22.0 + 36.0);
        // half and full with the insets: the home bar under the content, the notch kept clear
        let phone = insets(47.0, 34.0);
        let ctx = Context::default();
        theme::apply(&ctx);
        let s = pass(&ctx, 0.0, Detent::Half, 0.0, true, vec![], phone);
        assert_eq!((s.content, s.height), (422.0, 457.0));
        let ctx = Context::default();
        theme::apply(&ctx);
        let s = pass(&ctx, 0.0, Detent::Full, 0.0, true, vec![], phone);
        assert_eq!((s.content, s.rect.top()), (699.0, 844.0 - 734.0));
        // marked parts: the next frame's peek is the handle and their sum
        let ctx = Context::default();
        theme::apply(&ctx);
        pass(&ctx, 0.0, Detent::Peek, 120.4, false, vec![], none);
        let s = pass(&ctx, 0.0, Detent::Peek, 120.4, false, vec![], none);
        assert_eq!(s.heights.peek, 22.0 + 120.0);
    }

    #[test]
    fn the_height_eases_over_220_ms() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let none = insets(0.0, 0.0);
        pass(&ctx, 0.0, Detent::Peek, 0.0, true, vec![], none);
        let mid = pass(&ctx, 1.0, Detent::Half, 0.0, true, vec![], none);
        assert_eq!(mid.content, 90.0, "the transition starts where it was");
        let mid = pass(&ctx, 1.11, Detent::Half, 0.0, true, vec![], none);
        let t = anim::ease_out(0.11 / 0.22);
        assert!((mid.content - (90.0 + (422.0 - 90.0) * t)).abs() < 0.5, "{}", mid.content);
        let end = pass(&ctx, 1.3, Detent::Half, 0.0, true, vec![], none);
        assert_eq!(end.content, 422.0);
    }

    fn press(at: Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(at), Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }]
    }

    fn release(at: Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(at), Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }]
    }

    #[test]
    fn a_tap_steps_and_a_drag_snaps_to_the_nearest() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let none = insets(0.0, 0.0);
        pass(&ctx, 0.0, Detent::Peek, 0.0, true, vec![], none);
        pass(&ctx, 0.1, Detent::Peek, 0.0, true, vec![], none);
        // the handle is at 754–776: a tap (down, then up 2 px away) steps to half
        let handle = Pos2::new(195.0, 765.0);
        let s = pass(&ctx, 1.0, Detent::Peek, 0.0, true, press(handle), none);
        assert_eq!(s.ask, None);
        let s = pass(&ctx, 1.1, Detent::Peek, 0.0, true, release(handle + Vec2::new(0.0, -2.0)), none);
        assert_eq!(s.ask, Some(Detent::Half));
        // a drag from peek: the sheet follows the finger, then snaps
        let s = pass(&ctx, 2.0, Detent::Peek, 0.0, true, press(handle), none);
        assert!(s.dragging);
        let s = pass(&ctx, 2.1, Detent::Peek, 0.0, true, vec![Event::PointerMoved(handle + Vec2::new(0.0, -200.0))], none);
        assert_eq!(s.content, 290.0, "no easing while dragged");
        let s = pass(&ctx, 2.2, Detent::Peek, 0.0, true, vec![Event::PointerMoved(handle + Vec2::new(0.0, -300.0))], none);
        assert_eq!(s.content, 390.0);
        let s = pass(&ctx, 2.3, Detent::Peek, 0.0, true, release(handle + Vec2::new(0.0, -300.0)), none);
        assert_eq!(s.ask, Some(Detent::Half));
        assert!(!s.dragging);
        // down past peek: held 40 under it, back to peek
        pass(&ctx, 3.0, Detent::Half, 0.0, true, vec![], none);
        let top = pass(&ctx, 3.5, Detent::Half, 0.0, true, vec![], none).rect.top();
        let grab = Pos2::new(195.0, top + 12.0);
        pass(&ctx, 4.0, Detent::Half, 0.0, true, press(grab), none);
        let s = pass(&ctx, 4.1, Detent::Half, 0.0, true, vec![Event::PointerMoved(grab + Vec2::new(0.0, 600.0))], none);
        assert_eq!(s.content, 50.0);
        let s = pass(&ctx, 4.2, Detent::Half, 0.0, true, release(grab + Vec2::new(0.0, 600.0)), none);
        assert_eq!(s.ask, Some(Detent::Peek));
    }

    #[test]
    fn up_and_down_with_the_keyboard_on_the_handle() {
        let ctx = Context::default();
        theme::apply(&ctx);
        let none = insets(0.0, 0.0);
        pass(&ctx, 0.0, Detent::Peek, 0.0, true, vec![], none);
        ctx.memory_mut(|m| m.request_focus(sheet_id().with("grab")));
        pass(&ctx, 0.1, Detent::Peek, 0.0, true, vec![], none);
        let key = |key: Key| vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }];
        let s = pass(&ctx, 0.2, Detent::Peek, 0.0, true, key(Key::ArrowUp), none);
        assert_eq!(s.ask, Some(Detent::Half));
        let s = pass(&ctx, 0.3, Detent::Peek, 0.0, true, key(Key::ArrowDown), none);
        assert_eq!(s.ask, None, "nothing below peek");
        let s = pass(&ctx, 0.4, Detent::Full, 0.0, true, key(Key::ArrowDown), none);
        assert_eq!(s.ask, Some(Detent::Half));
    }
}
