# 07 — Backend decoupling from Tauri, packaging and CI for the native app

Source of truth: `src/lib/api.ts`, everything under `src-tauri/`, the workspace `Cargo.toml`, `crates/fr24`,
`package.json`, `scripts/` and `.github/workflows/*.yml`, as they stand on branch `rust-port` before the port
(app version 0.3.0; locked: tauri 2.12.1, wry 0.57.0, tauri-plugin-updater 2.13.2, tauri-plugin-opener 2.7.0,
tokio 1.53.2, reqwest 0.13.5, rustls 0.23.45). File:line references are to that tree. `RS:` means
`src-tauri/src/`.

Part A (sections 1–9) records what exists. Part B (sections 10–17) proposes how to remove Tauri, package a
winit + wgpu + egui binary for the five platforms, and what CI has to become. Statements about third-party tools
that could not be checked from this repository are marked **(verify)**.

Contents: 1 Headline findings · 2 Workspace and build inputs · 3 Tauri commands · 4 Events, channels, URI
schemes · 5 Plugins, window, app state, setup sequence · 6 Files on disk · 7 Updates today · 8 Mobile specifics ·
9 CI and release pipeline · 10 Target crate API · 11 Replacing each Tauri facility · 12 The updater without
Tauri · 13 Packaging per platform · 14 CI after the port · 15 Scripts and the "no TypeScript" rule ·
16 Migration order · 17 Risks and open questions.

---

## 1. Headline findings

1. Tauri is thin here. 44 commands, 2 events, 2 IPC channels, 2 custom URI schemes, 2 plugins. Of 34 Rust source
   files, 20 mention `tauri`, and in 13 of those it is only `#[tauri::command]`, `tauri::async_runtime::spawn` or
   `tauri::http::Response`. Only `lib.rs`, `updater.rs`, `mobile_update.rs`, `keys.rs`, `export.rs`,
   `taxiways.rs` and `trafikverket.rs` use an `AppHandle`: three of them only for a directory path, and
   `trafikverket.rs` only to emit one event.
2. Most backend state is already process-global (`OnceLock` statics per module), not Tauri-managed. Only
   `AppState` (8 fields, `RS:lib.rs:63-80`) is Tauri-managed, and it is trivially a plain struct behind an `Arc`.
3. All commands return `Result<T, String>` or a plain value, with `T: Serialize`. Serialization exists only to
   cross the webview boundary; natively the functions return the structs directly. Field renames
   (`camelCase` vs `snake_case`, section 3.4) stop mattering except for the three on-disk formats in section 6.
4. The two URI schemes (`railtiles`, `buildingtiles`) are plain `async fn(path) -> bytes`; the HTTP wrapping is
   incidental.
5. The updater is the only substantial Tauri-specific piece. Installed copies (Windows/macOS/Linux) will fetch
   `latest.json` from the GitHub release and install what it points to. **The first native release must still be
   installable by the old Tauri updater**, or every existing desktop install is stranded (section 12.1).
6. Android in-place upgrade needs the same `applicationId` (`com.felix.rustradar`), the same signing key, and a
   `versionCode` not lower than Tauri's (section 8.1). iOS needs the same bundle id and a higher build number.
7. Paths: use `dirs::{cache_dir,config_dir}().join("com.felix.rustradar")` to land in exactly the folders Tauri
   used on desktop, so `keys.json` and the caches survive. `directories::ProjectDirs` does **not** produce the
   same folders (section 11.1).
8. Webview `localStorage` (`rustradar.v1`: settings and filters, doc 06 §23) cannot be read by the native app.
   Settings reset on upgrade unless a migration is written; that is an owner decision (section 17).
9. CI is built around `bun` + `tauri-action`. Nothing in the native build needs Node. `scripts/version.mjs`,
   `scripts/release-version.sh`, `scripts/check-signing-key.sh` and `scripts/testflight-age.mjs` are release
   tooling used by CI and must be replaced or kept deliberately (section 15).

---

## 2. Workspace and build inputs

### 2.1 Root `Cargo.toml`

- `[workspace] resolver = "3"`, members `src-tauri` and `crates/fr24`.
- `[profile.dev.package.{image,png,fdeflate,miniz_oxide,flate2,crc32fast}] opt-level = 3`: image codecs are
  optimised even in dev builds because the weather radar decodes a dozen 7000×3500 grids at once. Keep.
- `[profile.release]`: `codegen-units = 1`, `lto = true`, `opt-level = 3`, `panic = "abort"`, `strip = true`.
  Keep; note `panic = "abort"` means a panic in any thread ends the app (already true today).

### 2.2 `src-tauri/Cargo.toml`

- Package `rustradar` 0.3.0, edition 2024. Library `rustradar_lib` with
  `crate-type = ["staticlib", "cdylib", "rlib"]` (staticlib for iOS, cdylib for Android, rlib for the desktop
  binary). `src-tauri/src/main.rs` is 6 lines: `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`
  and `rustradar_lib::run()`.
- Build dependency `tauri-build 2`; `build.rs` is `tauri_build::build()` (generates the context, defines the
  `desktop` / `mobile` cfg aliases, embeds the Windows resource/icon).
- Dependencies: `tauri 2` (feature `devtools`), `tauri-plugin-opener 2`, `serde` (`derive`, `rc`), `serde_json`,
  `fr24` (path), `futures-util` (no default features, `std`), `tokio 1` (`time`, `macros`, `sync`, `rt`),
  `flate2`, `image 0.25` (`png` only), `reqwest 0.13` (no default features, `stream`), `bzip2-rs 0.1.2`,
  `contour 0.13.1` (`f32`).
- `[target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies] tauri-plugin-updater = "2"`.
- `cfg(desktop)` / `cfg(mobile)` are used at `RS:lib.rs:13-14, 33, 642-645, 648, 652, 686-689, 745-752` and
  `RS:mobile_update.rs:89, 95`. They come from `tauri-build`; without it they must become
  `cfg(any(target_os = "android", target_os = "ios"))` and its negation.

### 2.3 `crates/fr24`

Already Tauri-free. Rust port of the Python `fr24` library: gRPC-web + protobuf (`prost 0.14`, compiled in
`build.rs` by `protox 0.9`, pure Rust, no `protoc`), JSON API, auth. Dependencies: `base64`, `bytes`, `dirs 6`,
`futures-util`, `prost`, `prost-types`, `rand`, `reqwest 0.13` (`json`, `gzip`, `deflate`, `zstd`, `stream`,
`form`, `query`; default TLS = rustls with the platform verifier), `serde`, `serde_json`, `thiserror`, `tokio`
(`sync`). Android only: `webpki-root-certs 1`.

Points the port depends on:

- `fr24::client_builder()` (`crates/fr24/src/lib.rs:79-86`): on Android the platform certificate verifier must be
  initialised from Java and panics otherwise, so the builder is given Mozilla's roots instead
  (`tls_certs_only(...)`). **Every** HTTP client in the backend is built through it
  (`RS:adsblol.rs:202`, `geocode.rs:356`, `rail.rs:151`, `ships.rs:357`, `taxiways.rs:87`, `trafikverket.rs:1649`,
  `train_media.rs:58`, `trains.rs:340`, and `Fr24::new`). Any new client the native app adds (basemap tiles,
  images, terrain, the updater) must do the same or it will panic on Android.
- `Fr24::new()` uses `connect_timeout 10 s`, `read_timeout 30 s` and no total timeout (a total timeout would cut
  the `follow_flight` stream).
- `Fr24::login(None)` reads credentials from env vars `fr24_username`, `fr24_password`, `fr24_subscription_key`,
  `fr24_token`, overridden by the `[global]` section of `fr24.conf` at `dirs::config_dir()/fr24/fr24.conf`
  (Windows: `dirs::config_local_dir()/fr24/fr24/fr24.conf`) (`crates/fr24/src/auth.rs:59-75`).
- Public surface used by the app: `Fr24::{new, login, set_auth, auth, live_feed_limit, http, live_feed_area,
  flight_details, follow_flight, top_flights, find, airport_list, flight_list, airlines, airports}`,
  `fr24::bbox::WORLD`, `fr24::grpc::{BoundingBox, LiveFeedParams, LiveFeedField, FlightDetailsParams,
  FlightDetailsRecord, FollowFlightParams, TopFlightsParams, TopFlightRecord}`, `fr24::json::{AirportListParams,
  FindParams, FindEntry, FlightListParams, FlightListQuery, FlightListRecord}`, `fr24::proto::common::{Flight,
  Icon, PositionBuffer}`, `fr24::auth::Credentials`.

### 2.4 `package.json` scripts (all go away with the frontend, except where noted)

| Script | Command | After the port |
|---|---|---|
| `dev`, `build`, `preview`, `prepare`, `check`, `check:watch` | vite / svelte-kit / svelte-check | deleted |
| `test` | `bun test src` | replaced by `cargo test --workspace` (doc 05) |
| `tauri` | `tauri` CLI | deleted |
| `bump` | `node scripts/version.mjs` | replaced (section 15.1) |
| `build:aircraft` | `node scripts/aircraft/build.mjs` | offline asset tool, section 15.2 |
| `build:trains` | `node scripts/trains/build.mjs` | offline asset tool, section 15.2 |

`patchedDependencies` patches `@deck.gl/extensions@9.4.0`; irrelevant after the port.

### 2.5 Compiled-in data and bundled static files

- `src-tauri/data/flow.json` and `src-tauri/data/sth.json` (1.8 MB together) are `include_str!`-ed
  (`RS:flow.rs:26`, `RS:sth.rs:29`). Unchanged by the port.
- `src-tauri/tests/data/*` are test fixtures (`include_str!`/`include_bytes!` in tests). Several tests are
  env-driven and skipped by default (`RAIL_TILES`, `SIGNAL_TILES`, `LIVE_DIR`, `TRAFIKVERKET_KEY`, …).
- `static/` (27 MB) is the web root Tauri embeds in the binary via `frontendDist: "../build"`:
  `static/aircraft/{icons.ktx2, icons-outline.ktx2, index.json, meshes.bin}` (1.9 MB),
  `static/trains/{index.json, meshes.bin}` (1.9 MB), `static/rail/railroads.bin` (568 KB),
  `static/realistic/*.ktx2` (13 files, 21 MB, plus `LICENSE-streets-gl.txt`),
  `static/street-trees/{malmo,umea}.bin` (1.2 MB), `static/favicon.png`, `static/updater.html`.
  The native app must ship these itself (section 11.8).
- Fonts (`@fontsource-variable/overpass`, `overpass-mono`) and icons (`@lucide/svelte`) come from `node_modules`
  today. The native app needs the font files and icon glyphs checked into the repository (doc 06 §26, §29).

---

## 3. Tauri commands

Registered in `tauri::generate_handler!` at `RS:lib.rs:719-770`. 44 distinct names; `pending_update` and
`restart_to_update` each have a desktop and a mobile implementation (48 functions in all).

Conventions: `CmdResult<T> = Result<T, String>` (`RS:lib.rs:88`). Tauri converts JS camelCase argument names to
the Rust snake_case parameter names (`flightId` → `flight_id`, `onEvent` → `on_event`, `fileName` →
`file_name`). "sync" means a non-`async fn` (Tauri runs it on its thread pool; all of these are short and
non-blocking except where noted). `State`/`AppHandle` parameters are injected, not sent by the frontend.

### 3.1 Flightradar24, reference data, radar (module `lib.rs`)

