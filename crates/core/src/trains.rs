//! Port of `trains.ts` without the canvas atlas (S1.14, doc 05 §12.4): colours and names per
//! kind and operator, motion between refreshes, the credits each source asks for, the icon keys
//! and sizes, what a stop shows, and the signal block a train is in.
//!
//! `trainAtlas` (Canvas 2D) is drawn by `scene::atlas::trains`: a row per kind ([`TRAIN_KINDS`],
//! keyed by [`train_kind_key`]) then one per operator ([`OPERATORS`], keyed `op{i}`), each with a
//! `car-` and a `box-` cell, as [`train_icon`] names them.
//!
//! Times are whole milliseconds (`now_ms`) as everywhere in this crate; frame times are the
//! model's `i64` Unix milliseconds.

use std::f64::consts::PI;
use std::sync::LazyLock;

use regex::Regex;

use crate::freshness::{Freshness, age_label};
use crate::geo::Rgb;
use crate::model::{Train, TrainBlock, TrainKind, TrainSource, TrainStop, f32_as_js};
use crate::num::js_round;

/// `Math.min(a, b)`: NaN when either is NaN, and -0 below +0.
pub(crate) fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.max(a, b)`: NaN when either is NaN, and +0 above -0.
pub(crate) fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// JavaScript's `\s` (and what `trim()` removes): Unicode white space without U+0085, plus the
/// byte-order mark U+FEFF. `char::is_whitespace` has U+0085 and not U+FEFF.
pub(crate) fn is_js_space(c: char) -> bool {
    c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace())
}

/// A latitude and longitude in degrees (`{ lat, lon }` in the TypeScript).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

/// Where a train is drawn: `(lat, lon, unix ms)`, as `Train::frames` holds them.
pub type Frame = (f64, f64, i64);

// --- Kinds and operators ---------------------------------------------------------

/// The kinds in display order (fast to slow, passengers first).
pub const TRAIN_KINDS: [TrainKind; 5] =
    [TrainKind::HighSpeed, TrainKind::Intercity, TrainKind::Regional, TrainKind::Suburban, TrainKind::Other];

/// A kind's name on the wire, which is also its atlas key (`car-highSpeed`).
pub fn train_kind_key(kind: TrainKind) -> &'static str {
    match kind {
        TrainKind::HighSpeed => "highSpeed",
        TrainKind::Intercity => "intercity",
        TrainKind::Regional => "regional",
        TrainKind::Suburban => "suburban",
        TrainKind::Other => "other",
    }
}

/// The kind a wire name stands for; none for a name this app doesn't know. (The TypeScript's
/// `KINDS[kind] ?? KINDS.other` fallback has no counterpart here: a `TrainKind` is always known.)
pub fn train_kind_named(name: &str) -> Option<TrainKind> {
    TRAIN_KINDS.into_iter().find(|kind| train_kind_key(*kind) == name)
}

/// Calm tones that read on the dark map, on hues the ship palette leaves free, and clear of the
/// amber reserved for aircraft and selection.
pub fn train_color(kind: TrainKind) -> Rgb {
    match kind {
        TrainKind::HighSpeed => [226, 92, 196], // magenta
        TrainKind::Intercity => [128, 132, 246], // periwinkle
        TrainKind::Regional => [96, 212, 160],  // mint
        TrainKind::Suburban => [112, 190, 232], // cerulean
        TrainKind::Other => [158, 150, 140],    // warm grey
    }
}

pub fn train_label(kind: TrainKind) -> &'static str {
    match kind {
        TrainKind::HighSpeed => "High-speed",
        TrainKind::Intercity => "Intercity",
        TrainKind::Regional => "Regional",
        TrainKind::Suburban => "Suburban",
        TrainKind::Other => "Other",
    }
}

/// Operators with a colour of their own (Sweden's, as Trafikverket names them), so a train is
/// recognised by who runs it; others take their kind's colour. First match wins, on the trimmed
/// name, case-insensitively; the index is the atlas row `op{i}`.
///
/// The patterns are the TypeScript's regexes. JavaScript's `\b` is ASCII, so it is written
/// `(?-u:\b)`: there a following `ö` or `å` ends the word, in the regex crate's Unicode mode it
/// does not. The case-insensitive flag stays Unicode, so `Å` matches `å` as in JavaScript.
pub const OPERATORS: [(&str, Rgb); 16] = [
    (r"^sj(?-u:\b)", [232, 232, 236]),                                 // silver
    (r"^sl(?-u:\b)|storstockholms", [70, 146, 236]),                   // blue
    ("mälartåg", [236, 84, 84]),                                       // red
    ("västtrafik|västtåg", [62, 200, 236]),                            // sky
    ("skånetrafiken|pågatåg", [196, 92, 222]),                         // purple
    ("öresundståg", [150, 132, 246]),                                  // lavender
    ("arlanda express", [250, 212, 64]),                               // yellow
    ("snälltåget", [120, 214, 92]),                                    // green
    ("norrtåg", [58, 204, 176]),                                       // teal
    (r"^vy(?-u:\b)", [240, 92, 150]),                                  // pink
    ("bergslagen", [244, 152, 56]),                                    // orange
    (r"upptåget|^ul(?-u:\b)", [250, 120, 40]),                         // tangerine
    ("östgöta", [214, 70, 130]),                                       // raspberry
    ("x-trafik|x-tåget", [150, 220, 60]),                              // lime
    ("krösatåg|kronoberg|jönköping|kalmar|blekinge", [236, 120, 110]), // coral
    ("värmland", [110, 180, 255]),                                     // cornflower
];

/// [`OPERATORS`]' patterns, compiled once with the `i` flag.
static OPERATOR_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    OPERATORS.iter().map(|(pattern, _)| Regex::new(&format!("(?i){pattern}")).expect("operator pattern")).collect()
});

/// The row of [`OPERATORS`] an operator's name matches.
fn operator_index(operator: Option<&str>) -> Option<usize> {
    // `operator ? … : -1`: an empty name is falsy
    let operator = operator.filter(|o| !o.is_empty())?;
    let trimmed = operator.trim_matches(is_js_space);
    OPERATOR_PATTERNS.iter().position(|re| re.is_match(trimmed))
}

/// The train's colour: its operator's, else its kind's.
pub fn train_color_of(train: &Train) -> Rgb {
    match operator_index(train.operator.as_deref()) {
        Some(i) => OPERATORS[i].1,
        None => train_color(train.kind),
    }
}

// --- Credits ---------------------------------------------------------------------

/// An attribution and where it links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credit {
    pub text: &'static str,
    pub url: &'static str,
}

/// Every train source (`Object.keys(TRAIN_CREDITS)`).
pub const TRAIN_SOURCES: [TrainSource; 1] = [TrainSource::Trafikverket];

/// The attribution each source asks for (`TRAIN_CREDITS`).
pub fn train_credit(source: TrainSource) -> Credit {
    match source {
        TrainSource::Trafikverket => {
            Credit { text: "Trains in Sweden: Trafikverket, CC0", url: "https://data.trafikverket.se/" }
        }
    }
}

/// The 3D train models' maker, credited (as he asked) wherever they are drawn.
pub const MODEL_CREDIT: Credit = Credit {
    text: "3D trains: Lardman",
    url: "https://3dwarehouse.sketchup.com/user/1082279160696486794315506/Lardman",
};

/// The credits for the sources of these trains, once each in the order they first appear, and
/// the models' maker where any has a model.
pub fn train_credits<'a>(trains: impl IntoIterator<Item = &'a Train>) -> Vec<Credit> {
    let mut sources: Vec<TrainSource> = Vec::new();
    let mut modelled = false;
    for train in trains {
        if !sources.contains(&train.source) {
            sources.push(train.source);
        }
        // `trains.some((t) => t.model)`: an empty name is falsy
        modelled |= train.model.as_deref().is_some_and(|m| !m.is_empty());
    }
    let mut credits: Vec<Credit> = sources.into_iter().map(train_credit).collect();
    if modelled {
        credits.push(MODEL_CREDIT);
    }
    credits
}

/// What a source is called in the panel (`TRAIN_SOURCE_NAMES`).
pub fn train_source_name(source: TrainSource) -> &'static str {
    match source {
        TrainSource::Trafikverket => "Trafikverket",
    }
}

// --- Motion ----------------------------------------------------------------------

/// The frames' segment around `now_ms`: indexes and how far along (0..1); none without frames.
fn segment(frames: &[Frame], now_ms: i64) -> Option<(usize, usize, f64)> {
    if frames.is_empty() {
        return None;
    }
    if frames.len() == 1 || now_ms <= frames[0].2 {
        return Some((0, 0, 0.0));
    }
    let last = frames.len() - 1;
    if now_ms >= frames[last].2 {
        return Some((last, last, 0.0));
    }
    let mut i = 0;
    while i < last - 1 && frames[i + 1].2 <= now_ms {
        i += 1;
    }
    let t0 = frames[i].2;
    let t1 = frames[i + 1].2;
    Some((i, i + 1, if t1 > t0 { (now_ms - t0) as f64 / (t1 - t0) as f64 } else { 1.0 }))
}

/// Where the train is at `now_ms` (Unix ms): along its frames when it has them (held at the first
/// before it starts and at the last after it ends, never extrapolated), else its reported
/// position.
pub fn position_at(train: &Train, now_ms: i64) -> LatLon {
    position_along(&train.frames, train.lat, train.lon, now_ms)
}

/// [`position_at`] on other frames than the train's own (its `predicted` ones, in dev builds),
/// with `lat`, `lon` where there are none.
pub fn position_along(frames: &[Frame], lat: f64, lon: f64, now_ms: i64) -> LatLon {
    let Some((i, j, f)) = segment(frames, now_ms) else {
        return LatLon { lat, lon };
    };
    let (lat0, lon0, _) = frames[i];
    let (lat1, lon1, _) = frames[j];
    // a leg across the antimeridian goes the short way round
    let mut d_lon = lon1 - lon0;
    if d_lon > 180.0 {
        d_lon -= 360.0;
    } else if d_lon < -180.0 {
        d_lon += 360.0;
    }
    let mut lon = lon0 + d_lon * f;
    if lon > 180.0 {
        lon -= 360.0;
    } else if lon < -180.0 {
        lon += 360.0;
    }
    LatLon { lat: lat0 + (lat1 - lat0) * f, lon }
}

const DEG: f64 = PI / 180.0;

/// The direction of travel at `now_ms`: along the current frame leg, else the reported heading.
pub fn heading_at(train: &Train, now_ms: i64) -> Option<f64> {
    heading_along(&train.frames, train.heading.map(f32_as_js), now_ms)
}

/// [`heading_at`] on other frames than the train's own, with `heading` where they give none.
pub fn heading_along(frames: &[Frame], heading: Option<f64>, now_ms: i64) -> Option<f64> {
    if let Some((s, _, _)) = segment(frames, now_ms)
        && frames.len() > 1
    {
        // the leg being travelled, or the last one once past the end
        let i = s.min(frames.len() - 2);
        let (lat0, lon0, _) = frames[i];
        let (lat1, lon1, _) = frames[i + 1];
        if lat0 != lat1 || lon0 != lon1 {
            let y = ((lon1 - lon0) * DEG).sin() * (lat1 * DEG).cos();
            let x = (lat0 * DEG).cos() * (lat1 * DEG).sin()
                - (lat0 * DEG).sin() * (lat1 * DEG).cos() * ((lon1 - lon0) * DEG).cos();
            return Some(((y.atan2(x) / DEG) % 360.0 + 360.0) % 360.0);
        }
    }
    heading
}

// --- Wording -----------------------------------------------------------------------

/// "5 min late", "2 min early", "on time"; empty when unknown.
pub fn delay_text(minutes: Option<i32>) -> String {
    match minutes {
        None => String::new(),
        Some(0) => "on time".to_owned(),
        Some(m) if m > 0 => format!("{m} min late"),
        Some(m) => format!("{} min early", -i64::from(m)),
    }
}

/// A train's position counts as stale past this age (seconds): the feeds report every few
/// seconds to a minute.
pub const TRAIN_STALE_S: i64 = 5 * 60;

/// `Position 40 s ago`, or past [`TRAIN_STALE_S`] `Position 12 min old`, in the warning tone.
pub fn train_position_age(position_unix: i64, now_ms: i64) -> Freshness {
    let age = js_max(0.0, now_ms as f64 / 1000.0 - position_unix as f64);
    if age > TRAIN_STALE_S as f64 {
        Freshness { stale: true, label: format!("Position {} old", age_label(age)) }
    } else {
        Freshness { stale: false, label: format!("Position {} ago", age_label(age)) }
    }
}

/// A train is left on the map this long after it stands at its last stop (ms).
pub const TRAIN_ENDED_MS: i64 = 30_000;

/// Whether a train is still shown: not more than [`TRAIN_ENDED_MS`] past the end of its run.
pub fn train_running(train: &Train, now_ms: i64) -> bool {
    match train.ended_unix {
        None => true,
        Some(ended) => now_ms.saturating_sub(ended.saturating_mul(1000)) <= TRAIN_ENDED_MS,
    }
}

// --- On the map ----------------------------------------------------------------------

/// A train with a direction is a carriage with a nose; one without, a rounded square. Coloured
/// by operator when it has a colour.
pub fn train_icon(kind: TrainKind, heading: Option<f64>, operator: Option<&str>) -> String {
    format!("{}-{}", if heading.is_none() { "box" } else { "car" }, colour_key(kind, operator))
}

/// The atlas row a train's colour is in: its operator's, else its kind's.
fn colour_key(kind: TrainKind, operator: Option<&str>) -> String {
    match operator_index(operator) {
        Some(i) => format!("op{i}"),
        None => train_kind_key(kind).to_owned(),
    }
}

/// Base icon length in pixels per kind: long-distance trains a little longer, commuter trains
/// shorter.
fn icon_px(kind: TrainKind) -> f64 {
    match kind {
        TrainKind::HighSpeed => 24.0,
        TrainKind::Intercity => 23.0,
        TrainKind::Regional => 21.0,
        TrainKind::Suburban => 20.0,
        TrainKind::Other => 19.0,
    }
}

/// Smaller zoomed out; in steps, so sizes update rarely.
pub fn train_icon_scale(zoom: f64) -> f64 {
    if zoom < 4.0 {
        0.7
    } else if zoom < 6.0 {
        0.82
    } else if zoom < 9.0 {
        0.92
    } else {
        1.0
    }
}

/// Icon size in pixels (IconLayer `getSize`: the cell's height).
pub fn train_icon_size(kind: TrainKind, heading: Option<f64>, zoom: f64) -> f64 {
    icon_px(kind) * train_icon_scale(zoom) * if heading.is_none() { 0.8 } else { 1.0 }
}

/// The tone a delay is shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelayTone {
    Good,
    Warn,
    Bad,
}

impl DelayTone {
    /// The TypeScript's name for it (a badge class).
    pub fn as_str(self) -> &'static str {
        match self {
            DelayTone::Good => "good",
            DelayTone::Warn => "warn",
            DelayTone::Bad => "bad",
        }
    }
}

/// The delay's tone: a few minutes is ordinary for a train, past 5 worth noting, past 30 bad.
pub fn delay_tone(minutes: Option<i32>) -> Option<DelayTone> {
    let minutes = minutes?;
    Some(if minutes >= 30 {
        DelayTone::Bad
    } else if minutes >= 5 {
        DelayTone::Warn
    } else {
        DelayTone::Good
    })
}

// --- Stops ---------------------------------------------------------------------------

/// What a stop shows: its time (departure, or arrival at the last), as it now stands, and
/// whether the train is past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StopView {
    /// Unix seconds: the actual time, else the estimate, else the plan.
    pub time: Option<i64>,
    pub planned: Option<i64>,
    /// Minutes off the plan, when there's something to compare.
    pub late: Option<i64>,
    pub passed: bool,
}

pub fn stop_view(stop: &TrainStop) -> StopView {
    let t = stop.departure.or(stop.arrival);
    let planned = t.and_then(|t| t.planned);
    let time = t.and_then(|t| t.actual.or(t.estimated)).or(planned);
    let late = match (planned, time) {
        (Some(p), Some(t)) if t != p => Some(js_round((t - p) as f64 / 60.0) as i64),
        _ => None,
    };
    // `Boolean(actual)`: a time of 0 is falsy
    let actual = match stop.departure {
        Some(departure) => departure.actual,
        None => stop.arrival.and_then(|a| a.actual),
    };
    StopView { time, planned, late, passed: actual.is_some_and(|a| a != 0) }
}

/// The first stop not yet passed; none when the run is over (`-1` in the TypeScript).
pub fn next_stop(stops: &[TrainStop]) -> Option<usize> {
    let next = stops.iter().rposition(|stop| stop_view(stop).passed).map_or(0, |last| last + 1);
    (next < stops.len()).then_some(next)
}

/// The product a train runs as ("Öresundståg"), when its name carries one: Trafikverket's are
/// "<product> <number>".
pub fn train_product(train: &Train) -> Option<String> {
    if train.source != TrainSource::Trafikverket || train.name.starts_with("Train ") {
        return None;
    }
    // `name.replace(/\s+\S+$/, "")`: the last word and the space before it, when there is both
    let name = train.name.as_str();
    let word = name.trim_end_matches(|c: char| !is_js_space(c));
    let before = word.trim_end_matches(is_js_space);
    let stripped = if word.len() < name.len() && before.len() < word.len() { before } else { name };
    let product = stripped.trim_matches(is_js_space);
    (!product.is_empty() && product != name).then(|| product.to_owned())
}

// --- Signal blocks -----------------------------------------------------------------------

/// A train drawn this near a block's track is in it, metres.
const IN_BLOCK_M: f64 = 12.0;
/// A train stays in the block it was in while that block's track is at most this much further
/// from it than the nearest block's, metres. Blocks overlap (one runs on through a crossover
/// onto the track beside its own), so the nearest of two at the same place flips as the train
/// moves along; this is more than it strays from its track as drawn (a metre), less than the
/// 4.5 m to the next track over.
const KEEP_BLOCK_M: f64 = 2.0;

/// The nearest a point is to a line of `[lon, lat]` points, metres (flat).
fn metres_to_line(p: [f64; 2], line: &[[f64; 2]]) -> f64 {
    let k = (p[1] * PI / 180.0).cos();
    let mut best = f64::INFINITY;
    for pair in line.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (ax, ay) = ((a[0] - p[0]) * k, a[1] - p[1]);
        let (dx, dy) = ((b[0] - a[0]) * k, b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 { js_max(0.0, js_min(1.0, -(ax * dx + ay * dy) / len2)) } else { 0.0 };
        best = js_min(best, (ax + dx * t).hypot(ay + dy * t) * 111_320.0);
    }
    best
}

/// The block a train drawn at `p` (`[lon, lat]`) is in: the one it was in (`was`) while it is
/// still on that block's track, else the nearest, if near enough; none when none is.
pub fn block_at<'a>(p: [f64; 2], blocks: &'a [TrainBlock], was: Option<&str>) -> Option<&'a str> {
    let mut best: Option<(&'a str, f64)> = None;
    let mut kept = f64::INFINITY;
    let mut kept_id: Option<&'a str> = None;
    for block in blocks {
        // `Math.min(...lines)`: Infinity for a block without lines
        let d = block.lines.iter().fold(f64::INFINITY, |d, line| js_min(d, metres_to_line(p, line)));
        if was == Some(block.id.as_str()) {
            kept = d;
            kept_id = Some(&block.id);
        }
        if d <= IN_BLOCK_M && best.is_none_or(|(_, best)| d < best) {
            best = Some((&block.id, d));
        }
    }
    match best {
        // `kept` is finite only when a block is `was`, so `kept_id` holds it
        Some((_, d)) if kept <= js_min(IN_BLOCK_M, d + KEEP_BLOCK_M) => kept_id,
        best => best.map(|(id, _)| id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_close;
    use crate::model::StopTime;

    fn train(over: impl FnOnce(&mut Train)) -> Train {
        let mut t = Train {
            id: "vbb:1".into(),
            source: TrainSource::Trafikverket,
            name: "RE1".into(),
            kind: TrainKind::Regional,
            lat: 52.51,
            lon: 13.435,
            heading: None,
            speed_kmh: None,
            destination: None,
            operator: None,
            delay_minutes: None,
            position_unix: 1_000,
            frames: vec![],
            reports: vec![],
            predicted: vec![],
            cars: None,
            tail: vec![],
            structures: vec![],
            ended_unix: None,
            shape: None,
            model: None,
        };
        over(&mut t);
        t
    }

    // --- colours and labels

    #[test]
    fn every_kind_has_a_distinct_colour_and_a_label() {
        let mut colors: Vec<Rgb> = TRAIN_KINDS.iter().map(|k| train_color(*k)).collect();
        colors.sort();
        colors.dedup();
        assert_eq!(colors.len(), TRAIN_KINDS.len());
        assert_eq!(
            TRAIN_KINDS.map(train_label),
            ["High-speed", "Intercity", "Regional", "Suburban", "Other"]
        );
    }

    #[test]
    fn unknown_kinds_fall_back_to_other() {
        // TypeScript: trainColor("tram") is other's. A TrainKind is always a known kind, so the
        // fallback is where a name is read: an unknown one is no kind, and reads as other
        assert_eq!(train_kind_named("tram"), None);
        let tram = train_kind_named("tram").unwrap_or(TrainKind::Other);
        assert_eq!(train_color(tram), train_color(TrainKind::Other));
        assert_eq!(train_label(tram), "Other");
        for kind in TRAIN_KINDS {
            assert_eq!(train_kind_named(train_kind_key(kind)), Some(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(train_kind_key(kind)));
        }
    }

    #[test]
    fn clear_of_the_ship_palette_and_of_the_selection_amber() {
        // ships.ts `shipColor` for SHIP_COLOR_KEYS [0, 2, 3, 4, 5, 6, 7, 8]
        let ships: [Rgb; 8] = [
            [140, 152, 168],
            [72, 198, 194],
            [98, 152, 240],
            [116, 188, 112],
            [236, 128, 156],
            [218, 92, 92],
            [228, 234, 242],
            [192, 136, 236],
        ];
        let amber: Rgb = [240, 180, 60];
        let dist = |a: Rgb, b: Rgb| {
            let d = |i: usize| f64::from(a[i]) - f64::from(b[i]);
            (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt()
        };
        for kind in TRAIN_KINDS {
            let c = train_color(kind);
            for s in ships {
                assert!(dist(c, s) > 25.0, "{kind:?} near ship colour {s:?}");
            }
            assert!(dist(c, amber) > 60.0, "{kind:?} near amber");
        }
    }

    // --- positionAt

    fn frames() -> Vec<Frame> {
        vec![(52.51, 13.435, 10_000), (52.5085, 13.45, 20_000), (52.503, 13.469, 40_000)]
    }

    #[test]
    fn without_frames_the_reported_position() {
        assert_eq!(position_at(&train(|_| {}), 15_000), LatLon { lat: 52.51, lon: 13.435 });
    }

    #[test]
    fn interpolates_between_frames() {
        let t = train(|t| t.frames = frames());
        assert_eq!(position_at(&t, 10_000), LatLon { lat: 52.51, lon: 13.435 });
        let mid = position_at(&t, 15_000);
        assert_close!(mid.lat, 52.50925, 6);
        assert_close!(mid.lon, 13.4425, 6);
        let later = position_at(&t, 30_000);
        assert_close!(later.lat, 52.50575, 6);
        assert_close!(later.lon, 13.4595, 6);
        assert_eq!(position_at(&t, 20_000), LatLon { lat: 52.5085, lon: 13.45 });
    }

    #[test]
    fn holds_at_the_ends_rather_than_guessing() {
        let t = train(|t| t.frames = frames());
        assert_eq!(position_at(&t, 0), LatLon { lat: 52.51, lon: 13.435 });
        assert_eq!(position_at(&t, 99_000), LatLon { lat: 52.503, lon: 13.469 });
        assert_eq!(position_at(&train(|t| t.frames = vec![(1.0, 2.0, 5)]), 100), LatLon { lat: 1.0, lon: 2.0 });
    }

    #[test]
    fn crosses_the_antimeridian_the_short_way() {
        let t = train(|t| t.frames = vec![(10.0, 179.5, 0), (10.0, -179.5, 1_000)]);
        assert_close!(position_at(&t, 250).lon, 179.75, 6);
        assert_close!(position_at(&t, 750).lon, -179.75, 6);
    }

    // --- headingAt

    #[test]
    fn along_the_frames_else_the_reported_heading() {
        let east = train(|t| {
            t.heading = Some(10.0);
            t.frames = vec![(0.0, 0.0, 0), (0.0, 1.0, 1_000)];
        });
        assert_close!(heading_at(&east, 500).unwrap(), 90.0, 3);
        assert_close!(heading_at(&east, 5_000).unwrap(), 90.0, 3);
        assert_eq!(heading_at(&train(|t| t.heading = Some(10.0)), 0), Some(10.0));
        assert_eq!(heading_at(&train(|_| {}), 0), None);
    }

    // --- credits and wording

    #[test]
    fn one_credit_per_source_in_view() {
        let trafikverket = train_credit(TrainSource::Trafikverket);
        let credits = train_credits(&[train(|_| {}), train(|_| {})]);
        assert_eq!(credits, [trafikverket]);
        assert_eq!(train_credits(&[train(|t| t.model = Some("x2000".into()))]), [trafikverket, MODEL_CREDIT]);
        assert!(trafikverket.text.contains("CC0"));
    }

    #[test]
    fn delays() {
        assert_eq!(delay_text(Some(5)), "5 min late");
        assert_eq!(delay_text(Some(-2)), "2 min early");
        assert_eq!(delay_text(Some(0)), "on time");
        assert_eq!(delay_text(None), "");
    }

    // --- on the map

    #[test]
    fn a_carriage_with_a_heading_a_square_without_unknown_kinds_as_other() {
        assert_eq!(train_icon(TrainKind::Intercity, Some(90.0), None), "car-intercity");
        assert_eq!(train_icon(TrainKind::Suburban, None, None), "box-suburban");
        // "tram" reads as other (see `unknown_kinds_fall_back_to_other`)
        let tram = train_kind_named("tram").unwrap_or(TrainKind::Other);
        assert_eq!(train_icon(tram, Some(0.0), None), "car-other");
        assert_eq!(train_icon(TrainKind::HighSpeed, Some(0.0), None), "car-highSpeed");
    }

    #[test]
    fn sizes_stay_modest_and_grow_with_zoom() {
        for kind in TRAIN_KINDS {
            let close = train_icon_size(kind, Some(0.0), 12.0);
            assert!(close > 12.0);
            assert!(close <= 24.0);
            assert!(train_icon_size(kind, Some(0.0), 2.0) < close);
            assert!(train_icon_size(kind, None, 12.0) < close);
        }
        assert!(
            train_icon_size(TrainKind::HighSpeed, Some(0.0), 12.0)
                > train_icon_size(TrainKind::Suburban, Some(0.0), 12.0)
        );
    }

    // --- panel

    #[test]
    fn positions_go_stale_after_five_minutes() {
        assert_eq!(
            train_position_age(1_000, 1_040_000),
            Freshness { stale: false, label: "Position 40 s ago".into() }
        );
        assert!(!train_position_age(1_000, (1_000 + 300) * 1000).stale);
        assert_eq!(
            train_position_age(1_000, (1_000 + 720) * 1000),
            Freshness { stale: true, label: "Position 12 min old".into() }
        );
        // a report from a clock a little ahead reads as just now
        assert_eq!(train_position_age(1_000, 990_000).label, "Position 0 s ago");
    }

    #[test]
    fn delay_tones() {
        assert_eq!(delay_tone(None), None);
        assert_eq!(delay_tone(Some(-2)), Some(DelayTone::Good));
        assert_eq!(delay_tone(Some(4)), Some(DelayTone::Good));
        assert_eq!(delay_tone(Some(5)), Some(DelayTone::Warn));
        assert_eq!(delay_tone(Some(30)), Some(DelayTone::Bad));
        assert_eq!(delay_tone(Some(30)).map(DelayTone::as_str), Some("bad"));
    }

    #[test]
    fn every_source_has_a_name_and_a_credit() {
        for source in TRAIN_SOURCES {
            assert!(!train_source_name(source).is_empty());
            assert!(!train_credit(source).text.is_empty());
        }
        assert_eq!(train_credit(TrainSource::Trafikverket).url, "https://data.trafikverket.se/");
    }

    // --- stops

    fn at(planned: i64, actual: Option<i64>, estimated: Option<i64>) -> Option<StopTime> {
        Some(StopTime { planned: Some(planned), actual, estimated })
    }

    fn stop(arrival: Option<StopTime>, departure: Option<StopTime>) -> TrainStop {
        TrainStop { arrival, departure, ..TrainStop::default() }
    }

    fn stops() -> Vec<TrainStop> {
        vec![
            stop(None, at(1000, Some(1120), None)),
            stop(at(2000, Some(2060), None), at(2100, Some(2160), None)),
            stop(at(3000, None, Some(3300)), at(3060, None, None)),
            stop(at(4000, None, None), None),
        ]
    }

    #[test]
    fn a_stop_shows_its_departure_as_it_now_stands() {
        let stops = stops();
        assert_eq!(stop_view(&stops[0]), StopView { time: Some(1120), planned: Some(1000), late: Some(2), passed: true });
        assert_eq!(stop_view(&stops[2]), StopView { time: Some(3060), planned: Some(3060), late: None, passed: false });
        assert_eq!(stop_view(&stops[3]).time, Some(4000));
    }

    #[test]
    fn the_next_stop_follows_the_last_one_passed() {
        assert_eq!(next_stop(&stops()), Some(2));
        assert_eq!(next_stop(&[stop(None, at(1, None, None))]), Some(0));
        assert_eq!(next_stop(&[stop(at(1, Some(2), None), None)]), None);
    }

    #[test]
    fn operators_have_colours_of_their_own_others_their_kinds() {
        let of = |operator: &str| {
            train_color_of(&train(|t| {
                t.kind = TrainKind::Regional;
                t.operator = Some(operator.into());
            }))
        };
        assert_ne!(of("SJ"), train_color(TrainKind::Regional));
        assert_eq!(of("Someone else"), train_color(TrainKind::Regional));
        let icon = train_icon(TrainKind::Regional, Some(90.0), Some("Mälartåg"));
        assert!(Regex::new(r"^car-op[0-9]+$").unwrap().is_match(&icon), "{icon}");
        assert_eq!(train_icon(TrainKind::Regional, None, None), "box-regional");
    }

    #[test]
    fn operator_rows_of_real_names() {
        // doc 02 §14.3.11: real operator names land on the rows the TypeScript gives them
        for (operator, row) in [
            ("SJ Götalandståg", Some(0)),
            ("SJ", Some(0)),
            ("sj", Some(0)),
            // JavaScript's \b is ASCII: "ö" after "SJ" ends the word there
            ("SJÖ", Some(0)),
            ("SL", Some(1)),
            ("Storstockholms Lokaltrafik", Some(1)),
            ("Mälartåg", Some(2)),
            ("MÄLARTÅG", Some(2)),
            ("Västtrafik", Some(3)),
            ("Skånetrafiken", Some(4)),
            ("Pågatåg", Some(4)),
            ("Öresundståg", Some(5)),
            ("Arlanda Express", Some(6)),
            ("Snälltåget", Some(7)),
            ("Norrtåg", Some(8)),
            ("Vy", Some(9)),
            ("Vy Tåg", Some(9)),
            ("  Vy  ", Some(9)),
            ("Tåg i Bergslagen", Some(10)),
            ("Upptåget", Some(11)),
            ("UL", Some(11)),
            ("Östgötatrafiken", Some(12)),
            ("X-Trafik", Some(13)),
            ("Krösatåg", Some(14)),
            ("Kalmar länstrafik", Some(14)),
            ("Värmlandstrafik", Some(15)),
            ("Vyborg", None),
            ("Slussen", None),
            ("Someone else", None),
            ("", None),
        ] {
            assert_eq!(operator_index(Some(operator)), row, "{operator:?}");
        }
        assert_eq!(operator_index(None), None);
        assert_eq!(train_icon(TrainKind::Regional, Some(0.0), Some("Vy")), "car-op9");
    }

    // --- products

    #[test]
    fn a_swedish_trains_product_is_its_name_without_the_number() {
        let product = |name: &str| train_product(&train(|t| t.name = name.into()));
        assert_eq!(product("Öresundståg 1322").as_deref(), Some("Öresundståg"));
        assert_eq!(product("SJ Snabbtåg 531").as_deref(), Some("SJ Snabbtåg"));
        assert_eq!(product("Train 1322"), None);
        // as `/\s+\S+$/`: nothing to strip without a space before the last word, or after it
        // (where the trim still leaves a product that differs from the name)
        assert_eq!(product("Pendeltåg"), None);
        assert_eq!(product("Pendeltåg  ").as_deref(), Some("Pendeltåg"));
        assert_eq!(product(" 2245"), None);
        assert_eq!(product("SJ  Regional\u{a0}88").as_deref(), Some("SJ  Regional"));
    }

    // --- trainRunning

    #[test]
    fn a_train_is_dropped_30_s_after_it_stands_at_its_last_stop() {
        assert!(train_running(&train(|_| {}), 1_000_000_000_000));
        assert!(train_running(&train(|t| t.ended_unix = Some(1000)), 1_030_000));
        assert!(!train_running(&train(|t| t.ended_unix = Some(1000)), 1_030_001));
    }

    // --- blockAt
    // two tracks running east at Klostergården, 4.5 m apart: block "a" on the north one runs on
    // through a crossover at 1 km onto the south one, where block "b" already runs

    const LAT0: f64 = 55.6954;
    const LON0: f64 = 13.1727;

    fn spot(east: f64, north: f64) -> [f64; 2] {
        let m = 1.0 / 111_320.0;
        [LON0 + (east * m) / (LAT0 * PI / 180.0).cos(), LAT0 + north * m]
    }

    fn block(id: &str, line: &[[f64; 2]]) -> TrainBlock {
        TrainBlock { id: id.into(), signal: None, lines: vec![line.to_vec()] }
    }

    fn blocks() -> Vec<TrainBlock> {
        vec![
            block("a", &[spot(0.0, 4.5), spot(1_000.0, 4.5), spot(1_050.0, 0.0), spot(2_000.0, 0.0)]),
            block("b", &[spot(0.0, 0.0), spot(2_000.0, 0.0)]),
        ]
    }

    /// Drawn up to a metre either side of its track as it moves along.
    fn jitter(k: i32) -> f64 {
        (if k % 2 != 0 { 1.0 } else { -1.0 }) * ((f64::from(k) * 0.37) % 1.0)
    }

    #[test]
    fn a_train_stays_in_its_own_block_where_another_runs_on_beside_it_then_onto_its_track() {
        let blocks = blocks();
        let mut was: Option<String> = None;
        for k in 0..100 {
            was = block_at(spot(20.0 * f64::from(k), jitter(k)), &blocks, was.as_deref()).map(str::to_owned);
            assert_eq!(was.as_deref(), Some("b"), "k = {k}");
        }
    }

    #[test]
    fn a_train_going_through_the_crossover_stays_in_the_block_it_came_in() {
        // (whichever block comes first in the list)
        let mut reversed = blocks();
        reversed.reverse();
        let mut was: Option<String> = None;
        for k in 0..100 {
            let east = 20.0 * f64::from(k);
            let north = if east < 1_000.0 {
                4.5
            } else if east < 1_050.0 {
                4.5 - (east - 1_000.0) * 0.09
            } else {
                0.0
            };
            was = block_at(spot(east, north + jitter(k)), &reversed, was.as_deref()).map(str::to_owned);
            assert_eq!(was.as_deref(), Some("a"), "k = {k}");
        }
    }

    #[test]
    fn a_train_goes_on_into_the_next_block_and_is_in_none_off_the_track() {
        let mut next = blocks();
        next.push(block("c", &[spot(2_000.0, 0.0), spot(3_000.0, 0.0)]));
        assert_eq!(block_at(spot(2_010.0, 0.5), &next, Some("b")), Some("c"));
        assert_eq!(block_at(spot(500.0, 4.5), &next, Some("b")), Some("a"));
        assert_eq!(block_at(spot(500.0, 20.0), &next, None), None);
    }

    #[test]
    fn javascript_space_is_not_rusts_white_space() {
        assert!(is_js_space(' ') && is_js_space('\u{a0}') && is_js_space('\u{feff}') && is_js_space('\u{3000}'));
        assert!(!is_js_space('\u{85}') && !is_js_space('x'));
        assert!(js_min(f64::NAN, 1.0).is_nan());
        assert!(js_max(-1.0, f64::NAN).is_nan());
        assert!(js_max(0.0, -0.0).is_sign_positive());
    }
}
