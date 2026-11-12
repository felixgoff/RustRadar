//! The camera's own work each frame, after the transition step: the lock on an aircraft (item
//! A10; doc 01 §2.4, §3.9, doc 03 §11.3–11.4), `renderFrame`'s steps 3, 4 and 6 (G:3171-3223):
//!
//! 3. **Which controller**: locked on an aircraft, a train or a ship, the wheel and pinches zoom
//!    about the centre and every drag orbits ([`Controller::set_locked`], `controllerFor`).
//! 4. **The lock**: while the camera follows the selected aircraft (`follow && selected`) and no
//!    move of the app's runs (`Date.now() > transitionUntil`), the view is re-aimed at the pose
//!    the aircraft is drawn at ([`locked_view`]) and handed over with the follow limits (pitch up
//!    to 85, closer than zoom 20 until the camera is 1.3 aircraft lengths away). On the flat map
//!    that raises the camera's target (`position`); when such a lock ends the target comes back
//!    to the ground with the plain limits.
//! 6. **The ease back**: a lock that ends closer than zoom 20 or steeper than 60° eases back
//!    within them over 600 ms. deck holds each frame of that transition within the free limits
//!    it is handed with it, so the view is back at its first frame, and the 600 ms only keep the
//!    app's other moves (a new lock, the terrain aim) off, as today.
//!
//! Step 2 (centring on a followed train or ship) is [`Camera::follow_vehicle`], which the frame
//! runs before the transition step with where the scene builders last drew the vehicle; step 5
//! (the camera's aim easing to the ground's height under it, G:3204-3212) belongs to item Z4,
//! between steps 4 and 6.
//!
//! The scene is rebuilt every frame while locked close in (zoom above 7, no move of the app's
//! running): [`super::AppState::locked_steady`] reads the controller's mode this step sets, and
//! [`crate::scene_builder::Pacer`] refreshes on it (doc 01 §2.6).
//!
//! **What it is handed.** The page's `following` (the camera follows the selected aircraft: the
//! flight panel's Follow, the key C, `select(…, { follow })`, cleared when the follow stream ends
//! or another place, train or ship is picked) and its `selected` row (the selected flight with
//! the follow stream's report in place) live outside this file: their owners hand them over with
//! [`Camera::set_follow`] and [`Camera::set_selected`] whenever they change, before
//! [`super::AppState::advance_camera`] runs.
//!
//! Deviations from today's app:
//! - Today a gesture is re-aimed inside `onViewStateChange`, after each event; here the next
//!   frame's step re-aims it, before anything is drawn with it. Several events between two frames
//!   are re-aimed once, which gives the same view (the re-aim of a drag, a centre zoom or a pinch
//!   does not depend on the one before).
//! - Today the lock re-aims at each rendered frame, every 150 or 500 ms at zoom 7 and below; here
//!   at each frame drawn, which at those zooms is the scene refresh's (the loop sleeps until it)
//!   unless a gesture, a transition or an animation draws more often.
//! - When the lock ends, today's app hands deck no new limits unless the target was raised (plain
//!   `LIMITS`) or the view eases back (`limits()`), so deck keeps the locked ones (up to 85° and
//!   zoom 23) until the app's next own move re-sends `limits()`. The port's own moves do not
//!   re-send them, so the free camera's limits are handed over at once when the lock ends.

use rustradar_core::geo::Basemap;
use rustradar_core::model::{LiveFlight, f32_as_js};
use rustradar_map::layers::aircraft::assets;
use rustradar_scene::aircraft_models::size_of;
use rustradar_scene::camera::{self as cam, FOLLOW_MAX_PITCH, FollowLock, LIMITS, Limits, ViewState};
use rustradar_scene::controller::{EASE_BACK_MS, locked_view, released_ease_back};

#[cfg(doc)]
use rustradar_scene::controller::Controller;

use super::Cx;

