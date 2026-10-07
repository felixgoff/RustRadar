//! gRPC-web API.
//!
//! Endpoint: `https://data-feed.flightradar24.com`
//!
//! Service name: `fr24.feed.api.v1.Feed`
//!
//! Methods:
//!
//! - `LiveFeed`
//! - `Playback`
//! - `NearestFlights`
//! - `LiveFlightsStatus`
//! - `FollowFlight` (server streaming)
//! - `TopFlights`
//! - `LiveTrail` (unstable: returns empty `DATA` frame as of Sep 2024)
//! - `HistoricTrail` (unstable: returns empty `DATA` frame)
//! - `FetchSearchIndex` (unstable: gateway timeout)
//! - `FlightDetails`
//! - `PlaybackFlight`

use futures_util::{Stream, StreamExt};
use prost::Message;
use reqwest::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::error::{GrpcError, GrpcErrorKind, Result};
use crate::headers::now_s;
use crate::proto::v1::common::{
    self, DataSource, DelayStatus, FlightStage, Icon, RestrictionVisibility, Service, Status,
    TrafficType,
};
use crate::proto::v1::{
    FlightDetailsRequest, FlightDetailsResponse, FollowFlightRequest, FollowFlightResponse,
    FollowedFlight, Geolocation, LiveFeedRequest, LiveFlightStatus, LiveFlightsStatusRequest,
    LocationBoundaries, NearbyFlight, NearestFlightsRequest, PlaybackFlightRequest,
    PlaybackFlightResponse, PlaybackRequest, TopFlightsRequest, VisibilitySettings,
};
use crate::proto::{self, Frame, FrameDecoder};

pub const GRPC_URL: &str = "https://data-feed.flightradar24.com/fr24.feed.api.v1.Feed";

//
// helpers
//

fn method_url(method: &str) -> String {
    format!("{GRPC_URL}/{method}")
}

/// Rejects "trailers-only" responses, where the gRPC status is sent in the
/// HTTP headers instead of a trailers frame.
fn check_header_status(headers: &HeaderMap) -> std::result::Result<(), GrpcError> {
    proto::status_from_pairs(headers.iter().filter_map(|(k, v)| {
        let k = k.as_str();
        k.starts_with("grpc-")
            .then(|| Some((k.to_owned(), v.to_str().ok()?.to_owned())))
            .flatten()
    }))
}

async fn send(
    client: &reqwest::Client,
    headers: &HeaderMap,
    method: &str,
    message: &impl Message,
) -> Result<reqwest::Response> {
    let response = client
        .post(method_url(method))
        .headers(headers.clone())
        .body(proto::encode_message(message))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(crate::Error::HttpStatus(response.status()));
    }
    check_header_status(response.headers())?;
    Ok(response)
}

/// Make a unary gRPC-web call.
pub async fn unary<Resp: Message + Default>(
    client: &reqwest::Client,
    headers: &HeaderMap,
    method: &str,
    message: &impl Message,
) -> Result<Resp> {
    let body = send(client, headers, method, message)
        .await?
        .bytes()
        .await?;
    proto::parse_data(&body)
}

/// Make a server-streaming gRPC-web call, yielding one message per `DATA`
/// frame until the trailers frame or the end of the response body.
pub async fn server_stream<Resp: Message + Default, Req: Message>(
    client: &reqwest::Client,
    headers: &HeaderMap,
    method: &str,
    message: &Req,
) -> Result<impl Stream<Item = Result<Resp>> + use<Resp, Req>> {
    // The proxy buffers compressed streams: over HTTP/2 the response headers
    // never arrive, over HTTP/1.1 frames are held back. Request it uncompressed.
    let mut headers = headers.clone();
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    let response = send(client, &headers, method, message).await?;
    Ok(decode_stream(response.bytes_stream()))
}

