//! The hover card (item U18; doc 06 §18.1, doc 03 §10.4; `HoverCard.svelte`): the card that follows
//! the pointer over an aircraft, its photo when one is known over the flight's essentials.
//! Non-interactive (`pointer-events: none`): painted on a layer of its own over the map and under
//! every panel (`z-index: 2` inside the globe, which is `isolation: isolate`), so the map keeps the
//! pointer. Ships, trains, signals and airport codes get the map's tooltip instead
//! ([`super::tooltip`]).
//!
//! **Data**: `state::pick`'s [`HoverCard`] (`hoverCard`, G:3451-3474, item A9): the hovered
//! flight's latest row, its route's cities, the progress, the details' photos and logo from the
//! hover loader (`core::hover`: the 300 ms dwell, one request at a time, the cache) or from the
//! selected flight's own stream, the loader's status, and the pointer. The window takes it from
//! the state before the egui pass ([`crate::state::AppState::hover_card`] refreshes the cache's
//! recency).
//!
//! **Placement** ([`place`]): 16 px right of and below the pointer; on the pointer's left when
//! the card would come within 8 px of the map's right edge, above it when within 8 px of the
//! bottom (by the card's height now, as `bind:offsetHeight` rounds it); never nearer than 8 px to
//! the left or top edge. The card's corner is put on a whole device pixel.
//!
//! **Anatomy**, 272 px wide (Chromium's layout of today's markup and app.css):
//! - **Photo slot** (`.photo-slot`): open while the details load, or are in and have a photo that
//!   has not failed; it eases its height between 0 and the photo's 153 px (16:9 of 272) and its
//!   6 px gap below over 180 ms (`grid-template-rows: 0fr ↔ 1fr`, `--ease-out`), clipping the
//!   photo from the bottom. The photo block is `--surface-raised`, radius 4: `.photo`'s own
//!   background outranks `.skeleton`'s gradient, so it stands flat while loading, as Chromium draws
//!   it. The picture (`images[0].thumbnail || .medium`, through `app::images`) covers it
//!   (`object-fit: cover`) and fades in over 160 ms once loaded; then the credit `© {copyright}`
//!   at left 8, bottom 4 (11 px `--text-1` over its shadow, cut with `…` 16 px short of the
//!   width). A picture that fails closes the slot (`photoFailed`).
//! - **Body** (`.body.surface`): `--surface` with a 1 px `--line` border, radius 4, padding
//!   8 / 12 / 12, its rows 2 px apart:
//!   - the head row (at least 26 px, items centred, 8 apart): the callsign (`callsign || flight ||
//!     "No callsign"`, Mono 14/700, the one item that shrinks, cut with `…`); the type code chip
//!     (Mono 11/600 `--text-2`, a 1 px `--line-control` border, radius 4, padding 0 5, line 16);
//!     the registration (Mono 11 `--text-3`); the airline's logo pushed to the right
//!     (`margin-left: auto`) on a `--text-1` tile, radius 4, padding 2 × 5 round 22 px of content,
//!     the picture at most 72 × 18 with its aspect, centred; while the logo loads the tile is its
//!     padding alone (an `<img alt="">` not loaded yet takes no room), and a logo that fails is
//!     left out (`logoFailed`);
//!   - the route (13 px `--text-2`, cut with `…`): `{from} to {to}`, `From {from}`, `To {to}`, or
//!     nothing;
//!   - the progress track when there is a progress: 2 px, radius 1, `--line`, 8 px above and 4
//!     below it; its fill `--amber` (Flightradar24's figure) or `--text-3` (the great-circle
//!     estimate), `clamp(value, 0, 1)` of the width, easing to a new value over 400 ms;
//!   - the numbers (Mono 12 `--text-2`): `{alt} ft · {speed} kt` (the altitude with thousands
//!     separators, `toLocaleString()` in English) or `On ground · {speed} kt`; the `·` is Overpass
//!     Mono's own, a full cell wide (`--font-data` names no "Middle Dot" face), not the narrowed
//!     dot `text::job` sets in UI text.
//!
//! **Mounting**: the card's own state (the photo that loaded and when, the photo and logo that
//! failed, the transitions) lives while a card is drawn in each pass, as the component lives while
//! `hoverCard` is set: moving from one aircraft to the next keeps it (the photo slot stays open
//! through the next flight's loading); a pass without a card ends it. Entering, the slot and the
//! track stand at their values (no transition on an element's first style).
//!
//! **Checked** against Chromium's layout of today's component, server-rendered by Svelte with the
//! same props and laid out with app.css at 1440 × 900 (`tests/hover_card.rs`: loading, with its
//! photo and logo, without a photo and with a long callsign, bare): every box, border, rule and
//! the track land on Chromium's pixels or within half a pixel, the photo's pixels within 3 levels;
//! text sits up to a pixel lower (egui's baseline).
//!
//! Deviations:
//! - The card is drawn with the shell only (`Engine::shell`, the window's): the headless shots and
//!   the map's tests draw the map alone, as U6's shell is (an aircraft hovered there would
//!   otherwise gain the card in pictures written without it).
//! - The credit's `text-shadow: 0 1px 2px` is drawn unblurred, 1 px down, at half its alpha (as
//!   the panels' photos, `super::panel`).

