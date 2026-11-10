//! Port of `hover-details.ts` (S1.15, doc 05 §11.4, doc 03 §10.3): what the hover card adds to
//! the feed's own data: the aircraft's photo, the airline's logo and the flight's progress, from
//! `flight_details`. The source is unofficial and rate-limited, so a flight is only fetched once
//! the pointer has rested on it, one request at a time, and the answers are cached.
//!
//! The TypeScript's timer and promise become calls with the time: [`HoverLoader::tick`] runs the
//! dwell timer (`setTimeout`) and [`HoverLoader::settled`] the request's `then`/`catch`/`finally`.
//! Both return the flight whose details to fetch now, if any; the caller fetches it and hands the
//! answer to `settled`. `onchange` has no twin: the caller knows when it called `settled`, and
//! that is when the card should re-read. Every method that reads the clock takes `now_ms`.

use indexmap::IndexMap;

use crate::geo::sphere::LonLat;
use crate::model::{AircraftImage, FlightDetails, is_rate_limited};
use crate::trains::{js_max, js_min};

/// The few fields of a flight's details the hover card shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HoverInfo {
    pub images: Vec<AircraftImage>,
    /// Flightradar24 airline id for [`crate::model::airline_logo_url`], or 0.
    pub logo_id: u32,
    /// 0–100, as Flightradar24 reports it; `None` when it doesn't.
    pub progress_pct: Option<u32>,
}

/// Keeps the cache small: a flight's details carry its whole trail.
pub fn hover_info(details: &FlightDetails) -> HoverInfo {
    let schedule = details.schedule.as_ref();
    let progress = details.progress.as_ref();
    // `schedule?.painted_as_id || schedule?.operated_by_id || 0`
    let logo_id = match schedule {
        Some(s) if s.painted_as_id != 0 => s.painted_as_id,
        Some(s) => s.operated_by_id,
        None => 0,
    };
    HoverInfo {
        images: details.aircraft.as_ref().map(|a| a.images.clone()).unwrap_or_default(),
        logo_id,
        // an empty progress record (no route known) reports 0 with no distances
        progress_pct: progress
            .filter(|p| p.traversed_distance > 0 || p.remaining_distance > 0)
            .map(|p| p.progress_pct),
    }
}

/// `Loading`: asked for or about to be; `Ready`: in hand; `Unavailable`: the request failed or
/// the source is rate-limiting, so the card goes without.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoverStatus {
    Loading,
    Ready,
    Unavailable,
}

impl HoverStatus {
    /// The name in the TypeScript.
    pub fn as_str(self) -> &'static str {
        match self {
            HoverStatus::Loading => "loading",
            HoverStatus::Ready => "ready",
            HoverStatus::Unavailable => "unavailable",
        }
    }
}

/// A cached answer: when it was stored, and the info, or `None` for a failed request.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    at: i64,
    info: Option<HoverInfo>,
}

/// `HoverLoaderOptions` without `fetch`, `onchange` and `now` (see the module docs).
#[derive(Debug, Clone, Copy)]
pub struct HoverLoaderOptions {
    /// True for an error that means "slow down".
    pub is_rate_limited: fn(&str) -> bool,
    /// How long the pointer must rest on a flight before it is fetched, ms.
    pub dwell_ms: i64,
    pub capacity: usize,
    pub ttl_ms: i64,
    /// A failed flight is not asked for again for this long, ms.
    pub failure_ttl_ms: i64,
    /// After a rate-limit answer, nothing is asked for this long, ms.
    pub cooldown_ms: i64,
}

impl Default for HoverLoaderOptions {
    fn default() -> Self {
        HoverLoaderOptions {
            is_rate_limited,
            dwell_ms: 300,
            capacity: 100,
            ttl_ms: 10 * 60_000,
            failure_ttl_ms: 2 * 60_000,
            cooldown_ms: 60_000,
        }
    }
}

