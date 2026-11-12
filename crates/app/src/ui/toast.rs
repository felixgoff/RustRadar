//! UpdateToast (doc 06 §22; `UpdateToast.svelte`): the offer to restart into a newer version that
//! the updater has downloaded (desktop) or found (Android), at the bottom left over everything but
//! the dev panel and the modal dialogs. The UI only: the updater (stage 11, P3 and P4) sets
//! `state.update_ready` (from `BackendEvent::UpdateReady`, and at start from its `pending_update`)
//! and answers [`Updater::restart_to_update`] ([`install_updater`]): on the desktop
//! `update::install::connect`'s `Restart` (the hourly watch's download installed off the UI
//! thread, the app relaunched and closed; a failure comes back as the sub line), on Android
//! `update::android::connect`'s `Download`; the window installs either again on a rebuilt egui
//! context.
//!
//! **Shown** while a version is known (`state.update_ready`) and the toast was not dismissed for
//! that version (`Later` hides it for the session; a newer version shows it again).
//!
//! **Layout** (`.toast.surface`, fixed at left 16, bottom 40; [`layout`]): one flex row, items
//! centred on its height, 12 apart, inside `padding: 12px 12px 12px 16px` and the 1 px `--line`
//! border, radius 4, `--surface`:
//! 1. `RefreshCw` 16/1.75 in `--amber`;
//! 2. the text: `RustRadar {version} is ready` (13 px at 600, `--text-1`) over the sub line (12 px
//!    `--text-3`): `Restart to start using it. It takes a few seconds.`, on Android `Download it,
//!    then open the file to install.`, or the last error; each wraps;
//! 3. the primary button (`--amber` filled, white text; [`theme::AMBER_HOVER`] under the pointer;
//!    at 0.7 with the default cursor while disabled): `Restart`, `Restarting…` while it restarts
//!    (disabled), on Android `Download`;
//! 4. the close button (`X` 16/1.75, named `Later`, its tooltip `Later: it installs the next time
//!    RustRadar opens`, on Android `Later`).
//!
//! The row is as wide as its content, at most `min(440px, 100vw - 32px)` of content box (the
//! padding and border come on top, so a phone's toast reaches the window's right edge, as today);
//! where the content does not fit, CSS's flex shrinking ([`flex_shrink`]) takes the room from the
//! text (`min-width: 0`) and the close button (down to its icon's 16 px) in proportion to their
//! widths: today's `.icon-button` is a flex item that keeps no width of its own.
//!
//! **Behaviour** (the component's script): Restart (`restart()`) clears the error and, except on
//! Android, disables itself as `Restarting…`, then asks the updater; a failure shows its message
//! (without a leading `Error: `) in the sub line and enables the button again. On success the app
//! restarts (desktop) or the browser downloads the package (Android), and the toast stays as it is.
//!
//! **Order** (z 30, doc 06 §28): above the panels, the sidebar and the search dropdown, under the
//! dev panel (z 50) and the modal dialogs (the browser's top layer, which makes the page inert): an
//! egui area of `Order::Foreground` brought to the top each pass, except while the dev panel is
//! open (which then stays above it); while the keys dialog or the shortcuts' help is open, one of
//! `Order::Middle` (under the dialog's backdrop, taking no input), still above the panels
//! ([`order`]).
//!
//! **Checked** against Chromium's layout of today's markup (its template and scoped styles with
//! app.css and the fonts, at 1440 × 900 and 390 × 844: the plain offer, Restarting…, a long error,
//! Android's, the hover; `tests/toast.rs`).
//!
//! Deviations:
//! - Text sits a pixel lower than Chromium's (egui's baseline, as everywhere in the app).
//! - No `role="status"` live region: egui gives the toast's buttons to AccessKit, not the text.
//! - With no updater installed the Restart button answers [`NO_UPDATER`]; the toast cannot show
//!   then, as only the updater reports a version.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Area, Color32, Context, CornerRadius, CursorIcon, Id, LayerId, Order, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};

use super::icons::{self, Icon};
use super::text::{self, Style};
use super::widgets::button::{Button, HEIGHT as BUTTON_HEIGHT, focus_ring, transition_color};
use super::widgets::icon_button::{self, IconButton};
use super::{anim, theme};
use crate::state::AppState;
use crate::state::aircraft::Answer;