use std::sync::Arc;

use egui::epaint::{Galley, RectShape};
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, Context, CornerRadius, Id, LayerId, Order, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};
use rustradar_core::model::{LiveFlight, airline_logo_url};
use rustradar_core::text::group_thousands;

use super::panel::{self, Remote};
use super::text::{self, Style};
use super::{anim, theme};
use crate::state::pick::HoverCard;

/// `CARD_WIDTH`.
pub const WIDTH: f32 = 272.0;
/// `OFFSET`: how far right of and below the pointer the card's corner goes.
pub const OFFSET: f32 = 16.0;
/// `MARGIN`: the least room between the card and the map's edges.
pub const MARGIN: f32 = 8.0;
/// The photo's aspect ratio (`aspect-ratio: 16 / 9`).
pub const PHOTO_ASPECT: f32 = 16.0 / 9.0;
/// The photo's height across the card.
pub const PHOTO_HEIGHT: f32 = WIDTH / PHOTO_ASPECT;
/// The gap under the photo (`margin-bottom: 6px`).
pub const PHOTO_GAP: f32 = 6.0;
/// The credit's place in the photo (`left: var(--space-2); bottom: var(--space-1)`).
pub const CREDIT_INSET: Vec2 = Vec2::new(theme::SPACE_2, theme::SPACE_1);
/// The body's padding: top, sides, bottom (`var(--space-2) var(--space-3) var(--space-3)`).
pub const BODY_PAD: [f32; 3] = [theme::SPACE_2, theme::SPACE_3, theme::SPACE_3];
/// The gap between the body's rows.
pub const ROW_GAP: f32 = 2.0;
/// The head row's least height and the gap between its items.
pub const HEAD_HEIGHT: f32 = 26.0;
pub const HEAD_GAP: f32 = theme::SPACE_2;
/// The type chip's side padding and line height.
pub const CHIP_PAD: f32 = 5.0;
pub const CHIP_LINE: f32 = 16.0;
/// The logo tile's content height, padding and the picture's largest size.
pub const LOGO_HEIGHT: f32 = 22.0;
pub const LOGO_PAD: Vec2 = Vec2::new(5.0, 2.0);
pub const LOGO_MAX: Vec2 = Vec2::new(72.0, 18.0);
/// The progress track: its height and the room above and below it.
pub const TRACK_HEIGHT: f32 = 2.0;
pub const TRACK_MARGIN: [f32; 2] = [theme::SPACE_2, theme::SPACE_1];
/// What the card says without a callsign or a flight number.
pub const NO_CALLSIGN: &str = "No callsign";

/// The callsign shown: `callsign || flight || "No callsign"`.
pub fn callsign(flight: &LiveFlight) -> &str {
    [flight.callsign.as_str(), flight.flight.as_str()].into_iter().find(|s| !s.is_empty()).unwrap_or(NO_CALLSIGN)
}

/// The route line: `{from} to {to}`, `From {from}`, `To {to}`, or nothing.
pub fn route(from: &str, to: &str) -> String {
    match (from.is_empty(), to.is_empty()) {
        (false, false) => format!("{from} to {to}"),
        (false, true) => format!("From {from}"),
        (true, false) => format!("To {to}"),
        (true, true) => String::new(),
    }
}

/// The numbers line: `{alt} ft · {speed} kt` (`alt.toLocaleString()`), or `On ground · {speed} kt`.
pub fn numbers(flight: &LiveFlight) -> String {
    let altitude = if flight.on_ground { "On ground".to_owned() } else { format!("{} ft", group_thousands(f64::from(flight.alt))) };
    format!("{altitude} · {} kt", flight.speed)
}

/// Where the card's top left corner goes for the pointer at `pointer` in a map of `area` (both CSS
/// px from the map's top left) with the card `height` px tall (see the module documentation).
pub fn place(pointer: [f32; 2], area: Vec2, height: f32) -> Pos2 {
    let [x, y] = pointer;
    let mut left = x + OFFSET;
    if left + WIDTH > area.x - MARGIN {
        left = x - OFFSET - WIDTH;
    }
    let mut top = y + OFFSET;
    if top + height > area.y - MARGIN {
        top = y - OFFSET - height;
    }
    Pos2::new(left.max(MARGIN), top.max(MARGIN))
}

