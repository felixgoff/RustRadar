//! A Rust port of [fr24](https://github.com/abc8747/fr24), a client for the
//! Flightradar24 gRPC-web and JSON APIs.
//!
//! ```no_run
//! # async fn run() -> fr24::Result<()> {
//! use fr24::{Fr24, grpc::{BoundingBox, LiveFeedParams, FlightRecord}};
//!
//! let fr24 = Fr24::new()?;
//! let bbox = BoundingBox { south: 42.0, north: 52.0, west: -8.0, east: 10.0 };
//! let response = fr24.live_feed(&LiveFeedParams::new(bbox)).await?;
//! for flight in &response.flights_list {
//!     println!("{:?}", FlightRecord::from(flight));
//! }
//! # Ok(()) }
//! ```
//!
//! Upstream's polars/parquet caching, CLI and TUI are not ported.

pub mod auth;
pub mod bbox;
mod error;
pub mod grpc;
pub mod headers;
pub mod json;
pub mod proto;
pub mod static_data;

use std::time::Duration;

use futures_util::{Stream, StreamExt};
use reqwest::header::HeaderMap;

pub use error::{Error, GrpcError, GrpcErrorKind, Result};

use auth::{Authentication, Credentials};
use grpc::{
    BoundingBox, FlightDetailsParams, FollowFlightParams, LiveFeedParams, LiveFeedPlaybackParams,
    LiveFlightsStatusParams, NearestFlightsParams, PlaybackFlightParams, TopFlightsParams,
};
use json::{AirportListParams, Find, FindParams, FlightListParams, PlaybackParams};
use proto::v1::{
    FetchSearchIndexRequest, FetchSearchIndexResponse, FlightDetailsResponse, FollowFlightResponse,
    HistoricTrailRequest, HistoricTrailResponse, LiveFeedResponse, LiveFlightsStatusResponse,
    LiveTrailRequest, LiveTrailResponse, NearestFlightsResponse, PlaybackFlightResponse,
    PlaybackResponse, TopFlightsResponse,
};

/// Maximum number of times a saturated slice is halved by [`Fr24::live_feed_world`].
pub const MAX_WORLD_SPLITS: usize = 3;

/// The result of [`Fr24::live_feed_world`].
#[derive(Debug, Default)]
pub struct WorldFeed {
    /// Flights, deduplicated by flight ID.
    pub flights: Vec<proto::common::Flight>,
    /// Latest server time across all responses, Unix milliseconds.
    pub server_time_ms: u64,
    /// Number of requests made.
    pub requests: usize,
    /// Slices that failed to load.
    pub errors: Vec<(BoundingBox, Error)>,
}

/// The high-level Flightradar24 client.
#[derive(Debug, Clone)]
pub struct Fr24 {
    http: reqwest::Client,
    auth: Option<Authentication>,
    device_id: String,
    grpc_headers: HeaderMap,
    json_headers: HeaderMap,
}

