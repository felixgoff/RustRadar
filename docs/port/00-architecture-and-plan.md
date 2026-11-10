# 00 — Architecture and staged plan for the native Rust port

Status: decided. This document is the entry point for the port. Docs 01–07 in this folder are the specifications
(what the app does today, with constants and `file:line` references); this document says how the native app is
built and in which order. Where a spec document lists options or open questions, the choice made here wins
(section 7 lists every one).

Target: one native binary per platform (Windows, macOS, Linux, Android, iOS). A winit window, wgpu rendering, an
egui UI styled to today's look. No webview, no TypeScript, no Tauri. Priority: feature parity first, visual polish
second.

Conventions

- `RS:` = `src-tauri/src/` as it is today. `G:` = `src/lib/Globe.svelte`. "doc 03 §5.4" = section 5.4 of
  `03-globe-aircraft-layers-interaction.md`.
- Work items have ids (`S1.4`, `E7`, …). An item is sized for one engineer-agent: it names the files it owns, the
  spec sections it implements, the TypeScript it ports and how it is accepted.
- "Shot" = a headless screenshot produced by `rustradar-app --screenshot` through SwiftShader Vulkan (section 8).
- Rules for every item: the workspace compiles and `cargo test --workspace` passes when the item lands; no
  `cargo fmt`, no prettier; no API keys or secrets in code, logs, fixtures or docs; an item writes only the files
  it lists.

Contents: 1 Architecture · 2 Threading, frame loop, data flow · 3 Staged plan · 4 Parity checklist ·
5 Risk register · 6 Deliberate deviations and post-parity list · 7 Decisions on the mappers' open questions ·
8 Test and screenshot harness · 9 Questions still open for the owner.

Facts checked while writing this document (not taken from the spec documents)