/// Splits a gRPC-web body stream into decoded messages.
pub fn decode_stream<Resp, S, B, E>(body: S) -> impl Stream<Item = Result<Resp>>
where
    Resp: Message + Default,
    S: Stream<Item = std::result::Result<B, E>>,
    B: AsRef<[u8]>,
    E: Into<crate::Error>,
{
    let state = (Box::pin(body), FrameDecoder::new(), false);
    futures_util::stream::unfold(state, |(mut body, mut decoder, done)| async move {
        if done {
            return None;
        }
        loop {
            match decoder.next_frame() {
                Some(Ok(Frame::Data(payload))) => {
                    let item = Resp::decode(payload).map_err(Into::into);
                    return Some((item, (body, decoder, false)));
                }
                Some(Ok(Frame::Trailers(Ok(())))) => return None,
                Some(Ok(Frame::Trailers(Err(e))) | Err(e)) => {
                    return Some((Err(e.into()), (body, decoder, true)));
                }
                None => {}
            }
            match body.next().await {
                Some(Ok(chunk)) => decoder.push(chunk.as_ref()),
                Some(Err(e)) => return Some((Err(e.into()), (body, decoder, true))),
                None if decoder.remaining() > 0 => {
                    let err = GrpcError::new(GrpcErrorKind::Malformed);
                    return Some((Err(err.into()), (body, decoder, true)));
                }
                None => return None,
            }
        }
    })
}

fn enum_name<E: TryFrom<i32>>(value: i32, name: impl Fn(&E) -> &'static str) -> String {
    E::try_from(value)
        .map(|e| name(&e).to_owned())
        .unwrap_or_else(|_| value.to_string())
}

/// Converts a hex flight ID (e.g. `"3962fcd2"` or `"0x3962fcd2"`) to an integer.
pub fn parse_flight_id(flight_id: &str) -> Result<u32> {
    let s = flight_id.trim();
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u32::from_str_radix(s, 16)
        .map_err(|e| crate::Error::InvalidArgument(format!("invalid flight id {flight_id:?}: {e}")))
}

/// Converts a flight ID to its lowercase hex representation, as used in URLs.
pub fn flight_id_hex(flight_id: u32) -> String {
    format!("{flight_id:x}")
}

//
// live feed
//

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    /// Latitude, minimum, degrees
    pub south: f32,
    /// Latitude, maximum, degrees
    pub north: f32,
    /// Longitude, minimum, degrees
    pub west: f32,
    /// Longitude, maximum, degrees
    pub east: f32,
}

/// Optional fields of the live feed, selected with a `FieldMask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveFeedField {
    Flight,
    Reg,
    Route,
    Type,
    /// Requires authentication.
    Squawk,
    /// Requires authentication.
    Vspeed,
    /// Requires authentication.
    Airspace,
    /// Requires authentication.
    LogoId,
    /// Requires authentication.
    Age,
}

impl LiveFeedField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flight => "flight",
            Self::Reg => "reg",
            Self::Route => "route",
            Self::Type => "type",
            Self::Squawk => "squawk",
            Self::Vspeed => "vspeed",
            Self::Airspace => "airspace",
            Self::LogoId => "logo_id",
            Self::Age => "age",
        }
    }
}

