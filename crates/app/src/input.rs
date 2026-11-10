//! Input between egui and the map (plan 1.5, 2.2; doc 01 §3.1–3.8; doc 06 §3.13): every window
//! event goes to egui first; the map takes a gesture only when egui does not want the pointer.
//!
//! [`map_event`] turns winit's events into [`MapEvent`]s, which carry nothing of winit (CSS pixels
//! from the canvas's top left, wheel deltas with the DOM's sign), so tests drive the map without a
//! window. [`MapInput`] turns them into the controller's gestures as deck.gl 9.4 does in today's app
//! (doc 01 §3.2):
//!
//! | Input | Result |
//! |---|---|
//! | left or middle drag | pan (north up) |
//! | right drag, or a drag with Ctrl, Shift, Alt or Meta | turn and tilt |
//! | wheel | zoom about the pointer; with Shift a quarter as fast |
//! | double click or double tap | one level in about the pointer over 300 ms; with a modifier, out |
//! | trackpad two-finger scroll (pixel deltas) | pan; with Ctrl, zoom |
//! | trackpad pinch and twist (macOS) | zoom about the pointer; the map turns with the fingers |
//! | one finger | pan |
//! | two fingers | pinch: zoom about them, the place first pinched following them; no twist, no tilt |
//! | two fingers dragged together at once | pan |
//! | arrows | pan 100 px; with a modifier, turn 15° (left, right) or tilt 10° (up, down) |
//! | `+` `=` / `-` | `zoom_by(±1)` (one binding: plan section 6, item 10) |
//! | `n` / `t` | north up and flat / tilt to 55°, or back to 0 from above 5° |
//!
//! Locked on a vehicle, the controller makes every drag orbit, zooms about the centre and drops the
//! trackpad's pan ([`Controller::set_locked`], which the state sets each frame as `renderFrame`
//! sets `controllerFor(locked)`); here the two-finger pan's recognizer is off while locked, as
//! deck turns its events off with `multiTouchDrag: null`, so two fingers only pinch.
//!
//! # Gestures as deck hears them
//!
//! deck reads pointers through mjolnir 3.1's port of hammer.js, and its timing shapes what a drag
//! does, so it is ported here (`Hammer`): one session from the first pointer down to the last up
//! (the mouse with any button, or fingers), hammer's input data (centre rounded to whole pixels,
//! distance from the first input, deltas carried over when fingers come and go, scale against the
//! first two fingers, the direction over the last 25 ms), and deck's recognizers in deck's order
//! with their links (`RECOGNIZERS` in `@deck.gl/core/dist/lib/constants.js`):
//!
//! - `multipan` (two pointers, 10 px) and `pinch` (two pointers, any change of scale) need each
//!   other to fail, so whichever begins first owns the session;
//! - `pan` (one pointer, past 1 px and moving in some direction: in the first 25 ms only sideways
//!   movement counts) runs beside `pinch`; a drag starts where it is recognised, so the first pixel
//!   or so of travel is not panned, as in deck;
//! - `dblclick` (two taps within 300 ms and 10 px, each shorter than 250 ms and moving less than
//!   9 px) and `click` need each other to fail, so each fires only 300 ms after its last release,
//!   once no further press has come: a double click zooms 300 ms after the second release.
//!
//! The recognizers' events then call the controller's gesture methods exactly as deck's
//! `Controller.handleEvent` does (`pancancel`, `pinchcancel` and `multipancancel` are not deck's
//! events and change nothing).
//!
//! # What the app wires (`crates/app/src/app.rs`, not this item's file)
//!
//! - every window event: [`map_event`], then [`MapInput::handle`] (wired);
//! - each frame, before the camera step: [`MapInput::update`], which fires the click and double
//!   click when their 300 ms are up, and [`MapInput::next_due_ms`] in the schedule (until then
//!   they fire with the next window event);
//! - after the egui pass: [`MapInput::set_key_gate`] with egui's focus and the open dialogs, and
//!   [`MapInput::set_cursor`] on the pass's platform output with the hover state (item A9);
//! - the clicks for picking: [`MapInput::take_clicks`] (item A9).
//!
//! # Deviations
//!
//! - A wheel notch (winit's `LineDelta` of one line) counts as the 100 px with `wheelDeltaY` 120
//!   that WebView2 reports for a notch (100/3 px a line, Windows' default 3 lines a notch), so a
//!   notch zooms 0.548 levels as on Windows today; WebKit's mouse notches are smaller (about 4 px a
//!   slow notch on macOS, with acceleration). A pixel delta (a trackpad, a Magic Mouse) carries
//!   WebKit's `wheelDeltaY = -3·deltaY`, so it pans as today; the plan's "PixelDelta pans,
//!   LineDelta zooms" (doc 01 §3.7) falls out of `trackpad.ts` unchanged (a sideways line delta
//!   still pans, as `deltaX != 0` does today). Wheel events with no movement are dropped.
//! - The trackpad pinch's scale is `1 + Σ magnification` (the documented meaning of AppKit's
//!   magnification, which WebKit's `GestureEvent.scale` reports since `gesturestart`), its rotation
//!   the negated sum of winit's counterclockwise degrees (WebKit's is clockwise).
//! - A drag that ends with a cancelled touch, or with no recognizer's end (deck leaves its drag flag
//!   set then), ends the controller's drag when the session ends, so the loop stops drawing
//!   continuously; losing the window's focus cancels the session (its releases go elsewhere).
//! - A press of the mouse while a finger is down is ignored: Windows also sends a touch as mouse
//!   messages, which would make one finger look like two pointers.

use rustradar_core::num::js_round;
use rustradar_scene::controller::{ArrowKey, Controller, DeltaMode, TILT, WheelInput};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::keyboard::{Key, KeyCode, PhysicalKey};

/// A wheel notch, as WebView2 reports one: 100 CSS pixels (Chromium's 100/3 px a line, three lines
/// a notch on Windows' default setting)...
const LINE_PX: f64 = 100.0;
/// ...with the legacy `wheelDeltaY` of a notch, 120 (positive scrolling up).
const LEGACY_PER_LINE: f64 = 120.0;
/// WebKit's legacy `wheelDeltaY` of a trackpad's scroll: −3 × its pixel delta (`trackpad.ts`).
const WEBKIT_LEGACY_PER_PX: f64 = -3.0;
/// The tilt key goes back to 0 from above this pitch (`+page.svelte:1210`, `view.pitch > 5`).
const TILTED_ABOVE: f64 = 5.0;
/// The most clicks kept for the picking code to take ([`MapInput::take_clicks`]).
const MAX_CLICKS: usize = 8;

/// A mouse button, as the map tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    /// Its bit in the DOM's `MouseEvent.buttons`.
    fn mask(self) -> u8 {
        match self {
            Button::Left => 1,
            Button::Right => 2,
            Button::Middle => 4,
        }
    }
}

/// The modifier keys held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Cmd on macOS, the Windows key elsewhere.
    pub meta: bool,
}

impl Modifiers {
    /// mjolnir's `isFunctionKeyPressed`: any of the four held. A drag with one rotates.
    pub fn func_key(&self) -> bool {
        self.shift || self.ctrl || self.alt || self.meta
    }
}

/// Where a touch or a trackpad gesture is (winit's `TouchPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

impl From<TouchPhase> for Phase {
    fn from(phase: TouchPhase) -> Self {
        match phase {
            TouchPhase::Started => Phase::Started,
            TouchPhase::Moved => Phase::Moved,
            TouchPhase::Ended => Phase::Ended,
            TouchPhase::Cancelled => Phase::Cancelled,
        }
    }
}

/// A key, as the map's keys read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyName {
    /// A key that types this one character (the DOM's `key`): `n`, `N`, `+`, `=`, ...
    Char(char),
    /// An arrow key, by its physical position (deck reads the DOM's `code`).
    Arrow(ArrowKey),
    /// Anything else.
    Other,
}

/// The key of a winit key event: the arrows by their physical key, the rest by the character they
/// type.
pub fn key_name(logical: &Key, physical: PhysicalKey) -> KeyName {
    match physical {
        PhysicalKey::Code(KeyCode::ArrowLeft) => return KeyName::Arrow(ArrowKey::Left),
        PhysicalKey::Code(KeyCode::ArrowRight) => return KeyName::Arrow(ArrowKey::Right),
        PhysicalKey::Code(KeyCode::ArrowUp) => return KeyName::Arrow(ArrowKey::Up),
        PhysicalKey::Code(KeyCode::ArrowDown) => return KeyName::Arrow(ArrowKey::Down),
        _ => {}
    }
    if let Key::Character(text) = logical {
        let mut chars = text.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return KeyName::Char(c);
        }
    }
    KeyName::Other
}

/// An input event in the map's terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapEvent {
    /// The mouse is at this point (CSS pixels from the canvas's top left).
    PointerMoved([f64; 2]),
    /// The mouse left the window.
    PointerLeft,
    Button { button: Button, pressed: bool },
    /// A wheel or trackpad scroll as a DOM `WheelEvent`: deltas positive to the right and down,
    /// and the legacy `wheelDeltaY` where the platform has one.
    Wheel { delta_x: f64, delta_y: f64, mode: DeltaMode, wheel_delta_y: Option<f64> },
    Modifiers(Modifiers),
    /// The window lost the keyboard focus: a gesture in progress ends (its release goes elsewhere).
    FocusLost,
    /// A finger at `at` (CSS pixels).
    Touch { id: u64, phase: Phase, at: [f64; 2] },
    /// A trackpad pinch (macOS): this change of magnification, positive apart.
    Pinch { delta: f64, phase: Phase },
    /// A trackpad twist (macOS): this many degrees, counterclockwise positive.
    Rotate { delta: f64, phase: Phase },
    /// A key went down (repeats included, as the DOM's `keydown`) or up.
    Key { key: KeyName, pressed: bool },
}

/// What may take the keys (doc 06 §3.13), as of the last egui pass: set by the app after each pass
/// ([`MapInput::set_key_gate`]). By default every key reaches the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyGate {
    /// A text field has the keyboard (egui's `text_edit_focused`): no shortcut is taken ("typing").
    pub typing: bool,
    /// A widget has the keyboard focus (egui's `egui_wants_keyboard_input`): the arrows are its,
    /// as deck hears keys only while its canvas has the focus.
    pub ui_focused: bool,
    /// The keys dialog (modal) or the shortcuts help is open: the map's shortcuts are off.
    pub shortcuts_off: bool,
}

/// A click or tap on the map, for picking (deck's `onClick`, item A9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    /// Where, in CSS pixels, rounded as hammer rounds.
    pub at: [f64; 2],
    /// hammer's `tapCount`: 1 for a click, 2 for a double click (deck reports both to `onClick`;
    /// a triple click comes as a click with a count of 2 and does not zoom).
    pub tap_count: u32,
    /// The modifiers at the release.
    pub modifiers: Modifiers,
}

/// The macOS trackpad's pinch-and-twist in progress (WebKit's `gesturestart` to `gestureend`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct TrackpadGesture {
    /// The map took it (else it began over egui).
    map: bool,
    pinching: bool,
    rotating: bool,
    /// Σ of the pinch's magnification since it began.
    magnification: f64,
    /// Σ of the twist's degrees since it began, counterclockwise.
    rotation: f64,
}

/// The gesture the controller took last, to end it if no recognizer does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Active {
    Pan,
    Pinch,
    MultiPan,
}

/// The map's input state: the mouse's position and buttons, the modifiers, the gesture recognizers,
/// a trackpad gesture, the clicks for picking and the key gate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapInput {
    pointer: Option<[f64; 2]>,
    modifiers: Modifiers,
    /// The DOM's `buttons`: the mouse buttons held, wherever they were pressed.
    buttons: u8,
    hammer: Hammer,
    active: Option<Active>,
    gesture: Option<TrackpadGesture>,
    clicks: Vec<Click>,
    gate: KeyGate,
}

