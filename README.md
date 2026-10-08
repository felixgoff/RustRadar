# RustRadar

A live flight radar on a 3D globe: Tauri 2 + SvelteKit (Svelte 5) + deck.gl 9,
backed by a native Rust port of the [fr24](https://github.com/abc8747/fr24)
Python library.

- Every live aircraft (~20k worldwide), drawn from the real shape of its type.
  Around 99.7% of live traffic resolves to one of 40 aircraft models; the rest
  (fighters, drones, balloons, ground vehicles) keep the built-in low-poly
  shapes. What you see depends on the camera:
  - **Flat and wide** — a top-down silhouette of the actual type, which is how
    a map read from straight above wants to look.
  - **Tilted** — the real mesh, about 220 triangles, enough for the planform,
    engines and fin.
  - **Zoom 11 and closer** — the same mesh at about 2,600 triangles,
    tilted or not.
- On screen aircraft are true to scale but clamped between 8 px (whole globe,
  rising to 24 px at city level) and 72 px. They are dead-reckoned between feed
  refreshes so they move smoothly, and altitude is exaggerated when zoomed out
  and true to scale up close.
- Airline liveries: zoomed in, aircraft take their airline's colours on the
  fuselage, belly, tail and engines. About 60 high-traffic airlines have a
  hand-made scheme; every other airline gets colours derived from its logo
  (cached on disk). Military aircraft are grey.
- Click an aircraft (or pick one from search, "Most tracked" or an airport
  board) and the camera centres on it and tracks it, aimed at the aircraft's
  altitude rather than the ground beneath it, so zooming closes on the aircraft
  itself even when the view is tilted. Drag the map, or press `C`, to let go.
  The panel and the 3D trail (coloured by altitude) update live from FR24's
  streaming `FollowFlight` RPC. The panel shows the airline, origin and
  destination with local times and delays, an altitude profile chart, the
  route, aircraft details (serial number, age, photos) and the aircraft's
  recent flights. `C` makes the camera follow it.
- Routes: with a Flightradar24 subscription that includes flight plans, the
  filed route (airways and waypoints of the current AIRAC cycle) is drawn and
  its ICAO route string shown. Without one, the map shows the track flown so
  far and the great circle to the destination, labelled as an estimate. The
  flown track is Flightradar24's, with the earlier part filled in from the
  OpenSky Network's tracks API when it saw more of the flight (anonymous
  access allows about 400 calls a day, so tracks are cached and fetched only
  for the open flight, at most every five minutes).
  Flightradar24 only sends flight plans to signed-in subscribers, and there is
  no free worldwide source of filed routes.
- Filters: aircraft categories (filtered by the server, so hidden categories
  cost nothing), altitude range, airborne or on ground, airlines, aircraft
  types, airports and callsign/flight/registration text. They persist between
  sessions, as do the map settings.
- Airport boards: click an airport's code on the map (or in the flight panel,
  or search for it) for live departures and arrivals in local time, with
  status, gate and aircraft. Airborne flights open on the map.
- Day and night shading with civil, nautical and astronomical twilight, and an
  optional 3D weather radar (`W`). Heights are measured where a weather service
  publishes 3D reflectivity, and estimated from intensity elsewhere.
  - **The contiguous US and southern Canada** — NOAA MRMS: reflectivity
    actually measured, about 1 km resolution, quality-controlled (no bird or
    insect clutter), new every two minutes, drawn at 18 heights: 1, 1.5, 2,
    2.5, 3, 3.5, 4, 4.5, 5, 5.5, 6, 7, 8, 9, 10, 11, 12 and 14 km.
  - **Central Europe** — the DWD WN composite (roughly 1.4-18.8°E,
    45.7-56.2°N: Germany plus neighbouring countries): reflectivity on a 1 km
    grid, refreshed every five minutes, lifted to heights estimated from
    intensity (WN is a nowcast; only its analysis frame is drawn). Free open
    data from Deutscher Wetterdienst, no key needed.
  - **Elsewhere** — RainViewer, lifted to heights estimated from the tile
    colour.

  Tilt the camera to see storm cores rise as columns; looking straight down
  shows the flat composite.
- A dark vector map drawn from OpenFreeMap tiles (water, borders, roads,
  runways and taxiways), or satellite imagery. Place and airport names are a
  separate text layer, never baked into the map. 3D buildings from zoom 13.
- Tilt and rotate (right-drag, or the map controls) down to street level.

## Layout

```
crates/fr24/     Rust port of fr24: gRPC-web + protobuf, JSON API, auth (see its README)
src-tauri/       Tauri backend: commands wrapping the fr24 client, airport
                 boards, reference data (airlines, airports; cached weekly),
                 logo-derived livery colours, sign-in
src/app.css      Design tokens and global styles
scripts/aircraft/ Offline pipeline: reads the Flightradar24 3D models and
                 writes static/aircraft/ (meshes + silhouettes)
static/aircraft/ Generated aircraft meshes, silhouette atlas and type index
src/lib/         Svelte UI: Globe (deck.gl), aircraft models and liveries,
                 panels, filters, routes and AIRAC cycles, day/night, API bindings
```

