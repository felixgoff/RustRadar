# fr24 (Rust)

A Rust port of [abc8747/fr24](https://github.com/abc8747/fr24) (v0.4.0), a client
for Flightradar24's unofficial gRPC-web and JSON APIs.

Protobuf definitions are copied verbatim from upstream (`proto/fr24/proto`) and
compiled at build time with [`protox`](https://crates.io/crates/protox), so no
`protoc` install is needed.

## Coverage

| Upstream (Python)                          | Rust                                                     |
| ------------------------------------------ | -------------------------------------------------------- |
| `proto/__init__.py` (framing, trailers)    | `proto::{encode_message, parse_data, FrameDecoder}`      |
| `proto/headers.py` (headers, device id)    | `headers`                                                |
| `authentication.py` (env/config, login)    | `auth`                                                   |
| `grpc.py` params + `*_dict` conversions    | `grpc::*Params`, `grpc::*Record`                         |
| `json.py` (flight list, airports, find...) | `json`, `Fr24::{flight_list, airport_list, playback, find}` |
| `static/bbox.py`                           | `bbox`                                                   |
| `FR24` service facade                      | `Fr24`                                                   |

gRPC methods: `LiveFeed`, `Playback`, `NearestFlights`, `LiveFlightsStatus`,
`FollowFlight` (server streaming, parsed incrementally), `TopFlights`,
`LiveTrail`, `HistoricTrail`, `FetchSearchIndex`, `FlightDetails` and
`PlaybackFlight`.

Additions over upstream:

- `Fr24::live_feed_world` fetches the whole world concurrently and halves any
  slice that hits the per-request flight limit. `Fr24::live_feed_area` does the
  same for only the slices overlapping a bounding box.
- `FrameDecoder` handles multiple `DATA` frames and trailers in one body (listed as a todo
  upstream).
- `FlightDetailsRecord::apply_update` merges incremental `FollowFlight` messages.
- `FlightPlanRecord` exposes the filed flight plan (ICAO route string,
  waypoints, alternates) that `FlightDetails` and `FollowFlight` send to
  subscribers; anonymous sessions never receive one.
- `LiveFeedParams::services` and `sources` filter the feed by aircraft
  category (passenger, cargo, military, ...) and data source on the server.
- `json::FlightListRecord` flattens the flight list (one aircraft's or one
  flight number's past and scheduled flights).
- `static_data` and `Fr24::{airlines, airports, aircraft_family, countries}`
  fetch the reference lists the mobile apps use (airline ICAO/IATA codes,
  airports with FR24 ids, coordinates and IANA timezones).

Not ported: the polars/parquet cache, CLI and TUI.

## Usage

```rust
use fr24::{Fr24, grpc::{FlightRecord, LiveFeedParams}};

let mut fr24 = Fr24::new()?;
fr24.login(None).await?; // optional: reads fr24_username/fr24_password etc.
let feed = fr24.live_feed(&LiveFeedParams::new(fr24::bbox::FRANCE_UIR)).await?;
for flight in &feed.flights_list {
    println!("{:?}", FlightRecord::from(flight));
}
```

Run the live smoke test against the real API:

```sh
cargo run -p fr24 --example smoke
```

Credentials work as upstream: environment variables `fr24_username` +
`fr24_password` or `fr24_subscription_key` + `fr24_token`, overridden by the
`[global]` section of `fr24.conf` in the platform config dir
(`%LOCALAPPDATA%\fr24\fr24\fr24.conf` on Windows, `~/.config/fr24/fr24.conf` on Linux).

## Notes

- Streaming calls send `Accept-Encoding: identity`. With compression on, FR24's
  proxy buffers the stream: on HTTP/2 the response headers never arrive.
- This uses an unofficial API. Respect Flightradar24's terms of service.

## License

MIT. The protobuf definitions and overall design are derived from fr24,
© 2023 Abraham Cheung (see `LICENSE-fr24`).
