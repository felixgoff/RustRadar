//! Animation (doc 06 §1.3, §27; plan 1.5): the one easing curve, every transition's duration,
//! CSS transitions on egui's clock, the infinite animations' phases, and reduced motion.
//!
//! - [`ease_out`] is `--ease-out`, `cubic-bezier(0.16, 1, 0.3, 1)`, solved as Blink solves a
//!   cubic Bézier timing function (Newton's method, then bisection, to 1e-7).
//! - [`toggle`] animates a two-state transition (hover, pressed, a switch's knob) from 0 to 1
//!   and back; [`value`] a numeric one (a sheet's height, a compass needle,
//!   a progress fill). Both follow CSS Transitions: a change mid-way starts from where the value
//!   is, and going back to where the running transition began takes only as long as the way
//!   already come (the reversing shortening factor, CSS Transitions §3).
//! - [`enter`] plays an element's entering animation once (a detail panel, a popover, a dialog).
//! - [`cycle`] is an infinite animation's phase (skeleton shimmer, spinner, live dot) and keeps
//!   egui repainting while one is on screen (doc 06 §27).
//! - Reduced motion (`prefers-reduced-motion: reduce`, app.css:242-250) makes every transition and
//!   animation 0.01 ms long and run once: transitions jump to their end, infinite animations stand
//!   still at their start ([`cycle`] answers `None`). The platform glue sets it from the system's
//!   setting with [`set_reduced_motion`]; nothing here reads the system.
//!
//! Times are in seconds of egui's clock (`RawInput::time`), so a shot's frames, a second apart,
//! see every 150 ms transition finished.

use egui::{Context, Id};

/// `--ease-out`: `cubic-bezier(0.16, 1, 0.3, 1)`.
pub const EASE_OUT: CubicBezier = CubicBezier::new(0.16, 1.0, 0.3, 1.0);
/// CSS's own `ease-out` keyword, `cubic-bezier(0, 0, 0.58, 1)`: only the live dot's pulse uses it
/// (FlightPanel:1340), every transition uses [`EASE_OUT`].
pub const CSS_EASE_OUT: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);