/// A CSS transition of one number with `--ease-out` (CSS Transitions §3, as `anim`'s: a change of
/// target starts from the current value, going back where a running one began takes as long as the
/// way come). Kept in the card's memory, so a new mount starts without one.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Tween {
    from: f32,
    to: f32,
    /// Where it counts as having begun when a later one asks to go back, and the shortening.
    reversing_from: f32,
    shortening: f32,
    start: f64,
    seconds: f32,
}

impl Tween {
    /// One standing at `value` (an element's first style: no transition).
    fn at_rest(value: f32) -> Tween {
        Tween { from: value, to: value, reversing_from: value, shortening: 1.0, start: 0.0, seconds: 0.0 }
    }

    fn progress(&self, now: f64) -> f32 {
        if self.seconds <= 0.0 { 1.0 } else { (((now - self.start) / f64::from(self.seconds)) as f32).clamp(0.0, 1.0) }
    }

    fn value(&self, now: f64) -> f32 {
        self.from + (self.to - self.from) * anim::ease_out(self.progress(now))
    }

    /// Towards `target` over `seconds` from `now`: the value now.
    fn toward(&mut self, target: f32, seconds: f32, now: f64) -> f32 {
        if self.to != target {
            let current = self.value(now);
            *self = if self.progress(now) < 1.0 && target == self.reversing_from {
                let shortening = (anim::ease_out(self.progress(now)) * self.shortening + (1.0 - self.shortening)).abs().clamp(0.0, 1.0);
                Tween { from: current, to: target, reversing_from: self.to, shortening, start: now, seconds: seconds * shortening }
            } else {
                Tween { from: current, to: target, reversing_from: current, shortening: 1.0, start: now, seconds }
            };
        }
        self.value(now)
    }

    fn running(&self, now: f64) -> bool {
        self.progress(now) < 1.0
    }
}

/// The card's own state while it is mounted (see the module documentation).
#[derive(Debug, Clone, Default)]
struct Memory {
    /// The pass it was last drawn in; whether this mount has drawn yet.
    pass: u64,
    mounted: bool,
    /// `photoLoaded` and when (egui's time); `photoFailed`; `logoFailed`.
    photo_loaded: String,
    loaded_at: f64,
    photo_failed: String,
    logo_failed: u32,
    /// The photo slot's openness; the track's fill, while the track is there.
    slot: Tween,
    fill: Option<Tween>,
    /// `cardHeight`: the card's height at the last pass.
    height: f32,
}

fn memory_id() -> Id {
    Id::unique("rustradar::hover_card")
}

/// The layer the card is painted on: over the map, under every panel.
pub fn layer() -> LayerId {
    LayerId::new(Order::Background, Id::unique("rustradar::hover_card::layer"))
}

/// What [`ui`] drew, screen points.
#[derive(Debug, Clone, PartialEq)]
pub struct Painted {
    /// The whole card: the photo slot and the body.
    pub rect: Rect,
    /// The photo slot as open as it is now (zero high when shut), and the photo block in it.
    pub slot: Rect,
    pub photo: Rect,
    /// Whether the picture is drawn, and its opacity.
    pub picture: Option<f32>,
    /// The body (`.body.surface`), its border included.
    pub body: Rect,
    /// The head row's items: the callsign's line box, the chip, the registration's line box, the
    /// logo tile (each when there).
    pub callsign: Rect,
    pub chip: Option<Rect>,
    pub reg: Option<Rect>,
    pub logo: Option<Rect>,
    /// The logo's picture, when loaded.
    pub logo_picture: Option<Rect>,
    /// The route's and the numbers' line boxes, the track and its fill.
    pub route: Option<Rect>,
    pub track: Option<Rect>,
    pub fill: Option<Rect>,
    pub numbers: Rect,
    /// The texts drawn: the callsign (cut as shown), the route, the numbers, the credit.
    pub callsign_text: String,
    pub route_text: String,
    pub numbers_text: String,
    pub credit: Option<String>,
}

fn layout(ctx: &Context, job: LayoutJob) -> Arc<Galley> {
    ctx.fonts_mut(|f| f.layout_job(job))
}

/// The text a galley shows (its rows' glyphs, the cut's `…` included).
fn shown(galley: &Galley) -> String {
    galley.rows.iter().map(|r| r.text()).collect()
}

/// `.data` text on one row as Chromium sets it: every character in Overpass Mono, its `·` too.
/// `--font-data` names no "Middle Dot" face, so the dot keeps Mono's full advance there, where
/// [`text::job`] narrows it as in UI text.
fn data_job(text: &str, style: &Style) -> LayoutJob {
    let mut job = LayoutJob { wrap: TextWrapping::no_max_width(), break_on_newline: true, ..Default::default() };
    job.append(text, 0.0, style.format());
    job
}

/// Paints `galley` centred in the line box `line` from its left.
fn paint_text(painter: &Painter, galley: Arc<Galley>, line: Rect) {
    let at = Pos2::new(line.left(), line.top() + 0.5 * (line.height() - galley.size().y));
    painter.galley(at, galley, theme::TEXT_1);
}