/// Whether this build is Android's (`/Android/i.test(navigator.userAgent)`; doc 07 §11.7): the
/// button downloads the package rather than restarting.
pub const ANDROID: bool = cfg!(target_os = "android");

/// The sub line on the desktop.
pub const SUB: &str = "Restart to start using it. It takes a few seconds.";
/// The sub line on Android.
pub const SUB_ANDROID: &str = "Download it, then open the file to install.";
/// The primary button: ready, restarting, and Android's.
pub const RESTART: &str = "Restart";
pub const RESTARTING: &str = "Restarting…";
pub const DOWNLOAD: &str = "Download";
/// The close button's accessible name, and its tooltip on the desktop and on Android.
pub const LATER: &str = "Later";
pub const LATER_TIP: &str = "Later: it installs the next time RustRadar opens";
pub const LATER_TIP_ANDROID: &str = "Later";
/// What Restart answers when no updater is installed.
pub const NO_UPDATER: &str = "No updater in this build.";

/// The title: `RustRadar {version} is ready`.
pub fn title(version: &str) -> String {
    format!("RustRadar {version} is ready")
}

/// `left: var(--space-4)`.
pub const LEFT: f32 = theme::SPACE_4;
/// `bottom: 40px`.
pub const BOTTOM: f32 = 40.0;
/// `max-width: min(440px, calc(100vw - 2 * var(--space-4)))`, of the content box.
pub const MAX_WIDTH: f32 = 440.0;
pub const WINDOW_MARGIN: f32 = 2.0 * theme::SPACE_4;
/// `padding: var(--space-3) var(--space-3) var(--space-3) var(--space-4)`.
pub const PAD_Y: f32 = theme::SPACE_3;
pub const PAD_RIGHT: f32 = theme::SPACE_3;
pub const PAD_LEFT: f32 = theme::SPACE_4;
/// `gap: var(--space-3)`.
pub const GAP: f32 = theme::SPACE_3;
/// The icons: `RefreshCw` and `X`, 16 px at stroke 1.75.
pub const ICON: icons::Spec = Icon::RefreshCw.at(16.0, icons::STROKE_THIN);
pub const CLOSE_ICON: icons::Spec = Icon::X.at(16.0, icons::STROKE_THIN);
/// `.button:disabled { opacity: 0.7 }`.
pub const DISABLED_OPACITY: f32 = 0.7;

/// The title's style: 13 px at 600 in `--text-1`.
pub const fn title_style() -> Style {
    Style::ui(theme::TEXT_MD, 600, theme::TEXT_1)
}

/// The sub line's style: 12 px in `--text-3`.
pub const fn sub_style() -> Style {
    Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_3)
}

// ------------------------------------------------------------------------------------------------
// the updater's side

/// What the toast asks of the updater (stage 11 installs it with [`install_updater`]).
pub trait Updater: Send {
    /// `restart_to_update`: the desktop installs the downloaded version and restarts into it (an
    /// answer comes only when that failed); Android opens the package's download.
    fn restart_to_update(&mut self) -> Answer<Result<(), String>>;
}

/// Without an updater: Restart answers [`NO_UPDATER`].
struct NoUpdater;

impl Updater for NoUpdater {
    fn restart_to_update(&mut self) -> Answer<Result<(), String>> {
        Answer::ready(Err(NO_UPDATER.into()))
    }
}

/// The toast's state (the component's `dismissed`, `restarting`, `error`), kept by egui between
/// passes.
#[derive(Default)]
pub struct Toast {
    updater: Option<Box<dyn Updater>>,
    /// The version `Later` was pressed for.
    pub dismissed: Option<String>,
    pub restarting: bool,
    pub error: Option<String>,
    asking: Option<Answer<Result<(), String>>>,
}

impl std::fmt::Debug for Toast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Toast")
            .field("dismissed", &self.dismissed)
            .field("restarting", &self.restarting)
            .field("error", &self.error)
            .field("asking", &self.asking.is_some())
            .finish_non_exhaustive()
    }
}

impl Toast {
    /// Whether the toast shows for `version` (`version && dismissed !== version`).
    pub fn shows(&self, version: Option<&str>) -> bool {
        version.is_some_and(|v| self.dismissed.as_deref() != Some(v))
    }

