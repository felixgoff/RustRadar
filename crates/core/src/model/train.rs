//! `api.ts` train DTOs (S1.5): trains, their stops, routes, signal blocks, pictures, the
//! bridges and tunnels of `bridges.ts`, and the road pieces of `road_widths`.
//!
//! Wire names and number types as the backend sends them (`RS:trains.rs`, `RS:train_class.rs`,
//! `RS:trafikverket.rs`, `RS:train_media.rs`, `RS:blocks.rs`, `RS:rail.rs`, `RS:roads.rs`).

use serde::{Deserialize, Serialize};

/// Where a train comes from: one regional public feed each (`api.ts:601`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrainSource {
    #[default]
    Trafikverket,
}

impl TrainSource {
    /// The name on the wire, which also prefixes the ids of the source's trains.
    pub fn as_str(self) -> &'static str {
        match self {
            TrainSource::Trafikverket => "trafikverket",
        }
    }
}

/// What kind of service a train is (`api.ts:603`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrainKind {
    HighSpeed,
    Intercity,
    Regional,
    Suburban,
    /// Also any kind this build does not know: the TypeScript drew those as "other"
    /// (`KINDS[kind] ?? KINDS.other`, `trains.ts:23`).
    #[serde(other)]
    Other,
}

/// How a train's cars are drawn, from the vehicles its product runs with (`api.ts:605`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrainShape {
    /// A long, pointed nose.
    HighSpeed,
    /// Two rows of windows.
    DoubleDeck,
    /// A multiple unit with a short, rounded nose.
    Regional,
    /// A locomotive in front of its coaches.
    Loco,
    /// A single railcar with a blunt cab.
    Railcar,
}

/// A position report as the dev map marks it (`api.ts:609`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sighting {
    pub lat: f64,
    pub lon: f64,
    pub unix_ms: i64,
    pub speed_kmh: Option<f32>,
    /// The newest report; else the one before it, the train is drawn from.
    pub latest: bool,
}

/// A bridge or a tunnel on the track (`bridges.ts:220`): its line (`[lon, lat]`), and whether
/// it goes on as one past each end (cut by a tile's edge, or mapped in pieces).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Structure {
    pub tunnel: bool,
    pub line: Vec<[f64; 2]>,
    pub joined: [bool; 2],
}

/// A train on the move, merged from regional public feeds (`api.ts:618`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Train {
    /// Source-prefixed and stable while the train runs, e.g. `trafikverket:2026-10-13:523`.
    pub id: String,
    pub source: TrainSource,
    /// e.g. "SJ Snabbtåg 523", "Pendeltåg 2245".
    pub name: String,
    pub kind: TrainKind,
    pub lat: f64,
    pub lon: f64,
    /// Degrees clockwise from north; none when the source has none (or it stands still).
    pub heading: Option<f32>,
    pub speed_kmh: Option<f32>,
    pub destination: Option<String>,
    pub operator: Option<String>,
    /// Minutes late at the last or next stop (negative: early).
    pub delay_minutes: Option<i32>,
    /// When the position was reported, Unix seconds.
    pub position_unix: i64,
    /// Where it is drawn as `(lat, lon, unix_ms)`, in time order, from about now on (along the
    /// track, for trains on one); empty when there is nothing to move it by.
    pub frames: Vec<(f64, f64, i64)>,
    /// Dev builds: its latest report and the one before. Left out when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<Sighting>,
    /// Dev builds: where the real train most likely is, as `frames`. Left out when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub predicted: Vec<(f64, f64, i64)>,
    /// How many cars (and locomotives) it runs with, where its operator says.
    #[serde(default)]
    pub cars: Option<u32>,
    /// The track it has come along, `(lat, lon)` back from the start of `frames`, for its
    /// cars. Left out when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<(f64, f64)>,
    /// The bridges and tunnels along `tail` and `frames`, for its cars to be drawn up on or
    /// down in. Left out when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub structures: Vec<Structure>,
    /// When it came to a stand at its last stop, Unix seconds: the map drops it a little after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_unix: Option<i64>,
    /// How its cars are drawn; regional where not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<TrainShape>,
    /// The 3D model its cars are drawn with, where there is one for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// A time at a stop, Unix seconds: as planned, as now expected, as it happened (`api.ts:669`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopTime {
    pub planned: Option<i64>,
    pub estimated: Option<i64>,
    pub actual: Option<i64>,
}

