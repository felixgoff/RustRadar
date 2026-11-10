//! Input between egui and the map (plan 1.5, 2.2): every window event goes to egui first; the map
//! takes a gesture only when egui does not want the pointer.
//!
//! The map's side reads [`MapEvent`]s, which carry nothing of winit (CSS pixels from the canvas's top
//! left, deltas with the DOM's sign), so it can be driven by tests; [`map_event`] makes them from
//! winit's events. [`MapInput`] turns them into the controller's gestures (doc 01 §3.2):
//!
//! - a left (or middle) drag pans; a right drag, or a drag with Ctrl, Shift, Alt or Meta held,
//!   rotates; every drag orbits when the camera is locked (the controller decides);
//! - the wheel zooms about the pointer, a trackpad's two-finger scroll pans and Ctrl + scroll (a
//!   pinch outside WebKit) zooms ([`Controller::wheel`]).
//!
//! A gesture the map started is the map's to the end: moves and the release reach it even over an
//! egui area, as `egui_wants_pointer_input` already says false then. Item B8 completes the gesture
//! table: double-click, pinch and rotation gestures, touch, keys and the cursor.

use rustradar_scene::controller::{Controller, DeltaMode, WheelInput};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};

/// A mouse button, as the map tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
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

/// An input event in the map's terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapEvent {
    /// The pointer is at this point (CSS pixels from the canvas's top left).
    PointerMoved([f64; 2]),
    /// The pointer left the window.
    PointerLeft,
    Button { button: Button, pressed: bool },
    /// A wheel or trackpad scroll, deltas positive to the right and down (the DOM's sign).
    Wheel { delta_x: f64, delta_y: f64, mode: DeltaMode },
    Modifiers(Modifiers),
    /// The window lost the keyboard focus: a drag in progress ends (its release goes elsewhere).
    FocusLost,
}

/// The map's input state: where the pointer is, the modifiers, the drag the map holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapInput {
    pointer: Option<[f64; 2]>,
    modifiers: Modifiers,
    /// The button whose drag the controller took.
    drag: Option<Button>,
}

impl MapInput {
    /// Where the pointer is, if it is over the window.
    pub fn pointer(&self) -> Option<[f64; 2]> {
        self.pointer
    }

    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Whether the map holds a drag.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Feeds one event at `now_ms` (local clock). `egui_wants_pointer` is egui's
    /// `egui_wants_pointer_input()` from its last pass: then a press or a wheel is egui's, not the
    /// map's. Returns whether the camera may have moved (draw a frame).
    pub fn handle(&mut self, event: MapEvent, egui_wants_pointer: bool, controller: &mut Controller, now_ms: f64) -> bool {
        match event {
            MapEvent::PointerMoved(at) => {
                self.pointer = Some(at);
                self.drag.is_some() && controller.pan_move(at)
            }
            // a drag goes on outside the window (the platform keeps sending its moves)
            MapEvent::PointerLeft => {
                self.pointer = None;
                false
            }
            MapEvent::Button { button, pressed: true } => {
                let Some(at) = self.pointer else { return false };
                if egui_wants_pointer || self.drag.is_some() {
                    return false;
                }
                let took = controller.pan_start(at, button == Button::Right, self.modifiers.func_key(), now_ms);
                if took {
                    self.drag = Some(button);
                }
                took
            }
            MapEvent::Button { button, pressed: false } => {
                if self.drag != Some(button) {
                    return false;
                }
                self.drag = None;
                controller.pan_end()
            }
            MapEvent::Wheel { delta_x, delta_y, mode } => {
                let Some(at) = self.pointer else { return false };
                if egui_wants_pointer {
                    return false;
                }
                let wheel = WheelInput {
                    delta_x,
                    delta_y,
                    delta_mode: mode,
                    ctrl_key: self.modifiers.ctrl,
                    shift_key: self.modifiers.shift,
                    wheel_delta_y: None,
                };
                controller.wheel(&wheel, at, now_ms)
            }
            MapEvent::Modifiers(modifiers) => {
                self.modifiers = modifiers;
                false
            }
            MapEvent::FocusLost => {
                if self.drag.take().is_some() {
                    return controller.pan_end();
                }
                false
            }
        }
    }
}