/// The camera lock on an aircraft (and, from item Z4, the ground height under the camera).
#[derive(Debug)]
pub struct Camera {
    /// The camera follows the selected aircraft (the page's `following`, the globe's `follow`).
    follow: bool,
    /// The selected flight's row (the page's `selected`).
    selected: Option<LiveFlight>,
    /// Following a train or ship, once the camera has flown there (`vehicleLocked`).
    vehicle_locked: bool,
    /// The page's `followingVehicle` as [`Self::follow_vehicle`] last saw it (its effect runs
    /// when it changes).
    vehicle_wanted: bool,
    /// How far the locked camera may tilt right now (`lockedMaxPitch`), as the last re-aim found.
    locked_max_pitch: f64,
    /// The last step was locked on an aircraft (`wasLocked`), to notice the lock ending.
    was_locked: bool,
    /// The locked camera's target is raised to the aircraft (`raised`: the flat map).
    raised: bool,
    /// `follow` at the last step, to rebuild the scene when it changes.
    follow_seen: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            follow: false,
            selected: None,
            vehicle_locked: false,
            vehicle_wanted: false,
            locked_max_pitch: FOLLOW_MAX_PITCH,
            was_locked: false,
            raised: false,
            follow_seen: false,
        }
    }
}

/// The aircraft's largest dimension, metres: its published model's size, else its icon class's
/// (`aircraftModels.forFlight(typecode, icon)?.size ?? sizeOf(icon)`).
pub fn size_m(flight: &LiveFlight) -> f64 {
    assets::published().and_then(|m| m.for_flight(&flight.typecode, &flight.icon)).map_or_else(|| size_of(&flight.icon), |m| m.size)
}

impl Camera {
    /// Whether the camera follows the selected aircraft (`follow`).
    pub fn follow(&self) -> bool {
        self.follow
    }

    /// The camera follows the selected aircraft, or lets go of it (the page's `following`). It
    /// locks only while a flight is selected too.
    pub fn set_follow(&mut self, follow: bool) {
        self.follow = follow;
    }

    /// The selected flight's row, as handed in.
    pub fn selected(&self) -> Option<&LiveFlight> {
        self.selected.as_ref()
    }

    /// The selected flight's row (the page's `selected`: the follow stream's report in place), or
    /// none. Handed in each time it changes; the lock poses it with the motion model, which also
    /// keeps its newer reports.
    pub fn set_selected(&mut self, flight: Option<LiveFlight>) {
        self.selected = flight;
    }

    /// Following a train or ship once the camera has flown there (`vehicleLocked`, items T1 and
    /// H1): the controller is then the locked one too.
    pub fn set_vehicle_locked(&mut self, locked: bool) {
        self.vehicle_locked = locked;
    }

    /// Whether a train or ship is followed ([`Self::set_vehicle_locked`]).
    pub fn vehicle_locked(&self) -> bool {
        self.vehicle_locked
    }

    /// Following a train or ship (`followVehicle`, G:3141-3177), before the transition step:
    /// `wanted` is the page's `followingVehicle` (the key C or the panel's Follow with a train or
    /// ship selected), `at` is `vehicleAt()`, where the followed train or ship is drawn now (none
    /// while the camera follows an aircraft).
    ///
    /// - The effect, when `wanted` changes: turned on, the camera flies to `at` over 800 ms, at
    ///   least to zoom 13 ([`rustradar_scene::controller::Controller::fly_to_vehicle`]), and is
    ///   locked on the vehicle from then; turned off, it lets go. Turned on with nothing to fly
    ///   to, nothing happens until it is turned on again, as today.
    /// - `renderFrame`'s step 2: while locked and no move of the app's runs, the camera is kept
    ///   on the vehicle (its centre set to `at`), which also brings a drag back to it at the next
    ///   frame (today's `onViewStateChange` does it at the gesture's event).
    pub fn follow_vehicle(&mut self, wanted: bool, at: Option<[f64; 2]>, controller: &mut rustradar_scene::controller::Controller, now_ms: f64) {
        if wanted != self.vehicle_wanted {
            self.vehicle_wanted = wanted;
            if !wanted {
                self.vehicle_locked = false;
            } else if let Some([longitude, latitude]) = at {
                controller.fly_to_vehicle(longitude, latitude, now_ms);
                self.vehicle_locked = true;
            }
        }
        if self.vehicle_locked
            && now_ms > controller.transition_until()
            && let Some([longitude, latitude]) = at
        {
            let view = controller.view();
            if view.longitude != longitude || view.latitude != latitude {
                controller.set_view(ViewState { longitude, latitude, ..view });
            }
        }
    }