    /// `restart()`: the error cleared, the button disabled except on `android`, the updater asked.
    pub fn restart(&mut self, android: bool) {
        self.restarting = !android;
        self.error = None;
        let updater = self.updater.get_or_insert_with(|| Box::new(NoUpdater));
        self.asking = Some(updater.restart_to_update());
    }

    /// The updater's answer, once it came: a failure's message (`String(e)` without a leading
    /// `Error: `) and the button enabled again. Returns whether it is still waiting.
    pub fn poll(&mut self) -> bool {
        let Some(ask) = &self.asking else { return false };
        match ask.poll() {
            None => true,
            Some(answer) => {
                self.asking = None;
                if let Err(e) = answer {
                    self.error = Some(strip_error_prefix(&e).to_owned());
                    self.restarting = false;
                }
                false
            }
        }
    }

    /// `Later`: hidden for `version`.
    pub fn dismiss(&mut self, version: &str) {
        self.dismissed = Some(version.to_owned());
    }

    /// The sub line: the error, else the platform's note.
    pub fn sub(&self, android: bool) -> &str {
        self.error.as_deref().unwrap_or(if android { SUB_ANDROID } else { SUB })
    }

    /// The primary button's label.
    pub fn button_label(&self, android: bool) -> &'static str {
        if android {
            DOWNLOAD
        } else if self.restarting {
            RESTARTING
        } else {
            RESTART
        }
    }
}

/// `String(e).replace(/^Error:\s*/, "")`.
fn strip_error_prefix(e: &str) -> &str {
    e.strip_prefix("Error:").map_or(e, str::trim_start)
}

fn toast_id() -> Id {
    Id::unique("rustradar::update_toast")
}

/// The toast's state in `ctx`.
pub fn toast(ctx: &Context) -> Arc<Mutex<Toast>> {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(toast_id().with("state"), || Arc::new(Mutex::new(Toast::default()))).clone())
}

/// Hands the toast's Restart to `updater` (the stage 11 updater; tests).
pub fn install_updater(ctx: &Context, updater: Box<dyn Updater>) {
    toast(ctx).lock().unwrap_or_else(PoisonError::into_inner).updater = Some(updater);
}

// ------------------------------------------------------------------------------------------------
// layout

/// One flex item for [`flex_shrink`]: its flex base size and its least size (`min-width`, or the
/// automatic minimum: the smaller of its width and its content's least width).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlexItem {
    pub base: f32,
    pub min: f32,
}

/// CSS's flexible lengths for a row that overflows (CSS Flexbox §9.7, every item `flex-shrink: 1`,
/// no `max-width`): each item shrinks in proportion to its base size, items that reach their
/// least size are frozen there and the rest shrink on, until the row fits `container` less `gaps`.
/// A row that fits keeps every base size.
pub fn flex_shrink(container: f32, gaps: f32, items: &[FlexItem]) -> Vec<f32> {
    let mut size: Vec<f32> = items.iter().map(|i| i.base).collect();
    let mut frozen = vec![false; items.len()];
    if items.iter().map(|i| i.base).sum::<f32>() + gaps <= container {
        return size;
    }
    // items that cannot shrink at all are frozen at once (base below its least size: a
    // hypothetical size above the base)
    for (n, item) in items.iter().enumerate() {
        if item.base <= item.min {
            size[n] = item.min.max(item.base);
            frozen[n] = true;
        }
    }
    loop {
        if frozen.iter().all(|&f| f) {
            return size;
        }
        let used: f32 = items.iter().zip(&frozen).zip(&size).map(|((item, &f), &s)| if f { s } else { item.base }).sum();
        let free = container - gaps - used;
        let scaled: f32 = items.iter().zip(&frozen).filter(|(_, f)| !**f).map(|(item, _)| item.base).sum();
        let mut violation = 0.0;
        let mut targets = vec![0.0; items.len()];
        for (n, item) in items.iter().enumerate() {
            if frozen[n] {
                continue;
            }
            let ratio = if scaled > 0.0 { item.base / scaled } else { 0.0 };
            let target = item.base + free.min(0.0) * ratio;
            let clamped = target.max(item.min);
            violation += clamped - target;
            targets[n] = clamped;
            size[n] = clamped;
        }
        for (n, item) in items.iter().enumerate() {
            if frozen[n] {
                continue;
            }
            // no violation: all frozen; a positive one: the items held at their least size
            if violation <= 0.0 || targets[n] <= item.min {
                frozen[n] = true;
            }
        }
    }
}