/// One stop of a train's run (`api.ts:676`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainStop {
    pub name: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// None at the first stop.
    pub arrival: Option<StopTime>,
    /// None at the last stop.
    pub departure: Option<StopTime>,
    pub track: Option<String>,
    pub canceled: bool,
    /// Notices at the stop: delay causes, replacement buses.
    pub notes: Vec<String>,
}

/// A photo of a train, from the Wikipedia article on its product or operator (`api.ts:691`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainPhoto {
    pub url: String,
    /// The article it comes from.
    pub title: String,
    pub page_url: String,
}

/// Pictures for the train panel, from Wikimedia: a photo of the train and its operator's logo,
/// when found (`api.ts:699`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainMedia {
    pub photo: Option<TrainPhoto>,
    pub logo: Option<String>,
}

/// Live news of the followed train (`api.ts:708`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TrainEvent {
    /// A new position (and with it the way ahead).
    Train { train: Box<Train> },
    /// The stops with their times as they now stand.
    Stops { stops: Vec<TrainStop> },
}

/// A place on a train's way ahead: a station it stops at, or one it passes; at its platform
/// where the track is known (`api.ts:720`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteStop {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub track: Option<String>,
    pub stops: bool,
}

/// A train's way ahead along the track, from where it is to its last station (`api.ts:722`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainRoute {
    /// Stretches of track, `[lon, lat]`; the first starts at the train. Gaps where the track
    /// isn't known yet, never straight lines.
    pub segments: Vec<Vec<[f64; 2]>>,
    /// The places still ahead.
    pub stops: Vec<RouteStop>,
    /// False while some track is still loading: ask again shortly.
    pub complete: bool,
}

/// A signal block: from a main signal to the next its trains meet, `[lon, lat]` stretches,
/// branching where the track does (`api.ts:732`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainBlock {
    pub id: String,
    pub signal: Option<String>,
    pub lines: Vec<Vec<[f64; 2]>>,
}

/// The answer of `train_blocks` (`api.ts:734`): the blocks starting in an area, and the bridges
/// and tunnels round it; while `complete` is false the track or signals are still loading.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrainBlocks {
    pub blocks: Vec<TrainBlock>,
    pub structures: Vec<Structure>,
    pub complete: bool,
}

/// A stretch of a numbered Swedish road, from Trafikverket's road database: its line
/// (`[lon, lat]`), width (metres) and whether it is paved (`api.ts:737`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RoadPiece {
    pub line: Vec<[f64; 2]>,
    pub width: f32,
    pub paved: bool,
}

/// Whether the saved Trafikverket key works (`api.ts:754`): none saved, refused, or fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrafikverketKeyStatus {
    None,
    Invalid,
    Ok,
}

