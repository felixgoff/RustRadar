# Port notes 05: pure logic modules and their tests

Scope: every TypeScript module in `src/lib/` that is logic rather than drawing (and the pure-logic halves of the drawing modules), every `*.test.ts`, and what `src-tauri/src/` already has in Rust for each. This is the checklist for the line-by-line logic port: purpose, exported API, dependencies, size, Rust overlap, difficulty, tests, target Rust module, and porting order.

Conventions used below

- Paths are relative to `src/lib/` unless they say otherwise. `file.ts:12` is a line reference. `RS:` means `src-tauri/src/`.
- Line counts are `wc -l` on branch `rust-port`. Test counts are what `bun test src` actually runs per file (394 cases in 40 files, all passing on this branch, 4.7 s); where a file builds cases in a loop the static `test(` count is lower and both are given.
- Difficulty: **S** under an hour of mechanical work, **M** half a day, **L** one to two days, **XL** several days and needs its test-suite ported first.
- "Detail elsewhere" points at docs 01-04, which already record the constants of the modules that draw. This document does not repeat them; it records what they leave out.
- Crate names (`crates/core`, `crates/backend`, `crates/scene`, `crates/app`) are a proposal (section 3); if the architecture document settles on other names, only the prefixes change.

Contents

1. Headline findings
2. Inventory (every module, one row each)
3. Target crate layout and module mapping
4. What Rust already has: overlap, module by module
5. Recommended porting order
6. Module notes: foundations (`api`, `geo`, `time`, `freshness`, `filters`, `export`, `keys`, `signin-values`, `reference`, `phone`, `weather`)
7. Module notes: sun and shadows (`daynight`, `shadow`, `building-shadow`)
8. Module notes: routes (`flightpath`, `routes`)
9. Module notes: taxiways (`taxinet`, `taxiways`)
10. Module notes: aircraft motion (`motion.ts`), in full
11. Module notes: small UI-facing logic (`adsb`, `aircraft-labels`, `aircraft-readings`, `hover-details`, `places`, `screen`, `trackpad`)
12. Module notes: ships and trains (`ships`, `ship-list`, `ship-details`, `trains`, `train-list`, `train-cars`, `train-route`, `bridges`)
13. Module notes: geometry for the scene (`roofs`, `streets-gl`, `radar`, `radar-mesh`, `lod-tileset`, `terrain-layer`, and the rest in brief)
14. JavaScript semantics that change the answer in Rust
15. Test port plan
16. Open questions and risks
Appendix A. Every test case, by file

---

## 1. Headline findings