/// The map's share of a winit event on a window of `scale_factor` device pixels per CSS pixel, if
/// it has one.
///
/// winit's scroll deltas are positive when the content should move right and down, the DOM's when
/// the view scrolls right and down: their signs are turned. A line delta stays in lines (a mouse
/// wheel's notches), a pixel delta (a trackpad, some mice) becomes CSS pixels.
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
            MouseScrollDelta::LineDelta(x, y) => MapEvent::Wheel { delta_x: -f64::from(*x), delta_y: -f64::from(*y), mode: DeltaMode::Line },
            MouseScrollDelta::PixelDelta(p) => MapEvent::Wheel { delta_x: -css(p.x), delta_y: -css(p.y), mode: DeltaMode::Pixel },
        },
        WindowEvent::ModifiersChanged(m) => {
            let s = m.state();
            MapEvent::Modifiers(Modifiers { shift: s.shift_key(), ctrl: s.control_key(), alt: s.alt_key(), meta: s.super_key() })
        }
        WindowEvent::Focused(false) => MapEvent::FocusLost,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use rustradar_scene::camera::ViewState;
    use winit::dpi::PhysicalPosition;
    use winit::event::DeviceId;

    use super::*;

    const SIZE: [f64; 2] = [1100.0, 720.0];

    fn controller() -> Controller {
        Controller::new(ViewState { longitude: 10.0, latitude: 35.0, zoom: 4.0, bearing: 0.0, pitch: 0.0, position: None }, SIZE)
    }

    fn drag(input: &mut MapInput, c: &mut Controller, button: Button, from: [f64; 2], to: [f64; 2]) {
        input.handle(MapEvent::PointerMoved(from), false, c, 1000.0);
        input.handle(MapEvent::Button { button, pressed: true }, false, c, 1000.0);
        for i in 1..=10 {
            let t = i as f64 / 10.0;
            let at = [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t];
            input.handle(MapEvent::PointerMoved(at), false, c, 1000.0 + 16.0 * i as f64);
        }
        input.handle(MapEvent::Button { button, pressed: false }, false, c, 1200.0);
    }

    #[test]
    fn a_left_drag_pans_and_a_right_drag_rotates() {
        let mut c = controller();
        let mut input = MapInput::default();
        let before = c.view();
        drag(&mut input, &mut c, Button::Left, [550.0, 360.0], [450.0, 360.0]);
        let panned = c.view();
        assert!(panned.longitude > before.longitude, "dragging left moves the map east: {panned:?}");
        assert_eq!(panned.bearing, before.bearing);
        assert!(!input.dragging());

        let mut c = controller();
        drag(&mut input, &mut c, Button::Right, [550.0, 360.0], [650.0, 300.0]);
        let turned = c.view();
        assert_ne!(turned.bearing, 0.0, "{turned:?}");
        assert!(turned.pitch > 0.0, "dragging up tilts: {turned:?}");
        assert_eq!((turned.longitude, turned.latitude), (before.longitude, before.latitude));
    }

    #[test]
    fn a_modifier_makes_a_left_drag_rotate() {
        let mut c = controller();
        let mut input = MapInput::default();
        input.handle(MapEvent::Modifiers(Modifiers { shift: true, ..Default::default() }), false, &mut c, 0.0);
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
        assert!(!input.handle(MapEvent::Button { button: Button::Left, pressed: true }, true, &mut c, 0.0));
        assert!(!input.dragging());
        assert!(!input.handle(MapEvent::PointerMoved([300.0, 100.0]), false, &mut c, 16.0));
        assert!(!input.handle(MapEvent::Wheel { delta_x: 0.0, delta_y: -3.0, mode: DeltaMode::Line }, true, &mut c, 32.0));
        assert_eq!(c.view(), controller().view());
    }

    #[test]
    fn a_drag_the_map_started_stays_the_maps_over_egui() {
        let mut c = controller();
        let mut input = MapInput::default();
        input.handle(MapEvent::PointerMoved([550.0, 360.0]), false, &mut c, 0.0);
        assert!(input.handle(MapEvent::Button { button: Button::Left, pressed: true }, false, &mut c, 0.0));
        // over a panel now; the release still ends the map's drag
        assert!(input.handle(MapEvent::PointerMoved([200.0, 360.0]), true, &mut c, 16.0));
        // another button meanwhile is ignored
        assert!(!input.handle(MapEvent::Button { button: Button::Right, pressed: true }, true, &mut c, 20.0));
        input.handle(MapEvent::Button { button: Button::Left, pressed: false }, true, &mut c, 32.0);
        assert!(!input.dragging());
        assert!(!c.is_dragging());
        assert!(c.view().longitude > 10.0);
    }

    #[test]
    fn losing_the_focus_ends_a_drag() {
        let mut c = controller();
        let mut input = MapInput::default();
        input.handle(MapEvent::PointerMoved([550.0, 360.0]), false, &mut c, 0.0);
        input.handle(MapEvent::Button { button: Button::Left, pressed: true }, false, &mut c, 0.0);
        input.handle(MapEvent::PointerMoved([500.0, 360.0]), false, &mut c, 16.0);
        assert!(c.is_dragging());
        input.handle(MapEvent::FocusLost, false, &mut c, 20.0);
        assert!(!input.dragging() && !c.is_dragging());
        let after = c.view();
        assert!(!input.handle(MapEvent::PointerMoved([300.0, 360.0]), false, &mut c, 40.0));
        assert_eq!(c.view(), after);
    }

    #[test]
    fn the_wheel_zooms_about_the_pointer() {
        let mut c = controller();
        let mut input = MapInput::default();
        // no pointer yet: nowhere to zoom about
        assert!(!input.handle(MapEvent::Wheel { delta_x: 0.0, delta_y: -1.0, mode: DeltaMode::Line }, false, &mut c, 0.0));
        input.handle(MapEvent::PointerMoved([550.0, 360.0]), false, &mut c, 0.0);
        // one notch up, as winit gives it (LineDelta(0, 1)) and turned to the DOM's sign
        let notch = WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::LineDelta(0.0, 1.0),
            phase: winit::event::TouchPhase::Moved,
        };
        let event = map_event(&notch, 2.0).expect("a wheel event");
        assert_eq!(event, MapEvent::Wheel { delta_x: -0.0, delta_y: -1.0, mode: DeltaMode::Line });
        assert!(input.handle(event, false, &mut c, 16.0));
        assert!(c.view().zoom > 4.0, "{:?}", c.view());
    }

    #[test]
    fn winit_events_in_css_pixels() {
        let moved = WindowEvent::CursorMoved { device_id: DeviceId::dummy(), position: PhysicalPosition::new(300.0, 120.0) };
        assert_eq!(map_event(&moved, 2.0), Some(MapEvent::PointerMoved([150.0, 60.0])));
        let pixels = WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(10.0, -40.0)),
            phase: winit::event::TouchPhase::Moved,
        };
        assert_eq!(map_event(&pixels, 2.0), Some(MapEvent::Wheel { delta_x: -5.0, delta_y: 20.0, mode: DeltaMode::Pixel }));
        let press = WindowEvent::MouseInput { device_id: DeviceId::dummy(), state: ElementState::Pressed, button: MouseButton::Right };
        assert_eq!(map_event(&press, 1.0), Some(MapEvent::Button { button: Button::Right, pressed: true }));
        let back = WindowEvent::MouseInput { device_id: DeviceId::dummy(), state: ElementState::Released, button: MouseButton::Back };
        assert_eq!(map_event(&back, 1.0), None);
        assert_eq!(map_event(&WindowEvent::Focused(false), 1.0), Some(MapEvent::FocusLost));
        assert_eq!(map_event(&WindowEvent::Focused(true), 1.0), None);
    }
}