/// Where everything of the toast goes in a window `screen`.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The `.surface` box, border included, at Chromium's fractional place.
    pub card: Rect,
    pub icon: Rect,
    /// The text's column: the title's top left and the sub line's.
    pub text: Rect,
    pub title_at: Pos2,
    pub sub_at: Pos2,
    /// The two lines as laid out in the text's width.
    pub title: egui::text::LayoutJob,
    pub sub: egui::text::LayoutJob,
    pub button: Rect,
    pub close: Rect,
}

/// The toast's layout in `ctx` for a window `screen` with `title`, `sub` and the primary
/// `button` (see the module documentation).
pub fn layout(ctx: &Context, screen: Rect, title: &str, sub: &str, button: &Button) -> Layout {
    let measure = |t: &str, s: &Style| text::measure(ctx, text::job(t, s)).x;
    let text_max = measure(title, &title_style()).max(measure(sub, &sub_style()));
    let button_w = button.size(ctx).x;
    let items = [
        FlexItem { base: ICON.size, min: ICON.size },
        // `.text { min-width: 0 }`
        FlexItem { base: text_max, min: 0.0 },
        // a button's text is one word: it keeps its width
        FlexItem { base: button_w, min: button_w },
        // `.icon-button { width: 32px }`, at least its icon
        FlexItem { base: icon_button::SIZE, min: CLOSE_ICON.size },
    ];
    let gaps = GAP * (items.len() - 1) as f32;
    let max_content: f32 = items.iter().map(|i| i.base).sum::<f32>() + gaps;
    let least: f32 = items.iter().map(|i| i.min).sum::<f32>() + gaps;
    let frame = PAD_LEFT + PAD_RIGHT + 2.0 * theme::BORDER;
    // shrink-to-fit in what the window leaves right of `left` (the right edge taken at 0), then
    // the content box's max-width
    let available = screen.width() - LEFT - frame;
    let max_width = MAX_WIDTH.min(screen.width() - WINDOW_MARGIN);
    let content = max_content.min(available).min(max_width).max(least);
    let widths = flex_shrink(content, gaps, &items);
    let text_w = widths[1];
    let title_job = text::wrapped(title, &title_style(), text_w);
    let sub_job = text::wrapped(sub, &sub_style(), text_w);
    let title_h = text::measure(ctx, title_job.clone()).y;
    let sub_h = text::measure(ctx, sub_job.clone()).y;
    let text_h = title_h + sub_h;
    let row_h = text_h.max(BUTTON_HEIGHT).max(icon_button::SIZE);
    let height = row_h + 2.0 * PAD_Y + 2.0 * theme::BORDER;
    let card = Rect::from_min_size(Pos2::new(screen.left() + LEFT, screen.bottom() - BOTTOM - height), Vec2::new(content + frame, height));
    let top = card.top() + theme::BORDER + PAD_Y;
    let mut x = card.left() + theme::BORDER + PAD_LEFT;
    let mut place = |w: f32, h: f32| {
        let r = Rect::from_min_size(Pos2::new(x, top + (row_h - h) / 2.0), Vec2::new(w, h));
        x += w + GAP;
        r
    };
    let icon = place(widths[0], ICON.size);
    let text_rect = place(text_w, text_h);
    let button_rect = place(widths[2], BUTTON_HEIGHT);
    let close = place(widths[3], icon_button::SIZE);
    Layout {
        card,
        icon,
        text: text_rect,
        title_at: text_rect.min,
        sub_at: Pos2::new(text_rect.left(), text_rect.top() + title_h),
        title: title_job,
        sub: sub_job,
        button: button_rect,
        close,
    }
}

/// `rect` with each edge on the nearest whole pixel, as Chromium paints a box.
fn snapped(rect: Rect) -> Rect {
    Rect::from_min_max(rect.min.round(), rect.max.round())
}

/// The toast's area order (see the module documentation): `Order::Middle` under an open modal
/// dialog, else `Order::Foreground`.
pub fn order(dialog_open: bool) -> Order {
    if dialog_open { Order::Middle } else { Order::Foreground }
}

// ------------------------------------------------------------------------------------------------
// drawing

