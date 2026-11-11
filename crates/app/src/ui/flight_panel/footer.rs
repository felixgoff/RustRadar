//! The flight panel's footer (item U10; doc 06 §7.5; `FlightPanel.svelte`'s `<footer>`), pinned
//! to the bottom of the panel ([`super::super::panel::body`]).
//!
//! - **Status** (13 px, 8 px between its parts): `live`: the pulsing `--live` dot and `Live`
//!   (600, `--live`); `ended`: the hollow dot and `Last seen <Clock>` in the departure airport's
//!   zone (else the viewer's), `--text-2`, with the tooltip `The last report, in {IATA} time` (or
//!   `your time zone`), or `Not live` when no report was ever timed; `connecting` and
//!   `reconnecting`: the 900 ms spinner (14/2) and `Connecting…` / `Reconnecting…`.
//! - **Buttons**: Follow ([`super::super::panel::follow_button`]: the icon alone, 32 × 32, unless
//!   the flight is live, so the footer keeps one row; pressed while the camera follows the
//!   aircraft; [`Action::ToggleFollow`], which locks or frees the camera as the key C does), and
//!   `Flightradar24` with `ExternalLink` 14/1.75, opening the flight on Flightradar24's site.

use egui::text::LayoutJob;
use egui::{Context, Pos2, Rect, Response, Vec2};

use super::super::icons::{self, Icon};
use super::super::panel::{self, Footer, Piece};
use super::super::text::{self, Style};
use super::super::theme;
use super::super::widgets::button::Button;
use super::super::widgets::live_dot::{self, LiveDot};
use super::super::widgets::spinner;
use super::View;
use crate::state::Action;
use crate::state::flight::FollowState;

/// The status's texts.
pub const LIVE: &str = "Live";
pub const NOT_LIVE: &str = "Not live";
pub const LAST_SEEN: &str = "Last seen ";
pub const CONNECTING: &str = "Connecting…";
pub const RECONNECTING: &str = "Reconnecting…";
/// The Flightradar24 button's text.
pub const FLIGHTRADAR24: &str = "Flightradar24";

/// The gap between the status's parts (`.status { gap: var(--space-2) }`).
const STATUS_GAP: f32 = theme::SPACE_2;

/// `.status`: 13 px `--text-2`.
const fn status_style() -> Style {
    Style::ui(theme::TEXT_MD, theme::WEIGHT_REGULAR, theme::TEXT_2)
}

/// What leads the status.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Lead {
    Dot(LiveDot),
    Spinner,
}

impl Lead {
    fn size(self) -> f32 {
        match self {
            Lead::Dot(_) => live_dot::SIZE,
            Lead::Spinner => spinner::SIZE,
        }
    }
}

/// The status: what leads it, its text, its tooltip.
pub fn status(view: &View<'_>) -> (LayoutJob, Option<String>, bool) {
    let style = status_style();
    let mut job = LayoutJob::default();
    match view.status {
        FollowState::Live => {
            text::append(&mut job, LIVE, &Style::ui(theme::TEXT_MD, 600, theme::LIVE));
            (job, None, true)
        }
        FollowState::Ended => {
            let seen = view.seen_at();
            if seen != 0.0 {
                text::append(&mut job, LAST_SEEN, &style);
                panel::append_clock(&mut job, &view.clocks, seen, view.seen_zone(), &style);
                (job, Some(format!("The last report, in {}", view.seen_place())), false)
            } else {
                text::append(&mut job, NOT_LIVE, &style);
                (job, None, false)
            }
        }
        FollowState::Connecting | FollowState::Reconnecting => {
            text::append(&mut job, if view.status == FollowState::Connecting { CONNECTING } else { RECONNECTING }, &style);
            (job, None, false)
        }
    }
}

/// The footer of `view` (its status measured in `ctx`): its status and its two buttons.
pub fn footer(ctx: &Context, view: &View<'_>) -> Footer<'static> {
    let (job, tip, _) = status(view);
    let lead = match view.status {
        FollowState::Live => Lead::Dot(LiveDot::Live),
        FollowState::Ended => Lead::Dot(LiveDot::Ended),
        FollowState::Connecting | FollowState::Reconnecting => Lead::Spinner,
    };
    let live = view.status == FollowState::Live;
    let follow = panel::follow_button(view.following, !live);
    let fr24 = Button::new().text(FLIGHTRADAR24).icon(Icon::ExternalLink.at(14.0, icons::STROKE_THIN));
    Footer { status: status_piece(ctx, lead, job), status_tooltip: tip, buttons: vec![follow, fr24] }
}

/// The status as one piece: its lead, 8 px, its text, centred on a 13 px line.
fn status_piece(ctx: &Context, lead: Lead, job: LayoutJob) -> Piece<'static> {
    let line = status_style().line_px();
    let text_w = text::measure(ctx, job.clone()).x;
    let size = Vec2::new(lead.size() + STATUS_GAP + text_w, line.max(lead.size()));
    Piece::Custom(
        size,
        Box::new(move |painter, rect: Rect| {
            let mid = rect.center().y;
            let lead_rect = Rect::from_center_size(Pos2::new(rect.left() + lead.size() / 2.0, mid), Vec2::splat(lead.size()));
            match lead {
                Lead::Dot(dot) => dot.paint(painter, lead_rect.center()),
                Lead::Spinner => spinner::paint(painter, lead_rect, spinner::SIZE, theme::TEXT_2),
            }
            let galley = painter.layout_job(job);
            painter.galley(Pos2::new(lead_rect.right() + STATUS_GAP, mid - galley.size().y / 2.0), galley, theme::TEXT_2);
        }),
    )
}

/// What the footer's buttons asked: Follow toggles the camera's lock, Flightradar24 opens the
/// flight's page.
pub fn act(ctx: &Context, view: &View<'_>, responses: &[Response], actions: &mut Vec<Action>) {
    if responses.first().is_some_and(Response::clicked) {
        actions.push(Action::ToggleFollow);
    }
    if responses.get(1).is_some_and(Response::clicked) {
        ctx.open_url(egui::OpenUrl::new_tab(view.fr24_url()));
    }
}