/// Hover and pressed colour changes: buttons, rows, chips, inputs' borders (150 ms).
pub const HOVER: f32 = 0.150;
/// The detail panel entering on desktop: 200 ms, from opacity 0 and [`PANEL_ENTER_DX`] to the right.
pub const PANEL_ENTER: f32 = 0.200;
/// How far right of its place the detail panel starts entering, px.
pub const PANEL_ENTER_DX: f32 = 8.0;
/// A popover (layers, settings) entering: 180 ms, from opacity 0 and [`POPOVER_ENTER_DX`] to the
/// right (on phones [`POPOVER_ENTER_DY_PHONE`] below).
pub const POPOVER_ENTER: f32 = 0.180;
/// How far right a popover starts, px.
pub const POPOVER_ENTER_DX: f32 = 6.0;
/// How far below a popover starts on phones, px.
pub const POPOVER_ENTER_DY_PHONE: f32 = 24.0;
/// A dialog entering: 180 ms, from opacity 0 and [`DIALOG_ENTER_DY`] below.
pub const DIALOG_ENTER: f32 = 0.180;
/// How far below a dialog starts, px.
pub const DIALOG_ENTER_DY: f32 = 6.0;
/// The map controls moving aside when a panel opens (`right`, 200 ms).
pub const CONTROLS_SHIFT: f32 = 0.200;
/// The map controls moving up on phones (`bottom`, 220 ms).
pub const CONTROLS_SHIFT_PHONE: f32 = 0.220;
/// The phone's top bar hiding and showing (translate and fade).
pub const TOP_BAR: f32 = 0.220;
/// The bottom sheet's height (none while it is dragged).
pub const SHEET: f32 = 0.220;
/// The flight's progress fill and marker.
pub const PROGRESS: f32 = 0.600;
/// The hover card's progress fill.
pub const HOVER_CARD_PROGRESS: f32 = 0.400;
/// The hover card's photo fading in.
pub const PHOTO_FADE: f32 = 0.160;
/// The hover card's photo slot opening and closing.
pub const PHOTO_SLOT: f32 = 0.180;
/// A switch's knob sliding [`SWITCH_KNOB_TRAVEL`].
pub const SWITCH_KNOB: f32 = 0.150;
/// How far a switch's knob slides, px.
pub const SWITCH_KNOB_TRAVEL: f32 = 14.0;
/// The mode bar's eye toggle revealing (width, margin, opacity).
pub const EYE_REVEAL: f32 = 0.150;
/// A disclosure chevron turning 90°.
pub const CHEVRON: f32 = 0.150;
/// The compass needle turning.
pub const COMPASS: f32 = 0.150;
/// The airport board dimming to [`BOARD_BUSY_OPACITY`] while it loads.
pub const BOARD_BUSY: f32 = 0.150;
/// The busy board's opacity.
pub const BOARD_BUSY_OPACITY: f32 = 0.75;
/// The skeleton's shimmer: one sweep, linear, forever.
pub const SKELETON_PERIOD: f32 = 1.4;
/// The spinner: one turn, linear, forever.
pub const SPINNER_PERIOD: f32 = 0.9;
/// The live dot's pulse: a ring from 0 to [`LIVE_DOT_RING`] px, eased out, forever.
pub const LIVE_DOT_PERIOD: f32 = 2.0;
/// The live dot's ring at the end of a pulse, px.
pub const LIVE_DOT_RING: f32 = 6.0;
/// What reduced motion makes of every duration: 0.01 ms.
pub const REDUCED: f32 = 0.000_01;

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl CubicBezier {
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        CubicBezier { x1, y1, x2, y2 }
    }

    /// The curve's progress at `x` (the fraction of the duration gone), as Blink computes it
    /// (`gfx::CubicBezier::Solve`): 0 before the start, 1 after the end.
    pub fn at(&self, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        let s = self.solve_x(x, 1e-7);
        sample(s, self.y1, self.y2)
    }

    /// The curve's parameter where its x is `x`: a few Newton steps, then bisection.
    fn solve_x(&self, x: f64, epsilon: f64) -> f64 {
        let mut t = x;
        for _ in 0..4 {
            let error = sample(t, self.x1, self.x2) - x;
            if error.abs() < epsilon {
                return t;
            }
            let slope = slope(t, self.x1, self.x2);
            if slope.abs() < 1e-6 {
                break;
            }
            t -= error / slope;
        }
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        t = x;
        while lo < hi {
            let v = sample(t, self.x1, self.x2);
            if (v - x).abs() < epsilon {
                return t;
            }
            if x > v {
                lo = t;
            } else {
                hi = t;
            }
            let next = 0.5 * (lo + hi);
            if next == t {
                break;
            }
            t = next;
        }
        t
    }
}

/// One coordinate of a unit cubic Bézier (ends at 0 and 1) with control coordinates `p1`, `p2`.
fn sample(t: f64, p1: f64, p2: f64) -> f64 {
    let c = 3.0 * p1;
    let b = 3.0 * (p2 - p1) - c;
    let a = 1.0 - c - b;
    ((a * t + b) * t + c) * t
}

fn slope(t: f64, p1: f64, p2: f64) -> f64 {
    let c = 3.0 * p1;
    let b = 3.0 * (p2 - p1) - c;
    let a = 1.0 - c - b;
    (3.0 * a * t + 2.0 * b) * t + c
}

/// `--ease-out` at `t` (0–1): how far the transition has come when `t` of its time has gone.
pub fn ease_out(t: f32) -> f32 {
    EASE_OUT.at(f64::from(t)) as f32
}

fn reduced_id() -> Id {
    Id::unique("rustradar::anim::reduced_motion")
}

/// Turns reduced motion on or off for `ctx` (the system's `prefers-reduced-motion`).
pub fn set_reduced_motion(ctx: &Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(reduced_id(), on));
}

/// Whether `ctx` runs with reduced motion.
pub fn reduced_motion(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(reduced_id())).unwrap_or(false)
}