impl MapInput {
    /// Where the mouse is, if it is over the window.
    pub fn pointer(&self) -> Option<[f64; 2]> {
        self.pointer
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Whether a mouse button or a finger the map took is down (its gesture session runs).
    pub fn dragging(&self) -> bool {
        !self.hammer.store.is_empty()
    }

    /// The key gate after the egui pass (see [`KeyGate`]).
    pub fn set_key_gate(&mut self, gate: KeyGate) {
        self.gate = gate;
    }

    /// The clicks and taps recognised since the last call, oldest first (at most a few are kept).
    pub fn take_clicks(&mut self) -> Vec<Click> {
        std::mem::take(&mut self.clicks)
    }

    /// When a pending click or double click is due (local Unix ms), else infinity.
    pub fn next_due_ms(&self) -> f64 {
        self.hammer.next_due().unwrap_or(f64::INFINITY)
    }

    /// Fires the click and double click whose 300 ms are up (once a frame, before the camera
    /// step). Returns whether the camera may have moved.
    pub fn update(&mut self, controller: &mut Controller, now_ms: f64) -> bool {
        let fired = self.hammer.advance(now_ms);
        let mut moved = false;
        for (_, gesture) in fired {
            moved |= self.apply(gesture, controller, now_ms);
        }
        moved
    }

    /// deck's cursor (G:3714, `getCursor`): grabbing while the controller drags, a pointing hand
    /// over something pickable (`hovering`, item A9), else the open hand.
    pub fn cursor(&self, controller: &Controller, hovering: bool) -> egui::CursorIcon {
        if controller.is_dragging() {
            egui::CursorIcon::Grabbing
        } else if hovering {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::Grab
        }
    }

    /// Puts [`Self::cursor`] into an egui pass's `output` while the mouse is over the map, not
    /// over egui (call after the pass; egui-winit then shows it).
    pub fn set_cursor(&self, egui: &egui::Context, output: &mut egui::PlatformOutput, controller: &Controller, hovering: bool) {
        if self.pointer.is_some() && !egui.is_pointer_over_egui() {
            output.cursor_icon = self.cursor(controller, hovering);
        }
    }

    /// Feeds one event at `now_ms` (local clock). `egui_wants_pointer` is egui's
    /// `egui_wants_pointer_input()` from its last pass: then a press, a touch, a wheel or a trackpad
    /// gesture is egui's, not the map's; a gesture the map started stays the map's to its end.
    /// Returns whether the camera may have moved (draw a frame).
    pub fn handle(&mut self, event: MapEvent, egui_wants_pointer: bool, controller: &mut Controller, now_ms: f64) -> bool {
        // the timers that came due before this event fire first, as they would in the browser
        let moved = self.update(controller, now_ms);
        let took = match event {
            MapEvent::PointerMoved(at) => {
                self.pointer = Some(at);
                self.mouse(InputKind::Move, None, controller, now_ms)
            }
            // a drag goes on outside the window (the platform keeps sending its moves)
            MapEvent::PointerLeft => {
                self.pointer = None;
                false
            }
            MapEvent::Button { button, pressed: true } => {
                let before = self.buttons;
                self.buttons |= button.mask();
                if before != 0 {
                    // another button while one is held: the DOM's `pointermove`
                    self.mouse(InputKind::Move, None, controller, now_ms)
                } else if egui_wants_pointer || self.pointer.is_none() || self.hammer.has_touch() {
                    // `pointerdown` lands on the canvas only: not over egui, and not as Windows'
                    // copy of a touch
                    false
                } else {
                    self.mouse(InputKind::Start, Some(button), controller, now_ms)
                }
            }
            MapEvent::Button { button, pressed: false } => {
                self.buttons &= !button.mask();
                let kind = if self.buttons == 0 { InputKind::End } else { InputKind::Move };
                self.mouse(kind, Some(button), controller, now_ms)
            }
            MapEvent::Wheel { delta_x, delta_y, mode, wheel_delta_y } => match self.pointer {
                Some(at) if !egui_wants_pointer && (delta_x != 0.0 || delta_y != 0.0) => {
                    let wheel = WheelInput {
                        delta_x,
                        delta_y,
                        delta_mode: mode,
                        ctrl_key: self.modifiers.ctrl,
                        shift_key: self.modifiers.shift,
                        wheel_delta_y,
                    };
                    controller.wheel(&wheel, at, now_ms)
                }
                _ => false,
            },
            MapEvent::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                false
            }
            MapEvent::FocusLost => {
                self.buttons = 0;
                let mut moved = false;
                for (id, at) in self.hammer.store.clone() {
                    moved |= self.pointer_input(InputKind::Cancel, id, at, false, controller, now_ms);
                }
                if let Some(g) = self.gesture.take()
                    && g.map
                {
                    controller.gesture_end();
                }
                moved
            }
            MapEvent::Touch { id, phase, at } => {
                let kind = match phase {
                    Phase::Started => InputKind::Start,
                    Phase::Moved => InputKind::Move,
                    Phase::Ended => InputKind::End,
                    Phase::Cancelled => InputKind::Cancel,
                };
                // a finger that comes down over egui is egui's
                if kind == InputKind::Start && egui_wants_pointer {
                    false
                } else {
                    self.pointer_input(kind, PointerId::Touch(id), at, false, controller, now_ms)
                }
            }
            MapEvent::Pinch { delta, phase } => self.trackpad(true, delta, phase, egui_wants_pointer, controller),
            MapEvent::Rotate { delta, phase } => self.trackpad(false, delta, phase, egui_wants_pointer, controller),
            MapEvent::Key { key, pressed: true } => self.key(key, controller, now_ms),
            MapEvent::Key { pressed: false, .. } => false,
        };
        moved | took
    }

    /// The mouse as a pointer: `changed` is the button pressed or released (the DOM's `button`),
    /// `None` for a move (its `buttons` say which are held).
    fn mouse(&mut self, kind: InputKind, changed: Option<Button>, controller: &mut Controller, now_ms: f64) -> bool {
        // out of the window mid-drag, the release is where the mouse was last seen
        let Some(at) = self.pointer.or_else(|| self.hammer.position(PointerId::Mouse)) else { return false };
        // mjolnir's `whichButtons`: the held buttons on a move, else the one that changed
        let right_button = match changed {
            Some(button) if kind != InputKind::Move => button == Button::Right,
            _ => self.buttons & Button::Right.mask() != 0,
        };
        self.pointer_input(kind, PointerId::Mouse, at, right_button, controller, now_ms)
    }

    /// One pointer event through the recognizers, their gestures to the controller.
    fn pointer_input(
        &mut self,
        kind: InputKind,
        id: PointerId,
        at: [f64; 2],
        right_button: bool,
        controller: &mut Controller,
        now_ms: f64,
    ) -> bool {
        let cx = InputCx { now_ms, modifiers: self.modifiers, right_button, multipan: !controller.locked() };
        let Some((gestures, is_final)) = self.hammer.input(kind, id, at, &cx) else { return false };
        let mut moved = false;
        for gesture in gestures {
            moved |= self.apply(gesture, controller, now_ms);
        }
        if is_final {
            // deck leaves its drag set when the session ends on a cancel: end it (module docs)
            if controller.is_dragging() {
                moved |= match self.active {
                    Some(Active::MultiPan) => controller.multi_pan_end() || controller.pan_end(),
                    Some(Active::Pinch) => controller.pinch_end(),
                    _ => controller.pan_end(),
                };
            }
            self.active = None;
        }
        moved
    }

    /// A recognised gesture as deck's `Controller.handleEvent` takes it.
    fn apply(&mut self, gesture: Gesture, controller: &mut Controller, now_ms: f64) -> bool {
        match gesture {
            Gesture::PanStart { at, right_button, func_key } => {
                let took = controller.pan_start(at, right_button, func_key, now_ms);
                if took {
                    self.active = Some(Active::Pan);
                }
                took
            }
            Gesture::PanMove { at } => controller.pan_move(at),
            Gesture::PanEnd => controller.pan_end(),
            Gesture::PinchStart { at } => {
                let took = controller.pinch_start(at, now_ms);
                if took {
                    self.active = Some(Active::Pinch);
                }
                took
            }
            Gesture::PinchMove { at, scale } => controller.pinch_move(at, scale),
            Gesture::PinchEnd => controller.pinch_end(),
            Gesture::MultiPanStart { at, delta } => {
                let took = controller.multi_pan_start(at, delta, now_ms);
                if took {
                    self.active = Some(Active::MultiPan);
                }
                took
            }
            Gesture::MultiPanMove { delta } => controller.multi_pan_move(delta),
            Gesture::MultiPanEnd => controller.multi_pan_end(),
            // not deck's events
            Gesture::PanCancel | Gesture::PinchCancel | Gesture::MultiPanCancel => false,
            Gesture::DoubleClick(tap) => {
                self.push_click(tap);
                controller.double_click(tap.at, tap.modifiers.func_key(), now_ms)
            }
            Gesture::Click(tap) => {
                self.push_click(tap);
                false
            }
        }
    }

    fn push_click(&mut self, tap: TapInput) {
        if self.clicks.len() == MAX_CLICKS {
            self.clicks.remove(0);
        }
        self.clicks.push(Click { at: tap.at, tap_count: tap.count, modifiers: tap.modifiers });
    }

    /// The trackpad's pinch (`pinch`) or twist as WebKit's one `gesture*` stream (G:3564-3592):
    /// it starts at the pointer with the first of the two to start and ends with the last to end.
    fn trackpad(&mut self, pinch: bool, delta: f64, phase: Phase, egui_wants_pointer: bool, controller: &mut Controller) -> bool {
        let delta = if delta.is_finite() { delta } else { 0.0 };
        if phase == Phase::Started && self.gesture.is_none() {
            let map = !egui_wants_pointer;
            if map {
                let [w, h] = controller.size();
                controller.gesture_start(self.pointer.unwrap_or([w / 2.0, h / 2.0]));
            }
            self.gesture = Some(TrackpadGesture { map, ..Default::default() });
        }
        let Some(mut g) = self.gesture else { return false };
        match phase {
            Phase::Started if pinch => g.pinching = true,
            Phase::Started => g.rotating = true,
            Phase::Moved => {}
            Phase::Ended | Phase::Cancelled => {
                if pinch {
                    g.pinching = false;
                } else {
                    g.rotating = false;
                }
                self.gesture = (g.pinching || g.rotating).then_some(g);
                if self.gesture.is_none() && g.map {
                    controller.gesture_end();
                }
                return false;
            }
        }
        if pinch {
            g.magnification += delta;
        } else {
            g.rotation += delta;
        }
        self.gesture = Some(g);
        if !g.map || delta == 0.0 {
            return false;
        }
        // WebKit's `scale` since `gesturestart`, and its clockwise `rotation`
        controller.gesture_change(1.0 + g.magnification, -g.rotation);
        true
    }

    /// A key down: the arrows as deck's keyboard handler, the camera shortcuts of the page's
    /// (`+page.svelte:1202-1265`): ignored while typing, with a dialog open, or with Ctrl, Cmd or
    /// Alt held; a letter with Shift is its own shortcut, Caps Lock alone counts as the plain one.
    fn key(&mut self, key: KeyName, controller: &mut Controller, now_ms: f64) -> bool {
        match key {
            KeyName::Arrow(arrow) => {
                if self.gate.ui_focused {
                    return false;
                }
                controller.arrow_key(arrow, self.modifiers.func_key(), now_ms)
            }
            KeyName::Char(c) => {
                let m = self.modifiers;
                if self.gate.typing || self.gate.shortcuts_off || m.ctrl || m.meta || m.alt {
                    return false;
                }
                let lower: String = c.to_lowercase().collect();
                let upper: String = c.to_uppercase().collect();
                let key = if lower != upper {
                    if m.shift { upper } else { lower }
                } else {
                    c.to_string()
                };
                match key.as_str() {
                    "t" => {
                        let pitch = if controller.view().pitch > TILTED_ABOVE { 0.0 } else { TILT };
                        controller.set_pitch(pitch, now_ms);
                    }
                    "n" => controller.reset_orientation(now_ms),
                    "+" | "=" => controller.zoom_by(1.0, now_ms),
                    "-" => controller.zoom_by(-1.0, now_ms),
                    _ => return false,
                }
                true
            }
            KeyName::Other => false,
        }
    }
}

/// The map's share of a winit event on a window of `scale_factor` device pixels per CSS pixel, if
/// it has one.
///
/// winit's scroll deltas are positive when the content should move right and down, the DOM's when
/// the view scrolls right and down: their signs are turned. A line delta (a mouse wheel's notches)
/// becomes the pixels WebView2 reports for it; a pixel delta (a trackpad) becomes CSS pixels with
/// WebKit's legacy `wheelDeltaY` (see the module's deviations).
pub fn map_event(event: &WindowEvent, scale_factor: f64) -> Option<MapEvent> {
    let css = |v: f64| v / scale_factor;
    Some(match event {
        WindowEvent::CursorMoved { position, .. } => MapEvent::PointerMoved([css(position.x), css(position.y)]),
        WindowEvent::CursorLeft { .. } => MapEvent::PointerLeft,
        WindowEvent::MouseInput { state, button, .. } => MapEvent::Button {
            button: match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
                _ => return None,
            },
            pressed: *state == ElementState::Pressed,
        },
        WindowEvent::MouseWheel { delta, .. } => match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                let (x, y) = (f64::from(*x), f64::from(*y));
                MapEvent::Wheel {
                    delta_x: -x * LINE_PX,
                    delta_y: -y * LINE_PX,
                    mode: DeltaMode::Pixel,
                    wheel_delta_y: Some(y * LEGACY_PER_LINE),
                }
            }
            MouseScrollDelta::PixelDelta(p) => {
                let delta_y = -css(p.y);
                MapEvent::Wheel { delta_x: -css(p.x), delta_y, mode: DeltaMode::Pixel, wheel_delta_y: Some(WEBKIT_LEGACY_PER_PX * delta_y) }
            }
        },
        WindowEvent::ModifiersChanged(m) => {
            let s = m.state();
            MapEvent::Modifiers(Modifiers { shift: s.shift_key(), ctrl: s.control_key(), alt: s.alt_key(), meta: s.super_key() })
        }
        WindowEvent::Focused(false) => MapEvent::FocusLost,
        WindowEvent::Touch(touch) => MapEvent::Touch {
            id: touch.id,
            phase: touch.phase.into(),
            at: [css(touch.location.x), css(touch.location.y)],
        },
        WindowEvent::PinchGesture { delta, phase, .. } => MapEvent::Pinch { delta: *delta, phase: (*phase).into() },
        WindowEvent::RotationGesture { delta, phase, .. } => MapEvent::Rotate { delta: f64::from(*delta), phase: (*phase).into() },
        // the presses winit makes up for keys held when the window gains the focus are not keydowns
        WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
            MapEvent::Key { key: key_name(&event.logical_key, event.physical_key), pressed: event.state == ElementState::Pressed }
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// hammer.js as mjolnir 3.1 has it, with deck 9.4's recognizers
// ---------------------------------------------------------------------------------------------