    /// The flight the camera is locked on (`follow && selected`).
    pub fn locked_flight(&self) -> Option<&LiveFlight> {
        self.selected.as_ref().filter(|_| self.follow)
    }

    /// Whether the camera is locked on an aircraft (`follow && selected`).
    pub fn following(&self) -> bool {
        self.locked_flight().is_some()
    }

    /// How far the locked camera may tilt now (`lockedMaxPitch`): 85°, or less on the globe so
    /// the camera stays above the aircraft. It keeps its last value once the lock ends.
    pub fn locked_max_pitch(&self) -> f64 {
        self.locked_max_pitch
    }

    /// Whether the locked camera's target is raised to the aircraft (the flat map's lock).
    pub fn raised(&self) -> bool {
        self.raised
    }

    /// What the follow limits read of the locked flight on a map `height_px` CSS pixels high.
    fn follow_lock(&self, height_px: f64) -> Option<FollowLock> {
        let flight = self.locked_flight()?;
        let latitude = f32_as_js(flight.lat);
        Some(FollowLock { latitude, max_zoom: cam::follow_max_zoom(size_m(flight), latitude, Some(height_px)), max_pitch: self.locked_max_pitch })
    }

    /// The camera's limits now (`limits()`, G:294-300) on `basemap` and a map `height_px` CSS
    /// pixels high: the follow limits while locked on an aircraft, else the basemap's.
    pub fn limits(&self, basemap: Basemap, height_px: f64) -> Limits {
        cam::limits(basemap, self.follow_lock(height_px).as_ref())
    }

    /// The closest zoom reachable now (`maxZoomNow()`, G:302): the locked flight's
    /// [`cam::follow_max_zoom`], else 20.
    pub fn max_zoom_now(&self, height_px: f64) -> f64 {
        cam::max_zoom_now(self.follow_lock(height_px).map(|lock| lock.max_zoom))
    }