/// The hover card's details loader: debounced, serialised and cached `flight_details` requests.
#[derive(Debug, Clone)]
pub struct HoverLoader {
    opts: HoverLoaderOptions,
    /// Insertion order is recency order: least recently used first.
    cache: IndexMap<u32, Entry>,
    hovered: Option<u32>,
    /// The dwell timer: when it fires, and the flight it was set for.
    timer: Option<(i64, u32)>,
    in_flight: Option<u32>,
    /// The one flight waiting for the request in flight to finish.
    pending: Option<u32>,
    cool_until: i64,
}

impl Default for HoverLoader {
    fn default() -> Self {
        HoverLoader::new(HoverLoaderOptions::default())
    }
}

impl HoverLoader {
    pub fn new(options: HoverLoaderOptions) -> Self {
        HoverLoader {
            opts: options,
            cache: IndexMap::new(),
            hovered: None,
            timer: None,
            in_flight: None,
            pending: None,
            cool_until: 0,
        }
    }

    /// The pointer is now on `flight_id`, or on no flight.
    pub fn hover(&mut self, flight_id: Option<u32>, now_ms: i64) {
        if flight_id == self.hovered {
            return;
        }
        self.hovered = flight_id;
        self.timer = None;
        let Some(id) = flight_id else { return };
        if self.entry(id, now_ms).is_some() || self.cooling_down(now_ms) {
            return;
        }
        self.timer = Some((now_ms + self.opts.dwell_ms, id));
    }

    /// Runs the dwell timer: once it is due and the pointer is still on its flight, that flight
    /// is asked for. Returns the flight to fetch now, if any.
    pub fn tick(&mut self, now_ms: i64) -> Option<u32> {
        let (due, id) = self.timer?;
        if now_ms < due {
            return None;
        }
        self.timer = None;
        if self.hovered == Some(id) { self.request(id) } else { None }
    }

    /// When [`tick`](Self::tick) next has something to do, if ever: for scheduling a wake-up.
    pub fn deadline(&self) -> Option<i64> {
        self.timer.map(|(due, _)| due)
    }

    /// Cached details for a flight, if fresh.
    pub fn info(&mut self, flight_id: u32, now_ms: i64) -> Option<&HoverInfo> {
        self.entry(flight_id, now_ms)?.info.as_ref()
    }

    pub fn status(&mut self, flight_id: u32, now_ms: i64) -> HoverStatus {
        if let Some(entry) = self.entry(flight_id, now_ms) {
            return if entry.info.is_some() { HoverStatus::Ready } else { HoverStatus::Unavailable };
        }
        if self.in_flight == Some(flight_id) || self.pending == Some(flight_id) || self.timer.is_some() {
            return HoverStatus::Loading;
        }
        if self.cooling_down(now_ms) { HoverStatus::Unavailable } else { HoverStatus::Loading }
    }

    /// Stop any waiting request, e.g. when the globe goes away.
    pub fn dispose(&mut self) {
        self.timer = None;
        self.hovered = None;
        self.pending = None;
    }

    /// The request for `flight_id` (the one a `tick` or an earlier `settled` returned) has
    /// settled: its details, or the error's text. Returns the flight to fetch next, if any.
    pub fn settled(&mut self, flight_id: u32, result: Result<&FlightDetails, &str>, now_ms: i64) -> Option<u32> {
        match result {
            Ok(details) => self.store(flight_id, Some(hover_info(details)), now_ms),
            Err(message) => {
                if (self.opts.is_rate_limited)(message) {
                    self.cool_until = now_ms + self.opts.cooldown_ms;
                } else {
                    self.store(flight_id, None, now_ms);
                }
            }
        }
        self.in_flight = None;
        let next = self.pending.take();
        // only if the pointer is still there: anything else is stale
        match next {
            Some(next)
                if Some(next) == self.hovered && self.entry(next, now_ms).is_none() && !self.cooling_down(now_ms) =>
            {
                self.request(next)
            }
            _ => None,
        }
    }

    fn cooling_down(&self, now_ms: i64) -> bool {
        now_ms < self.cool_until
    }