/// `seconds` as `ctx` runs it: [`REDUCED`] under reduced motion.
pub fn duration(ctx: &Context, seconds: f32) -> f32 {
    if reduced_motion(ctx) { REDUCED.min(seconds) } else { seconds }
}

/// A running CSS transition of one number (CSS Transitions §3).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Transition {
    from: f32,
    to: f32,
    /// Where the transition counts as having begun when a later one asks to go back.
    reversing_adjusted_from: f32,
    /// The reversing shortening factor.
    shortening: f32,
    start: f64,
    /// Its duration, shortened.
    seconds: f32,
}

impl Transition {
    fn progress(&self, now: f64) -> f32 {
        if self.seconds <= 0.0 {
            return 1.0;
        }
        (((now - self.start) / f64::from(self.seconds)) as f32).clamp(0.0, 1.0)
    }

    fn eased(&self, now: f64) -> f32 {
        ease_out(self.progress(now))
    }

    fn at(&self, now: f64) -> f32 {
        let e = self.eased(now);
        self.from + (self.to - self.from) * e
    }

    fn running(&self, now: f64) -> bool {
        self.progress(now) < 1.0
    }
}

/// The value of `id`'s transition at `now` towards `target`, over `seconds` with `--ease-out`,
/// a change of target starting a new transition from the current value (with CSS's shortening
/// when it goes back). The first call stands at `target`.
fn transition(ctx: &Context, id: Id, target: f32, seconds: f32, now: f64) -> f32 {
    let seconds = duration(ctx, seconds);
    let id = id.with("rustradar::anim::transition");
    let previous: Option<Transition> = ctx.data(|d| d.get_temp(id));
    let next = match previous {
        None => Transition { from: target, to: target, reversing_adjusted_from: target, shortening: 1.0, start: now, seconds: 0.0 },
        Some(t) if t.to == target => t,
        Some(t) => {
            let current = t.at(now);
            if t.running(now) && target == t.reversing_adjusted_from {
                // going back where the running one began: as long as the way come
                let shortening = (t.eased(now) * t.shortening + (1.0 - t.shortening)).abs().clamp(0.0, 1.0);
                Transition { from: current, to: target, reversing_adjusted_from: t.to, shortening, start: now, seconds: seconds * shortening }
            } else {
                Transition { from: current, to: target, reversing_adjusted_from: current, shortening: 1.0, start: now, seconds }
            }
        }
    };
    if previous != Some(next) {
        ctx.data_mut(|d| d.insert_temp(id, next));
    }
    if next.running(now) {
        ctx.request_repaint();
    }
    next.at(now)
}

fn now(ctx: &Context) -> f64 {
    ctx.input(|i| i.time)
}

/// A two-state transition of `id` (hover, pressed, open): 0 when `on` has been false, 1 when true,
/// eased over `seconds` with `--ease-out` in between. Keeps egui repainting while it moves.
pub fn toggle(ctx: &Context, id: Id, on: bool, seconds: f32) -> f32 {
    transition(ctx, id, if on { 1.0 } else { 0.0 }, seconds, now(ctx))
}

/// A numeric transition of `id` towards `target` over `seconds` with `--ease-out` (CSS's
/// `transition: <property> <seconds> var(--ease-out)`). The first call stands at `target`.
pub fn value(ctx: &Context, id: Id, target: f32, seconds: f32) -> f32 {
    transition(ctx, id, target, seconds, now(ctx))
}

/// A keyframe animation that plays once from 0 to 1 over `seconds` with `--ease-out`, from the
/// first pass `id` is drawn in, and again when it is drawn after a pass without it (`animation:
/// enter 200ms var(--ease-out)` on an element that appears: the detail panels, popovers and
/// dialogs). Keeps egui repainting while it plays.
pub fn enter(ctx: &Context, id: Id, seconds: f32) -> f32 {
    let seconds = duration(ctx, seconds);
    let id = id.with("rustradar::anim::enter");
    let (time, pass) = (now(ctx), ctx.cumulative_pass_nr());
    let start = match ctx.data(|d| d.get_temp::<(f64, u64)>(id)) {
        Some((start, last)) if last + 1 >= pass => start,
        _ => time,
    };
    ctx.data_mut(|d| d.insert_temp(id, (start, pass)));
    let t = if seconds <= 0.0 { 1.0 } else { (((time - start) / f64::from(seconds)) as f32).clamp(0.0, 1.0) };
    if t < 1.0 {
        ctx.request_repaint();
    }
    ease_out(t)
}