pub const DEFAULT_LIVE_FEED_FIELDS: [LiveFeedField; 4] = [
    LiveFeedField::Flight,
    LiveFeedField::Reg,
    LiveFeedField::Route,
    LiveFeedField::Type,
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveFeedParams {
    pub bounding_box: BoundingBox,
    /// Whether to include stats in the given area.
    pub stats: bool,
    /// Maximum number of flights (should be set to 1500 for unauthorized
    /// users, 2000 for authorized users).
    pub limit: i32,
    /// Maximum time since last message update, seconds.
    pub maxage: i32,
    /// Fields to include. For unauthenticated users, a maximum of 4 fields
    /// can be included.
    pub fields: Vec<LiveFeedField>,
    /// Categories to include, as [`Service`] values. Defaults to all.
    pub services: Vec<i32>,
    /// Data sources to include, as [`DataSource`] values. Defaults to the
    /// ten the web client sends.
    pub sources: Vec<i32>,
}

impl LiveFeedParams {
    pub fn new(bounding_box: BoundingBox) -> Self {
        Self {
            bounding_box,
            stats: false,
            limit: 1500,
            maxage: 14400,
            fields: DEFAULT_LIVE_FEED_FIELDS.to_vec(),
            services: (0..12).collect(),
            sources: (0..10).collect(),
        }
    }

    pub fn to_proto(&self) -> LiveFeedRequest {
        let bbox = self.bounding_box;
        LiveFeedRequest {
            bounds: Some(LocationBoundaries {
                north: bbox.north,
                south: bbox.south,
                west: bbox.west,
                east: bbox.east,
            }),
            settings: Some(VisibilitySettings {
                sources_list: self.sources.clone(),
                services_list: self.services.clone(),
                traffic_type: TrafficType::All as i32,
                only_restricted: Some(false),
            }),
            field_mask: Some(prost_types::FieldMask {
                paths: self.fields.iter().map(|f| f.as_str().to_owned()).collect(),
            }),
            highlight_mode: false,
            stats: Some(self.stats),
            limit: Some(self.limit),
            maxage: Some(self.maxage),
            restriction_mode: Some(RestrictionVisibility::NotVisible as i32),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveFeedPlaybackParams {
    pub live_feed: LiveFeedParams,
    /// Start timestamp, Unix seconds. `None` means "now".
    pub timestamp: Option<i64>,
    /// Duration of prefetch, `floor(7.5*(multiplier))` seconds.
    ///
    /// For 1x playback, this should be 7 seconds.
    pub duration: i32,
    /// High frequency mode
    pub hfreq: Option<i32>,
}

impl LiveFeedPlaybackParams {
    pub fn new(bounding_box: BoundingBox, timestamp: Option<i64>) -> Self {
        Self {
            live_feed: LiveFeedParams::new(bounding_box),
            timestamp,
            duration: 7,
            hfreq: None,
        }
    }

    pub fn to_proto(&self) -> PlaybackRequest {
        let timestamp = self
            .timestamp
            .unwrap_or_else(|| now_s() - i64::from(self.duration)) as i32;
        PlaybackRequest {
            live_feed_request: Some(self.live_feed.to_proto()),
            timestamp,
            prefetch: timestamp + self.duration,
            hfreq: self.hfreq,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NearestFlightsParams {
    /// Latitude, degrees, -90 to 90
    pub lat: f32,
    /// Longitude, degrees, -180 to 180
    pub lon: f32,
    /// Radius, metres
    pub radius: u32,
    /// Maximum number of aircraft to return
    pub limit: u32,
}

impl NearestFlightsParams {
    pub fn new(lat: f32, lon: f32) -> Self {
        Self {
            lat,
            lon,
            radius: 10_000,
            limit: 1500,
        }
    }

    pub fn to_proto(&self) -> NearestFlightsRequest {
        NearestFlightsRequest {
            location: Some(Geolocation {
                lat: self.lat,
                lon: self.lon,
            }),
            radius: self.radius,
            limit: self.limit,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveFlightsStatusParams {
    pub flight_ids: Vec<u32>,
}

impl LiveFlightsStatusParams {
    pub fn to_proto(&self) -> LiveFlightsStatusRequest {
        LiveFlightsStatusRequest {
            flight_ids_list: self.flight_ids.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowFlightParams {
    /// Must be live, or the response will contain an empty `DATA` frame error.
    pub flight_id: u32,
    /// [FAA LADD](https://www.faa.gov/pilots/ladd) visibility mode.
    pub restriction_mode: i32,
}

impl FollowFlightParams {
    pub fn new(flight_id: u32) -> Self {
        Self {
            flight_id,
            restriction_mode: RestrictionVisibility::NotVisible as i32,
        }
    }

    pub fn to_proto(&self) -> FollowFlightRequest {
        FollowFlightRequest {
            flight_id: self.flight_id,
            restriction_mode: self.restriction_mode,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopFlightsParams {
    /// Maximum number of top flights to return (1-10)
    pub limit: u32,
}

impl Default for TopFlightsParams {
    fn default() -> Self {
        Self { limit: 10 }
    }
}

impl TopFlightsParams {
    pub fn to_proto(&self) -> TopFlightsRequest {
        TopFlightsRequest { limit: self.limit }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlightDetailsParams {
    /// Must be live, or the response will contain an empty `DATA` frame error.
    pub flight_id: u32,
    /// [FAA LADD](https://www.faa.gov/pilots/ladd) visibility mode.
    pub restriction_mode: i32,
    /// Whether to include `flight_plan` and `aircraft_details` in the response.
    pub verbose: bool,
}

impl FlightDetailsParams {
    pub fn new(flight_id: u32) -> Self {
        Self {
            flight_id,
            restriction_mode: RestrictionVisibility::NotVisible as i32,
            verbose: true,
        }
    }

    pub fn to_proto(&self) -> FlightDetailsRequest {
        FlightDetailsRequest {
            flight_id: self.flight_id,
            restriction_mode: self.restriction_mode,
            verbose: self.verbose,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackFlightParams {
    /// Must not be live, or the response will contain an empty `DATA` frame error.
    pub flight_id: u32,
    /// Actual time of departure (ATD) of the historic flight, Unix seconds.
    pub timestamp: u64,
}

impl PlaybackFlightParams {
    pub fn to_proto(&self) -> PlaybackFlightRequest {
        PlaybackFlightRequest {
            flight_id: self.flight_id,
            timestamp: self.timestamp,
            restriction_mode: RestrictionVisibility::NotVisible as i32,
        }
    }
}

//
// records: flat, serializable views of the protobuf messages
//

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecentPositionRecord {
    /// Offset from the current latitude, 1e5 degrees
    pub delta_lat: i32,
    /// Offset from the current longitude, 1e5 degrees
    pub delta_lon: i32,
    /// Delta time, milliseconds
    pub delta_ms: u32,
}

fn position_buffer(pb: Option<&common::PositionBuffer>) -> Vec<RecentPositionRecord> {
    pb.map(|pb| {
        pb.recent_positions_list
            .iter()
            .map(|p| RecentPositionRecord {
                delta_lat: p.delta_lat,
                delta_lon: p.delta_lon,
                delta_ms: p.delta_ms,
            })
            .collect()
    })
    .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlightRecord {
    /// Last update, Unix milliseconds
    pub timestamp: u64,
    pub flightid: u32,
    pub latitude: f32,
    pub longitude: f32,
    /// True track, degrees clockwise from north
    pub track: i32,
    /// Barometric altitude, feet
    pub altitude: i32,
    /// Ground speed, knots
    pub ground_speed: i32,
    pub on_ground: bool,
    pub callsign: String,
    pub source: String,
    pub registration: String,
    /// IATA flight number, e.g. `CX8747`.
    pub flight_number: String,
    pub origin: String,
    pub destination: String,
    pub typecode: String,
    pub icon: String,
    pub eta: i32,
    pub squawk: i32,
    /// Feet per minute
    pub vertical_speed: i32,
    pub position_buffer: Vec<RecentPositionRecord>,
}

impl From<&common::Flight> for FlightRecord {
    fn from(f: &common::Flight) -> Self {
        let extra = f.extra_info.clone().unwrap_or_default();
        let route = extra.route.unwrap_or_default();
        Self {
            timestamp: f.timestamp_ms,
            flightid: f.flightid as u32,
            latitude: f.lat,
            longitude: f.lon,
            track: f.track,
            altitude: f.alt,
            ground_speed: f.speed,
            on_ground: f.on_ground,
            callsign: f.callsign.clone(),
            source: enum_name(f.source, DataSource::as_str_name),
            registration: extra.reg,
            flight_number: extra.flight,
            origin: route.from,
            destination: route.to,
            typecode: extra.r#type,
            icon: enum_name(f.icon, Icon::as_str_name),
            eta: extra.schedule.map(|s| s.eta).unwrap_or_default(),
            squawk: extra.squawk,
            vertical_speed: extra.vspeed,
            position_buffer: position_buffer(f.position_buffer.as_ref()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NearbyFlightRecord {
    #[serde(flatten)]
    pub flight: FlightRecord,
    /// Distance from the location, metres
    pub distance: u32,
}

impl From<&NearbyFlight> for NearbyFlightRecord {
    fn from(nf: &NearbyFlight) -> Self {
        Self {
            flight: nf
                .flight
                .as_ref()
                .map(FlightRecord::from)
                .unwrap_or_else(|| FlightRecord::from(&common::Flight::default())),
            distance: nf.distance,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveFlightStatusRecord {
    pub flight_id: u32,
    pub latitude: f32,
    pub longitude: f32,
    pub status: String,
    pub squawk: u32,
}

impl From<&LiveFlightStatus> for LiveFlightStatusRecord {
    fn from(fs: &LiveFlightStatus) -> Self {
        let data = fs.data.unwrap_or_default();
        Self {
            flight_id: fs.flight_id,
            latitude: data.lat,
            longitude: data.lon,
            status: enum_name(data.status, Status::as_str_name),
            squawk: data.squawk,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopFlightRecord {
    pub flight_id: u32,
    pub live_clicks: u32,
    pub total_clicks: u32,
    pub flight_number: String,
    pub callsign: String,
    pub squawk: u32,
    pub from_iata: String,
    pub from_city: String,
    pub to_iata: String,
    pub to_city: String,
    #[serde(rename = "type")]
    pub typecode: String,
    pub full_description: String,
}

impl From<&FollowedFlight> for TopFlightRecord {
    fn from(ff: &FollowedFlight) -> Self {
        Self {
            flight_id: ff.flight_id,
            live_clicks: ff.live_clicks,
            total_clicks: ff.total_clicks,
            flight_number: ff.flight_number.clone(),
            callsign: ff.callsign.clone(),
            squawk: ff.squawk,
            from_iata: ff.from_iata.clone(),
            from_city: ff.from_city.clone(),
            to_iata: ff.to_iata.clone(),
            to_city: ff.to_city.clone(),
            typecode: ff.r#type.clone(),
            full_description: ff.full_description.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrailPointRecord {
    /// Unix seconds
    pub timestamp: u64,
    pub latitude: f32,
    pub longitude: f32,
    /// Feet
    pub altitude: i32,
    /// Knots
    pub ground_speed: u32,
    /// Degrees clockwise from north
    pub track: u32,
    /// Feet per minute
    pub vertical_speed: i32,
    pub source: i32,
}

impl From<&common::TrailPoint> for TrailPointRecord {
    fn from(tp: &common::TrailPoint) -> Self {
        Self {
            timestamp: tp.snapshot_id,
            latitude: tp.lat,
            longitude: tp.lon,
            altitude: tp.altitude,
            ground_speed: tp.spd,
            track: tp.heading,
            vertical_speed: tp.vspd,
            source: tp.source,
        }
    }
}

/// Enhanced Mode-S data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmsRecord {
    pub ias: i32,
    pub tas: i32,
    pub mach: i32,
    pub mcp: i32,
    pub fms: i32,
    pub oat: i32,
    pub qnh: i32,
    pub wind_dir: i32,
    pub wind_speed: i32,
    pub altitude_gps: i32,
    pub agpsdiff: i32,
    pub apflags: i32,
    pub rs: i32,
}

impl From<&common::EmsInfo> for EmsRecord {
    fn from(ems: &common::EmsInfo) -> Self {
        Self {
            ias: ems.ias,
            tas: ems.tas,
            mach: ems.mach,
            mcp: ems.amcp,
            fms: ems.afms,
            oat: ems.oat,
            qnh: ems.qnh,
            wind_dir: ems.wind_dir,
            wind_speed: ems.wind_speed,
            altitude_gps: ems.agps,
            agpsdiff: ems.agpsdiff,
            apflags: ems.apflags,
            rs: ems.rs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AircraftImageRecord {
    pub url: String,
    pub copyright: String,
    pub thumbnail: String,
    pub medium: String,
    pub large: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AircraftRecord {
    /// ICAO 24-bit address
    pub icao_address: u32,
    pub reg: String,
    pub typecode: String,
    /// e.g. `Airbus A350-941`
    pub full_description: String,
    pub service: String,
    pub registered_owners: String,
    /// Manufacturer serial number, when published.
    pub msn: Option<String>,
    /// e.g. `2017-06-28`, when published.
    pub birth_date: Option<String>,
    /// Years, when published.
    pub age: Option<u32>,
    pub images: Vec<AircraftImageRecord>,
}

impl From<&common::AircraftInfo> for AircraftRecord {
    fn from(a: &common::AircraftInfo) -> Self {
        Self {
            icao_address: a.icao_address,
            reg: a.reg.clone(),
            typecode: a.r#type.clone(),
            full_description: a.full_description.clone(),
            service: enum_name(a.service, Service::as_str_name),
            registered_owners: a.registered_owners.clone(),
            msn: Some(a.msn.clone()).filter(|m| a.msn_available && !m.is_empty()),
            birth_date: Some(a.ac_birth_date.clone()).filter(|d| !d.is_empty()),
            age: (a.age_available && a.ac_birth_date.len() > 3).then_some(a.ac_age),
            images: a
                .images_list
                .iter()
                .map(|i| AircraftImageRecord {
                    url: i.url.clone(),
                    copyright: i.copyright.clone(),
                    thumbnail: i.thumbnail.clone(),
                    medium: i.medium.clone(),
                    large: i.large.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRecord {
    pub flight_number: String,
    /// Flightradar24 id of the operating airline.
    pub operated_by_id: u32,
    /// Flightradar24 id of the airline whose livery the aircraft wears.
    pub painted_as_id: u32,
    pub origin_id: u32,
    pub destination_id: u32,
    pub diverted_id: u32,
    /// Unix seconds
    pub scheduled_departure: u32,
    pub scheduled_arrival: u32,
    pub actual_departure: u32,
    pub actual_arrival: u32,
    pub arr_terminal: String,
    pub arr_gate: String,
}

impl From<&common::ScheduleInfo> for ScheduleRecord {
    fn from(s: &common::ScheduleInfo) -> Self {
        Self {
            flight_number: s.flight_number.clone(),
            operated_by_id: s.operated_by_id,
            painted_as_id: s.painted_as_id,
            origin_id: s.origin_id,
            destination_id: s.destination_id,
            diverted_id: s.diverted_to_id,
            scheduled_departure: s.scheduled_departure,
            scheduled_arrival: s.scheduled_arrival,
            actual_departure: s.actual_departure,
            actual_arrival: s.actual_arrival,
            arr_terminal: s.arr_terminal.clone(),
            arr_gate: s.arr_gate.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlightProgressRecord {
    /// Metres
    pub traversed_distance: u32,
    /// Metres
    pub remaining_distance: u32,
    /// Seconds
    pub elapsed_time: u32,
    /// Seconds
    pub remaining_time: u32,
    /// Unix seconds
    pub eta: u32,
    /// Metres
    pub great_circle_distance: u32,
    /// Seconds
    pub mean_flight_time: u32,
    pub flight_stage: String,
    pub delay_status: String,
    pub progress_pct: u32,
}

impl From<&common::FlightProgress> for FlightProgressRecord {
    fn from(p: &common::FlightProgress) -> Self {
        Self {
            traversed_distance: p.traversed_distance,
            remaining_distance: p.remaining_distance,
            elapsed_time: p.elapsed_time,
            remaining_time: p.remaining_time,
            eta: p.eta,
            great_circle_distance: p.great_circle_distance,
            mean_flight_time: p.mean_flight_time,
            flight_stage: enum_name(p.flight_stage, FlightStage::as_str_name),
            delay_status: enum_name(p.delay_status, DelayStatus::as_str_name),
            progress_pct: p.progress_pct,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlightInfoRecord {
    /// Unix milliseconds
    pub timestamp_ms: u64,
    pub flightid: u32,
    pub latitude: f32,
    pub longitude: f32,
    pub track: i32,
    /// Feet
    pub altitude: i32,
    /// Knots
    pub ground_speed: i32,
    /// Feet per minute
    pub vertical_speed: i32,
    pub on_ground: bool,
    pub callsign: String,
    pub squawk: i32,
    pub source: String,
    /// Free-form, e.g. `Shannon UIR`
    pub airspace: String,
    pub ems: EmsRecord,
}

impl From<&common::ExtendedFlightInfo> for FlightInfoRecord {
    fn from(i: &common::ExtendedFlightInfo) -> Self {
        Self {
            timestamp_ms: i.timestamp_ms,
            flightid: i.flightid,
            latitude: i.lat,
            longitude: i.lon,
            track: i.track,
            altitude: i.alt,
            ground_speed: i.speed,
            vertical_speed: i.vspeed,
            on_ground: i.on_ground,
            callsign: i.callsign.clone(),
            squawk: i.squawk,
            source: enum_name(i.source, DataSource::as_str_name),
            airspace: i.airspace.clone(),
            ems: i.ems_info.as_ref().map(EmsRecord::from).unwrap_or_default(),
        }
    }
}

/// A filed flight plan: the route as resolved by Flightradar24's provider
/// against the current AIRAC cycle. Only sent to subscribers whose plan
/// includes flight plans; anonymous requests get none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightPlanRecord {
    /// ICAO code of the departure airport.
    pub departure: String,
    /// ICAO code of the destination airport.
    pub destination: String,
    /// ICAO field 15 route, e.g. `N0450F370 UMLAT T418 WELIN`.
    pub route: String,
    pub length: f64,
    /// Waypoint positions as sent: fixed-point latitude and longitude whose
    /// scale is not documented upstream (the app infers it).
    pub waypoints: Vec<[i32; 2]>,
    /// ICAO codes of the alternate airports.
    pub alternates: Vec<String>,
}

impl FlightPlanRecord {
    fn from_proto(p: &common::FlightPlan) -> Option<Self> {
        if p.waypoints_list.is_empty() && p.flight_plan_icao.is_empty() {
            return None;
        }
        let alternate = |a: &Option<common::AltArrival>| {
            a.as_ref()
                .and_then(|a| a.arrival.as_ref())
                .map(|f| f.airport.clone())
                .filter(|code| !code.is_empty())
        };
        Some(Self {
            departure: p.departure.clone(),
            destination: p.destination.clone(),
            route: p.flight_plan_icao.clone(),
            length: p.length,
            waypoints: p
                .waypoints_list
                .iter()
                .map(|w| [w.latitude, w.longitude])
                .collect(),
            alternates: [alternate(&p.alt_arrival_1), alternate(&p.alt_arrival_2)]
                .into_iter()
                .flatten()
                .collect(),
        })
    }
}

/// Details of a flight, from `FlightDetails`, `FollowFlight` or `PlaybackFlight`.
///
/// Sections are `None` when absent from the message: subsequent `FollowFlight`
/// messages only contain `schedule`, `progress` and `flight`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlightDetailsRecord {
    pub aircraft: Option<AircraftRecord>,
    pub schedule: Option<ScheduleRecord>,
    pub progress: Option<FlightProgressRecord>,
    pub flight: Option<FlightInfoRecord>,
    pub flight_plan: Option<FlightPlanRecord>,
    pub trail: Vec<TrailPointRecord>,
    pub position_buffer: Vec<RecentPositionRecord>,
}

impl From<&FlightDetailsResponse> for FlightDetailsRecord {
    fn from(r: &FlightDetailsResponse) -> Self {
        Self {
            aircraft: r.aircraft_info.as_ref().map(Into::into),
            schedule: r.schedule_info.as_ref().map(Into::into),
            progress: r.flight_progress.as_ref().map(Into::into),
            flight: r.flight_info.as_ref().map(Into::into),
            flight_plan: r
                .flight_plan
                .as_ref()
                .and_then(FlightPlanRecord::from_proto),
            trail: r.flight_trail_list.iter().map(Into::into).collect(),
            position_buffer: position_buffer(r.position_buffer.as_ref()),
        }
    }
}

impl From<&FollowFlightResponse> for FlightDetailsRecord {
    fn from(r: &FollowFlightResponse) -> Self {
        Self {
            aircraft: r.aircraft_info.as_ref().map(Into::into),
            schedule: r.schedule_info.as_ref().map(Into::into),
            progress: r.flight_progress.as_ref().map(Into::into),
            flight: r.flight_info.as_ref().map(Into::into),
            flight_plan: r
                .flight_plan
                .as_ref()
                .and_then(FlightPlanRecord::from_proto),
            trail: r.flight_trail_list.iter().map(Into::into).collect(),
            position_buffer: position_buffer(r.position_buffer.as_ref()),
        }
    }
}

impl From<&PlaybackFlightResponse> for FlightDetailsRecord {
    fn from(r: &PlaybackFlightResponse) -> Self {
        Self {
            aircraft: r.aircraft_info.as_ref().map(Into::into),
            schedule: r.schedule_info.as_ref().map(Into::into),
            progress: r.flight_progress.as_ref().map(Into::into),
            flight: r.flight_info.as_ref().map(Into::into),
            flight_plan: None,
            trail: r.flight_trail_list.iter().map(Into::into).collect(),
            position_buffer: Vec::new(),
        }
    }
}

impl FlightDetailsRecord {
    /// Merge a subsequent `FollowFlight` message into this record: sections
    /// present in `update` replace the current ones, and the new position is
    /// appended to the trail.
    pub fn apply_update(&mut self, update: FlightDetailsRecord) {
        let FlightDetailsRecord {
            aircraft,
            schedule,
            progress,
            flight,
            flight_plan,
            trail,
            position_buffer,
        } = update;
        if aircraft.is_some() {
            self.aircraft = aircraft;
        }
        if flight_plan.is_some() {
            self.flight_plan = flight_plan;
        }
        if schedule.is_some() {
            self.schedule = schedule;
        }
        if progress.is_some() {
            self.progress = progress;
        }
        if !trail.is_empty() {
            self.trail = trail;
        } else if let Some(f) = &flight {
            let timestamp = f.timestamp_ms / 1000;
            if self.trail.last().is_none_or(|p| p.timestamp < timestamp) {
                self.trail.push(TrailPointRecord {
                    timestamp,
                    latitude: f.latitude,
                    longitude: f.longitude,
                    altitude: f.altitude,
                    ground_speed: f.ground_speed.max(0) as u32,
                    track: f.track.rem_euclid(360) as u32,
                    vertical_speed: f.vertical_speed,
                    source: 0,
                });
            }
        }
        if flight.is_some() {
            self.flight = flight;
        }
        if !position_buffer.is_empty() {
            self.position_buffer = position_buffer;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flight_ids() {
        assert_eq!(parse_flight_id("3962fcd2").unwrap(), 962788562);
        assert_eq!(parse_flight_id("0x3962FCD2").unwrap(), 962788562);
        assert!(parse_flight_id("nope").is_err());
        assert_eq!(flight_id_hex(962788562), "3962fcd2");
    }

    #[test]
    fn live_feed_request() {
        let bbox = crate::bbox::FRANCE_UIR;
        let req = LiveFeedParams::new(bbox).to_proto();
        assert_eq!(req.settings.as_ref().unwrap().sources_list.len(), 10);
        assert_eq!(
            req.field_mask.unwrap().paths,
            ["flight", "reg", "route", "type"]
        );
        assert_eq!(req.limit, Some(1500));
    }

    #[tokio::test]
    async fn stream_decoding() {
        use crate::proto::encode_message;
        use crate::proto::v1::TopFlightsRequest;

        let mut body = encode_message(&TopFlightsRequest { limit: 1 });
        body.extend(encode_message(&TopFlightsRequest { limit: 2 }));
        let trailers = b"grpc-status:0\r\n";
        body.push(0x80);
        body.extend((trailers.len() as u32).to_be_bytes());
        body.extend(trailers);
        let chunks: Vec<std::result::Result<Vec<u8>, crate::Error>> =
            body.chunks(4).map(|c| Ok(c.to_vec())).collect();
        let items: Vec<Result<TopFlightsRequest>> =
            decode_stream(futures_util::stream::iter(chunks))
                .collect()
                .await;
        let limits: Vec<u32> = items.into_iter().map(|r| r.unwrap().limit).collect();
        assert_eq!(limits, [1, 2]);
    }
}