| Check | Result |
|---|---|
| Native stack (spike, `crates/app`) | winit 0.30.13 + wgpu 30.0.1 + egui 0.37.0 render offscreen through SwiftShader Vulkan; `cargo check` is clean for `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`, `aarch64-apple-ios`, `aarch64-linux-android`. Nothing was linked or run for a cross target, and the interactive window has never been run. |
| KTX2 transcoding | `basisu_c_sys 0.9.1` (feature `extra`) builds here in 40 s and transcodes the repository's files: `earth.color.ktx2` (ETC1S, 4096×2048, 13 mips) to BC7 in 99 ms, `ground.color.ktx2` (UASTC+Zstd, 23 layers) to BC7 in 85 ms and to ASTC 4×4 in 46 ms, `aircraft/icons.ktx2` to BC7 in 11 ms. ETC2 and RGBA8 targets also work. Its build script runs `bindgen` (needs libclang on the build host) and compiles C++; not checked for Android or iOS. |
| Overpass fonts | `wuff 0.2.10` (pure Rust) decodes `node_modules/@fontsource-variable/overpass/files/overpass-latin-wght-normal.woff2` to an 89 KB TTF that keeps its `wght` axis (100–900, default 400). Overpass Mono: axis 300–700, **default 300**. epaint 0.37 takes per-font and per-text variation coordinates (`FontTweak::coords`, `TextFormat::coords`), so weights 550/650/750 are exact. |
| deck.gl tile zoom rule | `node_modules/@deck.gl/geo-layers/dist/tileset-2d/utils.js:208`: `z = round(view.zoom + log2(512 / tileSize) + zoomOffset)`, then clamped to `[minZoom, maxZoom]`. So 512-px vector tiles use `round(zoom)` (doc 01 §5.6 says "≈ floor": wrong), 256-px tiles use `round(zoom + 1)` (confirms doc 02's radar assumption; imagery clamps at 19). |
| deck.gl world coordinates | `@math.gl/web-mercator` `lngLatToWorld`: `x = 512·(λ + π)/(2π)`, `y = 512·(π + ln tan(π/4 + φ/2))/(2π)`; y grows **north**. |
| winit 0.30.13 on Android | `Window::set_ime_allowed` shows and hides the soft keyboard, but the backend never reads `GameTextInput` text events and emits no `Ime` events; text arrives only as key events. `android-activity 0.6.1` exposes `AndroidApp::text_input_state()` / `set_text_input_state()` (GameActivity) and `content_rect()`. |
| egui 0.37 safe areas | `RawInput::safe_area_insets` exists; egui-winit fills it on iOS only. |
| Lucide icons | `node_modules/@lucide/svelte` 1.54.0 holds each icon's geometry as JSON (`iconData.node`: `path`, `circle`, `rect`, `line`, … with attributes) in `dist/icons/<name>.svelte`. |
| Crates on crates.io today | `earcutr 0.5.0`, `glam 0.34.3`, `tiny-skia 0.12.0`, `resvg 0.49.0`, `jiff 0.2.41`, `pmtiles 0.28.0`, `minisign-verify 0.3.0`, `cargo-packager 0.11.8`, `cargo-ndk 4.1.2`, `rfd 0.18.0`, `webbrowser 1.2.6`, `image-webp 0.2.4`. |
| Tools in this container | `bun` and `node` 22 are installed (needed once, in stage 1, to generate fixtures from the TypeScript before it is deleted). |

---

## 1. Target architecture

### 1.1 Workspace layout

```
Cargo.toml                     [workspace], [workspace.dependencies], profiles (unchanged)
crates/
  fr24/        package fr24               unchanged: Flightradar24 client
  core/        package rustradar-core     pure logic and data types. No I/O, no clock, no async, no GPU, no UI
  backend/     package rustradar-backend  today's src-tauri/src without Tauri: HTTP, caches, feeds, streams
  scene/       package rustradar-scene    CPU side of the map: camera, tiles, tessellation, meshing, label
                                          layout, picking, per-kind instance builders. No GPU, no I/O
  map/         package rustradar-map      wgpu: pipelines, passes, tile runtime, textures, headless render
  app/         package rustradar-app      winit event loop, egui UI, app state, persistence, platform glue,
                                          updater. Renamed to `rustradar` in stage 12
  xtask/       package xtask              developer and release tooling (never shipped, never cross-compiled)
src-tauri/     package rustradar          until stage 12: a thin Tauri shim over rustradar-backend
assets/        fonts/, icons/lucide/      new vendored assets (stage 1). `static/` moves here in stage 12
platform/      android/, ios/             Gradle project and xcodegen spec (stage 2 prototype, stage 11 final)
```

Dependency direction (an arrow reads "depends on"); nothing else is allowed:

```
app ──► map ──► scene ──► core
 │                         ▲
 └────► backend ───────────┘
           └──► fr24
xtask ──► core (only for shared constants); src-tauri ──► backend
```

- `core` depends on nothing in the workspace. It owns the data types (`core::model`) that `api.ts` describes, so
  logic, backend and scene agree on one definition.
- `map` never depends on `backend`. It receives bytes through a trait (`map::io::TileFetcher`) that `app`
  implements with `backend::tiles`. Headless tests implement it with files on disk.
- `scene` and `core` never read a clock: every function takes `now_ms`. This is what lets the 394 TypeScript
  tests port one to one (doc 05 §3).
- `app` is the only crate that knows winit, egui and the platforms.

### 1.2 Crate responsibilities and module maps

**`rustradar-core`** (doc 05 §3 mapping table applies unchanged, with these additions)

| Module | Holds |
|---|---|
| `model/{flight,airport,ship,train,radar,place,adsb,weather,keys}.rs` | the DTOs of `api.ts`, `serde` derives kept (caches, settings and keys files need them) |
| `num.rs`, `text.rs` | the JavaScript-semantics helpers of doc 05 §14 (`js_round`, `sign0`, `to_fixed`, wraps, `group_thousands`, `fold`, natural compare) |
| `geo.rs`, `geo/sphere.rs` | `geo.ts`; great-circle helpers of `flightpath.ts:76-133` |
| `mvt.rs` | the one Mapbox-Vector-Tile decoder for the whole app, extracted from the hand-written protobuf reader in `RS:rail.rs:189-270` (`Pb`, `fields`, `packed`, `value`) |
| `time.rs`, `time/zone_names.rs` | `time.ts`; generated zone-abbreviation table |
| `sun.rs` | `daynight.ts`, `shadow.ts` |
| `motion/aircraft/*` | `motion.ts` (doc 05 §10) |
| `flightpath/*`, `routes.rs`, `airac.rs`, `taxinet.rs` | doc 05 §8–9 |
| the rest | one module per row of doc 05 §2.1 (`filters`, `settings`, `export`, `keys_file`, `signin`, `reference`, `weather`, `adsb`, `labels`, `readings`, `hover`, `places`, `ships`, `ship_list`, `ship_details`, `trains`, `train_list`, `train_cars`, `train_route`, `freshness`, `liveries`) |

**`rustradar-backend`** (doc 07 §10 is the API): every file of `RS:` except `lib.rs`, `main.rs`, `updater.rs`;
`motion.rs` is renamed `rail_motion.rs`. New modules added by later stages: `tiles.rs` (disk-cached HTTP tile and
TileJSON fetcher), `images.rs` (photo and logo fetch + decode), `streets_gl.rs`, `overture.rs` (PMTiles),
`canopy.rs` (COG range reads), `landcover.rs` (WMS and Planetary Computer rasters), `clouds.rs` (EUMETSAT
picture), `terrain.rs` (elevation tiles), `rainviewer.rs`, `convert.rs` (fr24 records → `core::model`).

**`rustradar-scene`**

| Module | Holds | Spec |
|---|---|---|
| `camera.rs` | `ViewState`, globe and Mercator viewports (f64 matrices), project/unproject, limits, `meters_per_pixel` | doc 01 §2 |
| `controller.rs` | north-up pan, drag-rotate, wheel zoom, pinch, linear transitions, follow/lock view | doc 01 §3, doc 03 §11.4 |
| `tiles.rs`, `lod.rs` | tile ids and bounds, deck's zoom rule and view-frustum selection, distance LOD selection, the two refinement strategies, look-ahead | doc 01 §5, doc 04 §13 |
| `geom/{densify,grid_cut,earcut,path}.rs` | ≤ 5° subdivision for the globe, `cutPolygonByGrid` port, `earcutr` wrapper, path segment records | docs 01 §2, 02 §3.4 |
| `tess/{vector,ground,buildings}.rs` | decoded MVT features → fills, path segments, extrusions | docs 01 §5.6, 04 §6–7 |
| `sdf.rs`, `text.rs`, `declutter.rs` | TinySDF port, glyph atlas packing, label layout, greedy collision with fades | docs 01 §5.9/§6, 03 §7 |
| `pick.rs` | CPU picking | doc 03 §10.1 |
| `aircraft.rs`, `aircraft_assets.rs`, `aircraft_models.rs`, `ships.rs`, `ship_models.rs`, `trains.rs`, `train_models.rs` | per-kind instance builders (icons, meshes, shadows, labels) and mesh data | docs 02, 03 |
| `radar/{mod,iso,mesh,arena}.rs`, `rail.rs`, `rail_overview.rs`, `bridges.rs`, `blocks.rs` | radar meshing, rail styling, bridges | doc 02 |
| `look.rs`, `ground.rs`, `roofs.rs`, `building_shadow.rs`, `trees.rs`, `clouds.rs`, `landcover.rs`, `landcover_ground.rs`, `streets_gl.rs`, `road_widths.rs`, `overture.rs`, `canopy.rs`, `street_trees.rs` | realistic map, CPU side | doc 04 |
| `hillshade.rs`, `terrain/{martini,cover}.rs` | hillshade, RTIN mesh, cover-texture bookkeeping | doc 04 §14 |
| `atlas/{ships,trains}.rs` | the two icon atlases drawn with tiny-skia (port of the Canvas 2D drawing) | doc 02 §4.2, §5.2 |
| `gpu_types.rs` | `bytemuck::Pod` vertex and instance structs shared with `map` | – |

**`rustradar-map`**

| Module | Holds |
|---|---|
| `gpu.rs` | adapter/device/queue, limits, feature detection (moved from the spike's `crates/app/src/gpu.rs`) |
| `target.rs`, `frame.rs` | offscreen map target, composite pass, per-frame globals uniform |
| `shaders/*.wgsl`, `shaders.rs` | WGSL sources and a concatenating loader; `lib/project.wgsl` (projection), `lib/rlight.wgsl` (light model) |
| `pipelines/{fill,path,icon,mesh,text,circle,bitmap,shadow}.rs` | the eight generic pipelines |
| `tiles.rs`, `jobs.rs`, `io.rs` | tile runtime (queue, priorities, cancellation, LRU, upload budget), worker pool, `TileFetcher` trait |
| `textures.rs` | KTX2 loading and transcoding, stand-in textures |
| `layers/*.rs` | one file per layer group: `earth`, `vector_map`, `imagery`, `labels`, `night`, `buildings`, `aircraft`, `ships`, `trains`, `rail`, `blocks`, `radar`, `route`, `markers`, `ground`, `trees`, `clouds`, `hillshade`, `terrain`, `sky` |
| `renderer.rs` | `MapRenderer`: owns the layers in draw order, `prepare` + `draw` |
| `post.rs` | tilt-shift blur, rain streaks |
| `headless.rs` | render a `SceneInput` to an RGBA image without a window |

**`rustradar-app`**

| Module | Holds |
|---|---|
| `main.rs`, `lib.rs`, `app.rs` | entry points (desktop `main`, `android_main`, iOS `rustradar_ios_main`), winit `ApplicationHandler` |
| `state/{mod,settings,shortcuts,camera,pick,aircraft,flight,liveries,taxiways,airport,trains,blocks,ships,weather,radar,search}.rs`, `dev.rs` | the state machine of doc 06 §3, one file per feed or concern |
| `persist.rs` | `settings.json` (schema of doc 06 §23), atomic write, debounce |
| `requests.rs` | `Pending<T>`, the wake callback, the tokio runtime handle |
| `scene_builder/{mod,base,aircraft,flight,trains,ships,radar,realistic}.rs` | turns state into `map::SceneInput` each scene refresh, one file per kind |
| `input.rs` | pointer, wheel, touch and gesture routing between egui and the map |
| `ui/{theme,fonts,text,icons,anim,widgets/*}.rs` | design system |
| `ui/{shell,sidebar,panel,modebar,search,lists/*,filters/*,flight_panel/*,train_panel,ship_panel,airport_panel,chart,export_menu,map_controls,menus,keys_dialog,shortcuts,hover_card,tooltip,toast,dev_panel,attribution,sheet,phone,map_error}.rs` | one file (or folder) per component of doc 06 |
| `images.rs` | remote image → egui texture cache |
| `platform/{mod,desktop,android,ios}.rs` | directories, open URL, reveal file, clipboard, soft keyboard bridge, safe areas, back gesture |
| `update/{mod,manifest,install,splash}.rs` | updater |
| `headless.rs` | `--screenshot` path |

### 1.3 Third-party crates

Pinned in `[workspace.dependencies]`; members write `dep.workspace = true`.

| Need | Crate | Why this one; what was rejected |
|---|---|---|
| Window, events | `winit 0.30.13` | the only version egui-winit 0.37 accepts (0.31 is beta) |
| GPU | `wgpu 30` | pinned by egui-wgpu 0.37; Vulkan, Metal, DX12, GLES |
| UI | `egui 0.37`, `egui-wgpu 0.37`, `egui-winit 0.37` (no default features, per-target features as in the spike) | immediate mode fits the request/poll backend pattern; `egui_extras` and `eframe` are not used (we own the loop and the renderer) |
| Maths | `glam 0.34` (`DVec3`, `DMat4` on the CPU; `Vec3`, `Mat4` for uniforms) | small, no generics; nalgebra rejected as heavier with no gain |
| GPU structs | `bytemuck 1` with explicit padding | `encase` rejected: the one large uniform (`rlight`, 192 bytes) has documented offsets (doc 04 §4.1) |
| Polygon triangulation | `earcutr 0.5` | the same algorithm deck.gl and math.gl use (earcut), so hole handling and winding expectations carry over |
| Line rendering | none: an own instanced segment pipeline | widths are in screen pixels with minimum/maximum clamps and must stay constant under zoom and pitch, so expansion happens in the vertex shader as in deck's PathLayer. `lyon` tessellates on the CPU at a fixed world width and cannot do that; rejected |
| Polygon grid cutting, marching squares, RTIN | own code in `scene` (`grid_cut`, `radar::iso`, `terrain::martini`) | each is a 100–300 line port whose exact behaviour is pinned by tile-edge seams (doc 02 §14.3); the `contour` crate stays in `backend` for the measured volumes only; `i_overlay`, `martini` crates rejected |
| MVT decoding | own `core::mvt` (from `RS:rail.rs`) | already written and tested in the backend; no protobuf code generation; one place to implement "absent numeric property reads as 0" (doc 04 §17.9). `prost`, `mvt-reader`, `geozero` rejected |
| KTX2 / Basis textures | `basisu_c_sys 0.9.1` (default features plus `extra`: the combination verified) | verified above on every file in `static/`; handles UASTC+Zstd and ETC1S in one call and picks BC7 / ASTC / ETC2 / RGBA8. `basis-universal 0.3.1` rejected (no ETC1S-in-KTX2 path); pure-Rust parsing with `ktx2` + `ruzstd` rejected (still needs a block transcoder). Fallback if a cross build fails: section 5, K7 |
| Images | `image 0.25` (`png`, `jpeg`, `webp`) | already a dependency; pure-Rust decoders (`png`, `zune-jpeg`, `image-webp`); terrain WebP is lossless |
| 2D raster (icon atlases, Lucide icons, glyph masks) | `tiny-skia 0.12`, `resvg 0.49` (no default features) | Canvas-2D-like anti-aliased strokes with round caps and joins; resvg reads the Lucide SVGs as they are |
| Font outlines for map labels | `skrifa` (the version epaint 0.37 already pulls in, 0.44) | variable-axis aware, already compiled; `fontdue` rejected (no variable fonts) |
| Async, HTTP | `tokio 1` (`rt-multi-thread`, `time`, `macros`, `sync`), `reqwest 0.13` through `fr24::client_builder()` | unchanged; every client must be built through `fr24::client_builder()` or it panics on Android (doc 07 §2.3) |
| Time zones | `jiff 0.2` with the bundled tz database | Android and iOS expose no portable tz database; abbreviations come from our generated table, not from jiff |
| Text folding, regex, ordered maps | `unicode-normalization 0.1`, `regex 1`, `indexmap 2` | doc 05 §14 |
| PMTiles (Overture heights) | `pmtiles 0.28` (reqwest backend) | maintained, async range reads |
| Paths | `dirs 6` joined with `com.felix.rustradar` by hand | lands in exactly Tauri's folders (doc 07 §11.1); `directories` rejected |
| Open URL, file dialog | `webbrowser 1.2` (desktop, Android, iOS), `rfd 0.18` (desktop only) | phones use no file dialog in the first native release |
| Updater | own module with `minisign-verify 0.3`, `semver 1`, `flate2`, `tar` | keeps `RS:updater.rs`'s control flow; `cargo-packager-updater` rejected as unverified |
| Desktop bundles | `cargo-packager 0.11` (CLI, CI only) | the Tauri bundler extracted, so the NSIS and MSI installers accept the old Tauri updater's flags and upgrade in place |
| Android, iOS builds | `cargo-ndk 4`, Gradle (existing project, GameActivity), `xcodegen` | doc 07 §13.4–13.5 |
| woff2 → TTF, release tooling | `wuff 0.2` and friends, in `xtask` only | pure Rust; runs once |

Not used, deliberately: `lyon`, `prost` for tiles, `rayon` (jobs need priorities and cancellation), `naga_oil`
(plain concatenation is enough for two shared WGSL libraries), ICU (`icu_*`), `eframe`, `cosmic-text`/`glyphon`
(map labels are single-line SDF text with a halo, which neither provides).

### 1.4 Rendering architecture

#### 1.4.1 Colour and targets

- The map is always drawn into an **offscreen `Rgba8Unorm` texture** (not sRGB) with 4× MSAA and a
  `Depth24PlusStencil8` attachment, resolved into a single-sample texture. Colours from the specs go in unchanged
  and blend in gamma space exactly as on the WebGL canvas (doc 01 §10.1). The realistic shaders apply exposure and
  `pow(1/2.2)` themselves (doc 04 conventions).
- Map resolution = window size × `map_scale`, `map_scale = scale_factor` on dark and satellite,
  `min(scale_factor, 1.5)` on realistic (doc 01 §2.5).
- A **composite pass** draws that texture onto the surface (bilinear when scaled), applying tilt-shift blur and
  rain when on, then the **UI pass** draws egui on top (`LoadOp::Load`). The surface uses the first non-sRGB
  format; if a platform offers only sRGB formats, the composite shader linearises and egui-wgpu takes the sRGB
  path (the spike has this branch; untested).
- Clear colour of the map texture: `#1e1e1e` (`--ink`) on dark and satellite (the "space" around the globe); on
  realistic the sky pass fills it (doc 04 §3.6).

#### 1.4.2 Coordinate spaces and precision

The GPU never sees absolute world coordinates. Everything is relative to the camera centre and computed in f64 on
the CPU before it becomes f32.

| Space | Definition |
|---|---|
| Geographic | lon, lat in degrees (f64), height in metres |
| World Mercator `W` | deck's common space on the flat map: 512 units round the world, `x = 512·(lon + 180)/360`, `y = 512·(π + ln tan(π/4 + φ/2))/(2π)` (north up), `z` in units with `units_per_metre = 512 / (40 075 016.686·cos φ)` |
| Globe common `S` | sphere of radius 256 units: `D = (h/6 370 972 + 1)·256`, `x = sin λ·cos φ·D`, `y = −cos λ·cos φ·D`, `z = sin φ·D` (doc 01 §2.2) |
| Local Mercator `L` | `W` minus an f64 anchor, scaled so a mesh's extent is about 1; stored as f32. Tile meshes use the tile's north-west corner as anchor and tile units (`u, v ∈ [0, 1]`, v south), plus `z` in metres |
| Render | camera-centre-relative f32 |

- **Static meshes** (tile content, radar meshes, night quads, trails, routes, rail lines) are stored once in
  local Mercator. Each draw carries a small `Frame` uniform: the anchor relative to the view centre (computed in
  f64), the scale, and the tile's Mercator-y terms. One WGSL library, `lib/project.wgsl`, has both paths:
  - flat (zoom > 12): `clip = frame_matrix · vec4(local, 1)`, with `frame_matrix = view_proj · translate(anchor − centre) · scale` computed in f64.
  - globe (zoom ≤ 12): the vertex shader turns local Mercator into lon/lat (longitude as a difference from the
    view centre; latitude through the inverse Mercator) and then into the sphere position relative to the view
    centre, using `sin`/`cos` of the centre supplied from the CPU. Budget: ≤ 0.25 px error at zoom 12, checked by a
    unit test that runs the same f32 formulas in Rust against `scene::camera` in f64 (item E4).
  One tile cache therefore serves both projections; nothing reloads at zoom 12 (doc 01 §5.6 notes that today it
  does).
- **Dynamic instances** (aircraft, ships, trains, markers, rings, labels) are projected on the CPU in f64 at each
  scene refresh: a camera-relative position, and for meshes a full instance matrix built from a true
  east/north/up frame at the object (so deck's `yawOffset = 180` on the globe and the flipped shadow sun are not
  reproduced; doc 03 §20.3).
- Large primitives on the globe are subdivided so that no edge spans more than 5° (`scene::geom::densify`, and
  `grid_cut` for polygon interiors), matching `resolution: 5` (doc 01 §2).

#### 1.4.3 Camera

`scene::camera` reproduces deck.gl 9.4 exactly (doc 01 §2), including the three zoom/latitude couplings
(`scale = 2^zoom/(π·cos lat)`, the zoom change while panning north or south, and limits shifted by
`log2(cos lat)`), the hard projection swap at zoom 12, linear transitions and no inertia. Golden values generated
from deck.gl itself (item X5) pin `project`/`unproject` for both viewports.

#### 1.4.4 Frame passes

```
scene refresh (paced, 1.4.9)   state → SceneInput → instance buffers, label layout, tile selection
per frame:
  0  uploads                   finished tile jobs (budget 4 tiles or 8 MB per frame), instance buffers
  1  cover passes (stage 9)    ≤ 2 terrain cover textures re-rendered per frame, orthographic, no MSAA
  2  map pass                  MSAA×4 colour + depth/stencil, clear; layers in the order of doc 03 §3
  3  composite pass            map texture → surface, optional tilt-shift and rain
  4  ui pass                   egui
```

One command encoder per frame; egui's `update_buffers` command buffers are submitted before it (spike pitfall 5).

#### 1.4.5 Draw-state presets

Blend everywhere: colour `SrcAlpha, OneMinusSrcAlpha`; alpha `One, OneMinusSrcAlpha`. A layer's opacity
multiplies fragment alpha. Depth compare `LessEqual` unless stated. No polygon offset anywhere: order is the draw
order, and ground layers write no depth (doc 02 §14.3.10).

| Preset | Cull | Depth write | Depth compare | Stencil | Spec |
|---|---|---|---|---|---|
| `DEFAULT` | back | on | less-equal | – | doc 01 §4 |
| `NO_CULL` | none | on | less-equal | – | |
| `SURFACE` | back | off | less-equal | – | |
| `SURFACE_NO_CULL`, `RADAR_LIFTED`, `SHIP`, `TRAIN`, `CLOUD` | none | off | less-equal | – | docs 01 §4, 02 §1.2 |
| `LABEL`, `THROUGH_GROUND`, `RAISED_NO_CULL` | none | off | always | – | |
| `SHADOW` | none | off | less-equal | compare `Equal` (reference 0), pass `IncrementClamp`, fail and depth-fail `Keep`, write mask `0xff` | doc 03 §3.1 |

The stencil buffer is cleared to 0 once per frame by the map pass; every shadow (aircraft, building, foot shade,
tree) uses `SHADOW`, so a pixel is darkened at most once. Draw order of shadows is as specified (doc 04 §1: foot
shade, then building shadows, then the buildings; all aircraft shadows before any aircraft).

#### 1.4.6 Generic pipelines

| Pipeline | Replaces (deck.gl) | Vertex / instance data | Used by |
|---|---|---|---|
| `fill` | SolidPolygonLayer (flat) | local Mercator position, RGBA or paint | water, land, night, radar, ground areas |
| `path` | PathLayer + PathStyleExtension | per-segment instance: previous, start, end, next (local Mercator + z), width, colour, cumulative length; flags: width in px or metres, min/max px, miter (limit 4) or round joints, butt or round caps, dash array, justified dashes | borders, roads, rail, trail, route, blocks, ground lines |
| `mesh` | SimpleMeshLayer, extruded polygons | shared vertex buffer (position, normal, colour); per-instance matrix, colour, highlight; a second vertex entry takes tile-local positions with a `Frame` instead of an instance matrix (extruded buildings); Phong with the light rig of doc 01 §4 | aircraft, gear, ship models, train cars, buildings |
| `icon` | IconLayer | per-instance position, atlas rectangle, size px, angle, tint, highlight; billboard or ground-plane | aircraft icons and outlines, ship and train icons |
| `text` | TextLayer (SDF) | per-glyph quad: anchor, offset px, atlas rectangle, colour; uniforms: outline width and colour, optional background box | place, airport, aircraft, signal, block, speed labels |
| `circle` | ScatterplotLayer | per-instance position, radius (px or metres), fill, stroke, stroke width | selection halos and rings, airport dots, signals, stops |
| `bitmap` | BitmapLayer | densified textured quad in local Mercator, tint, opacity | imagery, hillshade, clouds |
| `shadow` variants | – | the `mesh`/`fill` shaders with the `SHADOW` state | aircraft, building, tree shadows |

Realistic-map shaders (ground, building, tree, tree shadow, overlay, cloud, terrain, sky) are separate WGSL files
that include `lib/rlight.wgsl` (doc 04 §16.1). Every WGSL file is parsed and validated with `naga` in a unit test
that needs no GPU.

#### 1.4.7 Layers

Layers are **persistent objects in a fixed order**, not descriptors diffed by id. `MapRenderer` holds one struct
per row of the layer table in doc 03 §3 and draws them in that order; each computes its own visibility from the
`SceneInput`.

```rust
pub trait Layer {
    /// Main thread, before encoding: react to the view and the data, request tiles, upload buffers.
    fn prepare(&mut self, cx: &mut PrepareCx<'_>);
    /// Record draw calls for one pass (the map pass, or a terrain cover pass).
    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, cx: &DrawCx<'a>);
    /// How it stands on 3D terrain (stage 9).
    fn drape(&self) -> Drape { Drape::None }      // None | Cover | Offset
}
```

`map::SceneInput` is plain data: the view, the settings that affect drawing, and per-kind draw lists already
produced by `scene` builders (`Vec<IconInstance>`, `Vec<MeshInstance>`, label inputs, selected-flight geometry).
`map` knows nothing about flights, trains or the UI.

#### 1.4.8 Tiles

- **Selection** (pure, `scene::tiles` and `scene::lod`): deck's rule `z = round(zoom + log2(512/tile_size))`
  clamped to the source's range, with frustum traversal on the globe; or the distance LOD rule of doc 04 §13.2 for
  the realistic ground, buildings, platforms and terrain.
- **Refinement** (pure): `best_available` (deck's default: loaded ancestors or descendants stand in) or
  `keep_coarser_until_in` (doc 04 §13.4, never draws a place twice).
- **Runtime** (`map::tiles::TileSet<T>`): a request queue ordered by priority, at most `max_requests` in flight
  per source (8 unless the spec says otherwise), cancellation of requests for tiles no longer selected (look-ahead
  tiles exempt, doc 04 §13.3), an LRU of decoded content, and GPU upload within the per-frame budget.
- **Fetching**: `map::io::TileFetcher::fetch(source, z, x, y) -> Receiver<Result<Bytes, FetchError>>`; `app`
  implements it on tokio with `backend::tiles`: on-disk cache under `<cache>/tiles/<source>/<z>/<x>/<y>` (fresh for
  the response's `max-age`, else 7 days; a stale copy is used if the download fails; total size capped at 512 MB on
  desktop and 128 MB on phones, oldest first). The OpenFreeMap URL template is read from its TileJSON at start-up,
  never hard-coded (doc 01 §5.6).
- **Decoding** runs on the worker pool: bytes → `core::mvt` → `scene::tess` → CPU buffers in tile-local
  coordinates. A tile is decoded once and serves the globe and the flat map.

#### 1.4.9 Scene refresh pacing

Camera motion and the UI redraw at display rate. The **scene refresh** (aircraft poses, instance buffers, label
layout, night polygons) runs at the intervals of doc 01 §2.6: every frame when locked on a vehicle and zoom > 7;
50 ms at zoom > 7; 150 ms at zoom > 4; else 500 ms. A change of settings, selection, hover or data, or a finished
tile, forces one immediately. When nothing else needs a redraw, the event loop sleeps until the next refresh is
due.

#### 1.4.10 Text on the map

A dedicated SDF pipeline (egui text has no outline). `scene::sdf` ports TinySDF with deck's settings: glyphs
rasterised at 64 px from Overpass at weight 600 (skrifa outline → tiny-skia mask), buffer 6, radius 12, cutoff
0.25, packed on demand into a 2048² `R8Unorm` atlas. Layout is per character with advance widths and no kerning,
as deck does. The fragment shader draws the fill and a halo of width 3 SDF units in `[6,12,19,220]`. Signal and
block labels use Overpass Mono, speed labels Overpass 650 (today: system monospace and Inter; section 6).
Collision is a CPU greedy pass per group (`"labels"`, `"aircraft-labels"`): sort by priority, keep a label if its
claimed rectangle hits no kept one, fade over 150 ms (docs 01 §5.9, 03 §7.5).

#### 1.4.11 Picking

CPU picking in `scene::pick`, no GPU read-back. Each scene refresh records the pickable things in draw order:
icons and far meshes as a screen point with radius `max(screen_px/2, 6)`; close meshes (aircraft at zoom ≥ 11,
ship models, train cars) as oriented boxes tested against the pointer ray; labels as rectangles. The topmost hit
within 6 px wins; ties go to the nearest to the pointer (doc 03 §10.1). Hover forces a scene refresh so the
highlight appears at once.

#### 1.4.12 Terrain (stage 9)

Draping follows deck's design, because it reuses the 2D ground renderer unchanged and matches today's look (doc
04 §14.5, second option):

- Terrain tiles: Mapterhorn WebP with Mapzen PNG fallback on 404, RTIN mesh (error 2 m), sea-level clamp, selected
  by `SURFACE_LOD`.
- **Cover textures**: every layer with `Drape::Cover` (ground, hillshade, rail map, radar ground, shadows, foot
  shade, night) is drawn top-down with an orthographic camera into one texture per terrain tile (512, 1024 or
  2048 px by on-screen size; `Rgba8Unorm` plus a stencil attachment for the shadow rule); the terrain mesh samples
  it and adds haze (`TERRAIN_AIR`). Covers are cached, re-rendered when their content, LOD or look changes, at most
  two per frame, within a memory budget of 256 MB on desktop and 96 MB on phones. Animated ground effects stay
  frozen between re-renders, as today.
- `Drape::Offset` layers (buildings, trees, vehicles, labels, markers) get their height on the CPU: bilinear
  lookup in the decoded height tiles when instance buffers are built.
- The trail and route of the selected flight keep their true altitude (section 6).

#### 1.4.13 Textures

`map::textures` loads the KTX2 files on the worker pool with `basisu_c_sys`: target BC7 when the adapter has
`TEXTURE_COMPRESSION_BC`, else ASTC 4×4, else ETC2, else RGBA8 (doc 04 §5.4 order; for the two ETC1S Earth
pictures the target is requested explicitly so an ASTC-only device does not fall to RGBA8). Stand-in 1×1 textures
are bound until loading finishes (doc 04 §5.3). If the device's maximum texture size is below 4096, the Earth
pictures skip their top mip.

Baseline GPU requirement: WebGL2-class limits (`Limits::downlevel_webgl2_defaults()` raised to the adapter's
resolution) and **no optional feature**. No compute, no storage buffers. This is what keeps GLES 3.0 Android
devices in scope.

### 1.5 UI architecture (egui styled to today's look)

- **Tokens**: `ui::theme` holds every colour, size, radius and spacing of doc 06 §1 as constants (the spike's
  `theme.rs` is the seed). Panels are opaque `--surface` with a 1 px `--line` border, radius 4, no shadow.
- **Fonts**: the Overpass and Overpass Mono variable fonts are vendored as TTF in `assets/fonts/` (decoded once
  from the `.woff2` files by `cargo xtask vendor-fonts`; subsets latin, latin-ext, cyrillic, cyrillic-ext,
  vietnamese, registered in that fallback order) and embedded with `include_bytes!`. Each weight in use is its own
  egui font family backed by the same bytes with a different `wght` coordinate: `ui-400`, `ui-500`, `ui-550`,
  `ui-600`, `ui-650`, `ui-700`, `ui-750`, `mono-400`, `mono-650`, `mono-700` (Mono's axis stops at 700 and its
  default is 300, so the coordinate is always set). egui's `default_fonts` feature is turned off.
- **Middle dot**: Overpass's `·` has no advance width. All UI text goes through `ui::text::job()`, which builds a
  `LayoutJob` and sets every `·` in the `mono-400` family (a fixed advance), replacing the web build's "Middle
  Dot" font trick (doc 06 §1.2).
- **Icons**: the Lucide icons in use (doc 06 §26, about 50) are vendored as SVG files in `assets/icons/lucide/`
  by `cargo xtask vendor-icons` (from `node_modules/@lucide/svelte`, ISC licence file included) and embedded.
  `ui::icons` rasterises them with resvg at the requested size × `pixels_per_point`, with the stroke width
  substituted (1.75, 2 or 2.5), as white alpha masks cached as egui textures and tinted at draw time. No icon font
  (it would fix the stroke width at one value), no network.
- **Widgets**: custom-painted, in `ui/widgets/`: `button`, `icon_button`, `badge`, `skeleton`, `alert`,
  `segmented`, `toggle_row`, `chips`, `token_input`, `tabs`, `text_field`, `range_slider`, `spinner`, `live_dot`,
  `tooltip`. Each reproduces sizes, colours and states from doc 06 §1.4 and its component section. Transitions use
  egui's animation manager with the easing `cubic-bezier(0.16, 1, 0.3, 1)` implemented in `ui::anim` (150 ms
  unless doc 06 §27 says otherwise). While a spinner, skeleton or live dot is visible the UI requests continuous
  repaint; otherwise it repaints on input, data arrival and the 1 s clock tick.
- **Layout**: each floating card is its own `egui::Area` at a fixed position (sidebar 320, detail 360, popover
  300, dialogs 440/360, hover card 272). The sidebar's shrink rule (list shrinks three times as fast as the filter
  panel, list minimum 96) is a small height solver in `ui::sidebar` (doc 06 §29). Phones use one bottom sheet with
  three detents and a top bar, padded by the safe-area insets.
- **Input routing**: pointer and keyboard events go to egui first; the map receives a gesture only when the
  pointer is not over an egui area and egui does not want the pointer or the keyboard.
- **Remote images** (aircraft, ship and train photos, airline and operator logos): `app::images` maps a URL to
  loading / texture / failed, fetches through `backend::images` (disk cache, 64 MB cap), decodes off the UI
  thread, and shows the skeleton meanwhile.
- **Text input on phones**: iOS uses winit's keyboard events through egui-winit. Android uses GameActivity and an
  own bridge (`platform::android::text`): while an egui text field has focus, the field's text and selection are
  mirrored to `AndroidApp::set_text_input_state`, the soft keyboard is shown, and each frame
  `AndroidApp::text_input_state()` is read back and written into the field (all text fields are single-line). The
  Android content rectangle supplies `RawInput::safe_area_insets`.
- **Formatting**: numbers and dates are fixed to English (`12,345`, `Mon 6 Oct`); sorting uses a natural compare
  over folded strings (section 7).

### 1.6 Assets

All of `static/` (27 MB) is embedded with `include_bytes!` on every platform, through one module per crate
(`map::assets`, `app::assets`) so that paths appear in one place. The files stay in `static/` while the Tauri
build exists and move to `assets/` in stage 12.

---

## 2. Threading, frame loop and data flow

### 2.1 Threads

| Thread(s) | Runs | Rules |
|---|---|---|
| **Main** (the winit event loop; on Android the `android_main` thread, on iOS the UI thread) | input, app state, `core` logic including the aircraft motion model, `scene` instance builders, egui, wgpu encoding, submit and present | never blocks on the network or on a lock held across I/O; never calls `Handle::block_on` |
| **Tokio runtime** (multi-thread, `min(4, cores)` workers, created in `main` before the event loop and kept for the process lifetime) | everything in `backend`: feeds, streams, tile and image downloads, disk caches | reached through `backend::rt()`, a process-global `&'static Handle` set once in `Backend::start`. `tokio::spawn` is allowed only inside `async fn`; synchronous code spawns through `rt()` (a missed call site aborts the process under `panic = "abort"`, doc 07 §11.4) |
| **Worker pool** (`map::jobs`, plain threads, `clamp(cores − 2, 2, 6)`) | CPU work: MVT decode and tessellation, radar contouring, hillshade, tree planting, RTIN meshing, KTX2 transcoding, image decoding | each job has a priority and a cancel flag; results return over an `std::sync::mpsc` channel and wake the main thread |
| Tokio blocking pool | the few blocking backend calls that exist today (`trains::route`, `save_export`) | unchanged |

Waking the UI: one `EventLoopProxy<UserEvent>`; `wake()` sends `UserEvent::Wake` unless one is already pending
(an `AtomicBool`). The backend receives it as `Arc<dyn Fn() + Send + Sync>` (doc 07 §10.1).

### 2.2 Frame loop

```
user_event(Wake) / input / timer due      → window.request_redraw()
window_event                              → egui-winit first; what egui does not take goes to input.rs
                                            (pan, rotate, zoom, pinch, click, hover) → scene::controller
RedrawRequested:
  1  clocks       local_now_ms (system clock); feed_now_ms = motion.now()
  2  state.poll   drain BackendEvents, Pending<T> results, Following<T> streams, worker results;
                  start requests whose deadline has passed (feed intervals of doc 06 §3.1)
  3  camera       advance the running transition, the follow lock, the terrain ground ease → ViewState
  4  scene        if a refresh is due or forced (1.4.9): scene_builder → SceneInput → renderer.update
  5  ui           egui run: draw from state, collect actions, apply actions to state
  6  render       renderer.prepare (uploads, tile requests); encode cover, map, composite, ui; submit; present
  7  schedule     Poll while a camera transition, a gesture or an egui animation runs; otherwise
                  WaitUntil(min(next scene refresh, egui's repaint delay, next state deadline))
```

- Actions, not callbacks: UI code returns `Vec<Action>` (select flight, set filter, fly to, open panel, …); the
  state machine applies them after the egui run. This keeps `ui/*` free of backend calls and testable with a
  fixture state.
- Two clocks, as today (doc 03 §20.12): aircraft, their shadows and the night layer use the feed clock; ships,
  trains, camera transitions and the realistic sky use the local clock.
- Suspend and resume (Android, iOS): on `suspended` drop the surface, stop feed timers, drop the follow streams and
  clear the watched-train set; on `resumed` recreate the surface and restart the feeds. The device, the pipelines
  and the caches survive. A lost device rebuilds `MapRenderer` from the CPU-side caches; if that fails the map
  error card is shown (doc 01 §9.5).

### 2.3 Data flow

```
network ─► backend (tokio) ─► Pending<T> / Following<T> / BackendEvent ─► app::state (main thread)
                                                                              │
                 core: filters, lists, wording, motion.pose(), routes ◄───────┤
                                                                              ├─► ui::* (egui) ─► Actions ─► state
                 scene builders: icons, meshes, labels, pick list ◄───────────┤
                                                                              ▼
tiles: map::tiles ─► TileFetcher (app → backend::tiles, tokio) ─► bytes   SceneInput
                 └─► map::jobs (decode, tessellate) ─► CPU buffers ─► upload ─► MapRenderer ─► GPU
```

| Data | Produced by | Reaches the main thread as | Consumed by |
|---|---|---|---|
| Aircraft snapshot | `Backend::live_flights` every 8 s (regional) or 15 s (world) | `Pending<Result<LiveSnapshot, String>>` | `state::aircraft` → `Motion::update` → list, filters, scene builder |
| Followed flight | `Backend::follow_flight` | `Following<FollowEvent>` drained each frame | `state::flight` → panel, trail, route |
| Trains, ships | `trains::in_view`, `ships::in_view` on camera settle and interval | `Pending<…>`; `BackendEvent::TrainsChanged` throttled to 1500 ms | `state::trains`, `state::ships` |
| Radar volumes | `Backend::radar_frame` every 120 s | `Pending<…>` carrying the band polygons directly (no byte encoding) | worker pool → `scene::radar` meshes |
| Map tiles | `TileFetcher` | worker result channel | `map::tiles` |
| Remote images | `backend::images` | `Pending<…>` | `app::images` → egui textures |
| Settings | UI actions | – | `persist` writes `settings.json` 500 ms after the last change |
| Update ready | updater task | `BackendEvent::UpdateReady` | update toast |

---

## 3. Staged plan

Twelve stages. Every stage, and every item within it, leaves the workspace compiling with tests passing. `src-tauri`
and the Svelte app keep building and working until stage 12, so the two apps can be compared side by side
throughout.

| Stage | Goal | Items |
|---|---|---|
| 1 | Workspace restructure, backend extracted from Tauri, all pure logic ported with its tests, assets and fixtures captured from the TypeScript | 35 |
| 2 | Map engine foundations: camera, passes, pipelines, tiles, text, picking, textures, app skeleton | 19 |
| 3 | Dark and satellite basemaps | 9 |
| 4 | Aircraft | 11 |
| 5 | Trains | 9 |
| 6 | Ships | 4 |
| 7 | UI shell and panels | 20 |
| 8 | Realistic mode | 12 |
| 9 | Terrain | 4 |
| 10 | Weather radar | 5 |
| 11 | Packaging, CI for five platforms, updater | 11 |
| 12 | Removal of TypeScript and Tauri | 4 |
| | | **143** |

Two tracks run side by side after stage 2: the **map track** (stages 3 → 4 → 5 → 6, then 8 → 9 → 10) owns
`crates/scene` and `crates/map`; the **UI track** (stage 7) owns `crates/app/src/ui`. They meet in
`crates/app/src/state` and `scene_builder.rs`, whose files are assigned to single items below. Stage 11 items
P1–P3 can start any time after stage 2.

Standard acceptance for every item, in addition to its own: `cargo test --workspace` passes;
`cargo clippy --workspace --all-targets` is clean for the crates the item touched; `cargo check -p rustradar`
(the Tauri shim) still passes until stage 12.

### Stage 1 — Workspace, backend extraction, pure logic

Goal: the Tauri-free crates exist; `src-tauri` is a shim over `rustradar-backend`; every non-drawing TypeScript
module has a tested Rust twin; everything that must be captured from the TypeScript or from `node_modules` before
they are deleted is captured. Nothing is drawn in this stage; `crates/app` is not touched.

Rules for the logic ports (doc 05 §14–15): transliterate first, refactor later; one TypeScript `test(...)` becomes
one `#[test]` with the same name in snake case (doc 05 appendix A lists all 394); use `core::num`/`core::text`
wherever the TypeScript used a JavaScript built-in whose semantics differ; pass `now_ms`, never read a clock;
`assert_close!(a, v, digits)` means `|a − v| < 0.5·10^−digits`.

| Id | Item | Files owned | Implements / ports | Acceptance |
|---|---|---|---|---|
| S1.1 | **Scaffold.** Add packages `rustradar-core`, `rustradar-backend`, `rustradar-scene`, `rustradar-map`, `xtask`; add `[workspace.dependencies]` with the pins of 1.3; create every module file named in 1.2 for `core` and `scene` as an empty stub declared in `lib.rs`, so later items fill only their own files; add `assert_close!` in `core::testing`; `.cargo/config.toml` alias `xtask = "run -p xtask --"` | root `Cargo.toml`, `crates/{core,backend,scene,map,xtask}/Cargo.toml`, their `src/lib.rs`, stubs, `.cargo/config.toml` | doc 05 §3, this doc 1.1–1.3 | workspace builds; `cargo tree -p rustradar-core` lists no workspace crate; `cargo tree -p rustradar-scene` lists no tokio, reqwest or wgpu |
| S1.2 | **Backend, part 1: infrastructure and stateless modules.** `Dirs`, `BackendEvent`, `Notifier`, `Following<T>`, `rt()`; move with `git mv`: `adsblol, blocks, board, building_lod, devlog, dwd, flow, geocode, lines, liveries, motion (→ rail_motion), mrms, opensky, osm_platforms, radar, rail, reference, roads, runways, session, ships, sth, train_class, train_media, weather`, plus `src-tauri/data` and `src-tauri/tests`; apply doc 07 §10.2 and §11.4–11.6 (drop `#[tauri::command]`, `tauri::async_runtime::spawn` → `rt().spawn`, URI-scheme handlers → `rail::tile`, `building_lod::tile`); the shim keeps each command as a `#[tauri::command]` wrapper | `crates/backend/src/**`, `crates/backend/{data,tests}`, `src-tauri/src/{lib.rs,commands.rs}`, `src-tauri/Cargo.toml` | doc 07 §3, §4.3, §10–11 | `cargo test -p rustradar-backend` runs the moved tests unchanged and green; `grep -rn tauri crates/backend/src` finds nothing in the moved files; the shim compiles |
| S1.3 | **Backend, part 2: stateful modules.** Move `keys, export, taxiways, trains, trafikverket`; `AppState` → `Backend` (`Arc<Inner>`, the eight fields of doc 07 §5.3) with the method list of doc 07 §10.2; `follow_flight`/`trains::follow` return `Following<T>`; `trains-changed` → `BackendEvent::TrainsChanged`; `cfg(desktop/mobile)` → `cfg(target_os)`; copy `newer_apk`, `parse_version` and their test to `backend::mobile_update`. `updater.rs` and `mobile_update.rs` stay in the shim | `crates/backend/src/{lib,keys,export,taxiways,trains,trafikverket,mobile_update}.rs`, `src-tauri/src/*` | doc 07 §3–5, §10, §12.3 | all 181 backend `#[test]`s pass in `rustradar-backend`; `cargo tree -p rustradar-backend` has no `tauri`; the Tauri app still runs (`bun tauri dev` smoke check by a human, not required for agents); `grep -rn "tokio::spawn" crates/backend/src` hits only lines inside `async fn` |
| S1.4 | **One set of DTOs.** Replace the backend's serde structs that mirror `api.ts` with `core::model` types (after S1.3 and S1.5); for records owned by `fr24` (`FlightDetailsRecord`, `TopFlightRecord`, `FindEntry`, `FlightListRecord`) add `From` impls in `backend::convert` | `crates/backend/src/**` (type definitions only), `crates/backend/src/convert.rs` | doc 05 §6.1, doc 07 §3.3–3.4 | for every converted type a test asserts `serde_json::to_value(core) == to_value(original)` on a fixture from `crates/backend/tests/data`; the shim's JSON output is unchanged |
| S1.5 | **`core::model`**: every type of `api.ts` as a plain struct or enum with `serde` derives and the same field names on the wire | `crates/core/src/model/*.rs` | `api.ts` (754 lines), doc 05 §6.1 | deserialises the JSON fixtures in `crates/backend/tests/data` and `src/lib/testdata/landings.json` (copied to `crates/core/tests/data/`) |
| S1.6 | **`core::num`, `core::text`**: `js_round`, `sign0`, `to_fixed`, `wrap180`, `wrap360`, `wrap_lon`, `clamp01`, `smoothstep`, `group_thousands`, `fold` (NFD, strip marks, lower-case), natural compare | `crates/core/src/{num,text}.rs` | doc 05 §14; `train-list.ts` `fold` | new tests: every row of the doc 05 §14 table that names a trap has a test (`js_round(-2.5) == -2`, `to_fixed(2.5, 0) == "3"`, `sign0(0.0) == 0`, …) |
| S1.7 | **`core::geo`** and `geo::sphere` (`distance_m`, `bearing`, `destination`, `intermediate`, `great_circle`, `unwrap`) | `crates/core/src/geo.rs`, `geo/sphere.rs` | `geo.ts` (126); `flightpath.ts:76-133`; docs 01 §3.13, 02 §1.1 | new tests for `altitude_color`, `elevation`, `effective_exaggeration`, `meters_per_pixel`, `visible_radius`, `view_area`, `cos_angle`; `extrapolate` ported as a test helper |
| S1.8 | **Six small modules**: `freshness`, `signin`, `labels`, `readings`, `adsb`, `places` | one file each in `crates/core/src/` | `freshness.ts`, `signin-values.ts`, `aircraft-labels.ts`, `aircraft-readings.ts`, `adsb.ts`, `places.ts`; doc 05 §6.4, §6.8, §11.1–11.5 | 45 tests ported (13 + 7 + 8 + 9 + 2 + 6) |
| S1.9 | **`weather`, `keys_file`, `export`** | `crates/core/src/{weather,keys_file,export}.rs` | `weather.ts` (`conditionsOf`, `PRESETS`, `mixConditions`, `SKIES`), `keys.ts`, `export.ts`; doc 05 §6.5–6.7, §6.11 | 10 tests ported (3 + 7); new test for `mix_conditions`; exported numbers print as JavaScript does (`10668`, not `10668.0`) |
| S1.10 | **`core::sun`** | `crates/core/src/sun.rs` | `daynight.ts`, `shadow.ts`; docs 01 §5.10, 05 §7.1–7.2 | 13 tests ported; new tests for `night_polygons` (quad counts per threshold, taper to a point) |
| S1.11 | **`core::time`** with the generated zone table. A Node script (run once, kept under `scripts/port/`) runs today's `zoneName` over every IANA zone for a January and a July instant and writes `(zone, offset minutes) → name` (Node's ICU stands in for the webview's; today's answers already differ between WebView2 and WebKit); the Rust module looks names up there, then `COMMON_NAMES`, then `utc_label` | `crates/core/src/time.rs`, `time/zone_names.rs`, `scripts/port/zone-names.mjs` | `time.ts` (252); doc 05 §6.3 | 13 tests ported; a test walks the generated table and checks it against jiff's offsets for both instants |
| S1.12 | **`reference`, `filters`, `settings`** | `crates/core/src/{reference,filters,settings}.rs` | `reference.svelte.ts`, `filters.ts`; docs 05 §6.5, §6.9, 06 §23 | 22 tests ported (the module mock becomes a plain `airline_icao`); new tests for every loading rule of doc 06 §23 including the legacy keys |
| S1.13 | **Ships logic** | `crates/core/src/{ships,ship_details,ship_list}.rs` | `ships.ts` (without the canvas atlas), `ship-details.ts`, `ship-list.ts`; doc 05 §12.1–12.3 | 22 tests ported (4 + 4 + 14) |
| S1.14 | **Trains logic** | `crates/core/src/{trains,train_list,train_cars,train_route}.rs` | `trains.ts` (without the canvas atlas), `train-list.ts`, `train-cars.ts`, `train-route.ts`; doc 05 §12.4–12.7 | 61 tests ported (23 + 20 + 6 + 12); operator colour regexes use `(?i)` with ASCII word boundaries `(?-u:\b)`; a test checks "SJ Götalandståg" → row 0 and "Vy" → row 9 (doc 02 §14.3.11) |
| S1.15 | **`hover`, `taxinet`, `liveries`** | `crates/core/src/{hover,taxinet,liveries}.rs` | `hover-details.ts`, `taxinet.ts`, `liveries.ts` (curated table, aliases, military set); docs 05 §9.1, §11.4, 03 §17 | 7 tests ported; new tests for the taxiway grid index and for livery lookup |
| S1.16 | **`flightpath`, `routes`, `airac`** | `crates/core/src/flightpath/*.rs`, `routes.rs`, `airac.rs` | `flightpath.ts` (948), `routes.ts` (134); docs 03 §15, 05 §8 | 15 tests ported (`dubins`, `turnThenStraight` as test helpers); new tests for `decode_waypoints`, `merge_trail`, `route_for`, `airac_cycle`; matches the golden routes of S1.17 to 1e-9° |
| S1.17 | **Golden fixtures from the TypeScript.** A bun script runs `Motion` over each flight of `landings.json` and three synthetic ones at a 100 ms step and writes every `Pose`; and `estimateRoute` on fixed inputs | `scripts/port/golden.ts`, `crates/core/tests/data/golden/*.json` | doc 05 §15 | files exist, are deterministic (two runs are byte-identical), under 5 MB in total |
| S1.18 | **Aircraft motion, part 1**: constants, small functions, `Track`, samples, smoothing spline, fits, Hermite, altitude | `crates/core/src/motion/aircraft/{mod,consts,track,samples,spline,fit,altitude}.rs` | `motion.ts` (doc 05 §10.1–10.7) | the tests of doc 05 appendix A `motion.test.ts` that exercise only these parts pass (the item lists which) |
| S1.19 | **Aircraft motion, part 2**: delay, phase, QNH, prediction, evaluation, blending | `…/{delay,phase,qnh,predict,evaluate,blend}.rs` | doc 05 §10.8–10.11 | further `motion.test.ts` cases pass |
| S1.20 | **Aircraft motion, part 3**: measured data, taxi snap, gear | `…/{measured,taxi_snap,gear}.rs`, `crates/core/tests/motion.rs` | doc 05 §10.12–10.13 | all 46 tests pass; the golden poses of S1.17 match to 1e-6 (positions to 1e-9°) |
| S1.21 | **`core::mvt`**: layers → features (geometry type, rings or lines in tile units, typed properties), a layer-name filter, `num_or_zero` for "absent or 0"; `backend::rail` and `building_lod` switch to it | `crates/core/src/mvt.rs`, the decode parts of `crates/backend/src/{rail,blocks,building_lod}.rs` | `RS:rail.rs:189-340`; doc 04 §17.9 | decodes the tile fixtures in `crates/backend/tests/data`; the existing rail and building tests still pass |
| S1.22 | **Scene, small**: `screen`, `building_shadow`, `hillshade`, `rail_overview`, `street_trees` | one file each in `crates/scene/src/` | `screen.ts`, `building-shadow.ts`, `terrain-layer.ts` (pure part), `rail-overview.ts`, `street-trees.ts`; docs 01 §3.14, 04 §7.6, §14.2, §8.7, 02 §8.3 | 29 tests ported (4 + 10 + 13 + 2); `hillshade` rounds as `Uint8ClampedArray` does |
| S1.23 | **Scene, lookups**: `landcover`, `canopy` (pure part), `overture` (pure part), `streets_gl`, `road_widths` | one file each | `landcover.ts`, `canopy.ts`, `overture.ts`, `streets-gl.ts`, `road-widths.ts`; doc 04 §6.6–6.7, §7.7, §8.6, §11 | 20 tests ported (6 + 2 + 9 + 3) |
| S1.24 | **Geometry kernel, roofs, bridges**: `geom::grid_cut` (port of `cutPolygonByGrid`, holes kept), `geom::densify`, `geom::earcut` (thin wrapper over `earcutr`), then `roofs`, `bridges` | `crates/scene/src/geom/*.rs`, `roofs.rs`, `bridges.rs` | `@math.gl/polygon` `cutPolygonByGrid`; `roofs.ts`, `bridges.ts`; docs 02 §9, §14.3.1, 04 §7.5 | 18 tests ported (8 + 10); new tests: `grid_cut` keeps total area and clips exactly at grid lines; `densify` leaves no edge over 5° |
| S1.25 | **Model data**: `train_models`, `ship_models`, `aircraft_models`, `aircraft_assets` (parsers for `index.json` and `meshes.bin`) | one file each | `train-models.ts`, `ship-models.ts`, `models.ts`, `aircraft.ts`; docs 02 §4.6, §5.6, 03 §4 | 19 tests ported (3 + 16); new tests parse the real `static/aircraft` and `static/trains` files and check counts and bounding boxes |
| S1.26 | **`scene::rail`, `scene::lod`** | `crates/scene/src/{rail,lod}.rs` | `rail-layer.ts` (pure part), `lod-tileset.ts`; docs 02 §8, 04 §13 | 16 tests ported (11 + 5) |
| S1.27 | **Radar meshing**: palette (one definition, shared with `backend::radar`), reflectivity, echo top, coverage, `IsoRings` marching squares, `even_odd`, `build_mesh`, `build_walls`, `polygon_walls`, `simplify_line`, `MeshArena` | `crates/scene/src/radar/{mod,iso,mesh,arena}.rs` | `radar.ts`, `radar-mesh.ts`, `radar-arena.ts`; doc 02 §3 | 6 tests ported; new tests: `reflectivity` inverts the palette; two neighbouring synthetic tiles contour to edges that meet exactly; wall lines skip tile edges |
| S1.28 | **Realistic logic**: `look` (`transmittance`, `look_for`, `display_rgb`, sky gradient stops, `scene_sun`, `roof_colour`, `building_style`), `ground` (`area_paint`, `line_of`, the surface and paint tables), `clouds` strips | `crates/scene/src/{look,ground,clouds}.rs` | `realistic.ts` (pure part), `realistic-ground.ts` (pure part), `clouds.ts`; doc 04 §3, §6.1–6.5, §7.2, §9 | 15 tests ported |
| S1.29 | **Trees and land-cover ground**: `plant_tile`, `hash2`, `leaf_phase`, `tiles_around`, `tree_shadows`, `card_arrays`; `cover_outlines` using `radar::iso` | `crates/scene/src/{trees,landcover_ground}.rs` | `tree-layer.ts`, `tree-models.ts`, `landcover-ground.ts`; doc 04 §8, §11 | 10 tests ported; `hash2` matches JavaScript's 32-bit wrapping on a table of 20 inputs taken from the TypeScript |
| S1.30 | **Fonts.** `cargo xtask vendor-fonts` decodes the ten `.woff2` subsets with `wuff` and writes TTFs plus the OFL licence | `crates/xtask/src/fonts.rs`, `assets/fonts/*` | this doc 1.5; doc 06 §1.2 | a test loads each TTF with `epaint::text::FontData` and finds the `wght` axis (Overpass 100–900, Mono 300–700) |
| S1.31 | **Icons.** `cargo xtask vendor-icons` reads `iconData.node` of each icon in doc 06 §26 and writes one Lucide-format SVG per icon plus the ISC licence | `crates/xtask/src/icons.rs`, `assets/icons/lucide/*.svg` | doc 06 §26 | every icon named in doc 06 §26 has a file; each parses with `resvg` |
| S1.32 | **Golden values from deck.gl.** A bun script builds deck's globe and Mercator viewports for 24 views (zoom 0.5–20, latitudes 0, 45, 60, 80, with pitch and bearing) and writes `project`, `unproject`, view-projection matrices and `getTileIndices` results for tile sizes 256 and 512 | `scripts/port/deck-golden.ts`, `crates/scene/tests/data/deck/*.json` | doc 01 §2; `@deck.gl/core` viewports; `@deck.gl/geo-layers/dist/tileset-2d` | files exist and are deterministic |
| S1.33 | **Reference pictures of today's app.** Build the web app, open it in the Chromium already in this container at 1440×900 and 390×844 with the dev URL hashes (`#@lat,lon,zoom,pitch,bearing`, `#demo`) for 12 fixed views across the three basemaps, and save PNGs. Without the backend there is no live data: these are references for the basemaps and the shell | `scripts/port/reference-shots.mjs`, `docs/port/reference/*.png`, `docs/port/reference/README.md` (view list) | docs 01, 04, 06 | 12 views × 2 sizes saved; the README names each view's hash |
| S1.34 | **CI.** Add a job running `cargo test --workspace --locked` and `cargo clippy` for the new crates with SwiftShader or lavapipe available, and a job running `cargo check` for `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`, `aarch64-apple-ios`, `aarch64-linux-android` on `rustradar-app`. Existing jobs unchanged | `.github/workflows/rust.yml` | doc 07 §14.1 | workflow is valid YAML (`actionlint` if available); jobs are additive |
| S1.35 | *(only if the owner asks, section 9)* **Settings bridge**: the Svelte app writes `settings.json` to the config folder through a new shim command on every change, so a last Tauri release carries settings over | `src/lib/filters.ts`, `src-tauri/src/commands.rs`, `crates/backend/src/settings_file.rs` | docs 06 §23, 07 §17.2 | file appears beside `keys.json` with the schema of doc 06 §23 |