/// deck's `pan`: a one-pointer drag starts past this many pixels...
const PAN_THRESHOLD: f64 = 1.0;
/// ...`multipan`: two pointers dragged together, past this many.
const MULTIPAN_THRESHOLD: f64 = 10.0;
/// hammer's Pinch: any change of scale beyond this begins one.
const PINCH_THRESHOLD: f64 = 0.0;
/// hammer's Tap: taps follow each other within this many ms...
const TAP_INTERVAL_MS: f64 = 300.0;
/// ...each press shorter than this...
const TAP_TIME_MS: f64 = 250.0;
/// ...moving less than this many pixels...
const TAP_THRESHOLD: f64 = 9.0;
/// ...and within this many pixels of the one before.
const TAP_POS_THRESHOLD: f64 = 10.0;
/// hammer's `COMPUTE_INTERVAL`: the direction of movement is taken over at least this many ms.
const COMPUTE_INTERVAL_MS: f64 = 25.0;

/// hammer's `RecognizerState` bits.
const POSSIBLE: u8 = 1;
const BEGAN: u8 = 2;
const CHANGED: u8 = 4;
const ENDED: u8 = 8;
/// `Recognized` is `Ended`.
const RECOGNIZED: u8 = 8;
const CANCELLED: u8 = 16;
const FAILED: u8 = 32;

/// hammer's `InputDirection` bits.
const DIRECTION_NONE: u8 = 0;
const DIRECTION_LEFT: u8 = 1;
const DIRECTION_RIGHT: u8 = 2;
const DIRECTION_UP: u8 = 4;
const DIRECTION_DOWN: u8 = 8;
const DIRECTION_ALL: u8 = 15;

/// hammer's `InputEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Start,
    Move,
    End,
    Cancel,
}

/// A pointer in hammer's store: the mouse, or a finger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerId {
    Mouse,
    Touch(u64),
}

/// What an input carries besides its pointers: the clock, the source event's modifiers and right
/// button, and whether deck has the two-finger pan on (`multiTouchDrag`, off while locked).
#[derive(Debug, Clone, Copy, PartialEq)]
struct InputCx {
    now_ms: f64,
    modifiers: Modifiers,
    right_button: bool,
    multipan: bool,
}

/// hammer's input data (`computeInputData`), the fields deck's recognizers read.
#[derive(Debug, Clone, PartialEq)]
struct Input {
    kind: InputKind,
    pointers: Vec<[f64; 2]>,
    /// The pointers' centre, rounded to whole pixels (`getCenter`).
    center: [f64; 2],
    time: f64,
    /// Since the session's first input.
    delta_time: f64,
    /// From the first input's centre (or the first two fingers' centre).
    distance: f64,
    /// Movement of the centre over the session, carried over when fingers come and go.
    delta: [f64; 2],
    /// The first two fingers' distance now over their distance when they came down.
    scale: f64,
    /// The direction over the last 25 ms or more (`computeIntervalInputData`).
    direction: u8,
    modifiers: Modifiers,
    right_button: bool,
}

/// `simpleCloneInputData`: the first input and the first with two pointers, pointers rounded.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    pointers: Vec<[f64; 2]>,
    center: [f64; 2],
    time: f64,
}

impl Snapshot {
    fn of(pointers: &[[f64; 2]], time: f64) -> Self {
        let pointers: Vec<[f64; 2]> = pointers.iter().map(|p| [js_round(p[0]), js_round(p[1])]).collect();
        Snapshot { center: center_of(&pointers), pointers, time }
    }
}

/// hammer's `manager.session`: what one gesture session remembers.
#[derive(Debug, Clone, Default, PartialEq)]
struct Session {
    first: Option<Snapshot>,
    first_multiple: Option<Snapshot>,
    offset_delta: [f64; 2],
    prev_delta: [f64; 2],
    /// The previous input's kind and delta.
    prev: Option<(InputKind, [f64; 2])>,
    /// The last interval sample: time, delta, direction.
    last_interval: Option<(f64, [f64; 2], u8)>,
    /// `curRecognizer`.
    current: Option<Rec>,
}

/// deck's recognizers, in its order (`dblclickdrag`, off in the app, is left out: it never begins
/// and never stands in another's way).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rec {
    MultiPan,
    Pinch,
    Pan,
    DoubleClick,
    Click,
}

const ORDER: [Rec; 5] = [Rec::MultiPan, Rec::Pinch, Rec::Pan, Rec::DoubleClick, Rec::Click];

impl Rec {
    /// `requireFailure`, which hammer links both ways: these must have failed (or be possible)
    /// before this one may emit.
    fn require_fail(self) -> &'static [Rec] {
        match self {
            Rec::MultiPan => &[Rec::Pinch, Rec::Pan],
            Rec::Pinch | Rec::Pan => &[Rec::MultiPan],
            Rec::DoubleClick => &[Rec::Click],
            Rec::Click => &[Rec::DoubleClick],
        }
    }

    /// `recognizeWith`: these run beside each other.
    fn simultaneous(self, other: Rec) -> bool {
        matches!((self, other), (Rec::Pan, Rec::Pinch) | (Rec::Pinch, Rec::Pan))
    }
}

/// hammer's Pan recognizer.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PanRec {
    pointers: usize,
    threshold: f64,
    state: u8,
    /// The delta at its last emit (`pX`).
    px: Option<f64>,
}

impl PanRec {
    fn new(pointers: usize, threshold: f64) -> Self {
        PanRec { pointers, threshold, state: POSSIBLE, px: None }
    }

    /// `attrTest`: the right number of pointers, and begun or moved far enough in some direction.
    fn valid(&self, input: &Input) -> bool {
        input.pointers.len() == self.pointers && (self.state & BEGAN != 0 || self.direction_test(input))
    }

    /// `directionTest` with `InputDirection.All`: no direction over the last interval falls back to
    /// the sideways delta alone.
    fn direction_test(&self, input: &Input) -> bool {
        let mut has_moved = true;
        let mut distance = input.distance;
        let mut direction = input.direction;
        if direction & DIRECTION_ALL == 0 {
            let x = input.delta[0];
            direction = if x == 0.0 {
                DIRECTION_NONE
            } else if x < 0.0 {
                DIRECTION_LEFT
            } else {
                DIRECTION_RIGHT
            };
            has_moved = Some(x) != self.px;
            distance = x.abs();
        }
        has_moved && distance > self.threshold && direction & DIRECTION_ALL != 0
    }
}

/// hammer's Tap recognizer.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TapRec {
    taps: u32,
    state: u8,
    count: u32,
    p_time: Option<f64>,
    p_center: Option<[f64; 2]>,
    /// When its delayed emit is due.
    timer: Option<f64>,
    /// The release it would report.
    input: Option<TapInput>,
}

impl TapRec {
    fn new(taps: u32) -> Self {
        TapRec { taps, state: POSSIBLE, count: 0, p_time: None, p_center: None, timer: None, input: None }
    }

    /// `TapRecognizer.process`; `delayed` is `hasRequireFailures()`.
    fn process(&mut self, input: &Input, delayed: bool) -> u8 {
        let valid_pointers = input.pointers.len() == 1;
        let valid_movement = input.distance < TAP_THRESHOLD;
        let valid_touch_time = input.delta_time < TAP_TIME_MS;
        self.timer = None;
        if input.kind == InputKind::Start && self.count == 0 {
            // `failTimeout`: its timer only sets the state it already has
            return FAILED;
        }
        if valid_movement && valid_touch_time && valid_pointers {
            if input.kind != InputKind::End {
                return FAILED;
            }
            let valid_interval = self.p_time.is_none_or(|t| input.time - t < TAP_INTERVAL_MS);
            let valid_multi_tap = self.p_center.is_none_or(|c| point_distance(c, input.center) < TAP_POS_THRESHOLD);
            self.p_time = Some(input.time);
            self.p_center = Some(input.center);
            if !valid_multi_tap || !valid_interval {
                self.count = 1;
            } else {
                self.count += 1;
            }
            self.input = Some(TapInput { at: input.center, count: self.count, modifiers: input.modifiers });
            if self.count.is_multiple_of(self.taps) {
                if !delayed {
                    return RECOGNIZED;
                }
                self.timer = Some(input.time + TAP_INTERVAL_MS);
                return BEGAN;
            }
        }
        FAILED
    }
}

/// A tap's release, as its event reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TapInput {
    at: [f64; 2],
    /// `tapCount`.
    count: u32,
    modifiers: Modifiers,
}

/// A recognised gesture event, as deck's controller hears it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    PanStart { at: [f64; 2], right_button: bool, func_key: bool },
    PanMove { at: [f64; 2] },
    PanEnd,
    PanCancel,
    PinchStart { at: [f64; 2] },
    PinchMove { at: [f64; 2], scale: f64 },
    PinchEnd,
    PinchCancel,
    MultiPanStart { at: [f64; 2], delta: [f64; 2] },
    MultiPanMove { delta: [f64; 2] },
    MultiPanEnd,
    MultiPanCancel,
    DoubleClick(TapInput),
    Click(TapInput),
}

/// hammer's Manager with its pointer store and deck's recognizers.
#[derive(Debug, Clone, PartialEq)]
struct Hammer {
    /// The pointers down, in the order they came (`PointerEventInput.store`).
    store: Vec<(PointerId, [f64; 2])>,
    session: Session,
    multipan: PanRec,
    pinch: u8,
    pan: PanRec,
    dblclick: TapRec,
    click: TapRec,
}

impl Default for Hammer {
    fn default() -> Self {
        Hammer {
            store: Vec::new(),
            session: Session::default(),
            multipan: PanRec::new(2, MULTIPAN_THRESHOLD),
            pinch: POSSIBLE,
            pan: PanRec::new(1, PAN_THRESHOLD),
            dblclick: TapRec::new(2),
            click: TapRec::new(1),
        }
    }
}

impl Hammer {
    /// Where a pointer that is down was last.
    fn position(&self, id: PointerId) -> Option<[f64; 2]> {
        self.store.iter().find(|(p, _)| *p == id).map(|(_, at)| *at)
    }

    /// Whether a finger is down.
    fn has_touch(&self) -> bool {
        self.store.iter().any(|(id, _)| matches!(id, PointerId::Touch(_)))
    }