/// Where an infinite animation of `period` seconds stands now, 0–1 (linear time; ease it as its
/// keyframes do), with egui asked to repaint at once; `None` under reduced motion, where it
/// stands still at its start.
pub fn cycle(ctx: &Context, period: f32) -> Option<f32> {
    if reduced_motion(ctx) || period <= 0.0 {
        return None;
    }
    ctx.request_repaint();
    Some((now(ctx) / f64::from(period)).rem_euclid(1.0) as f32)
}

/// The skeleton's shimmer (app.css:222-233) at `phase` (0–1 of [`SKELETON_PERIOD`]), at `u` (0 at
/// the element's left edge, 1 at its right): how much of `--surface-hover` is mixed into
/// `--surface-raised` there, 0–1.
///
/// The gradient (`--surface-raised` → `--surface-hover` at 50 % → `--surface-raised`) is two
/// widths wide (`background-size: 200% 100%`) and repeats; `background-position` runs from 0 to
/// −200 %, which moves it right by `(100 % − 200 %) × −200 %` = two widths: the highlight sweeps
/// from left to right, across the element in the second half of each cycle (doc 06 §1.4 says
/// right to left; the CSS moves it rightwards).
pub fn skeleton_mix(phase: f32, u: f32) -> f32 {
    let along = (u - 2.0 * phase).rem_euclid(2.0) / 2.0;
    1.0 - (2.0 * along - 1.0).abs()
}

/// The spinner's turn at `phase` (0–1 of [`SPINNER_PERIOD`]), radians.
pub fn spinner_angle(phase: f32) -> f32 {
    std::f32::consts::TAU * phase
}