| Command | Arguments (JS names) | Returns | Kind | Line | Notes |
|---|---|---|---|---|---|
| `live_flights` | `area: BoundingBox \| null`, `services: number[] \| null` | `LiveSnapshot` | async | 194 | `area` default `fr24::bbox::WORLD`; `limit = fr24.live_feed_limit()`; adds field `Vspeed` when signed in; `live_feed_area(&template, area, WORLD_CONCURRENCY = 8)`. Errors only when zero flights came back and at least one slice failed (first error's text). `errors[]` entries are `"{west:.0}..{east:.0}: {e}"`. |
| `flight_details` | `flightId: u32` | `FlightDetailsRecord` | async | 247 | |
| `follow_flight` | `flightId: u32`, `onEvent: Channel<FollowEvent>` | `()` | sync | 272 | Spawns `follow_task`, aborts the previous one (`AppState.follow`). Section 4.2. |
| `unfollow_flight` | none | `()` | sync | 280 | Aborts the task. |
| `top_flights` | none | `Vec<TopFlightRecord>` | async | 337 | `TopFlightsParams { limit: 10 }`. |
| `search` | `query: String` | `Vec<FindEntry>` | async | 352 | Trimmed; fewer than 2 bytes → empty list; `FindParams { limit: 20 }`. |
| `reference_data` | none | `Arc<ReferenceData>` | async | 370 | Memoised in `AppState.reference`; disk cache `reference.json` (section 6). |
| `airport_runways` | `airports: String[]` | `HashMap<String, Vec<Runway>>` | async | 387 | At most 16 codes, trimmed + upper-cased ICAO; unknown codes left out; disk cache `runways.json`. |
| `airport_board` | `code: String`, `mode: "arrivals" \| "departures"`, `page: u32 \| null` | `board::Board` | async | 418 | `page` default 1, `limit 25`, code trimmed + upper-cased IATA. |
| `aircraft_history` | `registration: String` | `Vec<FlightListRecord>` | async | 439 | `limit 10`, trimmed. |
| `airline_colors` | `iatas: String[]` | `HashMap<String, Option<LogoColors>>` | async | 458 | Filters with `liveries::is_iata`, at most 40 per call; missing ones fetched 6 at a time (`buffer_unordered(6)`), then the whole cache is rewritten to `livery-colors-2.json`. Holds `AppState.colors` for the whole call. |
| `radar_frame` | `area: BoundingBox \| null` | `Vec<radar::VolumeInfo>` | async | 540 | Sources `mrms` (bounds `mrms::BOUNDS`) and `dwd` (west 1.4, south 45.6, east 18.9, north 56.3). A source is fetched when its bounds overlap `area` (`in_view`, antimeridian-aware, 517-532) and its volume is missing or older than `radar::MAX_AGE = 480 s`. `radar_fetching` de-duplicates overlapping calls. Failed refresh keeps the previous volume. Error `"no radar volume could be loaded"` only when a fetch ran and nothing is held. |
| `radar_image` | `source: String`, `name: String` | raw bytes (`tauri::ipc::Response`, arrives as `ArrayBuffer`) | async | 610 | `name` is `ground`, `coverage` or a height in km such as `3` (matched within 0.01). Errors `"no such radar source"`, `"no image by that name"`. The only command with a binary response. |
| `opensky_track` | `icao24: String` | `Vec<opensky::TrackPoint>` | async | 629 | Cache in `AppState.opensky` (TTL 240 s, back-off flag). |
| `open_devtools` | none (injected `WebviewWindow`) | `()` | sync | 641 | Desktop only; opens the web inspector (Cmd/Ctrl+Alt+I). No native equivalent; drop. |

### 3.2 Other modules

| Command | Arguments (JS names) | Returns | Kind | Module:line | Notes |
|---|---|---|---|---|---|
| `adsb_hex` | `hex: String` | `adsblol::Snapshot` | async | `adsblol.rs:380` | Lower-cased; must be 6 hex digits else `"not an ICAO 24-bit address"`. Spacing 2 s, TTL 1.5 s, back-off 10 s → 600 s. |
| `adsb_callsign` | `callsign: String` | `adsblol::Snapshot` | async | `adsblol.rs:390` | Upper-cased; 1–8 ASCII alphanumerics else `"not a callsign"`. |
| `geocode` | `query: String`, `lat: f64 \| null`, `lon: f64 \| null`, `lang: String \| null` | `Vec<Place>` | async | `geocode.rs:437` | Fewer than `MIN_QUERY` (3) chars → empty. Spacing 1 s, TTL 30 min, back-off 5 s → 300 s, timeout 8 s. |
| `session_info` | none | `SessionInfo { authenticated, expires }` | sync | `session.rs:30` | |
| `sign_in` | `email: String`, `password: String` | `SessionInfo` | async | `session.rs:35` | Clones the client, logs in, swaps `AppState.fr24`. Errors `"Flightradar24 didn't return a session for these details."`, `"Sign-in failed: {e}"`. |
| `sign_out` | none | `SessionInfo` | sync | `session.rs:57` | `set_auth(None)` on a clone, swap. |
| `get_keys` | none | `Keys` | sync | `keys.rs:73` | From the in-memory cell. |
| `save_keys` | `keys: Keys` | `Keys` | sync | `keys.rs:80` | Trims (not the password), writes `keys.json` atomically with mode 0600, updates the cell. Does blocking file I/O. |
| `airport_taxiways` | `icao: String`, `lat: f64`, `lon: f64` | `Arc<Vec<taxiways::Line>>` | async | `taxiways.rs:244` | Errors `"not an ICAO airport code"`, `"not a position"`; otherwise never errors (empty list on failure, retried after 10 min). Disk cache `taxiways/{ICAO}.json`. One airport at a time (holds the module mutex across the fetch). |
| `save_export` | `fileName: String`, `contents: String` | `String` (full path written) | async | `export.rs:57` | Downloads folder (else home), `safe_file_name` (max stem 120 chars), numbered `name (2).ext` if taken. Does blocking file I/O. |
| `pending_update` | none | `Option<String>` (version) | sync | `updater.rs:87` / `mobile_update.rs:90` | Section 7. |
| `restart_to_update` | none | `Result<(), String>` | sync | `updater.rs:94` / `mobile_update.rs:96` | Desktop: install + relaunch. Android: opens the APK URL. Error `"No update is waiting"`. |
| `ships_in_view` | `west, south, east, north, zoom: f64` (flat, not nested) | `Vec<Ship>` | async | `ships.rs:478` | One request at a time (`in_flight` gate); same area within 10 s from memory; spacing 2 s; back-off 10 s → 600 s; error `"VesselFinder is resting; retrying in {n} s"`. |
| `ship_details` | `mmsi: u32` | `ShipDetails` | async | `ships.rs:664` | `0` → `"not a ship"`; per-ship memory 60 s; shares the feed's spacing/back-off. |
| `trains_in_view` | `west, south, east, north, zoom: f64` | `Vec<Train>` | async | `trains.rs:572` | `"not an area"`; stamps `TRAFIKVERKET_ASKED` (streams run while asked within 120 s); stale-while-revalidate cache; error only when every source failed. |
| `train_stops` | `id: String` | `Vec<trafikverket::Stop>` | async | `trains.rs:560` | Empty for non-`trafikverket:` ids. |
| `train_route` | `id: String` | `trafikverket::TrainRoute` | async | `trains.rs:513` | Runs in `spawn_blocking` (CPU heavy). |
| `train_blocks` | `west, south, east, north: f64` | `TrainBlocks { blocks, structures, complete }` | async | `trains.rs:524` | `spawn_blocking`; structures padded by 0.03°. |
| `watch_trains` | `ids: String[]` | `()` | sync | `trains.rs:546` | Keeps only `trafikverket:` ids; sets the view-stream set. |
| `hide_train_operators` | `names: String[]` | `()` | sync | `trains.rs:554` | |
| `follow_train` | `id: String`, `onEvent: Channel<TrainEvent>` | `Result<(), String>` | sync | `trains.rs:491` | Only for `trafikverket:` ids with a key; replaces the previous followed train. Section 4.2. |
| `unfollow_train` | none | `()` | sync | `trains.rs:503` | |
| `train_media` | `source: String`, `product: String \| null`, `operator: String \| null` | `TrainMedia { photo, logo }` | async | `train_media.rs:199` | Memoised unless empty. Never errors. |
| `weather_at` | `lat: f64`, `lon: f64` | `Weather { cloud, cloudLow, fog, precipitation, symbol, temperature }` | async | `weather.rs:70` | `"no such place"`; grid cell cache 600 s; MET Norway. |
| `road_widths` | `lon: f64`, `lat: f64` | `Arc<Vec<RoadPiece>>` | async | `roads.rs:243` | Empty without a Trafikverket key or outside lon 10–25, lat 55–69.5; per-cell cache 24 h, reset when the key changes. |
| `product_codes` | none | `Vec<ProductSeen { code, name, class, known, runs }>` | sync | `trafikverket.rs:411` | Dev panel. |
| `trafikverket_key_status` | none | `"none" \| "invalid" \| "ok"` | async | `trafikverket.rs:1081` | Errors when Trafikverket cannot be reached. |
| `dev_log` | `since: u64 \| null` | `Vec<devlog::Entry>` | sync | `devlog.rs:169` | Entries with `id > since`; empty in release builds (`ENABLED = cfg!(debug_assertions)`). Keeps 400 entries, 512 KiB of body each. |
| `dev_log_clear` | none | `()` | sync | `devlog.rs:176` | |

Commands called from the frontend outside `api.ts` (so easy to miss): `save_export`
(`src/lib/export.ts:208`, `KeysDialog.svelte:93`), `get_keys`/`save_keys` (`src/lib/keys.ts:63, 73`),
`weather_at` (`src/lib/weather.ts:88`), `dev_log`/`dev_log_clear` (`DevPanel.svelte:46, 86`),
`pending_update`/`restart_to_update` (`UpdateToast.svelte:20, 29`), `open_devtools` (`+page.svelte:1248`).
`product_codes` is registered but nothing in `src/` calls it; port it only if the dev panel should show it.

### 3.3 Return shapes defined in `lib.rs`

`LiveFlight` (`RS:lib.rs:99-131`, camelCase): `id: u32`, `lat: f32`, `lon: f32`, `track: i32` (degrees clockwise
from north), `alt: i32` (feet), `speed: i32` (knots), `on_ground: bool`, `timestamp_ms: u64`, `callsign`,
`flight`, `reg`, `typecode`, `origin`, `destination`, `icon` (Flightradar24 icon class name, empty if unknown),
`positions: Vec<[i32; 3]>` (`[Δlat, Δlon, Δms]`, point = `lat − Δlat·1e-5`, `lon − Δlon·1e-5`,
`timestamp_ms − Δms`, newest first), `vspeed: Option<i32>` (feet per minute, only when signed in).

`buffered_positions` (`RS:lib.rs:139-149`): keeps a buffered position only when its `delta_ms` is strictly greater
than the last kept one and `<= MAX_BUFFER_MS = 60_000` (drops underflowed deltas near 2^32).

`LiveSnapshot` (`RS:lib.rs:180-189`, camelCase): `flights`, `server_time_ms: u64`, `requests: usize`,
`errors: Vec<String>`, `elapsed_ms: u64`, `authenticated: bool`.

Constants: `WORLD_CONCURRENCY = 8`, `FOLLOW_MAX_FAILURES = 3`, `FOLLOW_RETRY_DELAY = 3 s`.

### 3.4 Serde naming, for reference only

`api.ts` shows which structs serialize camelCase (`LiveFlight`, `LiveSnapshot`, `ReferenceData.fetchedAt`,
`BoardRow`, `RadarVolume`, `Ship`, `ShipDetails`, `AdsbAircraft`, `Train`, `Weather`, `Keys`, `SessionInfo`,
`RunwayEnd`, `devlog::Entry`) and which stay snake_case because they are `fr24` record types (`FlightDetails`,
`TrailPoint`, `TopFlight`, `HistoryRow`, `OpenSkyPoint`, `FindEntry`). Natively this only matters for what is
written to disk: `keys.json` (camelCase, section 6), `reference.json`, `runways.json`, `livery-colors-2.json`,
`taxiways/*.json`, `osm-platforms/*.json`. Keep those serde attributes unchanged so existing caches stay valid;
all other `Serialize` derives may stay (harmless) or go.

---

## 4. Events, channels and URI schemes

### 4.1 Global events (Rust → every window)

| Event | Payload | Emitted at | Frontend listener | Behaviour |
|---|---|---|---|---|
| `update-ready` | `String`: the version | `RS:updater.rs:77` (desktop, after a background download), `RS:mobile_update.rs:68` (Android, a newer release with an APK exists) | `src/lib/UpdateToast.svelte:21` | Shows the update toast. `pending_update` covers a window that missed the event. |
| `trains-changed` | `()` | `RS:trafikverket.rs:1598`, when a live stream's reports changed the trains cache (`trains::patch` returned true) | `src/routes/+page.svelte:617` | The page re-requests `trains_in_view` for the current view, at most once per `TRAINS_LIVE_MS = 1500` ms (trailing: if one is pending, further events are ignored; wait = `max(0, last + 1500 − now)`), and only while trains are shown. |

### 4.2 Channels (Rust → one caller, ordered)

`FollowEvent` (`RS:lib.rs:259-268`), serialized `{ "event": "details" | "error" | "ended", "data": … }`:

- `Details(Box<FlightDetailsRecord>)`: the merged record after every stream message. The first message of a
  subscription is a full snapshot (`update.aircraft.is_some()` → replace); later ones are merged with
  `details.apply_update(update)`.
- `Error(String)`: a subscription or message failed; the task retries.
- `Ended(String)`: sent once, text `"flight is no longer live"`, after `FOLLOW_MAX_FAILURES = 3` consecutive
  subscriptions that delivered nothing. The task then exits.

`follow_task` (`RS:lib.rs:286-333`): loop { subscribe; forward messages; on stream end or error wait
`FOLLOW_RETRY_DELAY = 3 s` and resubscribe }. `failures` resets to 0 whenever a subscription delivered at least
one message. A failed `channel.send` (receiver gone) ends the task.

`TrainEvent` (`RS:trafikverket.rs:1618-1625`), serialized `{ "kind": "train", "train": Train }` or
`{ "kind": "stops", "stops": Stop[] }`. `trafikverket::follow` (`RS:trafikverket.rs:1723-1797`): pushes the stops
once at start; runs two Trafikverket SSE streams (positions, announcements), each reopened `RECONNECT = 3 s`
after it closes; an announcement message coalesces the queue and pushes the stops again; a position message
newer than the last and not older than `STALE_REPORT_MS` becomes a `Train` event. Ends when the channel's
receiver is gone or the task is aborted.

### 4.3 Custom URI schemes

Registered with `register_asynchronous_uri_scheme_protocol` (`RS:lib.rs:666-674`). The webview addresses them as
`railtiles://localhost/{z}/{x}/{y}` on macOS/Linux/iOS and `http://railtiles.localhost/{z}/{x}/{y}` on Windows
and Android (`src/lib/rail-layer.ts:312`); `buildingtiles` likewise.

| Scheme | Handler | Path | Response |
|---|---|---|---|
| `railtiles` | `rail::serve(path)` `RS:rail.rs:513-534` | `/{z}/{x}/{y}` | `Content-Type: application/x-protobuf`, `Access-Control-Allow-Origin: *`. 400 if not three integers; 204 if `z` is outside `MIN_ZOOM..=ZOOM` and not `PLATFORM_ZOOM`, or `x`/`y` out of range, or the tile is empty; 200 with the MVT bytes from `rail::tile_bytes(z, x, y)`; 502 on fetch error (logged). |
| `buildingtiles` | `building_lod::serve(path)` `RS:building_lod.rs:561-584` | `/{z}/{x}/{y}` | Same headers. 400 / 204 (outside `MIN_ZOOM..=MAX_ZOOM`, or empty) / 200 with `encode(&grid((z,x,y)).await?)` (an MVT the backend synthesises) / 502. |

Natively both become direct function calls returning `Result<Option<Bytes>, String>` (`None` = the 204 case).
Whether the renderer still wants MVT bytes or the decoded structures (`rail::decode`, the building `Grid`) is
for docs 02 and 04; decoding once in the backend and handing over typed geometry avoids an MVT round trip.

---

## 5. Plugins, window, app state, setup

### 5.1 Plugins

- `tauri-plugin-opener` (all platforms; capability `opener:default` in `src-tauri/capabilities/default.json`).
  Frontend uses `openUrl(url)` (external links: JetPhotos, adsb.lol, Flightradar24, VesselFinder, Wikipedia,
  attribution credits, the keys page's links) and `revealItemInDir(path)` (after `save_export`, in
  `export.ts:210` and `KeysDialog.svelte:94`). Backend uses `app.opener().open_url(url, None)` in
  `RS:mobile_update.rs:104` (Android APK download).
- `tauri-plugin-updater` (desktop only, `RS:lib.rs:652-653`). Configuration in `tauri.conf.json` `plugins.updater`:
  a minisign public key (`pubkey`), one endpoint
  `https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json`, and
  `windows.installMode = "quiet"`.
- Capabilities: one file, window `main`, permissions `core:default` and `opener:default`. No meaning natively.

### 5.2 Window (`tauri.conf.json` `app.windows[0]`)

Title `RustRadar`; inner size 1440 × 900 logical px; minimum 900 × 600; background `#04060b`
(RGBA 4, 6, 11, 255); `maximized: true`; `visible: false` (shown by the updater task once the update check is
done, section 7.1). CSP is `null`. Product name `RustRadar`, identifier `com.felix.rustradar`.

Updater splash window (`RS:updater.rs:156-169`): label `updater`, 380 × 150 logical px, not resizable, no
decorations, not maximizable or minimizable, always on top, centred, background RGBA (4, 6, 11, 255), page
`static/updater.html` (body `#04060b`, text `#e9edf3`, 14px system font; a 1px `#1b2230` border; `h1` "RustRadar"
15px weight 650; status text `#9aa5b8`; a 3px bar `#141a26` radius 2 with fill `#fcb442`, width transition
0.15 s ease-out; indeterminate state = a 30 % sliver sweeping `translateX(-100%)` → `340%` over 1.2 s
ease-in-out, infinite). Status strings: `"Updating RustRadar…"` (indeterminate), `"Downloading update…"` (with a
fraction, pushed at most once per 1 %), `"Installing…"` (fraction 1.0).

### 5.3 `AppState` (`RS:lib.rs:63-80`)

| Field | Type | Purpose |
|---|---|---|
| `fr24` | `RwLock<Arc<Fr24>>` | Swapped on sign-in/out; `client()` clones the `Arc`. |
| `follow` | `Mutex<Option<JoinHandle<()>>>` | The followed flight's stream task. |
| `reference` | `tokio::sync::Mutex<Option<Arc<ReferenceData>>>` | Airlines + airports, loaded once. |
| `runways` | `tokio::sync::Mutex<Option<Arc<RunwayData>>>` | Loaded once. |
| `colors` | `tokio::sync::Mutex<Option<liveries::ColorCache>>` | Livery colours, read from disk on first use. |
| `radar` | `tokio::sync::Mutex<HashMap<&'static str, Arc<radar::Volume>>>` | Newest volume per source. |
| `radar_fetching` | `Mutex<HashSet<&'static str>>` | Sources with a fetch under way. |
| `opensky` | `tokio::sync::Mutex<opensky::Cache>` | Tracks + back-off. |

Everything else is module-level statics: `keys::cell()`; `updater::ready()` / `mobile_update::ready()`;
`devlog::log()`; `adsblol` cache + client; `geocode` cache + client; `ships` cache, gate, client, tracks, details
cache; `taxiways` cache + client; `trains` states, gates, client, `followed()`, `TRAFIKVERKET_ASKED`;
`trafikverket` products, plans, ended, chosen, station/delay/service caches, operator trains, hidden operators,
watched set, operational numbers, stream client, history, places; `rail` store, cache dir, client, platform
cache; `blocks` signal store; `building_lod` tilejson, semaphore, cache dir, grids; `osm_platforms` store, cache
dir; `roads` cells; `train_media` client + cache; `weather` cache; `flow` / `sth` indexes.

### 5.4 Setup sequence (`RS:lib.rs:649-718`)

1. `Fr24::new()` (panics with `"failed to build http client"` on failure).
2. Plugins: opener; updater (desktop).
3. `manage(AppState { … })`.
4. Register the two URI schemes.
5. `setup`:
   1. `app_cache_dir()` → `osm_platforms::set_cache_dir(dir)`, `building_lod::set_cache_dir(dir)`,
      `rail::set_cache_dir(dir)` (each appends its own subfolder, section 6).
   2. `trafikverket::start_operator_streams(handle)` and `trafikverket::start_view_streams(handle)`: two
      forever-running supervisor tasks (section 5.5).
   3. Desktop: spawn `updater::run(handle)`. Mobile: spawn `mobile_update::run(handle)`. Either shows the main
      window.
   4. `keys::load(handle)` reads `keys.json` synchronously into the global cell.
   5. Spawn the launch login: clone the client; credentials from the saved keys if both email and password are
      non-empty, else `None` (env / `fr24.conf`); `fr24.login(creds)`; on `Ok(Some(_))` swap the client **only
      if** `Arc::ptr_eq(&slot, &before)` (a sign-in or sign-out made meanwhile wins); on `Err` log
      `"fr24 login failed, continuing anonymously: {e}"`.
6. `invoke_handler`, then `run(tauri::generate_context!())`.

### 5.5 Background tasks that outlive any command

| Task | Started | Loop | Notes |
|---|---|---|---|
| Operator streams supervisor | `RS:trafikverket.rs:1372` | every `STREAM_CHECK = 5 s` | One SSE stream per operator chunk (`MAX_STREAM_TRAINS = 150`), only while a Trafikverket key is saved and `trains::trafikverket_wanted()` (map asked within 120 s). Restarts all streams when the key changes; reopens an operator's stream for new trains at most every `REOPEN_FOR_NEW = 60 s`; each stream reconnects after `RECONNECT = 3 s`. Reports go through `move_trains` → `trains::patch` → `trains-changed`. |
| View stream supervisor | `RS:trafikverket.rs:1535` | every 1 s | One SSE stream for the `watch_trains` set once it has been stable for `WATCH_SETTLE = 2 s`; restarted on set or key change; closed when empty. |
| Updater | `RS:updater.rs:103` / `mobile_update.rs:75` | hourly | Section 7. |
| Follow flight / follow train | per command | until aborted | Section 4.2. |
| Fire-and-forget fetches | `RS:blocks.rs:191`, `osm_platforms.rs:78`, `rail.rs:578`, `trains.rs:421` | once | Tile / platform / signal / refresh fetches spawned from synchronous code paths, bounded by `MAX_LOADING`-style limits. All guarded by `cfg!(test)` so tests never touch the network. |

All of these are spawned with `tauri::async_runtime::spawn` (Tauri's own multi-threaded tokio runtime) or
`tauri::async_runtime::spawn_blocking` (`RS:trafikverket.rs:1587`, `trains.rs:516, 525`).

---

## 6. Files on disk

Tauri path API calls: `app.path().app_cache_dir()` (`RS:lib.rs:95, 676`, `taxiways.rs:262-265`),
`app.path().app_config_dir()` (`RS:keys.rs:54-59`), `app.path().download_dir()` and `home_dir()`
(`RS:export.rs:58-62`).

Tauri resolves these as `dirs::cache_dir()/<identifier>` and `dirs::config_dir()/<identifier>` on desktop, with
identifier `com.felix.rustradar`:

| | Cache dir | Config dir |
|---|---|---|
| Linux | `$XDG_CACHE_HOME` or `~/.cache` + `/com.felix.rustradar` | `$XDG_CONFIG_HOME` or `~/.config` + `/com.felix.rustradar` |
| macOS | `~/Library/Caches/com.felix.rustradar` | `~/Library/Application Support/com.felix.rustradar` |
| Windows | `%LOCALAPPDATA%\com.felix.rustradar` | `%APPDATA%\com.felix.rustradar` (Roaming) |
| Android | the app context's cache dir, `/data/user/0/com.felix.rustradar/cache` **(verify on a device)** | the app context's data/files dir **(verify on a device before choosing the native path)** |
| iOS | `<container>/Library/Caches/com.felix.rustradar` **(verify)** | `<container>/Library/Application Support/com.felix.rustradar` **(verify)** |

What is stored:

| Path | Dir | Written by | Format | Freshness |
|---|---|---|---|---|
| `keys.json` | config | `RS:keys.rs:80-95` | pretty JSON, camelCase fields `fr24Email`, `fr24Password`, `trafiklabGtfs`, `trafiklabGtfsStatic`, `trafiklabOxyfi`, `trafikverket`; missing fields read as empty (`#[serde(default)]`). Written to `keys.json.tmp` (created new, mode 0600 on Unix, `sync_all`) then renamed. | forever |
| `reference.json` | cache | `RS:reference.rs:60-80` | JSON `ReferenceData { fetchedAt, airlines, airports }` | weekly (`MAX_AGE_S`); also refetched if no airport has an `alt`; stale copy used if the download fails |
| `runways.json` | cache | `RS:runways.rs:61-78` | JSON `RunwayData` (OurAirports) | weekly, stale fallback |
| `livery-colors-2.json` | cache | `RS:liveries.rs:26-40` | JSON map IATA → colours or null | never expires (the `-2` is the schema version) |
| `taxiways/{ICAO}.json` | cache | `RS:taxiways.rs:205-226` | JSON lines | a month |
| `rail-tiles/…` | cache | `RS:rail.rs:137-139, 444-456` | raw MVT tiles (OpenRailwayMap) | `DISK_FRESH = 14 days`, stale fallback |
| `rail-signals/{SIGNAL_ZOOM}/{x}/{y}.pbf` | cache (sibling of `rail-tiles`) | `RS:blocks.rs:129-158` | raw MVT | same constant as rail |
| `building-lod/{z}/{x}/{y}.grid` | cache | `RS:building_lod.rs:254-282` | raw deflate of `CELLS × CELLS` bytes | `DISK_FRESH = 30 days` |
| `osm-platforms/{k0}_{k1}.json` | cache | `RS:osm_platforms.rs:57-126` | JSON platforms | `DISK_FRESH = 30 days`, stale fallback |
| exported flight paths | Downloads (else home) | `RS:export.rs:57-67` | KML/GPX/CSV/GeoJSON text built by the caller; also the keys export file | user files |

Not on disk from Rust: radar volumes, ships, trains, weather, geocoding (memory only). The webview stores
`rustradar.v1` in `localStorage` (doc 06 §23) inside the webview's own data directory; and the webview's HTTP
cache holds basemap tiles, textures and images. The native app has neither and needs its own settings file and
its own tile cache (owned by docs 01/04; they should live under the same cache dir).

---

## 7. Updates today

### 7.1 Desktop (`RS:updater.rs`, Windows/macOS/Linux)

At launch (`run`, 103-115):

1. Skipped entirely in debug builds or when the process was started with `--updated`
   (`UPDATED_FLAG`, guards against an install loop).
2. `updater.check()` with a `CHECK_TIMEOUT = 8 s` timeout. No update, timeout or any error → go to step 5.
3. Open the splash window, show `"Updating RustRadar…"`, then `download_and_install` with progress
   (section 5.2 for the strings).
4. `relaunch`: spawn `tauri::process::current_binary()` (not `current_exe`, which is stale after an AppImage has
   been replaced under itself) with the original arguments minus `--updated`, plus `--updated`; then
   `app.exit(0)`. On Windows the NSIS installer has already taken over and relaunches the app itself, so this
   line is never reached.
5. Close the splash if any; `show()` + `set_focus()` the main window.
6. `watch` (58-83): never in debug builds. Every `WATCH_EVERY = 3600 s`: check (8 s timeout); if a version newer
   than the one already downloaded exists, download it into memory (`Ready { update, bytes }`), then emit
   `update-ready` with the version. A failed check or download waits for the next hour.

`pending_update()` returns the downloaded version. `restart_to_update()` takes `Ready`, calls
`update.install(bytes)` (error `"Couldn't install the update: {e}"`), then `relaunch`.

Principle stated in the module docs: an update is never a reason not to start. Offline, slow, failed download,
bad signature all fall through to opening the installed version.

### 7.2 What the Tauri updater does underneath (needed for compatibility)

- Fetches the endpoint (`latest.json`). Shape **(verify against a published release's `latest.json`)**:
  `{ "version": "x.y.z", "notes": "...", "pub_date": "RFC 3339", "platforms": { "<target>": { "signature":
  "<base64 of the minisign .sig file>", "url": "<download URL>" } } }` with targets such as `windows-x86_64`,
  `darwin-aarch64`, `darwin-x86_64`, `linux-x86_64`; newer plugin versions also look for
  `<os>-<arch>-<installer>` keys (e.g. `linux-x86_64-appimage`, `windows-x86_64-nsis`) first.
- Compares `version` with the running app's version (semver, must be greater).
- Downloads `url`, verifies the minisign signature against the configured public key.
- Installs per platform: Windows runs the downloaded NSIS installer (`installMode: "quiet"` → silent, plus
  flags telling the Tauri NSIS template that this is an update and to relaunch afterwards); macOS unpacks
  `RustRadar.app.tar.gz` over the running `.app`; Linux overwrites the running AppImage file (only when the app
  runs as an AppImage; `.deb`/`.rpm` installs are handled by the package manager or not at all, depending on
  plugin version).
- Artifacts are produced by `tauri build --config src-tauri/tauri.updater.conf.json`
  (`bundle.createUpdaterArtifacts: true`): each installer plus a `.sig`, signed with the private key in
  `TAURI_SIGNING_PRIVATE_KEY` (password `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). `tauri-action` assembles and
  uploads `latest.json` (`includeUpdaterJson: true`).

### 7.3 Phones (`RS:mobile_update.rs`)

- `run`: show the main window; return immediately in debug builds and on iOS (TestFlight updates iOS).
- Android: now and every `CHECK_EVERY = 3600 s`, GET
  `https://api.github.com/repos/felixgoff/RustRadar/releases/latest` with headers `User-Agent: RustRadar`,
  `Accept: application/vnd.github+json`. `newer_apk(body, current)`: parse `tag_name` and the running version as
  `(major, minor, patch)` (leading `v` stripped; anything not three integers → no update); if the release is
  strictly newer and has an asset whose name ends in `.apk`, remember `(version, browser_download_url)` and emit
  `update-ready`.
- `restart_to_update` opens that URL with the opener; the browser downloads the APK and Android's installer
  takes over. `UpdateToast.svelte:15` detects Android by user agent and labels the button accordingly (doc 06
  §22); natively use `cfg!(target_os = "android")`.
- Tests: `finds_a_newer_apk` (`RS:mobile_update.rs:113-120`), compiled on desktop too (`cfg(any(mobile, test))`).

---

## 8. Mobile specifics

### 8.1 Android (`src-tauri/gen/android`, checked in)

- Gradle: AGP 9.3.1, Kotlin plugin 2.2.10 (`build.gradle.kts`); `compileSdk 37`, `targetSdk 37`, `minSdk 24`,
  namespace and `applicationId` `com.felix.rustradar`; Java/Kotlin target 1.8;
  `org.gradle.jvmargs=-Xmx2048m`, configuration cache on, `android.builtInKotlin=false`, `android.newDsl=false`,
  `android.proguard.failOnMissingFiles=false` (`gradle.properties`).
- `versionCode` / `versionName` are read from `tauri.properties` (`tauri.android.versionCode`,
  `tauri.android.versionName`), a file the Tauri CLI generates at build time (git-ignored). Tauri derives the
  code from the semver as `major·1 000 000 + minor·1 000 + patch` **(verify with `aapt dump badging` on a
  released APK)**; 0.3.0 → 3000. Android refuses to install an APK with a lower `versionCode` over an existing
  install, so the native build must use the same formula or a larger one.
- Signing: if `gen/android/keystore.properties` exists (`storeFile`, `password`, `keyAlias`; the same password
  for store and key) the release build type is signed with it; otherwise the release APK is unsigned. CI writes
  the file from secrets. Updates only install over a copy signed with the same key.
- Build types: debug (`usesCleartextTraffic = true`, debuggable, JNI debuggable, keeps debug symbols in the
  `.so`), release (`usesCleartextTraffic = false`, optimisation/minify on, every `*.pro` file as ProGuard rules).
- Manifest: permission `INTERNET` only; `android.software.leanback` not required; one activity `.MainActivity`,
  `exported`, `launchMode="singleTask"`, `configChanges="orientation|keyboardHidden|keyboard|screenSize|locale|
  smallestScreenSize|screenLayout|uiMode"`, intent filters `MAIN` + `LAUNCHER` + `LEANBACK_LAUNCHER`; theme
  `Theme.rustradar` (parent `Theme.Material3.DayNight.NoActionBar`); a `FileProvider`
  (`${applicationId}.fileprovider`, `res/xml/file_paths.xml`: `external-path` and `cache-path`, both `.`), which
  only the opener plugin uses.
- `MainActivity.kt` (11 lines): `class MainActivity : TauriActivity()` calling `enableEdgeToEdge()` before
  `super.onCreate`. So the web content draws under the system bars and insets come from CSS `env(safe-area-*)`.
- Dependencies: `androidx.webkit:webkit:1.14.0`, `appcompat 1.7.1`, `activity-ktx 1.10.1`,
  `material 1.12.0`, `lifecycle-process 2.10.0`.
- `buildSrc/` holds Tauri's Gradle `rust` plugin (`RustPlugin.kt`, `BuildTask.kt`): it runs
  `tauri android android-studio-script` per ABI so Gradle builds the Rust cdylib. `app/build.gradle.kts` also
  applies generated `tauri.build.gradle.kts` and `settings.gradle` applies `tauri.settings.gradle` (both
  git-ignored, generated by the CLI). All of this is Tauri-only.
- Resources: launcher icons in `mipmap-*` (legacy, round, adaptive foreground), `values/strings.xml`
  (`app_name` and `main_activity_title` both `RustRadar`), `values/colors.xml`, `values/themes.xml`,
  `values-night/themes.xml`, `layout/activity_main.xml`.
- ABIs: a release builds all four (`aarch64`, `armv7`, `i686`, `x86_64`) into one universal APK; the debug
  build in CI is `--target aarch64` only. NDK `27.0.12077973`, JDK 17 (Temurin).

### 8.2 iOS

- No Xcode project in the repository. CI generates it each run: `bun tauri ios init --ci` (needs `xcodegen` and
  `cocoapods` from Homebrew), then `bun tauri icon src-tauri/icons/icon.png`, then copies
  `src-tauri/icons/ios/*.png` (18 icons, flattened onto white because App Store Connect refuses an alpha
  channel) over `src-tauri/gen/apple/Assets.xcassets/AppIcon.appiconset/`.
- `src-tauri/Info.ios.plist` is merged into the app's `Info.plist`: `ITSAppUsesNonExemptEncryption = false`
  (HTTPS only, so TestFlight needs no export-compliance answer per build). Must be carried over.
- Bundle id `com.felix.rustradar` (also read by `scripts/testflight-age.mjs` from `tauri.conf.json`).
- Rust targets installed: `aarch64-apple-ios`, `aarch64-apple-ios-sim`.
- Unsigned path: `bun tauri ios build --no-sign` → `RustRadar_{version}_unsigned.ipa` for AltStore/Sideloadly.
- Signed path: `bun tauri ios build --export-method app-store-connect` with automatic signing through the App
  Store Connect API key (env `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_ID`, `APPLE_API_KEY_PATH`,
  `APPLE_DEVELOPMENT_TEAM`; the `.p8` written to `~/.appstoreconnect/private_keys/AuthKey_{KEY_ID}.p8`), then
  `xcrun altool --upload-app --type ios --file <ipa> --apiKey … --apiIssuer …`.
- In-app update: none; TestFlight. Builds expire after 90 days, hence the refresh workflow (section 9.3).
- Build number: the release's version by default; the refresh workflow overrides it with
  `--config '{"bundle":{"iOS":{"bundleVersion":"<YYYYMMDDHH>"}}}'`.

---

## 9. CI and release pipeline

### 9.1 `.github/workflows/rust.yml` ("Build")

Triggers: push to `main`, tags `v*`, pull requests to `main`, manual. `permissions: contents: read` by default.
Concurrency group per workflow + ref; in-progress runs are cancelled only for pull requests. Env
`CARGO_TERM_COLOR=always` and `RELEASE` = `true` for a push to `main` or a `v*` tag.

| Job | Runner | Needs | What it does |
|---|---|---|---|
| `check` | ubuntu-22.04 | | apt: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`; bun; Rust stable; `swatinem/rust-cache` (`workspaces: ". -> target"`, `shared-key: check`); `bun install --frozen-lockfile`; `bun run bump --check` (the four version files agree); `bun run check` (svelte-check); `bun test src`; `bun run build` (because `generate_context!` needs `../build` at compile time); `cargo test --workspace --locked`. |
| `version` | ubuntu-latest | | `bash scripts/release-version.sh` → output `version`; writes a step summary. |
| `bundle` | matrix | `check`, `version` | `contents: write`. Matrix: `windows-x86_64` (windows-latest, no args), `macos-aarch64` (macos-latest, `--target aarch64-apple-darwin`), `macos-x86_64` (macos-latest, `--target x86_64-apple-darwin`), `linux-x86_64` (ubuntu-22.04, no args). Linux apt adds `xdg-utils`. Rust targets: both mac targets on macOS. Release only: `scripts/check-signing-key.sh`; for a `main` push, `node scripts/version.mjs <version>` (changes only this build's files, nothing is committed). Then `tauri-apps/tauri-action@v1` with `tauriScript: bun tauri`, `args: <matrix> [--config src-tauri/tauri.updater.conf.json on release]`, `uploadWorkflowArtifacts: true`, `workflowArtifactNamePattern: "[name]_[version]_[platform]_[arch]_[bundle]"`, `tagName`: the tag name on a tag, `v__VERSION__` on a `main` push, empty otherwise (no release), `releaseName: "RustRadar v__VERSION__"`, `releaseBody: "Installed copies update to this version automatically the next time they open."`, `releaseDraft: false`, `prerelease: false`, `includeUpdaterJson: true`. |
| `android` | ubuntu-22.04 | `check`, `version` | JDK 17; `android-actions/setup-android@v3` (`packages: platform-tools`); `sdkmanager "ndk;27.0.12077973"`, `NDK_HOME`; bun; Rust with the four Android targets; cache key `android`; version override on a `main` release; keystore step (release only): without all three secrets a warning and no release APK, else decode the keystore to `$RUNNER_TEMP/release.jks` and write `src-tauri/gen/android/keystore.properties`. Then `bun tauri android build --apk` (signed release) or `bun tauri android build --apk --debug --target aarch64`. The APK is copied to `dist/RustRadar_{version}.apk` or `dist/RustRadar_{version}_debug.apk`; artifact `android-apk`. |
| `ios` | macos-latest | `check`, `version` | bun; Rust with the two iOS targets; cache key `ios`; App Store Connect key step (release only; a warning and an unsigned IPA if any of the four secrets is missing); version override; Xcode project (section 8.2); unsigned or App Store build; TestFlight upload; artifact `ios-ipa` (`dist/RustRadar_{version}.ipa` or `…_unsigned.ipa`). |
| `mobile-release` | ubuntu-latest | `version`, `bundle`, `android`, `ios` | `contents: write`. Runs when not cancelled, `bundle` succeeded and this is a release push. Downloads both artifacts (either may be missing), then `gh release upload <tag> … --clobber`, skipping `*_debug.apk`. Tag = the tag name, or `v{version}`. |

Bundle outputs with `bundle.targets: "all"` (Tauri v2 defaults): Windows `.msi` (WiX) and `-setup.exe` (NSIS);
macOS `.app` and `.dmg` (per architecture; not universal, not signed, not notarized: the Apple env vars are
commented out, `rust.yml:157-165`); Linux `.deb`, `.rpm`, `.AppImage`. With the updater config, also the
updater artifacts and `.sig` files, and one `latest.json` on the release.

Icons: `src-tauri/icons/{32x32.png, 128x128.png, 128x128@2x.png, icon.icns, icon.ico, icon.png}`, the Windows
Store `Square*Logo.png` / `StoreLogo.png` set, and `icons/ios/`.

### 9.2 Secrets (names only)

| Secret | Used by | For |
|---|---|---|
| `GITHUB_TOKEN` | `bundle`, `mobile-release` | creating the release, uploading assets |
| `TAURI_SIGNING_PRIVATE_KEY` | `bundle` | signing updater artifacts (minisign key file, base64, one line of about 350 characters; checked by `scripts/check-signing-key.sh`) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `bundle` | that key's password |
| `ANDROID_KEYSTORE` | `android` | the release keystore, base64 |
| `ANDROID_KEYSTORE_PASSWORD` | `android` | store and key password |
| `ANDROID_KEY_ALIAS` | `android` | key alias |
| `APPLE_API_ISSUER` | `ios`, `testflight-refresh` | App Store Connect issuer id |
| `APPLE_API_KEY_ID` | `ios`, `testflight-refresh` | API key id |
| `APPLE_API_KEY` | `ios`, `testflight-refresh` | the `.p8` contents |
| `APPLE_DEVELOPMENT_TEAM` | `ios`, `testflight-refresh` | team id |

Mentioned but commented out (not configured): `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
`APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` (macOS Developer ID signing and
notarization), and Windows code signing. Desktop builds are unsigned today.

### 9.3 Version logic

- The version lives in four files that must agree: `package.json`, `src-tauri/tauri.conf.json`,
  `src-tauri/Cargo.toml` (the first `version = "…"`), `Cargo.lock` (the `name = "rustradar"` entry).
  `scripts/version.mjs`: no argument prints them; `--check` exits 1 if they differ; `patch` / `minor` / `major`
  / `x.y.z` rewrites all four (refuses if they currently disagree).
- `scripts/release-version.sh`: `current` = `package.json`'s version; `latest` = the highest `vX.Y.Z` tag on
  `origin` (via `git ls-remote --tags --refs`, or the `TAGS` env var for tests; pre-release and odd tags
  ignored). No tags → `current`. `current` strictly newer than `latest` → `current`. Otherwise →
  `latest` with patch + 1. So a merge to `main` that forgot to bump still publishes a new, higher version.
- A `v*` tag publishes exactly the version in the files (no override step).
- `.github/workflows/bump.yml` ("Bump version", manual, input `bump` default `patch`): runs
  `node scripts/version.mjs <input>`, commits `"Version {version}"` as `github-actions[bot]` and pushes to the
  branch the workflow was started from. `contents: write`.

### 9.4 `.github/workflows/testflight-refresh.yml`

Daily at 06:17 UTC and manual (`force` boolean). `MAX_AGE_DAYS = 60`.

- Job `check` (ubuntu): writes the `.p8`, runs `node scripts/testflight-age.mjs` (signs an ES256 JWT good for
  15 minutes, audience `appstoreconnect-v1`; `GET /v1/apps?filter[bundleId]=…`, then
  `GET /v1/builds?filter[app]=…&sort=-uploadedDate&limit=1`; outputs `days` (1e6 when there is no build) and
  `version`); `due = force || days > 60`.
- Job `upload` (macos, when due): full-history checkout; newest tag matching `v[0-9]*.[0-9]*.[0-9]*` by version
  sort is checked out; same iOS setup as `rust.yml`; `node scripts/version.mjs "${TAG#v}"`;
  `BUILD_NUMBER=$(date -u +%Y%m%d%H)`; `bun tauri ios build --export-method app-store-connect --config
  '{"bundle":{"iOS":{"bundleVersion":"$BUILD_NUMBER"}}}'`; `altool` upload.

Consequence for the port: this workflow checks out the **latest release tag** and builds it with whatever
tooling that tag needs. Until the first native release is tagged it keeps building the Tauri app; after it, the
workflow on `main` must already contain the native build steps. Both must be switched in the same change.

---

## 10. Target: a Tauri-free backend crate

Follows the layout in doc 05 §3 (`crates/core`, `crates/backend`, `crates/scene`, `crates/app`). This section
defines `crates/backend` (package `rustradar-backend`): today's `src-tauri/src` minus Tauri.

### 10.1 Shape

```rust
// crates/backend/src/lib.rs
pub struct Dirs { pub cache: PathBuf, pub config: PathBuf, pub downloads: Option<PathBuf> }

pub struct Backend { /* Arc<Inner> */ }
impl Clone for Backend { /* cheap */ }

pub enum BackendEvent {
    /// A live stream moved trains in the cache: ask trains_in_view again (throttle 1500 ms, section 4.1).
    TrainsChanged,
    /// A newer version is ready: downloaded (desktop) or downloadable (Android).
    UpdateReady { version: String },
}

impl Backend {
    /// Builds the HTTP client, stores the directories, loads keys.json, starts the two Trafikverket
    /// supervisors and the launch login. `events` is how background work reaches the UI; `wake` is called
    /// after every event or completed request so the UI thread redraws (egui: `ctx.request_repaint()`;
    /// winit: `EventLoopProxy::send_event`).
    pub fn start(dirs: Dirs, rt: tokio::runtime::Handle,
                 events: std::sync::mpsc::Sender<BackendEvent>,
                 wake: Arc<dyn Fn() + Send + Sync>) -> Result<Backend, String>;
}
```

`Inner` holds the eight `AppState` fields unchanged, plus `dirs`, `rt`, `events`, `wake`. The module-level
statics stay as they are in the first pass (they work, and tests rely on them); folding them into `Inner` is a
later cleanup, not a port requirement.

### 10.2 Commands become plain functions

Mechanical rules:

- Delete `#[tauri::command]`; delete `State<'_, AppState>` parameters and make the function a method on
  `Backend` (or keep it free where it never used state).
- Replace `AppHandle` parameters by `&self.dirs` (paths) or `&self.events` (emit).
- Keep `Result<T, String>` as the error type for now; the UI prints these strings verbatim today (doc 06), and
  `api.ts`'s `isRateLimited` matches `/status 8|too many requests/i` on the text.
- Async functions stay `async fn`; the UI never awaits them directly (section 10.4).

Signatures (grouped as in section 3):

```rust
impl Backend {
    // Flightradar24
    pub async fn live_flights(&self, area: Option<BoundingBox>, services: Option<Vec<i32>>) -> Result<LiveSnapshot, String>;
    pub async fn flight_details(&self, flight_id: u32) -> Result<FlightDetailsRecord, String>;
    pub fn follow_flight(&self, flight_id: u32) -> Following<FollowEvent>;      // replaces the previous one
    pub fn unfollow_flight(&self);
    pub async fn top_flights(&self) -> Result<Vec<TopFlightRecord>, String>;
    pub async fn search(&self, query: &str) -> Result<Vec<FindEntry>, String>;
    pub async fn reference_data(&self) -> Result<Arc<ReferenceData>, String>;
    pub async fn airport_runways(&self, airports: Vec<String>) -> Result<HashMap<String, Vec<Runway>>, String>;
    pub async fn airport_board(&self, code: &str, mode: board::Mode, page: Option<u32>) -> Result<board::Board, String>;
    pub async fn aircraft_history(&self, registration: &str) -> Result<Vec<FlightListRecord>, String>;
    pub async fn airline_colors(&self, iatas: Vec<String>) -> Result<HashMap<String, Option<LogoColors>>, String>;
    pub async fn opensky_track(&self, icao24: &str) -> Result<Vec<opensky::TrackPoint>, String>;
    // session and keys
    pub fn session_info(&self) -> SessionInfo;
    pub async fn sign_in(&self, email: &str, password: String) -> Result<SessionInfo, String>;
    pub fn sign_out(&self) -> SessionInfo;
    pub fn keys(&self) -> Keys;
    pub fn save_keys(&self, keys: Keys) -> Result<Keys, String>;
    // radar
    pub async fn radar_frame(&self, area: Option<BoundingBox>) -> Result<Vec<radar::VolumeInfo>, String>;
    pub async fn radar_image(&self, source: &str, name: &str) -> Result<Vec<u8>, String>;
    // export
    pub fn save_export(&self, file_name: &str, contents: &str) -> Result<PathBuf, String>;
}

// free functions: they use only module statics today
pub mod adsblol   { pub async fn hex(hex: &str) -> Result<Snapshot, String>;
                    pub async fn callsign(callsign: &str) -> Result<Snapshot, String>; }
pub mod geocode   { pub async fn geocode(query: &str, lat: Option<f64>, lon: Option<f64>, lang: Option<&str>) -> Result<Vec<Place>, String>; }
pub mod ships     { pub async fn in_view(west: f64, south: f64, east: f64, north: f64, zoom: f64) -> Result<Vec<Ship>, String>;
                    pub async fn details(mmsi: u32) -> Result<ShipDetails, String>; }
pub mod trains    { pub async fn in_view(west: f64, south: f64, east: f64, north: f64, zoom: f64) -> Result<Vec<Train>, String>;
                    pub async fn stops(id: &str) -> Result<Vec<Stop>, String>;
                    pub async fn route(id: String) -> Result<TrainRoute, String>;          // spawn_blocking inside
                    pub async fn blocks(west: f64, south: f64, east: f64, north: f64) -> Result<TrainBlocks, String>;
                    pub fn watch(ids: Vec<String>);
                    pub fn hide_operators(names: Vec<String>);
                    pub fn follow(id: String) -> Result<Following<TrainEvent>, String>;
                    pub fn unfollow(); }
pub mod train_media { pub async fn lookup(source: &str, product: Option<String>, operator: Option<String>) -> TrainMedia; }
pub mod weather   { pub async fn at(lat: f64, lon: f64) -> Result<Weather, String>; }
pub mod roads     { pub async fn widths(lon: f64, lat: f64) -> Result<Arc<Vec<RoadPiece>>, String>; }
pub mod taxiways  { pub async fn airport(icao: &str, lat: f64, lon: f64, cache_dir: &Path) -> Result<Arc<Vec<Line>>, String>; }
pub mod trafikverket { pub async fn key_status() -> Result<KeyStatus, String>;   // enum None | Invalid | Ok
                       pub fn product_codes() -> Vec<ProductSeen>; }
pub mod devlog    { pub fn entries(since: Option<u64>) -> Vec<Entry>; pub fn clear(); }
// the URI schemes
pub mod rail          { pub async fn tile(z: u32, x: u32, y: u32) -> Result<Option<Arc<[u8]>>, String>; }
pub mod building_lod  { pub async fn tile(z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>, String>; }
```

`open_devtools` is dropped. `save_export` loses `async` (it never awaited anything); call it from a blocking
task. On phones it needs a different destination (section 11.3).

### 10.3 Channels and events

```rust
/// A running stream: events arrive in order; dropping it (or calling stop) aborts the task.
pub struct Following<T> { pub rx: tokio::sync::mpsc::UnboundedReceiver<T>, task: tokio::task::AbortHandle }
impl<T> Drop for Following<T> { fn drop(&mut self) { self.task.abort() } }
```

- `tauri::ipc::Channel<T>::send(v) -> Result` maps one to one onto `UnboundedSender<T>::send(v)`. A closed
  receiver returns `Err`, exactly the condition `follow_task` and `trafikverket::follow` already use to stop
  (`RS:lib.rs:308-313`, `trafikverket.rs:1790`). After every send call `wake()`.
- The UI drains `rx.try_recv()` each frame. The "replaces any previously followed flight" rule stays inside
  `Backend` (`AppState.follow`), so the UI may also simply drop the old `Following`.
- `app.emit("trains-changed", ())` → `events.send(BackendEvent::TrainsChanged)` + `wake()`.
  `start_operator_streams(app)`, `start_view_streams(app)`, `stream_positions(app, …)` and `move_trains(app, …)`
  (`RS:trafikverket.rs:1372, 1432, 1535, 1570`) take a small `Notifier { events, wake }` clone instead of
  `AppHandle`.
- `app.emit("update-ready", version)` → `BackendEvent::UpdateReady { version }`.
- The 1500 ms throttle on `trains-changed` is frontend logic (`+page.svelte:608-626`) and moves into the app's
  state machine (doc 06 §3).

### 10.4 Calling async backend functions from an immediate-mode UI

The UI thread must never block on the network. One pattern for every request:

```rust
/// app side
pub struct Pending<T>(std::sync::mpsc::Receiver<T>);
impl<T> Pending<T> { pub fn poll(&self) -> Option<T> { self.0.try_recv().ok() } }

pub fn request<T: Send + 'static>(rt: &tokio::runtime::Handle, wake: Arc<dyn Fn() + Send + Sync>,
                                 fut: impl Future<Output = T> + Send + 'static) -> Pending<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    rt.spawn(async move { let _ = tx.send(fut.await); wake(); });
    Pending(rx)
}
```

The JS call sites translate directly: `invoke(...).then(...)` becomes "store a `Pending`, poll it in `update`".
Where the page ignores a stale answer (a `gone` flag or request counter in an `$effect`), drop the `Pending`
and create a new one; the task still completes, its send fails silently.

---

## 11. Replacing each Tauri facility

### 11.1 Paths

| Today | Replacement |
|---|---|
| `app.path().app_cache_dir()` | desktop: `dirs::cache_dir()?.join("com.felix.rustradar")`; Android/iOS: below |
| `app.path().app_config_dir()` | desktop: `dirs::config_dir()?.join("com.felix.rustradar")` |
| `app.path().download_dir()` / `home_dir()` | desktop: `dirs::download_dir().or_else(dirs::home_dir)` |

- Use the `dirs` crate (already a dependency of `fr24`, v6), joining the identifier by hand. That reproduces
  Tauri's desktop folders exactly, so `keys.json` and every cache carry over. The `directories` crate's
  `ProjectDirs::from("com", "felix", "rustradar")` yields different folder names on Linux (`rustradar`) and
  Windows (`felix\rustradar\{config,cache}`), which would orphan the keys file. Do not use it without a
  migration step.
- Android: `dirs` has no notion of the app sandbox. Take the paths from `android_activity::AndroidApp`
  (`internal_data_path()`; cache = the context's `getCacheDir()` via JNI, or `internal_data_path()/cache`).
  Check on a device where the Tauri build put `keys.json` and use the same file so Android users keep their
  keys; if it cannot be matched, look for the old file once at first launch and copy it.
- iOS: `dirs` resolves under the sandbox container's `Library/` **(verify on a device/simulator)**; otherwise
  use `NSSearchPathForDirectoriesInDomains` through `objc2-foundation`.
- Resolve once in the app's `main` and pass `Dirs` to `Backend::start`. The three `set_cache_dir` statics
  (`rail`, `building_lod`, `osm_platforms`) and the signal path derived from rail's
  (`RS:blocks.rs:129-131`) are filled from it exactly as `setup` does today.

### 11.2 Opener

| Today | Replacement |
|---|---|
| `openUrl(url)` (frontend), `app.opener().open_url` (backend) | desktop: the `open` crate (`open::that_detached(url)`) or `webbrowser`; Android: an `ACTION_VIEW` intent through JNI (`webbrowser` does this with `ndk-context` **(verify)**); iOS: `UIApplication.open(_:)` through `objc2-ui-kit` (`webbrowser` supports iOS **(verify)**) |
| `revealItemInDir(path)` | Windows: `explorer /select,"<path>"`; macOS: `open -R <path>`; Linux: D-Bus `org.freedesktop.FileManager1.ShowItems`, falling back to `xdg-open <parent dir>`. The `opener` crate has a `reveal` feature doing this **(verify)**. Phones: not applicable (section 11.3). |

Wrap both in one small module `app::platform::{open_url, reveal}` so call sites stay platform-free. egui's own
`ctx.open_url` / `OpenUrl` output is handled by `egui-winit` with `webbrowser` on desktop; do not rely on it on
phones without testing.

### 11.3 Saving exports on phones

`save_export` writes to the Downloads folder. In the Tauri build on Android/iOS this path is the app's own
sandbox or fails **(verify what happens today on a phone; the feature may already be broken there)**. Natively:
desktop unchanged; Android: write into the app's external files dir and hand it to a share/`ACTION_SEND` or
`ACTION_CREATE_DOCUMENT` intent; iOS: `UIActivityViewController` / `UIDocumentPickerViewController`. Lowest
effort that is honest: desktop writes to Downloads and reveals; phones show the system share sheet. This is new
platform code either way and is a candidate for "after parity".

### 11.4 Async runtime

- Add `rt-multi-thread` to tokio's features and create one runtime in `main`:
  `tokio::runtime::Builder::new_multi_thread().enable_all().build()`. Keep it alive for the process lifetime
  (leak it or hold it in `main`).
- `tauri::async_runtime::spawn(f)` → `handle.spawn(f)`; `spawn_blocking` likewise; `JoinHandle::abort()` is the
  same API. `tauri::async_runtime::JoinHandle<()>` in type positions
  (`RS:lib.rs:67`, `trains.rs:482-483`, `trafikverket.rs:1374, 1538`) → `tokio::task::JoinHandle<()>`.
  `spawn_blocking(...).await` yields `Result<T, JoinError>`; the existing `.map_err(|e| e.to_string())` still
  compiles.
- Hazard: several spawns happen in **synchronous** functions that may be called from any thread
  (`rail::tracks_of`, `osm_platforms::near`, `blocks`, `trains::from_source`, `follow_flight`, `follow_train`).
  Under Tauri that worked from anywhere. `tokio::spawn` panics outside a runtime context, and the winit UI
  thread is outside it. Use a process-global handle: `backend::rt()` returning a `&'static Handle` set once in
  `Backend::start`, and spawn through that everywhere. With `panic = "abort"` a missed call site is a crash, so
  grep for `tokio::spawn` after the port and allow it only inside `async fn`s.
- Never call `Handle::block_on` on the UI thread.

### 11.5 HTTP

No change: `reqwest 0.13` on rustls, clients from `fr24::client_builder()`. Things the webview did over HTTP
that the native app must now do itself (owned by docs 01–04 and 06, listed here so the backend crate grows the
right modules): basemap/vector/terrain/satellite tiles, PMTiles ranges, KTX2 textures (bundled, section 11.8),
airline logos (`https://cdn.flightradar24.com/assets/airlines/logotypes/{id}.png`), aircraft photos (JetPhotos
URLs from `FlightDetails.aircraft.images`), ship photos
(`https://static.vesselfinder.net/ship-photo/{id}/{0|1}`), train photos and logos (Wikimedia), with decoding
through the `image` crate (today only `png` is enabled; JPEG and WebP features are needed for photos) and an
on-disk cache replacing the browser cache. Honour the politeness rules in the README (spacing, back-off).

### 11.6 Events, channels, URI schemes, binary responses

Sections 10.3 and 4.3. `tauri::ipc::Response::new(bytes)` → return `Vec<u8>`. `tauri::http::Response` in
`rail::serve` / `building_lod::serve` → `Result<Option<bytes>, String>` (keep the status logic: 400 → `Err`,
204 → `Ok(None)`, 502 → `Err` after logging).

### 11.7 Window, lifecycle, misc

| Today | Replacement |
|---|---|
| Window config (section 5.2) | winit `WindowAttributes`: title `RustRadar`, `with_inner_size(LogicalSize(1440, 900))`, `with_min_inner_size(LogicalSize(900, 600))`, `with_maximized(true)`, `with_visible(false)` until the first frame is ready (avoids a white flash; clear colour RGBA 4, 6, 11, 255), window icon from `icons/icon.png` (winit `with_window_icon` on Windows/X11) |
| Start hidden, update first | Run the update check before creating the main window, or create it hidden; splash = a second small undecorated winit window (380 × 150, always on top, centred, not resizable) drawn with egui to the spec in 5.2 |
| `app.exit(0)` | `event_loop.exit()` |
| `tauri::process::current_binary` | `std::env::current_exe()`, except under AppImage use the `APPIMAGE` env var (the path of the AppImage file itself); this is what `current_binary` does and why `updater.rs:172-175` uses it |
| `app.package_info().version` | `env!("CARGO_PKG_VERSION")` |
| `#![windows_subsystem = "windows"]` in release | keep in the new `main.rs` |
| `tauri-build` (Windows icon + manifest resource) | a `build.rs` using `winresource` (or `embed-resource`) to embed `icon.ico`, version info and a manifest declaring per-monitor-v2 DPI awareness and common-controls v6 |
| `cfg(desktop)` / `cfg(mobile)` | `cfg(not(any(target_os = "android", target_os = "ios")))` / `cfg(any(...))` |
| `open_devtools`, `devtools` feature | none; an egui debug overlay if wanted |
| `window.eval` (splash progress) | plain state shared with the splash UI |
| Webview user-agent sniffing for Android/Windows (`rail-layer.ts:312`, `UpdateToast.svelte:15`) | `cfg!(target_os = …)` |
| `isTauri()` guards and browser-only fallbacks/demo paths (`+page.svelte:167, 581, 1338, 1487` and others) | always true; delete the fallbacks |
| Single-instance | not enforced today; nothing to port |

### 11.8 Bundled assets

Tauri embeds `static/` in the binary. Options natively:

1. `include_bytes!` everything (about 27 MB added to the binary). Works identically on all five platforms, keeps
   the desktop updater a single-file swap, no resource lookup code. Recommended for parity.
2. Platform resources (next to the exe; `Contents/Resources`; Android `assets/` through `AAssetManager`; iOS
   bundle). Smaller RAM footprint on phones, but five lookup paths and installer changes.

Either way move the files to a neutral folder (for example `assets/`) since `static/` is a SvelteKit
convention. The KTX2 files are Basis Universal (UASTC and ETC1S) and need a native transcoder; that is doc 04's
subject, noted here because it adds a C++ dependency (`basis-universal` crate) to every cross-compile
(Android NDK, iOS) **(verify it builds for all targets early)**.

---

## 12. The updater without Tauri

### 12.1 The compatibility constraint

Existing desktop installs run the Tauri updater against
`https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json`. For them to reach the native app:

- That URL must keep serving a manifest in the Tauri format (section 7.2).
- Its entries must point at artifacts the Tauri updater knows how to install: Windows, an NSIS installer that
  tolerates the arguments Tauri passes for a quiet update and relaunches the app at the end; macOS, a
  `.app.tar.gz` whose top-level entry is `RustRadar.app`; Linux, an AppImage.
- Each must be signed with the **same** minisign key (the one behind `TAURI_SIGNING_PRIVATE_KEY`), with the
  signature encoded the way Tauri expects (base64 of the `.sig` file's text).
- The bundle identifier, the Windows install location / uninstall registry key and the product name must stay
  the same, or the old version is left installed beside the new one.

Simplest policy: keep that manifest format and that key permanently, so there is one updater format before and
after the port.

### 12.2 Recommended implementation

Option A (recommended, lowest risk): **cargo-packager + cargo-packager-updater** (CrabNebula; the Tauri bundler
and updater extracted for any Rust binary) **(verify current state, target support and key compatibility before
committing)**. It is expected to give: the same NSIS/WiX templates (so the installer accepts the update flags
the old Tauri updater sends), `.app` + `.dmg`, `.deb`, AppImage, pacman; a signer using the same minisign
scheme; and an updater crate with the same `check → download → verify → install` flow and manifest shape as
`tauri-plugin-updater`. If its signer cannot read the existing Tauri key file, sign with the Tauri CLI's
`tauri signer sign` (a standalone binary via `cargo install tauri-cli`) for the artifacts only.

Option B (hand-rolled, fully owned): a module `app::update` of roughly 300 lines.

```rust
pub struct Manifest { version: semver::Version, notes: Option<String>, platforms: HashMap<String, Entry> }
pub struct Entry { url: String, signature: String }
pub async fn check(http: &reqwest::Client, current: &Version) -> Result<Option<Update>, Error>;  // 8 s timeout at the call site
impl Update {
    pub async fn download(&self, on_chunk: impl FnMut(usize, Option<u64>)) -> Result<Vec<u8>, Error>; // verifies before returning
    pub fn install(&self, bytes: Vec<u8>) -> Result<(), Error>;
}
```

- Verification: `minisign-verify` (already in `Cargo.lock` through the plugin) with the public key compiled in
  (the value currently at `tauri.conf.json` → `plugins.updater.pubkey`; it is a public key, base64 of a
  minisign `.pub` file). Tauri signatures are pre-hashed minisign signatures **(verify by checking a released
  `.sig` with the new code before shipping)**.
- Install, Windows: write the bytes to `%TEMP%\RustRadar-<version>-setup.exe`, start it with `/S` (and whatever
  relaunch flag the installer script defines), exit immediately. The installer replaces the files and starts
  the app.
- Install, macOS: unpack the `.tar.gz` into a temp dir on the same volume, rename the running bundle aside,
  rename the new one into place, delete the old (roll back on failure). The bundle path is `current_exe()`
  three levels up (`RustRadar.app/Contents/MacOS/rustradar`). If the app sits in a folder the user cannot write
  (e.g. `/Applications` owned by another admin) this fails; fall through to "open the installed version".
- Install, Linux: only when `APPIMAGE` is set: write the new AppImage beside the old, `chmod +x`, rename over it
  (the `self-replace` crate does the same atomically for a plain binary). `.deb` / `.rpm` / Arch installs do not
  self-update; the check should not offer one (detect: `APPIMAGE` unset).
- Relaunch: as `updater.rs:172-184` (same `--updated` flag and argument filtering).
- Signing in CI: the `minisign`/`rsign2` crates behind a `cargo xtask sign <file>` reading the key and password
  from the same two secrets, or the Tauri CLI's signer as above.
- Manifest in CI: `cargo xtask latest-json` writes `latest.json` from the artifact list; upload with
  `gh release upload`.

Either option keeps `updater.rs`'s control flow (section 7.1) nearly line for line: only `app.updater()`,
`Update`, the splash window and `relaunch` change. Carry the constants: `CHECK_TIMEOUT = 8 s`,
`WATCH_EVERY = 3600 s`, `UPDATED_FLAG = "--updated"`, progress pushed at most once per 1 %.

### 12.3 Phones

`mobile_update.rs` already contains no Tauri logic beyond `AppHandle` plumbing: `check` needs an HTTP client and
`env!("CARGO_PKG_VERSION")`; `emit` becomes `BackendEvent::UpdateReady`; `open_url` becomes
`platform::open_url`. `newer_apk`, `parse_version` and the test move unchanged. iOS still does nothing.

Possible later improvement (not parity): download the APK in-app and fire the package installer intent
(`REQUEST_INSTALL_PACKAGES` permission + `FileProvider`). Today's behaviour (open the URL in the browser) needs
neither, so the manifest's `FileProvider` can be removed for now.

---

## 13. Packaging the winit + wgpu + egui app

Common: one binary crate `crates/app` (package `rustradar`, keep the package name so `Cargo.lock`'s entry and
the version tooling stay simple), with `[lib] crate-type = ["rlib", "cdylib", "staticlib"]` plus `src/main.rs`
for desktop, the same split as today. Desktop entry `fn main()`; Android entry
`#[unsafe(no_mangle)] fn android_main(app: AndroidApp)`; iOS entry: either the Rust `main` built as the app
executable, or a staticlib called from a tiny `main.m`.

### 13.1 Windows

- Build: `cargo build --release --target x86_64-pc-windows-msvc` (today's only Windows target; ARM64 is not
  shipped).
- Resource/manifest/icon through `build.rs` (section 11.7).
- Installers today: NSIS `-setup.exe` and WiX `.msi`. Keep NSIS as the primary (it is what the updater uses);
  the MSI is optional and can be dropped unless someone depends on it (open question).
  Tools: cargo-packager (both), or a checked-in `.nsi` script run by `makensis` (preinstalled on
  `windows-latest` **(verify)**), or `cargo-wix` for MSI.
- Install scope and location must match the Tauri NSIS default (per-user,
  `%LOCALAPPDATA%\RustRadar`, uninstall key under `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\`
  with the product name) **(verify against an installed copy)** so the new installer upgrades in place.
- WebView2 is no longer needed: remove the bootstrapper the Tauri installer embeds/downloads.
- Runtime: wgpu uses DX12 (default) or Vulkan; no extra DLLs to ship with a static CRT
  (`-C target-feature=+crt-static`) or the VC++ runtime otherwise **(decide; Tauri builds link the CRT
  dynamically and rely on it being present)**.
- Code signing: none today (SmartScreen warns). Unchanged unless the owner wants it.

### 13.2 macOS

- Build both `aarch64-apple-darwin` and `x86_64-apple-darwin` from the arm64 runner (as today), separately; a
  universal binary via `lipo` is optional and would change the updater's platform keys, so keep two builds.
- Bundle: `RustRadar.app/Contents/{Info.plist, MacOS/rustradar, Resources/icon.icns}`. `Info.plist` needs
  `CFBundleIdentifier = com.felix.rustradar`, `CFBundleName`/`CFBundleDisplayName = RustRadar`,
  `CFBundleShortVersionString` and `CFBundleVersion` = the version, `CFBundleExecutable`, `CFBundleIconFile`,
  `NSHighResolutionCapable = true`, `LSMinimumSystemVersion` (Tauri's default is 10.13; wgpu/Metal needs a
  Metal-capable system, pick 10.15 or 11.0 **(decide)**), `NSPrincipalClass = NSApplication`,
  `LSApplicationCategoryType`. Tools: cargo-packager or `cargo-bundle`, or a 30-line script.
- Disk image: `hdiutil create -volname RustRadar -srcfolder <staging with RustRadar.app and an /Applications
  symlink> -format UDZO RustRadar_{version}_{arch}.dmg` (or `create-dmg`).
- Updater artifact: `tar -czf RustRadar.app.tar.gz RustRadar.app` + signature.
- Signing/notarization: not done today. Apple silicon requires at least an ad-hoc signature on the executable
  (the linker adds one); re-sign the finished bundle ad hoc (`codesign --force --deep -s - RustRadar.app`) so
  the bundle seal is valid after resources are added. Real distribution would be
  `codesign --options runtime -s "Developer ID Application: …"` then `xcrun notarytool submit --wait` with an
  App Store Connect key and `xcrun stapler staple`, on both the `.app` and the `.dmg`. That needs the Developer
  ID secrets listed as commented out in 9.2. The existing `APPLE_API_*` secrets can drive `notarytool`, but a
  Developer ID certificate is still required.
- No entitlements are needed for outbound HTTPS without the hardened runtime/sandbox.

### 13.3 Linux

- Build on `ubuntu-22.04` (glibc 2.35 baseline, as today) for `x86_64-unknown-linux-gnu`.
- Build-time system packages: none of the WebKitGTK set. winit/wgpu load X11, Wayland, xkbcommon and
  Vulkan/GL at run time; `pkg-config` and headers are only needed if a dependency links them (for example a
  GTK file dialog; the port needs no file dialog today because exports go straight to Downloads and the keys
  import reads a path/drag-and-drop, see doc 06 §20 **(verify how the keys import picks a file natively)**).
- Run-time dependencies to declare: `libc6`, `libgcc-s1`, `libxkbcommon0`, `libwayland-client0` or `libx11-6` +
  `libxcursor1` + `libxi6` + `libxrandr2`, `libvulkan1` or `libgl1`/`libegl1`, `fontconfig` if system fonts are
  used (the app ships Overpass, so not needed).
- `.deb`: `cargo-deb` (metadata in `Cargo.toml`: depends, assets for the binary, the `.desktop` file and
  hicolor icons 32/128/256). `.rpm`: `cargo-generate-rpm`. AppImage: `linuxdeploy` + `appimagetool`
  (`AppDir/usr/bin/rustradar`, `rustradar.desktop`, icon), or cargo-packager. Arch: a `PKGBUILD` in the
  repository (a `-bin` package repackaging the release tarball or `.deb` is the cheapest), or cargo-packager's
  pacman target; Arch is **not** produced today, so it is an addition, not parity.
- `.desktop` file: `Name=RustRadar`, `Exec=rustradar`, `Icon=rustradar` (Tauri names these after the product /
  binary; keep the same names so the package upgrades cleanly **(verify against the current `.deb`)**),
  `Categories=Utility;`, `StartupWMClass` = the winit app id; set the Wayland `app_id` / X11 `WM_CLASS` in winit
  (`with_name("rustradar", "rustradar")`) to match.
- AppImage must stay named and laid out so the old updater's in-place overwrite produces a working file (any
  valid AppImage does).

### 13.4 Android

- Activity glue: winit's Android backend runs on `android-activity`, in one of two flavours:
  - **GameActivity** (recommended): a Jetpack `GameActivity` subclass (a few lines of Kotlin/Java, `androidx.games:games-activity`
    AAR, `AppCompat` theme). Better input (IME/soft keyboard via `GameTextInput`, which the search box and keys
    dialog need), insets API.
  - NativeActivity: no Java at all, declared purely in the manifest, but soft-keyboard text input is poor. Not
    suitable for an app with text fields.
- Keep the existing Gradle project (move `src-tauri/gen/android` → `platform/android`), and strip Tauri from it:
  delete `buildSrc/` (the `rust` plugin), `id("rust")`, the `rust { }` block,
  `apply(from = "tauri.build.gradle.kts")`, `apply from: 'tauri.settings.gradle'`, the `tauri.properties`
  lookup, `androidx.webkit`, the `FileProvider` (unless section 11.3 needs it); change `MainActivity` to extend
  `GameActivity`, load the library (`System.loadLibrary("rustradar")` or
  `<meta-data android:name="android.app.lib_name" android:value="rustradar"/>`), keep `enableEdgeToEdge()`-like
  behaviour and pass window insets to Rust (winit/android-activity exposes the content rect; egui must pad by
  it, doc 06 §2 phone layout).
- Native build: `cargo ndk -t arm64-v8a -t armeabi-v7a -t x86 -t x86_64 -o platform/android/app/src/main/jniLibs
  build --release -p rustradar --lib` (cargo-ndk), then `./gradlew assembleRelease`. `libc++_shared.so` must be
  packaged if any C++ dependency (GameActivity glue, Basis transcoder) links it dynamically.
  Alternative: `xbuild` (`x build --platform android --arch arm64 --format apk`) builds an APK without Gradle
  but supports NativeActivity only and gives less control over the manifest/signing **(verify)**; not
  recommended here because the Gradle project and signing setup already exist.
- `versionCode`/`versionName`: pass from CI as Gradle properties
  (`-PversionName=$v -PversionCode=$((major*1000000 + minor*1000 + patch))`), section 8.1.
- Signing: unchanged (`keystore.properties`, same three secrets, same key).
- Graphics: wgpu → Vulkan where available, GLES 3 fallback (`minSdk 24`). Old/low-end devices with broken
  Vulkan drivers are a known wgpu pain point; allow forcing GLES via a setting or env.
- Lifecycle: the surface is destroyed on `Suspended` and recreated on `Resumed`; all wgpu surface-bound state
  must be rebuilt, and background network tasks should pause (streams keep running today in the webview only
  while the activity is alive).
- TLS: already handled (section 2.3). JNI needs (`open_url`, cache dir, share sheet): `jni` + `ndk-context`.
- 16 KB page size: NDK r27 with default flags produces compatible libraries for `targetSdk 35+` **(verify with
  the Play/`zipalign -c -P 16` check)**; relevant only if the APK ever goes to Play.

### 13.5 iOS

- winit supports iOS (UIKit), wgpu uses Metal. Targets `aarch64-apple-ios` (device), `aarch64-apple-ios-sim`
  (simulator).
- Xcode project: check in an `xcodegen` spec (`platform/ios/project.yml`; `xcodegen` is already installed by
  CI) describing one app target: bundle id `com.felix.rustradar`, deployment target (Tauri's default is iOS
  13/14; pick one with solid Metal + winit support, e.g. 15.0 **(decide)**), `Info.plist` with
  `ITSAppUsesNonExemptEncryption = false`, `UILaunchStoryboardName` or `UILaunchScreen` (required for
  full-screen on modern devices), supported orientations, `UIRequiresFullScreen` as wanted,
  `CFBundleShortVersionString = $(MARKETING_VERSION)`, `CFBundleVersion = $(CURRENT_PROJECT_VERSION)`; the
  asset catalog with the flattened icons from `icons/ios/`; a pre-build script phase running
  `cargo build --release --target <arch from $ARCHS/$PLATFORM_NAME> -p rustradar --lib` and linking
  `librustradar.a`, plus frameworks `UIKit`, `Metal`, `QuartzCore`, `CoreGraphics`, `Foundation`, `Security`.
  Alternative: `cargo-xcode` generates such a project from `Cargo.toml` **(verify it handles app targets and
  not only libraries)**. No CocoaPods needed any more.
- Unsigned IPA: `xcodebuild -scheme RustRadar -configuration Release -sdk iphoneos -derivedDataPath build
  CODE_SIGNING_ALLOWED=NO`, then `mkdir Payload && cp -r build/…/RustRadar.app Payload/ && zip -r
  RustRadar_{version}_unsigned.ipa Payload`.
- App Store / TestFlight: `xcodebuild archive -allowProvisioningUpdates -authenticationKeyPath <p8>
  -authenticationKeyID $APPLE_API_KEY_ID -authenticationKeyIssuerID $APPLE_API_ISSUER
  DEVELOPMENT_TEAM=$APPLE_DEVELOPMENT_TEAM MARKETING_VERSION=$v CURRENT_PROJECT_VERSION=$build`, then
  `xcodebuild -exportArchive -exportOptionsPlist ExportOptions.plist` (`method = app-store-connect`,
  `signingStyle = automatic`), then the same `xcrun altool --upload-app`. This is what `tauri ios build
  --export-method app-store-connect` wraps.
- Known weak spots: on-screen keyboard and text input through winit on iOS, safe-area insets (notch, home
  indicator), and app suspend/resume. Prototype a text field and the bottom sheet on a device before building
  the rest (section 17).

### 13.6 Identity that must not change

| | Value |
|---|---|
| Bundle id / applicationId / identifier | `com.felix.rustradar` |
| Product name | `RustRadar` |
| Android signing key | the keystore behind `ANDROID_KEYSTORE` |
| Updater key | the key behind `TAURI_SIGNING_PRIVATE_KEY` |
| Updater endpoint | `https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json` |
| Release tag scheme | `vX.Y.Z` (`mobile_update` parses it; `testflight-refresh` and `release-version.sh` match on it) |
| APK asset name | must end in `.apk` (first such asset wins); today `RustRadar_{version}.apk` |
| Config/cache folders | section 6 |

---

## 14. CI after the port

### 14.1 `rust.yml`

| Job | Changes |
|---|---|
| `check` | Remove bun, `bun install`, svelte-check, `bun test`, `bun run build`, and the WebKitGTK apt packages. Keep `cargo test --workspace --locked`. Replace `bun run bump --check` (section 15.1). Add `cargo clippy --workspace` if wanted. If wgpu tests need a GPU, gate them (`#[ignore]` or a `gpu` feature) or install `mesa-vulkan-drivers` (lavapipe) for software rendering. Add `cargo check` for the mobile targets here or rely on the mobile jobs. |
| `version` | Unchanged if the script keeps working; it reads `package.json`, so point it at `Cargo.toml` (section 15.1). |
| `bundle` | Replace `tauri-action` with explicit steps per OS: build (`cargo build --release --locked --target …`), package (section 13.1–13.3), on release sign the updater artifacts and write `latest.json`, `actions/upload-artifact` for every run, and on release create the GitHub release once and upload. `tauri-action` created the release from whichever matrix leg got there first and merged `latest.json` across legs; do this explicitly: a new final job `release` (needs all `bundle` legs) downloads all artifacts, builds one `latest.json`, runs `gh release create v$VERSION --title "RustRadar v$VERSION" --notes "Installed copies update to this version automatically the next time they open."` (not a draft, not a prerelease) and uploads everything. Keep `scripts/check-signing-key.sh` (it is bash, not TypeScript). |
| `android` | Remove bun. Add `cargo install cargo-ndk --locked` (or `taiki-e/install-action`). Build with cargo-ndk + `./gradlew assembleRelease` (signed) or `assembleDebug` with only `arm64-v8a` (debug). Keystore step: change the path to the new project folder. APK path changes to `platform/android/app/build/outputs/apk/...`. |
| `ios` | Remove bun, CocoaPods, `tauri ios init`, `tauri icon`. `xcodegen generate` from the checked-in spec, then the `xcodebuild` commands in 13.5. |
| `mobile-release` | Unchanged, but must run after the new `release` job. |

Cache keys can stay; drop the `bun.lock`-dependent steps. Build times: no frontend build, but wgpu/naga and
shader validation add Rust compile time; LTO + `codegen-units = 1` already dominates.

### 14.2 `bump.yml` and `testflight-refresh.yml`

- `bump.yml`: only the version command changes.
- `testflight-refresh.yml`: replace `node scripts/testflight-age.mjs` (section 15.1), the bun/CocoaPods/Tauri
  steps and the build command (`CURRENT_PROJECT_VERSION=$BUILD_NUMBER` replaces the `--config` JSON). Because it
  builds the latest **tag**, land these changes in the same commit range as the first native release
  (section 9.4), and expect one awkward window: if the workflow's steps are native but the newest tag is still
  a Tauri build, the refresh fails. Mitigation: make the job detect which tree it checked out
  (`[ -f src-tauri/tauri.conf.json ]`) for the transition, or temporarily disable the schedule until the first
  native tag exists.

### 14.3 Things `tauri-action` did implicitly that now need explicit steps

- Creating the release and tolerating it already existing.
- Naming artifacts (`[name]_[version]_[platform]_[arch]_[bundle]`); installed-app update URLs come from
  `latest.json`, not from names, but README links and Obtainium match on the release's asset list (`.apk`).
- Producing `.sig` files and merging per-platform entries into one `latest.json`.
- Setting the version inside the bundles (now only from `Cargo.toml`/`CARGO_PKG_VERSION`, which is simpler).

---

## 15. Scripts and the "no TypeScript" rule

None of `scripts/` is TypeScript: they are JavaScript (`.mjs`), Python and bash. Whether they must go is an
owner decision (section 17). Facts:

### 15.1 Release tooling (run by CI)

| Script | Does | Depends on | After the port |
|---|---|---|---|
| `scripts/version.mjs` | Reads/sets the version in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `Cargo.lock` | Node | The version shrinks to two places: the app crate's `Cargo.toml` and its `Cargo.lock` entry. Replace with `cargo xtask bump [patch|minor|major|x.y.z|--check]` (a tiny `xtask` crate in the workspace) or `cargo set-version` from `cargo-edit` plus `cargo update -p rustradar --offline`. |
| `scripts/release-version.sh` | Picks the release version (section 9.3) | bash, node (one `node -p` to read `package.json`), git | Keep; replace the `node -p` with `cargo metadata --no-deps --format-version 1 \| jq` or `cargo xtask version`. |
| `scripts/check-signing-key.sh` | Validates the signing-key secret's shape without printing it | bash, base64 | Keep unchanged while the Tauri-format key is used. |
| `scripts/testflight-age.mjs` | Asks App Store Connect for the newest build's age | Node (crypto, fetch), reads the bundle id from `tauri.conf.json` | Port to `cargo xtask testflight-age` (`p256`/`jsonwebtoken` + `reqwest`), or keep the script and hard-code the bundle id (Node is preinstalled on GitHub runners; no `package.json` needed). |

### 15.2 Offline asset generators (run by hand; outputs are committed)

| Script | Input | Output | Depends on |
|---|---|---|---|
| `scripts/aircraft/build.mjs` (+ `gltf1.mjs`) | Flightradar24's glTF 1.0 models, downloaded to `.cache/fr24-models/` | `static/aircraft/meshes.bin`, `index.json` (two LODs: 220 and 2600 triangles, vertex clustering), silhouette atlases into `textures/aircraft/` (128 px cells; outline radius 9, pad 12) | Node (zlib) |
| `scripts/trains/build.mjs` (+ `collada.mjs`) | SketchUp Collada exports in a folder given as an argument (8 models, cut positions in the script) | `static/trains/meshes.bin`, `index.json` (3 cars per model, 1500 triangles each, unit box 3.1 m wide × 4.1 m high) | Node, an external image tool for textures' average colour |
| `scripts/compress-textures.mjs` | `textures/realistic/*.webp`, `textures/aircraft/*` | `static/realistic/*.ktx2`, `static/aircraft/*.ktx2` (Basis Universal: UASTC for colour/normal/mask/icons, ETC1S for the two Earth pictures, with mip levels) | Node, **`node_modules/@loaders.gl/textures`'s Basis encoder**, Python + Pillow; reads layer order from `src/lib/realistic-textures.ts` |
| `scripts/realistic-textures.py` | a Streets GL checkout | `textures/realistic/*.webp` (512 px; glow 128 px) | Python, Pillow, NumPy |
| `scripts/earth-texture.py`, `night-texture.py` | NASA Blue Marble / Black Marble JPEGs | `textures/realistic/earth.color.webp`, `earth.night.webp` (4096 × 2048) | Python, Pillow, NumPy |
| `scripts/build-rail-overview.py` | Natural Earth railroads GeoJSON | `static/rail/railroads.bin` (`RRO1` format) | Python stdlib |
| `scripts/street-trees.py` | Malmö and Umeå open data | `static/street-trees/{malmo,umea}.bin` (`RST1` format) | Python stdlib |
| `scripts/flow.py`, `scripts/sth.py` | Trafikverket GeoPackages | `crates/backend/data/flow.json`, `crates/backend/data/sth.json` (moved from `src-tauri/data/` in stage 1) | Python stdlib |

All outputs are checked in, so none of these block the port. Two break when the frontend is deleted:
`compress-textures.mjs` loses its encoder (it lives in `node_modules`) and its layer-order source
(`src/lib/realistic-textures.ts`). If the textures will ever be regenerated, either keep a minimal
`package.json` under `scripts/` for tooling, switch to the `basisu` command-line encoder, or port to Rust with
the `basis-universal` crate's encoder; and move the layer lists to a data file both the tool and the renderer
read. Output paths (`static/…`) change if assets move (section 11.8).

---

## 16. Suggested migration order (backend and packaging only)

1. Create `crates/backend` by moving `src-tauri/src/*.rs` and `src-tauri/{data,tests}`; apply sections 10–11
   mechanically (attributes, `AppHandle`, runtime, cfg aliases). `cargo test -p rustradar-backend` must pass
   with the existing tests (about 190 `#[test]` functions) unchanged. Keep `src-tauri` building against it as a thin shim
   (`#[tauri::command]` wrappers calling the backend) for as long as the Svelte app is needed for side-by-side
   comparison. This step alone removes Tauri from everything except ~300 lines.
2. `crates/app`: winit window + wgpu clear + egui "hello", with `Backend::start`, the request/poll pattern, and
   one real request (top flights). Build it for all five targets in CI immediately (unsigned, artifacts only)
   so platform problems surface while the code is small: Android GameActivity + soft keyboard, iOS project +
   keyboard + safe area, Basis transcoder cross-compiles.
3. Updater module (section 12) behind a feature, tested against a throwaway GitHub repository with the real
   manifest format and a test key; then verify, on a machine with the current Tauri release installed, that the
   old app updates itself to a native build (Windows, macOS, AppImage).
4. Packaging and the new release job; publish native builds as pre-releases (not picked up by
   `releases/latest`) until parity is signed off.
5. Switch: delete `src/`, `src-tauri/`, `package.json`, `bun.lock`, `node_modules`, `patches/`,
   `svelte.config.js`, `vite.config.js`, `tsconfig.json`, `.svelte-kit/`, `static/updater.html`; update
   `README.md`, `.gitignore`, the three workflows.

---

## 17. Risks and open questions

Questions for the owner:

1. **Scripts**: does "no more TypeScript" also mean no JavaScript/Python tooling (`scripts/*.mjs`, `*.py`)?
   Section 15 assumes release tooling moves to Rust (`xtask`) and offline asset generators may stay.
2. **Settings migration**: the webview's `localStorage` (`rustradar.v1`: mode, basemap, filters, …) is not
   readable natively. Accept a one-time reset to defaults, or invest in reading WebKit/WebView2/WKWebView
   storage per platform (fragile; not recommended)?
3. **Windows MSI**: keep shipping an `.msi` beside the NSIS installer, or NSIS only?
4. **Arch package**: wanted as a new deliverable (not produced today)? `-bin` PKGBUILD, or just the AppImage?
5. **macOS signing/notarization and Windows code signing**: stay unsigned as today, or add (needs a Developer ID
   certificate / a code-signing certificate and new secrets)?
6. **Minimum OS versions**: macOS (10.15 or 11?), iOS (15?), Android stays `minSdk 24`?
7. **Packaging tool**: adopt cargo-packager (+ its updater) or own the scripts and a ~300-line updater? Affects
   how much is "pure Rust in this repository".
8. **Exports on phones**: share sheet (new platform code) or desktop-only for the first native release?
9. **Assets**: embed all 27 MB in the binary (simple, same everywhere) or ship as platform resources?
10. **Dev tooling**: the web inspector shortcut (Cmd/Ctrl+Alt+I) disappears; is an egui debug overlay wanted in
    its place? The dev panel (Ctrl/Cmd+Shift+D, `devlog`) ports as is.

Risks:

- **Stranding existing installs** if the first native release is not installable by the old Tauri updater
  (wrong NSIS flags, different install location, wrong signature encoding, missing `latest.json` platform key).
  Test the real upgrade path on all three desktop OSes before publishing to `releases/latest`.
- **Android upgrade refused** if `versionCode` drops or the signing key differs; verify the current formula on
  a released APK first.
- **Tokio context panics** from `tokio::spawn` on non-runtime threads (section 11.4), fatal under
  `panic = "abort"`.
- **Text input on phones** (winit + egui): soft keyboard show/hide, IME composition, and Swedish characters
  (å, ä, ö are needed for station search). GameActivity on Android and UIKit text input on iOS are the least
  proven parts of the stack. Prototype first.
- **Safe areas / edge-to-edge** on both phones: the web build gets insets from CSS `env()`; natively they must
  be queried and applied by hand.
- **Surface loss and lifecycle** on Android/iOS (suspend/resume) with a large wgpu scene.
- **Driver variance**: Vulkan on older Android, GL fallback on Linux without Vulkan, DX12 on old Windows GPUs.
  The webview hid this behind ANGLE/WebGL.
- **Basis/KTX2 transcoding** adds a C++ dependency to every cross build.
- **Binary size and memory**: embedding assets adds ~27 MB to each binary and to the Android APK per ABI unless
  assets go into the APK's `assets/` once (a reason to prefer resources on Android specifically).
- **TestFlight refresh during the transition** builds the newest tag with `main`'s workflow (section 14.2).
- **AppImage relaunch** must use `$APPIMAGE`, not `current_exe`.
- **macOS self-update without signing**: replacing an unsigned bundle works today; if signing is added later,
  the replaced bundle must be signed with the same identity or Gatekeeper re-prompts.
- **`dirs` on iOS/Android**: do not assume; verify the resolved folders on devices before the first build that
  writes `keys.json`.
- **Image decoding**: photos and logos need JPEG/WebP decoders (`image` features) and an HTTP image cache that
  the webview provided for free; unbounded caches on phones are a storage risk.
- **Third-party tool claims marked (verify)** in this document (cargo-packager capabilities and key
  compatibility, `webbrowser`/`opener` platform coverage, `xbuild`, `cargo-xcode`, exact Tauri updater manifest
  keys and NSIS flags, Tauri's Android path and `versionCode` rules) were written from knowledge of those
  projects, not checked against their current releases from this sandbox. Confirm each before depending on it.