impl Fr24 {
    /// Create an anonymous client with sensible timeouts.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self::with_client(http))
    }

    /// Create an anonymous client from an existing [`reqwest::Client`].
    ///
    /// Note that a total request timeout on the client will also cut off
    /// long-lived streams such as [`Fr24::follow_flight`].
    pub fn with_client(http: reqwest::Client) -> Self {
        let device_id = headers::device_id().to_owned();
        Self {
            grpc_headers: headers::grpc_headers(&device_id, None),
            json_headers: headers::json_headers(&device_id),
            http,
            auth: None,
            device_id,
        }
    }

    /// Log in, reading credentials from the environment / config file if
    /// `creds` is `None`. Falls back to anonymous access if there are no
    /// credentials or the token has expired.
    pub async fn login(&mut self, creds: Option<Credentials>) -> Result<Option<&Authentication>> {
        let creds = match creds {
            Some(c) => Some(c),
            None => auth::get_credentials(auth::default_config_file().as_deref()),
        };
        let auth = match creds {
            Some(creds) => auth::login(&self.http, &creds).await?,
            None => None,
        };
        self.set_auth(auth);
        Ok(self.auth.as_ref())
    }

    pub fn set_auth(&mut self, auth: Option<Authentication>) {
        let token = auth.as_ref().and_then(|a| a.access_token());
        self.grpc_headers = headers::grpc_headers(&self.device_id, token);
        self.auth = auth;
    }

    pub fn auth(&self) -> Option<&Authentication> {
        self.auth.as_ref()
    }

    /// Maximum number of flights per live feed request.
    pub fn live_feed_limit(&self) -> i32 {
        if self.auth().and_then(|a| a.access_token()).is_some() {
            2000
        } else {
            1500
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    //
    // gRPC
    //

    pub async fn live_feed(&self, params: &LiveFeedParams) -> Result<LiveFeedResponse> {
        self.unary("LiveFeed", &params.to_proto()).await
    }

    /// Fetch the live feed for the entire world by splitting it into
    /// longitude slices (see [`bbox::world_at`]) and querying them
    /// concurrently. `template` provides everything but the bounding box.
    ///
    /// Slices that hit `template.limit` are split in half and re-fetched, up
    /// to [`MAX_WORLD_SPLITS`] times, since the per-request limit would
    /// otherwise silently drop flights in congested airspace.
    pub async fn live_feed_world(
        &self,
        template: &LiveFeedParams,
        concurrency: usize,
    ) -> WorldFeed {
        self.live_feed_area(template, bbox::WORLD, concurrency)
            .await
    }

    /// Like [`Fr24::live_feed_world`], but only for the slices overlapping
    /// `area` (see [`bbox::area_at`]).
    pub async fn live_feed_area(
        &self,
        template: &LiveFeedParams,
        area: BoundingBox,
        concurrency: usize,
    ) -> WorldFeed {
        let mut world = WorldFeed::default();
        let mut seen = std::collections::HashSet::new();
        let mut pending = bbox::area_at(area, headers::now_s());

        for depth in 0..=MAX_WORLD_SPLITS {
            let results: Vec<_> = futures_util::stream::iter(std::mem::take(&mut pending))
                .map(|bounding_box| async move {
                    let params = LiveFeedParams {
                        bounding_box,
                        ..template.clone()
                    };
                    (bounding_box, self.live_feed(&params).await)
                })
                .buffer_unordered(concurrency.max(1))
                .collect()
                .await;
            world.requests += results.len();

            for (bbox, result) in results {
                match result {
                    Ok(r)
                        if r.flights_list.len() >= template.limit as usize
                            && depth < MAX_WORLD_SPLITS =>
                    {
                        let mid = (bbox.west + bbox.east) / 2.0;
                        pending.push(BoundingBox { east: mid, ..bbox });
                        pending.push(BoundingBox { west: mid, ..bbox });
                    }
                    Ok(r) => {
                        world.server_time_ms = world.server_time_ms.max(r.server_time_ms);
                        world.flights.extend(
                            r.flights_list
                                .into_iter()
                                .filter(|f| seen.insert(f.flightid)),
                        );
                    }
                    Err(e) => world.errors.push((bbox, e)),
                }
            }
            if pending.is_empty() {
                break;
            }
        }
        world
    }

    pub async fn live_feed_playback(
        &self,
        params: &LiveFeedPlaybackParams,
    ) -> Result<PlaybackResponse> {
        self.unary("Playback", &params.to_proto()).await
    }

    pub async fn nearest_flights(
        &self,
        params: &NearestFlightsParams,
    ) -> Result<NearestFlightsResponse> {
        self.unary("NearestFlights", &params.to_proto()).await
    }

    pub async fn live_flights_status(
        &self,
        params: &LiveFlightsStatusParams,
    ) -> Result<LiveFlightsStatusResponse> {
        self.unary("LiveFlightsStatus", &params.to_proto()).await
    }

    /// Follow a live flight. The first message contains the full details
    /// (aircraft info, trail...), subsequent ones only contain updates.
    pub async fn follow_flight(
        &self,
        params: &FollowFlightParams,
    ) -> Result<impl Stream<Item = Result<FollowFlightResponse>> + use<>> {
        grpc::server_stream(
            &self.http,
            &self.grpc_headers,
            "FollowFlight",
            &params.to_proto(),
        )
        .await
    }

    pub async fn top_flights(&self, params: &TopFlightsParams) -> Result<TopFlightsResponse> {
        self.unary("TopFlights", &params.to_proto()).await
    }

    /// Unstable: returns an empty `DATA` frame as of Sep 2024.
    pub async fn live_trail(&self, flight_id: u32) -> Result<LiveTrailResponse> {
        self.unary("LiveTrail", &LiveTrailRequest { flight_id })
            .await
    }

    /// Unstable: returns an empty `DATA` frame.
    pub async fn historic_trail(&self, flight_id: u32) -> Result<HistoricTrailResponse> {
        self.unary("HistoricTrail", &HistoricTrailRequest { flight_id })
            .await
    }

    /// Unstable: gateway timeout.
    pub async fn search_index(&self) -> Result<FetchSearchIndexResponse> {
        self.unary("FetchSearchIndex", &FetchSearchIndexRequest {})
            .await
    }

    /// Returns an empty `DATA` frame error if the flight is not live.
    pub async fn flight_details(
        &self,
        params: &FlightDetailsParams,
    ) -> Result<FlightDetailsResponse> {
        self.unary("FlightDetails", &params.to_proto()).await
    }

    /// Returns an empty `DATA` frame error if the flight is live.
    pub async fn playback_flight(
        &self,
        params: &PlaybackFlightParams,
    ) -> Result<PlaybackFlightResponse> {
        self.unary("PlaybackFlight", &params.to_proto()).await
    }

    async fn unary<Resp: prost::Message + Default>(
        &self,
        method: &str,
        message: &impl prost::Message,
    ) -> Result<Resp> {
        grpc::unary(&self.http, &self.grpc_headers, method, message).await
    }

    //
    // JSON
    //

    /// Query flight list data. To determine if there are more pages, check
    /// `.result.response.page.more`.
    pub async fn flight_list(&self, params: &FlightListParams) -> Result<serde_json::Value> {
        self.get_json(
            "https://api.flightradar24.com/common/v1/flight/list.json",
            params.query_pairs(),
        )
        .await
    }

    pub async fn airport_list(&self, params: &AirportListParams) -> Result<serde_json::Value> {
        self.get_json(
            "https://api.flightradar24.com/common/v1/airport.json",
            params.query_pairs(),
        )
        .await
    }

    /// Fetch historical track playback data for a given flight.
    pub async fn playback(&self, params: &PlaybackParams) -> Result<serde_json::Value> {
        self.get_json(
            "https://api.flightradar24.com/common/v1/flight-playback.json",
            params.query_pairs(),
        )
        .await
    }

    /// General search.
    pub async fn find(&self, params: &FindParams) -> Result<Find> {
        self.get_json(
            "https://www.flightradar24.com/v1/search/web/find",
            params.query_pairs(),
        )
        .await
    }

    //
    // static reference data
    //

    /// All airlines known to Flightradar24 (name, IATA and ICAO codes).
    pub async fn airlines(&self) -> Result<static_data::Rows<static_data::Airline>> {
        self.get_static(static_data::AIRLINES_URL, &[]).await
    }

    /// All airports known to Flightradar24, with positions and timezones.
    pub async fn airports(&self) -> Result<static_data::Rows<static_data::Airport>> {
        self.get_static(static_data::AIRPORTS_URL, &[("version", "0")])
            .await
    }

    /// Aircraft families and the type designators in each.
    pub async fn aircraft_family(&self) -> Result<static_data::Rows<static_data::AircraftFamily>> {
        self.get_static(static_data::AIRCRAFT_FAMILY_URL, &[]).await
    }

    pub async fn countries(&self) -> Result<serde_json::Value> {
        self.get_static(static_data::COUNTRIES_URL, &[]).await
    }

    async fn get_static<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let mut headers = headers::default_headers();
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("*/*"),
        );
        let response = self
            .http
            .get(url)
            .headers(headers)
            .query(query)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::HttpStatus(response.status()));
        }
        let bytes = response.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        mut query: Vec<(&'static str, String)>,
    ) -> Result<T> {
        // with_auth
        match self.auth().and_then(|a| a.subscription_key()) {
            Some(key) => query.push(("token", key.to_owned())),
            None => query.push(("device", self.device_id.clone())),
        }
        let response = self
            .http
            .get(url)
            .headers(self.json_headers.clone())
            .query(&query)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::HttpStatus(response.status()));
        }
        let bytes = response.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}