/// The live dot's ring at `phase` (0–1 of [`LIVE_DOT_PERIOD`]): its spread in px and its alpha
/// as a fraction of 0.6 (`box-shadow: 0 0 0 0 … / 0.6` → `0 0 0 6px … / 0`, CSS's `ease-out`;
/// colour [`super::theme::LIVE_PULSE`]).
pub fn live_dot_ring(phase: f32) -> (f32, f32) {
    let e = CSS_EASE_OUT.at(f64::from(phase)) as f32;
    (LIVE_DOT_RING * e, 1.0 - e)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `curve` at `x`, worked out by bisection on the curve to 1e-12.
    fn reference(curve: CubicBezier, x: f64) -> f64 {
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if sample(mid, curve.x1, curve.x2) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        sample(0.5 * (lo + hi), curve.y1, curve.y2)
    }

    #[test]
    fn ease_out_is_the_css_curve() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(-0.5), 0.0);
        assert_eq!(ease_out(1.5), 1.0);
        for i in 1..100 {
            let x = f64::from(i) / 100.0;
            for curve in [EASE_OUT, CSS_EASE_OUT] {
                assert!((curve.at(x) - reference(curve, x)).abs() < 1e-6, "{curve:?} at {x}: {} {}", curve.at(x), reference(curve, x));
            }
        }
        // fast out of the start, slow into the end, never past it
        assert!((ease_out(0.1) - 0.4944).abs() < 1e-3, "{}", ease_out(0.1));
        assert!((ease_out(0.5) - 0.9718).abs() < 1e-3, "{}", ease_out(0.5));
        let mut last = 0.0;
        for i in 0..=1000 {
            let y = ease_out(i as f32 / 1000.0);
            assert!(y >= last && y <= 1.0, "monotone at {i}");
            last = y;
        }
        // y1 = y2 = 1: the curve's y is 1 − (1 − s)³ of its parameter s
        let s: f64 = 0.3;
        let x = sample(s, 0.16, 0.3);
        assert!((EASE_OUT.at(x) - (1.0 - (1.0 - s).powi(3))).abs() < 1e-6);
        // linear
        let linear = CubicBezier::new(0.0, 0.0, 1.0, 1.0);
        assert!((linear.at(0.37) - 0.37).abs() < 1e-6);
    }

    fn frame(ctx: &Context, time: f64, mut f: impl FnMut(&Context)) {
        let input = egui::RawInput { time: Some(time), ..Default::default() };
        ctx.run_ui(input, |ui| f(ui.ctx())).textures_delta.clear();
    }

    #[test]
    fn a_transition_eases_from_where_it_stands() {
        let ctx = Context::default();
        let id = Id::unique("hover");
        let mut v = -1.0;
        frame(&ctx, 0.0, |c| v = toggle(c, id, false, HOVER));
        assert_eq!(v, 0.0, "the first call stands at its state");
        frame(&ctx, 1.0, |c| v = toggle(c, id, true, HOVER));
        assert_eq!(v, 0.0, "starts at the change");
        frame(&ctx, 1.075, |c| v = toggle(c, id, true, HOVER));
        assert!((v - ease_out(0.5)).abs() < 1e-4, "half the time: {v}");
        frame(&ctx, 1.15, |c| v = toggle(c, id, true, HOVER));
        assert_eq!(v, 1.0);
        frame(&ctx, 5.0, |c| v = toggle(c, id, true, HOVER));
        assert_eq!(v, 1.0);

        // a numeric transition, retargeted mid-way: starts from where it is
        let id = Id::unique("height");
        frame(&ctx, 10.0, |c| v = value(c, id, 100.0, SHEET));
        assert_eq!(v, 100.0);
        frame(&ctx, 10.0, |c| v = value(c, id, 300.0, SHEET));
        assert_eq!(v, 100.0);
        frame(&ctx, 10.11, |c| v = value(c, id, 300.0, SHEET));
        let mid = 100.0 + 200.0 * ease_out(0.5);
        assert!((v - mid).abs() < 0.01, "{v} {mid}");
        frame(&ctx, 10.11, |c| v = value(c, id, 500.0, SHEET));
        assert!((v - mid).abs() < 0.01, "retargeted from where it stood: {v}");
        frame(&ctx, 10.33, |c| v = value(c, id, 500.0, SHEET));
        assert_eq!(v, 500.0, "a full duration from the retarget");
    }

    #[test]
    fn going_back_takes_as_long_as_the_way_come() {
        let ctx = Context::default();
        let id = Id::unique("row");
        let mut v = 0.0;
        frame(&ctx, 0.0, |c| v = toggle(c, id, false, HOVER));
        frame(&ctx, 1.0, |c| v = toggle(c, id, true, HOVER));
        // 15 ms in, the pointer leaves
        frame(&ctx, 1.015, |c| v = toggle(c, id, false, HOVER));
        let reached = ease_out(0.1);
        assert!((v - reached).abs() < 1e-4, "{v}");
        // the way back is shortened to `reached` of 150 ms
        let back = f64::from(HOVER * reached);
        frame(&ctx, 1.015 + back * 0.5, |c| v = toggle(c, id, false, HOVER));
        assert!((v - reached * (1.0 - ease_out(0.5))).abs() < 1e-4, "half way back in time: {v}");
        frame(&ctx, 1.015 + back + 1e-6, |c| v = toggle(c, id, false, HOVER));
        assert_eq!(v, 0.0, "back after {back} s");
    }

    #[test]
    fn an_entering_element_plays_once_from_the_pass_it_appears_in() {
        let ctx = Context::default();
        let id = Id::unique("panel");
        let mut v = -1.0;
        frame(&ctx, 2.0, |c| v = enter(c, id, PANEL_ENTER));
        assert_eq!(v, 0.0, "starts invisible");
        frame(&ctx, 2.1, |c| v = enter(c, id, PANEL_ENTER));
        assert!((v - ease_out(0.5)).abs() < 1e-4, "{v}");
        frame(&ctx, 2.2, |c| v = enter(c, id, PANEL_ENTER));
        assert_eq!(v, 1.0);
        frame(&ctx, 9.0, |c| v = enter(c, id, PANEL_ENTER));
        assert_eq!(v, 1.0, "stays entered while drawn");
        // closed for a pass, opened again: plays again
        frame(&ctx, 9.5, |_| {});
        frame(&ctx, 10.0, |c| v = enter(c, id, PANEL_ENTER));
        assert_eq!(v, 0.0);
        frame(&ctx, 10.2, |c| v = enter(c, id, PANEL_ENTER));
        assert_eq!(v, 1.0);
    }

    #[test]
    fn reduced_motion_jumps_and_stands_still() {
        let ctx = Context::default();
        assert!(!reduced_motion(&ctx));
        let mut phase = None;
        frame(&ctx, 0.7, |c| phase = cycle(c, SKELETON_PERIOD));
        assert!((phase.expect("running") - 0.5).abs() < 1e-6);
        set_reduced_motion(&ctx, true);
        assert!(reduced_motion(&ctx));
        assert_eq!(duration(&ctx, PANEL_ENTER), REDUCED);
        frame(&ctx, 0.7, |c| phase = cycle(c, SPINNER_PERIOD));
        assert_eq!(phase, None);
        let id = Id::unique("panel");
        let mut v = 0.0;
        frame(&ctx, 1.0, |c| v = toggle(c, id, false, PANEL_ENTER));
        frame(&ctx, 1.0, |c| v = toggle(c, id, true, PANEL_ENTER));
        frame(&ctx, 1.001, |c| v = toggle(c, id, true, PANEL_ENTER));
        assert_eq!(v, 1.0, "at its end a millisecond later");
        set_reduced_motion(&ctx, false);
        assert_eq!(duration(&ctx, PANEL_ENTER), PANEL_ENTER);
    }

    #[test]
    fn the_infinite_animations() {
        // at rest the gradient's middle (the highlight) is at the right edge, its ends at the left
        assert_eq!(skeleton_mix(0.0, 1.0), 1.0);
        assert_eq!(skeleton_mix(0.0, 0.0), 0.0);
        assert_eq!(skeleton_mix(0.0, 0.5), 0.5);
        // half a cycle on, the highlight is at the left edge; three quarters on, in the middle
        assert!((skeleton_mix(0.5, 0.0) - 1.0).abs() < 1e-6);
        assert!((skeleton_mix(0.75, 0.5) - 1.0).abs() < 1e-6);
        assert!(skeleton_mix(0.75, 0.0) < 0.6 && skeleton_mix(0.75, 1.0) < 0.6);
        // it moves right: the peak's place grows with the phase from 0.5 to 1
        let peak = |phase: f32| (0..=100).map(|i| i as f32 / 100.0).max_by(|a, b| skeleton_mix(phase, *a).total_cmp(&skeleton_mix(phase, *b))).unwrap();
        assert!(peak(0.6) < peak(0.7) && peak(0.7) < peak(0.8) && peak(0.8) < peak(0.9), "{} {} {} {}", peak(0.6), peak(0.7), peak(0.8), peak(0.9));
        assert!((spinner_angle(0.25) - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(live_dot_ring(0.0), (0.0, 1.0));
        assert_eq!(live_dot_ring(1.0), (LIVE_DOT_RING, 0.0));
        // CSS's ease-out, not --ease-out: at half time 68 % of the way (--ease-out: 95 %)
        let (spread, alpha) = live_dot_ring(0.5);
        assert!((spread - 6.0 * CSS_EASE_OUT.at(0.5) as f32).abs() < 1e-5 && (alpha - (1.0 - spread / 6.0)).abs() < 1e-5, "{spread} {alpha}");
        assert!((CSS_EASE_OUT.at(0.5) - 0.6846).abs() < 1e-3, "{}", CSS_EASE_OUT.at(0.5));
    }
}