    /// The next delayed tap emit.
    fn next_due(&self) -> Option<f64> {
        match (self.dblclick.timer, self.click.timer) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Fires the delayed tap emits due by `now_ms`, earliest first, with when each was due.
    fn advance(&mut self, now_ms: f64) -> Vec<(f64, Gesture)> {
        let mut out = Vec::new();
        while let Some(due) = self.next_due().filter(|&t| t <= now_ms) {
            // the double click's first on a tie: its timer would have been set first
            let rec = if self.dblclick.timer == Some(due) { Rec::DoubleClick } else { Rec::Click };
            let tap = if rec == Rec::DoubleClick { &mut self.dblclick } else { &mut self.click };
            tap.timer = None;
            tap.state = RECOGNIZED;
            let input = tap.input;
            let mut gestures = Vec::new();
            self.try_emit_tap(rec, input, &mut gestures);
            out.extend(gestures.into_iter().map(|g| (due, g)));
        }
        out
    }

    /// One pointer event (`PointerEventInput.handler`, then `inputHandler`). Returns the gestures it
    /// recognised and whether it ended the session, or `None` for a pointer not down (a hover).
    fn input(&mut self, kind: InputKind, id: PointerId, at: [f64; 2], cx: &InputCx) -> Option<(Vec<Gesture>, bool)> {
        let index = match (kind, self.store.iter().position(|(p, _)| *p == id)) {
            (InputKind::Start, None) => {
                self.store.push((id, at));
                self.store.len() - 1
            }
            (_, Some(index)) => index,
            (_, None) => return None,
        };
        self.store[index].1 = at;
        let remove = matches!(kind, InputKind::End | InputKind::Cancel);
        // one pointer changes per event
        let others = self.store.len() - 1;
        let is_first = kind == InputKind::Start && others == 0;
        let is_final = remove && others == 0;
        if is_first {
            self.session = Session::default();
        }
        let input = self.compute(kind, cx);
        let mut out = Vec::new();
        self.recognize(&input, cx, &mut out);
        self.session.prev = Some((kind, input.delta));
        if remove {
            self.store.remove(index);
        }
        Some((out, is_final))
    }

    /// `computeInputData` for the store as it is.
    fn compute(&mut self, kind: InputKind, cx: &InputCx) -> Input {
        let pointers: Vec<[f64; 2]> = self.store.iter().map(|(_, p)| *p).collect();
        let time = cx.now_ms;
        let s = &mut self.session;
        if s.first.is_none() {
            s.first = Some(Snapshot::of(&pointers, time));
        }
        if pointers.len() > 1 && s.first_multiple.is_none() {
            s.first_multiple = Some(Snapshot::of(&pointers, time));
        } else if pointers.len() == 1 {
            s.first_multiple = None;
        }
        let (first_center, first_time) = s.first.as_ref().map_or(([0.0, 0.0], time), |f| (f.center, f.time));
        let offset_center = s.first_multiple.as_ref().map_or(first_center, |m| m.center);
        let center = center_of(&pointers);
        let delta_time = time - first_time;
        let distance = point_distance(offset_center, center);
        // `computeDeltaXY`
        if kind == InputKind::Start || s.prev.is_some_and(|(k, _)| k == InputKind::End) {
            s.prev_delta = s.prev.map_or([0.0, 0.0], |(_, d)| d);
            s.offset_delta = center;
        }
        let delta = [s.prev_delta[0] + (center[0] - s.offset_delta[0]), s.prev_delta[1] + (center[1] - s.offset_delta[1])];
        let scale = match &s.first_multiple {
            Some(m) => point_distance(pointers[0], pointers[1]) / point_distance(m.pointers[0], m.pointers[1]),
            None => 1.0,
        };
        // `computeIntervalInputData`: the first input is its own last sample (no velocity yet)
        let direction = match s.last_interval {
            Some((t, _, dir)) if kind == InputKind::Cancel || time - t <= COMPUTE_INTERVAL_MS => dir,
            None if kind == InputKind::Cancel => DIRECTION_NONE,
            last => {
                let d = last.map_or(delta, |(_, d, _)| d);
                let dir = direction_of(delta[0] - d[0], delta[1] - d[1]);
                s.last_interval = Some((time, delta, dir));
                dir
            }
        };
        Input {
            kind,
            pointers,
            center,
            time,
            delta_time,
            distance,
            delta,
            scale,
            direction,
            modifiers: cx.modifiers,
            right_button: cx.right_button,
        }
    }

    fn state(&self, rec: Rec) -> u8 {
        match rec {
            Rec::MultiPan => self.multipan.state,
            Rec::Pinch => self.pinch,
            Rec::Pan => self.pan.state,
            Rec::DoubleClick => self.dblclick.state,
            Rec::Click => self.click.state,
        }
    }

    fn set_state(&mut self, rec: Rec, state: u8) {
        match rec {
            Rec::MultiPan => self.multipan.state = state,
            Rec::Pinch => self.pinch = state,
            Rec::Pan => self.pan.state = state,
            Rec::DoubleClick => self.dblclick.state = state,
            Rec::Click => self.click.state = state,
        }
    }

    fn enabled(rec: Rec, cx: &InputCx) -> bool {
        rec != Rec::MultiPan || cx.multipan
    }

    /// `Recognizer.reset`: only a tap has something to drop (its delayed emit).
    fn reset(&mut self, rec: Rec) {
        match rec {
            Rec::DoubleClick => self.dblclick.timer = None,
            Rec::Click => self.click.timer = None,
            _ => {}
        }
    }

    /// `canEmit`: everything this one requires to fail has failed, or is still possible.
    fn can_emit(&self, rec: Rec) -> bool {
        rec.require_fail().iter().all(|&r| self.state(r) & (FAILED | POSSIBLE) != 0)
    }

    /// `Manager.recognize`.
    fn recognize(&mut self, input: &Input, cx: &InputCx, out: &mut Vec<Gesture>) {
        let mut current = self.session.current;
        if current.is_some_and(|r| self.state(r) & RECOGNIZED != 0) {
            current = None;
            self.session.current = None;
        }
        for rec in ORDER {
            if current.is_none_or(|c| c == rec || rec.simultaneous(c)) {
                self.recognize_one(rec, input, cx, out);
            } else {
                self.reset(rec);
            }
            if current.is_none() && self.state(rec) & (BEGAN | CHANGED | ENDED) != 0 {
                current = Some(rec);
                self.session.current = current;
            }
        }
    }

    /// `Recognizer.recognize`.
    fn recognize_one(&mut self, rec: Rec, input: &Input, cx: &InputCx, out: &mut Vec<Gesture>) {
        if !Self::enabled(rec, cx) {
            self.reset(rec);
            self.set_state(rec, FAILED);
            return;
        }
        if self.state(rec) & (RECOGNIZED | CANCELLED | FAILED) != 0 {
            self.set_state(rec, POSSIBLE);
        }
        let state = match rec {
            Rec::MultiPan => attr_process(self.multipan.state, input.kind, self.multipan.valid(input)),
            Rec::Pan => attr_process(self.pan.state, input.kind, self.pan.valid(input)),
            Rec::Pinch => {
                let valid = input.pointers.len() == 2 && (self.pinch & BEGAN != 0 || (input.scale - 1.0).abs() > PINCH_THRESHOLD);
                attr_process(self.pinch, input.kind, valid)
            }
            Rec::DoubleClick | Rec::Click => {
                // `hasRequireFailures`: the other tap is on
                let delayed = rec.require_fail().iter().any(|&r| Self::enabled(r, cx));
                let tap = if rec == Rec::DoubleClick { &mut self.dblclick } else { &mut self.click };
                tap.process(input, delayed)
            }
        };
        self.set_state(rec, state);
        if state & (BEGAN | CHANGED | ENDED | CANCELLED) == 0 {
            return;
        }
        // `tryEmit`
        match rec {
            Rec::DoubleClick | Rec::Click => {
                let input = if rec == Rec::DoubleClick { self.dblclick.input } else { self.click.input };
                self.try_emit_tap(rec, input, out);
            }
            _ => {
                if !self.can_emit(rec) {
                    self.set_state(rec, FAILED);
                    return;
                }
                self.emit_attr(rec, state, input, out);
            }
        }
    }

    /// A tap's `tryEmit`: it reports only once recognised.
    fn try_emit_tap(&mut self, rec: Rec, input: Option<TapInput>, out: &mut Vec<Gesture>) {
        if !self.can_emit(rec) {
            self.set_state(rec, FAILED);
            return;
        }
        if let Some(input) = input
            && self.state(rec) == RECOGNIZED
        {
            out.push(if rec == Rec::DoubleClick { Gesture::DoubleClick(input) } else { Gesture::Click(input) });
        }
    }

    /// `AttrRecognizer.emit` (and Pan's): `start` or `move` below `Ended`, else `end` or `cancel`.
    fn emit_attr(&mut self, rec: Rec, state: u8, input: &Input, out: &mut Vec<Gesture>) {
        let phase = if state & CANCELLED != 0 {
            Phase::Cancelled
        } else if state & ENDED != 0 {
            Phase::Ended
        } else if state & CHANGED != 0 {
            Phase::Moved
        } else {
            Phase::Started
        };
        let at = input.center;
        let gesture = match (rec, phase) {
            (Rec::Pan, Phase::Started) => {
                Gesture::PanStart { at, right_button: input.right_button, func_key: input.modifiers.func_key() }
            }
            (Rec::Pan, Phase::Moved) => Gesture::PanMove { at },
            (Rec::Pan, Phase::Ended) => Gesture::PanEnd,
            (Rec::Pan, Phase::Cancelled) => Gesture::PanCancel,
            (Rec::Pinch, Phase::Started) => Gesture::PinchStart { at },
            (Rec::Pinch, Phase::Moved) => Gesture::PinchMove { at, scale: input.scale },
            (Rec::Pinch, Phase::Ended) => Gesture::PinchEnd,
            (Rec::Pinch, Phase::Cancelled) => Gesture::PinchCancel,
            (Rec::MultiPan, Phase::Started) => Gesture::MultiPanStart { at, delta: input.delta },
            (Rec::MultiPan, Phase::Moved) => Gesture::MultiPanMove { delta: input.delta },
            (Rec::MultiPan, Phase::Ended) => Gesture::MultiPanEnd,
            (Rec::MultiPan, Phase::Cancelled) => Gesture::MultiPanCancel,
            (Rec::DoubleClick | Rec::Click, _) => return,
        };
        match rec {
            Rec::Pan => self.pan.px = Some(input.delta[0]),
            Rec::MultiPan => self.multipan.px = Some(input.delta[0]),
            _ => {}
        }
        out.push(gesture);
    }
}

/// `AttrRecognizer.process`.
fn attr_process(state: u8, kind: InputKind, valid: bool) -> u8 {
    let is_recognized = state & (BEGAN | CHANGED) != 0;
    if is_recognized && (kind == InputKind::Cancel || !valid) {
        state | CANCELLED
    } else if is_recognized || valid {
        if kind == InputKind::End {
            state | ENDED
        } else if state & BEGAN == 0 {
            BEGAN
        } else {
            state | CHANGED
        }
    } else {
        FAILED
    }
}

/// `getCenter`: the pointers' mean, rounded to whole pixels as `Math.round` rounds.
fn center_of(pointers: &[[f64; 2]]) -> [f64; 2] {
    let n = pointers.len() as f64;
    let (x, y) = pointers.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
    [js_round(x / n), js_round(y / n)]
}

fn point_distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (x, y) = (b[0] - a[0], b[1] - a[1]);
    (x * x + y * y).sqrt()
}

/// `getDirection`.
fn direction_of(dx: f64, dy: f64) -> u8 {
    if dx == dy {
        DIRECTION_NONE
    } else if dx.abs() >= dy.abs() {
        if dx < 0.0 { DIRECTION_LEFT } else { DIRECTION_RIGHT }
    } else if dy < 0.0 {
        DIRECTION_UP
    } else {
        DIRECTION_DOWN
    }
}

#[cfg(test)]
mod tests {
    use rustradar_scene::camera::ViewState;
    use rustradar_scene::controller::wheel_scale;
    use winit::dpi::PhysicalPosition;
    use winit::event::{DeviceId, Touch};
    use winit::keyboard::{NamedKey, SmolStr};

    use super::*;

    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// The canvas's centre.
    const MID: [f64; 2] = [550.0, 360.0];
    const T0: f64 = 1.0e12;

    fn start_view() -> ViewState {
        ViewState { longitude: 10.0, latitude: 35.0, zoom: 4.0, bearing: 0.0, pitch: 0.0, position: None }
    }

    fn controller() -> Controller {
        Controller::new(start_view(), SIZE)
    }

    /// Feeds `event` at `t`, egui wanting nothing.
    fn send(input: &mut MapInput, c: &mut Controller, event: MapEvent, t: f64) -> bool {
        input.handle(event, false, c, t)
    }

    fn press(button: Button) -> MapEvent {
        MapEvent::Button { button, pressed: true }
    }

    fn release(button: Button) -> MapEvent {
        MapEvent::Button { button, pressed: false }
    }

    fn touch(id: u64, phase: Phase, at: [f64; 2]) -> MapEvent {
        MapEvent::Touch { id, phase, at }
    }

    fn key(c: char) -> MapEvent {
        MapEvent::Key { key: KeyName::Char(c), pressed: true }
    }