/// The hover card in the UI pass: `card`, if there is one (see the module documentation).
pub fn ui(ctx: &Context, card: Option<&HoverCard>) -> Option<Painted> {
    let card = card?;
    let id = memory_id();
    let pass = ctx.cumulative_pass_nr();
    let mut memory: Memory = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    // a card not drawn in the pass before is mounted afresh
    if memory.pass + 1 < pass || !memory.mounted {
        memory = Memory::default();
    }
    memory.pass = pass;
    let painted = paint(ctx, card, &mut memory);
    ctx.data_mut(|d| d.insert_temp(id, memory));
    Some(painted)
}

fn paint(ctx: &Context, card: &HoverCard, memory: &mut Memory) -> Painted {
    let ppp = ctx.pixels_per_point();
    let time = ctx.input(|i| i.time);
    let map = ctx.content_rect();
    let flight = &card.flight;

    // the photo: `images[0].thumbnail || medium`, and whether its room is kept
    let image = card.info.as_ref().and_then(|i| i.images.first());
    let src = image.map(|i| if i.thumbnail.is_empty() { i.medium.as_str() } else { i.thumbnail.as_str() }).unwrap_or("");
    let mut picture = None;
    if !src.is_empty() && memory.photo_failed != src {
        match panel::remote(ctx, src) {
            Remote::Ready(texture) => {
                if memory.photo_loaded != src {
                    memory.photo_loaded = src.to_owned();
                    memory.loaded_at = time;
                }
                picture = Some(texture);
            }
            Remote::Failed => memory.photo_failed = src.to_owned(),
            Remote::Loading => {}
        }
    }
    let has_photo = !src.is_empty() && memory.photo_failed != src;
    let photo_open = card.status == rustradar_core::hover::HoverStatus::Loading || (card.status == rustradar_core::hover::HoverStatus::Ready && has_photo);
    let target = if photo_open { 1.0 } else { 0.0 };
    if !memory.mounted {
        memory.slot = Tween::at_rest(target);
        memory.mounted = true;
    }
    let open = memory.slot.toward(target, anim::duration(ctx, anim::PHOTO_SLOT), time);
    if memory.slot.running(time) {
        ctx.request_repaint();
    }
    let slot_height = open * (PHOTO_HEIGHT + PHOTO_GAP);

    // the logo: `info.logoId` unless it failed
    let logo_id = card.info.as_ref().map_or(0, |i| i.logo_id);
    let logo_id = if logo_id != memory.logo_failed { logo_id } else { 0 };
    let logo = (logo_id != 0).then(|| panel::remote(ctx, &airline_logo_url(logo_id)));
    if logo == Some(Remote::Failed) {
        memory.logo_failed = logo_id;
    }
    let logo = logo.filter(|l| *l != Remote::Failed);

    // the body's rows
    let inner = WIDTH - 2.0 * theme::BORDER - 2.0 * BODY_PAD[1];
    let callsign_style = Style::data(theme::TEXT_BASE, 700, theme::TEXT_1);
    let chip_style = Style::data(theme::TEXT_XS, 600, theme::TEXT_2).line_height(CHIP_LINE / theme::TEXT_XS);
    let reg_style = Style::data(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_3);
    let route_style = Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2);
    let numbers_style = Style::data(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2);
    let chip = (!flight.typecode.is_empty()).then(|| layout(ctx, text::job(&flight.typecode, &chip_style)));
    let reg = (!flight.reg.is_empty()).then(|| layout(ctx, text::job(&flight.reg, &reg_style)));
    let logo_picture = match logo {
        Some(Remote::Ready(texture)) => Some((texture.id, panel::contain(texture.size, LOGO_MAX))),
        _ => None,
    };
    let chip_width = chip.as_ref().map(|g| g.size().x + 2.0 * CHIP_PAD + 2.0 * theme::BORDER);
    let reg_width = reg.as_ref().map(|g| g.size().x);
    let logo_width = logo.map(|_| logo_picture.map_or(0.0, |(_, size)| size.x) + 2.0 * LOGO_PAD.x);
    let fixed: Vec<f32> = [chip_width, reg_width, logo_width].into_iter().flatten().collect();
    let room = inner - fixed.iter().sum::<f32>() - HEAD_GAP * fixed.len() as f32;
    let callsign_text = callsign(flight);
    let mut callsign_job = text::job(callsign_text, &callsign_style);
    let natural = text::measure(ctx, callsign_job.clone()).x;
    if natural > room {
        text::truncate(&mut callsign_job, room.max(0.0));
    }
    let callsign_galley = layout(ctx, callsign_job);
    let callsign_width = natural.min(room.max(0.0));
    let route_text = route(&card.from, &card.to);
    let route_galley = (!route_text.is_empty()).then(|| layout(ctx, text::truncated(&route_text, &route_style, inner)));
    let numbers_text = numbers(flight);
    let numbers_galley = layout(ctx, data_job(&numbers_text, &numbers_style));
    let progress = card.progress;
    let pct = progress.map_or(0.0, |p| p.value.clamp(0.0, 1.0)) as f32;

    // heights down the body
    let rows: Vec<f32> = [
        Some(HEAD_HEIGHT),
        route_galley.as_ref().map(|_| route_style.line_px()),
        progress.map(|_| TRACK_MARGIN[0] + TRACK_HEIGHT + TRACK_MARGIN[1]),
        Some(numbers_style.line_px()),
    ]
    .into_iter()
    .flatten()
    .collect();
    let body_height = 2.0 * theme::BORDER + BODY_PAD[0] + BODY_PAD[2] + rows.iter().sum::<f32>() + ROW_GAP * (rows.len() - 1) as f32;
    let height = slot_height + body_height;
    memory.height = height;

    // where: from the pointer, the corner on a whole device pixel (`offsetHeight` is whole px)
    let corner = place([card.pointer[0] as f32, card.pointer[1] as f32], map.size(), height.round());
    let corner = Pos2::new(panel::snap(map.min.x + corner.x, ppp), panel::snap(map.min.y + corner.y, ppp));
    let rect = Rect::from_min_size(corner, Vec2::new(WIDTH, height));
    let painter = ctx.layer_painter(layer()).with_clip_rect(map);

    // the photo slot
    let slot = Rect::from_min_size(corner, Vec2::new(WIDTH, slot_height));
    let photo = Rect::from_min_size(corner, Vec2::new(WIDTH, PHOTO_HEIGHT));
    let mut picture_opacity = None;
    let mut credit = None;
    if slot_height > 0.0 {
        let clip = painter.with_clip_rect(panel::snap_rect(slot, ppp).intersect(map));
        let block = panel::snap_rect(photo, ppp);
        clip.rect_filled(block, CornerRadius::same(theme::RADIUS), theme::SURFACE_RAISED);
        if let Some(texture) = picture.filter(|_| memory.photo_loaded == src) {
            let t = if anim::reduced_motion(ctx) { 1.0 } else { ((time - memory.loaded_at) as f32 / anim::PHOTO_FADE).clamp(0.0, 1.0) };
            if t < 1.0 {
                ctx.request_repaint();
            }
            let opacity = anim::ease_out(t);
            let shape = RectShape::filled(block, CornerRadius::same(theme::RADIUS), Color32::WHITE.gamma_multiply(opacity))
                .with_texture(texture.id, panel::cover_uv(texture.size, block.size()));
            clip.add(shape);
            picture_opacity = Some(opacity);
            if let Some(copyright) = image.map(|i| i.copyright.as_str()).filter(|c| !c.is_empty()) {
                let style = Style::ui(theme::TEXT_XS, theme::WEIGHT_REGULAR, theme::TEXT_1);
                let text = format!("© {copyright}");
                let galley = layout(ctx, text::truncated(&text, &style, WIDTH - 2.0 * CREDIT_INSET.x));
                let line = Rect::from_min_size(Pos2::new(photo.left() + CREDIT_INSET.x, photo.bottom() - CREDIT_INSET.y - style.line_px()), Vec2::new(galley.size().x, style.line_px()));
                let at = Pos2::new(line.left(), line.top() + 0.5 * (line.height() - galley.size().y));
                let shadow = theme::PHOTO_TEXT_SHADOW.gamma_multiply(0.5);
                clip.galley_with_override_text_color(at + Vec2::new(0.0, 1.0), galley.clone(), shadow);
                credit = Some(shown(&galley));
                clip.galley(at, galley, theme::TEXT_1);
            }
        }
    }

    // the body
    let body = Rect::from_min_size(Pos2::new(corner.x, corner.y + slot_height), Vec2::new(WIDTH, body_height));
    let body_px = panel::snap_rect(body, ppp);
    painter.rect(body_px, CornerRadius::same(theme::RADIUS), theme::SURFACE, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
    let left = body.left() + theme::BORDER + BODY_PAD[1];
    let mut y = body.top() + theme::BORDER + BODY_PAD[0];

    // head row: items centred in it, 8 apart, the logo at the right
    let centred = |h: f32| y + 0.5 * (HEAD_HEIGHT - h);
    let mut x = left;
    let callsign_line = Rect::from_min_size(Pos2::new(x, centred(callsign_style.line_px())), Vec2::new(callsign_width, callsign_style.line_px()));
    let callsign_shown = shown(&callsign_galley);
    paint_text(&painter, callsign_galley, callsign_line);
    x += callsign_width;
    let mut chip_rect = None;
    if let (Some(galley), Some(width)) = (chip, chip_width) {
        x += HEAD_GAP;
        let r = Rect::from_min_size(Pos2::new(x, centred(CHIP_LINE + 2.0 * theme::BORDER)), Vec2::new(width, CHIP_LINE + 2.0 * theme::BORDER));
        painter.rect_stroke(panel::snap_rect(r, ppp), CornerRadius::same(theme::RADIUS_SM), Stroke::new(theme::BORDER, theme::LINE_CONTROL), StrokeKind::Inside);
        let line = Rect::from_min_size(Pos2::new(r.left() + theme::BORDER + CHIP_PAD, r.top() + theme::BORDER), Vec2::new(galley.size().x, CHIP_LINE));
        paint_text(&painter, galley, line);
        chip_rect = Some(r);
        x += width;
    }
    let mut reg_rect = None;
    if let (Some(galley), Some(width)) = (reg, reg_width) {
        x += HEAD_GAP;
        let line = Rect::from_min_size(Pos2::new(x, centred(reg_style.line_px())), Vec2::new(width, reg_style.line_px()));
        paint_text(&painter, galley, line);
        reg_rect = Some(line);
    }
    let (mut logo_rect, mut logo_picture_rect) = (None, None);
    if let Some(width) = logo_width {
        let tile_height = LOGO_HEIGHT + 2.0 * LOGO_PAD.y;
        let r = Rect::from_min_size(Pos2::new(left + inner - width, centred(tile_height)), Vec2::new(width, tile_height));
        painter.rect_filled(panel::snap_rect(r, ppp), CornerRadius::same(theme::RADIUS_SM), theme::TEXT_1);
        if let Some((texture, size)) = logo_picture {
            let at = Pos2::new(r.left() + LOGO_PAD.x, r.top() + LOGO_PAD.y + 0.5 * (LOGO_HEIGHT - size.y));
            let image = Rect::from_min_size(at, size);
            painter.image(texture, image, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            logo_picture_rect = Some(image);
        }
        logo_rect = Some(r);
    }
    y += HEAD_HEIGHT;

    // the route
    let mut route_rect = None;
    if let Some(galley) = route_galley {
        y += ROW_GAP;
        let line = Rect::from_min_size(Pos2::new(left, y), Vec2::new(galley.size().x, route_style.line_px()));
        paint_text(&painter, galley, line);
        route_rect = Some(line);
        y += route_style.line_px();
    }

    // the progress track: a new track stands at its value, then eases to the next
    let (mut track_rect, mut fill_rect) = (None, None);
    if let Some(progress) = progress {
        y += ROW_GAP;
        let track = Rect::from_min_size(Pos2::new(left, y + TRACK_MARGIN[0]), Vec2::new(inner, TRACK_HEIGHT));
        let track_px = panel::snap_rect(track, ppp);
        painter.rect_filled(track_px, CornerRadius::same(1), theme::LINE);
        // a new track stands at its value, then eases to the next
        let fill = memory.fill.get_or_insert(Tween::at_rest(pct));
        let scale = fill.toward(pct, anim::duration(ctx, anim::HOVER_CARD_PROGRESS), time);
        if fill.running(time) {
            ctx.request_repaint();
        }
        let fill = Rect::from_min_size(track_px.min, Vec2::new(track_px.width() * scale, track_px.height()));
        if fill.width() > 0.0 {
            // the fill is clipped by the track's rounded box (`overflow: hidden`)
            let clip = painter.with_clip_rect(fill.intersect(map));
            clip.rect_filled(track_px, CornerRadius::same(1), if progress.measured { theme::AMBER } else { theme::TEXT_3 });
        }
        track_rect = Some(track);
        fill_rect = Some(fill);
        y += TRACK_MARGIN[0] + TRACK_HEIGHT + TRACK_MARGIN[1];
    }
    if progress.is_none() {
        memory.fill = None;
    }

    // the numbers
    y += ROW_GAP;
    let numbers_line = Rect::from_min_size(Pos2::new(left, y), Vec2::new(numbers_galley.size().x, numbers_style.line_px()));
    paint_text(&painter, numbers_galley, numbers_line);

    Painted {
        rect,
        slot,
        photo,
        picture: picture_opacity,
        body,
        callsign: callsign_line,
        chip: chip_rect,
        reg: reg_rect,
        logo: logo_rect,
        logo_picture: logo_picture_rect,
        route: route_rect,
        track: track_rect,
        fill: fill_rect,
        numbers: numbers_line,
        callsign_text: callsign_shown,
        route_text,
        numbers_text,
        credit,
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::hover::{HoverInfo, HoverStatus};
    use rustradar_core::model::AircraftImage;

    use super::*;
    use crate::state::pick::Progress;

    fn flight() -> LiveFlight {
        LiveFlight {
            id: 0x3a8f_21c4,
            lat: 59.6,
            lon: 17.9,
            track: 200,
            alt: 34_000,
            speed: 451,
            callsign: "SAS1417".into(),
            flight: "SK1417".into(),
            reg: "SE-ROM".into(),
            typecode: "A20N".into(),
            origin: "ARN".into(),
            destination: "CPH".into(),
            ..Default::default()
        }
    }

    fn card(status: HoverStatus, info: Option<HoverInfo>) -> HoverCard {
        HoverCard {
            flight: flight(),
            from: "Stockholm".into(),
            to: "Copenhagen".into(),
            progress: Some(Progress { value: 0.42, measured: true }),
            info,
            status,
            pointer: [400.0, 300.0],
        }
    }

    /// Runs one pass of a 1440 × 900 context at `time` drawing `card`.
    fn pass(ctx: &Context, time: f64, card: Option<&HoverCard>) -> Option<Painted> {
        let input = egui::RawInput {
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))),
            ..Default::default()
        };
        let mut painted = None;
        let mut output = ctx.run_ui(input, |u| painted = ui(u.ctx(), card));
        output.textures_delta.clear();
        painted
    }

    fn context() -> Context {
        let ctx = Context::default();
        theme::apply(&ctx);
        ctx
    }

    #[test]
    fn the_texts() {
        let mut f = flight();
        assert_eq!(callsign(&f), "SAS1417");
        f.callsign.clear();
        assert_eq!(callsign(&f), "SK1417");
        f.flight.clear();
        assert_eq!(callsign(&f), "No callsign");
        assert_eq!(route("Stockholm", "Copenhagen"), "Stockholm to Copenhagen");
        assert_eq!(route("Stockholm", ""), "From Stockholm");
        assert_eq!(route("", "CPH"), "To CPH");
        assert_eq!(route("", ""), "");
        assert_eq!(numbers(&flight()), "34,000 ft · 451 kt");
        let mut ground = flight();
        ground.on_ground = true;
        ground.speed = 12;
        assert_eq!(numbers(&ground), "On ground · 12 kt");
        ground.on_ground = false;
        ground.alt = 975;
        assert_eq!(numbers(&ground), "975 ft · 12 kt");
    }

    #[test]
    fn placed_right_and_below_the_pointer_and_flipped_at_the_edges() {
        let area = Vec2::new(1440.0, 900.0);
        assert_eq!(place([400.0, 300.0], area, 262.0), Pos2::new(416.0, 316.0));
        // within 8 px of the right edge: on the pointer's left
        assert_eq!(place([1144.0, 300.0], area, 262.0), Pos2::new(1160.0, 316.0));
        assert_eq!(place([1145.0, 300.0], area, 262.0), Pos2::new(1145.0 - 16.0 - 272.0, 316.0));
        // within 8 px of the bottom: above it
        assert_eq!(place([400.0, 614.0], area, 262.0), Pos2::new(416.0, 630.0));
        assert_eq!(place([400.0, 615.0], area, 262.0), Pos2::new(416.0, 615.0 - 16.0 - 262.0));
        // never nearer than 8 px to the left and top edges
        assert_eq!(place([100.0, 100.0], Vec2::new(300.0, 200.0), 262.0), Pos2::new(8.0, 8.0));
    }

    #[test]
    fn the_body_s_rows_add_up_as_css_lays_them_out() {
        let ctx = context();
        // details in, no photo: the slot shut, the body alone
        let ready = card(HoverStatus::Ready, Some(HoverInfo { images: Vec::new(), logo_id: 0, progress_pct: Some(42) }));
        let p = pass(&ctx, 0.0, Some(&ready)).unwrap();
        assert_eq!(p.slot.height(), 0.0);
        assert_eq!(p.rect.min, Pos2::new(416.0, 316.0));
        // 1 + 8 + 26 + 2 + 18.2 + 2 + 14 + 2 + 16.8 + 12 + 1
        assert!((p.body.height() - 103.0).abs() < 1e-3, "{}", p.body.height());
        assert!((p.callsign.top() - (p.body.top() + 9.0 + 3.2)).abs() < 1e-3);
        assert_eq!(p.chip.unwrap().height(), 18.0);
        assert!((p.chip.unwrap().top() - (p.body.top() + 13.0)).abs() < 1e-3);
        assert!((p.route.unwrap().top() - (p.body.top() + 37.0)).abs() < 1e-3);
        assert!((p.track.unwrap().top() - (p.body.top() + 65.2)).abs() < 1e-3);
        assert_eq!(p.track.unwrap().width(), 246.0);
        assert!((p.numbers.top() - (p.body.top() + 73.2)).abs() < 1e-3);
        assert_eq!(p.route_text, "Stockholm to Copenhagen");
        assert_eq!(p.numbers_text, "34,000 ft · 451 kt");
        assert_eq!(p.callsign_text, "SAS1417");
        // the chip after the callsign, the registration after it, 8 apart
        let (callsign, chip, reg) = (p.callsign, p.chip.unwrap(), p.reg.unwrap());
        assert!((chip.left() - callsign.right() - 8.0).abs() < 1e-3 && (reg.left() - chip.right() - 8.0).abs() < 1e-3);
        assert_eq!(p.logo, None);
    }

    #[test]
    fn the_photo_slot_is_open_while_loading_and_eases_shut_without_a_photo() {
        let ctx = context();
        let loading = card(HoverStatus::Loading, None);
        let p = pass(&ctx, 0.0, Some(&loading)).unwrap();
        assert_eq!(p.slot.height(), PHOTO_HEIGHT + PHOTO_GAP, "open at once on mounting");
        assert_eq!(p.photo.size(), Vec2::new(272.0, 153.0));
        assert!((p.body.top() - p.rect.top() - 159.0).abs() < 1e-3);
        assert_eq!(p.picture, None, "no picture yet");
        // the details came without a photo: the slot shuts over 180 ms
        let ready = card(HoverStatus::Ready, Some(HoverInfo::default()));
        pass(&ctx, 1.0, Some(&ready));
        let mid = pass(&ctx, 1.09, Some(&ready)).unwrap();
        assert!(mid.slot.height() > 0.0 && mid.slot.height() < 159.0 * 0.2, "{} eased", mid.slot.height());
        let shut = pass(&ctx, 1.2, Some(&ready)).unwrap();
        assert_eq!(shut.slot.height(), 0.0);
        // unavailable: shut too
        let unavailable = card(HoverStatus::Unavailable, None);
        assert_eq!(pass(&ctx, 1.3, Some(&unavailable)).unwrap().slot.height(), 0.0);
        // a photo whose picture cannot load (no loader for it here) closes the slot as well
        let photo = HoverInfo { images: vec![AircraftImage { thumbnail: "test://nowhere.jpg".into(), copyright: "A. Spotter".into(), ..Default::default() }], logo_id: 0, progress_pct: None };
        let failed = card(HoverStatus::Ready, Some(photo));
        assert_eq!(pass(&ctx, 2.0, Some(&failed)).unwrap().slot.height(), 0.0);
    }

    #[test]
    fn a_new_mount_stands_at_its_values() {
        let ctx = context();
        let ready = card(HoverStatus::Ready, Some(HoverInfo::default()));
        assert_eq!(pass(&ctx, 0.0, Some(&ready)).unwrap().slot.height(), 0.0);
        assert!(pass(&ctx, 0.1, None).is_none());
        // mounted again while loading: open at once, no transition
        let loading = card(HoverStatus::Loading, None);
        assert_eq!(pass(&ctx, 0.2, Some(&loading)).unwrap().slot.height(), 159.0);
    }

    #[test]
    fn the_fill_stands_then_eases_to_a_new_value() {
        let ctx = context();
        let mut c = card(HoverStatus::Ready, Some(HoverInfo::default()));
        let p = pass(&ctx, 0.0, Some(&c)).unwrap();
        assert!((p.fill.unwrap().width() - 0.42 * 246.0).abs() < 1e-3);
        c.progress = Some(Progress { value: 1.4, measured: false });
        pass(&ctx, 1.0, Some(&c));
        let mid = pass(&ctx, 1.1, Some(&c)).unwrap();
        let w = mid.fill.unwrap().width();
        assert!(w > 0.42 * 246.0 && w < 246.0, "{w}");
        assert!((pass(&ctx, 1.5, Some(&c)).unwrap().fill.unwrap().width() - 246.0).abs() < 1e-3, "clamped to 1");
        // no progress: no track
        c.progress = None;
        let p = pass(&ctx, 2.0, Some(&c)).unwrap();
        assert_eq!((p.track, p.fill), (None, None));
        assert!((p.body.height() - 87.0).abs() < 1e-3);
    }

    #[test]
    fn a_long_callsign_is_cut_and_the_others_keep_their_width() {
        let ctx = context();
        let mut c = card(HoverStatus::Ready, Some(HoverInfo::default()));
        c.flight.callsign = "AVERYLONGCALLSIGNTHATDOESNOTFIT".into();
        c.flight.reg = "N123456789XYZ".into();
        let p = pass(&ctx, 0.0, Some(&c)).unwrap();
        assert!(p.callsign_text.ends_with('…'), "{}", p.callsign_text);
        let reg = p.reg.unwrap();
        assert!(reg.right() <= p.body.right() - 13.0 + 1e-3, "inside the padding");
        assert!((p.chip.unwrap().left() - p.callsign.right() - 8.0).abs() < 1e-3);
    }

    #[test]
    fn near_the_bottom_right_the_card_goes_above_and_left() {
        let ctx = context();
        let mut c = card(HoverStatus::Loading, None);
        c.pointer = [1300.0, 800.0];
        let p = pass(&ctx, 0.0, Some(&c)).unwrap();
        let height = (159.0 + 103.0_f32).round();
        assert_eq!(p.rect.min, Pos2::new(1300.0 - 16.0 - 272.0, 800.0 - 16.0 - height));
    }
}