Parallelism

- S1.1 first, alone.
- Backend track, in order: S1.2 → S1.3 → S1.4 (S1.4 also needs S1.5).
- Core track: S1.5 and S1.6 first (in parallel with each other and with the backend track); then S1.7–S1.16 and
  S1.21 in parallel (each owns its own files); S1.14 before S1.13 (`ship_list` uses `train_list`'s `fold` wrapper);
  S1.12 after S1.13 and S1.14; S1.16 after S1.7 and S1.12; S1.18 → S1.19 → S1.20 in order, after S1.15 and S1.17.
- Scene track, after S1.5–S1.7: S1.22, S1.23, S1.24, S1.25, S1.26, S1.28 in parallel; S1.27 after S1.24; S1.29
  after S1.23 and S1.27.
- S1.17, S1.30–S1.34 at any time after S1.1.

End of stage: 387 of the 394 TypeScript tests have Rust twins (the other seven, `trackpad` and `ktx2-transcode`,
land in stage 2), beside the 197 existing Rust tests.

Deferred: anything that draws; `app::persist`, `app::taxiways`, the feed state machines (stages 4–7); the typed
replacement for `isRateLimited`'s text match (kept as text for now, doc 05 §16.11).

### Stage 2 — Map engine foundations

Goal: a window (and a headless path) showing an empty, correctly projected, correctly controlled globe, with every
generic piece later stages need: camera, passes, the eight pipelines, tiles, text, picking, textures, and the app
skeleton with the backend running. The spike's placeholder globe (`crates/app/src/map.rs`) and placeholder panel
are removed; its `gpu.rs`, `renderer.rs` and `headless.rs` move into `crates/map` and are rewritten to the design
of 1.4.

| Id | Item | Files owned | Implements / ports | Acceptance |
|---|---|---|---|---|
| E1 | **Camera.** `ViewState`, `GlobeViewport`, `MercatorViewport`, `viewport_for(view, size)` (swap at zoom 12), `project`, `unproject` (off-limb rays return the nearest limb point), `meters_per_pixel`, camera distance and altitude, `limits()`, `follow_max_zoom` | `crates/scene/src/camera.rs` | doc 01 §2.1–2.4 (matrices verbatim), `@math.gl/web-mercator` for the flat matrices | matches every golden view of S1.32: projected pixels within 1e-6 px, matrices within 1e-9 relative |
| E2 | **Controller.** North-up pan, drag-rotate, wheel zoom about the pointer (anchor strength 1 on the flat map), double-click zoom, pinch, `fly_to`, `jump_to`, `zoom_by`, `reset_orientation`, `set_pitch`, linear transitions with their durations, latitude clamp ±85, the latitude-shifted zoom limits; wheel/trackpad classification | `crates/scene/src/controller.rs` | doc 01 §3.1–3.8, §3.12; `controller.ts`, `trackpad.ts` | 3 `trackpad` tests ported; new tests: a pan keeps the grabbed point under the pointer; panning north changes zoom by `log2(cos new / cos old)`; `zoom_by` clamps to `[0.5, 20]`; transition end states |
| E3 | **Tile selection.** Tile ids, bounds, parent/children; deck's zoom rule; visible-tile selection for both viewports (frustum traversal with per-tile zoom on the globe and above 60° pitch); `best_available` refinement; request priority by distance | `crates/scene/src/tiles.rs` | `@deck.gl/geo-layers/dist/tileset-2d/{utils,tile-2d-traversal,tileset-2d}.js`; doc 01 §5.6, §5.8 | equals the golden tile lists of S1.32 for all 24 views and both tile sizes |
| E4 | **Render core.** `Gpu` (moved), offscreen map target (1.4.1), globals uniform (view-projection, projection mode, centre terms, viewport size, map scale, time), composite pass, WGSL loader, `lib/project.wgsl` with both projection paths, draw-state presets (1.4.5), `Layer` trait, `SceneInput`, `MapRenderer` with the empty ordered layer list, `map::headless::render` | `crates/map/src/{gpu,target,frame,shaders,renderer,headless,state}.rs`, `shaders/lib/project.wgsl`, `shaders/composite.wgsl`, `pipelines/mod.rs` and `layers/mod.rs` (declaring every file of 1.2 as a stub) | this doc 1.4.1–1.4.7 | headless frame is exactly `(30,30,30)` everywhere; test `all_shaders_validate` parses every `.wgsl` with naga; test `projection_precision` (f32 shader formulas re-implemented in Rust against E1) stays under 0.25 px for zoom 0.5–12 on the globe and under 0.01 px for zoom 12–23 on the flat map |
| E5 | **`fill` and `bitmap` pipelines; the earth.** Sphere of radius 6.3e6 m, 36 × 72 segments, colour by basemap, `DEFAULT` state, globe mode only; `land` rectangle ±180, ±85.06 densified to 5° | `crates/map/src/pipelines/{fill,bitmap}.rs`, `layers/earth.rs`, their `.wgsl` | doc 01 §5.4–5.5 | shot V1 (section 8): a disc of `(50,50,50)` on `(30,30,30)`; its edge matches E1's projection of the sphere's limb within 1.5 px; of two shots at the same zoom, the one centred on latitude 60 shows a disc twice as wide as the one centred on the equator (latitude coupling, `1/cos 60°`) |
| E6 | **`path` pipeline** (1.4.6) with px and metre widths, min/max px, miter limit 4, round joints and caps, dashes (array, justified, cumulative along the whole path), 3D paths | `crates/map/src/pipelines/path.rs`, `shaders/path.wgsl`, `crates/scene/src/geom/path.rs` | `@deck.gl/layers` PathLayer vertex shader, `@deck.gl/extensions` PathStyleExtension; docs 01 §5.6, 02 §14.3.9 | shot of a test scene: a 1 px and a 0.6 px line keep their width at zoom 3, 11 and 15 and at pitch 0 and 60 (measured coverage within 15 %); a dashed line shows the expected number of dashes |
| E7 | **`mesh` pipeline and the `shadow` variants** (the `mesh` and `fill` shaders with the `SHADOW` state), with the light rig: ambient white 1.0; camera light `[255,248,235]` × 0.55; key light `[255,244,225]` × 0.7 from direction `[-1,-2,-3]`; Phong material (ambient, diffuse, shininess, specular colour); `aim_key_light(to_sun, weight)`; highlight mix `mix(rgb, highlight.rgb, highlight.a)`; normals transformed by the instance scale and renormalised | `crates/map/src/pipelines/{mesh,shadow}.rs`, `shaders/mesh.wgsl` | docs 01 §4, §9.1, 02 §1.4, §14.3.6, 03 §3.1 | shot: a unit cube instance at a known place shows three faces of different brightness; with material depth 0 the colour is exactly the instance colour; two overlapping shadow quads drawn with the `SHADOW` state darken their overlap once |
| E8 | **`icon` pipeline**: atlas quads, billboard or ground-plane, size in px converted to world size at the icon's depth for ground-plane icons, angle, tint, highlight, alpha cutoff | `crates/map/src/pipelines/icon.rs`, `shaders/icon.wgsl` | `@deck.gl/layers` IconLayer; doc 02 §14.3.8 | shot: a billboard icon stays upright and the same size at pitch 0 and 60; a ground-plane icon foreshortens |
| E9 | **`circle` pipeline**: filled and stroked circles, radius in px or metres, stroke width in px | `crates/map/src/pipelines/circle.rs`, `shaders/circle.wgsl` | ScatterplotLayer; docs 02 §1.4, 03 §8.4 | shot: ring of 2 px stroke `[55,142,240,235]` over a halo `[55,142,240,56]` |
| E10 | **SDF text.** TinySDF port, on-demand glyph atlas, per-character layout (anchors start/middle/end, line offsets), the text pipeline with halo and optional background box | `crates/scene/src/{sdf,text}.rs`, `crates/map/src/pipelines/text.rs`, `shaders/text.wgsl` | deck TextLayer / `@mapbox/tiny-sdf`; docs 01 §5.9, §6, 03 §7.4 | shot: "Stockholm" at 14 px in `[241,238,231,245]` with a dark halo; its ink box is within 1 px of the same string laid out by epaint at 14 px in `ui-600`; a unit test checks the SDF of a filled square is symmetric and 0.75 (1 − cutoff) at the edge |
| E11 | **Declutter.** Greedy placement per group with priorities, claimed rectangles (with per-label padding and an optional size factor of 1.3), fade state per label id (150 ms), the "crisp" curve `smoothstep(0.3, 0.65, fade)` | `crates/scene/src/declutter.rs` | docs 01 §5.9, 03 §7.5 | unit tests: the higher priority wins an overlap; groups do not affect each other; a label that loses fades out over 150 ms |
| E12 | **Picking.** Pick list, point-with-radius, rectangle and oriented-box hits, topmost wins, 6 px radius | `crates/scene/src/pick.rs` | doc 03 §10.1 | unit tests for each hit kind and for draw-order priority |
| E13 | **Tile runtime, worker pool, fetch trait.** `jobs::Pool` (priority, cancel), `io::TileFetcher`, `TileSet<T>` (selection strategy and refinement strategy pluggable, `max_requests`, LRU size, look-ahead exemption, per-frame upload budget) | `crates/map/src/{jobs,io,tiles}.rs` | this doc 1.4.8; doc 04 §13.3 | unit tests with a fake fetcher: at most `max_requests` in flight; a tile that leaves the selection is cancelled; a failed tile leaves its loaded parent visible; LRU evicts the oldest unselected tile |
| E14 | **`backend::tiles`**: cached GET with per-host limits, freshness rules of 1.4.8, 404 as `Ok(None)`, the OpenFreeMap TileJSON lookup, cache size cap | `crates/backend/src/tiles.rs` | docs 01 §5.6, 07 §11.5 | tests against a local `tokio` HTTP listener: second request is served from disk; a stale file is used when the server fails; no test touches the public network |
| E15 | **Textures.** KTX2 → GPU texture (2D and 2D array) through `basisu_c_sys` on the worker pool, target choice, stand-ins, top-mip skipping | `crates/map/src/textures.rs` | docs 04 §5, 03 §4.4; `ktx2.ts`, `ktx2-transcode.ts` | 4 `ktx2-transcode` tests ported; a test transcodes `static/aircraft/icons.ktx2` to RGBA8 and checks its size (896 × 768, 10 mips) and that it is not blank |
| E16 | **Tile fixtures.** A small set of real tiles for headless shots: OpenFreeMap z0–2 world, and the tiles covering views V3–V6 at their zooms; Esri imagery, terrarium and one RainViewer frame for the same views; one Streets GL z16 tile. Downloaded once by `cargo xtask fixtures`, committed with a NOTICE file naming the sources and licences | `crates/xtask/src/fixtures.rs`, `crates/map/tests/data/tiles/**` | section 8 | under 40 MB; a file-backed `TileFetcher` in `crates/map/tests/common` serves them |
| E17 | **App skeleton.** Tokio runtime, `Dirs`, `Backend::start`, `Pending<T>`, the wake proxy, the frame loop of 2.2, input routing, `AppState` with a stub file per feed, `scene_builder/` with a stub per kind, CLI flags (`--view lat,lon,zoom[,pitch[,bearing]]`, `--basemap`, `--size`, `--scale`, `--now`, `--fixture`, `--demo`, `--screenshot`), window attributes of doc 07 §11.7 (1440 × 900, minimum 900 × 600, maximised, hidden until the first frame) | `crates/app/src/{main,lib,app,requests,input,headless}.rs`, `state/mod.rs` and stubs, `scene_builder/mod.rs` and stubs, `platform/mod.rs`; deletes `map.rs`, `gpu.rs`, `renderer.rs` | this doc 2; docs 07 §10.4, §11.7, 06 §3.15 | `--screenshot` renders the globe of E5 through `map::headless`; the binary starts the backend without a panic when run with `--screenshot` and no network; still `cargo check`s for the four cross targets |
| E18 | **Shot harness.** Helpers `pixel`, `assert_color`, `coverage(rect, colour, tolerance)`, `disc_radius`; named views V1–V8; the tests of E5–E10 written against them | `crates/app/tests/{shots.rs,common/*.rs}`, `crates/map/tests/common/*.rs` | section 8 | tests pass with SwiftShader and skip (pass, with a printed reason) without an adapter |
| E19 | **Phone shells, prototype.** Switch winit to `android-game-activity`; copy `src-tauri/gen/android` to `platform/android` and strip Tauri from it (doc 07 §13.4); `platform/ios/project.yml` for xcodegen with a `main.m` calling `rustradar_ios_main`; `platform::android` (directories from `AndroidApp`, safe area from `content_rect`, the text bridge of 1.5, back key → `Action::CloseTop`) and `platform::ios` (directories) | `platform/android/**`, `platform/ios/**`, `crates/app/src/platform/{android,ios}.rs`, `crates/app/Cargo.toml` | docs 07 §8, §13.4–13.5, 06 §29 | `cargo check` for both phone targets; CI builds a debug APK and an unsigned simulator app as artifacts. **Needs a person with devices**: the checklists in section 5 (K3, K4) run on one Android phone and one iPhone; findings recorded in `docs/port/device-notes.md` |

Parallelism: E1 first. Then E2, E3, E4, E11, E12, E14 in parallel. After E4: E5–E10, E13 (also needs E3) and E15
in parallel (each owns one pipeline or module; `mod.rs` files were stubbed by E4). E16 after E14. E17 after E2 and
E5. E18 after E17. E19 after E17.

Deferred: every real layer; the UI beyond an empty egui pass; terrain covers; post effects.

### Stage 3 — Dark and satellite basemaps

Goal: the dark map and the satellite map look like today's at every zoom, with labels, night shading, plain
buildings and complete camera input.

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| B1 | **Vector map tessellation.** Decoded features of source layers `water`, `boundary`, `transportation`, `aeroway` → fills and path segments with the palette, colour and width tables; fills before lines, tile feature order kept; over-zoom from z14; globe subdivision for tiles z ≤ 6 | `crates/scene/src/tess/vector.rs` | doc 01 §5.6 (tables verbatim) | unit tests on fixture tiles: a `water` polygon becomes triangles of `[30,30,30,255]`; a motorway becomes 1.4 px `[88,88,88,255]` segments; maritime boundaries produce nothing visible |
| B2 | **Vector map layer** (`map`), one `TileSet` for both projections, `SURFACE_NO_CULL`, tile source from the TileJSON | `crates/map/src/layers/vector_map.rs`, `crates/app/src/scene_builder/base.rs` | doc 01 §5.1, §5.6 | shots V2, V3, V4 from fixtures: sea pixels `(30,30,30)`, land `(50,50,50)`, border and road pixels present; crossing zoom 12 (V3 at 11.9 and 12.1) requests no new tile; compared by eye with `docs/port/reference/dark-*.png` |
| B3 | **True-width runways**: when `45 / mpp >= 4`, runway centre lines 45 m wide (the px runway width becomes 0) | `crates/map/src/layers/vector_map.rs` (runway part), `crates/scene/src/tess/vector.rs` (runway part) — after B2 | doc 01 §5.7 | shot V4: runway `(163,119,52)` width in px equals `45 / mpp` within 1 px |
| B4 | **Place and airport labels**: label tiles (`place`, `aerodrome_label`), `is_label`, text/size/colour/priority table, horizon filter at zoom ≤ 6 (`cos > 0.42`), marked airports removed, collision group `"labels"` with size factor 1.3, `label_scale`, pick rectangles for airports | `crates/scene/src/labels_map.rs`, `crates/map/src/layers/labels.rs` | doc 01 §5.9 | shots V2 and V3: city names in `(241,238,231)`, airport codes in `(55,142,240)`, no two label boxes overlap; unit test for the priority formula |
| B5 | **Night shading**: four stacked quad sets, `[2,6,14,46]`, recomputed when `floor(feed_now / 60 000)` changes, not on realistic | `crates/map/src/layers/night.rs` | doc 01 §5.10 | shots with `--now` at an equinox noon UTC: at `--view 0,180,1.9` the centre (ocean, full night) is `(15,17,21)` ± 2, which is `(30,30,30)` blended four times toward `(2,6,14)` at alpha 46/255; at `--view 0,0,1.9` an ocean pixel near the centre stays `(30,30,30)` |
| B6 | **Satellite imagery**: Esri World Imagery tiles, 256 px, zoom 0–19, tint `[150,158,180]`, `SURFACE`, Mercator-correct sampling on the globe, earth colour `[6,16,34]` | `crates/map/src/layers/imagery.rs` | doc 01 §5.2, §5.8 | shot V3 `--basemap satellite` from fixtures: mean colour of a sea region is blue-dominant and darker than the raw tile by the tint factors within 3 levels |
| B7 | **Plain buildings** (dark and satellite): `CITY_LOD` tiles (OpenFreeMap z13–14 and the backend's far-LOD tiles), extrusion to walls and roofs, `BUILDING_COLOR [58,68,84]`, material (ambient 0.42, diffuse 0.6, shininess 16, specular `[60,66,80]`), sun-aimed key light, stencil ground shadows | `crates/scene/src/tess/buildings.rs`, `crates/map/src/layers/buildings.rs` | docs 01 §9.3, 04 §7.1, §7.6, §13 | shot V5 `--basemap dark`: lit faces of different brightness; shadow pixels darker than the ground and none darkened twice (no pixel below the single-shadow level) |
| B8 | **Input, complete.** Gesture table (left-drag pan, right-drag or modifier-drag rotate, wheel zoom, shift-wheel quarter speed, double-click); trackpad pixel scroll = pan, ctrl + pixel scroll = zoom; macOS pinch and rotation gestures; touch: one finger pan, two fingers pinch and pan (no twist, no tilt); keys `+ = -` → `zoom_by(±1)` (one binding), `n`, `t`; cursor grab/grabbing/pointer; controller mode switch when locked | `crates/app/src/input.rs` | doc 01 §3.1–3.8; doc 06 §3.13 | unit tests feed synthetic winit-independent input events and check the resulting `ViewState`; **person check** on one desktop OS with a mouse and a trackpad |
| B9 | **Map error card and recovery; basemap switching** (pitch ease to the new maximum over 600 ms; limits re-applied; map scale re-evaluated) | `crates/app/src/ui/map_error.rs`, `crates/map/src/renderer.rs` (rebuild path) | doc 01 §2.4–2.5, §9.5 | a test forces the rebuild path and gets the same shot as before; card text and buttons as specified |

Parallelism: B1 first; B2 after B1; B3 after B2; B4, B5, B6, B7, B8, B9 in parallel with one another once B2 has
landed (B7 also needs E7).

Deferred: rail map, radar, hillshade and terrain (stages 5, 8, 9, 10); Streets GL buildings (stage 8).

### Stage 4 — Aircraft

Goal: live aircraft on the map exactly as today: icons far out, lit 3D models close in, liveries, gear, shadows,
labels, the selected flight's trail, route and markers, picking, hover highlight, camera lock. The panels arrive
in stage 7; until then selection is driven by clicks and by `--fixture` data, and shown on the map only.

Fixtures for this stage: `crates/app/tests/common/fixtures.rs` generates a deterministic synthetic snapshot (200
flights worldwide, 30 around Stockholm, 6 on the ground at Arlanda) and reuses the recorded flights of
`landings.json` for close-up shots; `--now` fixes the feed clock.

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| A1 | **Aircraft feed state**: `refresh`, `refresh_soon`, the view-change trigger (3° or 1.5 zoom levels, 800 ms debounce), intervals 8 s / 15 s, `MIN_GAP_MS` 4 s, back-off ×2 up to 6, `motion.sync_clock`, reference data load, top flights every 60 s, `shown` list (filters applied, selected flight kept), fixture mode | `crates/app/src/state/aircraft.rs` | doc 06 §3.1, §3.4 | unit tests drive the state with a fake clock and canned results: intervals, back-off sequence 1, 2, 4, 6, debounce, "only a successful round moves the freshness clock" |
| A2 | **Aircraft assets on the GPU**: mesh buffers for both LODs from `meshes.bin`, procedural models, the two icon atlases (KTX2), livery colour lookup with the backend's logo colours | `crates/map/src/layers/aircraft/assets.rs`, `crates/app/src/state/liveries.rs` | doc 03 §4, §17 | test: every model in `index.json` uploads; atlas cells of the icon classes are non-empty |
| A3 | **Aircraft scene builder**: the preamble and per-flight loop: `screen_px`, `min_px_at`, `depth_at`, `livery_amount`, `use_icons`, `detail`, cull by `visible_radius`, model lookup, world position with altitude exaggeration, grouping by model, colours (altitude colour, selection blue, liveries per part), standing on the wheels | `crates/scene/src/aircraft.rs`, `crates/app/src/scene_builder/aircraft.rs` | doc 03 §1–2, §5.1–5.3, §6 (CPU parts); doc 01 §9.1 | unit tests for each formula at the thresholds (zoom 3, 6, 9, 10, 11; pitch 5); golden instance counts for the synthetic snapshot at V1, V3, V4 |
| A4 | **Icon layers**: outlines `[8,12,20,215]` then icons, heading quantised by the 20 px step rule, `screen_angle`, sizes, highlight | `crates/map/src/layers/aircraft/icons.rs` | doc 03 §6; doc 02 §1.1 `screenAngle` | shot V2 with the synthetic snapshot: about the expected number of white aircraft-shaped blobs (connected components within 5 % of the instance count on screen); a selected one is `(55,142,240)` |
| A5 | **Mesh layers and gear**: instance transform from a true east/north/up frame (heading, pitch, bank, scale, z-scale `max(0.04, depth)`), per-group and per-part layers, material by depth, gear with `GEAR_DEPTH 0.8` and `[178,182,190]` | `crates/map/src/layers/aircraft/meshes.rs` | doc 03 §5.1–5.3 | shot V4 with aircraft on the ground at Arlanda: models aligned with the runway heading within 2° (measured from the principal axis of the blob); the same aircraft at zoom 11.9 and 12.1 keeps its heading (no half-turn at the projection swap) |
| A6 | **Aircraft shadows**: shadow matrix from the sun at the aircraft, fade over zoom 9–11, colour `[4,8,16]`, `SHADOW` state, all shadows before any aircraft | `crates/map/src/layers/aircraft/shadows.rs` | doc 03 §5.4; `core::sun` | shot: an aircraft at 300 m at local noon has a darker blob under it; at night (`--now`) none |
| A7 | **Aircraft labels**: text and priority (`core::labels`), which aircraft get one (zoom 4, far-side filter at zoom ≤ 6), layout (11 px callsign, 10 px height line above zoom 7, offsets), claimed box with the `spread` and `reserve` formulas, group `"aircraft-labels"`, crisp fade, label-set cache | `crates/scene/src/aircraft_labels.rs`, `crates/map/src/layers/aircraft/labels.rs` | doc 03 §7; doc 01 §9.2 | shot V3 with labels on: callsigns right of their aircraft, none overlapping; unit test of the padding formula at zoom 4, 7, 10 |
| A8 | **Selected flight on the map**: follow stream state (`connecting / live / reconnecting / ended`, auto-resume), `selected` row substitution, OpenSky merge, adsb.lol polling and `motion.measure`, trail with its shadow, route layers (filed, waypoints, before, ahead), origin/destination markers, focused-airport marker, searched-place marker, selection ring | `crates/app/src/state/flight.rs`, `crates/app/src/scene_builder/flight.rs`, `crates/map/src/layers/{route,markers}.rs` | doc 06 §3.5; doc 03 §8, §15; doc 02 §10 | state unit tests for the follow state machine; shot with a `landings.json` flight selected: a 3 px trail whose colour follows `altitude_color` (orange `[255,98,0]` at the ground end), its 1.5 px ground shadow `[241,238,231,60]`, the ring `[55,142,240,235]` at the aircraft, airport dots with codes |
| A9 | **Picking, hover, click**: pick list for icons and meshes, hover highlight, cursor, click rules (aircraft → select; nothing → deselect; non-airport label → nothing), hover loader hookup (`core::hover`; the card itself is U18) | `crates/app/src/state/pick.rs`, `crates/scene/src/aircraft.rs` (pick records) — after A3 | doc 03 §10; doc 01 §7 | unit test: a click 5 px from an icon selects it, 8 px does not; with two overlapping, the later-drawn wins |
| A10 | **Camera lock on an aircraft**: `locked_view`, limits when locked (pitch 85, `follow_max_zoom`, closest 1.3 lengths), unlock easing back within 600 ms, every-frame refresh when locked and zoom > 7 | `crates/scene/src/controller.rs` (follow part), `crates/app/src/state/camera.rs` | doc 01 §2.4, §3.9; doc 03 §11.4 | unit tests of `locked_view` against hand-computed cases; shot: locked on a climbing aircraft keeps it at the screen centre within 2 px |
| A11 | **Taxiways**: which airports' networks are loaded (zoom ≥ 12, LRU of 6, 5 min retry), handing networks to `Motion` | `crates/app/src/state/taxiways.rs` | doc 03 §16; `taxiways.ts` | unit test with a fake backend: the seventh airport evicts the first; a failed load is retried after 5 min |

Parallelism: A1, A2, A3 first, in parallel. Then A4, A5, A6, A7 in parallel (separate files under
`layers/aircraft/`). A8, A10, A11 in parallel after A1. A9 after A3 and A4.

Deferred: the hover card, flight panel and lists (stage 7); terrain offsets (stage 9).

### Stage 5 — Trains

Goal: trains, their cars, the rail map, signal blocks and the selected train's route, as today. Needs a
Trafikverket key at run time; shots use `--demo` (the demo trains of `demo-trains.ts`) and fixture tiles.

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| T1 | **Train state**: camera-settle debounce 600 ms, refresh 10 s, watched set (zoom ≥ 9, 40 nearest), `TrainsChanged` throttle 1500 ms, selection, live stream of the selected train, route loading and reloading (2.5 s / 30 s / 10 s / 3 s rules), key status check and its 30 s retry, operators hidden, demo trains | `crates/app/src/state/trains.rs`, `crates/app/src/dev.rs` | doc 06 §3.3, §3.9; `demo-trains.ts` (doc 02 §13.2) | unit tests with a fake clock for each timing rule; "a shorter answer while loading does not replace a fuller set" |
| T2 | **Train icon atlas and icon layer**: atlas drawn with tiny-skia, icon size, ground-plane icons, tunnel alpha 90, hover highlight, drawn position with the 2 s ease and the hold rule | `crates/scene/src/atlas/trains.rs`, `crates/scene/src/trains.rs`, `crates/map/src/layers/trains.rs`, `crates/app/src/scene_builder/trains.rs` | doc 02 §5.1–5.5, §4.4 (shared motion) | atlas test: every key maps to a non-empty cell of the stated size; shot V3 `--demo`: train icons in their kind colours along the track |
| T3 | **Train cars**: cars laid along frames and tail, plain car meshes, operator models from `static/trains`, minimum width 3 px, tunnel layers (depth always, alpha 90) | `crates/map/src/layers/train_cars.rs`, `crates/scene/src/trains.rs` (cars part) — after T2 | doc 02 §5.5–5.6 | shot at zoom 16 `--demo`: a train of N cars, each within 1 px of `length / mpp`; lighting differs between roof and side; the tail runs backward from the head (doc 02 §14.3.14) |
| T4 | **Rail map**: network from OpenFreeMap tiles (kinds, colours, widths, dashes, scale), overview below zoom 8 from `railroads.bin`, stations, the speed map with labels and its legend data | `crates/map/src/layers/rail.rs`, `crates/scene/src/rail.rs` (draw lists) | doc 02 §8.1–8.4 | shot V3 with the rail map on: main lines visible in the stated colours; at zoom 6 the overview draws |
| T5 | **Signal blocks and signals**: fetching, taken blocks every 500 ms with 2 m hysteresis, the six layers in order, signal names and block labels | `crates/scene/src/blocks.rs`, `crates/map/src/layers/blocks.rs`, `crates/app/src/state/blocks.rs` | doc 02 §6 | unit tests for `taken_blocks` hysteresis; shot from a backend fixture: casing under blocks, taken blocks in their colour |
| T6 | **Selected train's route and selection ring**: dashes, head dashes, way to the next stop, stop spots and names, halo and ring at the drawn position | `crates/map/src/layers/train_route.rs`, `crates/app/src/scene_builder/trains.rs` (route part) — after T2 | doc 02 §5.7, §7 | shot `--demo` with a train selected: dashed route ahead in `[55,142,240,210]`, ring around the train |
| T7 | **Bridges and tunnels**: deck heights, raised tile lines, `level_at` for cars and icons, `on_structures` | `crates/map/src/layers/rail.rs` (structures part) — after T4, `crates/scene/src/bridges.rs` (draw lists) | doc 02 §9 | unit tests: a line crossing a bridge structure gets the deck height along the structure and ramps at its ends; shot at a bridge in the fixture area: the raised line is drawn over the water; a tunnel section draws in the tunnel style |
| T8 | **Picking and tooltips** for trains, signals and airports on the map (egui tooltip styled as doc 03 §10.6) | `crates/app/src/ui/tooltip.rs`, `crates/scene/src/trains.rs` (pick records) | doc 02 §11 | unit test of the pick order ship → train; tooltip texts match the spec strings |
| T9 | **Ground rails and platforms on the realistic map** (data and tessellation only; drawn when stage 8 lands) and the dev-only sightings layer | `crates/scene/src/ground_rails.rs`, `crates/map/src/layers/sightings.rs` | doc 02 §8.6, §13.1 | unit tests on a fixture rail tile: ordinary track is 2.6 m wide with 1 m of ballast each side, transit track 2.2 m with 0.6 m, tunnels and trams give nothing; platforms extrude to 0.6 m |

Parallelism: T1, T2, T4, T5, T9 in parallel; T3 and T6 after T2; T7 after T4; T8 after T2.

Deferred: train list and panel (stage 7); the OpenRailwayMap raster layer is not ported (unused today, section 7).

### Stage 6 — Ships

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| H1 | **Ship state**: settle 600 ms, refresh 60 s, request counter for stale answers, selection, details refresh 60 s | `crates/app/src/state/ships.rs` | doc 06 §3.8 | unit tests with a fake clock |
| H2 | **Ship icon atlas and icon layer**: atlas with tiny-skia, type colours, icon size, alpha 235 / 105 for old positions, billboard icons turned by the projected screen angle, drawn position with ease and hold, model/icon split at quarter-zoom steps | `crates/scene/src/atlas/ships.rs`, `crates/scene/src/ships.rs`, `crates/map/src/layers/ships.rs`, `crates/app/src/scene_builder/ships.rs` | doc 02 §4.1–4.5 | atlas test as T2; shot V6 with a ship fixture: icons in type colours pointing along their headings |
| H3 | **Ship models**: procedural meshes per class, placement and offset, zoom ≥ 12.5 and ≥ 14 px, white mesh colour (alpha 120 when old), material ambient 0.5, diffuse 0.48, shininess 24, specular 40 | `crates/map/src/layers/ship_models.rs`, `crates/scene/src/ships.rs` (models part) — after H2 | doc 02 §4.6 | shot V6 at zoom 15: a hull of `length / mpp` px within 1 px, lit |
| H4 | **Selection halo and ring, picking, tooltip**: ring at the reported position with `max(bow, stern)` | `crates/app/src/scene_builder/ships.rs` (selection part), `crates/scene/src/ships.rs` (pick records) — after H2 | doc 02 §4.8, §11 | shot: halo and ring around the selected ship; pick unit test |

Parallelism: H1 and H2 in parallel; H3 and H4 after H2. Stage 6 can run alongside stage 5.

### Stage 7 — UI shell and panels

Goal: every component of doc 06 in egui, wired to the state machine, on desktop and phone layouts. This stage runs
in parallel with stages 3–6 from the moment E17 has landed; its items own files under `crates/app/src/ui/` and the
state files named.

UI acceptance uses fixture state: `--ui <name>` renders a named `AppState` fixture (no network) to a shot. Each
component item adds fixtures for its states and asserts sizes, positions and colours from doc 06 with the shot
helpers; text is judged on layout, size and colour, not glyph identity. Each item also compares its shot by eye
with the matching picture in `docs/port/reference/` where one exists.

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| U1 | **Theme, fonts, text, animation**: all tokens, the font families per weight, `text::job` with the middle-dot rule, truncation with ellipsis, the easing curve, reduced-motion switch | `crates/app/src/ui/{theme,fonts,text,anim}.rs`, `crates/app/src/assets.rs` | doc 06 §1, §27; this doc 1.5 | shot of a type specimen: each weight renders visibly different stroke thickness (ink coverage rises with weight); `"A · B"` has a gap on both sides of the dot; panel fill `(50,50,50)`, border `(68,68,68)` |
| U2 | **Icons**: resvg rasteriser, cache by (icon, size, stroke, scale), tinting | `crates/app/src/ui/icons.rs` | doc 06 §26 | shot of the icon sheet: every icon non-empty at 14, 16, 18 and 20 px; stroke 2.5 covers more ink than 1.75 |
| U3 | **Widgets A**: `button`, `icon_button`, `badge` (four tones), `skeleton` shimmer (1.4 s), `alert` (error, warn), `spinner` (900 ms), `live_dot` (2 s pulse), tooltip style | `crates/app/src/ui/widgets/{button,icon_button,badge,skeleton,alert,spinner,live_dot,tooltip}.rs` | doc 06 §1.4, §2.8 | shot of a widget gallery: button 32 px high with 12 px side padding and a `(110,110,110)` border; pressed toggle `(55,142,240)` border on 18 % blue |
| U4 | **Widgets B**: `segmented`, `toggle_row` (switch knob 150 ms, 14 px travel), `chips`, `token_input`, `tabs` (underline), `text_field`, `range_slider` | `crates/app/src/ui/widgets/{segmented,toggle_row,chips,token_input,tabs,text_field,range_slider}.rs` | doc 06 §6.1, §11.1, §12.1–12.2 | gallery shot with sizes from the spec; unit tests for token parsing and keyboard handling |
| U5 | **Settings and persistence**: `settings.json` load with the rules of doc 06 §23, debounced atomic save, modes and `also_show`, the Trafikverket key gate, what happens when a kind leaves the map | `crates/app/src/persist.rs`, `crates/app/src/state/settings.rs` | doc 06 §3.2–3.3, §23 | round-trip test; a file written by this code parses as the documented schema; a missing or corrupt file gives defaults |
| U6 | **Desktop shell**: areas and positions, sidebar height solver, primary card (brand, status, toolbar with the active-filter pill, counts, alerts and notes), list card slot, detail panel slot and stacking, panel enter animation, `close_top`, keyboard shortcuts | `crates/app/src/ui/{shell,sidebar}.rs`, `crates/app/src/state/shortcuts.rs` | doc 06 §2.3, §2.8–2.10, §3.11, §3.13 | shot `--ui shell-desktop` at 1440 × 900: sidebar at x 16–336, top 16, bottom 40; detail panel 360 wide at right 16, bottom 28; map visible in the gaps; unit test of the height solver (list shrinks three times as fast, minimum 96) |
| U7 | **ModeBar, attribution strip, dev readout** | `crates/app/src/ui/{modebar,attribution}.rs` | doc 06 §2.6–2.7, §6.2, §24.5 | attribution unit test: for each of the 18 conditions the string appears in the documented order with train credits first; shot of the ModeBar centred between x 352 and width − 352 |
| U8 | **SearchBox** with remote search, geocoding, local train and ship sections, keyboard navigation, search picks | `crates/app/src/ui/search.rs`, `crates/app/src/state/search.rs` | doc 06 §3.7, §5 | state tests for debounce and stale-answer handling; shot of the open dropdown |
| U9 | **Aircraft list and filters**: TopFlights, FilterPanel | `crates/app/src/ui/{lists/flights,filters/aircraft}.rs` | doc 06 §11, §14 | shots of both; filter changes reach `core::filters` (unit test through actions) |
| U10 | **FlightPanel, part 1**: shared panel anatomy, header, photo carousel, overview tab, footer | `crates/app/src/ui/flight_panel/{mod,overview,footer}.rs`, `crates/app/src/ui/panel.rs` | doc 06 §4, §7, §7.1, §7.5 | shots of the loading (skeleton), live and ended states from fixtures |
| U11 | **FlightPanel, part 2**: route, aircraft and history tabs | `crates/app/src/ui/flight_panel/{route,aircraft,history}.rs` | doc 06 §7.2–7.4 | shots per tab; the route legend's gradient uses the altitude stops at 0, 8.9, 17.8, 33.3, 55.6, 77.8, 100 % |
| U12 | **AltitudeChart and ExportMenu**, with saving to Downloads and reveal on desktop | `crates/app/src/ui/{chart,export_menu}.rs` | doc 06 §9, §10; `core::export` | chart shot from a fixture trail; export writes the four formats to a temp folder in a test |
| U13 | **AirportPanel** and the board state | `crates/app/src/ui/airport_panel.rs`, `crates/app/src/state/airport.rs` | doc 06 §3.6, §8.3 | shots of arrivals and departures; "kept alive but hidden while a flight is selected" unit test |
| U14 | **Trains UI**: TrainList, TrainFilterPanel, TrainPanel | `crates/app/src/ui/{lists/trains,filters/trains,train_panel}.rs` | doc 06 §8.1, §12, §15, §24.4 | shots from the demo trains |
| U15 | **Ships UI**: ShipList, ShipFilterPanel, ShipPanel | `crates/app/src/ui/{lists/ships,filters/ships,ship_panel}.rs` | doc 06 §8.2, §13, §16, §24.5 | shots from the ship fixture |
| U16 | **MapControls, LayersMenu, SettingsMenu**, including the 372 px inset when a panel is open and the compass needle | `crates/app/src/ui/{map_controls,menus}.rs` | doc 06 §19 | shots closed, layers open, settings open; toggles reach `Settings` (unit test) |
| U17 | **KeysDialog and ShortcutsHelp**, keys import and export (file dialog on desktop) | `crates/app/src/ui/{keys_dialog,shortcuts}.rs` | doc 06 §20–21; `core::keys_file` | shots; the web-inspector row is absent (section 7) |
| U18 | **HoverCard, images**: the card, its dwell and cache through `core::hover`, `app::images` and `backend::images` (disk cache, decode off-thread, failure memo) | `crates/app/src/ui/hover_card.rs`, `crates/app/src/images.rs`, `crates/backend/src/images.rs` | doc 06 §18.1; doc 03 §10.2–10.4 | card shot from a fixture; image cache tests against a local HTTP listener |
| U19 | **Phone layout**: breakpoint, top bar and its hide animation, BottomSheet with detents and dragging, landscape rules, safe-area padding, popovers as sheets | `crates/app/src/ui/{sheet,phone}.rs` | doc 06 §2.2, §2.4–2.5, §3.12, §17 | shots at 390 × 844 for peek, half and full with a 47 px top and 34 px bottom inset injected; **person check** on the devices of E19 |
| U20 | **DevPanel, update toast (UI only), desktop platform glue**: open URL, reveal file, clipboard | `crates/app/src/ui/{dev_panel,toast}.rs`, `crates/app/src/platform/desktop.rs` | doc 06 §22, §25; doc 07 §11.2 | toast shot; `open_url` and `reveal` compile on the three desktop targets |

Parallelism: U1 and U2 first (in parallel); then U3 and U4 (in parallel); U5 at any time after E17; then U6; then
U7–U18 and U20 in parallel (one file set each); U19 last.

Deferred: accessibility beyond what egui gives by default; exports on phones (section 6).

### Stage 8 — Realistic mode

Goal: `basemap = realistic` as today: the light model, sky, textured ground, buildings with windows and night
glow, shadows, trees, land cover, clouds, live weather, rain, tilt-shift. Order follows doc 04 §16.4. All GLSL is
rewritten to WGSL under the rules of doc 04 §16.2: derivatives and implicit-LOD sampling are taken before any
branch on a per-fragment id (`textureSampleGrad` or sample-then-`select`), `mod` is written out as
`x − y·floor(x/y)`, reversed `smoothstep` edges become `1 − smoothstep(lo, hi, x)`.

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| R1 | **Light model on the GPU, sky, base land.** The `rlight` uniform (192 bytes, offsets of doc 04 §4.1), `lib/rlight.wgsl` (vertex part and fragment library), the per-frame driver (`scene_sun`, weather mix above 3–10 km, `look_for`), the sky pass, `ground-land` | `crates/map/src/shaders/lib/rlight.wgsl`, `shaders/sky.wgsl`, `layers/{sky,ground/land}.rs`, `crates/map/src/look.rs`, `crates/app/src/scene_builder/realistic.rs` | doc 04 §1 (driver), §3, §4, §3.6; doc 03 §12 | shot V2 `--basemap realistic` with the sunny preset at local noon: a land pixel is within 4 levels of the CPU reference from `scene::look` (doc 04 §3.5); the sky's top and horizon colours match the gradient stops within 4 levels; a night `--now` gives a dark sky |
| R2 | **Ground shader and the globe ground.** Paint encoding as explicit attributes (`surface_id`, tint), the surface table as a uniform array owned by Rust, areas and lines (metre widths, minimum px, line UVs), procedural water, fields, snow, wetness; `ground-globe` from OpenMapTiles z0–14; ground texture arrays | `crates/map/src/shaders/ground.wgsl`, `layers/ground/{mod,vector}.rs`, `crates/scene/src/tess/ground.rs` | doc 04 §5, §6.1–6.5, §6.8, §6.9 | shot V3 realistic: water, woods, farmland and roads distinguishable and in the hues of `docs/port/reference/realistic-*.png`; `all_shaders_validate` passes; the rain preset darkens paved surfaces |
| R3 | **City ground with distance LOD.** `GROUND_LOD` (levels 9–14 and 16, bias 0.5, 80 km), `keep_coarser_until_in`, look-ahead, Streets GL z16 source and its tag rules, the three-pass order | `crates/map/src/layers/ground/city.rs`, `crates/backend/src/streets_gl.rs` | doc 04 §6.6, §6.9, §13 | shot V5 realistic from fixtures: near tiles come from z16, far ones from coarser levels, no gap and no place drawn twice (a tile-id debug overlay test) |
| R4 | **Realistic buildings**: `building_style` per building, walls as explicit quads with (along, up), flat roofs, the building shader (façades, window bays, night glow), Streets GL buildings at z16, far tiles as today (section 7) | `crates/map/src/shaders/building.wgsl`, `layers/buildings_realistic.rs`, `crates/scene/src/tess/buildings.rs` (realistic part) | doc 04 §7.1–7.3 | shot V5 realistic by day: textured façades with windows; by night: lit windows brighter than walls |
| R5 | **Foot shade and building shadows on the realistic map**: 5 m wall-outline paths, shadow parallelograms, shadow colour `[6,12,28]`, strength `round(sun_strength·1.55·20)/20`, foot shade 0.42, order foot → shadow → buildings per tile, haze fade | `crates/map/src/layers/building_shadows.rs`, `shaders/overlay.wgsl` | doc 04 §7.4, §7.6, §12; doc 01 §9.3 | shot V5 at a low sun: shadows point away from the sun azimuth within 3°; overlapping shadows are not darker than a single one |
| R6 | **Pitched roofs and Overture heights**: roof meshes, `backend::overture` (catalogue, PMTiles ranges, z14, cache 48 tiles) | `crates/map/src/layers/roofs.rs`, `crates/backend/src/overture.rs` | doc 04 §7.5, §7.7 | roof shot from the Streets GL fixture tile; `overture` tests against a small local PMTiles file |
| R7 | **Trees**: planting on the worker pool, tile cache, the tree and tree-shadow shaders, canopy heights (`backend::canopy`: COG range reads), registered street trees, radius rule, shadow alpha rule | `crates/map/src/shaders/{tree,tree_shadow}.wgsl`, `layers/trees.rs`, `crates/backend/src/canopy.rs` | doc 04 §8; doc 03 §3 row 8 | shot V5 at zoom 16 in a park: tree cards with alpha-tested silhouettes and ground shadows; instance count for the fixture tile equals `plant_tile`'s |
| R8 | **Land cover**: `backend::landcover` (NMD WMS colour decode, WorldCover through Planetary Computer, 96-tile cache), `ground-land-cover` layer | `crates/backend/src/landcover.rs`, `crates/map/src/layers/ground/landcover.rs` | doc 04 §11 | decode tests on fixture PNGs; shot: unmapped forest shows as woods |
| R9 | **Road widths, ground rails, platforms**: NVDB widths applied at zoom ≥ 14, rails from T9, platforms with `PLATFORM_LOD` | `crates/map/src/layers/ground/{rails,roads}.rs` | doc 04 §6.7; doc 02 §8.6 | shot at a station in the fixture area: ballast and platforms present |
| R10 | **Realistic globe**: Earth picture and night lights below the vector ground, per-fragment day/night, clouds (`backend::clouds`, hourly; opacity `1 − smoothstep(5.5, 6.5, zoom)`; 10 km altitude) | `crates/map/src/shaders/cloud.wgsl`, `layers/clouds.rs`, `layers/ground/earth_picture.rs`, `crates/backend/src/clouds.rs` | doc 04 §4 (globe terms), §5.1, §9 | shot V1 realistic: continents from the Earth picture; the night side shows city lights; clouds from a fixture JPEG |
| R11 | **Weather and post effects**: live weather state (10 min, 15 km, 2 min retry), presets, rain streaks, tilt-shift blur (2 px to 42 %, 5 px to 22 % of the height from top and bottom), the 1.5 map-scale cap | `crates/app/src/state/weather.rs`, `crates/map/src/post.rs`, `shaders/post.wgsl` | doc 04 §2, §10; doc 06 §3.10; doc 01 §1.4, §2.5 | state tests with a fake clock; shot with the rain preset shows streaks; with tilt-shift on, edge sharpness at the top and bottom rows is lower than at the centre |
| R12 | **Hillshade**: terrarium tiles z0–12, CPU hillshade on the worker pool, opacity table (dark 1, satellite 0.45, realistic 0.6), zoom fade, haze fade; on realistic it is always drawn | `crates/map/src/layers/hillshade.rs` | doc 04 §12, §14.2 | shot of the Alps from fixtures: relief visible; opacity follows the table (measured contrast ratio between basemaps) |

Parallelism: R1 first; R2 after R1; then R3, R10, R11, R12 in parallel; R4 after R3; then R5, R6, R7, R8, R9 in
parallel.

Deferred: draping onto 3D terrain (stage 9); a tree LOD (only if the performance budget of section 5 is missed).

### Stage 9 — Terrain

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| Z1 | **Terrain surface**: Mapterhorn tiles with the Mapzen fallback on 404, terrarium decode, RTIN mesh (error 2 m, 513² grid with back-filled edge), `SURFACE_LOD`, sea-level clamp, mesh colour `[34,39,48]`, growing elevation range | `crates/scene/src/terrain/martini.rs`, `crates/map/src/layers/terrain/{mod,mesh}.rs`, `crates/backend/src/terrain.rs` | doc 04 §14.1, §14.3 | martini unit tests (flat tile → 2 triangles; error bound respected on a synthetic ridge); shot of the Alps at pitch 60: a silhouette against the sky |
| Z2 | **Cover textures**: per-terrain-tile orthographic passes for `Drape::Cover` layers, cache, invalidation, two re-renders per frame, memory budget, the terrain shader sampling the cover and adding haze | `crates/map/src/layers/terrain/cover.rs`, `shaders/terrain.wgsl`, `crates/scene/src/terrain/cover.rs` | doc 04 §14.3, §14.5; this doc 1.4.12 | shot with terrain on: roads and water follow the relief; turning terrain off and on gives the same picture; cover memory stays under the budget (counter test) |
| Z3 | **Standing on the terrain**: CPU height lookup for buildings, trees, vehicles, labels, markers and rings; hillshade opacity 1.4 on realistic with terrain; the `terrain` setting on dark and satellite | `crates/scene/src/terrain/heights.rs`, the instance builders' height hook | doc 04 §14.3, §14.5; doc 03 §9 | shot: buildings in a hilly fixture area sit on the surface (their base pixels meet the terrain, no floating gap); unit test of bilinear lookup |
| Z4 | **Camera ground height**: target height eased 30 % per frame toward `ground_height` at zoom > 12 when not locked, 16-tile cache at zoom 12 | `crates/app/src/state/camera.rs` (ground part) | doc 04 §14.4; doc 01 §3.11 | unit test of the ease; shot at a mountain village: the view centre is the same ground point with terrain on and off within 3 px |

Parallelism: Z1 first; Z2 and Z4 in parallel; Z3 after Z2.

### Stage 10 — Weather radar

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| W1 | **RainViewer source**: frame list every 10 min, tile URL, 256 px tiles zoom 0–7, PNG → dBZ → contours → mesh on the worker pool, cache keyed with the intersecting coverage keys | `crates/backend/src/rainviewer.rs`, `crates/map/src/layers/radar/tiles.rs` | doc 02 §3.2, §3.4; doc 01 §5.11 | job test on a fixture PNG: bands and triangle counts stable; two adjacent fixture tiles meet without a gap |
| W2 | **Ground radar layer and arenas**: flat band polygons at opacity 0.55, `SURFACE_NO_CULL`, GPU arenas (growth 2, minimum 65 536 vertices) | `crates/map/src/layers/radar/{ground,arena}.rs` | doc 02 §3.5 | shot V2 with the radar fixture: NWS palette colours at 55 % over the map |
| W3 | **Lifted layers and walls**: `lift_for(pitch)`, levels, opacity `max(0.16, 0.34 − 0.03·i)·lift`, level-major order, `RADAR_LIFTED` state | `crates/map/src/layers/radar/lifted.rs` | doc 02 §3.5; doc 01 §5.11 | shot at pitch 45: stacked translucent layers with walls; at pitch 10 none |
| W4 | **Measured volumes** (MRMS, DWD): the backend hands over band polygons as structs (the byte encoding and `parseVolumePolygons` are dropped), meshing on the worker pool, ground and level layers | `crates/backend/src/radar.rs` (API), `crates/map/src/layers/radar/measured.rs` | doc 02 §3.3, §3.5 | backend tests unchanged in meaning; shot from a recorded volume fixture |
| W5 | **Radar state**: polling (RainViewer 10 min, measured 120 s for the view area), RainViewer blanked under measured coverage, arenas freed when the setting is off | `crates/app/src/state/radar.rs`, `crates/app/src/scene_builder/radar.rs` | doc 02 §3.6, §14.2 | state tests with a fake clock |

Parallelism: W1 and W4 in parallel; W2 after W1; W3 after W2; W5 at any time. Stage 10 needs only stage 3 and can
run alongside stages 8 and 9.

### Stage 11 — Packaging, CI for five platforms, updater

| Id | Item | Files owned | Implements | Acceptance |
|---|---|---|---|---|
| P1 | **Release tooling in Rust**: `cargo xtask bump [patch\|minor\|major\|x.y.z\|--check]`, `version`, `latest-json`, `sign`, `testflight-age`; `release-version.sh` reads the version through `cargo xtask version` | `crates/xtask/src/{bump,manifest,sign,testflight}.rs`, `scripts/release-version.sh` | doc 07 §9.3, §15.1 | unit tests for version arithmetic and for the manifest shape of doc 07 §7.2 |
| P2 | **Updater, core**: manifest parsing, platform key lookup (specific key first, then `<os>-<arch>`), semver compare, download with progress, signature check with the compiled-in public key | `crates/app/src/update/{mod,manifest}.rs` | doc 07 §7.2, §12.2 | tests against a local HTTP listener with a test key: a correctly signed download verifies; a tampered one is refused; a test that is ignored by default (it needs the network) downloads one published release artifact with its `.sig` and verifies it with the compiled-in public key |
| P3 | **Updater, install and flow**: launch check with the 8 s timeout, splash window (380 × 150, strings and progress rule), install per OS (NSIS run, `.app` swap with rollback, AppImage replace through `$APPIMAGE`), relaunch with `--updated`, hourly watch, `pending_update`, `restart_to_update`, toast wiring | `crates/app/src/update/{install,splash}.rs`, `crates/app/src/ui/toast.rs` (wiring) | doc 07 §5.2, §7.1, §12.2 | install logic unit-tested on temp folders (macOS swap and rollback, AppImage replace); "an update is never a reason not to start": every failure path opens the app |
| P4 | **Android update notice**: hourly release check, `UpdateReady`, open the APK URL | `crates/app/src/update/android.rs` | doc 07 §7.3, §12.3 | `finds_a_newer_apk` passes |
| P5 | **Windows package**: `build.rs` with icon, version info and a per-monitor-v2 manifest; cargo-packager NSIS and MSI with Tauri's identity (product name, per-user location, uninstall key) | `crates/app/build.rs`, `Packager.toml` (Windows part) | doc 07 §13.1, §13.6 | CI artifact installs on Windows (**person check**) |
| P6 | **macOS package**: `.app` for both architectures, `Info.plist` fields of doc 07 §13.2 (minimum macOS 11), ad-hoc signature, `.dmg`, `.app.tar.gz` | `Packager.toml` (macOS part), `platform/macos/Info.plist` | doc 07 §13.2 | CI artifacts open on macOS (**person check**) |
| P7 | **Linux packages**: `.deb`, `.rpm`, AppImage, `.desktop` file, icons, app id `rustradar` | `Packager.toml` (Linux part), `platform/linux/*` | doc 07 §13.3 | `.deb` installs in an `ubuntu:22.04` container and the binary starts with `--screenshot` |
| P8 | **Android release build**: cargo-ndk for four ABIs, Gradle release, signing with the existing keystore secrets, `versionCode = major·1 000 000 + minor·1000 + patch`, APK name `RustRadar_{version}.apk` | `platform/android/**`, CI steps | doc 07 §8.1, §13.4 | CI produces a signed APK; `aapt dump badging` shows `com.felix.rustradar` and the expected `versionCode` |
| P9 | **iOS release build**: xcodegen project, unsigned IPA, TestFlight archive and upload (deployment target iOS 15) | `platform/ios/**`, CI steps | doc 07 §8.2, §13.5 | CI produces the unsigned IPA; the TestFlight path is exercised on a tag (**person check**) |
| P10 | **Workflows rewritten**: `rust.yml` (check, bundle matrix, release job that builds one `latest.json` and creates the release once, android, ios), `bump.yml`, `testflight-refresh.yml` with detection of a Tauri tree during the transition | `.github/workflows/*.yml` | doc 07 §9, §14 | workflows lint clean; a dry run on a branch produces all artifacts |
| P11 | **Upgrade rehearsal** (**person**): publish a native build as a pre-release in a test repository with the real manifest format; on machines with the current Tauri release installed, let the old app update itself on Windows, macOS and from an AppImage; install the native APK over the Tauri APK | `docs/port/upgrade-rehearsal.md` (results) | doc 07 §12.1, §17 | all four upgrades succeed with keys kept; only then may a native build go to `releases/latest` |

Parallelism: P1, P2, P4 any time after stage 2; P3 after P2; P5–P9 in parallel after stage 7; P10 after P5–P9;
P11 last.

### Stage 12 — Removal of TypeScript and Tauri

Entry condition: the parity checklist of section 4 is complete and P11 has passed.

| Id | Item | Files | Acceptance |
|---|---|---|---|
| D1 | Move `static/**` to `assets/**` (drop `updater.html`), update `map::assets` and `app::assets`, update the offline scripts' output paths | `assets/**`, two `assets.rs` files, `scripts/*` | workspace builds; all shots unchanged |
| D2 | Delete `src/`, `src-tauri/`, `package.json`, `bun.lock`, `patches/`, `svelte.config.js`, `vite.config.js`, `tsconfig.json`, `scripts/version.mjs`, `scripts/testflight-age.mjs`, `scripts/port/`; rename package `rustradar-app` to `rustradar` (binary `rustradar`), carrying the version on from the last Tauri release | repository root, `crates/app/Cargo.toml`, `Cargo.lock` | `git ls-files '*.ts' '*.svelte' '*.js' '*.mjs'` lists only the offline asset generators kept under `scripts/` (section 9, question 7); `cargo tree` has no `tauri`, `wry` or `webkit` |
| D3 | `README.md`, `PRODUCT.md`, `.gitignore`, third-party notices (Overpass OFL, Lucide ISC, Streets GL MIT, Basis Universal Apache-2.0, tile fixtures) | documents | build instructions work from a clean clone |
| D4 | Final parity audit: walk section 4 against the reference pictures and the last Tauri build; record every remaining difference in section 6 | this document | signed off by the owner |

---

## 4. Parity checklist

Tick a line when the native app does it and the item's acceptance has passed. "Spec" is where the exact numbers
are.

**Camera and input**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☑ | Globe at zoom ≤ 12, flat Web Mercator above, hard swap, same scale at the centre | 01 §2 | E1 |
| ☑ | Zoom/latitude coupling (globe grows toward the poles, pan changes zoom, shifted limits) | 01 §2.2, §2.4 | E1, E2 |
| ☐ | Limits: zoom 0.5–20, pitch 60 (75 realistic, 85 locked), latitude ±85 | 01 §2.4 | E2, A10 |
| ☐ | Pan (north-up), rotate and tilt, wheel zoom about the pointer, shift-wheel, double-click | 01 §3.2–3.5 | E2, B8 |
| ☐ | Trackpad scroll pans; pinch and twist on macOS; touch pan and pinch | 01 §3.7–3.8 | B8 |
| ☐ | `fly_to` 1500 ms, `zoom_by` 250 ms, reset orientation 400 ms, tilt to 55°; linear, no inertia | 01 §1.3, §3.6 | E2 |
| ☑ | Scene refresh pacing 0 / 50 / 150 / 500 ms | 01 §2.6 | E17 |
| ☐ | Cursor: grab, grabbing, pointer | 01 §1.4 | B8 |
| ☐ | Lock on an aircraft; follow a train or ship | 01 §3.9–3.10 | A10, T1, H1 |

**Basemaps**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☐ | Earth sphere colour per basemap; land rectangle | 01 §5.4–5.5 | E5 |
| ☐ | Dark vector map: water, borders, roads, aeroways with the palette and widths | 01 §5.6 | B1, B2 |
| ☐ | True-width runways | 01 §5.7 | B3 |
| ☐ | Satellite imagery with tint; no borders or roads on it | 01 §5.8 | B6 |
| ☐ | Place names and airport codes, sizes, colours, priorities, declutter, label size setting | 01 §5.9 | B4 |
| ☐ | Night shading in four twilight steps | 01 §5.10 | B5 |
| ☐ | Plain 3D buildings with shadows | 01 §9.3, 04 §7 | B7 |
| ☐ | Map error card with copy details and retry | 01 §9.5 | B9 |

**Aircraft**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☐ | Live feed with intervals, back-off, regional or world view | 06 §3.4 | A1 |
| ☑ | Motion model: smoothing, delay, prediction, flare, taxi snap, gear | 05 §10 | S1.18–S1.20 |
| ☐ | Icons with outlines below zoom 11 at flat pitch; size floor 8–24 px | 03 §2, §6 | A3, A4 |
| ☐ | 3D models, two LODs, depth and material by zoom, landing gear | 03 §5 | A5 |
| ☐ | Liveries fading in over zoom 6–8 | 03 §17 | A2, A3 |
| ☐ | Altitude colours and altitude exaggeration (full ≤ zoom 6, true ≥ 11) | 02 §1.1, 03 §1.2 | A3 |
| ☐ | Ground shadows, zoom 9–11 fade, one shadow per pixel | 03 §5.4 | A6 |
| ☐ | Callsign labels, height line above zoom 7, declutter | 03 §7 | A7 |
| ☐ | Selected flight: trail with shadow, filed and estimated route, waypoints, airport markers, ring | 03 §8, §15 | A8 |
| ☐ | Follow stream states and auto-resume; OpenSky merge; adsb.lol measured data | 06 §3.5 | A8 |
| ☐ | Click to select, click empty map to deselect, hover highlight | 03 §10 | A9 |
| ☐ | Taxiway networks near the view | 03 §16 | A11 |

**Trains**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☐ | Trains in view, watched set, live updates, key gate | 06 §3.3, §3.9 | T1 |
| ☐ | Icons by kind and operator, tunnel fade, eased motion that never runs backward | 02 §5.1–5.5 | T2 |
| ☐ | Cars and operator models close in | 02 §5.6 | T3 |
| ☐ | Rail map: network, speed map with legend, overview, stations | 02 §8 | T4 |
| ☐ | Signal blocks and signals | 02 §6 | T5 |
| ☐ | Selected train's route, stops, ring | 02 §5.7, §7 | T6 |
| ☐ | Bridges and tunnels | 02 §9 | T7 |
| ☐ | Tooltips for trains and signals | 02 §11.4 | T8 |

**Ships**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☐ | Ships in view, refresh, details | 06 §3.8 | H1 |
| ☐ | Icons by type, old positions faded, eased motion | 02 §4.1–4.5 | H2 |
| ☐ | 3D hulls from zoom 12.5 | 02 §4.6 | H3 |
| ☐ | Selection halo and ring, tooltip | 02 §4.8, §11 | H4 |

**Weather radar**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☐ | RainViewer bands on the ground at 55 % | 02 §3.2, §3.5 | W1, W2 |
| ☐ | Lifted 3D stack with walls when tilted | 02 §3.5 | W3 |
| ☐ | Measured volumes (MRMS, DWD), RainViewer blanked under them | 02 §3.3 | W4, W5 |

**Realistic map and terrain**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☐ | Light model, sun, sky gradient, weather presets and live weather | 04 §2–4 | R1, R11 |
| ☐ | Textured ground: areas, roads, water, fields, snow, wetness | 04 §6 | R2, R3 |
| ☐ | Distance LOD tiles with look-ahead and no double drawing | 04 §13 | R3 |
| ☐ | Buildings: façades, windows, night glow, pitched roofs, Overture heights | 04 §7 | R4, R6 |
| ☐ | Foot shade and building shadows | 04 §7.4, §7.6 | R5 |
| ☐ | Trees with shadows, canopy heights, street trees, seasons | 04 §8 | R7 |
| ☐ | Land cover; road widths; ground rails and platforms | 04 §6.7, §11; 02 §8.6 | R8, R9 |
| ☐ | Earth picture and night lights on the globe; clouds | 04 §5.1, §9 | R10 |
| ☐ | Rain streaks; tilt-shift; map scale cap 1.5 | 04 §10; 01 §1.4, §2.5 | R11 |
| ☐ | Hillshade on all basemaps | 04 §14.2 | R12, Z3 |
| ☐ | 3D terrain surface, draped ground, objects standing on it, camera ground follow | 04 §14 | Z1–Z4 |

**UI**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☐ | Tokens, fonts and weights, icons | 06 §1, §26 | U1, U2 |
| ☐ | Desktop shell: sidebar, ModeBar, detail panel, map controls, attribution | 06 §2.3–2.10 | U6, U7 |
| ☐ | Modes and "also show"; trains gated by the key | 06 §3.3 | U5, U7 |
| ☐ | Search: flights, airports, places, local trains and ships | 06 §5 | U8 |
| ☐ | Aircraft list and filters | 06 §11, §14 | U9 |
| ☐ | Flight panel: overview, route, aircraft, history, photos, footer, follow | 06 §7 | U10, U11 |
| ☐ | Altitude chart; export KML, GPX, CSV, GeoJSON | 06 §9–10 | U12 |
| ☐ | Airport board | 06 §8.3 | U13 |
| ☐ | Train list, filters, panel; ship list, filters, panel | 06 §8.1–8.2, §12–13, §15–16 | U14, U15 |
| ☐ | Layers menu, settings menu, map controls, compass | 06 §19 | U16 |
| ☐ | Keys dialog with sign-in, import and export; shortcuts help; all shortcuts | 06 §3.13, §20–21 | U6, U17 |
| ☐ | Hover card with photo; map tooltips | 06 §18.1; 03 §10 | U18, T8 |
| ☐ | Phone layout: top bar, bottom sheet, landscape, safe areas, back gesture | 06 §2.4–2.5, §17 | U19, E19 |
| ☐ | Dev panel and dev readout (dev builds) | 06 §2.7, §25 | U7, U20 |
| ☐ | Settings persisted with the documented schema | 06 §23 | U5 |

**Platform and delivery**

| ☐ | Feature | Spec | Item |
|---|---|---|---|
| ☑ | Same config and cache folders on desktop; `keys.json` carried over | 07 §6, §11.1 | S1.3, E17 |
| ☐ | Open links, reveal exported files | 07 §11.2 | U20 |
| ☐ | Desktop self-update with splash, hourly check, toast | 07 §7.1, §12 | P2, P3 |
| ☐ | Android update notice | 07 §7.3 | P4 |
| ☐ | Installers: NSIS, MSI, dmg, deb, rpm, AppImage, APK, IPA / TestFlight | 07 §13 | P5–P9 |
| ☐ | Old Tauri installs update themselves to the native build | 07 §12.1 | P11 |

---

## 5. Risk register

| Id | Risk | Likelihood / impact | Mitigation | Trigger and fallback |
|---|---|---|---|---|
| K1 | **Android devices without working Vulkan.** The webview hid GPU differences behind ANGLE. wgpu must fall back to GLES 3 | medium / high on affected devices | Baseline is WebGL2-class limits with no optional feature (1.4.13); no compute or storage buffers anywhere; adapters are requested as Vulkan first, then GL; a setting and the environment variable `RUSTRADAR_BACKEND` force GL; ETC2 texture target exists for GLES | If Vulkan initialisation or the first frame fails, restart rendering on GL once and remember the choice in `settings.json` |
| K2 | **Desktop driver variance** (old DX12 hardware, Linux without Vulkan) | low / medium | Backend order DX12 → Vulkan → GL on Windows, Vulkan → GL on Linux; the map error card reports adapter name and backend; `--backend` flag | – |
| K3 | **Surface lifecycle on iOS and Android** (suspend, resume, rotation, lost surface). The spike's handling compiles but has never run | high / high | E19 device checklist: background and foreground 20 times, rotate, lock, split screen, low-memory kill; drop the surface on `suspended`, recreate on `resumed`, reconfigure on `Outdated`/`Lost`; keep CPU caches so GPU state can be rebuilt | If recreation is unreliable on a driver, rebuild the whole `MapRenderer` on resume (slower, always correct) |
| K4 | **Text input, safe areas and the back gesture on phones.** winit 0.30 does not deliver IME text on Android | high / high (search and keys need typing; Swedish letters) | GameActivity plus the own text bridge (1.5); iOS through winit key events; insets from `content_rect` (Android) and egui-winit (iOS). Checklist: type "Göteborg" and "Åre", paste, hold backspace, hide and show the keyboard, rotate with it open, notch and home-indicator padding, back closes the top panel then leaves the app | If the bridge fails on a device class: a small native text dialog (Android `EditText` through JNI) for the search field |
| K5 | **Map labels**: SDF quality at 11–14 px, glyph atlas growth, scripts Overpass lacks | medium / medium | TinySDF settings identical to today's; atlas rebuilt when full (least recently used glyphs dropped); labels prefer `name_en`, then `name:latin`; characters without a glyph are skipped | If small sizes look soft against the reference pictures, raise the atlas glyph size to 96 px (memory ×2.25) |
| K6 | **Stencil shadows** across MSAA, GLES and the terrain cover passes | low / medium | One `Depth24PlusStencil8` attachment for the map pass and one per cover size class; shadow order fixed in `MapRenderer`; the "no pixel darkened twice" shot checks (B7, R5) | – |
| K7 | **Basis transcoder cross builds.** `basisu_c_sys` compiles C++ and runs bindgen; verified on Linux x86-64 only | medium / medium | E15 and E19 build it for all four cross targets in CI early; cargo-ndk supplies the sysroot for bindgen | If a target cannot be built within a day's effort: `cargo xtask bake-textures` transcodes on the host to BC7 and ASTC payloads in a trivial container (header + mip-major data, zstd) loaded without C++, behind the cargo feature `baked-textures` (roughly 25–40 MB more per binary) |
| K8 | **Performance**: per-frame motion for thousands of aircraft, tile tessellation, up to 24 000 trees per tile, declutter, 4× MSAA on phones, about 110 MB of textures in the RGBA8 fallback | medium / high on phones | Budgets: 16 ms per frame on desktop and 33 ms on phones at view V5 realistic; frame time shown in the dev readout; all tile work off the main thread with an upload budget; knobs in order: MSAA off on phones, tree radius halved, `map_scale` capped at 1.0, top mips skipped | If V5 realistic misses the phone budget after the knobs, add a tree distance LOD (not present today) |
| K9 | **WGSL rules and shader translation** (uniform control flow, reversed `smoothstep`, naga's HLSL/MSL/GLSL output) for the 2 000 lines of realistic GLSL | high / medium | Rules of doc 04 §16.2 applied mechanically; `all_shaders_validate`; shots compared with the reference pictures; each realistic item runs its shot on Vulkan here and is rechecked on Metal and DX12 by a person | – |
| K10 | **Stranding installed copies**: the first native release must be installable by the old Tauri updater; Android needs the same key and a `versionCode` that does not drop | medium / very high | Same manifest format, key, endpoint, identity (doc 07 §13.6); cargo-packager's Tauri-derived installers; P11 rehearsal before anything reaches `releases/latest` | If the old updater cannot install the native NSIS or MSI package, ship one more Tauri release whose only change is an updater that can |
| K11 | **Motion model fidelity** (3 400 lines of numerics with sentinels and shared scratch) | medium / high | Transliteration, 46 tests, golden poses from the TypeScript (S1.17) | – |
| K12 | **Third-party endpoints**: `tiles.streets.gl` (one person's server, z16 only), keyless Esri imagery, RainViewer's palette and API, Mapterhorn, EUMETView, Planetary Computer, source.coop, OpenFreeMap | medium / medium (already true today) | All through `backend` with disk caches, a polite User-Agent, per-host limits and back-off; every layer degrades to "not drawn"; attribution strings carried over | – |
| K13 | **egui text and layout fidelity** against Chromium | certain / low | Judge on layout, sizes and colours; exact weights through the variable axis; reference pictures for side-by-side review | – |
| K14 | **Binary size and build time**: 27 MB of assets per binary and per Android ABI; wgpu and naga add about 70 s to a cold build; LTO | certain / low | Accept for the first release; measure APK size in P8 | If the APK exceeds 150 MB, move assets to the APK's `assets/` folder (read through `AAssetManager`) |
| K15 | **`tokio::spawn` outside the runtime** aborts the process | medium / high | `backend::rt()`; the grep check in S1.3; a debug assertion in `rt()` | – |
| K16 | **Remote image hosts** may want a Referer or a browser-like User-Agent | medium / low | U18 tries each host (JetPhotos, FR24 logos, VesselFinder, Wikimedia) and records the headers needed in `backend::images` | A host that refuses shows the placeholder |
| K17 | **Zone names and locale**: abbreviations differ from the tz database's | low / low | Generated table from today's code (S1.11); English formatting everywhere | – |
| K18 | **Device and OS coverage for checks**: agents can only run Linux headless | certain / medium | Items marked **person check**; CI builds every target from stage 1 on | – |

---

## 6. Deliberate deviations and the post-parity list

Known differences from today's app that this plan accepts:

1. Models use a true east/north/up frame; deck's 180° yaw on the globe and its flipped shadow sun are not
   reproduced. The picture is meant to be the same; A5 checks headings on both sides of zoom 12.
2. One tile cache for both projections: the vector map no longer reloads or flashes at zoom 12.
3. With 3D terrain on, the selected flight's trail and route stay at altitude (today they are probably flattened
   onto the ground, doc 03 §20.2).
4. Label decluttering is a CPU greedy pass; which of two equal neighbours wins can differ.
5. Signal names and block labels use Overpass Mono, speed labels Overpass at weight 650 (today: whatever system
   monospace and Inter resolve to).
6. Numbers and dates are always formatted the English way; lists sort with a natural compare without locale
   tailoring.
7. Removed web-only things: the web-inspector shortcut and its row in the shortcuts list, the "plain browser"
   alert, the browser fallbacks behind `isTauri()`; the dev URL hashes become `--view` and `--demo`.
8. Settings and filters reset to defaults on the first native launch (keys and sign-in survive), unless S1.35 is
   requested.
9. Export is desktop-only in the first native release; on phones the export menu is hidden.
10. One keyboard zoom binding (`+`, `=`, `-` → one level, 250 ms).
11. Ship and train icons are drawn with tiny-skia, not Canvas 2D: same shapes, not bit-identical edges.
12. Tiles and images are cached on disk by the app instead of by the webview's HTTP cache.
13. Label characters Overpass has no glyph for are skipped (the browser fell back to system fonts).
14. Rain and tilt-shift are GPU passes instead of DOM overlays.
15. On Android the soft keyboard works through the app's own text bridge; composition underlines are not shown.
16. Map label glyphs sit where TinySDF 2.3 draws them. Today deck 9.4 assumes tiny-sdf 2.0's pen position, so
    every glyph is drawn up to about 1 px too far left (the reference shows "Södertäl je"); the port does not
    reproduce that (E10).
17. Label collision tests each label's whole claimed box against the boxes kept so far; deck tests only the
    anchor patch and lets hidden labels' boxes still hide others. Labels come out somewhat sparser, aircraft
    labels most (E11; B4 and A7 decide whether to keep it).

Not in this plan; candidates after parity: twist-to-rotate and two-finger tilt on touch; fling inertia and eased
`fly_to`; long-press hover card on touch; far building tiles drawn roof-coloured and hazed (doc 04 §17.1);
animated water on draped terrain; in-app APK install; share-sheet export on phones; an Arch package; code signing
and notarisation; AccessKit accessibility; a Rust texture encoder replacing `compress-textures.mjs`; folding the
backend's module statics into `Backend`; a typed rate-limit error; locale-aware formatting; a fallback font for
non-Latin labels.

---

## 7. Decisions on the mappers' open questions

**Doc 01 (camera, basemap)**

| Question | Decision |
|---|---|
| Colour space | Offscreen non-sRGB `Rgba8Unorm` map target, gamma-space blending (1.4.1) |
| Zoom/latitude coupling | Reproduced exactly, all three places |
| Projection swap at zoom 12 | Hard swap; one tile cache, so nothing reloads |
| Twist and tilt gestures on phones | Not in parity scope (none today); post-parity list |
| Inertia, eased `fly_to` | Neither: no inertia, linear transitions |
| Keyboard zoom bound twice | One binding |
| Label collision and outlined text | CPU greedy declutter; own SDF text pipeline |
| Thin lines | 4× MSAA on the map pass |
| Attribution for Esri and OpenFreeMap | The attribution strip of doc 06 §2.6 carries both; unchanged |
| OpenFreeMap tile URL | Read from the TileJSON at start-up |
| Satellite basemap draws no borders or roads | Kept |

**Doc 02 (radar, ships, trains)**

| Question | Decision |
|---|---|
| `cutPolygonByGrid` | Ported as `scene::geom::grid_cut` (S1.24) |
| Reuse `RS:radar.rs` contouring for RainViewer | No: `IsoRings` is ported so tile edges join exactly; the backend keeps the `contour` crate for measured volumes |
| Radar tile zoom | Verified: `round(zoom + 1)` clamped to 0–7 |
| RainViewer palette dependence | Accepted; a unit test pins the palette inversion so a change shows as a clear failure, and the layer draws nothing on unknown colours |
| Light rig for ship and car materials | The rig of doc 01 §4 in the shared `mesh` pipeline (E7) |
| Train model normals | Normals transformed by the instance scale and renormalised (E7); checked with the demo trains (T3) |
| Icon atlases: tiny-skia or baked PNGs | tiny-skia at run time |
| Ground-plane train icons | Pixel size converted to world size at the icon's depth (E8) |
| Dash phase | Cumulative-distance attribute; small phase differences accepted |
| Rail lines hidden by buildings | Depth test on, depth write off; buildings write depth; no polygon offset anywhere |
| Operator regexes | `regex` with `(?i)` and ASCII word boundaries; tested on real names (S1.14) |
| `hullPolygon`, OpenRailwayMap raster | `hull_polygon` ported only as far as its tests need; the raster layer is not ported |
| Web fallbacks gone: measured radar, speed map, ground rails always on, also on phones | Yes |
| `Train.tail` orientation | Assumed as documented (back from the start of frames); T3's shot is the check |
| Ship selection ring quirk | Kept (reported position, `max(bow, stern)`) |

**Doc 03 (aircraft, interaction)**

| Question | Decision |
|---|---|
| Selected aircraft colour with liveries on | As today: liveries replace the blue tint above zoom 6 |
| Trail and route with terrain on | Kept at altitude (deviation 3) |
| deck's 180° yaw and flipped sun | True ENU frame (deviation 1) |
| Selection ring radius and position | As today (icon-class size, un-raised position) |
| Label declutter | CPU greedy with a 150 ms fade |
| Icon atlases in KTX2 | Transcoded at run time like every other texture (E15) |
| Hover on touch | None, as today; long-press is on the post-parity list |
| `toLocaleString` grouping | Fixed English grouping |
| 1.5 map-scale cap and refresh intervals | Kept |
| Sizing by metres-per-pixel at the view centre | Kept |
| Two clocks | Kept |
| Markers showing through the globe | Kept |

**Doc 04 (realistic, terrain)**

| Question | Decision |
|---|---|
| Far building tiles (z13) | As today: flat, wall colour, alpha `255 − code`, no realistic shading |
| Terrain draping | Cover textures per terrain tile (1.4.12); animated effects stay frozen between re-renders as today |
| Surface format | Non-sRGB target; the shaders keep their own `pow(1/2.2)` |
| KTX2 / Basis crate | `basisu_c_sys 0.9.1`, verified on both encodings; fallback K7 |
| WGSL legality | Every occurrence rewritten under doc 04 §16.2; naga validation in a test |
| Path UVs for sleepers and pier boards | Implemented per doc 04 and checked by eye against the reference pictures in R2 |
| "Absent numeric property reads as 0" | `core::mvt::num_or_zero`, used by the Streets GL rules |
| Third-party endpoints | All behind `backend` with disk caches and limits (K12) |
| Mobile performance of trees and textures | Budgets and knobs of K8 |
| `time` wrapping every 100 000 s; foot shade taking the stencil first | Kept |

**Doc 05 (logic and tests)**

| Question | Decision |
|---|---|
| Crate boundaries | As proposed, plus `rustradar-map` between `scene` and `app`; `RS:motion.rs` becomes `backend::rail_motion` |
| Settings in localStorage | Reset on first native launch; optional bridge S1.35 |
| Zone abbreviations | Generated lookup table checked in (S1.11) |
| Locale | English everywhere |
| `motion.ts` approach | Transliterate, port all tests, golden fixtures before the TypeScript goes (S1.17–S1.20) |
| earcut, `cutPolygonByGrid`, d3-contour | `earcutr`; own `grid_cut`; own `IsoRings` port (also used for land cover) |
| GeoTIFF, PMTiles, KTX2 crates | Own COG range reader in `backend::canopy` (as `canopy.ts` does it by hand); `pmtiles`; `basisu_c_sys` |
| Dead exports | Ported as test helpers with their tests |
| Modules without tests | New tests written with each port (listed per item) |
| Small differences between duplicates | `weather.rs`'s `parse` kept (no weather without cloud fraction); 60 s buffer kept in the backend and 30 s window in the motion model; `isRateLimited` stays a text match for now |
| `trackpad.ts` heuristics | The pure functions are ported with their tests (E2); B8 maps winit's pixel and line deltas onto them |

**Doc 06 (UI)**

| Question | Decision |
|---|---|
| Zone abbreviations; number and date formats | Generated table; English |
| Weights 550 / 650 / 750 | Exact, through the variable `wght` axis (verified); middle dot from Overpass Mono |
| Web-only features | Inspector shortcut and row, plain-browser alert: dropped; URL hashes: CLI flags |
| Desktop minimum window 900 × 600 | Kept, so the phone layout is for phones only |
| Update toast and splash | Kept, driven by the own updater (P2, P3) |
| Mobile text input, safe areas, back gesture | GameActivity and the text bridge; insets fed to egui; back key → close top (K4) |
| File pickers, save and reveal, open URL | `rfd` and reveal on desktop; `webbrowser` everywhere; no file dialog and no export on phones in the first release |
| Remote images | `app::images` with a disk cache; per-host headers recorded in U18 |
| Glyph-level parity | Not a goal; layout, colours and sizes are |
| `oklch` literals | U1 converts each `oklch` literal exactly (the OKLCH → sRGB formula in a unit test) instead of using the approximations in doc 06 §1.1 |

**Doc 07 (backend, packaging)**

| Question | Decision |
|---|---|
| Scripts and "no TypeScript" | Release tooling moves to `xtask` (Rust). The offline asset generators (JavaScript and Python, run by hand, outputs committed) stay as they are; see section 9 |
| Settings migration | Reset; optional bridge |
| Packaging tool | cargo-packager for desktop bundles; own updater client |
| Windows MSI | Kept beside NSIS, so MSI installs can still update |
| Arch package | Not produced (not produced today) |
| Signing and notarisation | Unsigned, as today |
| Minimum OS versions | macOS 11, iOS 15, Android `minSdk 24` |
| Exports on phones | Desktop only in the first native release |
| Assets | All embedded with `include_bytes!`; revisit for Android only if K14 triggers |
| First native release installable by the old updater | P11 rehearsal is a release gate |
| Android `versionCode` | `major·1 000 000 + minor·1000 + patch`, checked against a released APK in P8 |
| `tokio::spawn` off the runtime | `backend::rt()` everywhere (K15) |
| Soft keyboard, insets, surface loss | Prototyped in stage 2 (E19) |
| Basis as a C++ dependency | Accepted, with fallback K7 |
| TestFlight refresh during the transition | The workflow detects a Tauri tree (`src-tauri/tauri.conf.json`) and uses the old steps for it (P10) |
| Unverified tool claims | Each is checked inside the item that depends on it (P5–P9, U20) |

---

## 8. Test and screenshot harness

- Unit and integration tests: `cargo test --workspace`. GPU tests need a Vulkan software rasteriser:
  `VK_ICD_FILENAMES=/opt/pw-browsers/chromium-1243/chrome-linux64/vk_swiftshader_icd.json` in this container;
  in CI, Chrome's SwiftShader ICD or `mesa-vulkan-drivers` (lavapipe). Without an adapter the GPU tests print
  `skipped: …` and pass.
- Shot command:
  `cargo run -p rustradar-app -- --screenshot out.png --size 1100x720 [--scale 2] --view <lat,lon,zoom[,pitch[,bearing]]> --basemap <dark|satellite|realistic> [--now <unix ms>] [--fixture <dir>] [--demo] [--ui <fixture name>]`.
  A shot never touches the network: tiles come from `crates/map/tests/data/tiles`, feeds from the fixture, and a
  missing tile is simply not drawn. The headless path runs frames until no tile job is pending, then two more with
  the egui clock one second apart (areas are invisible on their first frame, spike pitfall 9).
- Named views (`lat, lon, zoom, pitch, bearing`):

  | Name | View | Used for |
  |---|---|---|
  | V1 | 35, 10, 1.9, 0, 0 | the initial view; whole globe |
  | V2 | 52, 10, 4.5, 0, 0 | Europe; labels, radar, aircraft icons |
  | V3 | 59.33, 18.07, 9, 0, 0 | Stockholm region; vector map, trains, rail map |
  | V4 | 59.6519, 17.9186, 13.5, 45, 30 | Arlanda, flat map, pitched; runways, aircraft models |
  | V5 | 59.3293, 18.0686, 15.5, 60, 20 | Stockholm city; buildings, realistic ground, trees |
  | V6 | 57.70, 11.93, 13, 0, 0 | Gothenburg harbour; ships |
  | V7 | 78.2, 15.6, 6, 0, 0 | Svalbard; latitude coupling, far-side filters |
  | V8 | 0, 179.5, 3, 0, 0 | antimeridian wrap |

- Assertions are properties, not golden images: exact or toleranced colours at chosen pixels, coverage of a colour
  in a rectangle, sizes measured in pixels, connected-component counts, "no pixel darker than". They hold across
  rasterisers. Golden PNGs are written to `target/shots/` for review and compared only when `SHOT_GOLDEN=1`.
- Visual review: every drawing item ends by viewing its shots beside the matching file in
  `docs/port/reference/` (S1.33) and noting differences in its report.
- Checks only a person can do (interactive window, phones, other operating systems, installers) are marked
  **person check** in the item and collected in `docs/port/device-notes.md`.

---

## 9. Questions still open for the owner

The plan proceeds on the defaults below; an answer only changes the item named.

1. **Settings on upgrade.** Default: settings and filters reset once (keys and sign-in survive). Alternative:
   item S1.35 plus one more Tauri release before the native one, so settings carry over.
2. **Who runs the person checks?** The plan needs, at a few points, an Android phone, an iPhone, a Windows PC and
   a Mac (E19, B8, U19, P5, P6, P9, P11). Agents can only build for those targets, not run on them.
3. **Signing.** Default: unsigned on macOS and Windows, as today. Notarisation and Windows code signing need new
   certificates and secrets.
4. **Windows MSI.** Default: keep shipping it beside the NSIS installer. Dropping it risks stranding anyone who
   installed from the MSI.
5. **Minimum OS versions.** Default: macOS 11, iOS 15, Android 7.0 (`minSdk 24`).
6. **Export on phones.** Default: hidden on phones in the first native release (a share sheet is new platform
   code). It is not known whether export works on phones today.
7. **Scripts.** Default: "no more TypeScript" covers the app and the release tooling. The offline asset generators
   under `scripts/` (five JavaScript files and seven Python files, run by hand) stay; `compress-textures.mjs`
   stops working once `node_modules` is gone and is replaced only if textures need regenerating.
8. **Touch rotation and tilt on phones.** Default: not added (today phones can only rotate and tilt with the
   on-screen controls).
9. **Formatting.** Default: English number grouping, dates and sorting for everyone.
10. **Arch package.** Default: none (not produced today).

---

## Stage 1 status

Checked on 2026-11-10 at commit `280050e` on `rust-port`.

**Done.** Items S1.1–S1.34 are committed. S1.35 (settings bridge) was not built, because the owner has not
asked for it (section 9, question 1). S1.2 was reported partial: it left `roads.rs` in the shim, and S1.3 then
moved it (as `roads::widths`). The checks of S1.1–S1.3 still hold:
- `cargo tree -p rustradar-core` lists no workspace crate.
- `rustradar-scene` pulls in no tokio, reqwest or wgpu.
- `rustradar-backend` pulls in no tauri.
- `tauri` and `tokio::spawn` appear in `crates/backend/src` only in comments.

Only one parity line of section 4 is ticked: the motion model. Its items are all in stage 1 and their acceptance
has passed (all 46 cases, golden poses). The app starts using it in stage 4. The folders line also needs E17, so
it stays unticked.

**Checks**

| Command | Result |
|---|---|
| `cargo check --workspace --all-targets` | clean |
| `cargo test --workspace` (SwiftShader) | 1131 passed, 0 failed, 22 ignored |
| `cargo clippy --workspace --all-targets` | no warnings |
| `cargo check -p rustradar` (shim) | passes |
| `bun run check` | 0 errors, 0 warnings |
| `bun test src` | 394 pass |

The 1131 passing tests, by crate:

| Crate | Passed | Ignored |
|---|---|---|
| core | 601 unit + 2 in `tests/motion.rs` | 0 |
| scene | 303 | 1 |
| backend | 193 | 21 |
| fr24 | 17 | 0 |
| xtask | 11 | 0 |
| app | 1 (`frame_is_not_uniform`, really rendered on SwiftShader) | 0 |
| shim | 2 | 0 |
| doc-tests | 1 | 0 |

**TypeScript twins.** Every one of the 387 TypeScript tests stage 1 ports has a Rust `#[test]` of the same name,
none of them ignored: 254 in core and 133 in scene. Names are in snake case. Ports dropped `describe` prefixes
and split camelCase (`syncClock` → `sync_clock`); an apostrophe was either dropped or turned into `_`
(`isn't` → `isn_t`). The 7 TypeScript tests with no twin are the ones planned for stage 2:
- `trackpad.test.ts` (3)
- `ktx2-transcode.test.ts` (4)

Core and scene hold 907 tests in all:
- 387 twins;
- 519 new tests;
- 1 stale ignored copy, described below (deleted since by C1).

**Ignored tests (22 at the check; 21 after C1)**
- **scene, 1 (deleted by C1):**
  `landcover::tests::land_cover_traced_into_rounded_outlines_a_wood_and_a_field_towns_and_water_left_out`.
  Its reason read "needs landcover_ground::cover_outlines (S1.29 …)". That port runs in `landcover_ground.rs`
  under the same name, so C1 deleted this copy; scene has no ignored test left.
- **backend, 21:** these were ignored before the port and give no reason string.
  - 10 need the network: `building_lod::real_buildings`, `dwd::live_run`, `geocode::live_photon`,
    `liveries::real_logos`, `mrms::live_volume`, `roads::real_roads`, `runways::downloads_the_list`,
    `ships::{live_ship_details, live_vesselfinder}`, `taxiways::downloads_copenhagen`.
  - 11 need data named by an environment variable (`RAIL_TILE`, `RAIL_TILES`, `SIGNAL_TILES`, `LIVE_DIR`,
    `NUMBERS`): `blocks::signal_headings_on_real_track`; `rail::{a_real_tile, lund_to_malmo,
    through_malmo_city_tunnel, a_train_from_lund_to_malmo_is_drawn_on_the_track,
    hjarup_offers_the_track_running_the_trains_way}`; `trafikverket::{replay_live, print_queries, bench_frames,
    replay_routes, replay_route_timeline}`.

**Still to do.** None of these stops the build or the tests. Item C1 (stage-1 cleanup) resolved 1, 2, 3, 5, 7
and 8; 4, 6, 9 and 10 remain.
1. **Resolved (C1).** ~~Time zones are already wrong.~~ `Cargo.lock` pinned `jiff-tzdb 0.1.8` (tz database
   2026c), which lacks the permanent summer time of Manitoba and Inuvik that began in November 2026, so
   `America/Winnipeg`, `Rainy_River`, `Canada/Central` and `Inuvik` showed the wrong clock. C1 moved the lock to
   `jiff-tzdb 0.1.9` (tz database 2026e; `jiff` itself stays 0.2.37) and re-ran `scripts/port/zone-names.mjs`:
   only the generated table's header changed, as Node's ICU already had 2026e. The test
   `zones_named_by_the_table_beyond_the_ported_cases` in `time.rs` now checks that the four zones read CST/MST
   in January 2026 and CDT/MDT in January 2027. When the tz database changes again: `cargo update -p jiff-tzdb`, re-run the script,
   `cargo test -p rustradar-core time::`.
2. **Resolved (C1).** ~~Two scripts write to a folder that is gone.~~ `scripts/sth.py` and `scripts/flow.py`
   now write to `crates/backend/data/` (run them from the repository root, as before); doc 07's table of
   generators says so too.
3. **Resolved (C1).** ~~Unused shim dependencies.~~ `futures-util` and `reqwest` are gone from
   `src-tauri/Cargo.toml`; `cargo check -p rustradar` and `cargo test -p rustradar` pass.
4. **Duplicated helpers.** Eighteen files keep their own copies of JavaScript's `Math.min`/`Math.max`,
   `Number()`, truthiness, white space or `trim()`. In core: geo, places, weather, trains, labels, routes,
   ship_details, flightpath. In scene: screen, building_shadow, hillshade, geom, landcover, overture, ground,
   bridges, rail, trees. They should become public helpers in `core::num`. (Left for a later item: C1 kept to
   the small, safe changes.)
5. **Resolved (C1).** ~~Duplicated tables.~~
   - `liveries.rs` imports `reference::airline_icao`; its private copy is gone.
   - `scene::ship_models::ship_color` calls `rustradar_core::ships::ship_color` (as `f64`), and the test
     `core::trains::clear_of_the_ship_palette_and_of_the_selection_amber` maps the colour keys through it, as
     the TypeScript did.
6. **One radar palette.** It is defined twice, in `backend::radar` and `scene::radar::mesh`. A test keeps the two
   equal. Moving it into core needs an item that owns `backend::radar` (W4).
7. **Resolved (C1).** ~~Unknown train kinds.~~ `model::TrainKind::Other` carries `#[serde(other)]`: any kind
   this build does not know reads as `Other` (and is sent on as `"other"`), as the TypeScript drew it. Test
   `model::train::tests::an_unknown_train_kind_reads_as_other`.
8. **Resolved (C1).** ~~One rustdoc warning.~~ `scene/src/road_widths.rs` names `INDEX_DEG` in plain code
   text instead of linking it.
9. **Stage 12 paths.** Tests read `static/…` through `CARGO_MANIFEST_DIR`; update them when `static/` moves to
   `assets/`.
10. **Disk space.** The container's disk holds about 39 GB, not the 252 GB `df` reports, and `target/` grows past
    20 GB. A full `cargo test --workspace` needed old test binaries, the cross-target folders and
    `target/debug/incremental` deleted first.

---

## Stage 2 status

Checked on 2026-11-10 at commit `c17c341` on `rust-port` by the stage-2 review (the item that owns no files).

**Done.** E1–E18 are committed and their acceptance holds, with the restated lines and gaps below. E19 is
partial: everything that can be done without a phone is done; the device checklists are pending (person
check). Five lines of section 4 are now ticked: the motion model (stage 1), the globe/flat swap, the
zoom/latitude coupling, the scene refresh pacing, and the desktop folders with `keys.json`.

**Checks**

| Command | Result |
|---|---|
| `cargo check --workspace --all-targets` | clean |
| `cargo test --workspace` (SwiftShader) | 1522 passed, 0 failed, 22 ignored; no GPU test skipped |
| `cargo clippy --workspace --all-targets` | no warnings |
| `cargo clippy -p rustradar-app --all-targets --target {aarch64-linux-android, aarch64-apple-ios}` | no warnings, through stand-in C compilers (E17's recipe); target folders deleted |
| "Phone shells" CI (`.github/workflows/phone-shells.yml`) | runs 42865363833 and 42866917494 passed (E19); run 42870433769 (commit `c17c341`, with the wiring) passed: the debug APK with the real NDK 27 and the simulator app |
| `cargo check -p rustradar` (shim) | passes |
| `bun run check` | 0 errors, 0 warnings |
| `bun test src` | 394 pass |
| app shot tests with no adapter (`VK_ICD_FILENAMES=/nonexistent.json WGPU_BACKEND=vulkan`) | 22 pass, each GPU test prints `skipped: …` |

The 1522 passing tests, by crate:

| Crate | Passed | Ignored |
|---|---|---|
| core | 602 unit + 2 in `tests/motion.rs` | 0 |
| scene | 424 | 0 |
| map | 161 unit + 9 in `tests/fixture_tiles.rs` | 0 |
| backend | 211 | 22 (the 21 of stage 1, and `tiles::live_openfreemap`, which needs the network) |
| app | 45 unit + 3 in `tests/headless.rs` + 19 in `tests/shots.rs` | 0 |
| xtask | 26 | 0 |
| fr24 | 17 + 1 doc-test | 0 |
| shim | 2 | 0 |

**TypeScript twins.** The 7 tests left for stage 2 are ported under their own names and pass:
`trackpad.test.ts` (3, in `scene::controller`) and `ktx2-transcode.test.ts` (4, in `map::textures`). All 394
TypeScript tests now have a Rust twin.

**What was looked at.** The headless binary (`cargo run -p rustradar-app -- --screenshot …`) at V1 dark (1100 × 720
and 640 × 400 × 2), V1 realistic, V2 satellite at 390 × 844, V7 satellite, V8, latitude 0 against 60 at zoom 2,
zoom 11.9 against 12.1 pitched, zoom 12 pitch 60 satellite: a land disc on ink where the globe shows, land or the
satellite earth colour edge to edge on the flat map, no seam at the antimeridian, the disc 1.7× wider at latitude
60. Also the 37 `e18-*` shots of the harness (cube, shadows, lines, dashes, icons, ring, "Stockholm"). For the
first time **the interactive window ran**: under Xvfb (X11, llvmpipe GL) it opened, drew the globe of V1, and a
left drag panned (the view moved north and the disc grew 1.18×, as deck's pan does at zoom 1.9), a right drag
tilted, the wheel zoomed in and out. Not run: Wayland, Windows, macOS, a real GPU, HiDPI, the maximised start.

**Per item**

| Item | Verdict | Evidence, and what differs from the row |
|---|---|---|
| E1 | done | `camera::tests::matches_every_golden_view`: all 24 views, every field; worst 3.3e-10 px, matrices 1.9e-16 relative. Off-limb unproject returns deck's closest approach, not the exact limb point. |
| E2 | done; one line restated | 3 trackpad twins; 23 scenarios run through deck 9.4's own `GlobeController` and the app's `NorthUpGlobeController`, matched to 1e-9. **"A pan keeps the grabbed point under the pointer" is not what today's app does**: deck moves the map 1.117× the pointer's travel and ignores pitch; the test of that name checks deck's rate. Keeping parity is the plan's priority; an exact grab would be a new deviation (owner's call). |
| E3 | done | `matches_every_golden_tile_list`: 144 lists (24 views × 6 configs), 4943 tiles, same tiles in the same order. |
| E4 | done | ink frame exact in 3 configurations; `all_shaders_validate`; `projection_precision` 0.0115 px on the globe, 0.0016 px flat, and the same on the GPU. |
| E5 | done; one line restated; one parity gap | V1's edge within 1.5 px of E1's limb in 72 sectors. **"Twice as wide at latitude 60" cannot hold on screen**: deck's camera stands 1.5 viewport heights away, so the disc grows 1.71×; the globe's radius at the centre's distance doubles (1.999), and that is what the test checks. **Gap**: see 4 below (the earth is unlit). |
| E6 | done | widths within 6 % at zoom 3, 11, 15, pitch 0 and 60, for lines running into the view; a line across the view at pitch 60 lies flat and is half as wide, as in deck; dashes 59, 42, 60, 42. |
| E7 | done | three face brightnesses; depth 0 gives exactly the instance colour; `SHADOW` darkens an overlap once (control with `SURFACE_NO_CULL` twice); roofs of `dark-v4` within 1 level. |
| E8 | done | billboard 48 px (area within 0.4 %) upright at pitch 0 and 60, bearing 30; ground icon foreshortened to 0.50–0.52 of the area. Sizes are deck's (exact at the focal depth), not constant at every depth. |
| E9 | done | ring `[55,142,240,235]` over halo `[55,142,240,56]` within deck's expected range on globe and flat map, × 1 and × 2; ring 2.000–2.001 px. |
| E10 | done; one deviation | ink box within 0.85 px (× 1) and 0.26 px (× 2) of epaint's; SDF square symmetric and 0.75 at the edge. Glyphs placed where TinySDF draws them, not where deck 9.4 does (section 6, item 16). |
| E11 | done | 12 unit tests, including the three of the row. Whole-box test (section 6, item 17). |
| E12 | done; plan 1.4.11 superseded | 17 unit tests for each hit kind, draw order, depth modes and pixel ratios. The nearest pixel wins, then the topmost item at that pixel (deck's `getClosestObject`); 1.4.11's "topmost wins, ties to the nearest" is reversed. A point's radius is its drawn footprint; the 6 px come on top. |
| E13 | done; **not plumbed in** | the 4 acceptance tests with a fake fetcher and 35 more (E13 ran them 25 times without a flake). Nothing outside the crate uses it yet (gap 1). |
| E14 | done | tests against a local tokio listener (second request from disk, stale file on failure); no test touches the network. No ETag revalidation. |
| E15 | done; runtime target always RGBA8 | 4 twins; the icon atlas as RGBA8 896 × 768 with 10 levels, not blank; BC7, ASTC and ETC2 uploads read back on SwiftShader. Gap 3. |
| E16 | done for its rows; too few views for stage 3 | 33.2 MB, 670 tiles, NOTICE; `FixtureTiles` and 9 tests. Gap 5. |
| E17 | done | `--screenshot` draws E5's globe through `map::headless`; the binary writes a shot with every proxy dead (test; E17 also ran it inside `unshare -rn`). The four cross targets were checked only with stand-in C compilers (gap 6). |
| E18 | done | 19 shot tests on SwiftShader; without an adapter all pass with `skipped:`; `SHOT_GOLDEN` records and compares. |
| E19 | partial | CI built the debug APK (real NDK 27) and the simulator app. The app-loop wiring that E19 could not do (its files were E17's) is done by this review (fix 1). Device checklists K3, K4, folders, safe area and back not run. |

**Fixes made by this review**
1. `c17c341`: the phone glue is wired into the app loop, as `docs/port/device-notes.md` §2 asked:
   `platform::android::init` before the folders are read; `platform::dirs()` defined per platform (Android
   from the activity, iOS inside the container); `before_frame`/`after_frame` around the frame, back →
   `Action::CloseTop`, else `leave()`; `platform::ios::init()` in `rustradar_ios_main`. Two broken intra-doc
   links in `platform/android.rs` fixed. device-notes §2 updated.

**Gaps** (each with the item that has it, and who should take it)
1. **Tile plumbing has no owner (E4, E13, E14, E16, E17).** `layers::Init` and `PrepareCx` carry no
   `TileRuntime` or `UploadBudget`; `MapRenderer::new` takes no runtime; the app has no `TileFetcher` on
   `backend::tiles` (spawned on `rt()`, waking the loop); the headless path has no fixture fetcher (`--fixture`
   is the feeds' folder) and does not wait on `TileSet::pending`. B2 owns only `layers/vector_map.rs` and
   `scene_builder/base.rs`, so stage 3 cannot draw a tile until an item owns `crates/map/src/{layers/mod.rs,
   renderer.rs, headless.rs}` and the app's fetcher. Suggest one item before B2 that also takes gaps 2 and 3.
2. **Light rig (E7).** `SceneInput` has no `lights: LightRig`; every lit layer (B7 buildings, A5 aircraft,
   ships, cars) needs the one rig aimed from `buildings && flat` (G:2723). `renderer.rs` is E4's.
3. **Texture compression unused (E15, E4).** `Gpu::open` requests no feature, so `LoadOptions::for_device`
   always picks RGBA8 (about 110 MB for the realistic set). Request
   `textures::compression_features(adapter.features())`: optional, so the baseline holds.
4. **The earth sphere is unlit (E5; found by E7).** deck 9.4 reads `material: false` as luma's default Phong
   material, lit. The dark map's polar caps beyond ±85° read (41,40,39)–(44,43,41) in `dark-v1`, (50,50,50) in
   the port; the satellite and realistic spheres are lit today as well. No later item owns `layers/earth.rs`;
   the owner of gap 1 or B6 should light it with `Material::DECK_DEFAULT` and the rig of gap 2.
5. **Fixtures for stage 3 (E16).** The set covers V3–V6 and OpenFreeMap's world at z0–2. B2 and B4 accept on
   shots at V2 (zoom 4.5, tiles z4–z5), which the set lacks; V7, V8, satellite at V1 and the realistic sources
   are not covered either. Extend `cargo xtask fixtures` (plan of `fixtures.rs`) before B2's shots.
6. **CI and cross targets (E17, P10).** `rust.yml` runs only on `main`, so none of the port has run there. Its
   `native-targets` job (`cargo check -p rustradar-app --target …` on ubuntu) fails for all four targets now
   that aws-lc-sys, zstd-sys and basisu_c_sys need target C toolchains; Windows and macOS were checked only
   with stand-in compilers and nothing was linked. P10 must fix this before the branch merges; it should also
   fold `phone-shells.yml` into `rust.yml`.
7. **Linux runtime library.** winit on X11 loads `libxkbcommon-x11.so.0` at start and panics without it (this
   container lacks it; the Xvfb check used a copy from the Ubuntu package in the scratch folder). The deb, rpm
   and AppImage of P7 must depend on or bundle it.
8. **Spike leftovers (E17).** `crates/app/src/ui.rs` (the placeholder panel, no longer drawn) and `theme.rs`
   are still compiled. U1/U6 replace them with the `ui/` folder; `ui.rs` and `ui/mod.rs` cannot both exist.
9. **Phones (E19).** Device checklists pending (below). Every single-line text field must call
   `platform::android::text::report` (U8, U9, U17). Back is always taken on Android, so Android 14+ shows no
   back-to-home preview until the state can say ahead of time whether anything is open.
10. **Rustdoc.** `cargo doc --workspace` warns in `core/src/ships.rs` (link to private `KINDS`),
    `core/src/trains.rs:436` (unclosed `<number>`) and `backend/src/radar.rs` (`contour` ambiguous); none in
    stage-2 files.

**Plan text superseded by stage 2** (rows left as written; the code follows deck and today's app):
- 1.4.2: the flat map's metres use math.gl's 40.03e6 m circumference at the point's latitude, the globe a
  radius of 6 370 972 m; `core::geo::meters_per_pixel` (40 075 016.686 m) is 0.1 % off both viewports' own
  `meters_per_pixel()`.
- 1.4.11 picking priority: see E12 above.
- 1.5 phones: the Android safe area comes from the window insets (`MainActivity.onApplyWindowInsets`), not
  `content_rect`; back reaches Rust through `nativeBack`, not as a key.
- E2 and E5 acceptance lines: see the table.

**Pending for the owner (person checks)**
- One Android phone and one iPhone: `docs/port/device-notes.md` §4–§6 (K3 surface lifecycle, K4 text input,
  safe area and back, folders). Get the builds from the "Phone shells" CI artifacts.
- On a downloaded APK: `unzip -l` shows only `lib/arm64-v8a/librustradar_app.so`; for Play, 16 KB page
  alignment (device-notes §7).
- The desktop window on Windows, macOS and a Wayland session, on a real GPU: opens maximised and hidden until
  the first frame, pans, tilts and zooms.

**Disk.** `target/` was 18 GB at the start of the check with 8.9 GB free; a full `cargo test --workspace` rebuilt
every workspace test binary. Stale test binaries in `target/debug/deps` were deleted to keep room.