    fn entry(&mut self, flight_id: u32, now_ms: i64) -> Option<&Entry> {
        let entry = self.cache.shift_remove(&flight_id)?;
        let ttl = if entry.info.is_some() { self.opts.ttl_ms } else { self.opts.failure_ttl_ms };
        if now_ms - entry.at > ttl {
            return None;
        }
        // most recently used
        let (index, _) = self.cache.insert_full(flight_id, entry);
        self.cache.get_index(index).map(|(_, entry)| entry)
    }

    fn store(&mut self, flight_id: u32, info: Option<HoverInfo>, now_ms: i64) {
        self.cache.shift_remove(&flight_id);
        self.cache.insert(flight_id, Entry { at: now_ms, info });
        while self.cache.len() > self.opts.capacity {
            self.cache.shift_remove_index(0);
        }
    }

    /// The flight to fetch now, or `None` when one is in flight and this one waits behind it.
    fn request(&mut self, flight_id: u32) -> Option<u32> {
        if self.in_flight.is_some() {
            self.pending = Some(flight_id); // replaces any earlier waiting flight
            return None;
        }
        self.in_flight = Some(flight_id);
        Some(flight_id)
    }
}

/// How far along its route a flight is, 0–1, from great-circle distances: flown from the origin
/// over flown plus remaining to the destination. An estimate; `None` when either end is unknown
/// or they are the same place. Points are `[lon, lat]` in degrees.
pub fn route_progress(position: LonLat, origin: Option<LonLat>, destination: Option<LonLat>) -> Option<f64> {
    let (Some(origin), Some(destination)) = (origin, destination) else { return None };
    let angle = |a: LonLat, b: LonLat| {
        let r = std::f64::consts::PI / 180.0;
        let (a_lon, a_lat, b_lon, b_lat) = (a[0], a[1], b[0], b[1]);
        let c = (a_lat * r).sin() * (b_lat * r).sin()
            + (a_lat * r).cos() * (b_lat * r).cos() * ((a_lon - b_lon) * r).cos();
        js_min(1.0, js_max(-1.0, c)).acos()
    };
    if angle(origin, destination) < 1e-4 {
        return None; // under ~600 m: no route to speak of
    }
    let flown = angle(origin, position);
    let remaining = angle(position, destination);
    Some(flown / (flown + remaining))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;
    use crate::model::{AircraftInfo, FlightProgress, ScheduleInfo};

    const DWELL: i64 = 5;

    fn details(id: u32) -> FlightDetails {
        FlightDetails {
            aircraft: Some(AircraftInfo {
                images: vec![AircraftImage {
                    url: "u".into(),
                    copyright: format!("p{id}"),
                    thumbnail: "t".into(),
                    medium: "m".into(),
                    large: "l".into(),
                }],
                ..AircraftInfo::default()
            }),
            ..FlightDetails::default()
        }
    }

    /// A loader whose requests settle only when the test says so, on a clock the test moves.
    struct Setup {
        loader: HoverLoader,
        calls: Vec<u32>,
        t: i64,
    }

    fn setup(t: i64) -> Setup {
        let loader = HoverLoader::new(HoverLoaderOptions {
            is_rate_limited: |m| m.contains("status 8"),
            dwell_ms: DWELL,
            ..HoverLoaderOptions::default()
        });
        Setup { loader, calls: Vec::new(), t }
    }

    impl Setup {
        fn hover(&mut self, id: Option<u32>) {
            self.loader.hover(id, self.t);
        }

        /// Lets `ms` pass, a millisecond at a time, running the timer as an event loop would.
        fn wait(&mut self, ms: i64) {
            for _ in 0..ms {
                self.t += 1;
                if let Some(id) = self.loader.tick(self.t) {
                    self.calls.push(id);
                }
            }
        }

        fn ok(&mut self, id: u32) {
            if let Some(next) = self.loader.settled(id, Ok(&details(id)), self.t) {
                self.calls.push(next);
            }
        }

        fn fail(&mut self, id: u32, message: &str) {
            if let Some(next) = self.loader.settled(id, Err(message), self.t) {
                self.calls.push(next);
            }
        }

        fn status(&mut self, id: u32) -> &'static str {
            self.loader.status(id, self.t).as_str()
        }
    }

    const START: i64 = 1_700_000_000_000;

    #[test]
    fn fetches_only_after_the_pointer_rests() {
        let mut s = setup(START);
        s.hover(Some(1));
        s.hover(Some(2));
        s.hover(Some(3));
        assert_eq!(s.calls, Vec::<u32>::new());
        s.wait(DWELL * 3);
        assert_eq!(s.calls, [3]);
        assert_eq!(s.status(3), "loading");
    }

    #[test]
    fn caches_answers_and_never_asks_twice() {
        let mut s = setup(START);
        s.hover(Some(1));
        s.wait(DWELL * 3);
        s.ok(1);
        s.wait(1);
        assert_eq!(s.status(1), "ready");
        assert_eq!(s.loader.info(1, s.t).unwrap().images[0].copyright, "p1");
        s.hover(None);
        s.hover(Some(1));
        s.wait(DWELL * 3);
        assert_eq!(s.calls, [1]);
    }

    #[test]
    fn one_request_at_a_time_and_stale_waiting_flights_are_dropped() {
        let mut s = setup(START);
        s.hover(Some(1));
        s.wait(DWELL * 3);
        s.hover(Some(2));
        s.wait(DWELL * 3);
        s.hover(Some(3));
        s.wait(DWELL * 3);
        assert_eq!(s.calls, [1]); // 2 and 3 wait; 3 replaced 2
        s.hover(None); // and the pointer left
        s.ok(1);
        s.wait(1);
        assert_eq!(s.calls, [1]);
    }

    #[test]
    fn the_waiting_flight_is_fetched_next_if_still_hovered() {
        let mut s = setup(START);
        s.hover(Some(1));
        s.wait(DWELL * 3);
        s.hover(Some(2));
        s.wait(DWELL * 3);
        s.ok(1);
        s.wait(1);
        assert_eq!(s.calls, [1, 2]);
    }

    #[test]
    fn rate_limiting_pauses_all_requests() {
        let mut s = setup(1_000_000);
        s.hover(Some(1));
        s.wait(DWELL * 3);
        s.fail(1, "grpc status 8");
        s.wait(1);
        assert_eq!(s.status(1), "unavailable");
        s.hover(Some(2));
        s.wait(DWELL * 3);
        assert_eq!(s.calls, [1]);
        assert_eq!(s.status(2), "unavailable");
        s.t += 61_000;
        s.hover(Some(3));
        s.wait(DWELL * 3);
        assert_eq!(s.calls, [1, 3]);
    }

    #[test]
    fn a_failure_is_remembered_for_a_while() {
        let mut s = setup(1_000_000);
        s.hover(Some(1));
        s.wait(DWELL * 3);
        s.fail(1, "offline");
        s.wait(1);
        s.hover(None);
        s.hover(Some(1));
        s.wait(DWELL * 3);
        assert_eq!(s.calls, [1]);
        assert_eq!(s.status(1), "unavailable");
        s.t += 3 * 60_000;
        s.hover(None);
        s.hover(Some(1));
        s.wait(DWELL * 3);
        assert_eq!(s.calls, [1, 1]);
    }

    #[test]
    fn route_progress_from_great_circle_distances() {
        let vno = [25.285, 54.634];
        let waw = [20.967, 52.166];
        assert_close!(route_progress(vno, Some(vno), Some(waw)).unwrap(), 0, 5);
        assert_close!(route_progress(waw, Some(vno), Some(waw)).unwrap(), 1, 5);
        let mid = [(vno[0] + waw[0]) / 2.0, (vno[1] + waw[1]) / 2.0];
        assert_close!(route_progress(mid, Some(vno), Some(waw)).unwrap(), 0.5, 1);
        assert_eq!(route_progress(vno, None, Some(waw)), None);
        assert_eq!(route_progress(vno, Some(vno), Some(vno)), None);
    }

    // New tests: the parts the TypeScript tests reach only through the card.

    #[test]
    fn hover_info_takes_the_painted_airline_then_the_operator_and_progress_only_with_a_route() {
        let mut d = details(7);
        let images = d.aircraft.clone().unwrap().images;
        assert_eq!(hover_info(&d), HoverInfo { images, logo_id: 0, progress_pct: None });
        d.schedule = Some(ScheduleInfo { painted_as_id: 0, operated_by_id: 81, ..ScheduleInfo::default() });
        assert_eq!(hover_info(&d).logo_id, 81);
        d.schedule = Some(ScheduleInfo { painted_as_id: 12, operated_by_id: 81, ..ScheduleInfo::default() });
        assert_eq!(hover_info(&d).logo_id, 12);
        d.progress = Some(FlightProgress { progress_pct: 0, ..FlightProgress::default() });
        assert_eq!(hover_info(&d).progress_pct, None);
        d.progress = Some(FlightProgress { remaining_distance: 5, progress_pct: 0, ..FlightProgress::default() });
        assert_eq!(hover_info(&d).progress_pct, Some(0));
        d.progress = Some(FlightProgress { traversed_distance: 9, progress_pct: 64, ..FlightProgress::default() });
        assert_eq!(hover_info(&d).progress_pct, Some(64));
        d.aircraft = None;
        assert!(hover_info(&d).images.is_empty());
    }

    #[test]
    fn defaults_match_the_typescript() {
        let o = HoverLoaderOptions::default();
        let durations = (o.dwell_ms, o.ttl_ms, o.failure_ttl_ms, o.cooldown_ms);
        assert_eq!((durations, o.capacity), ((300, 600_000, 120_000, 60_000), 100));
        assert!((o.is_rate_limited)("Too many requests"));
        assert!(!(o.is_rate_limited)("offline"));
    }

    #[test]
    fn answers_expire_and_the_least_recently_used_is_evicted() {
        let mut loader = HoverLoader::new(HoverLoaderOptions { capacity: 2, ..HoverLoaderOptions::default() });
        let mut t = START;
        for id in [1, 2] {
            loader.hover(Some(id), t);
            t += 300;
            assert_eq!(loader.tick(t - 1), None);
            assert_eq!(loader.tick(t), Some(id));
            assert_eq!(loader.settled(id, Ok(&details(id)), t), None);
        }
        // reading 1 makes 2 the least recently used, so storing 3 evicts 2
        assert!(loader.info(1, t).is_some());
        loader.hover(Some(3), t);
        t += 300;
        assert_eq!(loader.tick(t), Some(3));
        loader.settled(3, Ok(&details(3)), t);
        assert!(loader.info(2, t).is_none());
        assert_eq!(loader.status(1, t), HoverStatus::Ready);
        // fresh for exactly ten minutes
        let stored_3 = t;
        assert!(loader.info(3, stored_3 + 600_000).is_some());
        assert!(loader.info(3, stored_3 + 600_001).is_none());
        // and an expired entry is gone: hovering again asks again
        loader.hover(None, stored_3 + 600_001);
        loader.hover(Some(3), stored_3 + 600_001);
        assert_eq!(loader.deadline(), Some(stored_3 + 600_301));
    }

    #[test]
    fn dispose_drops_the_timer_and_the_waiting_flight() {
        let mut loader = HoverLoader::default();
        loader.hover(Some(1), 0);
        assert_eq!(loader.tick(300), Some(1));
        loader.hover(Some(2), 300);
        assert_eq!(loader.tick(600), None);
        assert_eq!(loader.status(2, 600), HoverStatus::Loading);
        loader.hover(Some(3), 600);
        loader.dispose();
        assert_eq!(loader.deadline(), None);
        assert_eq!(loader.tick(10_000), None);
        // the request in flight still lands, and nothing follows it
        assert_eq!(loader.settled(1, Ok(&details(1)), 10_000), None);
        assert_eq!(loader.status(1, 10_000), HoverStatus::Ready);
        assert_eq!(loader.status(2, 10_000), HoverStatus::Loading);
    }
}