The root `Cargo.toml` is a workspace for both crates. Build output goes to
`./target`.

### Why a Rust port instead of PyO3

fr24 is a thin layer over HTTP: protobuf messages in length-prefixed gRPC-web
frames, plus a few JSON endpoints. That ports cleanly to `reqwest` + `prost`.
PyO3 would mean shipping a Python interpreter, httpx/curl-cffi, protobuf and
polars inside the app bundle. The port keeps the app a single native binary and
uses fr24's `.proto` files verbatim. It compiles them with `protox`, so no
`protoc` install is needed.

## Develop

Prerequisites: Rust (stable), [Bun](https://bun.sh), and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
bun install
bun tauri dev        # run the app
bun tauri build      # release bundle
cargo test -p fr24   # unit tests
cargo run -p fr24 --example smoke   # exercise every endpoint against the live API
```

Keyboard (`?` lists them in the app): `/` search, `F` filters, `C` follow
the selected aircraft, `Esc` close the open panel, `+` / `-` zoom, `T` tilt,
`N` north up, `1` / `2` dark map or satellite, `L` place names, `B` buildings,
`P` liveries, `D` day and night, `W` weather radar.

Optional: a Flightradar24 account raises the per-request flight limit and,
depending on the plan, unlocks filed flight plans, squawk, vertical speed and
airspace. Sign in from the account button in the map controls; the session is
kept in memory only and forgotten when the app quits. Alternatively set
`fr24_username` / `fr24_password` (or `fr24_subscription_key` / `fr24_token`)
in the environment, or use fr24's config file (see `crates/fr24/README.md`).

## Design

The UI follows the [impeccable](https://github.com/pbakaus/impeccable)
guidelines for an app-style ("Operate") surface: solid surfaces rather than
glass, one icon set ([Lucide](https://lucide.dev)), and an OKLCH palette whose
text pairs all pass WCAG AA (navy ink shell; white aircraft; sodium amber for
selection and actions; green only for "live"; red only for failures). Type is
Overpass, with Overpass Mono for measured values. The signature detail is the
flight panel's header, set in the column rhythm of an ATC flight-progress
strip, with altitudes given as flight levels (FL380). Tokens live in
`src/app.css`.

## Being a polite client

Flightradar24's API is unofficial and undocumented, and it rate-limits
(gRPC status 8, "too many requests"). RustRadar fetches only the slices
covering the visible area when zoomed in. It refreshes the whole world every
15 s, a region every 8 s, and backs off up to 6× when told to slow down.

## Notes

- deck.gl 9.4 workarounds in `src/lib/Globe.svelte`:
  - GlobeView's default back-face culling hides camera-facing quads (text,
    rings, paths), so those layers disable it.
  - Wheel-zooming past zoom 12 calls a GlobeViewport-only method on the Web
    Mercator viewport GlobeView switches to there. A one-line shim fixes it.
  - GlobeView's local frame for mesh offsets is rotated half a turn relative
    to the flat projection, so model yaw is offset by 180° on the globe.
  - MVTLayer decodes tiles differently per projection, so the vector map has
    one layer instance per projection. Labels are decoded to lon/lat and
    drawn by a single TextLayer.
- Map data: OpenFreeMap (© OpenMapTiles, © OpenStreetMap contributors).
  Imagery: Esri World Imagery. Weather radar: NOAA MRMS (contiguous US and southern Canada, public
  `noaa-mrms-pds` bucket, decoded from GRIB2 in `src-tauri/src/mrms.rs`), the
  WN composite of Deutscher Wetterdienst (central Europe, open data from
  opendata.dwd.de, in `src-tauri/src/dwd.rs`) and RainViewer elsewhere
  (available up to zoom 7); the plumbing shared by all three is
  `src-tauri/src/radar.rs`. Airline logos for automatic liveries: pics.avs.io.
- Aircraft shapes come from [Flightradar24's 3D models](https://github.com/Flightradar24/fr24-3d-models)
  (glTF 1.0, in turn from the FlightGear project). `bun run build:aircraft`
  downloads them, reduces each by vertex clustering to two levels of detail,
  splits the triangles into livery parts, renders a top-down silhouette, and
  writes the result to `static/aircraft/`. Rerun it to pick up new models or
  change the triangle budgets; the downloads are cached in `.cache/`.
  **Those models are GPL-2.0**, so the generated files in `static/aircraft/`
  are a derivative work and carry that licence, while the rest of this project
  is MIT. Delete that directory (the built-in shapes take over) or replace it
  with your own models if that mix does not suit you.
- AIRAC cycles are computed, not fetched: they change every 28 days, and
  cycle 2401 took effect on 2024-01-25.