/// What the window's state says about the toast this pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown<'a> {
    /// The version the updater reported (`state.update_ready`).
    pub version: Option<&'a str>,
    /// Android's wording and button.
    pub android: bool,
    /// The keys dialog or the shortcuts' help is open: the toast goes under it.
    pub dialog_open: bool,
    /// The dev panel is open: it stays above the toast.
    pub dev_open: bool,
}

/// The toast for the window's state (the window's UI pass, after the panels and the map controls).
pub fn ui(ctx: &Context, state: &AppState) {
    let s = &state.shortcuts;
    show(ctx, Shown { version: state.update_ready.as_deref(), android: ANDROID, dialog_open: s.keys_open() || s.help_open(), dev_open: s.dev_open() });
}

/// Draws the toast as `shown` says, answering its buttons. Returns the card's rectangle when it
/// is shown.
pub fn show(ctx: &Context, shown: Shown<'_>) -> Option<Rect> {
    let state = toast(ctx);
    let mut t = state.lock().unwrap_or_else(PoisonError::into_inner);
    if t.poll() {
        // the answer wakes the loop; look again meanwhile all the same
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    let version = shown.version.filter(|_| t.shows(shown.version))?;
    let title = title(version);
    let sub = t.sub(shown.android).to_owned();
    let enabled = !t.restarting;
    let primary = Button::text_only(t.button_label(shown.android)).primary_hover(theme::AMBER_HOVER).enabled(enabled).dim_when_disabled(DISABLED_OPACITY);
    let laid = layout(ctx, ctx.content_rect(), &title, &sub, &primary);
    let order = order(shown.dialog_open);
    let id = toast_id().with("area");
    if !shown.dev_open {
        ctx.move_to_top(LayerId::new(order, id));
    }
    let card = snapped(laid.card);
    let mut restart = false;
    let mut later = false;
    Area::new(id).order(order).fixed_pos(card.min).fade_in(false).constrain(false).show(ctx, |ui: &mut Ui| {
        ui.painter().rect(card, CornerRadius::same(theme::RADIUS), theme::SURFACE, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
        ui.allocate_rect(card, Sense::hover());
        icons::paint(ui.painter(), laid.icon, ICON, theme::AMBER);
        let painter = ui.painter();
        painter.galley(laid.title_at, painter.layout_job(laid.title.clone()), Color32::PLACEHOLDER);
        painter.galley(laid.sub_at, painter.layout_job(laid.sub.clone()), Color32::PLACEHOLDER);
        restart = ui.put(snapped(laid.button), primary).clicked();
        later = close_button(ui, snapped(laid.close), if shown.android { LATER_TIP_ANDROID } else { LATER_TIP }).clicked();
    });
    if restart && enabled {
        t.restart(shown.android);
    }
    if later {
        t.dismiss(version);
    }
    Some(card)
}

/// The close button in `rect` (narrower than 32 when the row shrank it): `.icon-button`'s look,
/// its icon centred.
fn close_button(ui: &mut Ui, rect: Rect, tip: &str) -> egui::Response {
    let id = toast_id().with("later");
    let mut response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, LATER));
    let (fill, color, _) = IconButton::new(CLOSE_ICON, LATER).look(response.hovered(), true);
    let fill = transition_color(ui.ctx(), id.with("fill"), fill, anim::HOVER);
    let color = transition_color(ui.ctx(), id.with("color"), color, anim::HOVER);
    icon_button::paint(ui.painter(), rect, CLOSE_ICON, fill, color);
    if response.has_focus() {
        focus_ring(ui.painter(), rect, theme::RADIUS);
    }
    response = response.on_hover_cursor(CursorIcon::PointingHand);
    super::widgets::tooltip::show(response, tip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[track_caller]
    fn near(got: f32, chromium: f32, tolerance: f32) {
        assert!((got - chromium).abs() <= tolerance, "{got}, Chromium {chromium}");
    }

    #[test]
    fn a_row_that_fits_keeps_its_widths() {
        let items = [FlexItem { base: 16.0, min: 16.0 }, FlexItem { base: 249.71875, min: 0.0 }, FlexItem { base: 69.96875, min: 69.96875 }, FlexItem { base: 32.0, min: 16.0 }];
        assert_eq!(flex_shrink(440.0, 36.0, &items), vec![16.0, 249.71875, 69.96875, 32.0]);
    }

    #[test]
    fn an_overflowing_row_shrinks_the_text_and_the_close_button_as_chromium_does() {
        // the long error at 1440 (max-content widths from Chromium, 368.796875 as an f32 holds it): text 292.641, close 25.391
        let items = [FlexItem { base: 16.0, min: 16.0 }, FlexItem { base: 368.79688, min: 0.0 }, FlexItem { base: 69.96875, min: 69.96875 }, FlexItem { base: 32.0, min: 16.0 }];
        let w = flex_shrink(440.0, 36.0, &items);
        near(w[0], 16.0, 0.001);
        near(w[1], 292.641, 0.01);
        near(w[2], 69.969, 0.001);
        near(w[3], 25.391, 0.01);
        // a phone (content 344): text 196.813, close 25.219
        let items = [FlexItem { base: 16.0, min: 16.0 }, FlexItem { base: 249.71875, min: 0.0 }, FlexItem { base: 69.96875, min: 69.96875 }, FlexItem { base: 32.0, min: 16.0 }];
        let w = flex_shrink(344.0, 36.0, &items);
        near(w[1], 196.813, 0.01);
        near(w[3], 25.219, 0.01);
        // so narrow the close button reaches its icon: frozen at 16, the text takes the rest
        let w = flex_shrink(200.0, 36.0, &items);
        near(w[3], 16.0, 0.001);
        near(w[1], 200.0 - 36.0 - 16.0 - 69.96875 - 16.0, 0.001);
    }

    #[test]
    fn shown_until_later_for_that_version() {
        let mut t = Toast::default();
        assert!(!t.shows(None), "no version known");
        assert!(t.shows(Some("0.4.0")));
        t.dismiss("0.4.0");
        assert!(!t.shows(Some("0.4.0")));
        assert!(t.shows(Some("0.4.1")), "a newer version shows it again");
    }

    struct Canned(mpsc::Receiver<Result<(), String>>, Arc<Mutex<u32>>);

    impl Updater for Canned {
        fn restart_to_update(&mut self) -> Answer<Result<(), String>> {
            *self.1.lock().unwrap() += 1;
            let (tx, rx) = mpsc::channel();
            if let Ok(answer) = self.0.try_recv() {
                tx.send(answer).unwrap();
            }
            rx.into()
        }
    }

    #[test]
    fn restart_disables_until_a_failure_comes_back_without_its_prefix() {
        let (tx, rx) = mpsc::channel();
        let asked = Arc::new(Mutex::new(0));
        let mut t = Toast { updater: Some(Box::new(Canned(rx, asked.clone()))), ..Default::default() };
        assert_eq!((t.button_label(false), t.sub(false)), (RESTART, SUB));
        tx.send(Err("Error: the download's signature does not match".into())).unwrap();
        t.restart(false);
        assert_eq!(*asked.lock().unwrap(), 1);
        assert!(t.restarting);
        assert_eq!(t.button_label(false), RESTARTING);
        assert!(!t.poll(), "the answer was there");
        assert!(!t.restarting, "enabled again");
        assert_eq!(t.sub(false), "the download's signature does not match");
        assert_eq!(t.button_label(false), RESTART);
        // a second try clears the error while it waits
        t.restart(false);
        assert_eq!(t.error, None);
        assert!(t.poll(), "waiting");
        assert_eq!(t.sub(false), SUB);
    }

    #[test]
    fn android_downloads_and_stays_enabled() {
        let mut t = Toast::default();
        assert_eq!((t.button_label(true), t.sub(true)), (DOWNLOAD, SUB_ANDROID));
        t.restart(true);
        assert!(!t.restarting, "restarting = !android");
        assert_eq!(t.button_label(true), DOWNLOAD);
        // no updater installed
        t.poll();
        assert_eq!(t.sub(true), NO_UPDATER);
    }

    #[test]
    fn the_error_prefix_rule() {
        assert_eq!(strip_error_prefix("Error: no network"), "no network");
        assert_eq!(strip_error_prefix("Error:no network"), "no network");
        assert_eq!(strip_error_prefix("no Error: here"), "no Error: here");
    }

    #[test]
    fn under_a_dialog_the_toast_leaves_the_foreground() {
        assert_eq!(order(false), Order::Foreground);
        assert_eq!(order(true), Order::Middle);
    }
}