1. **`motion.ts` and `RS:motion.rs` share a name and nothing else.** `motion.ts` (3411 lines) is the *aircraft* model: smoothing splines, delayed playback, QNH learning, flare, taxiway snapping, gear. `RS:motion.rs` (2186 lines) is the *train and ship* model: predicted and drawn trains along track, producing `frames`. No function, constant or data structure is shared. The whole of `motion.ts` has to be ported; nothing in `motion.rs` can stand in for it. The only contact point is `frames`: `motion.rs` makes them, `trains.ts` `positionAt`/`headingAt` (and `train-cars.ts`, `train-route.ts`) read them (section 4.1).
2. **The "backend is already Rust" claim holds for I/O, not for logic.** Of the 40 tested TS modules, none has its logic duplicated in Rust. The Rust side fetches, parses, caches and (for trains, radar volumes, building LOD) pre-computes; the TS side holds every formatter, filter, sort, estimator, mesher and geometry helper listed here. The modules of table 2.1 alone are about 9 300 lines of logic to port (3 400 of them `motion.ts`), the pure parts of table 2.2 several thousand more, plus 5 700 lines of tests.
3. **Three modules are the bulk of the risk**: `motion.ts` (XL, 46 tests with recorded real flights in `testdata/landings.json`), `flightpath.ts` (L, Dubins planning, 15 tests in `routes.test.ts`), `time.ts` (M in lines, L in risk: it leans on `Intl` locale data that Rust has no equivalent of out of the box).
4. **Small duplicated constants exist on both sides** and should become one definition: the NWS radar palette, `CAR_M = 26`, the road cell (`CELL_M = 2000`, radius `6 371 008.8`), Photon's languages, the ground-phase set, the weather JSON parse (section 4.3).
5. **Several behaviours depend on JavaScript semantics** that differ in Rust and are pinned by tests: `Math.round` on negative halves, `Math.sign(0)`, `toFixed` ties, `toLocaleString`, `Intl.Collator` numeric ordering, NFD folding, `Map` insertion order used as an LRU, object identity used as a map key, `Uint8ClampedArray` rounding (section 14).
6. **Tests are portable almost one to one.** They are pure (bun's `test`/`expect`, no DOM except two canvas atlases which return `null` without `document`). Only `filters.test.ts` mocks a module (`reference.svelte`), which stops being necessary once `airlineIcao` is a plain function.
7. `geo.ts` `extrapolate`, and `flightpath.ts` `dubins` and `turnThenStraight`, have no callers outside tests (grep of `src/`). They can be ported as test helpers or dropped.

---

## 2. Inventory

Class: **P** pure logic (no I/O, no GPU, no DOM); **S** state or I/O glue around pure logic; **R** rendering module with a pure part worth porting as logic (the part is named). Tests column: cases run by bun.

### 2.1 Non-rendering modules

| Module | Lines | Class | Purpose | Internal deps | Rust today | Diff. | Tests | Target |
|---|---|---|---|---|---|---|---|---|
| `api.ts` | 754 | S | DTO types and 44 `invoke` wrappers | `bridges` (type) | every DTO exists as a serde struct (6.1) | S | - | `core::model`, `backend::*` |
| `geo.ts` | 126 | P | altitude colours, elevation, view radius and area, m/px | `api` (type) | none | S | none (`cosAngle` through the list sorts) | `core::geo` |
| `time.ts` | 252 | P | airport clocks, zone abbreviations, durations, punctuality | - | none (`RS:trains.rs:214` has `days_from_civil` only) | M (risk L) | 13 | `core::time` |
| `freshness.ts` | 47 | P | feed age wording and stale threshold | - | none | S | 13 | `core::freshness` |
| `filters.ts` | 230 | P+S | aircraft filters, settings, migration, persistence | `api`, `geo`, `reference`, `ship-list`, `train-list`, `weather` | none | M | 22 | `core::filters`, `core::settings`, `app::persist` |
| `export.ts` | 219 | P+S | KML, GPX, CSV, GeoJSON text of a trail | `api` | `RS:export.rs` saves the file only | M | 7 | `core::export` |
| `keys.ts` | 80 | P+S | keys file format, load and save | `api` | `RS:keys.rs` stores `keys.json` | S | 3 | `core::keys_file` |
| `signin-values.ts` | 26 | P | which values need a sign-in, and the note | - | none | S | 7 | `core::signin` |
| `reference.svelte.ts` | 86 | S | airline/airport indexes, lazy runways, `airlineIcao` | `api` | `RS:reference.rs`, `RS:runways.rs` load the data | S | - (mocked) | `core::reference` |
| `phone.svelte.ts` | 12 | S | phone layout media query | - | none | S | - | `app::layout` |
| `weather.ts` | 103 | P+S | weather to light conditions, presets, mix | `api` | `RS:weather.rs` fetches and parses | S | `conditionsOf`, `PRESETS` in `realistic.test` | `core::weather` |
| `daynight.ts` | 82 | P | subsolar point, twilight quads | `routes` (type) | none | S | in `shadow.test` | `core::sun` |
| `shadow.ts` | 146 | P | sun direction, local sun, shadow matrix and opacity | `daynight` | none | S | 13 | `core::sun` |
| `building-shadow.ts` | 121 | P | shadow parallelograms of extruded footprints | - | none | S | 10 | `scene::building_shadow` |
| `flightpath.ts` | 948 | P | Dubins paths, runway choice, arrival/departure estimates | `api` (types) | none | L | 15 (`routes.test`) | `core::flightpath` |
| `routes.ts` | 134 | P+S | waypoint decoding, trail merge, route, AIRAC | `api`, `flightpath`, `reference` | none | S | none (`routes.test.ts` imports `flightpath` only) | `core::routes`, `core::airac` |
| `taxinet.ts` | 138 | P | taxiway segment grid index, projection | `api` (type) | none (`RS:taxiways.rs` fetches lines) | S | via `motion.test` (3) | `core::taxinet` |
| `taxiways.ts` | 66 | S | which airports' networks are loaded (LRU of 6) | `api`, `motion` | `RS:taxiways.rs` | S | - | `app::taxiways` |
| `motion.ts` | 3411 | P (stateful) | aircraft motion model | `api` (types), `taxinet` | **none** (see 1.1) | XL | 46 | `core::motion::aircraft` |
| `adsb.ts` | 52 | P | match an adsb.lol aircraft to a flight | `api` (type) | none | S | 2 | `core::adsb` |
| `aircraft-labels.ts` | 33 | P | map label text and priority | `api` (type) | none | S | 8 | `core::labels` |
| `aircraft-readings.ts` | 106 | P | transponder readings for the panel | `api` (type) | none | S | 9 | `core::readings` |
| `hover-details.ts` | 175 | P (timers) | hover-card loader: dwell, cache, cooldown; route progress | `api` (types) | none | M | 7 | `core::hover` |
| `places.ts` | 78 | P | zoom for a searched place, its detail line, language | `api` (types) | `RS:geocode.rs` has the same language list | S | 6 | `core::places` |
| `screen.ts` | 36 | P | heading to on-screen angle through a projector | - | none | S | 4 | `scene::screen` |
| `trackpad.ts` | 39 | P | wheel/trackpad classification, pinch maths | - | none | S (mostly obsolete) | 3 | `app::input` |
| `ships.ts` | 124 | P (+canvas atlas) | ship colours, labels, icon size, hull polygon | `api` (type) | none | S | 4 | `core::ships` (atlas: `scene`) |
| `ship-list.ts` | 120 | P | ship search, filter, sort, stored filters | `api`, `geo`, `ships`, `train-list` | none | M | 14 | `core::ship_list` |
| `ship-details.ts` | 43 | P | ship card wording | `freshness`, `api` | none | S | 4 | `core::ship_details` |
| `trains.ts` | 341 | P (+canvas atlas) | train colours, frames interpolation, stops, blocks | `api`, `freshness` | `RS:motion.rs` `sample` is a near relative (4.1) | M | 23 | `core::trains` (atlas: `scene`) |
| `train-list.ts` | 123 | P | train search, filter, sort, `fold`, operators | `api`, `geo`, `trains` | none | M | 20 | `core::train_list`, `core::text` |
| `train-cars.ts` | 77 | P | cars laid back along frames and tail | - | `CAR_M` duplicated at `RS:trafikverket.rs:826` | S | 6 | `core::train_cars` |
| `train-route.ts` | 138 | P | selected train's way ahead, cursor | `api` (type) | none | M | 12 | `core::train_route` |
| `bridges.ts` | 316 | P | bridge decks, raised tile lines, `levelAt`, `onStructures` | - | `RS:rail.rs:1387` makes `Structure` | M | 10 | `scene::bridges` |
| `roofs.ts` | 246 | P | pitched roof triangles | earcut | none | M | 8 | `scene::roofs` |
| `streets-gl.ts` | 222 | P (+fetch) | Streets GL tag interpretation | loaders (fetch only) | none | S | 9 | `scene::streets_gl` |
| `demo-trains.ts` | 51 | P | made-up trains for dev builds | `api` (types) | none | S | - | `app::dev` (optional) |

### 2.2 Rendering modules with a pure part

Only the pure part is in scope here; the drawing is in docs 01-04.

| Module | Lines | Pure part | Rust today | Diff. | Tests | Target |
|---|---|---|---|---|---|---|
| `radar.ts` | 284 | `reflectivity`, `echoTopKm`, `coveredBy`, `coveragesOver`, `tileBounds`, `processTile`, `processVolume` | `RS:radar.rs` contours MRMS/DWD and encodes polygons | M | - | `scene::radar` |
| `radar-mesh.ts` | 901 | all of it: palette, `parseVolumePolygons`, `buildMesh`, `mergeMeshes`, `buildWalls`, `polygonWalls`, `contourBands`, `simplifyLine`, `evenOdd`, `blur` | palette and marching-squares cousins in `RS:radar.rs` | L | 6 (`radar-arena.test`) | `scene::radar_mesh` |
| `radar-arena.ts` | 210 | `MeshArena` bookkeeping (append, collapse, repack; `GROWTH = 2`, `MIN_VERTICES = 65536`) | none | M | 6 | `scene::radar_arena` |
| `radar-pool.ts`, `radar-worker.ts` | 197, 56 | worker protocol; becomes a thread pool | none | S | - | `app::jobs` |
| `lod-tileset.ts` | 312 | `distanceTileIndices`, `keepCoarserUntilIn`, look-ahead rule | none | M | 5 | `scene::lod` |
| `terrain-layer.ts` | 460 | `decodeTerrarium`, `tileRowMetersPerPixel`, `reliefExaggeration`, `hillshade`, `hillshadeFade`, `groundHeight` | none | S | 13 | `scene::hillshade` |
| `rail-layer.ts` | 542 | `railKind`, `railColor`, `railWidth`, `railDash`, `railWidthScale`, `isStation`, `Backoff`, `speedColor`, `speedKind`, `speedLabels`, `railTilesUrl` | none | M | 11 | `scene::rail` |
| `rail-overview.ts` | 78 | `parseRailOverview` | none | S | 2 | `scene::rail_overview` |
| `road-widths.ts` | 155 | `cellOf`, `stretchAt`, `withRoadWidths` | `RS:roads.rs:217` `cell_of` is the same formula | S | 3 | `scene::road_widths` |
| `landcover.ts` | 207 | `LC`, `wooded`, `classAt`, `nmdClasses`, `worldCoverClasses` | none | S | 6 (with `canopy`) | `scene::landcover` |
| `canopy.ts` | 103 | `heightAt`, `tallestNear`, `quadkey`, `placeInFile` | none | S (GeoTIFF range reads are the hard part, not logic) | in `landcover.test` | `scene::canopy` |
| `overture.ts` | 174 | `inside`, `tallBuildings` | none | S (PMTiles reader needed) | 2 | `scene::overture` |
| `street-trees.ts` | 93 | `parseTown`, `treesInTile` | none | S | via `tree-layer.test` | `scene::street_trees` |
| `tree-layer.ts` | 762 | `coverOf`, `blockOf`, `hash2`, `leafPhase`, `plantTile`, `tilesAround`, `treeShadows` | none | L | 10 | `scene::trees` |
| `tree-models.ts` | 52 | `cardArrays` | none | S | in `tree-layer.test` | `scene::trees` |
| `landcover-ground.ts` | 122 | `coverOutlines` (d3-contour) | none | M | - | `scene::landcover_ground` |
| `realistic.ts` | 1169 | `transmittance`, `lookFor`, `displayRgb`, `skyCss`, `sceneSun`, `fromLocal`, `roofColour`, `buildingStyle` | none | M | 15 (with `realistic-ground`) | `scene::look` |
| `realistic-ground.ts` | 945 | `areaPaint`, `lineOf`, paint tables | none | M | in `realistic.test` | `scene::ground` |
| `building-layer.ts` | 461 | `buildingHeight` | `RS:building_lod.rs` (far LOD tiles) | S | - | `scene::buildings` |
| `ship-models.ts` | 649 | `shipModelArrays`, `usableDims`, `shipClassOf`, `shipPlacement`, `offsetEastNorth`, `TYPICAL_LENGTH` | none | M | 16 (7 static) | `scene::ship_models` |
| `train-models.ts` | 204 | `carArrays`, `carPlaces`, `modelCar` | none | S | 3 | `scene::train_models` |
| `models.ts` | 512 | procedural aircraft meshes, `modelFor`, `sizeOf`, `MODEL_SIZE` | none | M | - | `scene::aircraft_models` |
| `aircraft.ts` | 189 | index and mesh-file parsing | none | S | - | `scene::aircraft_assets` |
| `liveries.ts` | 222 | curated liveries, aliases, colours from logo | `RS:liveries.rs` extracts logo colours | S | - | `core::liveries` |
| `clouds.ts` | 74 | `cloudStrips` | none | S | - | `scene::clouds` |
| `ktx2*.ts` | 53+20+94 | `isKtx2`, target choice | none | S (use a Rust basis/ktx2 crate) | 4 | `scene::ktx2` |
| `controller.ts` | 88 | north-up pan maths | none | M | - | `app::camera` (doc 01) |

Totals: 68 non-test `.ts` files, 18 899 lines (table 2.1: about 9 300); 40 test files, 5 725 lines, 394 cases.

---

## 3. Target crate layout and module mapping

Proposed workspace (the existing `crates/fr24` stays as it is):

```
crates/
  fr24/        existing: Flightradar24 client
  core/        package rustradar-core: pure logic, no I/O, no async, no GPU, no UI
  backend/     package rustradar-backend: today's src-tauri/src minus Tauri (HTTP, caches, feeds)
  scene/       package rustradar-scene: CPU-side geometry and tile logic the renderer consumes
  app/         package rustradar: winit + wgpu + egui, wiring, persistence, input
```

Rules that keep the port honest:

- `core` depends on nothing in the workspace. It owns the DTOs (`core::model`), so `backend` and `scene` both depend on it and the serde structs stop living next to the fetch code.
- `core` and `scene` take time as an argument (`now_ms: i64`); nothing in them reads a clock. This is what lets the tests port unchanged (the TS already does it almost everywhere: `Motion.pose(f, nowMs)`, `HoverLoader` `now`, `Backoff(now)`, `dayAt(..., now)`, `airacCycle(now)`).
- `RS:motion.rs` moves to `backend::rail_motion` (or `core::motion::rail`, it is pure) so that `core::motion::aircraft` can take the `motion.ts` port without a name clash.

| TS module | Rust module path |
|---|---|
| `api.ts` types | `crates/core/src/model/{flight,airport,ship,train,radar,place,adsb,weather,keys}.rs` |
| `api.ts` calls | plain `async fn`s on `crates/backend/src/*` (no command layer) |
| `geo.ts` | `crates/core/src/geo.rs` |
| `time.ts` | `crates/core/src/time.rs` (+ `time/zone_names.rs` for the tables) |
| `freshness.ts` | `crates/core/src/freshness.rs` |
| `filters.ts` | `crates/core/src/filters.rs` (`Filters`, `matcher`), `crates/core/src/settings.rs` (`Settings`, `merge_settings`, `shown_kinds`), `crates/app/src/persist.rs` |
| `export.ts` | `crates/core/src/export.rs`; saving stays in `crates/backend/src/export.rs` |
| `keys.ts` | `crates/core/src/keys_file.rs`; storage stays in `crates/backend/src/keys.rs` |
| `signin-values.ts` | `crates/core/src/signin.rs` |
| `reference.svelte.ts` | `crates/core/src/reference.rs` (indexes, `airline_icao`); loading in `backend::reference`, `backend::runways` |
| `phone.svelte.ts` | `crates/app/src/layout.rs` |
| `weather.ts` | `crates/core/src/weather.rs` |
| `daynight.ts`, `shadow.ts` | `crates/core/src/sun.rs` |
| `building-shadow.ts` | `crates/scene/src/building_shadow.rs` |
| `flightpath.ts` | `crates/core/src/flightpath/{sphere,dubins,runways,estimate}.rs` |
| `routes.ts` | `crates/core/src/routes.rs`, `crates/core/src/airac.rs` |
| `taxinet.ts` | `crates/core/src/taxinet.rs` |
| `taxiways.ts` | `crates/app/src/taxiways.rs` |
| `motion.ts` | `crates/core/src/motion/aircraft/{mod,consts,track,samples,spline,fit,altitude,predict,evaluate,blend,delay,phase,qnh,measured,taxi_snap,gear}.rs` |
| `adsb.ts` | `crates/core/src/adsb.rs` |
| `aircraft-labels.ts` | `crates/core/src/labels.rs` |
| `aircraft-readings.ts` | `crates/core/src/readings.rs` |
| `hover-details.ts` | `crates/core/src/hover.rs` |
| `places.ts` | `crates/core/src/places.rs` |
| `screen.ts` | `crates/scene/src/screen.rs` |
| `trackpad.ts` | `crates/app/src/input.rs` |
| `ships.ts`, `ship-list.ts`, `ship-details.ts` | `crates/core/src/{ships,ship_list,ship_details}.rs` |
| `trains.ts`, `train-list.ts`, `train-cars.ts`, `train-route.ts` | `crates/core/src/{trains,train_list,train_cars,train_route}.rs`; `fold` in `crates/core/src/text.rs` |
| `bridges.ts` | `crates/scene/src/bridges.rs` |
| `roofs.ts`, `streets-gl.ts` | `crates/scene/src/{roofs,streets_gl}.rs` |
| `radar.ts`, `radar-mesh.ts`, `radar-arena.ts` | `crates/scene/src/radar/{mod,mesh,arena}.rs` |
| `lod-tileset.ts` | `crates/scene/src/lod.rs` |
| `terrain-layer.ts` (pure part) | `crates/scene/src/hillshade.rs` |
| the rest of 2.2 | `crates/scene/src/<same name, snake case>.rs` |
| `*.test.ts` | `#[cfg(test)] mod tests` beside each module; `motion` and `flightpath` as `crates/core/tests/*.rs` with `crates/core/tests/data/landings.json` |

---

## 4. What Rust already has

### 4.1 `motion.rs` against `motion.ts`, precisely

| | `RS:motion.rs` (2186 lines, 33 tests) | `motion.ts` (3411 lines, 46 tests) |
|---|---|---|
| Subject | trains (and ships) | aircraft |
| Input | `Fix` (lat, lon, t, speed, heading), `Stop` list, a `Track` trait giving routed track with speed limits | `LiveFlight` reports with Flightradar24's position buffer, airports, optional `Measured` (adsb.lol), taxiway lines |
| Output | `Frame = (lat, lon, unix_ms)` lists: `Sim { frames, predicted }` | `Pose { lon, lat, altFt, heading, track, pitch, bank, gear, phase }` per frame |
| Model | predicted train on rails braking into stations; drawn train following it with bounded acceleration; 1 s steps | cubic smoothing spline through samples, 2-12 s delayed playback, arc extrapolation, critically damped blending, QNH, flare, phases |
| Runs | on the backend, once per report | on the render thread, per aircraft per frame |
| Shared code | none | none |

Closest relatives, which are still not interchangeable:

- `motion.rs:436` `sample(frames, t)` and `:489` `drawn_at` against `trains.ts:98` `positionAt` and `:117` `headingAt`. Both interpolate `frames`. Differences that tests pin (`trains.test.ts`): the TS holds at the first frame before it starts (`sample` returns `None`), holds at the last after it ends, takes the short way across the antimeridian, and computes the heading as a great-circle initial bearing of the current leg falling back to the *reported* heading. Port the TS functions as they are; do not substitute `sample`.
- `motion.rs:153-171` `distance`/`course`/`ahead` against `flightpath.ts:76-100` `distanceM`/`bearing`/`destination`. Same purpose, both on a 6 371 000 m sphere. After the port, keep one set in `core::geo::sphere` and have `rail_motion` use it (check the argument order: `motion.rs` is `(lat, lon)`, `flightpath.ts` is `[lon, lat]`).
- `motion.rs:519` `frames()` (ships dead-reckoned straight on) has no TS twin; `geo.ts:67` `extrapolate` is a different, unused dead-reckoner.

### 4.2 Module by module

| TS module | Rust module | What Rust does | What only TS does (must be ported) |
|---|---|---|---|
| `export.ts` | `RS:export.rs` (96 lines, 2 tests) | `save_export`: sanitises the file name (`safe_file_name`: `/ \ : * ? " < > |` and control characters to `-`, no leading or trailing dots, stem at most 120 characters, empty becomes `flight-path`), writes to Downloads (home if none) as `name`, `name (2)`, ... | all four file formats, the title, the file name |
| `keys.ts` | `RS:keys.rs` (133 lines, 2 tests) | `Keys` struct with the same six camelCase fields, trimmed on save, `keys.json` in the config folder, `get_keys`, `save_keys` | the shareable file (`keysFile`, `readKeysFile`), and its error messages |
| `weather.ts` | `RS:weather.rs` (144 lines, 3 tests) | `weather_at`: MET Norway fetch, 10 min cache per 0.1 degree cell (256 cells), `parse` | `conditionsOf`, `PRESETS`, `mixConditions`, `SKIES`. The TS browser fallback `fetchWeather` duplicates `parse` and is dropped (one difference: Rust `parse` returns `None` without `cloud_area_fraction`, the TS reads it as 0) |
| `reference.svelte.ts` | `RS:reference.rs`, `RS:runways.rs` | weekly cached airlines and airports; OurAirports runways CSV | the three lookup maps, `runways()` lazy ask with 60 s retry, `airlineIcao` |
| `taxinet.ts`, `taxiways.ts` | `RS:taxiways.rs` (368 lines, 5 tests) | Overpass query within 4 km, parse, month-long cache, 10 min retry | the grid index, the LRU of 6 airports, the 5 min client-side retry |
| `radar.ts`, `radar-mesh.ts` | `RS:radar.rs` (735 lines, 7 tests), `RS:mrms.rs`, `RS:dwd.rs` | decodes MRMS and DWD volumes, isobands with the `contour` crate, simplifies, encodes band polygons to bytes | RainViewer tile pixels to dBZ, its own marching squares (`contourBands`), triangulation (earcut on a 4 degree grid), walls, the arena. `parseVolumePolygons` exists only to decode what `encode_shapes` wrote and disappears when both are in one process |
| `ships.ts`, `ship-list.ts`, `ship-details.ts` | `RS:ships.rs` (993 lines, 11 tests) | VesselFinder binary feed, zoom mapping, ship tracks and `frames`, details, nav status wording | colours, labels, icon sizes, hull polygon, search, filters, sort, all card wording |
| `trains.ts`, `train-list.ts` | `RS:trains.rs`, `RS:trafikverket.rs`, `RS:train_class.rs` | everything upstream of the `Train` DTO, including `kind`, `shape`, `model`, `cars`, `tail`, `structures`, `frames` | colours per kind and operator, `positionAt`/`headingAt`, delay wording and tone, stop views, `trainProduct`, `blockAt`, search, filter, sort, operators |
| `train-cars.ts` | `RS:trafikverket.rs:826` | `CAR_M = 26.0`, `DEFAULT_CARS`, makes `tail` long enough for the cars | laying cars along frames and tail |
| `train-route.ts` | `RS:trafikverket.rs:1876-1889` | builds `TrainRoute` | trimming it to the drawn train, the cursor, `wayToStop` |
| `bridges.ts` | `RS:rail.rs:1387` | builds `Structure { tunnel, line, joined }` | deck heights, raising tile lines, `levelAt`, `onStructures` |
| `road-widths.ts` | `RS:roads.rs` (340 lines, 5 tests) | NVDB stretches per 2 km cell | matching tile roads to stretches (`MATCH_M = 10`, `MATCH_DEG = 30`, index cells `0.002` degrees) |
| `building-layer.ts` | `RS:building_lod.rs` (728 lines, 7 tests) | far-LOD building tiles (z9-12) built from z13 | near buildings, roofs, heights |
| `liveries.ts` | `RS:liveries.rs` (153 lines, 2 tests) | dominant colours of an airline logo | curated table, aliases, military set, livery from two colours |
| `adsb.ts` | `RS:adsblol.rs` (518 lines, 5 tests) | fetch, parse to `Aircraft` | matching to the selected flight |
| `places.ts` | `RS:geocode.rs` (749 lines, 10 tests) | Photon query, labels, kinds, bbox | zoom fitting, detail line |
| `routes.ts` `mergeTrail` | `RS:opensky.rs` | fetch and parse a track | the merge |
| `hover-details.ts`, board | `RS:board.rs` | parse board JSON | hover loader |
| `trains.ts` `blockAt` | `RS:blocks.rs` (432 lines, 4 tests) | builds blocks from signals | which block a drawn train is in |
| `time.ts` | - | nothing | all of it |
| `filters.ts` | - | nothing (categories are passed to `live_flights` as `services`) | all of it |
| `flightpath.ts` | - | nothing | all of it |
| `motion.ts` | - | nothing | all of it |

### 4.3 Constants defined on both sides (make them one)

| Constant | TS | Rust |
|---|---|---|
| NWS reflectivity palette, 14 bands, first band 5 dBZ, 5 dBZ each | `radar-mesh.ts:10-33` (`PALETTE`, `bandDbz`, `bandColor`) | `RS:radar.rs:128-160` (`PALETTE`, `color`, `band_of`) |
| Car length 26 m | `train-cars.ts:4` | `RS:trafikverket.rs:826` |
| Road cell 2000 m on radius 6 371 008.8 m | `road-widths.ts:12-13`, `cellOf` `:21` | `RS:roads.rs:23,28`, `cell_of` `:217` |
| Photon languages `en de fr it` | `places.ts:77` | `RS:geocode.rs:43` |
| Position buffer scale 1e-5 degrees, ms offsets | `motion.ts:167` (`BUFFER_SCALE`), `SAMPLE_WINDOW_MS = 30 000` | `RS:lib.rs:135-139` (`MAX_BUFFER_MS = 60 000`, `buffered_positions`) |
| Ground phases | `motion.ts:538`, `taxiways.ts:15` | - |
| MET Norway JSON reading | `weather.ts:87-103` | `RS:weather.rs:42` |
| Train kinds and shapes | `api.ts:603-605` | `RS:trains.rs:36`, `RS:train_class.rs:39` |

---

## 5. Recommended porting order

Dependency-sorted. Each tier only needs the tiers above it. Port a module's tests in the same change as the module.

**Tier 0: no dependencies**

1. `core::model` (the DTOs out of `RS:` and `api.ts`; keep `serde` derives, they are needed for caches and the keys/settings files)
2. `core::num` helpers from section 14 (`js_round`, `wrap180`, `wrap360`, `wrap_lon`, `clamp`, `smoothstep`, `sign0`, `to_fixed`)
3. `core::text` (`fold`, natural collation)
4. `core::geo` (`geo.ts`) and `core::geo::sphere` (`distanceM`, `bearing`, `destination`, `intermediate`, `greatCircle`, `unwrap` from `flightpath.ts:76-133`)
5. `core::freshness`, `core::signin`, `core::labels`, `core::readings`, `core::adsb`, `core::places`, `core::weather`, `core::keys_file`
6. `core::sun` (`daynight.ts`, then `shadow.ts`)

**Tier 1: need tier 0**

7. `core::time` (start early, it has the most unknowns; section 6.3)
8. `core::reference` (indexes and `airline_icao`)
9. `core::ships`, `core::ship_details`, `core::trains`
10. `core::train_list` (needs `trains`, `geo`, `text`), then `core::ship_list` (needs `ships`, `train_list::fold`)
11. `core::train_cars`, `core::train_route`
12. `core::export`
13. `core::filters`, `core::settings` (need `reference`, `ship_list`, `train_list`, `weather`)
14. `core::hover`
15. `core::taxinet`

**Tier 2: the big ones**

16. `core::flightpath` (needs `geo::sphere`), then `core::routes` and `core::airac` (need `flightpath`, `reference`)
17. `core::motion::aircraft` (needs `model`, `taxinet`, `num`). Order inside it: constants and helpers; `Blend`; samples (`reportPoints`, `mergeReport`, `insertSample`, `finishSamples`); `smoothSpline`; `fitTrack`/`fitAround`/`localRates`; `hermite`; altitude (`addAltitude`, `fitAltitude`, `airAltitude`); delay (`noteArrival`, `retarget`, `delayAt`); `decidePhase`; QNH; `predict`; `evaluate`; `drawn`/`setOffsets`/`blendAround`; measured; `taxiSnap`; `gear`. The tests in appendix A are listed in an order that follows this build-up.

**Tier 3: scene geometry (can run in parallel with tier 2, different crate)**

18. `scene::screen`, `scene::building_shadow`, `scene::hillshade`, `scene::rail_overview`, `scene::landcover`, `scene::canopy` (pure parts), `scene::streets_gl`
19. `scene::bridges`, `scene::roofs`, `scene::road_widths`, `scene::train_models`, `scene::ship_models`, `scene::rail`
20. `scene::lod`, `scene::radar::{mod,mesh,arena}`, `scene::look`, `scene::ground`, `scene::trees`

**Tier 4: glue in `app`**

21. `app::persist` (settings file), `app::taxiways`, `app::layout`, `app::input`, `app::jobs`

---

## 6. Module notes: foundations

### 6.1 `api.ts` (754 lines)

Every interface has a serde struct already; in the native app the `invoke` layer and the camelCase/snake_case JSON hop disappear and callers use the structs directly. Field names in the UI docs follow the TS names.

| TS type (`api.ts` line) | Rust struct |
|---|---|
| `LiveFlight` (5) | `RS:lib.rs:101` (private; make it `core::model::LiveFlight`). `positions: Vec<[i32; 3]>` = `[dLat, dLon (1e-5 deg), dMs]`, newest first; `vspeed: Option<i32>` only when signed in |
| `LiveSnapshot` (36) | `RS:lib.rs:182` (`flights, serverTimeMs, requests, errors, elapsedMs, authenticated`) |
| `TrailPoint` (45) | `crates/fr24/src/grpc.rs:628` `TrailPointRecord` (snake_case fields: `timestamp` s, `latitude`, `longitude`, `altitude` ft, `ground_speed` kt, `track`, `vertical_speed` fpm, `source`) |
| `AircraftImage` (56), `FlightPlan` (65), `FlightDetails` (76) | `grpc.rs:698`, `:876`, `:925` (with `:707` aircraft, `:753` schedule, `:791` progress, `:829` flight) |
| `FollowEvent` (134) | `RS:lib.rs:261` enum `details | error | ended`; the Tauri `Channel` becomes an mpsc channel |
| `TopFlight` (139) | `grpc.rs:592` |
| `FindEntry` (160) | `crates/fr24/src/json.rs:182`, details `:195-294` |
| `Airline` (167), `Airport` (173), `ReferenceData` (190) | `RS:reference.rs:15`, `:23`, `:44`. `Airport.alt` is optional feet; `timezone` an IANA name |
| `BoardRow` (199), `Board` (221), `BoardMode` (196) | `RS:board.rs:34`, `:59`, `:8` |
| `HistoryRow` (229) | built in `RS:lib.rs:439` `aircraft_history` from `json.rs:313` `FlightListRecord` (verify the mapping when moving it) |
| `LogoColors` (250) | `RS:liveries.rs:11` |
| `SessionInfo` (255) | `RS:session.rs:15` |
| `BoundingBox` (262) | `grpc.rs:175` (`f32`; west > east wraps the antimeridian) |
| `Place` (300), `PlaceKind` (287) | `RS:geocode.rs:67`, `:48` |
| `RunwayEnd` (324), `Runway` (335) | `RS:runways.rs:17`, `:33` |
| `TaxiLine` (347) | `RS:taxiways.rs:52` (`kind`: taxiway, taxilane, runway; `ref`; `points [lon, lat]`) |
| `RadarVolume` (373) | `RS:radar.rs:61` `VolumeInfo` |
| `Ship` (400), `NavStatus` (437), `ShipDetails` (446) | `RS:ships.rs:57`, `:528`, `:558` |
| `OpenSkyPoint` (493) | `RS:opensky.rs:20` |
| `AdsbAircraft` (512), `AdsbSnapshot` (566) | `RS:adsblol.rs:41`, `:135`. `Measured` (`motion.ts:61`) is a subset of it |
| `Train` (618), `Sighting` (609), `TrainKind`, `TrainShape` | `RS:trains.rs:46`, `:101`, `:36`, `RS:train_class.rs:39` |
| `StopTime` (669), `TrainStop` (676) | `RS:trafikverket.rs:984`, `:992` |
| `TrainPhoto`, `TrainMedia` (691-702) | `RS:train_media.rs:28`, `:37` |
| `TrainEvent` (708) | `RS:trafikverket.rs:1620` |
| `RouteStop`, `TrainRoute` (720-729) | `RS:trafikverket.rs:1876`, `:1889` |
| `TrainBlock` (732) | `RS:blocks.rs:36`; the `train_blocks` answer is `RS:trains.rs:537` |
| `Structure` (`bridges.ts:220`) | `RS:rail.rs:1387` |
| `RoadPiece` (737) | `RS:roads.rs:32` |
| `Weather` (`weather.ts:8`) | `RS:weather.rs:26` |
| `Keys` (`keys.ts:8`) | `RS:keys.rs:16` |

Non-type exports to carry over: `isRateLimited(message) = /status 8|too many requests/i` (`:277`; better as a typed error variant from `fr24`, keeping the string test as a fallback), `shipPhotoUrl(photoId, thumbnail)` = `https://static.vesselfinder.net/ship-photo/{urlencoded id}/{0 thumbnail | 1 full}` (`:486`), `shipPageUrl(mmsi)` = `https://www.vesselfinder.com/vessels/details/{mmsi}` (`:490`), `airlineLogoUrl(id)` = `https://cdn.flightradar24.com/assets/airlines/logotypes/{id}.png` (`:597`), `ADSB_LOL_URL = "https://adsb.lol"` (`:578`). `isTauri()` goes away with every branch that tests it.

The 44 commands (`RS:lib.rs:719-770`) and their TS wrappers: `live_flights, flight_details, follow_flight, unfollow_flight, top_flights, search, reference_data, airport_runways, airport_board, aircraft_history, airline_colors, radar_frame, radar_image, opensky_track, adsb_hex, adsb_callsign, geocode, session_info, sign_in, sign_out, get_keys, save_keys, airport_taxiways, save_export, open_devtools, pending_update, restart_to_update, ships_in_view, ship_details, trains_in_view, train_stops, train_route, watch_trains, train_blocks, weather_at, hide_train_operators, dev_log, dev_log_clear, product_codes, trafikverket_key_status, road_widths, follow_train, unfollow_train, train_media`.

### 6.2 `geo.ts` (126 lines)

```
type RGB = [u8; 3];  enum Basemap { Dark, Satellite, Realistic }
AIRCRAFT_COLOR = [255, 255, 255]      SELECTED_COLOR = [55, 142, 240]
ALTITUDE_STOPS (ft -> RGB): 0 [255,98,0]; 4000 [255,196,0]; 8000 [160,230,40]; 15000 [40,220,130];
                            25000 [0,200,230]; 35000 [70,125,255]; 45000 [205,85,255]
altitudeColor(altFt) -> RGB            linear between stops, each channel Math.round; clamped at both ends
elevation(altFt, exaggeration) = max(0, altFt) * 0.3048 * exaggeration                       (metres)
effectiveExaggeration(ex, zoom) = 1 + (ex - 1) * clamp01((11 - zoom) / 5)
metersPerPixel(zoom, lat) = 40_075_016.686 * cos(lat) / (512 * 2^zoom)
extrapolate(f, nowMs) -> [lon, lat]    unused outside tests: no move if onGround or speed < 40 kt; dt clamped 0..90 s
cosAngle(lon1, lat1, lon2, lat2) = sin a sin b + cos a cos b cos(dLon)
visibleRadius(zoom, pitch, w, h) = min(100, 1.3 * (hypot(w, h) / 2) * (360 / (512 * 2^zoom)) * (1 + 4 sin pitch))   (degrees)
viewArea(view, w, h) -> Option<{south, north, west, east}>
    r = visibleRadius; r >= 50 -> None (fetch the world)
    south = max(-90, lat - r); north = min(90, lat + r)
    dLon = r / max(0.05, cos(max(|south|, |north|))); dLon >= 180 -> west -180, east 180
    else west/east = wrap(lon -/+ dLon), wrap(x) = ((x + 540) % 360) - 180
unwrapLongitude(from, to)              to +/- 360 until within 180 of from
```

Note `SELECTED_COLOR`'s comment says amber but the value is the selection blue; the value is what the app shows.

### 6.3 `time.ts` (252 lines, 13 tests): the locale-data problem

Exports:

```
clockAt(unix_s | null, timeZone?) -> "13:25" | "—"                 24 h, Latin digits, zero padded
clockWithZone(unix_s | null, timeZone?) -> "13:25 BST" | "—"
minutesOfDay(unix_s, timeZone?) -> 0..1439
zoneName(unix_s, timeZone?) -> "BST" | "CEST" | "EDT" | "JST" | "KST" | "UTC+3" | "UTC+5:45" | "UTC−4" | "UTC"
dayAt(unix_s, timeZone?, now_ms) -> "Today" | "Yesterday" | "Tomorrow" | "Mon 6 Oct"
duration(seconds) -> "5 h 10 min" | "45 min"       h = floor(s / 3600); m = round((s % 3600) / 60), two digits after hours
delayMinutes(scheduled, actual) -> Option<i64>     round((actual - scheduled) / 60); None if either is 0 or missing
punctuality(minutes) -> Option<{label, tone}>      >= 45: "{duration} late" bad; >= 15: "{n} min late" warn;
                                                   <= -5: "{n} min early" good; else "On time" good
toneOf(color) -> Option<Tone>                      green good; yellow, orange warn; red bad
type Tone = good | warn | bad
```

Behaviour to reproduce:

- A falsy `unix` (0, null, undefined) gives the em dash `—`. A missing or unknown zone name falls back to the viewer's own zone (`formatter`, `:7-21`; `known`, `:153`).
- `utcLabel(minutes east)`: `UTC` at 0; otherwise `UTC` + (`−` U+2212 for negative, `+`) + whole hours + `:mm` only when minutes are non-zero (`:73-78`).
- `zoneName` (`:186-199`), cached by `zone|offset`:
  1. `own` = the given zone if known, else the viewer's zone. `zone` = `SAME_CLOCK[own]` or `own`.
  2. `abbreviation`: ask each of `NAMING_LOCALES` (`en-US, en-GB, en-CA, en-AU, en-NZ, en-IE, en-IN, en-ZA, en-SG, en-HK, ja-JP, pt-BR, es-AR`, in that order) for the short zone name; accept the first that matches `^[A-Z]{2,6}$`, is not `GMT` (remembered as a last resort), and is not an `OWNED` name belonging to another zone.
  3. else `COMMON_NAMES[zone][offset]`, else `utcLabel(offset)`.
- Tables (`:97-142`), to copy verbatim:
  - `SAME_CLOCK`: Europe/Jersey, Europe/Guernsey, Europe/Isle_of_Man, Europe/Belfast -> Europe/London; Asia/Saigon -> Asia/Ho_Chi_Minh; Asia/Rangoon -> Asia/Yangon; Asia/Katmandu -> Asia/Kathmandu; Asia/Istanbul -> Europe/Istanbul.
  - `OWNED`: `BST` -> Europe/London.
  - `COMMON_NAMES` (zone: offset minutes -> name): Asia/Seoul 540 KST; Europe/Moscow 180 MSK; Europe/Istanbul 180 TRT; Europe/Samara 240 SAMT; Asia/Manila 480 PHT; Asia/Bangkok 420 ICT; Asia/Ho_Chi_Minh 420 ICT; Asia/Jakarta 420 WIB; Asia/Karachi 300 PKT; Asia/Tashkent 300 UZT; Asia/Kathmandu 345 NPT; Asia/Yangon 390 MMT; Asia/Tehran 210 IRST; Asia/Baku 240 AZT; Asia/Tbilisi 240 GET; America/Bogota -300 COT; America/Lima -300 PET; America/Guayaquil -300 ECT; America/Caracas -240 VET; America/La_Paz -240 BOT; America/Santiago -240 CLT, -180 CLST; America/Montevideo -180 UYT; America/Asuncion -180 PYT; Pacific/Fiji 720 FJT.
- `dayAt` compares the calendar date of `unix` in the zone with that of `now`, `now - 1 day`, `now + 1 day`; otherwise weekday short, day, month short in the viewer's locale (`Mon 6 Oct` in English).

Port plan. Offsets and calendar fields are straightforward with `chrono-tz` or `jiff` (bundle the tz database: Android and iOS do not expose one portably). The abbreviation step is the problem: it is defined by CLDR's *per-locale* metazone short names as shipped in Chromium, not by the tz database. The tz database's own abbreviations give `BST`, `CEST`, `EDT`, `JST` correctly but also give `CST` for China, `IST` for India, Israel and Ireland, `+03` style for many zones, and `KST`/`MSK` directly. Recommended: implement `abbreviation` as a generated static table `(zone, is_dst) -> name` produced once by running the existing TypeScript in Chromium over every IANA zone for a winter and a summer instant, check the table in, and fall through to `COMMON_NAMES` and `utcLabel` exactly as now. The 13 tests in `time.test.ts` then port as they are. This is the one place where "port the logic" needs a data-generation step; see open question 3.

### 6.4 `freshness.ts` (47 lines, 13 tests)

```
STALE_MIN_MS = 30_000
staleAfter(intervalMs) = max(30_000, 2 * intervalMs)
ageLabel(seconds): s = max(0, floor(seconds)); < 60 -> "{s} s"; m = floor(s / 60) < 60 -> "{m} min";
                   else h = floor(m / 60): "{h} h {mm} min" (two digits) or "{h} h" when minutes are 0
feedFreshness(ageMs, intervalMs) -> { stale, label }: ageMs > staleAfter -> "Data {age} old" stale; else "Updated {age} ago"
struct FeedClock { at, interval }
afterRound(shown, round { arrived, at, interval }) -> arrived ? { at, interval } : shown
```

### 6.5 `filters.ts` (230 lines, 22 tests)

- `CATEGORIES` (id, label), ids 0-11: Passenger, Cargo, Military and government, Business jets, General aviation, Helicopters, Lighter than air, Gliders, Drones, Ground vehicles, Other, Uncategorised.
- `MAX_ALTITUDE = 50_000` (the slider's top means "and above").
- `Filters { categories: Vec<u8>, altitude: (i32, i32), airborne, on_ground, airlines: Vec<String> (ICAO), types: Vec<String> (prefixes), airports: Vec<String> (IATA), text }`. Default: all categories, `(0, 50_000)`, both true, empty lists, empty text.
- `serverCategories(f)`: `None` when all 12 or none are selected, else the list.
- `activeCount(f)`: number of true among: categories not all; altitude narrowed; airborne or onGround off; airlines; types; airports; trimmed text non-empty.
- `matcher(f)` (`:79-99`): no categories selected rejects everything. Otherwise per flight: ground/air switches; on the ground a minimum altitude above 0 rejects, in the air `alt < low` or (bounded and `alt > high`) rejects; airlines by `airlineIcao(callsign)`; types by `typecode.starts_with`; airports by origin or destination; text upper-cased, substring of callsign, flight or reg upper-cased.
- `Mode = aircraft | trains | ships`; `RailShows = speeds | blocks`.
- `Settings` and defaults (`:138-155`): `mode aircraft, alsoShow [], basemap dark, labels true, labelSize 1, buildings true, aircraftLabels false, daylight true, weather false, adsb true, railMap false, railShows speeds, sky live, tiltShift false, terrain false, exaggeration 10`.
- `mergeSettings(saved)` (`:168-192`), each rule has a test: copy only known keys whose JSON type equals the default's; `alsoShow` is the known modes present in the saved array, in `MODES` order, else empty; unknown `sky` or `railShows` fall back; legacy `trainBlocks: true` without `railShows` turns on `railMap` with `railShows = blocks`; a missing or unknown `mode` is derived from the legacy switches `aircraft`, `ships`, `trains` (missing counts as on): aircraft unless switched off, then ships, then trains.
- `shownKinds(settings)`: a kind is on if it is the mode or in `alsoShow`.
- Persistence: one JSON object `{ settings, filters, trainFilters, shipFilters }` under localStorage key `rustradar.v1`. `filters` are spread over the defaults with no validation; train and ship filters go through `mergeTrainFilters` / `mergeShipFilters` (12.2, 12.4). In Rust: `serde_json::Value` in, typed structs out, written to `<config dir>/settings.json`; keep the same JSON shape so a settings file can be carried over (open question 2).

### 6.6 `export.ts` (219 lines, 7 tests)

- `ExportFormat = kml | gpx | csv | geojson`; `EXPORT_FORMATS` labels and hints (`:15-20`): KML "Google Earth, with a 3D track that plays back in time"; GPX "Mapping and GPS apps"; CSV "Spreadsheets: time, position, altitude, speed, track"; GeoJSON "GIS tools and web maps".
- `ExportMeta { callsign, flightNumber, registration, aircraftType, origin, destination }`, empty strings left out.
- `exportPoints(trail)`: keep `timestamp > 0` with finite latitude and longitude; stable sort by timestamp.
- Helpers: `iso(unix)` = `YYYY-MM-DDTHH:MM:SSZ` (no milliseconds); `metres(ft)` = `round(max(0, ft) * 0.3048 * 10) / 10`; `coord(x)` = six decimals then shortest form (`Number(x.toFixed(6))`, so `4.5` not `4.500000`); `xml` escapes `& < > " '`.
- `exportTitle(meta)`: id = callsign or flight number or registration or `Flight`; ` (flightNumber)` when different from the id; ` ORIGIN–DESTINATION` with an en dash, `?` for a missing end, only when either is known.
- `exportFileName(meta, points, format)`: `{id or "flight"}{ ORIGIN-DEST when both}{ YYYY-MM-DD of the first point}.{format}`.
- Description: `Aircraft {type}`, `registration {reg}`, `{n} positions, {iso first} to {iso last}`, then the fixed sentence `Altitude is pressure altitude as reported. Exported from RustRadar; data from Flightradar24.`, joined with `. `.
- KML (`:76-105`): `gx:Track` with `extrude 1`, `altitudeMode absolute`, all `<when>` then all `<gx:coord>lon lat metres</gx:coord>`; style `track`: line `ff34a6f0` width 3, poly `4034a6f0`, icon scale 0.8 `http://maps.google.com/mapfiles/kml/shapes/airports.png`. Copy the template byte for byte, indentation included: the tests compare substrings.
- GPX 1.1, `creator="RustRadar"`, metadata name, desc and time of the first point, one `trkpt` per position with `ele` (metres) and `time`.
- CSV header `time_utc,unix_time,latitude,longitude,altitude_ft,ground_speed_kt,track_deg,vertical_speed_fpm,callsign`; integers by `Math.round`; callsign quoted only if it contains `"`, `,` or a newline; `\n` line ends and a final newline.
- GeoJSON: one `Feature` `LineString` `[lon, lat, metres]`; properties `name, callsign, flightNumber, registration, aircraftType, origin, destination` (null when empty), `altitude: "pressure altitude in metres, as reported"`, `times` (ISO), `groundSpeedKt`, `source: "Flightradar24, via RustRadar"`; `JSON.stringify(…, null, 2)` plus a newline. With serde_json: keep the property order (struct field order or `preserve_order`), and write whole numbers without `.0` (section 14).
- `saveExport`: in the app, `backend::export::save` then reveal in the file manager; the browser download branch is dropped. MIME types if a share sheet needs them on phones: `application/vnd.google-earth.kml+xml`, `application/gpx+xml`, `text/csv`, `application/geo+json`.

### 6.7 `keys.ts` (80 lines, 3 tests)

- `Keys { fr24Email, fr24Password, trafiklabGtfs, trafiklabGtfsStatic, trafiklabOxyfi, trafikverket }`, all strings, empty by default.
- `keysFile(keys, withLogin)`: `{ "format": "rustradar-keys", "version": 1, "keys": { non-empty fields, in field order } }`, two-space pretty print plus a newline; without `withLogin` the two `fr24*` fields are left out.
- `readKeysFile(text, current)`: invalid JSON, wrong `format`, or `keys` not an object -> error `That file isn't a RustRadar keys file.`; each known field that is a non-blank string replaces the current one (stored untrimmed here; `RS:keys.rs` trims on save); none found -> error `That file has no keys in it.`; returns the merged keys and the count.
- `loadKeys` / `saveKeys` become direct calls to `backend::keys`; the localStorage branch (`rustradar.keys`) is dropped.

### 6.8 `signin-values.ts` (26 lines, 7 tests)

```
SIGN_IN_VALUES = [verticalSpeed, squawk, airspace]
missingWithoutSignIn(authenticated, have) -> [] when authenticated, else the values not in hand, in that order
signInNote(missing): "" when none; all three -> "3 values need a Flightradar24 sign-in";
    else names ("vertical speed", "squawk", "airspace") joined with " and ", first letter upper-cased,
    + " needs" (one) or " need" (two) + " a Flightradar24 sign-in"
```

### 6.9 `reference.svelte.ts` (86 lines)

State: `ready`, `error`, `airlines`, `airports`; maps `airlinesByIcao`, `airportsById`, `airportsByCode` (ICAO first, then IATA, both in one map: the lengths never collide). `load()` is idempotent and retried after a failure. `runways(airport)`: `None` while unknown, an empty list when none are known or the airport has no ICAO code; asked once per code, and after a failure asked again 60 s later. `airline(icao)`, `airportById(id)` (0 is no id), `airport(code)` (upper-cased). `airlineIcao(callsign)`: `^([A-Z]{3})\d` -> the three letters, else `None`.

Rust: a plain struct built once from `ReferenceData`, shared as `Arc`; the reactive part (runways arriving later) becomes a repaint request from the task that fetched them. Motion keys airports by identity (`Map<Airport, …>`, `tr.field === tr.destAp`): give airports a stable index into the reference list and key by that.

### 6.10 `phone.svelte.ts` (12 lines)

`PHONE_QUERY = "(max-width: 899px), (max-height: 500px)"`; `layout.phone` follows it. Rust: `phone = width_points < 900.0 || height_points <= 500.0`, re-evaluated on resize, in logical (egui) points.

### 6.11 `weather.ts` (103 lines)

- `Weather { cloud, cloudLow, fog (0..1), precipitation (mm next hour), symbol, temperature (C) }`.
- `Sky = live | sunny | cloudy | rain | fog | snow`.
- `Conditions { sun, cloud, haze (per metre), wet, snow, cloudShadows, rain }`.
- `PRESETS`:

| | sun | cloud | haze | wet | snow | cloudShadows | rain |
|---|---|---|---|---|---|---|---|
| sunny | 1 | 0.05 | 2e-5 | 0 | 0 | 0 | 0 |
| cloudy | 0 | 1 | 7e-5 | 0 | 0 | 0 | 0 |
| rain | 0 | 1 | 1.8e-4 | 1 | 0 | 0 | 1 |
| fog | 0.04 | 1 | 4.5e-3 | 0.35 | 0 | 0 | 0 |
| snow | 0 | 1 | 1.4e-4 | 0 | 1 | 0 | 0 |

- `mixConditions(a, b, t)`: each field `a + (b - a) * clamp01(t)`.
- `conditionsOf(w)` (`:61-84`):
  - `snowing` = symbol contains `snow` or `sleet`, or contains `rain` at `temperature <= 0`.
  - `raining` = not snowing and (symbol contains `rain` or `thunder`, or `precipitation >= 0.1`).
  - `foggy = max(fog, 0.8 if symbol contains "fog")`.
  - `cover = max(cloud * 0.85, cloudLow)`; `sun = clamp01((0.92 - cover) / 0.6) * (1 - foggy)`.
  - `rain = clamp01(precipitation / 2)`.
  - `haze = max(2e-5 + 5e-5 * cloud + (raining ? 6e-5 + 1.2e-4 * rain : 0) + (snowing ? 1.2e-4 : 0), foggy * 4.5e-3)`.
  - result: `sun`; `cloud = clamp01(max(cover, foggy))`; `haze`; `wet = raining ? 0.55 + 0.45 * rain : foggy * 0.35`; `snow = snowing ? 1 : 0`; `cloudShadows = clamp01(4 * sun * (1 - sun)) * clamp01(cloud * 2)`; `rain = raining ? 0.35 + 0.65 * rain : 0`.
- `fetchWeather` is replaced by `backend::weather::weather_at`.

---

## 7. Module notes: sun and shadows

### 7.1 `daynight.ts` (82 lines)

`subsolarPoint(time_ms) -> { lat, lon }` (`:10-23`), low-precision almanac:

```
d = time / 86_400_000 - 10_957.5                       days since J2000.0
g = (357.529 + 0.98560028 d) deg                       mean anomaly
q = 280.459 + 0.98564736 d                             mean longitude, degrees
L = (q + 1.915 sin g + 0.02 sin 2g) deg                ecliptic longitude
e = (23.439 - 0.00000036 d) deg                        obliquity
RA = atan2(cos e sin L, cos L) (degrees);  declination = asin(sin e sin L) (degrees)
eqTime = ((((q - RA) % 360) + 540) % 360) - 180
utcHours = (time / 3_600_000) % 24
lon = ((((-15 (utcHours - 12) - eqTime) % 360) + 540) % 360) - 180;   lat = declination
```

`nightPolygons(time)`: for each of `TWILIGHT_STEPS = [0, -6, -12, -18]` degrees, quads per 2 degree meridian strip. `belowOn(lon, altitude, sun)` scans latitudes -85..85 in 1 degree steps and returns the first and last latitude where the sun's altitude is below the threshold (`sunAltitude = asin(clamp(sin lat sin d + cos lat cos d cos(lon - sunLon)))`). A strip with night on only one side tapers to the midpoint of the other side's interval. Quad order: `[lon, a0], [lon + 2, b0], [lon + 2, b1], [lon, a1]`. The `%` here is JavaScript's (sign of the dividend); the `+ 540` makes the result right, so copy the expressions as written or use `rem_euclid` consistently.

### 7.2 `shadow.ts` (146 lines, 13 tests)

```
sunDirection(time) -> [x, y, z]        unit, Earth-centred: x through 0E on the equator, z north
localSun(sun, lat, lon, out) -> [east, north, up]
    east  = -sinLon x + cosLon y
    north = -sinLat cosLon x - sinLat sinLon y + cosLat z
    up    =  cosLat cosLon x + cosLat sinLon y + sinLat z          (sine of the sun's altitude)
shadowMatrix(orientation [pitch, yaw, roll] deg, scale [sx, sy, sz], toSun, height, out, groundZ = 0) -> [f64; 16] column-major
    rotation exactly as deck.gl's mesh-layers matrix (shadow.ts:64-79), then kx = toSun.x / toSun.z, ky = toSun.y / toSun.z:
    col0 = (m00 - kx m20, m10 - ky m20, 0, 0); col1 = (m01 - kx m21, m11 - ky m21, 0, 0); col2 = (m02 - kx m22, m12 - ky m22, 0, 0)
    col3 = (-kx (height - groundZ), -ky (height - groundZ), groundZ, 1)
SHADOW_MAX_ALPHA = 0.5;  SHADOW_MIN_ALPHA = 0.05
SUN_DIAMETER = 0.53 deg;  MEMBER_FRACTION = 0.1;  LOW_SUN_DEG = 6
sunlight(sinSun) = sinSun > 0 ? min(1, asin(min(1, sinSun)) in degrees / 6) : 0
shadowOpacity(altFt, sizeM, sinSun):
    light = sunlight(sinSun); 0 -> 0
    slant = max(0, altFt) * 0.3048 / sinSun;  crisp = 0.1 * sizeM / (0.53 deg in radians)
    contrast = slant <= crisp ? 1 : (crisp / slant)^1.5
    alpha = 0.5 * contrast * light;  below 0.05 -> 0
```

Doc 03 section 5.4 has how the renderer uses these.

### 7.3 `building-shadow.ts` (121 lines, 10 tests)

`SHADOW_LENGTH_CAP = 10`, `BUILDING_SHADOW_ALPHA = 0.35`. `shadowStep(toSun, cap)`: `None` unless `up > 0`; `[0, 0]` with the sun overhead; else `length = min(hypot(east, north) / up, cap)` in the direction away from the sun. `buildingShadows(footprints, heights, step)`: for every ring edge whose outward normal faces away from the sun (`sign * (ey * dx - ex * dy) > 0`, `sign` = sign of the ring's signed area for an outline, its negative for a hole) emit a quad `a, b, b + d, a + d` (`d = step * height`) as two triangles `0 1 2, 0 2 3`. Rings with fewer than 3 vertices, zero area or height <= 0 cast nothing; `None` when nothing was emitted or the step is zero. Output is `Float32Array` positions and `Uint32Array` indices: use `f32`/`u32` so the tests' values match. `Math.sign(area)` is never 0 here because zero-area rings are skipped first.

---

## 8. Module notes: routes

### 8.1 `flightpath.ts` (948 lines, covered by `routes.test.ts`, 15 cases)

Doc 03 section 15 records the constants and the algorithms in prose (glide 3 degrees, `AIM_M`, final 2-25 NM in 0.25 NM steps with penalties 3 / 0.2 / 4, bank 25 degrees, terminal 140-230 kt, en route 140-480 kt, 500/350/600 ft per NM, `LOCAL_M` 150 km, `ENTRY_M`/`JOIN_M` 60 km, `CLIMB_OUT_M` 6 km, arc step 3 degrees, line step 2 km, great circle step 25 km). This section is the porting checklist.

Exported API:

```
type LonLat = [f64; 2];  type PathPoint = [f64; 3]  (lon, lat, ft MSL)
GLIDE_DEG = 3;  AIM_M = 50 ft * 0.3048 / tan(3 deg) (about 290.8 m);  FINAL_NM = 10
distanceM(a, b), distanceKm(a, b), bearing(a, b), destination(a, course, m), greatCircle(a, b, stepKm = 80) -> Vec<LonLat>
turnRadius(speedKt) = v^2 / (9.80665 * tan(25 deg)), v = kt * 0.514444
dubins(x0, y0, h0, x1, y1, h1, r) -> Vec<[f64; 2]>          test-only
turnThenStraight(x0, y0, h0, x1, y1, r) -> Vec<[f64; 2]>    test-only
struct Pose { at: LonLat, heading }
struct RunwayEndInfo { ident, heading, start, threshold, elevationFt, lengthM }
runwayEnds(runways, fieldElevationFt = 0) -> Vec<RunwayEndInfo>
centrelineOffset(end, p) -> { along, cross }
struct Traffic { id, lat, lon, track, alt, speed, onGround }
runwayUse(ends, traffic, ignoreId) -> per end { arrivals, departures }
enum RunwayReason { aligned, traffic, guessed };  struct RunwayChoice { end, reason }
struct Aircraft { lon, lat, track, altFt, speedKt }
chooseArrivalRunway(ends, ac, traffic, ignoreId) -> Option<RunwayChoice>
chooseDepartureRunway(ends, to, traffic) -> Option<RunwayChoice>
arrivalPath(ac, end), arrivalToPoint(ac, airport, elevationFt = 0),
departurePath(end, to), departureFromPoint(airport, elevationFt, to) -> Vec<PathPoint>
trimToPosition(path, here, searchM = 20_000, nearM = 3_000) -> Vec<PathPoint>
struct RunwayGuess { runway, reason }
struct EstimateInput { flight, trailStart, origin, destination, originRunways, destinationRunways, traffic }
estimateRoute(input) -> { kind: estimated | none, before, ahead, departure, arrival }
```

Internal pieces, in dependency order (each a private function to port): `intermediate`, `unwrap`, `toLocal`/`fromLocal` (azimuthal equidistant frame), `mod2pi`, `dubinsCandidates` (six words LSL, RSR, LSR, RSL, RLR, LRL; `:169-208`), `advance`, `samplePieces`, `toMath`, `Plan`, `samplePlan`, `directPlan`, `dubinsPlan` (keeps only candidates that really arrive: end within `max(1, r * 1e-4)` m and heading within `1e-3` rad), `turnThenStraightPlan`, `plan` (local below 150 km, else join + great circle + end turn), `sample`, `connect`, `HARD_SURFACE` regex, `flowInUse`, `alignedWith`, `landingOn`, `ROLLOUT_M = 1400`, `glideFt`, `arrivalPlan`, `straight`, `descend`, `departurePlan`, `climb`, `near` (`TRAFFIC_KM = 30`).

Things that are easy to get wrong:

- Angles: compass degrees outside, mathematical radians (counter-clockwise from east) inside the Dubins code. `toMath(h) = (90 - h) deg`.
- `runwayEnds` keeps hard-surfaced runways when the airport has any (`/ASP|CON|PEM|BIT|TAR|PAV|SEAL|MAC|CEM/i`), then ends with `lengthM >= min(0.7 * longest, 1800)`.
- `arrivalPlan` returns a length and a *lazy* `build` closure because `chooseArrivalRunway` plans every end and samples only the winner. Keep that split (a `Plan` value with a separate `build()`), the search over 93 fixes times every end is the hot path.
- `chooseArrivalRunway` precedence: landing on it now, lined up with it, the flow other traffic shows (arrivals counted twice, parallels within 15 degrees grouped; prefer the end with more arrivals, then the smaller cross distance), shortest plan.
- `estimateRoute` fills `ahead` only for an airborne flight with a destination, and `before` only when the origin is more than 15 km from the first recorded trail point; `speedKt` falls back to 250 when the trail point has none.
- `Array.reduce` without an initial value on `landing` and `aligned`: the first element is the seed and ties keep the earlier one.

### 8.2 `routes.ts` (134 lines, no tests of its own)

```
decodeWaypoints(plan, departure) -> Vec<LonLat>
    scales 1, 1e4, 1e5, 1e6, 1e7; keep those where every point is a valid coordinate; none -> empty;
    without a departure the first (smallest divisor); else the one whose first point is nearest the departure
mergeTrail(trail, opensky) -> { trail, added }
    first = first trail timestamp > 0; earlier = OpenSky points more than 30 s before it; fewer than 2 -> unchanged
    each becomes a TrailPoint with source -1, vertical_speed 0, ground_speed = min(700, round(km / 1.852 / dt * 3600))
    to the next point (the trail's first point after the last OpenSky one), 0 when dt <= 0
struct Route { origin, destination, kind: filed | estimated | none, filed, before, ahead, departure, arrival }
routeFor(flight, details, traffic) -> Route
    origin: schedule.origin_id, else flight.origin, else plan.departure
    destination: schedule.diverted_id or destination_id, else flight.destination, else plan.destination
    more than one decoded waypoint -> filed; else estimateRoute with the first trail point that has a position
airacCycle(now) -> { ident, effective, expires }
    epoch 2024-01-25T00:00Z (cycle 2401), period 28 days; n = floor((now - epoch) / period)
    ident = yy + two-digit (n - ceil((Jan 1 of that year - epoch) / period) + 1)
```

`routeFor` reads the global `reference`; in Rust pass `&Reference` in. Re-exports from `flightpath` exist only for import convenience.

---

## 9. Module notes: taxiways

### 9.1 `taxinet.ts` (138 lines)

`TaxiNetwork::new(lines)`: origin = mean of all points; `kx = M_PER_DEG * max(cos(lat0), 1e-3)` with `M_PER_DEG = 6_371_000 * pi / 180`; each consecutive pair of points longer than 0.1 m becomes a segment `(ax, ay, ux, uy, len)` in local east/north metres. Grid: `CELL_M = 50`; a segment is sampled every `len / ceil(len / 25)` and listed in the 3 x 3 cells around each sample (no duplicates in a row). `MARGIN_M = 100`: `covers(x, y)` is the bounding box grown by it. `x(lon)` wraps the longitude difference to +/-180 before scaling; `y(lat)`. `near(x, y)` returns the segment list of the point's cell. `project(net, s, x, y)` clamps the foot of the perpendicular to the segment and returns the point and distance (through module globals `projX`, `projY`, `projDist`; return a tuple in Rust). `cellKey = (cx + 32768) * 65536 + (cy + 32768)`: use an `(i32, i32)` key.

### 9.2 `taxiways.ts` (66 lines)

`TAXIWAY_ZOOM = 12`; at most `MAX_LOADED = 6` networks, least recently wanted evicted (`motion.setTaxiways(code, null)`); `RETRY_MS = 5 min` after an empty or failed answer; codes must be four characters after trim and upper-casing; an empty answer is not stored. `taxiwaysFor(id, phase)`: only for ground phases, asks for the network of `motion.groundField(id)`. Called per drawn aircraft while zoomed in, so the fast path (already loaded: touch the timestamp) must stay cheap.

---

## 10. Module notes: aircraft motion (`motion.ts`, 3411 lines, 46 tests)

Nothing of this exists in Rust. It is called per visible aircraft per frame (thousands of calls at 20-60 Hz), so the port must keep its allocation-free style: per-track `Vec`s that are cleared and refilled, and no per-call heap use beyond the returned `Pose` (a `Copy` struct in Rust). Doc 03 section 1.1 describes `Pose` from the renderer's side.

### 10.1 Public API

```
struct Measured { positionMs?, lat?, lon?, mlat?, seenMs, onGround, altBaro?, navQnh?, gs?, track?, trueHeading?, magHeading?,
                  roll?, trackRate?, baroRate?, geomRate?, tas?, navAltitudeMcp?, navAltitudeFms?, navHeading?, navModes? }
enum Phase { parked, taxi, takeoffRoll, initialClimb, climb, cruise, descent, approach, flare, landingRoll }
struct Pose { lon, lat, altFt, heading, track, pitch, bank, gear, phase }

impl Motion {
    now() -> ms                                   local clock + clockOffset (in Rust: pass the clock in)
    syncClock(serverTimeMs)                       ignores non-finite or < 1e12; first sample or a jump > 5000 ms is taken as is,
                                                  else offset += 0.1 * (sample - offset)
    setAirports(airports)                         byIata map; 1 degree grid, key (latCell + 90) * 360 + (lonCell mod 360)
    setTaxiways(icao, lines | null)               builds or forgets a TaxiNetwork; tracks pointing at the old one are detached
    hasTaxiways(icao) -> bool
    groundField(id) -> Option<Airport>            the track's field while its phase is a ground phase
    pose(f: &LiveFlight, nowMs) -> Pose           lazily ingests f when f.timestampMs is newer than the track's last FR24 report
    drawnTime(id, nowMs) -> ms                    nowMs - delay (2-12 s); nowMs for an unknown aircraft
    measure(id, m: Option<&Measured>, nowMs)      adsb.lol state for the selected aircraft; None drops it
    drawAltitude(id, altFt, lat, lon) -> ft       trail points at the same airport-relative height as the model
    fieldQnh(ap, nowMs) -> hPa | NaN
    prune(nowMs)                                  drops tracks untouched for 15 min
}
const motion = new Motion()                       a singleton; in Rust an owned value in the app state
```

`pose` flow (`:1080-1121`): get or create the track; ingest when the track is empty, or when the report is newer and the per-frame budget allows (`INGEST_BUDGET = 600` per distinct `nowMs`); prune once a minute; `T = nowMs - delay`; `state = stateAt(T)`; if the state changed since the last frame, `takeOver` (blend); evaluate with offsets; taxi snap; clamp pitch to -10..20 and bank to +/-30; gear.

### 10.2 Constants (all of them)

Units: ms unless named. `DEG = pi/180`, `EARTH_RADIUS_M = 6_371_000`, `M_PER_DEG = EARTH_RADIUS_M * DEG`, `KNOTS_TO_MS = 0.514444`, `FEET_TO_M = 0.3048`, `FPM_TO_MS = 0.3048 / 60`, `GRAVITY_MS2 = 9.80665`.

| Group | Constant = value |
|---|---|
| Samples | `SAMPLE_WINDOW_MS 30_000`; `JUMP_M 150`; `JUMP_LOOKAHEAD_MS 4_000`; `MIN_SAMPLE_GAP_MS 200`; `BACKWARD_LIMIT_S 30`; `BUFFER_SCALE 1e-5` |
| Estimators | `SMOOTHING 0.5`; `MAX_ACCEL_KTS 4` kt/s; `MAX_TURN_DEG_S 4`; `MAX_VRATE_FPM 8_000`; `MIN_VRATE_DT_S 4`; `MAX_VRATE_DT_S 300`; `MIN_ACCEL_DT_S 4`; `MAX_ACCEL_DT_S 120`; `MIN_TURN_DT_S 2`; `MAX_TURN_DT_S 60`; `MIN_TURN_CHORD_M 100`; `MIN_SPEED_CHORD_M 10`; `MAX_SPEED_MS 1200 kt` |
| Extrapolation | `MAX_EXTRAPOLATION_S 600`; `TURN_HORIZON_S 60`; `BANK_FADE_START_S 45`; `ACCEL_HORIZON_S 20`; `MAX_AIR_SPEED_CHANGE_KT 30`; `MAX_TAXI_ACCEL_KTS 1`; `MAX_TAXI_SPEED_KT 30`; `VERTICAL_FADE_START_S 60`; `VERTICAL_HORIZON_S 90`; `MAX_ALT_FT 60_000`; `MAX_CURVATURE 1/30` per m; `HEADING_MIN_SPEED_MS 1` |
| Blending | `BLEND_RATE 2.2` /s; `BLEND_END_KT 13.2`; `GENTLE_ACCEL 3` m/s2; `MIN_BLEND_RATE 0.5` /s; `GENTLE_MAX_M 40`; `SNAP_DISTANCE_M 5_000`; `SNAP_AGE_MS 120_000` |
| Measured | `MEAS_FRESH_MS 15_000`; `MEAS_FADE_MS 5_000`; `COINCIDENT_MS 1_000`; `DECRAB_AGL_FT 60`; `MAX_CRAB 30`; `ROLL_OUT_S 4`; `CAPTURE_FT_PER_FPM 0.15`; `MIN_CAPTURE_FT 100`; `MAX_CAPTURE_FT 800`; sources `SRC_MLAT 0 < SRC_FR24 1 < SRC_ADSB 2` |
| Playback | `MIN_DELAY_MS 2_000`; `MAX_DELAY_MS 12_000`; `DELAY_RATE 0.1`; `DELAY_ACCEL 0.01` /s; `DELAY_DEADBAND_MS 500`; `DELAY_SMOOTHING 0.3`; `DEFAULT_CADENCE_MS 8_000`; `MIN_CADENCE_MS 1_000`; `MAX_CADENCE_MS 60_000`; `MIN_MARGIN_MS 1_000`; `MAX_MARGIN_MS 3_000` |
| Smoothing | `SMOOTH_LAMBDA 20` s3; `ALT_SMOOTH_LAMBDA 200`; weights `WEIGHT_FR24 1`, `WEIGHT_ADSB 1`, `WEIGHT_MLAT 0.04`; `REPORTED_SLOPE_WEIGHT 10`; `REF_FULL_S 12`; `REF_ZERO_S 24`; `REF_PASSES 3`; `REF_LOCAL_MS 4_000`; `REF_MIN_CHORD_S 2`; `TURN_WINDOW_MS 2_000`; `BANK_LEAD_MS 1_000`; `TURN_HANDOVER_S 3`; `VRATE_HANDOVER_S 3`; `ALT_STEP_FT 300`; `ALT_WINDOW_MS 60_000`; `MAX_STATES 16`; `MEAS_HISTORY_MS 30_000`; `CONTINUITY_GAP_MS 1_000` |
| Pressure altitude | `STD_HPA 1013.25`; `ISA_H0_FT 145_442.16`; `ISA_EXP 0.190263`; `MIN_QNH_HPA 940`; `MAX_QNH_HPA 1_060`; `QNH_TTL_MS 3 h`; `QNH_NEIGHBOUR_KM 300`; `QNH_SMOOTHING 0.3`; `QNH_OUTLIER_HPA 5`; `QNH_OUTLIER_SMOOTHING 0.05`; `MEAS_QNH_MAX_AGL_FT 3_000`; `RUNWAY_FIELD_KM 8`; `RUNWAY_TOL_KNOWN_FT 150`; `RUNWAY_TOL_UNKNOWN_FT 600`; `LEVEL_FT 25`; `RUNWAY_DECEL_KTS 1`; `MIN_FLYING_KT jet 60, turboprop 50, light 30`; `LEVEL_MAX_DT_S 20` |
| Flare | `FLARE_HEIGHT_FT 50`; `TOUCHDOWN_SINK 0.2`; `FLARE_SPAN_FT = 2 * 50 / 1.2 = 83.33`; `FLARE_FLOOR_FT = 50 - 83.33 = -33.33`; `FLARE_A = 0.8 / (2 * 83.33) = 0.0048`; `REFERENCE_BLEND_RATE 0.8` /s; `REFERENCE_MAX_FT_S 30`; `TOUCHDOWN_FIT_MS 2_000` |
| Airports | `FIELD_RANGE_KM 40`; `NEAREST_FIELD_KM 15`; `FADE_FULL_KM 30`; `FADE_ZERO_KM 60`; `GRID_DEG 1` |
| Phases | `PARKED_MAX_KT 3`; `TAXI_MAX_KT 40`; `ROLL_ACCEL_KTS 0.5`; `APPROACH_MAX_AGL_FT 3_000`; `APPROACH_MAX_VRATE_FPM -200`; `APPROACH_RANGE_KM 25`; `FLARE_MAX_AGL_FT 60`; `INITIAL_CLIMB_MIN_VRATE_FPM 200`; `CLIMB_VRATE_FPM 500`; `CLIMB_HOLD_VRATE_FPM 250` |
| Takeoff and landing | `LANDING_DECEL_KTS 2`; `ROLLOUT_KT 20`; `TAKEOFF_ACCEL_KTS 2.5`; `TAKEOFF_OVERSPEED_KT 20`; `TAKEOFF_CLIMB_FPM 1_800`; `ROTATION_S 3`; `ROTATION_PITCH 8`; `TOUCHDOWN_EASE_S 4`; `ROTATION_KT jet 145, turboprop 110, light 60` |
| Attitude | `PITCH_OFFSET`: parked, taxi, takeoffRoll, landingRoll 0; initialClimb 6; climb 3; cruise 2; descent 1; approach 3; flare 5. `MIN_PITCH -10`; `MAX_PITCH 20`; `MAX_BANK 30` |
| Gear | `GEAR_TRANSIT_MS 8_000`; `GEAR_APPROACH_AGL_FT 2_000`; `GEAR_CLIMB_AGL_FT 400`; `GEAR_FALLBACK_ALT_FT 1_500`; `GEAR_FALLBACK_KT 180` |
| Housekeeping | `TRACK_TTL_MS 15 min`; `INGEST_BUDGET 600`; `PRUNE_INTERVAL_MS 60_000`; `CLOCK_RESYNC_MS 5_000`; `CLOCK_SMOOTHING 0.1`; `MIN_SANE_TIME_MS 1e12` |
| Taxi snap | `SNAP_NEAR_M 15`; `SNAP_FAR_M 30`; `SNAP_ALIGNED_DEG 20`; `SNAP_WIDE_DEG 35`; `SNAP_SLOW_KT 25`; `SNAP_FAST_KT 35`; `SNAP_LOOKAHEAD_MS 3_000`; `SNAP_SOFT_M 15`; `SNAP_NOW_SHARE 0.7`; `SNAP_AHEAD_LAG_MS 500`; `SNAP_GATE_RATE 0.8` /s; `SNAP_SPRING_RATE 5` /s; `SNAP_MIN_SPEED_MS 0.5` |
| Phase sets | `GROUND = {parked, taxi, takeoffRoll, landingRoll}`; `FROM_TAXI = {parked, taxi, takeoffRoll}`; `FROM_LANDING = {approach, flare, landingRoll, descent}` |

Aircraft class (`:569-580`), tested against the upper-cased typecode, anchored at the start:

- heli: icon `EC`, or `^(EC\d|H\d\d|AS3|AS5|AS6|R22|R44|R66|B06|B47|B407|B412|B429|A109|A119|A139|A169|A189|AW1|S76|S92|MI8|UH|H60|BK17|EH10|NH90|EXPL|CH47|GAZL|ALO)`
- turboprop: icon `Q300`, or `^(AT4|AT7|DH8|DHC|SF34|E120|JS3|JS4|B190|C208|PC12|PC6|D228|D328|SW4|F50|BE20|BE9|BE30|AN2[46]|AN3|L410|C130|A400|P180|TBM|SB20|Y12|C212|CN35|C295|BN2)`
- light: icon `C206`, `C303`, `GLID`, `AS20`, `ASW20`, or `^(C1\d\d|C2[0-9]|PA\d|P28|SR2|DA[24]|BE3|BE[0-9]{2}$|M20|RV|GLID|AS2|ASW|DG|LS\d|ULAC|TOBA|AT3|C42|P2002)`
- else jet. Order matters (heli, then turboprop, then light).

### 10.3 Small functions (port first, each is a few lines)

| Function (`motion.ts` line) | Definition |
|---|---|
| `clamp`, `wrapLon`, `wrap180`, `wrap360`, `smoothstep` (582-586) | `wrapX(v) = (((v + 180) % 360) + 360) % 360 - 180`; `wrap360(v) = ((v % 360) + 360) % 360`; `smoothstep(t) = 0 below 0, 1 above 1, t^2 (3 - 2t)` |
| `distanceKm` (589) | equirectangular: `hypot(wrap180(dLon) * cos(mean lat), dLat) * M_PER_DEG / 1000` |
| `trueAltitude(p, qnh)` (395) | `H0 - (H0 - p) * (1013.25 / qnh)^0.190263` |
| `qnhFrom(p, true)` (400) | `1013.25 / ((H0 - true) / (H0 - p))^(1 / 0.190263)` |
| `validQnh(q)` (405) | finite and 940..1060 |
| `flare(x)` (437) | `x` at or above 50; else `u = x - FLARE_FLOOR_FT`, `0` for `u <= 0`, `FLARE_A u^2 + 0.2 u` |
| `flareSlope(x)` (444) | `1`; `0`; `2 FLARE_A u + 0.2` |
| `verticalWeight(t)` (595) | `0` before -30 s; `1` to 60 s; then linear to `0` at 90 s |
| `verticalIntegral(t)` (602) | `max(t, -30)` to 60 s; then `60 + u - u^2 / 60` with `u = min(t, 90) - 60` |
| `verticalIntegralInverse(a)` (610) | `max(0, a)` to 60; else `60 + 30 - sqrt(900 - 60 (a - 60))`, NaN if the root is negative |
| `blend(t, k)` (619) | `1` for `t <= 0`; `0` for `k t >= 13.2`; else `(1 + kt) e^-kt` |
| `gentleRate(x0, v0)` (631) | `k = (-v0 + sqrt(v0^2 + 3 x0)) / x0` if `x0 > 1e-6`; else `3 / (2 v0)` if `v0 > 1e-6`; else `2.2`; clamped to 0.5..2.2 |
| `fieldFade(km)` (700) | `clamp((60 - km) / 30, 0, 1)` |
| `measWeight(tr, T)` (691) | `0` without measured data; else `1 - smoothstep((T - measMs - 15_000) / 5_000)` |
| `measFresh(tr, t)` (697) | has measured data and `t - measMs <= 15_000` |
| `airPitch(vs, speed, phase, agl)` (705) | `gamma = atan2(vs * FPM_TO_MS, max(speed, 10))` in degrees; offset by phase, and for approach or flare below 60 ft AGL eased from 3 to 5 as `1 - max(0, agl) / 60`; clamped -10..20 |
| `correctAlt(raw, w, qnh)` (3069) | `raw + w * (trueAltitude(raw, qnh) - raw)` when `w > 0` and the QNH is valid, else `raw` |
| `plausibleKm(tr, gapS)` (2546) | `max(speedKt, 250) * 2 * 0.514444 * |gapS| / 1000 + 0.5` |
| `spring(x0, v0, target, dt)` (2257) | critically damped at 5 /s: `d = x0 - target; b = v0 + 5 d; x = target + (d + b dt) e^-5dt; v = (v0 - 5 b dt) e^-5dt` |
| `profile(tr, dt)` (979), `ramp(tr, a, target, maxDur)` (1001) | piecewise constant-acceleration speed profile: distance and speed after `dt`; `ramp` appends a piece until `target` or `maxDur` (skipped when its duration is not positive and finite) |
| `seriesSlope`, `seriesAt` (3166, 3177) | monotone C1 cubic through a time series (slope 0 at an extremum or an end, limited to 3 times the smaller neighbouring difference), held flat beyond its ends |
| `pushSeries` (3151) | append a newer value (same time overwrites, older ignored), keep 30 s |

`Blend` (`:642-688`): a correction decaying critically damped from an offset and a rate, in two dimensions: `x(t) = (x0 + (v0 + k x0) t) e^-kt`, `v(t) = (v0 - k (v0 + k x0) t) e^-kt`, zero once `k t >= 13.2`. `add(now, dx, dy, dvx, dvy, k)` evaluates itself at `now`, adds, restarts, and takes `k` or (when `None`) `gentleRate` of the summed magnitude times `scale` (1 for metres, 0.3048 for feet). `clear()` sets its time to minus infinity. A track has four: `fast`, `gentle` (east/north metres), `altFast`, `altGentle` (feet).

### 10.4 Per-track state (`Track`, `:735-920`)

Port as one struct; group the fields as the source does. Rust types: times `f64` ms (the code relies on `-Infinity` and `NaN` sentinels; either keep `f64` with the same sentinels, which is the faithful and low-risk choice, or use `Option` and re-derive every comparison; section 14).

- identity: `id`, `touched`, `cls`, `icon`, `typecode`
- latest report: `reportMs`, `fr24Ms`, `lat0`, `lon0`, `alt`, `speedKt`, `trackDeg`, `onGround`, `originAp`, `destAp`, `field`, `fieldElev`, `fieldKm`
- estimates: `accel` (kt/s), `reportAccel`, `turnRate` (deg/s), `vrate` (fpm), `vrRefMs`, `vrRefRaw`, `vrRefW`, `vrRefGround`, `phase`
- pressure altitude: `qnh`, `qnhOwn`, `qnhOwnField`, `rawAlt`, `rawAltMs`, `rawW`, `groundMs`, `altBlendRate`
- horizontal samples (oldest first): `n`, `ts`, `lats`, `lons`, `xs`, `ys` (raw, local metres around the newest), `fx`, `fy` (smoothed values), `mx`, `my` (smoothed slopes, m/s), `om` (turn rate per sample, rad/s), `rTs`, `rOm`, `rAc` (the last fit's reference rates), `src`, `mixed`, `refLat`, `refLon`
- altitude samples (airborne only): `ats`, `aps` (pressure ft), `aws` (weights), `avs` (corrected), `afv`, `afd` (smoothed value, slope ft/s)
- report states: `states: Vec<State>` (at most 16), `activeMs`, `lastPoseMs`. `State { reportMs, phase, trackDeg, alt, vrate, field, fieldElev, liftoffMs, touchdownMs, touchdownPitch, capAlt }`
- measured: `meas`, `measMs`, `measPosMs`, `crab`, `tasMs`, `navTrack`, `capAlt`, `turnEndS`, `rollTs`, `rolls`, `rollBiases`, `crabTs`, `crabs`
- delay: `dFrom`, `dTo`, `dSlope`, `dMs`, `dDur`, `cadenceMs`, `delayTarget`, `newestMs`, `arrivalDataMs`
- extrapolation: `v0`, `h0` (rad), `kappa` (1/m), `sTurnMax` (m), `segA`, `segDur`, `profEndV`, `profEndT`, `touchdownMs`, `touchdownPitch`, `liftoffMs`
- blending: `offMs`, `fast`, `gentle`, `altFast`, `altGentle`, `offHdg`, `offTrk`, `offPitch`, `offBank`
- taxi snap: `snapNet`, `snapGate`, `snapMs`, `snapAx`, `snapAy`, `snapAheadMs`, `snapE`, `snapN`, `snapH`, `snapVE`, `snapVN`, `snapVH`, `snapHd`
- gear: `gearTarget` (-1 = not set), `gearFrom`, `gearMs`

Module-level scratch (`scratchA/B/C: Raw`, `profS/profV`, `hx/hy/hvx/hvy/hi/hu`, `amAlt/amVs`, `springV`, `newT/newLat/newLon`, `outT/...`, `reportFromMs`, `splineBuf`, `weightBuf`, `refX/...`, `resX/...`) becomes return values or a `Scratch` struct owned by `Motion` and passed down. `Raw` (`:923-947`) is the un-blended evaluation result: `lat, lon, altMsl, drawAlt, heading, track, pitch, bank, phase, speedMs, vsFpm, altRaw, vsRaw, agl, ground, vE, vN, modelBank, hasField`.

### 10.5 Taking in a report (`update`, `:1400-1532`)

1. Class from icon and typecode when either changed.
2. A report older than measured state already taken in only contributes samples and the route (`mergeReport(..., older = true)`, then `refresh` and `noteArrival`).
3. An airborne report with altitude 0 has no altitude: the model's own altitude and rate at that time are used (`evaluate` at `t` before anything changes).
4. Store speed (non-negative), track (wrapped), position (clamped, wrapped); `selectField`; `mergeReport`.
5. Acceleration: across reports when 4..120 s apart, along the buffer (`bufferAcceleration`), their mean when both, relaxing (`accel * 0.5`) when neither; smoothed 0.5 and clamped to +/-4 kt/s. `reportAccel` separately when 2..120 s apart.
6. QNH (`trackQnh`: own altimeter setting while fresh and below 3000 ft AGL, else own runway reading at this field, else `fieldQnh`), `rawW = fieldFade(fieldKm)`.
7. Ground or air: FR24's flag, corrected by `onRunway` (airborne flag but on the runway) or noted by `touchedDown` (ground flag after an approach). `setQnh` re-corrects the altitude and arms `altBlendRate = 0.8` when the newest altitude sample moved more than 50 ft.
8. Altitude: 0 on the ground (and `groundMs = t`); corrected and added to the altitude samples in the air; estimated otherwise.
9. Turn rate: buffer chords (`bufferTurnRate`, not on the ground), else reported tracks when 2..60 s apart, else 0; 0 when parked; smoothed 0.5, clamped +/-4 deg/s.
10. Vertical rate: reported `vspeed` when present (unless it is 0 alongside an estimated altitude); 0 on the ground; the model's own when the altitude was estimated; else altitude difference against the reference report at least 4 s old (from the field elevation if that reference was on the ground), smoothed 0.5 unless fresh, 0 after 300 s; clamped +/-8000 fpm.
11. `measuredRates`, `decidePhase`, `refit`, `noteArrival`.

`onRunway` (`:1545-1582`): needs a field within 8 km, not a helicopter; with no altitude, whatever the model had; the corrected altitude within 150 ft of the field (600 ft with no QNH); then on the runway if below flying speed, or previously on the ground and below rotation speed or level, or level after a landing phase while braking at 1 kt/s or more. A runway reading gives the field's QNH (`learnQnh`), and the aircraft's own (`qnhOwn`) only if it was on the ground before or the reading does not lift it.

`learnQnh` (`:1206`): new or expired (3 h) readings replace; otherwise exponential smoothing 0.3, or 0.05 for a reading more than 5 hPa off. `fieldQnh` (`:1184`): own reading while fresh; else the inverse-square-distance mean (`1 / (km + 20)^2`) of fields within 300 km, cached per field until any QNH is learned or 60 s pass.

`selectField` (`:1735`): destination within 40 km, else origin within 40 km, else nearest airport within 15 km (3 x 3 grid cells), else the previous field while within 60 km.

`decidePhase` (`:3372-3409`), with hysteresis from the previous phase:

- ground: below 3 kt parked; below 40 kt (or any helicopter) taxi; else by acceleration (`reportAccel`, else `accel`): after a landing phase takeoffRoll only above +1 kt/s, after a taxi phase landingRoll only below -1 kt/s, else +/-0.5 kt/s decides, else landingRoll at the destination field and takeoffRoll elsewhere.
- air, with a field and AGL below 3000 ft: within 25 km and descending faster than 200 fpm (or already on approach and not climbing more than 200 fpm): flare below 60 ft, else approach; within 25 km and climbing more than 200 fpm, or after takeoffRoll, or still initialClimb and not sinking more than 200 fpm: initialClimb.
- otherwise climb above +500 fpm (held above +250 after climb or initialClimb), descent below -500 (held below -250 after descent or approach), else cruise.

### 10.6 Samples (`:2380-2641`)

- `reportPoints`: buffer entries `[dLat, dLon, dMs]` become points at `lat0 - dLat * 1e-5`, `lon0 - dLon * 1e-5`, `t - dMs`; kept when `200 <= dMs <= 30_000` and valid; inserted in time order, dropping any closer than 200 ms to a neighbour; the report point last. `reportFromMs` = the oldest.
- `mergeReport`: a report implausibly far from the nearest-in-time held sample (`plausibleKm`, capped at 5 km), or (unmixed tracks) with a point more than 150 m off the smoothed path inside the samples or up to 4 s past them (`offPath`), clears the samples (or is dropped, returning `None`, when it is older than measured state). Then: drop buffer points older than the record; drop points a better source already has within 1 s (mixed tracks); merge in time order, a held sample within 1 s of a new point giving way; trim to the last 30 s; rebuild the local frame around the newest sample (`finishSamples`). Returns whether the newest sample is the report point itself.
- `mergeMeasured`, `insertSample`, `startMixing`: measured positions join in time order with source ranks; within 1 s the better source wins, of equals the newer arrival.
- `finishSamples`: `xs = wrap180(lon - refLon) * M_PER_DEG * max(cos(refLat), 1e-3)`, `ys = (lat - refLat) * M_PER_DEG`.

### 10.7 Smoothing (`:2643-3148`)

- `smoothSpline(n, ts, ws, lambda, ya, fa, da, yb, fb, db, endSlopeW, endSlopeA, endSlopeB)` (`:2664-2788`): the natural cubic smoothing spline in Hermite form, minimising `sum w (f - y)^2 + lambda * integral f''^2` (time in seconds), optionally pulling the last slope towards a value with weight `endSlopeW`. Block tridiagonal system with 2 x 2 blocks (value, slope), 11 numbers per row in a flat buffer, forward elimination then back substitution; two series share one factorisation. Port the arithmetic literally: it is 120 lines of scalar code with no library calls, and the tests ("the smoothed path stays on exact samples", "noisy positions are smoothed") check it to centimetres.
- `fitTrack(tr, reportedOnly)` (`:2833-2898`): one sample gets the reported velocity. Otherwise up to 3 passes of `fitAround`: the samples' residual from a reference path (which turns at `refOm` and accelerates at `refAc`, per sample, integrated backwards from the newest sample in steps of at most 1 s) is smoothed with `lambda = 20` and the reference added back. Reference rates start from the report's estimates weighted by `referenceWeight` (full for 12 s back, fading to 0 at 24 s), replaced where the previous fit covered the same times; later passes read them off the previous pass over +/-4 s (`localRates`, needing at least 2 s of chord) and stop when no rate changed by more than 0.02 deg/s or 0.02 m/s2. Parked or slower than 1 m/s: no reference (plain spline). When the newest sample is the report itself, its slope is pulled to the reported velocity with weight 10.
- `fitAltitude` (`:3021`): corrected altitude samples, `lambda = 200`; one sample takes the slope `vrate / 60`.
- `addAltitude` (`:3042`): a new flight segment after a ground report drops older samples; out-of-order samples are ignored; a jump over 300 ft at more than 10 000 fpm clears; within 1 s replaces the newest; keep 60 s.
- `hermite(tr, T)` (`:3085`), `altitudeAt` (`:3110`): cubic Hermite value and derivative on the segment containing `T` (search from the newest backwards).
- `turnNodes` (`:3128`): per-sample turn rate from the smoothed heading's change over +/-2 s (at least 0.5 s available), clamped to 4 deg/s.

### 10.8 Delay (`:3193-3305`)

`noteArrival`: when the newest sample advanced, cadence is smoothed (0.3) from the gap between report times clamped 1..60 s; `margin` = mean spacing of the last up to 3 sample gaps clamped 1..3 s; `target = clamp(cadence + margin + age, 2 s, 12 s)` with `age = now - newest`; smoothed 0.3 (set outright the first time); `retarget`. `retarget`: ignores changes under 500 ms; otherwise a cubic Hermite ease from the current delay and slope to the target over `dDur = max(1000, 1.5 * delta / 0.1, (4 s + sqrt(16 s^2 + 24 j delta)) / (2 j))` ms, `s` = current slope magnitude, `j = 0.01 / 1000`. `delayAt`, `delaySlope` evaluate the ease; `drawTime = now - delayAt`. `snapshot` keeps one `State` per report time (replacing a same-time one), at most 16; `stateAt(T)` is the newest state not after `T`, the oldest before them all.

### 10.9 Prediction (`predict`, `:1755-1836`)

From the end of the smoothed path: `h0` from the last slope (the reported track when parked or slower than 1 m/s), `v0` (0 when parked, capped at 1200 kt), `kappa = clamp(turnRate / v0, +/- 1/30)`. Speed profile by phase: taxi (acceleration clamped to +/-1 kt/s towards `max(v0, 30 kt)` or 0, for 20 s); takeoffRoll (2.5 kt/s to rotation + 20 kt, `liftoffMs = reportMs + max(0, rotation - speed) / 2.5` s; not for helicopters); landingRoll (at least 2 kt/s down to 20 kt); airborne (the estimated acceleration for 20 s within +/-30 kt, and on approach or flare with a field and a negative rate, a touchdown time from `verticalIntegralInverse((alt0 - fieldElev - FLARE_FLOOR_FT) / (-vrate / 60))`, after which the profile holds speed to touchdown, decelerates at 2 kt/s to 20 kt, and `touchdownPitch` is the flare pitch at 0.2 of the rate). `sTurnMax` = distance covered in 60 s; a fresh autopilot heading (`navTrack`) on the same side as the turn ends the turn early (`turnEndS`). `capAlt = captureAltitude` (the selected altitude ahead of the climb or descent, from fresh measured data, when `|vrate| >= 100` and not on approach or the ground).

### 10.10 Evaluation (`evaluate`, `:1844-2032`)

Horizontal: past the newest sample, along a constant-curvature arc for up to `sTurnMax` then straight, held after 600 s; before the first, straight back at the first slope for up to 30 s; inside, the Hermite piece. The bank's turn rate is read 1 s ahead of the drawn time inside the data, and past it eases over 3 s from the path's rate into the prediction's (`k v`), fading out between 45 and 60 s, or rolling out over 4 s before `turnEndS`.

Vertical by state: ground phases sit on the field (takeoff roll: rotation pitch eased over 3 s before `liftoffMs`, then a predicted climb at 1800 fpm through the flare curve); a predicted touchdown past `touchdownMs` gives a landing roll (pitch eased down over 4 s); otherwise `airAltitude` (smoothed curve inside the altitude samples; before them straight back; past them on at the estimated rate with the curve's own rate easing in over 3 s, levelling off at `capAlt` within a band of `clamp(|vrate| * 0.15, 100, 800)` ft), with the last 2 s before a predicted touchdown easing out any height left above the flare's floor. With a field: `agl = flare(alt - fieldElev)`, rate scaled by `flareSlope`, approach becomes flare below 60 ft. Pitch from `airPitch` (true airspeed when measured), bank `atan2(speed * omega, g)` times the fade, plus the measured roll's bias while fresh, clamped to 30 degrees. Heading = track + crab (measured, eased out below 60 ft AGL, none on the ground). `drawAlt` = 0 on the ground, AGL for a predicted climb off an unknown field, else `altMsl - fieldRemoval` (field elevation faded 30..60 km from the field, else the nearer of origin and destination).

### 10.11 Blending (`drawn`, `setOffsets`, `takeOver`, `blendAround`, `:1274-1397`)

Every change (a report, measured data, a state taking over) is wrapped: evaluate what is on screen before, apply the change, evaluate after at the same drawn time, and start blending out the difference. Position and height differences beyond those already blending are split by size between the fast blend (`k = 2.2`) and the gentle one (`gentleRate`), with `w = smoothstep((|d| - 40) / 40)`; velocities are matched too (C1), scaled by the playback rate `1 - delaySlope`. More than 5 km, or previous data older than 120 s, snaps (all blends cleared). A QNH change blends the height at `min(0.8, 30 e / |d|)` per second instead. Angles (heading, track, pitch, bank) blend from the offset with zero rate at 2.2 /s.

### 10.12 Measured data (`measure`, `applyMeasured`, `measuredRates`, `measuredAttitude`, `:1141-1732`)

A newer measured position becomes the latest report (position, ground flag, altitude, speed, track; acceleration when 4..120 s since the last). Roll (clamped 30) and crab (true heading minus track, clamped 30) are kept as 30 s series and shown at the drawn time; each roll is stored as a bias against the model's own bank at that time. While fresh and airborne: turn rate from `trackRate`, else from the roll above 60 kt (`g tan(roll) / v`); vertical rate from `baroRate`, else `geomRate`; true airspeed; and, unless `navModes` contains `lnav` (case-insensitive), the selected heading converted to a true track with the variation `trueHeading - magHeading` and less the crab. The on-board altimeter setting below 3000 ft AGL teaches the field's QNH.

### 10.13 Taxi snap (`taxiSnap`, `:2054-2216`) and gear (`gear`, `:2219-2250`)

Taxi snap: per frame, for an aircraft on the ground inside a network's box. Gate target `1 - smoothstep((kt - 25) / 10)` (0 when parked unless holding an offset), eased at 0.8 /s. For each segment within 30 m: share `a = smoothstep((30 - d) / 15) * e^(-d / 15) * (0.7 now^2 + 0.3 later^2)` where `now` and `later` are the cosines between the segment and the motion now and 3 s ahead (the direction ahead followed with a 500 ms lag). The shares blend the segments' nearest points and signed directions; the blend pulls by `smoothstep((30 - |p|) / 15) * smoothstep((c - cos 35) / (cos 20 - cos 35))`. Offsets (east, north, heading) follow through `spring` with the drift across the line fed forward; applied scaled by `smoothstep(gate)`. Iteration order over networks matters when two boxes overlap ("the first that covers"): keep insertion order.

Gear: down on the ground; with a field: on approach or flare below 2000 ft AGL, on initial climb below 400 ft, or below 400 ft and slower than 180 kt; without a field: below 1500 ft, slower than 180 kt and sinking more than 200 fpm. A new track starts at its target; a change eases over 8 s with `smoothstep`.

### 10.14 Tests

`motion.test.ts`: 46 cases, 1700 lines, with `testdata/landings.json` (37 kB: 9 recorded flights with touchdowns and lift-offs at AMS, LHR, ATL, ORD, DEN, JFK; `about`, `metarQnh`, `adsbNavQnh`, `airports`, `flights[].reports = [timestampMs, lat, lon, alt, speed, track, onGround, buffer]`). The cases are listed in appendix A in the file's order, which runs from the basics (dead reckoning, turns, spline) to the recorded flights and taxi snapping. They construct `new Motion()` and pass explicit `nowMs` throughout, so they port without a clock abstraction; `syncClock offsets now()` is the only one touching the wall clock.

---

## 11. Module notes: small UI-facing logic

### 11.1 `adsb.ts` (52 lines, 2 tests)

`matchAdsb(list, key { hex, callsign, reg, lat, lon }) -> Option<AdsbAircraft>`: with a hex, the aircraft with that lower-cased hex or none. Otherwise candidates are those whose normalised callsign (upper case, only `A-Z0-9`) equals the flight's and whose registration does not contradict it (both non-empty and different), or whose registration matches; a candidate must have a position and the flight finite coordinates; the nearest wins, and only within `MAX_MATCH_KM = 150` (equirectangular, `111.195` km per degree). `verticalRate(a) = baroRate ?? geomRate`.

### 11.2 `aircraft-labels.ts` (33 lines, 8 tests)

`aircraftLabel(flight)`: trimmed callsign, else flight number, else registration. `altitudeLabel(ft, onGround)`: `GND`; from 18 000 ft `FL` + three digits of `round(ft / 100)`; below it `{ft with thousands separators} ft` (the viewer's locale, section 14). `labelPriority(sizeM, altFt, onGround) = round(600 * clamp01(sizeM / 80) + 300 * low) - 450` with `low = 1` on the ground else `1 - clamp01(altFt / 45_000)`. `SELECTED_LABEL_PRIORITY = 1000`.

### 11.3 `aircraft-readings.ts` (106 lines, 9 tests)

`degrees(v)`: three digits of `round(v)` wrapped to 0..359, then `°`. `aircraftReadings(m) -> { first, more, unreported }`:

- `first`, always four in this order, `value: None` when not reported: Heading (`degrees(trueHeading)`), IAS (`{n} kt`), Mach (three decimals), Wind (`{degrees(windDir)} at {windSpeed} kt`, wide, title "Derived from the aircraft's airspeed, heading and ground track").
- the rest, in order, with the name a sentence uses: Bank (`{|roll| to 0 decimals}° R|L`, the letter only beyond 0.5 degrees; "bank"), GNSS altitude (`ft`; "GNSS altitude"), TAS (`kt`; "TAS"), Selected alt. (MCP else FMS, `ft`, title "Set on the autopilot (or by the flight management system)"; "selected altitude"), Selected hdg (`degrees`, title "Set on the autopilot; magnetic"; "selected heading"), OAT (` °C`; "OAT"), Autopilot modes (mapped `autopilot Autopilot, vnav VNAV, althold Altitude hold, approach Approach, lnav LNAV, tcas TCAS`, unknown ones upper-cased, joined with ` · `, wide, words; "autopilot modes").
- `more` = those reported; `unreported` = the names of those not.
- `listing(names)`: `a`, `a and b`, `a, b and c`.

Doc 03 section 18.1 has the panel's use of it.

### 11.4 `hover-details.ts` (175 lines, 7 tests)

`hoverInfo(details) -> { images, logoId, progressPct }`: `logoId = painted_as_id or operated_by_id or 0`; `progressPct` only when either distance is positive. `HoverLoader` options and defaults: `dwellMs 300`, `capacity 100`, `ttlMs 10 min`, `failureTtlMs 2 min`, `cooldownMs 60 s`, injectable `now`, `fetch`, `isRateLimited`, `onchange`. Rules: hovering a flight with no fresh entry and no cooldown starts the dwell timer; one request in flight, one waiting (the latest replaces any earlier), the waiting one is fetched only if still hovered; a rate-limit error starts the cooldown and caches nothing; another error caches a failure for 2 min; the cache is an LRU by insertion order (a read re-inserts). `status(id)`: ready, unavailable (failed entry, or cooling down with nothing pending), loading. `routeProgress(position, origin, destination)`: great-circle angle flown over flown plus remaining; `None` when an end is missing or the ends are under `1e-4` rad apart.

Rust shape: a struct with `hovered`, `dwell_deadline: Option<i64>`, `in_flight`, `pending`, `cool_until`, and an insertion-ordered map; `tick(now_ms)` replaces the timer and returns a request to start; `settled(id, result, now_ms)` replaces the promise callbacks. The tests drive it with a fake clock already. Doc 03 section 10.3 describes it from the card's side.

### 11.5 `places.ts` (78 lines, 6 tests)

`KIND_ZOOM`: address 17.5, poi 16.5, street 16, district 14, postcode 13, airport 13, town 12, city 11, region 7, country 5. `fitZoom([w, s, e, n], width, height) = log2(min(width * 0.7 * 360 / (512 * lonSpan), height * 0.7 * 360 / (512 * latSpan)))` with `lonSpan` across the antimeridian when `e < w`, `latSpan = (n - s) / cos(min(|mid lat|, 85))`, both at least `1e-6`. `placeView(place, w, h)`: with a bbox, `fitZoom` clamped to `kindZoom - 2 ..= kindZoom + 1.5`; overall clamped to 2..17.5; unknown kinds use poi's. `placeDetail(place)`: `{category or kind word}` then ` · {label without the leading "name, "}` when there is more to say; kind words Address, Street, Place, Airport, District, City, Town, Postcode, Region, Country. `placeLanguage(language)`: the first two letters lower-cased if one of en, de, fr, it.

### 11.6 `screen.ts` (36 lines, 4 tests)

`screenAngle(viewport, [lon, lat, z], heading, stepM) -> Option<f64>`: projects the point and one `stepM` metres ahead on the heading (`111_320` m per degree, longitude divided by `max(0.01, cos lat)`), returns `-atan2(dx, -dy)` in degrees, `None` when the two project less than `1e-3` px apart or to non-finite values. In Rust the `Projector` trait is the camera's `project(lon, lat, z) -> (x, y)`; the sign convention (anticlockwise, deck.gl's `getAngle`) should follow whatever the icon pipeline of doc 03 section 6 settles on.

### 11.7 `trackpad.ts` (39 lines, 3 tests)

`isTrackpadScroll`, `wheelPixels` exist to tell a trackpad from a wheel inside a browser; winit reports `MouseScrollDelta::PixelDelta` against `LineDelta` and native pinch and rotate gestures, so only `wrapBearing(b) = (((b % 360) + 540) % 360) - 180` and `pinched(start, e) = { zoom: start.zoom + log2(max(0.01, scale)), bearing: wrapBearing(start.bearing - rotation) }` carry over (doc 01 sections 3.7-3.8). `wheelPixels` scale, if line deltas are still wanted: 16 px a line, the page height a page.

---

## 12. Module notes: ships and trains

Colours, icons, atlases and 3D placement are recorded in doc 02 sections 4-9 and 12. Below: the logic API and the rules those sections do not spell out.

### 12.1 `ships.ts` (124 lines, 4 tests)

```
shipKindLabel(kind), shipColor(kind) -> RGB, shipKindKey(kind) -> kind if listed else 0, SHIP_COLOR_KEYS = [0, 2, 3, 4, 5, 6, 7, 8]
shipIcon(ship) = "{dot|hull}-{colour key}"            dot when heading is null
shipIconScale(zoom) = <3: 0.7; <5: 0.85; <7: 0.92; else 1
shipIconSize(ship, zoom) = [15, 18, 22][size] (18 if unknown) * scale * (0.62 for a dot)
hullPolygon(lon, lat, dims [bow, stern, port, stbd], heading) -> 5 points: bow tip at ((stbd - port) / 2, bow),
    shoulders at bow - 0.2 * length on each side, square stern; 111_320 m per degree, lon / max(0.01, cos lat)
shipAtlas()                                           canvas drawing; see doc 02 section 4.2
```

Kinds: 0 Unknown type [140,152,168]; 2 Tug or special craft [72,198,194]; 3 Passenger [98,152,240]; 4 Cargo [116,188,112]; 5 Fishing [236,128,156]; 6 Tanker [218,92,92]; 7 Search and rescue or law enforcement [228,234,242]; 8 Pleasure craft or sailing [192,136,236]; anything else "Other" in the unknown grey.

### 12.2 `ship-list.ts` (120 lines, 14 tests)

- `ShipSort = size | name | distance`; `ShipMotion = all | moving | moored`; `SHIP_SIZES` Small, Medium, Large.
- `shipKindShort(kind)`: 2 "Tug or special", 7 "Rescue or law", 8 "Pleasure or sailing", else the full label.
- `SHIP_COVERAGE = "Ships in view from VesselFinder; zoom in to see more."`
- `ShipFilters { hiddenKinds, motion, hiddenSizes, hideOld, sort }`, default `[], all, [], false, size`.
- `mergeShipFilters(saved)`: field by field, keeping only known kind keys, sizes 0-2, known motions and sorts, a boolean `hideOld`.
- `shipFilterCount`: hidden kinds, motion not all, hidden sizes, hideOld.
- `shipName(s)`: the name unless empty or equal to the MMSI as text.
- `shipMatches(ship, query)`: empty matches; all digits: MMSI prefix or folded name contains; else every folded word of the query is in the folded `shipName`.
- `shipFilter(f)`: hidden kind key or size; `hideOld` and old; moving needs a heading; moored needs none.
- `shipLength(s) = dims[0] + dims[1]` or none.
- `sortShips(list, sort, centre)`: by name: named before unnamed, collator (numeric, base sensitivity), then MMSI. By distance: larger `cosAngle` to the centre first, then name. By size: size class descending, then known lengths before unknown, longer first, then name.

### 12.3 `ship-details.ts` (43 lines, 4 tests)

`SHIP_STALE_S = 900`. `positionAge(positionUnix, nowMs)`: `Position {ageLabel} old` (stale) past it, else `Position {ageLabel} ago`. `navTone(status)`: warn for codes 2, 6, 14. `dimensions(length, beam)`: `366 × 48 m`, `366 m`, `48 m beam`, or none (en-GB thousands separators, rounded). `draught(current, max)`: `9.3 / 15.5 m`, `9.3 m`, `max 15.5 m`, none (one decimal). `latLon(lat, lon)`: `51.9213° N, 4.4812° E` (four decimals, absolute values). `isLocode(destination)`: `^[A-Z]{2} ?[A-Z2-9]{3}$`.

### 12.4 `trains.ts` (341 lines, 23 tests)

- Kind labels and colours: highSpeed High-speed [226,92,196]; intercity Intercity [128,132,246]; regional Regional [96,212,160]; suburban Suburban [112,190,232]; other Other [158,150,140]. `TRAIN_KINDS` in that order.
- Operator colours (`:30-47`), first match on the trimmed operator, case-insensitive, 16 entries in this order: `^sj\b` [232,232,236]; `^sl\b|storstockholms` [70,146,236]; `mälartåg` [236,84,84]; `västtrafik|västtåg` [62,200,236]; `skånetrafiken|pågatåg` [196,92,222]; `öresundståg` [150,132,246]; `arlanda express` [250,212,64]; `snälltåget` [120,214,92]; `norrtåg` [58,204,176]; `^vy\b` [240,92,150]; `bergslagen` [244,152,56]; `upptåget|^ul\b` [250,120,40]; `östgöta` [214,70,130]; `x-trafik|x-tåget` [150,220,60]; `krösatåg|kronoberg|jönköping|kalmar|blekinge` [236,120,110]; `värmland` [110,180,255]. The index doubles as the atlas row `op{i}`.
- `trainColorOf(train)`: the operator's colour, else the kind's.
- `TRAIN_CREDITS.trafikverket = { "Trains in Sweden: Trafikverket, CC0", https://data.trafikverket.se/ }`; `MODEL_CREDIT = { "3D trains: Lardman", https://3dwarehouse.sketchup.com/user/1082279160696486794315506/Lardman }`; `trainCredits(trains)`: each source's credit once, plus the model credit when any train has a model.
- `positionAt(train, nowMs)`: no frames: the reported position; one frame or before the first: the first; at or after the last: the last; else linear on the segment containing `nowMs` (a zero-length time span counts as the end), longitude the short way round and re-wrapped.
- `headingAt(train, nowMs)`: with two or more frames, the initial great-circle bearing of the current leg (the last leg once past the end) when its ends differ; else the reported heading.
- `delayText(minutes)`: empty, `on time`, `{n} min late`, `{n} min early`. `delayTone`: bad from 30, warn from 5, else good, none when unknown.
- `TRAIN_STALE_S = 300`; `trainPositionAge` as the ship's. `TRAIN_ENDED_MS = 30_000`; `trainRunning(train, nowMs)`.
- `trainIcon(kind, heading, operator) = "{box|car}-{op{i} | kind | other}"`; `trainIconScale(zoom)`: <4 0.7; <6 0.82; <9 0.92; else 1; `trainIconSize = [highSpeed 24, intercity 23, regional 21, suburban 20, other 19] * scale * (0.8 for a box)`.
- `stopView(stop) -> { time, planned, late, passed }`: the departure's times, else the arrival's; `time = actual ?? estimated ?? planned`; `late = round((time - planned) / 60)` when both known and different; `passed` = the departure's actual time is set (the arrival's at the last stop). `nextStop(stops)`: the index after the last passed stop, or -1 when that is past the end.
- `trainProduct(train)`: for Trafikverket names not starting `Train `, the name without its last word, when something is left.
- `blockAt(p, blocks, was)`: `IN_BLOCK_M = 12`, `KEEP_BLOCK_M = 2`; the nearest block within 12 m (flat metres, `111_320` per degree, longitude scaled by `cos lat`), but the previous block is kept while its track is within `min(12, nearest + 2)` m.

### 12.5 `train-list.ts` (123 lines, 20 tests)

- `TrainSort = delay | speed | name | distance`; `TRAIN_COVERAGE = "Live trains are shown in Sweden, from Trafikverket."`; `DELAYED_MINUTES = 5`.
- `TrainFilters { hiddenKinds, hiddenOperators, delayedOnly, sort }`, default `[], [], false, delay`; `mergeTrainFilters` validates each field; `trainFilterCount` counts the three filters.
- `fold(s)`: NFD, strip combining marks (`\p{M}`), lower case. Shared with `ship-list`.
- `trainMatches(train, query)`: every folded word in the folded `name destination operator`.
- `isDelayed(t)`: a finite delay of 5 minutes or more.
- `trainFilter(f)`: kind not hidden, operator (`""` for unknown) not hidden, and delayed when `delayedOnly`.
- `sortTrains`: delay or speed descending with unknown last, then name (collator), then id; name; distance by `cosAngle` descending.
- `distanceKm(lat, lon, centre) = acos(clamp(cosAngle)) * 6371`; `distanceLabel(km)`: under 1 km `{round(km * 100) * 10} m`, else `{round(km) with en-GB separators} km`.
- `trainOperators(trains)`: counts per operator; unknown (`""`) last, then most trains first, then by name.

### 12.6 `train-cars.ts` (77 lines, 6 tests)

`CAR_M = 26`, `GAP_M = 0.6`, `DEFAULT_CARS = 4`. `carLines(frames, tail, cars, nowMs, lengthOf = CAR_M - GAP_M, gap = GAP_M) -> Vec<Vec<[lon, lat]>>`, front car first: the path is the tail reversed (without its first point) up to the frames' start, then the frames without repeats; the head's distance along it is found from `nowMs` by linear interpolation over frame times; each car is the stretch from `front - length` to `front` (interior path points kept), the next front `gap` behind; before the path's start cars continue straight back along its first stretch. Flat metres: `111_320` per degree, longitude scaled by the cosine of the first point's latitude. At least one car.

### 12.7 `train-route.ts` (138 lines, 12 tests)

`ON_TRACK_M = 40`, `OFF_ROUTE_M = 300`, `SEARCH_M = 20_000`. `routeAhead(route, drawn, cursor, ahead) -> { head, rest, cursor, on }`: search forward from the cursor (which is only valid for the same `route` object: use a route generation number in Rust) for the first run of legs within 40 m of the drawn train and take its nearest point, else the nearest point within 20 km of route; short of the route's start, follow the train's own `ahead` points until one is on the first leg; further than 300 m, or beside the very end, there is no route (`on = false`, ask again); otherwise `head = [drawn, nearest point, end of that leg]` and `rest` = the route after that leg (segments with fewer than 2 points dropped), reusing the cursor's `rest` while on the same leg. `wayToStop(drawn, frames, nowMs)`: the frame points ahead up to the first repeated position (a stand), empty when it does not stop within the frames or stands there already. Doc 02 section 7 has the drawing.

### 12.8 `bridges.ts` (316 lines, 10 tests)

API: `RAIL_DECK_M = 6`; `omtDeck`, `ormDeck: DeckHeight`; `bridgeParts(lines, tile, height) -> Option<{ ground, raised, under }>` (binary tile lines); `raiseFeatures(features, tile, height) -> Option<{ ground, raised }>` (GeoJSON features); `Structure { tunnel, line, joined: [bool; 2] }`; `Level { tunnel, height }`, `GROUND`; `levelAt(p, heading, structures, within = 4) -> Level`; `onStructures(line, structures) -> Vec<{ tunnel, path: [lon, lat, z] }>`. The ramp, piece length and deck rules are in doc 02 section 9; the 10 tests cover `levelAt` (5), `onStructures` (3) and the tile-line raising (2).

---

## 13. Module notes: geometry for the scene

### 13.1 `roofs.ts` (246 lines, 8 tests)

`RoofInput { ring, holes, base, height, shape, orientation, direction }` in metres east/north; `RoofParts { positions, normals, uvs, walls }` (triangle soup). `pitched(shape)`: gabled, gambrel, saltbox, round -> gabled; hipped, mansard, quadrupleSaltbox, quadruple_saltbox -> hipped; pyramidal, dome, onion -> pyramidal; skillion. `boundingRectangle(points)`: minimum-area rectangle over the convex hull's edges (Andrew's monotone chain), returning centre, unit axes and half sizes.

`roofParts`: open the ring; nothing for fewer than 3 points, height at most 0.05, a flat shape or an area under 1 m2; outline made anticlockwise, holes clockwise. Planes as heights in units of the roof's height over the rectangle (`a` half-length along the ridge, `b` half-width; swapped when `orientation == "across"`; nothing when `b < 0.05`): gabled `1 -/+ v/b`; hipped adds `a/b -/+ u/b`; pyramidal `1 -/+ v/b`, `1 -/+ u/a`; skillion one plane falling along `direction` (degrees from north; default across) from 1 at the far edge to 0. Each plane's face is the footprint clipped (Sutherland-Hodgman) to where that plane is the lowest, triangulated with earcut (holes clipped the same way), heights `base + height * max(0, plane)`, texture coordinates in metres across and up the face. Gable walls: each footprint edge is split where two planes cross, and each piece whose roof edge stands more than 0.05 m above the eaves gets two triangles with the outward normal `(dy, -dx, 0) / len` and UVs in metres. Triangulation order comes from earcut: a Rust earcut (`earcutr`) may order triangles differently, which the tests do not depend on (they check heights, areas and counts).

### 13.2 `streets-gl.ts` (222 lines, 9 tests)

`SGL_TILES = https://tiles.streets.gl/vector/{z}/{x}/{y}`, `SGL_ZOOM = 16`, `SGL_FROM_ZOOM = 15.5`. `hexColour(v)`: a non-zero number masked to 24 bits, as a colour name when it is one of 17 named values (white, grey, lightgrey, darkgrey, silver, red, maroon, brown (two values), tan, beige, yellow, gold, orange, pink, green, blue; `:43-61`), else `#rrggbb`; 0 and non-numbers are none.

`sglBuilding(p)`: a level is 4 m. `roofType` one of flat, hipped, gabled, gambrel, pyramidal, onion, dome, round, skillion, mansard, quadruple_saltbox, saltbox (else flat). `roofHeight = roofHeight tag, else roofLevels (default 0 for flat, 1 otherwise) * 4`, capped at `height - minHeight` when a height is given. `height = height tag, else (levels, else minLevel + 1, else 1) * 4 + roofHeight`. `minHeight = minHeight tag, else min(minLevel * 4, height)`; a `buildingType == "roof"` keeps only its roof (`minHeight = max(minHeight, height - max(roofHeight, 1))`). Facade and roof materials through the tables at `:85-116`. Note `some(v)` treats 0 as "not mapped" (binary tiles decode a missing number as 0) while `num(v)` keeps 0.

`sglPath(p)`: `pathType` through `KINDS` (`:171-197`: kind and default lanes; raceway 6 m, pedestrian 8 m, runway 45 m, taxiway 20 m); surface through `SURFACES`, default dirt for foot kinds and tracks, asphalt otherwise; width = the tag, else the kind's, else (foot kinds) none, else lanes (tag, else half the default rounded down but at least 1 for a oneway, else the default) times 3 m, a single lane 4 m. `sglTree(p)`: needleleaved or genus abies, picea, pinus -> fir; fagus beech; tilia linden; quercus oak; else broadleaf.

### 13.3 `radar.ts`, `radar-mesh.ts`, `radar-arena.ts`

Doc 02 section 3 has the palette, sources, contouring and drawing. The logic API to port:

```
RADAR_LEVELS_KM = [1.5, 3, 4.5, 6, 8, 10, 12]
reflectivity(r, g, b, a) -> dBZ      a == 0: -inf; a < 250: a / 255 * 10; b > g and r < 160: 10 + 25 (595 - (r + g + b)) / 420;
                                     r > 240 and b < 100: 40 + 15 (224 - g) / 224; else 60
echoTopKm(dbz) = dbz == -inf ? 0 : clamp(2 + (dbz - 10) * 0.22, 1, 12)
coveredBy(coverages) -> Option<fn(lon, lat) -> bool>     cell lookup in each mask, x and y clamped to the last cell
coveragesOver(bounds, coverages), tileBounds({x, y, z}) -> [w, s, e, n]
processTile(...), processVolume(frame, ground, coverage, levels), transferables(...)
radar-mesh: BAND_COUNT = 14, bandDbz(band) = 5 + 5 band, bandColor(band) -> RGBA, parseVolumePolygons(buffer),
            buildMesh(polygons, gridDeg = 4), polygonData(mesh, fromBand), mergeMeshes(parts), buildWalls(lines) (WALL_ALPHA 0.75),
            polygonWalls(polygons), contourBands(...), simplifyLine(p, tolerance) (WALL_TOLERANCE 0.1), evenOdd(rings), blur(values, w, h)
radar-arena: MeshArena (GROWTH 2, MIN_VERTICES 65536), ArenaLayer
```

In one process the worker pool becomes `rayon` or a small thread pool, the transferable `ArrayBuffer`s become owned `Vec`s, and `parseVolumePolygons` with the backend's `encode_shapes` can be replaced by passing the shapes themselves. `cutPolygonByGrid` (math.gl) has no direct Rust crate equivalent and must be written or avoided (it exists so that large polygons follow the globe's curvature; on a native globe renderer the same subdivision is still needed).

### 13.4 `lod-tileset.ts` (312 lines, 5 tests)

`Lod { levels (ascending zooms with tiles), bias = 0, maxDistance = inf }`. `distanceTileIndices(viewport, lod, zRange, offset)`: quadtree descent from tile 0/0/0; a tile is culled by the frustum, by the horizon on the globe (`beyondHorizon`), and by `maxDistance`; at a zoom the source has, it is taken when `z >= levelFor(offset - floor(log2(max(distance * k, 1e-12)) - bias))`, where `k = scale / 2^zoom / viewport height` and `levelFor` snaps a wanted zoom up to the next level the source has (clamped to the lowest and highest). Bounding volumes: on the globe an oriented box from 5, 9 or 11 sample points of the tile (zoom >= 2, 1, 0); flat, an axis-aligned box in 512-unit world space. `keepCoarserUntilIn(tiles)`: deck.gl's no-overlap refinement, except that an empty or failed tile keeps its ancestor visible. Look-ahead while zooming: `LOOK_AHEAD_ZOOM = 1`, `MAX_AHEAD = 32`, `AHEAD_PRIORITY = 2e8`. The port needs the native camera's frustum planes, camera position and `project_position`; the selection rule itself is 40 lines. Docs 01 and 04 use it.

### 13.5 `terrain-layer.ts`, pure part (13 tests)

```
TERRARIUM_URL = https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png   (TERRARIUM_MAX_ZOOM 15)
MAPTERHORN_URL = https://tiles.mapterhorn.com/{z}/{x}/{y}.webp
decodeTerrarium(rgba) -> heights:  r * 256 + g + b / 256 - 32768   (metres, f32)
tileRowMetersPerPixel(z, y, row, size = 256) = 2 pi 6_378_137 * cos(lat of the row's middle) / (size * 2^z)
reliefExaggeration(z) = clamp(2^((11 - z) / 3), 1, 8)
HILLSHADE_STYLE = { azimuth 315, altitude 45, zFactor 1, shadow [4, 8, 16] alpha 135, highlight [255, 244, 225] alpha 40, gain 0.55 }
hillshade(heights, w, h, metersPerPixel(row), style, out) -> RGBA
    heights below 0 count as 0; Horn's 3x3 gradient with edge pixels using the neighbours there are;
    shade = (-dzdx lx - dzdy ly + lz) / sqrt(dzdx^2 + dzdy^2 + 1), light = (sin az cos alt, cos az cos alt, sin alt);
    d = (shade - lz) / gain; d < 0: shadow colour, alpha min(1, -d) * 135; else highlight colour, alpha min(1, d) * 40
HILLSHADE_MAX_ZOOM = 12;  hillshadeFade(zoom) = 1 - 0.4 * clamp01((zoom - 11) / 3)
hillshade opacity per basemap: dark 1, satellite 0.45, realistic 0.6
groundHeight(lon, lat, onLoad) -> Option<metres>     zoom-12 Mapterhorn tile under the point, at most 16 tiles kept, never below 0
```

The alpha is written into a `Uint8ClampedArray`: round to nearest, ties to even. Image decoding must not colour-manage or premultiply (the TS goes out of its way for that); with the `image` crate that is the default.

### 13.6 The rest, in brief

These have their constants in docs 02 and 04; the functions named in table 2.2 are the pure surface to port and test.

- `rail-layer.ts`: `Backoff` (`ORM_TIMEOUT_MS 5000`, pause `15 min` after an HTTP 429 or 4 failures in a row, a cancelled request is no failure), `railWidthScale`, `speedColor` over `SPEED_BANDS`, `speedLabels(features, size, minLength = 0.06, spacing = 0.18)`.
- `rail-overview.ts`: `parseRailOverview(buffer)` reads `static/rail/railroads.bin` (format in `scripts/build-rail-overview.py`), errors on anything else.
- `road-widths.ts`: `cellOf(lon, lat)`: `stepLat = 2000 / 6_371_008.8` in degrees, `j = floor(lat / stepLat)`, `stepLon = stepLat / max(0.05, cos(mid lat))`, `i = floor(lon / stepLon)`, key `"{i},{j}"`, centre the cell's middle; must stay identical to `RS:roads.rs:217`.
- `landcover.ts`: class table `LC`, `wooded(c)`, `classAt(grid, x, y)`, colour-to-class (`nmdClasses`) and code-to-class (`worldCoverClasses`) maps; tile size 256.
- `canopy.ts`: `quadkey(x, y, z)`, `placeInFile(tile)` (zoom-10 files, at least 512 px read), `heightAt`, `tallestNear`. Reading the cloud-optimised GeoTIFFs by range request needs a Rust COG reader.
- `overture.ts`: `inside(point, ring)` even-odd, `tallBuildings(features)`; needs a PMTiles reader; `KNOWN_RELEASE = "2026-09-23.1"`, zoom 14.
- `street-trees.ts`: `parseTown`, `treesInTile`; kinds broadleaf 0, conifer 1, beech 2, linden 3, oak 4.
- `tree-layer.ts`: `hash2(x, y, salt)` (integer hash, section 14), `leafPhase(date, lat)` (tropics always summer; day of year shifted half a year in the south; leaf-out day `118 + shift`, turn `262 - shift`, fall `300 - 0.8 shift`, `shift = (|lat| - 56) * 4`), `plantTile` (masks of 512 cells, at most 24 000 trees a tile), `tilesAround`, `treeShadows`.
- `realistic.ts`, `realistic-ground.ts`: `transmittance`, `lookFor`, `displayRgb`, `skyCss` (becomes a gradient description, not CSS), `sceneSun`, `fromLocal`, `roofColour`, `buildingStyle`, `areaPaint`, `lineOf`; 15 tests.
- `ship-models.ts`, `train-models.ts`, `models.ts`: procedural meshes and placement; doc 02 sections 4.6 and 5.6, doc 03 section 4.5.
- `liveries.ts`: `DEFAULT_LIVERY`, curated airlines, aliases, the military set (`RCH RRR GAF IAM BAF CTM CFC HKY ASY PAT CNV NAF`); doc 03 section 17.
- `demo-trains.ts`: dev only; drop unless a demo mode is wanted.

---

## 14. JavaScript semantics that change the answer in Rust

Put the helpers in `core::num` and `core::text` and use them wherever the TS used the JS built-in, so that the ported tests pass for the right reason.

| JavaScript | Behaviour | Rust trap | Do this | Where it bites |
|---|---|---|---|---|
| `Math.round(x)` | half rounds towards +infinity (`-2.5 -> -2`) | `f64::round` rounds half away from zero (`-3`) | `js_round(x) = (x + 0.5).floor()` | `delayMinutes`, `stopView.late`, `altitudeColor`, `labelPriority`, CSV export, `mergeTrail` speed |
| `Math.sign(0)` | `0` | `f64::signum(0.0)` is `1.0` | `sign0(x)` returning 0 for 0 | `motion.ts:1827` (turn side against `navTrack`), `seriesSlope`, `building-shadow` |
| `x.toFixed(n)` | ties round up in magnitude (`2.5 -> "3"`, `0.5 -> "1"`) | `format!("{:.n}")` rounds ties to even (`"2"`, `"0"`) | `to_fixed(x, n)` that adds the tie rule | bank reading, `latLon`, `draught`, export `coord` |
| `Number(x.toFixed(6))` then to string | shortest form, `-0` prints as `0` | `{:.6}` keeps trailing zeros | format, parse, print with `{}`; map `-0.0` to `0.0` | export coordinates |
| `JSON.stringify` of numbers | `10668` for a whole float | `serde_json` writes `10668.0` | serialise whole values as integers, or a custom serializer | GeoJSON export, keys file, settings file |
| `%` | sign of the dividend | same as Rust's `%` | copy the `((x % n) + n) % n` forms verbatim; do not "simplify" half of one | every wrap function |
| `x | 0`, `Math.imul`, `>>>` | 32-bit wrapping | `as i32` saturates on floats | `(x as i64) as i32`, `wrapping_mul`, shift on `u32` | `tree-layer.ts` `hash2` |
| comparisons with `NaN`, `undefined` | always false; `!(x > 0)` is true for NaN | same for `f64`, but indexing an empty `Vec` panics where JS read `undefined` | keep the negated forms; guard reads | `motion.ts:2280` `S.reportMs >= tr.ats[0]` with no altitude samples; `nearestSample` returning -1; `Math.min(...[])` giving `Infinity` in `blockAt` |
| `x === x` | NaN test | - | `!x.is_nan()` | `motion.ts:2313` (`capAlt`) |
| `-Infinity` / `NaN` sentinels | "never" and "unknown" | - | keep `f64` sentinels in `motion` first; convert to `Option` only after the tests pass | all of `Track` |
| `Map` iteration | insertion order | `HashMap` is unordered | `IndexMap`, or `Vec` plus linear search for small sets | `HoverLoader` LRU, `groundTiles`, taxiway networks ("first that covers"), `qnhs` (order affects only float summation) |
| object identity as key or test | `Map<Airport, …>`, `tr.field === tr.destAp`, `net === tr.snapNet`, `cursor.route === route` | no identity on values | airport index into the reference list; `Arc::ptr_eq` or ids for networks; a generation number for routes | `motion`, `train-route` |
| `toLocaleString()` without a locale | the viewer's locale's grouping and decimals | none in std | decide once (open question 4); tests assume an English locale | `altitudeLabel`, `aircraft-readings` `number` |
| `toLocaleString("en-GB")` | `1,240` | none in std | a `group_thousands` helper | `dimensions`, `distanceLabel` |
| `Intl.DateTimeFormat` | tz database plus CLDR names | none in std | section 6.3 | `time.ts` |
| `Intl.Collator(undefined, { numeric: true, sensitivity: "base" })` | natural order, case and accents ignored, locale tailoring | `str::cmp` is by code point | `icu_collator` with the same options, or a natural compare over `fold`ed strings | `sortShips`, `sortTrains` |
| `a.localeCompare(b)` | locale collation | - | same collator, default options | train ids, operator names |
| `normalize("NFD")` + `\p{M}` | accent stripping | none in std | `unicode-normalization` | `fold` |
| regex `\b`, `\d`, `\w` | ASCII | Unicode-aware in the `regex` crate | `(?-u:\b)`, `[0-9]` | operator colours (`^sj\b`, `^sl\b`, `^vy\b`, `^ul\b`: in JS a following `ö` or `å` counts as a boundary, in Unicode mode it does not); `shipMatches` `^\d+$`; `airlineIcao` |
| regex `i` flag with `å ä ö` | matches both cases | fine with `(?i)` in Unicode mode | keep Unicode on for the letters, ASCII only for `\b` | operator colours |
| `Uint8ClampedArray[i] = x` | clamp, round ties to even | `as u8` truncates | `x.clamp(0.0, 255.0).round_ties_even() as u8` | `hillshade` |
| `Float32Array` | stores `f32` | computing in `f64` and comparing exactly | use `f32` buffers where the TS did | `decodeTerrarium`, shadow meshes |
| `Array.prototype.sort` | stable | `sort_by` is stable, `sort_unstable_by` is not | use `sort_by`; comparators must be total (`partial_cmp().unwrap_or(Equal)` mirrors a `0` result) | every sort |
| `reduce` without a seed | first element seeds, ties keep the earlier | `min_by` returns the *last* of equal minima in some forms | write the loop | `chooseArrivalRunway` |
| string `slice`, `charAt`, `padStart`, `length` | UTF-16 units | byte offsets | only applied to ASCII here (`language.slice(0, 2)`, digits); assert or use `chars()` | `placeLanguage`, `signInNote`, `degrees` |
| module-level mutable scratch | shared results (`projX`, `hx`, `profS`, `amAlt`, `springV`) | global `static mut` | return tuples or small structs | `motion`, `taxinet` |
| `setTimeout`, promises | dwell timers, retry after 60 s | no event loop | deadlines checked on a tick with `now_ms`; tasks send results over channels | `HoverLoader`, `reference.runways`, `taxiways` |
| `Date.now()` defaults | hidden clock | - | always pass `now_ms` | `dayAt`, `airacCycle`, `Motion`, `Backoff` |

---

## 15. Test port plan

- Framework: plain `#[test]` functions. Keep the TS test names as the Rust function names in snake case (appendix A is the list); one TS `test(...)` becomes one `#[test]`.
- Assertions: `toBeCloseTo(v, digits)` means `|a - v| < 0.5 * 10^-digits` (default 2 digits): write an `assert_close!(a, v, digits)` macro with exactly that rule. `toEqual` on arrays of floats is exact equality.
- Fixtures: `testdata/landings.json` moves to `crates/core/tests/data/landings.json` and is read with `serde_json` (`include_str!`).
- Fakes: `HoverLoader` and `Backoff` take a clock; pass a `Cell<i64>`. `filters.test.ts`'s module mock disappears (`airline_icao` is a pure function; the test that needs "no airline" passes callsigns without the pattern, or `matcher` takes the lookup as a parameter).
- Canvas: `shipAtlas()` and `trainAtlas()` return `null` under bun and are untested; their Rust replacements need new tests (mapping keys and cell rectangles).
- Golden cross-check, recommended before the TypeScript is deleted: a bun script that runs `Motion` over each recorded flight in `landings.json` (and a few synthetic ones) at a fixed frame step and writes every `Pose` to JSON, and the same for `estimateRoute` on fixed inputs. The Rust ports must reproduce those to about `1e-6` (positions in degrees to `1e-9`). The 46 behavioural tests bound the result; the golden files catch a transcription slip in 3400 lines that still stays inside those bounds. Once generated the files are plain data and survive the removal of the TS.
- Existing Rust tests (181 `#[test]`s in `src-tauri/src`, 16 in `crates/fr24`) move with their modules unchanged.

Per-file counts (bun): adsb 2, aircraft-labels 8, aircraft-readings 9, bridges 10, building-shadow 10, export 7, filters 22, freshness 13, hover-details 7, keys 3, ktx2-transcode 4, landcover 6, lod-tileset 5, motion 46, overture 2, places 6, radar-arena 6, rail-layer 11, rail-overview 2, realistic 15, road-widths 3, roofs 8, routes 15, screen 4, shadow 13, ship-details 4, ship-list 14, ship-models 16 (7 written, the rest generated per ship class), ships 4, signin-values 7, streets-gl 9, terrain-layer 13, time 13, trackpad 3, train-cars 6, train-list 20, train-models 3, train-route 12, trains 23, tree-layer 10. Total 394.

Modules with logic and no tests today (write tests while porting): `geo.ts` (`altitudeColor`, `viewArea`, `visibleRadius`, `effectiveExaggeration`; only `cosAngle` is exercised, through the list sorts), `daynight.ts` `nightPolygons` (`subsolarPoint` is used by `shadow.test.ts`), `taxinet.ts` (through three motion tests only), `taxiways.ts`, `reference.svelte.ts`, `weather.ts` `mixConditions`, all of `routes.ts` (`decodeWaypoints`, `mergeTrail`, `routeFor`, `airacCycle`: `routes.test.ts` tests `flightpath.ts` only), `liveries.ts`, `models.ts`, `radar.ts` `reflectivity` and `echoTopKm`, `landcover-ground.ts`, `controller.ts`.

---

## 16. Open questions and risks

1. **Crate boundaries.** The `core` / `backend` / `scene` / `app` split in section 3 is this document's proposal; the mapping table needs re-prefixing if the architecture differs. The one hard requirement from the logic side is that `core` and `scene` never read a clock or do I/O.
2. **Existing users' settings.** Settings, filters and (in a browser) keys live in the web view's localStorage under `rustradar.v1`. A native app cannot read that store portably, so settings reset on upgrade unless the last Tauri release writes them to a file first, or a one-off reader for each platform's web view storage is written. Keys are already in `keys.json` and carry over. Decision needed from the owner.
3. **Zone abbreviations (`time.ts`).** Behaviour is defined by Chromium's CLDR data across 13 locales. Options: a generated table checked in (recommended, section 6.3), ICU4X time-zone names (large data, and its answers must still be filtered through the same 13-locale rule), or the tz database's own abbreviations plus a deny list (cheapest, but will differ from today for some zones). The tests cover a few dozen zones, not all of them.
4. **Locale.** Today number grouping (`altitudeLabel`, readings), the weekday and month in `dayAt`, and name collation follow the viewer's OS locale while every string in the UI is English. Either fix all of these to English (simplest, deterministic, matches the tests) or carry a locale through `core`. Fixing them changes what a non-English user sees in three small places.
5. **`motion.ts` fidelity.** 3400 lines of numerics with float sentinels, shared scratch and order-dependent state. Plan: transliterate first (same names, same structure, `f64` sentinels), port all 46 tests and the golden files, only then refactor to idiomatic Rust. Any "improvement" during transliteration is a risk without a way to see it.
6. **Performance of the motion model.** JS runs it for thousands of aircraft per frame with a 600-report ingest budget. Rust will be faster, and tracks are independent, so ingestion could move off the render thread later; not needed for parity.
7. **Triangulation and contour libraries.** `roofs.ts` and `radar-mesh.ts` use math.gl's `earcut` and `cutPolygonByGrid`; `landcover-ground.ts` uses `d3-contour`. `earcutr` and the `contour` crate (already a backend dependency) are close but not identical in output order or tie handling; tests compare areas and counts, so this is a visual-parity risk rather than a test risk. `cutPolygonByGrid` has to be written.
8. **Remote file formats.** `canopy.ts` (cloud-optimised GeoTIFF by range request), `overture.ts` (PMTiles), `ktx2` (Basis Universal transcoding) need Rust crates chosen; the logic around them is small.
9. **Dead and test-only exports.** `geo.extrapolate`, `flightpath.dubins`, `flightpath.turnThenStraight` are only used by tests. Port as `pub(crate)` test helpers or drop with their tests; say which.
10. **Small behavioural differences to settle when the duplicates merge.** `RS:weather.rs` `parse` gives no weather without `cloud_area_fraction` where the TS fallback read it as 0 (keep Rust's); `readKeysFile` stores untrimmed values that `keys.rs` trims on save (harmless); `RS:lib.rs` keeps buffer points up to 60 s while `motion.ts` uses only those up to 30 s (keep both, or cut the backend to 30 s).
11. **`isRateLimited`** matches error text (`status 8|too many requests`). With no IPC boundary the hover loader and the feed can match on the `fr24` error type instead; keep the text match until the error types are plumbed through.
12. **`trackpad.ts`** is mostly obsolete on winit; confirm that winit's pixel deltas and pinch and rotate gestures behave the same on all five platforms before deleting the heuristics (doc 01 section 3.7).

---

## Appendix A. Every test case, by file

Names as written in the test files; a prefix in square brackets is the `describe` group. Counts are the names found statically (385); bun runs 394 because `ship-models.test.ts` generates cases in loops.

### `adsb.test.ts` (2 cases)

1. matches by address first
2. without an address, by callsign or registration near the flight

### `aircraft-labels.test.ts` (8 cases)

1. [aircraftLabel] prefers the callsign, then the flight number, then the registration
2. [aircraftLabel] is empty when there is nothing to name it by
3. [altitudeLabel] flight levels from 18,000 ft
4. [altitudeLabel] feet below that, with thousands separators
5. [altitudeLabel] GND on the ground
6. [labelPriority] larger aircraft win over smaller ones at the same height
7. [labelPriority] lower aircraft win over higher ones of the same size
8. [labelPriority] stays in the collision filter's range, under the selected aircraft

### `aircraft-readings.test.ts` (9 cases)

1. [aircraftReadings] puts heading, IAS, Mach and wind first, always, in that order
2. [aircraftReadings] formats the first values in aviation notation
3. [aircraftReadings] marks a first value the aircraft doesn't report as null, keeping its place
4. [aircraftReadings] puts every other reported value behind More, nothing unreported
5. [aircraftReadings] leaves unreported values out of More and names them instead
6. [aircraftReadings] falls back to the FMS altitude when the panel's isn't reported
7. [aircraftReadings] with nothing beyond the basics, More is empty
8. [degrees] pads to three digits and wraps at 360
9. [listing] joins with commas and a final and

### `bridges.test.ts` (10 cases)

1. [levelAt] on the ground off them
2. [levelAt] up on a bridge, rising from its ends
3. [levelAt] an end where the bridge goes on stays up
4. [levelAt] in a tunnel
5. [levelAt] not a bridge it passes under, nor a track beside it
6. [onStructures] cut where it goes into and out of a tunnel
7. [onStructures] up along the bridge, in pieces
8. [onStructures] nothing to cut without structures
9. [raiseFeatures] bridges up, the rest left on the ground
10. [raiseFeatures] none without bridges

### `building-shadow.test.ts` (10 cases)

1. [shadowStep] is height / tan(altitude) away from the sun
2. [shadowStep] none at night or with the sun on the horizon
3. [shadowStep] caps the length at a low sun
4. [shadowStep] straight down with the sun overhead
5. [buildingShadows] a box's shadow, either winding, open or closed rings
6. [buildingShadows] a box under a sun from the south-east: walls facing away only
7. [buildingShadows] an L-shape's shadow is exact, including the inner corner
8. [buildingShadows] a courtyard's walls shade the yard
9. [buildingShadows] buildings of different heights in one call
10. [buildingShadows] nothing with no height, or with the sun overhead

### `export.test.ts` (7 cases)

1. points are in time order and timeless points are dropped
2. titles and file names say which flight it is
3. KML is a timed, extruded track in metres
4. GPX has one track point per position, with height and time
5. CSV has a header and a row per position
6. GeoJSON is a line with a time per vertex
7. names with markup are escaped in XML

### `filters.test.ts` (22 cases)

1. [mergeSettings] settings saved before rail map and terrain existed take their defaults
2. [mergeSettings] stored choices for the newer settings are kept
3. [mergeSettings] nothing saved, or something unreadable, gives the defaults
4. [mergeSettings] unknown keys and values of the wrong type are dropped
5. [mergeSettings] the old signal-blocks switch becomes the rail map showing the blocks
6. [mergeSettings] a setting since removed (airline liveries, now always on) is dropped
7. [mergeSettings] a mode that isn't one of the three falls back to aircraft
8. [mergeSettings] the defaults are not shared with the result
9. [the old aircraft, ships and trains switches become the mode] they are dropped from the settings
10. [the old aircraft, ships and trains switches become the mode] aircraft on (or never saved) starts in aircraft mode
11. [the old aircraft, ships and trains switches become the mode] aircraft off starts on ships, or on trains with the ships off too
12. [the old aircraft, ships and trains switches become the mode] everything off starts on aircraft
13. [the old aircraft, ships and trains switches become the mode] a stored mode wins over the old switches
14. [alsoShow] defaults to nothing, also for saves from before it or before the modes
15. [alsoShow] a stored list of kinds is kept
16. [alsoShow] unknown kinds and duplicates are dropped
17. [alsoShow] anything but an array becomes none
18. [alsoShow] the result doesn't share the default list
19. [alsoShow] an unknown mode with every old switch off still lands on a valid mode
20. [shownKinds] the focus alone
21. [shownKinds] the focus and the kinds shown with it
22. [shownKinds] the focus listed in alsoShow too changes nothing

### `freshness.test.ts` (13 cases)

1. [staleAfter] is two refresh intervals
2. [staleAfter] never drops below the minimum
3. [ageLabel] counts seconds under a minute
4. [ageLabel] counts whole minutes past a minute
5. [ageLabel] counts hours and minutes past an hour
6. [ageLabel] never goes negative
7. [feedFreshness] stays quiet within two refresh intervals
8. [feedFreshness] says how old the data is once past them
9. [feedFreshness] measures against the real interval, backoff included
10. [feedFreshness] a regional feed still gets the minimum's grace
11. [afterRound] a snapshot that arrives restarts the clock on its own schedule
12. [afterRound] a failed round changes nothing
13. [afterRound] failures after a success never turn stale data quiet again

### `hover-details.test.ts` (7 cases)

1. fetches only after the pointer rests
2. caches answers and never asks twice
3. one request at a time, and stale waiting flights are dropped
4. the waiting flight is fetched next if still hovered
5. rate limiting pauses all requests
6. a failure is remembered for a while
7. route progress from great-circle distances

### `keys.test.ts` (3 cases)

1. [keys file] leaves the login out unless asked
2. [keys file] round-trips over what's there, keeping what the file lacks
3. [keys file] rejects other files

### `ktx2-transcode.test.ts` (4 cases)

1. a texture array: every layer, every mip level down to 1×1
2. BC7 and ASTC: 16 bytes a 4×4 block
3. the aircraft icons keep their alpha
4. not a KTX2 file (Tauri's page for a missing one)

### `landcover.test.ts` (6 cases)

1. NMD's colours back to their classes: pine, birch, a mire, a field; clear and unknown are none
2. WorldCover's codes, its grey
3. a class at a place in its tile
4. the canopy's files by quadkey: Lund's
5. a town's register file, back to its trees in a tile
6. land cover traced into rounded outlines: a wood and a field, towns and water left out

### `lod-tileset.test.ts` (5 cases)

1. [tiles by their distance from the camera] a tilted view: fine near the camera, coarser off, and only the zooms there are
2. [tiles by their distance from the camera] straight down: the same everywhere; further up: coarser
3. [tiles by their distance from the camera] nothing past the farthest
4. [loading ahead while zooming] zooming in asks for the tiles of a level further in, besides the view's
5. [loading ahead while zooming] standing still asks for nothing more

### `motion.test.ts` (46 cases)

1. straight flight matches dead reckoning, a few seconds in the past
2. a constant-rate turn continues along its circle
3. the smoothed path stays on exact samples
4. a new report blends in without a jump and converges
5. an old report still moves
6. taxiing aircraft move; parked ones don't
7. an approach to a high field touches down and rolls out
8. phases through a takeoff
9. phases follow the drawn time, not the newest report
10. a takeoff roll lifts off by itself
11. phases through a landing
12. bank follows the turn; pitch follows the climb
13. gear animates when the target changes
14. syncClock offsets now()
15. trail heights match the model's
16. prune drops stale tracks
17. measured roll drives the bank smoothly
18. a measured true heading off the track shows the crab
19. a climb levels off at the selected altitude
20. a turn on the autopilot stops at the selected heading
21. a stale measured source falls back to the estimates smoothly
22. measured attitude is shown at the drawn time, not on arrival
23. interleaved Flightradar24 and measured samples stay continuous
24. noisy positions are smoothed: no sample-rate wiggle, close to the truth
25. a new sample causes no step in position or velocity
26. the delay follows the data's cadence, within 2-12 s, and eases
27. the drawn time stays about one sample spacing or more behind the newest data
28. the buffer is history: a braking roll is drawn slowing down, an accelerating one speeding up
29. the bank rolls into a turn from the data ahead, before the heading swings
30. a turn's bank, whenever the reports come: level before it, never the wrong way, at the physical bank in it
31. the wings level as the turn ends, as they rolled in as it began
32. the bank doesn't wobble with noisy positions
33. the buffer is history: it tells which way a turn goes
34. stale data never freezes, and fresh data is caught up smoothly
35. real touchdowns and lift-offs are smooth: no jump at the ground flag, gear down, never below ground
36. each runway tells its field's QNH, which corrects the next arrival
37. a pressure altitude 400 ft off either way: down on time once the field's QNH is known
38. a pressure altitude 400 ft off at a field whose QNH isn't known yet: no jump, down by the ground flag
39. sparse reports on short final: the descent carries on into a flare, not a drop at the ground report
40. a take-off: on the ground through the airborne-flagged roll, then off it smoothly from 0
41. no altitude on short final (a negative pressure altitude, sent as 0): the descent carries on
42. the selected aircraft's altimeter setting (adsb.lol nav_qnh) corrects it and its field
43. a measured crab is kicked out in the flare and gone on the runway
44. a taxiing aircraft is drawn on the taxiway centreline, turning with it
45. taxiway snapping leaves alone what isn't taxiing along a line
46. a taxiing aircraft near a taxiway is pulled onto it smoothly, and forgetting the network lets it go smoothly

### `overture.test.ts` (2 cases)

1. a point in a building's outline
2. only the buildings with a height or floors are kept, each part of a multipolygon

### `places.test.ts` (6 cases)

1. [placeView] uses the kind's zoom without an extent
2. [placeView] fits an extent, within reach of the kind's zoom
3. [placeView] a fitted extent is on screen on the globe
4. [placeView] handles extents across the antimeridian
5. [placeDetail] says what it is, then where
6. [placeLanguage] asks for names Photon has, else local ones

### `radar-arena.test.ts` (6 cases)

1. [MeshArena] draws the same triangles, in the same order, as merging the tiles
2. [MeshArena] a tile coming into view is appended, one leaving collapsed, and the rest left alone
3. [MeshArena] nothing changes for the same tiles
4. [MeshArena] walls keep their height and are packed afresh once the buffers are full
5. [MeshArena] bands below `fromBand` are left out
6. [MeshArena] an empty view frees the buffers

### `rail-layer.test.ts` (11 cases)

1. rail features by kind
2. roads, paths and other layers are not rail
3. mainlines are the brightest and widest, service track the dimmest and thinnest
4. tunnels are fainter and dashed; the same colour object each time
5. lines thicken as the view closes in, within bounds
6. stations and halts, not tram or subway stops
7. layers by zoom: the network from 8, stations from 12, OpenRailwayMap only when asked, from 10
8. OpenRailwayMap backoff: a 429 pauses at once, other failures after a run of them
9. [speed limits] each limit gets its band's colour; unmapped and other railways their own
10. [speed limits] one label per stretch: a double track says its limit once, sidings not at all
11. [speed limits] the tiles come through the app's protocol

### `rail-overview.test.ts` (2 cases)

1. the shipped overview parses, and covers Sweden
2. anything that isn't the file is refused

### `realistic.test.ts` (15 cases)

1. [realistic map: light] the low sun comes through more air: dimmer and redder
2. [realistic map: light] sun by day in fine weather, none under cloud or at night
3. [realistic map: light] clear air leaves the near distance clear; fog closes in beyond the middle of the view
4. [realistic map: light] a sunny day is taken darker than a grey one, so it isn't washed out; the night brighter
5. [realistic map: light] the screen's colours come through the same curve as the shaders'
6. [realistic map: light] the sky behind the map: its horizon where the tilt puts it, the sun's glow when in view
7. [realistic map: light] without day and night, a fine afternoon; a local direction turns into the Earth's and back
8. [realistic map: buildings] each building its own façade and roof, the same each time
9. [realistic map: buildings] a mapped colour is kept, toned down
10. [realistic map: weather] clear sky: sun; overcast: none; broken cloud: some, and the clouds' shadows
11. [realistic map: weather] rain wets, fog closes in, snow lies
12. [realistic map: weather] rain falls (more in a downpour), and veils the distance; fog more
13. [realistic map: the ground] each kind of area has its own surface; names of places none
14. [realistic map: the ground] roads at their widths with pavements, railways on ballast, tunnels not at all
15. [realistic map: the ground] OpenRailwayMap's tracks on ballast, but not in tunnels, out of use, or trams'; its platforms paved

### `road-widths.test.ts` (3 cases)

1. a road takes the stretch it is on and runs along
2. a tile's roads carry their stretches' widths and surfaces; its paths don't
3. the backend's squares, the same as roads.rs's

### `roofs.test.ts` (8 cases)

1. [roofParts] a gabled roof: the ridge along the longer side, gables at the ends
2. [roofParts] mapped across: the ridge across the longer side
3. [roofParts] hipped and pyramidal: no gables, the whole footprint covered
4. [roofParts] a skillion: down the way it faces, a wall under its high edge
5. [roofParts] an L-shaped house: covered whole, gables where its roof stands over the eaves
6. [roofParts] round a courtyard: the yard left open, walls under the roof facing into it
7. [roofParts] flat roofs and degenerate rings: nothing
8. the least rectangle of a turned box

### `routes.test.ts` (15 cases)

1. runway ends: both directions, displaced thresholds, true headings
2. runway choice: the one the aircraft is lined up with
3. runway choice: the direction other traffic is landing
4. runway choice without evidence: a guess, the end reached most directly
5. final approach: on the extended centreline, then the threshold and the aiming point
6. final approach: heights follow the 3° glide path to the touchdown zone
7. an aircraft below the glide path flies level until it meets it
8. the path leaves along the aircraft's track and turns smoothly
9. Dubins paths arrive at the pose asked for without kinks
10. far away: the en-route part is the great circle, only the ends are shaped
11. far away and turning: joins the great circle smoothly from the current track
12. departure: rolls on the runway, climbs out straight ahead, then turns onto the first tracked track
13. trimming starts the path at the aircraft as it moves along it
14. a whole estimate names its runways and stays an estimate
15. an aircraft already over the runway rolls out along it rather than going round to land again

### `screen.test.ts` (4 cases)

1. [screenAngle] matches bearing - heading at the centre of a flat, untilted view
2. [screenAngle] east points right and north points up at the globe's centre
3. [screenAngle] follows the meridians' lean away from the globe's centre
4. [screenAngle] has no direction for a zero step

### `shadow.test.ts` (13 cases)

1. [shadowMatrix] projects the oriented, scaled mesh onto the ground along the sun's rays
2. [shadowMatrix] with the sun overhead the shadow is the plan view, straight below
3. [shadowMatrix] a point at the origin lands height / tan(sun altitude) away from the sun
4. [shadowMatrix] lands on a ground plane below the map, meeting the wheels there
5. [shadowMatrix] writes into the array it is given
6. [localSun] is a unit vector
7. [localSun] puts the sun overhead at the subsolar point
8. [localSun] puts the sun on the horizon a quarter turn round the equator, and below it opposite
9. [localSun] points east of a place in its morning and north of it beyond the tropic
10. [shadowOpacity] is darkest on the ground
11. [shadowOpacity] fades with height and is gone at cruise
12. [shadowOpacity] small aircraft lose their shadow lower
13. [shadowOpacity] none at night, faint with the sun on the horizon

### `ship-details.test.ts` (4 cases)

1. [positionAge] recent reports read as an age
2. [positionAge] past a quarter of an hour, old
3. dimensions and draught leave out what isn't known
4. positions, statuses and destinations

### `ship-list.test.ts` (14 cases)

1. [shipMatches] by name, any case, every word
2. [shipMatches] by MMSI, from its start
3. [shipMatches] a ship with no name of its own is found by MMSI, not by its MMSI-as-name in a word search
4. [shipMatches] an empty search matches everything
5. [shipFilter] the defaults show every ship
6. [shipFilter] hidden types, the unknown group taking unlisted types
7. [shipFilter] moving has a heading; moored is the rest
8. [shipFilter] sizes and old positions
9. [shipFilter] each group counts once for the badge
10. [sortShips] by size: class first, then measured length, known before unknown
11. [sortShips] by name: unnamed ships last
12. [sortShips] by distance from the map's centre
13. [mergeShipFilters] nothing stored gives the defaults
14. [mergeShipFilters] stored choices are kept; unknown values dropped

### `ship-models.test.ts` (7 cases)

1. [ship models] ${cls}: whole triangles, unit normals, inside the unit box
2. [ship models] the hull's normals face outward: the bow's sides lean forward, the deck up
3. [shipClassOf] by type colour and length
4. [shipPlacement] true length and beam, centred between bow and stern, port and starboard
5. [shipPlacement] a small boat keeps its proportions
6. [shipPlacement] without dimensions: a typical size, centred
7. [shipPlacement] offsets turn with the heading

### `ships.test.ts` (4 cases)

1. [hullPolygon] points the bow along the heading, at true length and beam
2. [hullPolygon] longitude stretches with latitude
3. [type colours and icons] known and unknown types
4. [type colours and icons] bigger ships get bigger icons, moored ones a small dot

### `signin-values.test.ts` (7 cases)

1. [missingWithoutSignIn] lists all three when signed out with no other source
2. [missingWithoutSignIn] lists nothing when signed in, even with values absent
3. [missingWithoutSignIn] leaves out values another source supplies
4. [missingWithoutSignIn] lists nothing when every value has a source
5. [signInNote] counts all three
6. [signInNote] names one or two, so the reader knows which
7. [signInNote] says nothing when no value needs it

### `streets-gl.test.ts` (9 cases)

1. [sglBuilding] levels of 4 m, a pitched roof a level more
2. [sglBuilding] a mapped height holds its roof
3. [sglBuilding] numbers a binary tile fills with 0 are not mapped
4. [sglBuilding] a roof on posts is only its roof
5. [sglBuilding] materials and colours as mapped
6. [sglPath] roads as wide as their lanes
7. [sglPath] paths, surfaces and mapped widths
8. tree kinds
9. named colours back to their names

### `terrain-layer.test.ts` (13 cases)

1. terrarium pixels decode to metres
2. decoding reuses a buffer that is big enough
3. a tile's pixels are about 611 m at the equator at zoom 8 and shrink towards the poles
4. flat ground is fully transparent
5. a slope facing the north-west light is lit, one facing away is shaded
6. steeper slopes shade darker, up to the cap
7. edge pixels shade like the middle, so tiles meet without a seam
8. the sea floor isn't shaded
9. zoomed out, relief is exaggerated more
10. the hillshade at every zoom
11. the 3D surface is deck.gl's terrain, never below the sea
12. the hillshade is fainter over the imagery
13. close in, the hillshade fades back but never out

### `time.test.ts` (13 cases)

1. clocks are 24-hour
2. a missing time is a dash
3. minutes of the day agree with the clock
4. zones are named by their abbreviation, not an offset
5. a real name beats a locale's plain GMT at zero offset
6. UTC stays UTC
7. a zone without an abbreviation in common use falls back to its UTC offset
8. hubs that no locale names get their common English name
9. the Crown Dependencies keep London's names
10. BST is London's alone: Dhaka keeps its offset
11. old zone names are named like their current ones
12. clock with zone
13. an unknown zone falls back to the viewer's own rather than failing

### `trackpad.test.ts` (3 cases)

1. [trackpad] a two-finger scroll is told from a mouse wheel's notch
2. [trackpad] wheel movement in pixels
3. [trackpad] pinching apart zooms in, twisting turns the map

### `train-cars.test.ts` (6 cases)

1. [carLines] its cars back from where it is drawn now, front first
2. [carLines] standing where its frames start, its cars lie along the track behind
3. [carLines] without the track behind, straight on back
4. [carLines] cars of their own lengths, the gap between each
5. [carLines] models' cars end to end: their lengths take in the gap
6. [carLines] no frames, no cars

### `train-list.test.ts` (20 cases)

1. [trainMatches] an empty search matches everything
2. [trainMatches] by name or number, destination and operator, any case
3. [trainMatches] every word has to match, in any field
4. [trainMatches] accents don't matter
5. [trainMatches] a train without destination or operator still matches by name
6. [trainFilter] the defaults show every train
7. [trainFilter] hidden kinds are left out
8. [trainFilter] delayed only keeps trains 5 min or more late, not unknown delays
9. [trainFilter] hidden operators are left out, those not known as \"\"
10. [trainFilter] operators are listed most trains first, unknown last
11. [trainFilter] each group counts once for the badge
12. [sortTrains] by delay: most late first, early after on time, unknown last
13. [sortTrains] by speed: fastest first, unknown last
14. [sortTrains] by name: numbers in natural order
15. [sortTrains] by distance from the map's centre: nearest first
16. [sortTrains] the list passed in is left as it was
17. [distances] Helsinki to Tampere is about 160 km
18. [distances] labels
19. [mergeTrainFilters] nothing stored gives the defaults, not shared
20. [mergeTrainFilters] stored choices are kept; unknown kinds and sorts, and the old regions, are dropped

### `train-models.test.ts` (3 cases)

1. [train models] a car is a closed unit box of triangles, its body taking the train's colour
2. [train models] the shapes differ where their vehicles do
3. [train models] each car lies halfway along it, pointing from its back to its front

### `train-route.test.ts` (12 cases)

1. [routeAhead] starts exactly at the train and goes on from where it is along the route
2. [routeAhead] the part behind the train stays trimmed off as it goes on
3. [routeAhead] a route that comes back past the train: it is on the first pass
4. [routeAhead] a train on into the next stretch, past a gap, starts from there
5. [routeAhead] short of the route's start, it goes there along its own way ahead
6. [routeAhead] short of the start with no way of its own there: none drawn, ask again
7. [routeAhead] off the route, or past its end: none drawn, ask again
8. [routeAhead] beside the route, on another track: joined to it, but asked again
9. [routeAhead] the route on from the train stays the same lines while it is on the same leg
10. [routeAhead] a new route starts the search again
11. [wayToStop] from the train to where it next stands
12. [wayToStop] none while it stands, or with no stop ahead in its frames

### `trains.test.ts` (23 cases)

1. [colours and labels] every kind has a distinct colour and a label
2. [colours and labels] unknown kinds fall back to other
3. [colours and labels] clear of the ship palette and of the selection amber
4. [positionAt] without frames: the reported position
5. [positionAt] interpolates between frames
6. [positionAt] holds at the ends rather than guessing
7. [positionAt] crosses the antimeridian the short way
8. [headingAt] along the frames, else the reported heading
9. [credits and wording] one credit per source in view
10. [credits and wording] delays
11. [on the map] a carriage with a heading, a square without; unknown kinds as other
12. [on the map] sizes stay modest and grow with zoom
13. [panel] positions go stale after five minutes
14. [panel] delay tones
15. [panel] every source has a name and a credit
16. [stops] a stop shows its departure as it now stands
17. [stops] the next stop follows the last one passed
18. [stops] operators have colours of their own; others their kind's
19. [products] a Swedish train's product is its name without the number
20. [trainRunning] a train is dropped 30 s after it stands at its last stop
21. [blockAt] a train stays in its own block where another runs on beside it, then onto its track
22. [blockAt] a train going through the crossover stays in the block it came in
23. [blockAt] a train goes on into the next block, and is in none off the track

### `tree-layer.test.ts` (10 cases)

1. [trees] woods, parks and gardens grow trees; roads, buildings and water keep them off
2. [trees] a wood is planted thick, inside it only, and off its railway and building
3. [trees] the same trees every time
4. [trees] broadleaves green in summer, turning in autumn, bare in winter; the tropics always green
5. [trees] the tiles around a point, nearest first
6. [trees] the cards: two crossed quads a unit wide and tall, on the ground, the picture's top at the top
7. [trees from the canopy and the land cover] on unmapped land, trees stand where the canopy is, as tall as it
8. [trees from the canopy and the land cover] a wood's clearings are left open, and its trees are the land cover's kind
9. [trees from the canopy and the land cover] without a canopy, unmapped land the land cover calls woods is planted; fields and water aren't
10. [a town's registered trees] each stands where it is registered, of its kind and crown, and no other tree crowds it