impl TrafikverketKeyStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TrafikverketKeyStatus::None => "none",
            TrafikverketKeyStatus::Invalid => "invalid",
            TrafikverketKeyStatus::Ok => "ok",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(wire: Value) -> T {
        let value: T = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&value).unwrap(), wire);
        value
    }

    fn train_wire() -> Value {
        json!({
            "id": "trafikverket:2026-10-13:523", "source": "trafikverket", "name": "SJ Snabbtåg 523",
            "kind": "highSpeed", "lat": 59.33, "lon": 18.06, "heading": 212.5, "speedKmh": 160.0,
            "destination": "Göteborg C", "operator": "SJ", "delayMinutes": -2, "positionUnix": 1_791_000_000,
            "frames": [[59.33, 18.06, 1_791_000_000_000i64], [59.329, 18.059, 1_791_000_005_000i64]],
            "cars": null,
        })
    }

    #[test]
    fn train_round_trips_without_what_release_builds_leave_out() {
        let train: Train = round_trip(train_wire());
        assert_eq!(train.source, TrainSource::Trafikverket);
        assert_eq!(train.kind, TrainKind::HighSpeed);
        assert_eq!(train.frames[1], (59.329, 18.059, 1_791_000_005_000));
        assert!(train.reports.is_empty() && train.predicted.is_empty() && train.tail.is_empty());
        assert_eq!((train.cars, train.ended_unix, train.shape, train.model.as_ref()), (None, None, None, None));
    }

    #[test]
    fn train_round_trips_with_everything() {
        let mut wire = train_wire();
        wire["reports"] = json!([
            {"lat": 59.33, "lon": 18.06, "unixMs": 1_791_000_000_000i64, "speedKmh": 160.0, "latest": true},
            {"lat": 59.34, "lon": 18.07, "unixMs": 1_790_999_990_000i64, "speedKmh": null, "latest": false},
        ]);
        wire["predicted"] = json!([[59.33, 18.06, 1_791_000_000_000i64]]);
        wire["cars"] = json!(7);
        wire["tail"] = json!([[59.331, 18.061], [59.332, 18.062]]);
        wire["structures"] = json!([{"tunnel": true, "line": [[18.06, 59.33], [18.05, 59.32]], "joined": [false, true]}]);
        wire["endedUnix"] = json!(1_791_003_600);
        wire["shape"] = json!("doubleDeck");
        wire["model"] = json!("x2000");
        let train: Train = round_trip(wire);
        assert_eq!(train.shape, Some(TrainShape::DoubleDeck));
        assert_eq!(train.tail[1], (59.332, 18.062));
        assert_eq!(train.structures[0].joined, [false, true]);
        assert!(train.reports[0].latest);
    }

    #[test]
    fn train_without_a_cars_field_has_none() {
        // `cars?: number | null` in api.ts
        let mut wire = train_wire();
        wire.as_object_mut().unwrap().remove("cars");
        let train: Train = serde_json::from_value(wire).unwrap();
        assert_eq!(train.cars, None);
    }

    #[test]
    fn train_enums_on_the_wire() {
        for (kind, name) in [
            (TrainKind::HighSpeed, "highSpeed"),
            (TrainKind::Intercity, "intercity"),
            (TrainKind::Regional, "regional"),
            (TrainKind::Suburban, "suburban"),
            (TrainKind::Other, "other"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
            assert_eq!(serde_json::from_value::<TrainKind>(json!(name)).unwrap(), kind);
        }
        for (shape, name) in [
            (TrainShape::HighSpeed, "highSpeed"),
            (TrainShape::DoubleDeck, "doubleDeck"),
            (TrainShape::Regional, "regional"),
            (TrainShape::Loco, "loco"),
            (TrainShape::Railcar, "railcar"),
        ] {
            assert_eq!(serde_json::to_value(shape).unwrap(), json!(name));
            assert_eq!(serde_json::from_value::<TrainShape>(json!(name)).unwrap(), shape);
        }
        assert_eq!(serde_json::to_value(TrainSource::Trafikverket).unwrap(), json!(TrainSource::Trafikverket.as_str()));
        for status in [TrafikverketKeyStatus::None, TrafikverketKeyStatus::Invalid, TrafikverketKeyStatus::Ok] {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(status.as_str()));
            assert_eq!(serde_json::from_value::<TrafikverketKeyStatus>(json!(status.as_str())).unwrap(), status);
        }
    }

    #[test]
    fn an_unknown_train_kind_reads_as_other() {
        assert_eq!(serde_json::from_value::<TrainKind>(json!("nightTrain")).unwrap(), TrainKind::Other);
        assert_eq!(serde_json::from_value::<TrainKind>(json!("HighSpeed")).unwrap(), TrainKind::Other);
        let mut wire = train_wire();
        wire["kind"] = json!("museum");
        let train: Train = serde_json::from_value(wire).unwrap();
        assert_eq!(train.kind, TrainKind::Other);
        // and is sent on as "other"
        assert_eq!(serde_json::to_value(train).unwrap()["kind"], json!("other"));
    }

    fn stops_wire() -> Value {
        json!([
            {
                "name": "Stockholm C", "lat": 59.33, "lon": 18.06, "arrival": null,
                "departure": {"planned": 1_791_000_000, "estimated": null, "actual": 1_791_000_060},
                "track": "10", "canceled": false, "notes": [],
            },
            {
                "name": "Göteborg C", "lat": null, "lon": null,
                "arrival": {"planned": 1_791_011_000, "estimated": 1_791_011_300, "actual": null},
                "departure": null, "track": null, "canceled": true, "notes": ["Signalfel", "Buss ersätter"],
            },
        ])
    }

    #[test]
    fn train_stops_round_trip() {
        let stops: Vec<TrainStop> = round_trip(stops_wire());
        assert_eq!(stops[0].departure.unwrap().actual, Some(1_791_000_060));
        assert_eq!(stops[1].notes.len(), 2);
    }

    #[test]
    fn train_event_is_tagged_with_kind() {
        let event: TrainEvent = round_trip(json!({"kind": "train", "train": train_wire()}));
        assert!(matches!(event, TrainEvent::Train { ref train } if train.name == "SJ Snabbtåg 523"));
        let event: TrainEvent = round_trip(json!({"kind": "stops", "stops": stops_wire()}));
        assert!(matches!(event, TrainEvent::Stops { ref stops } if stops.len() == 2));
    }

    #[test]
    fn train_media_round_trips() {
        let media: TrainMedia = round_trip(json!({
            "photo": {"url": "https://upload.wikimedia.org/x.jpg", "title": "X 2000", "pageUrl": "https://sv.wikipedia.org/wiki/X_2000"},
            "logo": null,
        }));
        assert_eq!(media.photo.unwrap().title, "X 2000");
        assert_eq!(round_trip::<TrainMedia>(json!({"photo": null, "logo": "l.png"})).logo.as_deref(), Some("l.png"));
    }

    #[test]
    fn train_route_round_trips() {
        let route: TrainRoute = round_trip(json!({
            "segments": [[[18.06, 59.33], [18.05, 59.32]], [[17.9, 59.2], [17.8, 59.1]]],
            "stops": [
                {"name": "Flemingsberg", "lat": 59.22, "lon": 17.95, "track": "2", "stops": true},
                {"name": "Tullinge", "lat": 59.2, "lon": 17.9, "track": null, "stops": false},
            ],
            "complete": false,
        }));
        assert_eq!(route.segments[1][0], [17.9, 59.2]);
        assert!(!route.stops[1].stops);
    }

    #[test]
    fn train_blocks_round_trip() {
        let blocks: TrainBlocks = round_trip(json!({
            "blocks": [
                {"id": "b1", "signal": "Cst 101", "lines": [[[18.0, 59.0], [18.1, 59.1]], [[18.1, 59.1], [18.2, 59.2]]]},
                {"id": "b2", "signal": null, "lines": []},
            ],
            "structures": [{"tunnel": false, "line": [[18.0, 59.0], [18.01, 59.01]], "joined": [true, false]}],
            "complete": true,
        }));
        assert_eq!(blocks.blocks[0].lines.len(), 2);
        assert!(!blocks.structures[0].tunnel);
    }

    #[test]
    fn road_piece_round_trips() {
        let piece: RoadPiece = round_trip(json!({"line": [[13.0, 55.6], [13.01, 55.61]], "width": 7.5, "paved": true}));
        assert_eq!(piece.width, 7.5);
    }
}