    /// Each frame after the controller's transition step ([`super::AppState::advance_camera`]):
    /// `renderFrame`'s steps 3, 4 and 6 (see the module documentation).
    pub fn poll(&mut self, cx: &mut Cx<'_>) {
        let now = cx.clocks.local_ms;
        let (basemap, exaggeration) = (cx.persisted.settings.basemap, cx.persisted.settings.exaggeration);
        let height = cx.controller.size()[1];
        let locked = self.following();
        // the globe's props changed (`follow`, the lock): render now (G:3771-3777)
        if self.follow != self.follow_seen || locked != self.was_locked {
            cx.wants.scene = true;
        }
        self.follow_seen = self.follow;

        // 3: locked on an aircraft, the wheel has to zoom about the centre, or every notch shoves
        // the aircraft aside and the lock drags it straight back; a drag orbits for the same reason
        let wanted = locked || self.vehicle_locked;
        if wanted != cx.controller.locked() {
            cx.controller.set_locked(wanted);
        }

        // 4: re-aimed at the aircraft as it is drawn now, with the follow limits
        let aimed = match self.locked_flight() {
            Some(flight) if now > cx.controller.transition_until() => {
                let pose = cx.motion.pose(flight, cx.clocks.feed_ms);
                Some(locked_view(&cx.controller.view(), &pose, exaggeration, height))
            }
            _ => None,
        };
        if let Some(aimed) = aimed {
            self.locked_max_pitch = aimed.max_pitch;
            self.raised = aimed.view.position.is_some();
            cx.controller.set_limits(self.limits(basemap, height), self.max_zoom_now(height));
            cx.controller.set_view(aimed.view);
        } else if self.raised && !locked {
            // the lock was released with the camera raised: bring it back to the ground (deck is
            // handed `position: [0, 0, 0]` with the plain LIMITS)
            self.raised = false;
            cx.controller.set_limits(LIMITS, cam::max_zoom_now(None));
            cx.controller.set_view(ViewState { position: Some([0.0, 0.0, 0.0]), ..cx.controller.view() });
        }

        // 5: the terrain aim (item Z4) goes here

        // 6: released from a lock closer or flatter than the free camera allows: ease back within
        // its limits. deck starts that transition from the view under the limits it held, and
        // runs it, from its first frame, under the free ones it is handed with it (`goTo`'s
        // `...limits()`), which hold every frame within them: the ease is a snap, as today. The
        // free limits are handed over whether it eases or not (see the module documentation).
        if self.was_locked && !locked {
            let back = released_ease_back(&cx.controller.view());
            if let Some(back) = back {
                cx.controller.go_to(back, EASE_BACK_MS, now);
            }
            cx.controller.set_limits(self.limits(basemap, height), self.max_zoom_now(height));
            if back.is_some() {
                cx.controller.update(now);
            }
        }
        self.was_locked = locked;
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::{effective_exaggeration, elevation};
    use rustradar_core::motion::aircraft::{Motion, Pose};
    use rustradar_scene::camera::{REALISTIC_LIMITS, viewport_for};
    use rustradar_scene::controller::ZOOM_BY_MS;

    use super::*;
    use crate::scene_builder::refresh_interval_ms;
    use crate::state::{AppState, Startup};

    /// The tests' canvas, CSS pixels.
    const SIZE: [f64; 2] = [1100.0, 720.0];
    /// The local clock at the first frame (2026-06-21 10:00 UTC).
    const T0: f64 = 1_782_036_000_000.0;

    fn view(longitude: f64, latitude: f64, zoom: f64, bearing: f64, pitch: f64) -> ViewState {
        ViewState { longitude, latitude, zoom, bearing, pitch, position: None }
    }

    /// An A320 near Arlanda, `alt` ft up at 250 kt heading east, reported at `at_ms`.
    fn flight(alt: i32, at_ms: f64) -> LiveFlight {
        LiveFlight {
            id: 42,
            lat: 59.7,
            lon: 18.1,
            track: 90,
            alt,
            speed: 250,
            timestamp_ms: at_ms as u64,
            callsign: "SAS123".into(),
            typecode: "A320".into(),
            icon: "A320".into(),
            ..Default::default()
        }
    }

    fn state(v: ViewState) -> AppState {
        AppState::new(Startup::default(), None, None, None, v, SIZE)
    }

    /// The pose the aircraft is drawn at in the frame at `local_ms`, and where that is on screen
    /// in the controller's view, px from the centre.
    fn drawn(s: &mut AppState, local_ms: f64) -> (Pose, f64) {
        let f = s.camera.selected().cloned().expect("a selected flight");
        let feed = s.clocks(local_ms).feed_ms;
        let pose = s.motion.pose(&f, feed);
        let v = s.controller.view();
        let altitude = elevation(pose.alt_ft, effective_exaggeration(s.persisted.settings.exaggeration, v.zoom));
        let [x, y, _] = viewport_for(&v, s.controller.size()).project([pose.lon, pose.lat, altitude]);
        (pose, (x - SIZE[0] / 2.0).hypot(y - SIZE[1] / 2.0))
    }

    #[test]
    fn nothing_locks_without_both_a_selection_and_follow() {
        let v = view(18.0, 59.6, 9.0, 20.0, 40.0);
        let mut s = state(v);
        s.camera.set_follow(true);
        s.advance_camera(T0);
        assert!(!s.camera.following() && !s.controller.locked());
        assert_eq!(s.controller.view(), v, "no aircraft to aim at");
        s.camera.set_follow(false);
        s.camera.set_selected(Some(flight(8000, T0)));
        s.advance_camera(T0 + 16.0);
        assert!(!s.camera.following() && !s.controller.locked());
        assert_eq!(s.controller.view(), v, "selected, not followed");
        assert_eq!(s.controller.limits(), LIMITS);
    }

    /// Locks on `f` from `v` at `t`: the first frame of the lock.
    fn locked_on(v: ViewState, f: LiveFlight, t: f64) -> AppState {
        let mut s = state(v);
        s.camera.set_selected(Some(f));
        s.camera.set_follow(true);
        let (_, wants) = s.advance_camera(t);
        assert!(wants.scene, "the lock starting rebuilds the scene");
        assert!(s.camera.following() && s.controller.locked());
        s
    }

    #[test]
    fn on_the_globe_the_aim_is_pushed_ahead_with_the_follow_limits() {
        let v = view(18.0, 59.6, 9.0, 20.0, 40.0);
        let f = flight(8000, T0);
        let mut s = locked_on(v, f.clone(), T0);
        // the view is lockedView's of the pose the aircraft is drawn at
        let pose = Motion::new().pose(&f, T0);
        let want = locked_view(&v, &pose, s.persisted.settings.exaggeration, SIZE[1]);
        assert_eq!(s.controller.view(), want.view);
        assert_eq!(s.camera.locked_max_pitch(), want.max_pitch);
        assert!(!s.camera.raised() && s.controller.view().position.is_none());
        let (_, off) = drawn(&mut s, T0);
        assert!(off < 2.0, "{off} px off the centre");
        // the follow limits: up to the pitch that keeps the camera above it, and as close as 1.3
        // of its lengths, with deck's latitude shift undone
        let lat = f32_as_js(f.lat);
        let max_zoom = cam::follow_max_zoom(size_m(&f), lat, Some(SIZE[1]));
        assert!((20.0..=23.0).contains(&max_zoom), "{max_zoom}");
        let shift = (lat * std::f64::consts::PI / 180.0).cos().log2();
        let limits = s.controller.limits();
        assert_eq!(limits.min_zoom, 0.5);
        assert_eq!(limits.max_pitch, want.max_pitch);
        assert!((limits.max_zoom - (max_zoom - shift)).abs() < 1e-12, "{limits:?}");
        assert_eq!(s.controller.max_zoom_now(), max_zoom);
        assert_eq!(s.camera.limits(Basemap::Dark, SIZE[1]), limits);
        assert_eq!(s.camera.max_zoom_now(SIZE[1]), max_zoom);
    }

    #[test]
    fn on_the_flat_map_the_target_is_raised_to_the_aircraft() {
        let v = view(18.0, 59.6, 14.0, 300.0, 70.0);
        let f = flight(3000, T0);
        let mut s = locked_on(v, f.clone(), T0);
        let pose = Motion::new().pose(&f, T0);
        let w = s.controller.view();
        assert_eq!((w.longitude, w.latitude, w.zoom, w.bearing, w.pitch), (pose.lon, pose.lat, 14.0, 300.0, 70.0));
        assert_eq!(w.position, Some([0.0, 0.0, elevation(pose.alt_ft, 1.0)]), "true scale at zoom 14");
        assert!(s.camera.raised());
        assert_eq!(s.controller.limits().max_pitch, FOLLOW_MAX_PITCH);
        let (_, off) = drawn(&mut s, T0);
        assert!(off < 1e-6, "{off} px off the centre");
    }

    /// Climbing out at 3 000 ft/min from 3 000 ft, a report every 8 s as the feed brings them,
    /// reported 2 s before it arrives.
    fn climbing(t: f64) -> LiveFlight {
        let n = ((t - T0) / 8000.0).floor();
        let at = T0 + n * 8000.0 - 2000.0;
        LiveFlight { vspeed: Some(3000), positions: Vec::new(), ..flight(3000 + (n as i32) * 400, at) }
    }

    #[test]
    fn a_climbing_aircraft_stays_at_the_centre_every_frame() {
        for (v, within) in [(view(18.0, 59.6, 9.0, 30.0, 45.0), 2.0), (view(18.0, 59.6, 11.5, 250.0, 55.0), 2.0), (view(18.0, 59.6, 14.0, 120.0, 65.0), 1e-6)] {
            let mut s = locked_on(v, climbing(T0), T0);
            let mut first = None;
            let mut last = 0.0;
            let mut worst: f64 = 0.0;
            // 20 s at 60 frames a second
            for frame in 0..1200 {
                let t = T0 + f64::from(frame) * (1000.0 / 60.0);
                s.camera.set_selected(Some(climbing(t)));
                s.advance_camera(t);
                assert!(s.locked_steady(t) && refresh_interval_ms(s.controller.view().zoom, true) == 0.0, "locked close in: every frame");
                let (pose, off) = drawn(&mut s, t);
                first.get_or_insert(pose.alt_ft);
                last = pose.alt_ft;
                worst = worst.max(off);
            }
            eprintln!("zoom {}: climbed {:.0} ft to {last:.0} ft, at most {worst:.4} px off the centre", v.zoom, last - first.unwrap());
            assert!(last > first.unwrap() + 500.0, "it climbs: {first:?} → {last}");
            assert!(worst < within, "zoom {}: {worst} px off the centre", v.zoom);
        }
    }

    #[test]
    fn a_gesture_while_locked_is_re_aimed_before_it_is_drawn() {
        let mut s = locked_on(view(18.0, 59.6, 10.0, 0.0, 30.0), flight(6000, T0), T0);
        // the wheel at a corner zooms about the centre, the drag orbits
        assert!(s.controller.wheel_zoom([50.0, 40.0], 120.0));
        assert!(s.controller.pan_start([600.0, 400.0], false, false, T0 + 10.0));
        assert!(s.controller.pan_move([700.0, 380.0]));
        s.advance_camera(T0 + 16.0);
        let v = s.controller.view();
        assert!(v.zoom > 10.5 && v.bearing > 10.0 && v.pitch > 30.0, "zoomed, turned and tilted: {v:?}");
        let (_, off) = drawn(&mut s, T0 + 16.0);
        assert!(off < 2.0, "{off} px off the centre");
        assert!(s.controller.pan_end());
    }

    #[test]
    fn the_lock_waits_for_the_apps_own_moves() {
        let mut s = locked_on(view(18.0, 59.6, 10.0, 0.0, 30.0), flight(6000, T0), T0);
        let t = T0 + 100.0;
        s.controller.zoom_by(1.0, t);
        assert!(!s.locked_steady(t + 50.0), "a move of the app's runs");
        // during it the view is the transition's own
        let mut alone = s.controller.clone();
        alone.update(t + 100.0);
        s.advance_camera(t + 100.0);
        assert_eq!(s.controller.view(), alone.view());
        // and once it is over the lock takes it again
        s.advance_camera(t + ZOOM_BY_MS + 1.0);
        assert!(s.locked_steady(t + ZOOM_BY_MS + 1.0));
        assert!((s.controller.view().zoom - 11.0).abs() < 1e-9);
        let (_, off) = drawn(&mut s, t + ZOOM_BY_MS + 1.0);
        assert!(off < 2.0, "{off} px off the centre");
    }

    #[test]
    fn a_raised_lock_comes_back_to_the_ground_and_back_within_the_free_limits() {
        let mut s = locked_on(view(18.0, 59.6, 14.0, 120.0, 75.0), flight(3000, T0), T0);
        assert!(s.camera.raised());
        let t = T0 + 1000.0;
        s.advance_camera(t);
        let at = s.controller.view();
        s.camera.set_follow(false);
        let (_, wants) = s.advance_camera(t + 16.0);
        assert!(wants.scene, "the lock ending rebuilds the scene");
        assert!(!s.camera.raised() && !s.controller.locked());
        assert_eq!(s.controller.limits(), LIMITS);
        assert_eq!(s.controller.max_zoom_now(), 20.0);
        // steeper than 60°: the target on the ground, and back to 60° at once (deck holds the
        // 600 ms transition within the free limits from its first frame), the app's other moves
        // kept off for the 600 ms
        let v = s.controller.view();
        assert_eq!(v.position, Some([0.0, 0.0, 0.0]));
        assert_eq!((v.longitude, v.latitude, v.zoom, v.bearing, v.pitch), (at.longitude, at.latitude, 14.0, 120.0, 60.0));
        assert_eq!(s.controller.transition_until(), t + 16.0 + EASE_BACK_MS);
        assert!(s.controller.in_transition());
        s.advance_camera(t + 16.0 + EASE_BACK_MS / 2.0);
        assert_eq!(s.controller.view(), v, "half way");
        s.advance_camera(t + 16.0 + EASE_BACK_MS);
        assert_eq!(s.controller.view(), v);
        assert!(!s.controller.in_transition());
        let (_, wants) = s.advance_camera(t + 2000.0);
        assert!(!wants.scene && s.controller.view() == v, "nothing more to do");
    }

    #[test]
    fn released_on_the_globe_steeper_than_60_it_comes_back_to_60() {
        // low over the ground at zoom 11 the locked camera may tilt to 85°
        let mut s = locked_on(view(18.0, 59.6, 11.0, 40.0, 80.0), flight(1000, T0), T0);
        assert_eq!(s.controller.view().pitch, 80.0);
        assert!(!s.camera.raised());
        let at = s.controller.view();
        s.camera.set_selected(None);
        s.advance_camera(T0 + 16.0);
        let v = s.controller.view();
        assert_eq!(v, ViewState { pitch: 60.0, ..at });
        assert_eq!(s.controller.transition_until(), T0 + 16.0 + EASE_BACK_MS);
        assert_eq!(s.controller.limits(), LIMITS);
        s.advance_camera(T0 + 16.0 + EASE_BACK_MS);
        assert_eq!(s.controller.view(), v);
    }

    #[test]
    fn released_closer_than_zoom_20_comes_back_out() {
        // a light aircraft fills the screen past zoom 21
        let small = LiveFlight { typecode: "C172".into(), icon: "C206".into(), ..flight(500, T0) };
        let mut s = locked_on(view(18.0, 59.6, 21.0, 0.0, 40.0), small, T0);
        assert!(s.controller.max_zoom_now() > 21.0, "{}", s.controller.max_zoom_now());
        assert_eq!(s.controller.view().zoom, 21.0);
        s.camera.set_selected(None);
        let t = T0 + 16.0;
        s.advance_camera(t);
        assert!(!s.camera.following());
        assert_eq!(s.controller.transition_until(), t + EASE_BACK_MS);
        let v = s.controller.view();
        // to zoom 20, which deck's latitude shift of the limits holds at 20 + log2(cos lat)
        let shifted = 20.0 + (v.latitude * std::f64::consts::PI / 180.0).cos().log2();
        assert!((v.zoom - shifted).abs() < 1e-9, "{v:?}");
        assert_eq!(v.pitch, 40.0);
        s.advance_camera(t + EASE_BACK_MS);
        assert_eq!(s.controller.view(), v);
    }

    #[test]
    fn released_on_the_globe_within_the_free_limits_it_stays_with_the_free_limits() {
        for (basemap, free) in [(Basemap::Dark, LIMITS), (Basemap::Realistic, REALISTIC_LIMITS)] {
            let mut s = AppState::new(Startup { basemap: Some(basemap), ..Default::default() }, None, None, None, view(18.0, 59.6, 9.0, 0.0, 40.0), SIZE);
            s.camera.set_selected(Some(flight(8000, T0)));
            s.camera.set_follow(true);
            s.advance_camera(T0);
            assert_ne!(s.controller.limits(), free);
            let before = s.controller.view();
            s.camera.set_follow(false);
            s.advance_camera(T0 + 16.0);
            assert_eq!(s.controller.view(), before, "{basemap:?}: within zoom 20 and pitch 60, it stays");
            assert!(!s.controller.in_transition());
            assert_eq!(s.controller.limits(), free, "{basemap:?}");
            assert_eq!(s.camera.limits(basemap, SIZE[1]), free);
        }
    }

    #[test]
    fn a_followed_train_or_ship_locks_the_controller_too() {
        let mut s = state(view(18.0, 59.6, 13.0, 0.0, 0.0));
        s.camera.set_vehicle_locked(true);
        s.advance_camera(T0);
        assert!(s.controller.locked() && !s.camera.following());
        assert!(s.locked_steady(T0 + 1.0));
        assert_eq!(s.controller.view(), view(18.0, 59.6, 13.0, 0.0, 0.0), "its own step centres the camera, not this one");
        s.camera.set_vehicle_locked(false);
        s.advance_camera(T0 + 16.0);
        assert!(!s.controller.locked());
    }

    #[test]
    fn far_out_the_lock_refreshes_the_scene_on_the_zoom_interval() {
        let mut s = locked_on(view(18.0, 59.6, 6.0, 0.0, 20.0), flight(30_000, T0), T0);
        assert!(s.locked_steady(T0 + 1.0));
        assert_eq!(refresh_interval_ms(s.controller.view().zoom, true), 150.0);
        let (_, off) = drawn(&mut s, T0);
        assert!(off < 2.0, "{off} px off the centre");
        // the next frame asks for no refresh of its own
        let (_, wants) = s.advance_camera(T0 + 16.0);
        assert!(!wants.scene);
    }
}