    fn shift() -> MapEvent {
        MapEvent::Modifiers(Modifiers { shift: true, ..Default::default() })
    }

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() < tolerance
    }

    /// A drag from `from` to `to` in ten moves 16 ms apart, then the release.
    fn drag(input: &mut MapInput, c: &mut Controller, button: Button, from: [f64; 2], to: [f64; 2]) {
        input.handle(MapEvent::PointerMoved(from), false, c, 1000.0);
        input.handle(press(button), false, c, 1000.0);
        for i in 1..=10 {
            let t = i as f64 / 10.0;
            let at = [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t];
            input.handle(MapEvent::PointerMoved(at), false, c, 1000.0 + 16.0 * i as f64);
        }
        input.handle(release(button), false, c, 1200.0);
    }

    // -----------------------------------------------------------------------------------------
    // The recognizers against mjolnir 3.1.1's own
    // -----------------------------------------------------------------------------------------

    /// One step of a scenario: when, what (`d`own, `m`ove, `u`p, `c`ancel), which pointer (0 the
    /// mouse, else a finger), where.
    type Step = (f64, char, u64, f64, f64);

    /// Runs `steps` through the recognizers as the harness ran mjolnir's (below): the timers due by
    /// each step first, then the step, and two seconds of timers after the last.
    fn hammer_run(steps: &[Step], locked: bool) -> Vec<(f64, Gesture)> {
        let mut h = Hammer::default();
        let mut log = Vec::new();
        let mut last = 0.0;
        for &(t, what, id, x, y) in steps {
            log.extend(h.advance(t));
            let kind = match what {
                'd' => InputKind::Start,
                'm' => InputKind::Move,
                'u' => InputKind::End,
                _ => InputKind::Cancel,
            };
            let id = if id == 0 { PointerId::Mouse } else { PointerId::Touch(id) };
            let cx = InputCx { now_ms: t, modifiers: Modifiers::default(), right_button: false, multipan: !locked };
            if let Some((gestures, _)) = h.input(kind, id, [x, y], &cx) {
                log.extend(gestures.into_iter().map(|g| (t, g)));
            }
            last = t;
        }
        log.extend(h.advance(last + 2000.0));
        log
    }

    /// A gesture as the harness logged it: its event's name and, for those that have one, the
    /// centre deck reads.
    fn named(g: &Gesture) -> (&'static str, Option<[f64; 2]>) {
        match *g {
            Gesture::PanStart { at, .. } => ("panstart", Some(at)),
            Gesture::PanMove { at } => ("panmove", Some(at)),
            Gesture::PanEnd => ("panend", None),
            Gesture::PanCancel => ("pancancel", None),
            Gesture::PinchStart { at } => ("pinchstart", Some(at)),
            Gesture::PinchMove { at, .. } => ("pinchmove", Some(at)),
            Gesture::PinchEnd => ("pinchend", None),
            Gesture::PinchCancel => ("pinchcancel", None),
            Gesture::MultiPanStart { at, .. } => ("multipanstart", Some(at)),
            Gesture::MultiPanMove { .. } => ("multipanmove", None),
            Gesture::MultiPanEnd => ("multipanend", None),
            Gesture::MultiPanCancel => ("multipancancel", None),
            Gesture::DoubleClick(tap) => ("dblclick", Some(tap.at)),
            Gesture::Click(tap) => ("click", Some(tap.at)),
        }
    }

    fn assert_log(steps: &[Step], locked: bool, expected: &[(f64, &str, Option<[f64; 2]>)]) -> Vec<(f64, Gesture)> {
        let log = hammer_run(steps, locked);
        let got: Vec<(f64, &str, Option<[f64; 2]>)> = log.iter().map(|(t, g)| {
            let (name, at) = named(g);
            (*t, name, at)
        }).collect();
        assert_eq!(got, expected);
        log
    }

    // The expected logs are what mjolnir 3.1.1's hammer Manager emitted for the same pointer
    // events, run in bun with deck 9.4's `RECOGNIZERS` (the pan recognizer of the unlocked
    // controller's settings, `dblclick` on), a fake clock and fake timers.

    #[test]
    fn a_mouse_drag_starts_where_hammer_recognises_it() {
        // sideways in the first 25 ms: the fallback to the sideways delta starts it at once
        let fast = [(1000.0, 'd', 0, 550.0, 360.0), (1016.0, 'm', 0, 548.0, 360.0), (1032.0, 'm', 0, 540.0, 360.0), (1048.0, 'm', 0, 520.0, 360.0), (1064.0, 'u', 0, 520.0, 360.0)];
        assert_log(&fast, false, &[
            (1016.0, "panstart", Some([548.0, 360.0])),
            (1032.0, "panmove", Some([540.0, 360.0])),
            (1048.0, "panmove", Some([520.0, 360.0])),
            (1064.0, "panend", None),
        ]);
        // straight down: no direction until 25 ms have passed; centres rounded
        let down = [(1000.0, 'd', 0, 550.4, 360.6), (1010.0, 'm', 0, 550.4, 362.6), (1020.0, 'm', 0, 550.4, 364.6), (1030.0, 'm', 0, 550.4, 366.6), (1046.0, 'm', 0, 550.4, 380.6), (1062.0, 'u', 0, 550.4, 380.6)];
        assert_log(&down, false, &[
            (1030.0, "panstart", Some([550.0, 367.0])),
            (1046.0, "panmove", Some([550.0, 381.0])),
            (1062.0, "panend", None),
        ]);
        // an exact diagonal has no direction: the sideways delta decides
        let diagonal = [(1000.0, 'd', 0, 500.0, 300.0), (1030.0, 'm', 0, 503.0, 303.0), (1060.0, 'm', 0, 506.0, 306.0), (1090.0, 'm', 0, 510.0, 307.0), (1120.0, 'u', 0, 510.0, 307.0)];
        assert_log(&diagonal, false, &[
            (1030.0, "panstart", Some([503.0, 303.0])),
            (1060.0, "panmove", Some([506.0, 306.0])),
            (1090.0, "panmove", Some([510.0, 307.0])),
            (1120.0, "panend", None),
        ]);
        // a press a click ago, then a drag: the drag wins, no click
        let click_then_drag = [(1000.0, 'd', 0, 400.0, 300.0), (1060.0, 'u', 0, 400.0, 300.0), (1120.0, 'd', 0, 400.0, 300.0), (1160.0, 'm', 0, 420.0, 300.0), (1200.0, 'm', 0, 440.0, 310.0), (1240.0, 'u', 0, 440.0, 310.0)];
        assert_log(&click_then_drag, false, &[
            (1160.0, "panstart", Some([420.0, 300.0])),
            (1200.0, "panmove", Some([440.0, 310.0])),
            (1240.0, "panend", None),
        ]);
    }

    #[test]
    fn clicks_and_double_clicks_wait_300_ms_as_hammer_s() {
        let double = [(1000.0, 'd', 0, 400.0, 300.0), (1080.0, 'u', 0, 400.0, 300.0), (1200.0, 'd', 0, 403.0, 302.0), (1280.0, 'u', 0, 403.0, 302.0)];
        let log = assert_log(&double, false, &[(1580.0, "dblclick", Some([403.0, 302.0]))]);
        assert!(matches!(log[0].1, Gesture::DoubleClick(TapInput { count: 2, .. })));
        let single = [(1000.0, 'd', 0, 400.0, 300.0), (1080.0, 'u', 0, 401.0, 300.0)];
        assert_log(&single, false, &[(1380.0, "click", Some([401.0, 300.0]))]);
        // 310 ms between the releases: one click, the first cancelled by the second press
        let slow = [(1000.0, 'd', 0, 400.0, 300.0), (1080.0, 'u', 0, 400.0, 300.0), (1300.0, 'd', 0, 400.0, 300.0), (1390.0, 'u', 0, 400.0, 300.0)];
        assert_log(&slow, false, &[(1690.0, "click", Some([400.0, 300.0]))]);
        // 12 px apart: no double click
        let far = [(1000.0, 'd', 0, 400.0, 300.0), (1080.0, 'u', 0, 400.0, 300.0), (1150.0, 'd', 0, 412.0, 300.0), (1200.0, 'u', 0, 412.0, 300.0)];
        assert_log(&far, false, &[(1500.0, "click", Some([412.0, 300.0]))]);
        // held 300 ms: not a click
        assert_log(&[(1000.0, 'd', 0, 400.0, 300.0), (1300.0, 'u', 0, 400.0, 300.0)], false, &[]);
        // three clicks: the third press cancels the double click; a click counted twice
        let triple = [(1000.0, 'd', 0, 400.0, 300.0), (1060.0, 'u', 0, 400.0, 300.0), (1120.0, 'd', 0, 400.0, 300.0), (1180.0, 'u', 0, 400.0, 300.0), (1240.0, 'd', 0, 400.0, 300.0), (1300.0, 'u', 0, 400.0, 300.0)];
        let log = assert_log(&triple, false, &[(1600.0, "click", Some([400.0, 300.0]))]);
        assert!(matches!(log[0].1, Gesture::Click(TapInput { count: 2, .. })));
        // a double tap is a double click
        let taps = [(1000.0, 'd', 1, 300.0, 300.0), (1050.0, 'u', 1, 300.0, 300.0), (1150.0, 'd', 1, 304.0, 303.0), (1200.0, 'u', 1, 304.0, 303.0)];
        assert_log(&taps, false, &[(1500.0, "dblclick", Some([304.0, 303.0]))]);
    }

    #[test]
    fn two_fingers_pinch_or_pan_as_hammer_s() {
        let pinch = [
            (1000.0, 'd', 1, 500.0, 400.0),
            (1005.0, 'd', 2, 600.0, 400.0),
            (1021.0, 'm', 1, 490.0, 400.0),
            (1037.0, 'm', 2, 610.0, 401.0),
            (1053.0, 'm', 1, 450.0, 402.0),
            (1069.0, 'm', 2, 650.0, 404.0),
            (1085.0, 'u', 1, 450.0, 402.0),
            (1101.0, 'm', 2, 640.0, 404.0),
            (1117.0, 'm', 2, 620.0, 410.0),
            (1133.0, 'u', 2, 620.0, 410.0),
        ];
        let log = assert_log(&pinch, false, &[
            (1021.0, "pinchstart", Some([545.0, 400.0])),
            (1037.0, "pinchmove", Some([550.0, 401.0])),
            (1053.0, "pinchmove", Some([530.0, 402.0])),
            (1069.0, "pinchmove", Some([550.0, 403.0])),
            (1085.0, "pinchend", None),
            (1117.0, "panstart", Some([620.0, 410.0])),
            (1133.0, "panend", None),
        ]);
        let scales: Vec<f64> = log.iter().filter_map(|(_, g)| if let Gesture::PinchMove { scale, .. } = g { Some(*scale) } else { None }).collect();
        for (got, want) in scales.iter().zip([1.2000416659433122, 1.6000312496948303, 2.000099997500125]) {
            assert!(close(*got, want, 1e-12), "{scales:?}");
        }
        // dragged together 15 px at the first move: the two-finger pan owns the session...
        let together = [
            (1000.0, 'd', 1, 500.0, 400.0),
            (1005.0, 'd', 2, 600.0, 400.0),
            (1040.0, 'm', 1, 500.0, 430.0),
            (1056.0, 'm', 2, 600.0, 431.0),
            (1072.0, 'm', 1, 500.0, 450.0),
            (1088.0, 'u', 1, 500.0, 450.0),
            (1104.0, 'u', 2, 600.0, 431.0),
        ];
        let log = assert_log(&together, false, &[
            (1040.0, "multipanstart", Some([550.0, 415.0])),
            (1056.0, "multipanmove", None),
            (1072.0, "multipanmove", None),
            (1088.0, "multipanend", None),
        ]);
        let deltas: Vec<[f64; 2]> = log.iter().filter_map(|(_, g)| match g {
            Gesture::MultiPanStart { delta, .. } | Gesture::MultiPanMove { delta } => Some(*delta),
            _ => None,
        }).collect();
        assert_eq!(deltas, [[0.0, 15.0], [0.0, 31.0], [0.0, 41.0]]);
        // ...unless the camera is locked (no `multiTouchDrag`): then it is a pinch
        let locked = [
            (1000.0, 'd', 1, 500.0, 400.0),
            (1005.0, 'd', 2, 600.0, 400.0),
            (1040.0, 'm', 1, 500.0, 430.0),
            (1056.0, 'm', 2, 600.0, 431.0),
            (1072.0, 'u', 1, 500.0, 430.0),
            (1088.0, 'u', 2, 600.0, 431.0),
        ];
        assert_log(&locked, true, &[
            (1040.0, "pinchstart", Some([550.0, 415.0])),
            (1056.0, "pinchmove", Some([550.0, 431.0])),
            (1072.0, "pinchend", None),
            (1088.0, "panend", None),
        ]);
        // a second finger during a pan cancels it, and a pinch takes over
        let second = [
            (1000.0, 'd', 1, 500.0, 400.0),
            (1030.0, 'm', 1, 480.0, 400.0),
            (1060.0, 'm', 1, 460.0, 400.0),
            (1070.0, 'd', 2, 560.0, 400.0),
            (1090.0, 'm', 2, 570.0, 400.0),
            (1110.0, 'm', 1, 450.0, 400.0),
            (1130.0, 'u', 2, 570.0, 400.0),
            (1150.0, 'm', 1, 440.0, 405.0),
            (1170.0, 'u', 1, 440.0, 405.0),
        ];
        assert_log(&second, false, &[
            (1030.0, "panstart", Some([480.0, 400.0])),
            (1060.0, "panmove", Some([460.0, 400.0])),
            (1070.0, "pancancel", None),
            (1090.0, "pinchstart", Some([515.0, 400.0])),
            (1110.0, "pinchmove", Some([510.0, 400.0])),
            (1130.0, "pinchend", None),
            (1150.0, "panstart", Some([440.0, 405.0])),
            (1170.0, "panend", None),
        ]);
        // a cancelled touch cancels its pan
        let cancelled = [(1000.0, 'd', 1, 300.0, 300.0), (1030.0, 'm', 1, 320.0, 300.0), (1060.0, 'c', 1, 330.0, 300.0)];
        assert_log(&cancelled, false, &[(1030.0, "panstart", Some([320.0, 300.0])), (1060.0, "pancancel", None)]);
    }

    // -----------------------------------------------------------------------------------------
    // The mouse
    // -----------------------------------------------------------------------------------------

    #[test]
    fn a_left_drag_pans_and_a_right_drag_rotates() {
        let mut c = controller();
        let mut input = MapInput::default();
        let before = c.view();
        drag(&mut input, &mut c, Button::Left, [550.0, 360.0], [450.0, 360.0]);
        let panned = c.view();
        assert!(panned.longitude > before.longitude, "dragging left moves the map east: {panned:?}");
        assert_eq!(panned.bearing, before.bearing);
        assert!(!input.dragging() && !c.is_dragging());

        let mut c = controller();
        drag(&mut input, &mut c, Button::Right, [550.0, 360.0], [650.0, 300.0]);
        let turned = c.view();
        assert_ne!(turned.bearing, 0.0, "{turned:?}");
        assert!(turned.pitch > 0.0, "dragging up tilts: {turned:?}");
        assert_eq!((turned.longitude, turned.latitude), (before.longitude, before.latitude));
        // a middle drag pans
        let mut c = controller();
        drag(&mut input, &mut c, Button::Middle, [550.0, 360.0], [450.0, 360.0]);
        assert_eq!(c.view(), panned);
    }

    #[test]
    fn the_drag_is_deck_s_from_where_it_was_recognised() {
        // the pan starts at the first move past 1 px: what deck does with the same events
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, MapEvent::PointerMoved(MID), T0);
        assert!(!send(&mut input, &mut c, press(Button::Left), T0), "a press alone moves nothing");
        assert!(!c.is_dragging(), "nor does it grab before it moves");
        assert!(!send(&mut input, &mut c, MapEvent::PointerMoved([550.4, 360.0]), T0 + 8.0), "under a pixel");
        assert!(send(&mut input, &mut c, MapEvent::PointerMoved([548.0, 360.0]), T0 + 16.0));
        assert!(c.is_dragging());
        send(&mut input, &mut c, MapEvent::PointerMoved([500.0, 330.0]), T0 + 32.0);
        send(&mut input, &mut c, release(Button::Left), T0 + 48.0);
        let mut deck = controller();
        assert!(deck.pan_start([548.0, 360.0], false, false, T0 + 16.0));
        deck.pan_move([500.0, 330.0]);
        deck.pan_end();
        assert_eq!(c.view(), deck.view());
        // a right drag rotates from where it was recognised too
        let mut c = controller();
        send(&mut input, &mut c, press(Button::Right), T0 + 1000.0);
        send(&mut input, &mut c, MapEvent::PointerMoved([560.0, 350.0]), T0 + 1030.0);
        send(&mut input, &mut c, MapEvent::PointerMoved([600.0, 300.0]), T0 + 1060.0);
        send(&mut input, &mut c, release(Button::Right), T0 + 1090.0);
        let mut deck = controller();
        deck.pan_start([560.0, 350.0], true, false, T0 + 1030.0);
        deck.pan_move([600.0, 300.0]);
        deck.pan_end();
        assert_eq!(c.view(), deck.view());
        assert_ne!(c.view().bearing, 0.0);
    }

    #[test]
    fn a_modifier_makes_a_left_drag_rotate() {
        let mut c = controller();
        let mut input = MapInput::default();
        input.handle(shift(), false, &mut c, 0.0);
        assert!(input.modifiers().func_key());
        drag(&mut input, &mut c, Button::Left, [550.0, 360.0], [650.0, 360.0]);
        assert_ne!(c.view().bearing, 0.0);
        assert_eq!(c.view().longitude, 10.0);
    }

    #[test]
    fn egui_keeps_presses_and_wheels_it_wants() {
        let mut c = controller();
        let mut input = MapInput::default();
        input.handle(MapEvent::PointerMoved([100.0, 100.0]), true, &mut c, 0.0);
        assert!(!input.handle(press(Button::Left), true, &mut c, 0.0));
        assert!(!input.dragging());
        assert!(!input.handle(MapEvent::PointerMoved([300.0, 100.0]), false, &mut c, 16.0));
        let notch = MapEvent::Wheel { delta_x: 0.0, delta_y: -100.0, mode: DeltaMode::Pixel, wheel_delta_y: Some(120.0) };
        assert!(!input.handle(notch, true, &mut c, 32.0));
        input.handle(release(Button::Left), false, &mut c, 40.0);
        assert_eq!(c.view(), controller().view());
        assert!(input.take_clicks().is_empty(), "a click on egui is egui's");
    }

    #[test]
    fn a_drag_the_map_started_stays_the_maps_over_egui() {
        let mut c = controller();
        let mut input = MapInput::default();
        input.handle(MapEvent::PointerMoved([550.0, 360.0]), false, &mut c, 0.0);
        input.handle(press(Button::Left), false, &mut c, 0.0);
        assert!(input.dragging());
        assert!(input.handle(MapEvent::PointerMoved([540.0, 360.0]), false, &mut c, 16.0));
        // over a panel now: the drag goes on
        assert!(input.handle(MapEvent::PointerMoved([200.0, 360.0]), true, &mut c, 32.0));
        let panned = c.view();
        assert!(panned.longitude > 10.0);
        // another button meanwhile starts nothing new; the drag ends with the last button up, as
        // pointer events end
        input.handle(press(Button::Right), true, &mut c, 40.0);
        assert_eq!(c.view(), panned);
        input.handle(release(Button::Left), true, &mut c, 48.0);
        assert!(input.dragging() && c.is_dragging());
        input.handle(release(Button::Right), true, &mut c, 56.0);
        assert!(!input.dragging());
        assert!(!c.is_dragging());
        assert_eq!(c.view(), panned);
    }

    #[test]
    fn a_release_after_the_mouse_left_the_window_ends_the_drag() {
        let mut c = controller();
        let mut input = MapInput::default();
        input.handle(MapEvent::PointerMoved([550.0, 360.0]), false, &mut c, 0.0);
        input.handle(press(Button::Left), false, &mut c, 0.0);
        input.handle(MapEvent::PointerMoved([500.0, 360.0]), false, &mut c, 16.0);
        input.handle(MapEvent::PointerLeft, false, &mut c, 20.0);
        input.handle(release(Button::Left), false, &mut c, 24.0);
        assert!(!input.dragging() && !c.is_dragging());
    }

    #[test]
    fn losing_the_focus_ends_a_drag() {
        let mut c = controller();
        let mut input = MapInput::default();
        input.handle(MapEvent::PointerMoved([550.0, 360.0]), false, &mut c, 0.0);
        input.handle(press(Button::Left), false, &mut c, 0.0);
        input.handle(MapEvent::PointerMoved([500.0, 360.0]), false, &mut c, 16.0);
        assert!(c.is_dragging());
        input.handle(MapEvent::FocusLost, false, &mut c, 20.0);
        assert!(!input.dragging() && !c.is_dragging());
        let after = c.view();
        assert!(!input.handle(MapEvent::PointerMoved([300.0, 360.0]), false, &mut c, 40.0));
        assert_eq!(c.view(), after);
        // the release that comes back later changes nothing
        assert!(!input.handle(release(Button::Left), false, &mut c, 60.0));
    }

    #[test]
    fn a_double_click_zooms_one_level_300_ms_after_the_second_release() {
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, MapEvent::PointerMoved(MID), T0);
        send(&mut input, &mut c, press(Button::Left), T0);
        send(&mut input, &mut c, release(Button::Left), T0 + 80.0);
        send(&mut input, &mut c, press(Button::Left), T0 + 200.0);
        send(&mut input, &mut c, release(Button::Left), T0 + 280.0);
        assert_eq!(c.view(), start_view(), "nothing until hammer's 300 ms are up");
        assert_eq!(input.next_due_ms(), T0 + 580.0);
        assert!(!input.update(&mut c, T0 + 579.0));
        assert!(input.update(&mut c, T0 + 580.0));
        assert!(c.in_transition(), "deck's 300 ms zoom");
        assert_eq!(input.next_due_ms(), f64::INFINITY);
        c.update(T0 + 880.0);
        let v = c.view();
        assert!(close(v.zoom, 5.0, 1e-9) && close(v.longitude, 10.0, 1e-9) && close(v.latitude, 35.0, 1e-9), "{v:?}");
        assert_eq!(input.take_clicks(), [Click { at: MID, tap_count: 2, modifiers: Modifiers::default() }]);
        // with a modifier: one level out
        let mut c = controller();
        send(&mut input, &mut c, shift(), T0 + 2000.0);
        for t in [2000.0, 2080.0, 2200.0, 2280.0] {
            let event = if t == 2000.0 || t == 2200.0 { press(Button::Left) } else { release(Button::Left) };
            send(&mut input, &mut c, event, T0 + t);
        }
        input.update(&mut c, T0 + 2580.0);
        c.update(T0 + 2880.0);
        assert!(close(c.view().zoom, 3.0, 1e-9), "{:?}", c.view());
    }

    #[test]
    fn a_click_is_reported_after_300_ms_and_moves_nothing() {
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, MapEvent::PointerMoved([300.4, 200.6]), T0);
        send(&mut input, &mut c, press(Button::Left), T0);
        send(&mut input, &mut c, release(Button::Left), T0 + 90.0);
        assert!(input.take_clicks().is_empty());
        // the timer fires with the next event too, before it, when the app has not updated
        assert!(!send(&mut input, &mut c, MapEvent::PointerMoved([310.0, 200.0]), T0 + 400.0));
        assert_eq!(input.take_clicks(), [Click { at: [300.0, 201.0], tap_count: 1, modifiers: Modifiers::default() }]);
        assert_eq!(c.view(), start_view());
        // three clicks: no zoom, a click counted twice
        for (i, t) in [0.0, 60.0, 120.0, 180.0, 240.0, 300.0].into_iter().enumerate() {
            let event = if i % 2 == 0 { press(Button::Left) } else { release(Button::Left) };
            send(&mut input, &mut c, event, T0 + 1000.0 + t);
        }
        input.update(&mut c, T0 + 2000.0);
        assert_eq!(c.view(), start_view());
        assert_eq!(input.take_clicks().iter().map(|k| k.tap_count).collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn the_wheel_zooms_about_the_pointer() {
        let mut c = controller();
        let mut input = MapInput::default();
        // no pointer yet: nowhere to zoom about
        let notch = WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::LineDelta(0.0, 1.0),
            phase: TouchPhase::Moved,
        };
        let event = map_event(&notch, 2.0).expect("a wheel event");
        assert!(!send(&mut input, &mut c, event, 0.0));
        send(&mut input, &mut c, MapEvent::PointerMoved(MID), 0.0);
        // one notch up, as WebView2 reports it: 100 px, `wheelDeltaY` 120
        assert_eq!(event, MapEvent::Wheel { delta_x: -0.0, delta_y: -100.0, mode: DeltaMode::Pixel, wheel_delta_y: Some(120.0) });
        assert!(send(&mut input, &mut c, event, 16.0));
        let v = c.view();
        assert!(close(v.zoom, 4.0 + wheel_scale(100.0).log2(), 1e-9) && close(v.longitude, 10.0, 1e-9), "{v:?}");
        assert!(close(wheel_scale(100.0).log2(), 0.548, 1e-3), "a notch is about half a level, as on Windows");
        // with Shift a quarter as fast
        let mut c = controller();
        send(&mut input, &mut c, shift(), 20.0);
        send(&mut input, &mut c, event, 32.0);
        assert!(close(c.view().zoom, 4.0 + wheel_scale(25.0).log2(), 1e-9), "{:?}", c.view());
        // off the centre, about the pointer as deck's own wheel zoom (whose anchor on the globe
        // drifts with the zoom's latitude coupling: `controller::a_wheel_zoom_keeps_the_point_...`)
        let mut c = controller();
        send(&mut input, &mut c, MapEvent::Modifiers(Modifiers::default()), 40.0);
        send(&mut input, &mut c, MapEvent::PointerMoved([300.0, 200.0]), 40.0);
        send(&mut input, &mut c, event, 48.0);
        let mut deck = controller();
        assert!(deck.wheel_zoom([300.0, 200.0], 100.0));
        assert_eq!(c.view(), deck.view());
        assert!(c.view().latitude > 35.0 && c.view().longitude < 10.0, "toward the pointer: {:?}", c.view());
    }

    #[test]
    fn a_trackpad_scroll_pans_and_with_ctrl_zooms() {
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, MapEvent::PointerMoved([300.0, 200.0]), T0);
        // two fingers up on a trackpad: winit's pixel delta, the map scrolls down to the south
        let scroll = WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -40.0)),
            phase: TouchPhase::Moved,
        };
        let event = map_event(&scroll, 2.0).expect("a wheel event");
        assert!(send(&mut input, &mut c, event, T0));
        let v = c.view();
        assert!(v.latitude < 35.0 && close(v.longitude, 10.0, 1e-9) && v.zoom == 4.0, "{v:?}");
        // with Ctrl (a pinch outside WebKit) it zooms out about the pointer (here the centre, so
        // the latitude and with it the zoom's coupling stay)
        let mut c = controller();
        send(&mut input, &mut c, MapEvent::PointerMoved(MID), T0);
        send(&mut input, &mut c, MapEvent::Modifiers(Modifiers { ctrl: true, ..Default::default() }), T0);
        send(&mut input, &mut c, event, T0);
        assert!(close(c.view().zoom, 4.0 + wheel_scale(-20.0).log2(), 1e-9), "{:?}", c.view());
        // a sideways notch pans, as `deltaX != 0` does today
        let mut c = controller();
        send(&mut input, &mut c, MapEvent::Modifiers(Modifiers::default()), T0);
        let sideways = WindowEvent::MouseWheel { device_id: DeviceId::dummy(), delta: MouseScrollDelta::LineDelta(1.0, 0.0), phase: TouchPhase::Moved };
        send(&mut input, &mut c, map_event(&sideways, 1.0).expect("wheel"), T0);
        assert!(c.view().longitude < 10.0 && c.view().zoom == 4.0, "{:?}", c.view());
        // nothing to scroll: nothing done
        let mut c = controller();
        let still = MapEvent::Wheel { delta_x: 0.0, delta_y: 0.0, mode: DeltaMode::Pixel, wheel_delta_y: Some(0.0) };
        assert!(!send(&mut input, &mut c, still, T0));
        assert_eq!(c.view(), start_view());
    }

    #[test]
    fn a_trackpad_pinch_zooms_about_the_pointer_and_the_map_turns_with_the_fingers() {
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, MapEvent::PointerMoved(MID), T0);
        send(&mut input, &mut c, MapEvent::Pinch { delta: 0.0, phase: Phase::Started }, T0);
        assert!(send(&mut input, &mut c, MapEvent::Pinch { delta: 0.5, phase: Phase::Moved }, T0 + 16.0));
        send(&mut input, &mut c, MapEvent::Rotate { delta: 0.0, phase: Phase::Started }, T0 + 20.0);
        // counterclockwise 30°: the map turns counterclockwise with the fingers (bearing +30)
        send(&mut input, &mut c, MapEvent::Rotate { delta: 30.0, phase: Phase::Moved }, T0 + 24.0);
        send(&mut input, &mut c, MapEvent::Pinch { delta: 0.5, phase: Phase::Moved }, T0 + 32.0);
        // a NaN from the platform changes nothing
        assert!(!send(&mut input, &mut c, MapEvent::Pinch { delta: f64::NAN, phase: Phase::Moved }, T0 + 36.0));
        let v = c.view();
        assert!(close(v.zoom, 5.0, 1e-9) && close(v.bearing, 30.0, 1e-9), "{v:?}");
        assert!(close(v.longitude, 10.0, 1e-9) && close(v.latitude, 35.0, 1e-9), "{v:?}");
        // Ctrl + wheel meanwhile is the same pinch as WebKit sends it: swallowed
        send(&mut input, &mut c, MapEvent::Modifiers(Modifiers { ctrl: true, ..Default::default() }), T0 + 40.0);
        let ctrl_wheel = MapEvent::Wheel { delta_x: 0.0, delta_y: -2.5, mode: DeltaMode::Pixel, wheel_delta_y: Some(7.5) };
        send(&mut input, &mut c, ctrl_wheel, T0 + 40.0);
        assert_eq!(c.view(), v);
        // over when both have ended
        send(&mut input, &mut c, MapEvent::Pinch { delta: 0.0, phase: Phase::Ended }, T0 + 48.0);
        send(&mut input, &mut c, ctrl_wheel, T0 + 50.0);
        assert_eq!(c.view(), v, "the twist still runs");
        send(&mut input, &mut c, MapEvent::Rotate { delta: 0.0, phase: Phase::Ended }, T0 + 56.0);
        send(&mut input, &mut c, ctrl_wheel, T0 + 60.0);
        assert!(c.view().zoom > 5.0, "{:?}", c.view());
        // begun over egui, it is egui's
        let mut c = controller();
        input.handle(MapEvent::Pinch { delta: 0.0, phase: Phase::Started }, true, &mut c, T0 + 100.0);
        assert!(!input.handle(MapEvent::Pinch { delta: 1.0, phase: Phase::Moved }, false, &mut c, T0 + 116.0));
        input.handle(MapEvent::Pinch { delta: 0.0, phase: Phase::Ended }, false, &mut c, T0 + 132.0);
        assert_eq!(c.view(), start_view());
    }

    // -----------------------------------------------------------------------------------------
    // Touch
    // -----------------------------------------------------------------------------------------

    #[test]
    fn one_finger_pans() {
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, touch(3, Phase::Started, MID), T0);
        send(&mut input, &mut c, touch(3, Phase::Moved, [500.0, 360.0]), T0 + 16.0);
        assert!(c.is_dragging() && input.dragging());
        send(&mut input, &mut c, touch(3, Phase::Moved, [450.0, 360.0]), T0 + 32.0);
        send(&mut input, &mut c, touch(3, Phase::Ended, [450.0, 360.0]), T0 + 48.0);
        assert!(!c.is_dragging() && !input.dragging());
        let mut deck = controller();
        deck.pan_start([500.0, 360.0], false, false, T0 + 16.0);
        deck.pan_move([450.0, 360.0]);
        deck.pan_end();
        assert_eq!(c.view(), deck.view());
        assert!(c.view().longitude > 10.0);
        // a finger coming down over egui is egui's
        let mut c = controller();
        input.handle(touch(4, Phase::Started, MID), true, &mut c, T0 + 100.0);
        input.handle(touch(4, Phase::Moved, [400.0, 360.0]), false, &mut c, T0 + 116.0);
        input.handle(touch(4, Phase::Ended, [400.0, 360.0]), false, &mut c, T0 + 132.0);
        assert_eq!(c.view(), start_view());
    }

    #[test]
    fn two_fingers_pinch_zoom_about_them_without_twist_or_tilt() {
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, touch(1, Phase::Started, [450.0, 360.0]), T0);
        send(&mut input, &mut c, touch(2, Phase::Started, [650.0, 360.0]), T0 + 5.0);
        // turned a quarter round while spreading to twice the distance
        for k in 1..=8 {
            let (angle, r) = (std::f64::consts::FRAC_PI_2 * k as f64 / 8.0, 100.0 * (1.0 + k as f64 / 8.0));
            let (dx, dy) = (r * angle.cos(), r * angle.sin());
            let t = T0 + 5.0 + 16.0 * k as f64;
            send(&mut input, &mut c, touch(1, Phase::Moved, [MID[0] - dx, MID[1] - dy]), t);
            send(&mut input, &mut c, touch(2, Phase::Moved, [MID[0] + dx, MID[1] + dy]), t + 8.0);
        }
        let v = c.view();
        assert!(close(v.zoom, 5.0, 0.02), "twice as far apart: a level closer: {v:?}");
        assert_eq!((v.bearing, v.pitch), (0.0, 0.0), "no twist, no tilt");
        send(&mut input, &mut c, touch(1, Phase::Ended, [550.0, 160.0]), T0 + 200.0);
        send(&mut input, &mut c, touch(2, Phase::Ended, [550.0, 560.0]), T0 + 216.0);
        assert!(!c.is_dragging() && !input.dragging());
    }

    #[test]
    fn two_fingers_dragged_together_pan_unless_locked() {
        let steps = |input: &mut MapInput, c: &mut Controller| {
            send(input, c, touch(1, Phase::Started, [500.0, 400.0]), T0);
            send(input, c, touch(2, Phase::Started, [600.0, 400.0]), T0 + 5.0);
            send(input, c, touch(1, Phase::Moved, [500.0, 430.0]), T0 + 40.0);
            send(input, c, touch(2, Phase::Moved, [600.0, 431.0]), T0 + 56.0);
            send(input, c, touch(1, Phase::Moved, [500.0, 450.0]), T0 + 72.0);
            send(input, c, touch(2, Phase::Moved, [600.0, 450.0]), T0 + 80.0);
            send(input, c, touch(1, Phase::Ended, [500.0, 450.0]), T0 + 88.0);
            send(input, c, touch(2, Phase::Ended, [600.0, 450.0]), T0 + 104.0);
        };
        let mut c = controller();
        let mut input = MapInput::default();
        steps(&mut input, &mut c);
        let v = c.view();
        assert!(v.latitude > 35.0, "dragged down: the map moves down, the view north: {v:?}");
        assert!(!c.is_dragging());
        // locked: two fingers only pinch, about the centre
        let mut c = controller();
        c.set_locked(true);
        steps(&mut input, &mut c);
        let v = c.view();
        assert!(close(v.longitude, 10.0, 1e-6) && close(v.latitude, 35.0, 1e-6) && close(v.zoom, 4.0, 0.01), "{v:?}");
        assert!(!c.is_dragging());
    }

    #[test]
    fn lifting_a_finger_of_a_pinch_goes_on_as_a_pan() {
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, touch(1, Phase::Started, [500.0, 400.0]), T0);
        send(&mut input, &mut c, touch(2, Phase::Started, [600.0, 400.0]), T0 + 5.0);
        // apart in small steps (a first step moving their centre past 10 px would make it a
        // two-finger pan)
        for k in 1..=10 {
            let t = T0 + 5.0 + 16.0 * k as f64;
            send(&mut input, &mut c, touch(1, Phase::Moved, [500.0 - 5.0 * k as f64, 400.0]), t);
            send(&mut input, &mut c, touch(2, Phase::Moved, [600.0 + 5.0 * k as f64, 400.0]), t + 8.0);
        }
        assert!(c.is_dragging());
        send(&mut input, &mut c, touch(1, Phase::Ended, [450.0, 400.0]), T0 + 200.0);
        assert!(!c.is_dragging(), "the pinch ended with its first finger");
        let pinched = c.view();
        assert!(close(pinched.zoom, 5.0, 0.02), "{pinched:?}");
        send(&mut input, &mut c, touch(2, Phase::Moved, [640.0, 400.0]), T0 + 216.0);
        send(&mut input, &mut c, touch(2, Phase::Moved, [620.0, 400.0]), T0 + 232.0);
        send(&mut input, &mut c, touch(2, Phase::Moved, [560.0, 380.0]), T0 + 248.0);
        assert!(c.is_dragging(), "the other finger pans");
        send(&mut input, &mut c, touch(2, Phase::Ended, [560.0, 380.0]), T0 + 264.0);
        let panned = c.view();
        assert!(panned.longitude > pinched.longitude && panned.latitude < pinched.latitude, "{pinched:?} {panned:?}");
        assert!(!c.is_dragging() && !input.dragging());
    }

    #[test]
    fn a_cancelled_touch_ends_the_drag() {
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, touch(1, Phase::Started, [300.0, 300.0]), T0);
        send(&mut input, &mut c, touch(1, Phase::Moved, [320.0, 300.0]), T0 + 30.0);
        send(&mut input, &mut c, touch(1, Phase::Moved, [340.0, 300.0]), T0 + 45.0);
        assert!(c.is_dragging());
        let moved = c.view();
        send(&mut input, &mut c, touch(1, Phase::Cancelled, [360.0, 300.0]), T0 + 60.0);
        assert!(!c.is_dragging() && !input.dragging(), "deck would leave its drag set; the loop would draw on");
        assert_eq!(c.view(), moved, "the cancel itself moves nothing");
    }

    #[test]
    fn a_mouse_press_during_a_touch_is_ignored() {
        // Windows sends a touch as mouse messages too: one finger must stay one pointer
        let mut c = controller();
        let mut input = MapInput::default();
        send(&mut input, &mut c, touch(1, Phase::Started, MID), T0);
        send(&mut input, &mut c, MapEvent::PointerMoved(MID), T0 + 1.0);
        send(&mut input, &mut c, press(Button::Left), T0 + 1.0);
        send(&mut input, &mut c, touch(1, Phase::Moved, [500.0, 360.0]), T0 + 16.0);
        send(&mut input, &mut c, MapEvent::PointerMoved([500.0, 360.0]), T0 + 17.0);
        send(&mut input, &mut c, touch(1, Phase::Moved, [450.0, 360.0]), T0 + 32.0);
        send(&mut input, &mut c, MapEvent::PointerMoved([450.0, 360.0]), T0 + 33.0);
        send(&mut input, &mut c, touch(1, Phase::Ended, [450.0, 360.0]), T0 + 48.0);
        send(&mut input, &mut c, release(Button::Left), T0 + 49.0);
        let v = c.view();
        // (a pan's zoom moves by the trackball's latitude term only, doc 01 §3.3)
        assert!(v.longitude > 10.0 && close(v.zoom, 4.0, 1e-3), "panned, not pinched: {v:?}");
        assert!(!input.dragging());
    }

    // -----------------------------------------------------------------------------------------
    // Keys
    // -----------------------------------------------------------------------------------------

    #[test]
    fn camera_keys_tilt_turn_north_and_zoom() {
        let mut c = controller();
        let mut input = MapInput::default();
        assert!(send(&mut input, &mut c, key('t'), T0));
        c.update(T0 + 400.0);
        assert_eq!(c.view().pitch, TILT);
        send(&mut input, &mut c, key('t'), T0 + 1000.0);
        c.update(T0 + 1400.0);
        assert_eq!(c.view().pitch, 0.0, "back from above 5°");
        // turned and tilted with the arrows (Shift: deck's modifier), then `n`
        send(&mut input, &mut c, shift(), T0 + 2000.0);
        send(&mut input, &mut c, MapEvent::Key { key: KeyName::Arrow(ArrowKey::Left), pressed: true }, T0 + 2000.0);
        c.update(T0 + 2300.0);
        send(&mut input, &mut c, MapEvent::Key { key: KeyName::Arrow(ArrowKey::Up), pressed: true }, T0 + 2400.0);
        c.update(T0 + 2700.0);
        assert!(close(c.view().bearing, -15.0, 1e-9) && close(c.view().pitch, 10.0, 1e-9), "{:?}", c.view());
        send(&mut input, &mut c, MapEvent::Modifiers(Modifiers::default()), T0 + 2800.0);
        send(&mut input, &mut c, key('n'), T0 + 3000.0);
        c.update(T0 + 3400.0);
        assert_eq!((c.view().bearing, c.view().pitch), (0.0, 0.0));
        // `+` and `=` in by one level over 250 ms, `-` out
        send(&mut input, &mut c, key('+'), T0 + 4000.0);
        c.update(T0 + 4250.0);
        assert!(close(c.view().zoom, 5.0, 1e-9));
        send(&mut input, &mut c, key('='), T0 + 5000.0);
        c.update(T0 + 5250.0);
        send(&mut input, &mut c, key('-'), T0 + 6000.0);
        c.update(T0 + 6250.0);
        assert!(close(c.view().zoom, 5.0, 1e-9), "{:?}", c.view());
        // a key let go does nothing
        assert!(!send(&mut input, &mut c, MapEvent::Key { key: KeyName::Char('t'), pressed: false }, T0 + 7000.0));
    }

    #[test]
    fn shortcuts_keep_the_page_s_rules() {
        // from a turned and tilted view, so that `n`, `t` and the zoom keys each have work to do
        let pressed = |input: &mut MapInput, modifiers: Modifiers, c: char| {
            let mut ctl = Controller::new(ViewState { bearing: 20.0, pitch: 30.0, ..start_view() }, SIZE);
            input.handle(MapEvent::Modifiers(modifiers), false, &mut ctl, T0);
            input.handle(key(c), false, &mut ctl, T0);
            ctl.in_transition()
        };
        let mut input = MapInput::default();
        let none = Modifiers::default();
        let shift = Modifiers { shift: true, ..none };
        // a letter with Shift is its own shortcut (`N`, none); Caps Lock alone is the plain letter
        assert!(!pressed(&mut input, shift, 'N'));
        assert!(!pressed(&mut input, shift, 'n'));
        assert!(pressed(&mut input, none, 'T'));
        assert!(pressed(&mut input, none, 'N'));
        // `+` is typed with Shift on many layouts
        assert!(pressed(&mut input, shift, '+'));
        // with Ctrl, Cmd or Alt held: not a shortcut
        for held in [Modifiers { ctrl: true, ..none }, Modifiers { meta: true, ..none }, Modifiers { alt: true, ..none }] {
            assert!(!pressed(&mut input, held, 't'), "{held:?}");
        }
        assert!(!pressed(&mut input, none, 'x'));
        // typing, or a dialog open: no shortcut
        input.set_key_gate(KeyGate { typing: true, ..Default::default() });
        assert!(!pressed(&mut input, none, 't'));
        input.set_key_gate(KeyGate { shortcuts_off: true, ..Default::default() });
        assert!(!pressed(&mut input, none, '+'));
        // a focused widget takes the arrows, not the letters
        input.set_key_gate(KeyGate { ui_focused: true, ..Default::default() });
        assert!(pressed(&mut input, none, 'n'));
        let mut c = controller();
        assert!(!input.handle(MapEvent::Key { key: KeyName::Arrow(ArrowKey::Right), pressed: true }, false, &mut c, T0));
        input.set_key_gate(KeyGate::default());
        // the arrows pan 100 px as deck's keyboard does
        assert!(input.handle(MapEvent::Key { key: KeyName::Arrow(ArrowKey::Right), pressed: true }, false, &mut c, T0));
        let mut deck = controller();
        deck.arrow_key(ArrowKey::Right, false, T0);
        c.update(T0 + 300.0);
        deck.update(T0 + 300.0);
        assert_eq!(c.view(), deck.view());
        assert!(c.view().longitude > 10.0);
    }

    #[test]
    fn keys_by_character_arrows_by_position() {
        let code = |k| PhysicalKey::Code(k);
        assert_eq!(key_name(&Key::Character(SmolStr::new("n")), code(KeyCode::KeyN)), KeyName::Char('n'));
        assert_eq!(key_name(&Key::Character(SmolStr::new("+")), code(KeyCode::Equal)), KeyName::Char('+'));
        assert_eq!(key_name(&Key::Named(NamedKey::ArrowLeft), code(KeyCode::ArrowLeft)), KeyName::Arrow(ArrowKey::Left));
        assert_eq!(key_name(&Key::Named(NamedKey::ArrowDown), code(KeyCode::ArrowDown)), KeyName::Arrow(ArrowKey::Down));
        // deck reads the arrows' `code`: the keypad's arrows are not them
        assert_eq!(key_name(&Key::Named(NamedKey::ArrowLeft), code(KeyCode::Numpad4)), KeyName::Other);
        assert_eq!(key_name(&Key::Named(NamedKey::Escape), code(KeyCode::Escape)), KeyName::Other);
        assert_eq!(key_name(&Key::Character(SmolStr::new("ab")), code(KeyCode::KeyA)), KeyName::Other);
    }

    // -----------------------------------------------------------------------------------------
    // Cursor, lock, winit
    // -----------------------------------------------------------------------------------------

    #[test]
    fn the_cursor_is_deck_s_over_the_map_and_egui_s_over_egui() {
        let mut c = controller();
        let mut input = MapInput::default();
        assert_eq!(input.cursor(&c, false), egui::CursorIcon::Grab);
        assert_eq!(input.cursor(&c, true), egui::CursorIcon::PointingHand);
        send(&mut input, &mut c, MapEvent::PointerMoved(MID), T0);
        send(&mut input, &mut c, press(Button::Left), T0);
        assert_eq!(input.cursor(&c, true), egui::CursorIcon::PointingHand, "not grabbing until the drag starts");
        send(&mut input, &mut c, MapEvent::PointerMoved([540.0, 360.0]), T0 + 16.0);
        assert_eq!(input.cursor(&c, true), egui::CursorIcon::Grabbing);
        send(&mut input, &mut c, release(Button::Left), T0 + 32.0);

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE[0] as f32, SIZE[1] as f32));
        let pass = |at: egui::Pos2| {
            let raw = egui::RawInput { screen_rect: Some(screen), events: vec![egui::Event::PointerMoved(at)], ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                egui::Area::new(egui::Id::unique("card")).fixed_pos(egui::pos2(10.0, 10.0)).show(ui.ctx(), |ui| {
                    ui.allocate_space(egui::vec2(200.0, 200.0));
                });
            });
            out.textures_delta.clear();
            out
        };
        // twice: an area is laid out on its first pass
        pass(egui::pos2(550.0, 360.0));
        let mut out = pass(egui::pos2(550.0, 360.0));
        input.set_cursor(&ctx, &mut out.platform_output, &c, false);
        assert_eq!(out.platform_output.cursor_icon, egui::CursorIcon::Grab);
        let mut out = pass(egui::pos2(50.0, 50.0));
        input.set_cursor(&ctx, &mut out.platform_output, &c, true);
        assert_eq!(out.platform_output.cursor_icon, egui::CursorIcon::Default, "over the card: egui's");
        // the mouse out of the window: egui's
        send(&mut input, &mut c, MapEvent::PointerLeft, T0 + 40.0);
        let mut out = pass(egui::pos2(550.0, 360.0));
        input.set_cursor(&ctx, &mut out.platform_output, &c, false);
        assert_eq!(out.platform_output.cursor_icon, egui::CursorIcon::Default);
    }

    #[test]
    fn locked_every_drag_orbits_the_wheel_zooms_about_the_centre_and_the_trackpad_does_not_pan() {
        let mut c = controller();
        c.set_locked(true);
        let mut input = MapInput::default();
        drag(&mut input, &mut c, Button::Left, [550.0, 360.0], [650.0, 300.0]);
        let v = c.view();
        assert!(v.bearing != 0.0 && v.pitch > 0.0, "{v:?}");
        assert_eq!((v.longitude, v.latitude), (10.0, 35.0));
        // the wheel at a corner zooms about the centre
        let mut c = controller();
        c.set_locked(true);
        send(&mut input, &mut c, MapEvent::PointerMoved([100.0, 100.0]), T0);
        let notch = MapEvent::Wheel { delta_x: -0.0, delta_y: -100.0, mode: DeltaMode::Pixel, wheel_delta_y: Some(120.0) };
        send(&mut input, &mut c, notch, T0);
        let v = c.view();
        assert!(v.zoom > 4.5 && close(v.longitude, 10.0, 1e-6) && close(v.latitude, 35.0, 1e-6), "{v:?}");
        // a trackpad's scroll is swallowed
        let scroll = MapEvent::Wheel { delta_x: 3.5, delta_y: 12.25, mode: DeltaMode::Pixel, wheel_delta_y: Some(-36.75) };
        send(&mut input, &mut c, scroll, T0 + 16.0);
        assert_eq!(c.view(), v);
    }

    #[test]
    fn winit_events_in_css_pixels() {
        let moved = WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(300.0, 120.0) };
        assert_eq!(map_event(&moved, 2.0), Some(MapEvent::PointerMoved([150.0, 60.0])));
        let pixels = WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(10.0, -40.0)),
            phase: TouchPhase::Moved,
        };
        assert_eq!(
            map_event(&pixels, 2.0),
            Some(MapEvent::Wheel { delta_x: -5.0, delta_y: 20.0, mode: DeltaMode::Pixel, wheel_delta_y: Some(-60.0) }),
            "WebKit's legacy delta: −3 × the pixels"
        );
        let lines = WindowEvent::MouseWheel { device_id: DeviceId::dummy(), delta: MouseScrollDelta::LineDelta(0.5, -2.0), phase: TouchPhase::Moved };
        assert_eq!(
            map_event(&lines, 2.0),
            Some(MapEvent::Wheel { delta_x: -50.0, delta_y: 200.0, mode: DeltaMode::Pixel, wheel_delta_y: Some(-240.0) }),
            "WebView2's 100 px and 120 a notch, whatever the scale factor"
        );
        let press = WindowEvent::MouseInput { device_id: DeviceId::dummy(), state: ElementState::Pressed, button: MouseButton::Right };
        assert_eq!(map_event(&press, 1.0), Some(MapEvent::Button { button: Button::Right, pressed: true }));
        let back = WindowEvent::MouseInput { device_id: DeviceId::dummy(), state: ElementState::Released, button: MouseButton::Back };
        assert_eq!(map_event(&back, 1.0), None);
        assert_eq!(map_event(&WindowEvent::Focused(false), 1.0), Some(MapEvent::FocusLost));
        assert_eq!(map_event(&WindowEvent::Focused(true), 1.0), None);
        let finger = WindowEvent::Touch(Touch {
            device_id: DeviceId::dummy(),
            phase: TouchPhase::Cancelled,
            location: PhysicalPosition::new(300.0, 120.0),
            force: None,
            id: 7,
        });
        assert_eq!(map_event(&finger, 2.0), Some(MapEvent::Touch { id: 7, phase: Phase::Cancelled, at: [150.0, 60.0] }));
        let pinch = WindowEvent::PinchGesture { device_id: DeviceId::dummy(), delta: 0.25, phase: TouchPhase::Started };
        assert_eq!(map_event(&pinch, 2.0), Some(MapEvent::Pinch { delta: 0.25, phase: Phase::Started }));
        let twist = WindowEvent::RotationGesture { device_id: DeviceId::dummy(), delta: 12.5, phase: TouchPhase::Ended };
        assert_eq!(map_event(&twist, 2.0), Some(MapEvent::Rotate { delta: 12.5, phase: Phase::Ended }));
    }
}
