# Port notes 02: weather radar, ships, trains, rail map (globe layers)

Scope: `src/lib/Globe.svelte` lines 1300-2750 (plus the radar block at 1111-1338, selection rings at 3291-3354, picking/tooltips at 3356-3422 and 3713-3736, which those lines depend on) and
`radar.ts`, `radar-mesh.ts`, `radar-arena.ts`, `radar-pool.ts`, `radar-worker.ts`, `ships.ts`, `ship-models.ts`, `ship-list.ts`, `ship-details.ts`, `trains.ts`, `train-cars.ts`, `train-models.ts`, `train-route.ts`, `train-list.ts`, `demo-trains.ts`, `rail-layer.ts`, `rail-overview.ts`, `bridges.ts`.

All paths below are relative to `src/lib/` unless they say otherwise. `G:` means `Globe.svelte`. Colours are RGBA 0-255 unless stated. "Flat" means zoom > 12 (Web Mercator); at or below 12 the view is the globe.

Contents

1. Shared conventions and constants
2. Draw order (the part of `buildLayers` these layers live in)
3. Weather radar
4. Ships
5. Trains
6. Signal blocks and signals overlay
7. Selected train's route
8. Rail map (network, speed map, overview, stations, OpenRailwayMap raster, platforms)
9. Bridges and tunnels (`bridges.ts`)
10. Aircraft route / airport / place markers that sit in this line range
11. Picking, hover, tooltips, selection
12. Sidebar list logic (ship-list, train-list, ship-details)
13. Dev-only layers (sightings, demo trains)
14. Porting notes, open questions, risks

---

## 1. Shared conventions and constants

### 1.1 Geometry helpers (geo.ts, screen.ts)

| Name | Definition | Source |
|---|---|---|
| `FLAT_ABOVE_ZOOM` | `12`. `flat = zoom > 12` | G:266 |
| `EARTH_CIRCUMFERENCE_M` | `40_075_016.686` | geo.ts:10 |
| `FEET_TO_M` | `0.3048` | geo.ts:12 |
| `FEET_PER_KM` | `3280.84` | G:1155 |
| `DEGREES` | `PI / 180` | G:3069 |
| `metersPerPixel(zoom, lat)` | `EARTH_CIRCUMFERENCE_M * cos(lat) / (512 * 2^zoom)` (zoom is deck.gl zoom: 512 px world at zoom 0) | geo.ts:60 |
| `elevation(altFt, ex)` | `max(0, altFt) * 0.3048 * ex` metres | geo.ts:47 |
| `effectiveExaggeration(ex, zoom)` | `1 + (ex - 1) * clamp01((11 - zoom) / 5)` (full exaggeration at zoom <= 6, true scale at zoom >= 11) | geo.ts:54 |
| `cosAngle(lon1, lat1, lon2, lat2)` | `sin a sin b + cos a cos b cos(dLon)` | geo.ts:81 |
| `visibleRadius(zoom, pitch, w, h)` | `min(100, 1.3 * hypot(w,h)/2 * 360/(512*2^zoom) * (1 + 4 sin(pitch)))` degrees | geo.ts:92 |
| `viewArea(view, w, h)` | `null` when `visibleRadius >= 50`; else lat +/- r, lon +/- `r / max(0.05, cos(widestLat))`, wrapped to [-180,180); whole world in lon if `dLon >= 180` | geo.ts:104 |
| `ICON_HEADING_STEP_PX` | `20` | G:394 |
| `screenAngle(viewport, [lon,lat,z], heading, stepM)` | project the point and a point `stepM` metres ahead along `heading` (using 111 320 m/deg, lon divided by `max(0.01, cos lat)`); return `-atan2(dx, -dy)` in degrees (anticlockwise, IconLayer convention); `null` when non-finite or screen distance < 1e-3 px | screen.ts:21 |

Flat-earth metres used everywhere in this area: `111_320` m per degree of latitude; longitude scaled by `cos(lat)`.

### 1.2 Render-state presets (deck.gl `parameters`)

| Name | Meaning in wgpu terms | Source |
|---|---|---|
| `NO_CULL` | cull mode none | G:327 |
| `SURFACE` | depth write off (depth test still on, default less-equal): ground layers paint in layer order | G:332 |
| `SURFACE_NO_CULL` | cull none + depth write off | G:333 |
| `RADAR_LIFTED` | cull none + depth write off | G:1154 |
| `SHIP_PARAMETERS`, `TRAIN_PARAMETERS` | cull none + depth write off | G:1604, G:1882 |
| `THROUGH_GROUND` | cull none + depth write off + depth compare always | G:1885 |
| `LABEL_PARAMETERS` | cull none + depth compare always + depth write off | G:724 |
| `RAISED_NO_CULL` (rail-layer) | cull none + depth write off + depth compare always | rail-layer.ts:113 |
| `TRUE_DEPTH` (rail-layer) | `getPolygonOffset: null`: deck.gl normally nudges each successive layer slightly toward the camera with polygon offset; this disables it so the layer is at true depth and can be hidden by things that write depth (building walls, road-bridge decks) | rail-layer.ts:117 |

deck.gl's default blending applies to every layer here: colour `src-alpha, one-minus-src-alpha`, alpha `one, one-minus-src-alpha`. A layer's `opacity` multiplies fragment alpha.

### 1.3 Text style shared with other documents

`LABEL_FONT = "Overpass Variable"` (G:310), `LABEL_OUTLINE = [6, 12, 19, 220]` (G:725), SDF font settings `{ sdf: true, fontSize: 64, buffer: 6 }` (G:726). Signal names, block labels, dev labels use `"ui-monospace, monospace"`; speed labels use `"Inter, system-ui, sans-serif"` weight 650.

### 1.4 Highlight / selection blue

The accent used for hover and selection in this area is `[55, 142, 240]`:
- mesh hover highlight `[55, 142, 240, 110]` (ship meshes G:1803, train cars G:2023)
- icon hover highlight `[55, 142, 240, 200]` (ship icons G:1860, train icons G:2049)
- selection halo fill `[55, 142, 240, 56]`, selection ring stroke `[55, 142, 240, 235]` 2 px (G:3310-3318, G:3343-3351)
- route ahead `[55, 142, 240, 210]`, airport marker fill `[55, 142, 240, 255]`

deck.gl's highlight mixes the highlight colour over the fragment by the highlight alpha: `mix(color.rgb, highlight.rgb, highlight.a)` with alpha kept; replicate in the icon and mesh shaders.

---

## 2. Draw order

From `buildLayers()` (G:2520-2750). Only the part relevant here; "..." are layers owned by other documents. Later = drawn on top (subject to depth where depth testing is on).

1. Earth sphere, terrain surface, basemap ground (satellite imagery / realistic ground / dark vector map) ... (G:2638-2677)
   - In the realistic basemap, `railGroundLayers(...)` (ground rails + platforms, section 8.6) are pushed right after the ground tiles and before the relief shading (G:2663).
2. Rail map, if `railMap` is on: `railLayers({tileUrl, flat, zoom, workerUrl, speeds: railSpeeds})` (G:2679). Comment: "over the map, under the weather, ships, trains and aircraft".
3. Weather, if `weather` is on (G:2680-2690):
   1. RainViewer ground tile layer (`radar-<url>-<coverageKey>`), then the lifted RainViewer layers per level, lowest first, each `radar-lift-<km>` followed by its `radar-wall-<km>`.
   2. For each measured volume (in the order the backend returned the frames): `<source>-ground-<time>`, then per level above 1 km ascending: `<source>-<km>-<time>` then `<source>-wall-<km>-<time>`.
4. Night shade (`daylight && !realistic`) (G:2692).
5. City buildings (flat only), trees (realistic, flat, zoom >= TREES_ZOOM) (G:2694-2720).
6. Ship selection halo (if a ship is selected) (G:2727).
7. Ship layers: `ship-mesh-<class>` layers first (only at zoom >= 12.5), then `ship-icons` (G:2728).
8. Ship selection ring (G:2729).
9. Signal block layers (`blockLayers()`), in order: `train-blocks-casing`, `train-blocks`, `train-blocks-tunnels`, `train-signals`, `train-signal-names`, `train-block-labels` (G:2735).
10. Selected train's route (`trainRouteLayers`): `train-route-dashes`, `train-route-head-dashes`, `train-route-to-stop`, `train-route-stop-spot`, `train-route-stops` (G:2737).
11. Train selection halo (G:2738).
12. Trains: either `train-icons`, or the car mesh layers `train-car-meshes-<key>` and `train-car-meshes-<key>-tunnel` (G:2739). Note `trainLayers()` is *computed* before `blockLayers()` (G:2733) so the block layers see this frame's train positions, but it is *pushed* after.
13. Dev builds only: sighting layers (G:2741).
14. Train selection ring (G:2742).
15. Map labels, aircraft, routes etc. ... (G:2743 on).

Clock sources: ships and trains use the machine clock `Date.now()` (their `frames` are timed in Unix ms on this machine's clock, G:2728-2733); aircraft use `motion.now()` (the feed's clock).

---

## 3. Weather radar

Three sources end up as the same thing: filled polygons per 5 dBZ band in the NWS palette, drawn flat on the ground and, when the camera is tilted, as a stack of lifted translucent layers joined by vertical walls.

- **RainViewer** (global, raster tiles; reflectivity recovered from colour, heights *estimated* from reflectivity). Contoured on the client (workers).
- **Measured volumes** (NOAA MRMS over North America, DWD over central Europe), loaded and contoured by the Rust backend (`src-tauri/src/radar.rs`), handed to the UI as binary polygon blobs per height plus a coverage mask. RainViewer is blanked where a measured volume has coverage.

### 3.1 Palette and bands (radar-mesh.ts:9-33)

14 bands; band `i` covers `[5 + 5i, 10 + 5i)` dBZ; the last covers everything above. `bandDbz(band) = 5 + 5*band`.

| Band | dBZ from | RGB (hex) | RGB (dec) | Alpha |
|---|---|---|---|---|
| 0 | 5 | 04 e9 e7 | 4, 233, 231 | 150 |
| 1 | 10 | 01 9f f4 | 1, 159, 244 | 170 |
| 2 | 15 | 03 00 f4 | 3, 0, 244 | 190 |
| 3 | 20 | 02 fd 02 | 2, 253, 2 | 210 |
| 4 | 25 | 01 c5 01 | 1, 197, 1 | 230 |
| 5 | 30 | 00 8e 00 | 0, 142, 0 | 235 |
| 6 | 35 | fd f8 02 | 253, 248, 2 | 235 |
| 7 | 40 | e5 bc 00 | 229, 188, 0 | 235 |
| 8 | 45 | fd 95 00 | 253, 149, 0 | 235 |
| 9 | 50 | fd 00 00 | 253, 0, 0 | 235 |
| 10 | 55 | d4 00 00 | 212, 0, 0 | 235 |
| 11 | 60 | bc 00 00 | 188, 0, 0 | 235 |
| 12 | 65 | f8 00 fd | 248, 0, 253 | 235 |
| 13 | 70 | 98 54 c6 | 152, 84, 198 | 235 |

Alpha: `clamp(150 + (bandDbz - 5) * 4, 150, 235)` (radar-mesh.ts:30-33; same as `color()` in radar.rs). Wall vertices use `round(alpha * 0.75)` (`WALL_ALPHA = 0.75`, radar-mesh.ts:290, 317).

Colour is per vertex, flat, **unlit** (`material: false`, G:1194).

### 3.2 RainViewer source (G:1111-1124)

- Index: `GET https://api.rainviewer.com/public/weather-maps.json`. Take `maps.radar.past` last entry (`latest`). URL template: `${maps.host}${latest.path}/256/{z}/{x}/{y}/2/1_1.png` (256 px tiles, colour scheme 2 = "Universal Blue", options `1_1` = smoothed, snow shown). `null` if no frame or on any error.
- Refreshed when `Date.now() - radarCheckedAt > 600_000` ms (10 min), checked inside `buildLayers` while `weather` is on (G:2681).
- Tile layer (G:1340-1354): `minZoom 0`, `maxZoom 7`, `tileSize 256`, standard XYZ Web Mercator tiles, parameters `SURFACE` (depth write off). Layer id is `radar-${url}-${radarCoverageKey()}`: a change of frame URL or coverage key makes a new layer, i.e. **all tiles reload** (those not under a coverage come straight from the pool cache).
  - deck.gl picks tile zoom as roughly `round(viewZoom) + log2(512 / tileSize)` = `round(viewZoom) + 1`, clamped to `[0, 7]`; above 7 the z7 tiles are over-zoomed. (Verify against the tile-selection code written for the other tile layers; the radar must use the same selection with these min/max values.)
- Each tile request goes through `loadRadarTile(url, index, signal)` (radar-pool.ts:120). It is cancellable (the view moving on aborts it).

#### Colour to reflectivity (radar.ts:36-42)

`reflectivity(r, g, b, a)`:
```
a == 0                  -> -inf (no echo)
a < 250                 -> (a / 255) * 10            // drizzle: translucent beige, 0..10 dBZ
b > g && r < 160        -> 10 + 25 * (595 - (r+g+b)) / 420   // blues: light 10 -> dark 35
r > 240 && b < 100      -> 40 + 15 * (224 - g) / 224        // yellow 40 -> red 55
otherwise               -> 60                               // pinks and white (hail)
```
Used value is `max(0, reflectivity)`; pixels with alpha 0 are left at 0 dBZ.

#### Estimated echo top (radar.ts:48-57)

`echoTopKm(dbz) = dbz == -inf ? 0 : clamp(2 + (dbz - 10) * 0.22, 1, 12)`.

`RADAR_LEVELS_KM = [1.5, 3, 4.5, 6, 8, 10, 12]` (radar.ts:33): heights of the lifted layers above the ground layer.

`lowestBandAt(km)` = first band whose `echoTopKm(bandDbz(band)) >= km` (up to `BAND_COUNT`). Evaluated:

| Level km | fromBand | Bands kept (dBZ >=) |
|---|---|---|
| 1.5 | 1 | 10 (2.0 km; band 0 = 5 dBZ only reaches the 1 km floor, so drizzle stays on the ground layer) |
| 3 | 2 | 15 (2 + 5*0.22 = 3.1) |
| 4.5 | 4 | 25 (5.3) |
| 6 | 5 | 30 (6.4) |
| 8 | 7 | 40 (8.6) |
| 10 | 9 | 50 (10.8) |
| 12 | 11 | 60 (13 -> clamped 12) |

(Derived by evaluating the formula; band 0 = 5 dBZ gives 0.9 -> clamped 1.0 km, band 1 = 10 dBZ gives 2.0 km, band 3 = 20 dBZ gives 4.2 km, band 6 = 35 dBZ gives 7.5 km, band 8 = 45 dBZ gives 9.7 km, band 10 = 55 dBZ gives 11.9 km.)

#### Tile processing: `processTile(pixels, width, height, {x,y,z}, covered?)` (radar.ts:159-209)

1. `bounds = tileBounds(index)`: west `x/2^z*360-180`, east `(x+1)/2^z*360-180`, north `tileLatitude(y, z)`, south `tileLatitude(y+1, z)` with `tileLatitude(y, z) = atan(sinh(PI * (1 - 2y/2^z))) * 180/PI` (radar.ts:131-136).
2. Build `dbz: f32[width*height]` (row 0 at top). For each pixel with alpha != 0: if `covered(lon, lat)` at the pixel centre (lon linear across the tile at `(c+0.5)/width`; lat = `tileLatitude(index.y + (r+0.5)/height, z)`, i.e. Mercator rows) then skip (leave 0); else `dbz = max(0, reflectivity(...))`. Track whether any value > 0; if none, return `null` (most tiles).
3. `blur(dbz)`: separable 3x3 binomial `[1,2,1]/4` horizontally then vertically, edges clamped (radar-mesh.ts:884-901).
4. `contourBands(blurred, width, height, lines)` (section 3.4) gives band polygons and wall lines in unit tile coordinates (x 0..1 left to right, y 0..1 top to bottom).
5. Map every polygon and wall vertex to lon/lat: `lon = west + x*(east - west)`, `lat = tileLatitude(index.y + y, index.z)`.
6. `slices = buildMesh(polygons)` (2D triangles); return `null` if no indices. `walls = buildWalls(lines)` or `null` when no lines / no indices.
7. `levels`: for each `km` in `RADAR_LEVELS_KM` in order, `fromBand = lowestBandAt(km)`; stop at the first level where `slices.bandStarts[fromBand] >= slices.indices.length` (no triangles at or above that band; higher levels are emptier still). So a tile's `levels` are contiguous from the lowest level.
8. Result `TileMesh { id: "z/x/y", bounds, slices, walls, levels: [{km, fromBand}] }`. On the main thread it becomes `RadarTile` with `ground = polygonData(slices)` (the whole mesh, all bands).

#### Workers / pool (radar-pool.ts, radar-worker.ts)

Port as a small thread pool (or rayon/tokio blocking tasks); the protocol itself is incidental. Behaviour to keep:

- Pool size `clamp(hardwareConcurrency - 1, 2, 4)` workers (radar-pool.ts:36). Jobs go to the least busy worker.
- Job kinds: `tile` (fetch PNG, decode RGBA, `processTile`), `volume` (`processVolume`), `coverage` (set the coverage masks used by later tile jobs), `cancel` (abort a running tile fetch; a cancelled job resolves to null immediately).
- A tile job captures the coverage as it was when the job started (radar-worker.ts:32-33).
- Tile fetch failure / non-200 / abort -> `null` (tile stays empty) (radar.ts:139-153).
- Result cache (radar-pool.ts:111-151): LRU of at most `CACHE_SIZE = 96` finished tiles (including `null` results), keyed `"z/x/y|<keys of the coverages intersecting this tile's bounds, joined by |>"`. The whole cache is cleared when the frame URL changes. A hit moves the entry to most-recent. An aborted load is not cached.
- `setRadarCoverage(list)` (radar-pool.ts:97): no-op if the `|`-joined keys are unchanged; otherwise stores and broadcasts to the workers. `radarCoverageKey()` = sorted keys joined by `-`, or `"full"` when there are none (radar-pool.ts:109).
- `coveragesOver(bounds, coverages)` = coverages whose bounds intersect the tile bounds (inclusive compare) (radar.ts:124-129).

#### Coverage test (radar.ts:97-122)

`Coverage { key, bounds: [west, south, east, north], width, height, mask: u8[width*height] }`, one byte per cell, north row first, `1` = covered. `covered(lon, lat)`: false outside bounds; `x = min(w-1, floor((lon - west)/(east - west) * w))`, `y = min(h-1, floor((north - lat)/(north - south) * h))`; `mask[y*w + x] == 1`. With several coverages: any.

Coverage key (radar.ts:247): `"{source}:{west},{south},{east},{north}:{coverageWidth}x{coverageHeight}:{fnv1a(mask) in base 36}"`; FNV-1a 32-bit: `h = 0x811c9dc5; for each byte: h = (h ^ byte) * 0x01000193 (mod 2^32)`. Only the equality of keys matters; any stable hash will do in the port.

### 3.3 Measured volumes (G:1125-1151, radar-pool.ts:153-197, radar.ts:211-254)

Backend API (see `src/lib/api.ts:373-394`, implemented in `src-tauri/src/radar.rs`):
- `radar_frame(area: BoundingBox | null) -> RadarVolume[]` where `RadarVolume { source: string, time: unix seconds, bounds: {west, south, east, north}, levels: number[] (km), coverageWidth, coverageHeight }`.
- `radar_image(source, name) -> bytes` with `name` = `"ground"`, `"coverage"` or a level's km as a string (`String(km)`, e.g. `"3"`, `"4.5"`).

Polling: only in the app (`isTauri()`), when `Date.now() - measuredCheckedAt > 120_000` ms (2 min), with `viewArea(view, width, height)` as the area (so only sources on screen are fetched; "each is tens of megabytes") (G:2686-2688). `refreshMeasured` (G:1130):
- Re-entrancy guard `measuredLoading`; `measuredCheckedAt` is set at the start and again at the end.
- A volume already held with the same `source` and `time` is reused. If the set is unchanged (same count and all held) nothing happens.
- New volumes are loaded with `loadVolume(frame, radarImage)`: fetch `ground`, `coverage`, and each level blob in parallel, then `processVolume` (in a worker).
- After loading: `setRadarCoverage(measured.map(v => v.coverage))` and re-render. Errors are logged, the old set is kept.

Level blob format, little-endian (`parseVolumePolygons`, radar-mesh.ts:46-94):
```
u32 polygon_count
per polygon:
  u8  band
  u8 + u16 reserved        (3 bytes; ring_count is at offset +4)
  u32 ring_count
  per ring:
    u32 point_count
    point_count x (f32 lon, f32 lat)
```
Ring 0 is the outer ring, the rest holes; rings are not closed. An empty blob = no echo. Dropped: polygons with `band >= 14`, no rings, or an outer ring of fewer than 3 points; holes with fewer than 3 points. Truncation is an error.

`processVolume(frame, ground, coverage, levels[])` (radar.ts:232-254):
- `ground mesh = buildMesh(parseVolumePolygons(ground))`, null if empty.
- For each `frame.levels[i]` (km): `polygons = parse(levels[i])`, `slices = buildMesh(polygons)`; skipped if empty; `walls = buildWalls(polygonWalls(polygons))` or null if empty.
- `coverage` as in 3.2.

`MeasuredVolume { source, time, bounds, ground, levels: [{km, data, walls}], coverage }`.

In the native port the backend can hand these structures over directly (no byte blobs needed); the mesh building (`buildMesh`, `polygonWalls`, `buildWalls`) should still run off the UI thread.

### 3.4 Contouring and meshing (radar-mesh.ts)

#### `contourBands(values, width, height, walls?)` (511-589)

1. If `max(values) < 5` return nothing.
2. Pad the grid by `PAD = 2` cells on each side: `w = width + 4`, `h = height + 4`; the ring of cells at index `1..w-2` / `1..h-2` repeats the clamped edge values; the outermost ring (index 0 and `w-1`/`h-1`) is 0. (Contours then close outside the tile rather than along its edge.)
3. Thresholds: `bandDbz(b)` for every band with `bandDbz(b) <= max`.
4. For each threshold trace iso-rings of `values >= t` (marching squares, below). Convert ring points to unit coordinates: `x = (px - PAD) / width`, `y = (py - PAD) / height`. Drop rings with `|shoelace area| * width * height < 0.3` (specks under 0.3 cell).
5. Walls (if requested): for each band's rings, `clipRingToUnit(ring, band, walls)`, then simplify each new line with Douglas-Peucker at tolerance `WALL_TOLERANCE / max(width, height)` = `0.1 / max(width, height)`.
6. For each band: polygons = `evenOdd(rings[band] ++ rings[band + 1])` (the band is what is above its threshold but not above the next). Each polygon is cut on a grid of resolution `1/8` in unit coordinates (`cutPolygonByGrid` from `@math.gl/polygon`, `gridResolution: 1/8`), and only pieces whose outer-ring bounding-box centre lies strictly inside `(0,1) x (0,1)` are kept (this clips the padding away). Output `BandPolygon { band, positions: flat [x0,y0,...], holeIndices (vertex indices, earcut convention) }`.

#### Marching squares, `IsoRings` (591-749)

A port of d3-contour's `isorings` + `smoothLinear`, producing identical rings (no hole/polygon matching). Details needed to reproduce exactly:
- Grid `dx x dy`. Cell `(x, y)` for `x in -1..dx-1`, `y in -1..dy-1` has corner values at `(x,y)`, `(x+1,y)`, `(x+1,y+1)`, `(x,y+1)`; outside the grid counts as below. Case index `c = at(x, y+1) | at(x+1, y+1) << 1 | at(x+1, y) << 2 | at(x, y) << 3` where `at = value >= threshold ? 1 : 0`.
- Segment table `CASES[c]` (pairs of points `[x0, y0, x1, y1]` within the cell, in half-cell units), radar-mesh.ts:711-728:
```
0:  []
1:  [1.0,1.5, 0.5,1.0]
2:  [1.5,1.0, 1.0,1.5]
3:  [1.5,1.0, 0.5,1.0]
4:  [1.0,0.5, 1.5,1.0]
5:  [1.0,1.5, 0.5,1.0,  1.0,0.5, 1.5,1.0]
6:  [1.0,0.5, 1.0,1.5]
7:  [1.0,0.5, 0.5,1.0]
8:  [0.5,1.0, 1.0,0.5]
9:  [1.0,1.5, 1.0,0.5]
10: [0.5,1.0, 1.0,0.5,  1.5,1.0, 1.0,1.5]
11: [1.5,1.0, 1.0,0.5]
12: [0.5,1.0, 1.5,1.0]
13: [1.0,1.5, 1.5,1.0]
14: [0.5,1.0, 1.0,1.5]
15: []
```
  Each segment's ends are offset by the cell's `(x, y)`.
- Stitching: fragments are joined by start/end point (keyed `x*2 + 1 + (y*2 + 1) * (2*dx + 3)`); when a segment closes a fragment onto itself the ring is emitted with its first point repeated at the end.
- Optimisation: per row, only the span of columns that has values >= threshold (and the row below's span) is visited; this must not change the result.
- `smoothLinear` (731-749): for each ring point, where the point sits on an integer x (`0 < x < dx`), `x = smooth1(x, v(x-1, yt), v(x, yt), value)`; likewise for y. `smooth1(x, v0, v1, value)`: `a = value - v0; b = v1 - v0; d = a / b` (with the infinities handled as `sign(a)/sign(b)`), result `x + d - 0.5`, or `x` if `d` is NaN.

Any correct marching-squares + linear-interpolation implementation that yields closed, non-crossing rings is acceptable; exact d3 parity is only needed for pixel-identical output.

#### `clipRingToUnit(ring, band, out)` (441-461)

For each ring segment: Liang-Barsky clip to the unit square (unclipped ends keep their exact values so lines still join); drop the segment if it lies along an edge of the square (both ends within `1e-9` of x = 0, x = 1, y = 0 or y = 1); otherwise append to the current open polyline if it continues from the previous kept segment's end, else start a new `WallLine { band, positions }`.

#### `evenOdd(rings)` (823-881)

Sort rings by absolute area, largest first. For each ring in that order, its parent is the smallest earlier ring whose bbox contains its bbox and which contains the ring's first point (point-in-ring, even-odd crossing test). `depth = parent.depth + 1`; odd depth rings are holes of their parent; even depth rings are outer rings. A 32 x 32 spatial grid over the rings' bboxes is used only to speed up the parent search. Output: one polygon per even-depth ring: positions = outer ring then its holes; `holeIndices` in vertices.

#### `buildMesh(polygons, gridDeg = 4)` (122-183)

- Sort polygons by band ascending (stable).
- If a polygon's bbox spans more than one cell of a `GRID_DEG = 4` degree grid (`floor(min/4) != floor(max/4)` in x or y), cut it with `cutPolygonByGrid(..., gridResolution: 4)` first (so long edges don't cut through the globe as chords, and earcut stays fast); then triangulate each piece with earcut (2D, holes).
- Output `RadarMesh { size: 2, positions: f32 [lon, lat]*, colors: u8 RGBA per vertex (bandColor), indices: u32, bandStarts: u32[15], bandVertexStarts: u32[15] }`. Vertices and indices are in band order, so "band b and everything above" is the index range `bandStarts[b]..end` and the vertex range `bandVertexStarts[b]..end`. `bandStarts[14]`/`bandVertexStarts[14]` equal the totals.

#### `buildWalls(lines)` (299-359)

- Sort lines by band. For each polyline point (skipping a point identical to the previous one) emit two vertices `[x, y, 0]` and `[x, y, 1]`, colour = band colour with alpha `round(a * 0.75)`. For each consecutive pair emit two triangles: `(prev, prev+1, cur)`, `(prev+1, cur+1, cur)` where `prev`/`cur` are the foot vertex indices.
- Output `RadarMesh { size: 3, ... }` with the same band-start tables. z is 0 at the foot and 1 at the top; the layer's model matrix stretches it between two heights.

#### `polygonWalls(polygons)` for measured levels (370-402)

Wall lines from the rings as they arrive (before grid cutting). Every outer ring is a wall (closed: first point repeated). Rings are identified by `ringKey` = `"{n}:{minX},{minY}:{round(meanX)},{round(meanY)}"` on coordinates quantised with `round(v * 1e5)` (lowest point = westmost then southmost). If two polygons share the same outer ring key, one wall with the stronger band. A hole whose key matches an outer ring (or an earlier hole) is not a separate wall; it raises that wall's band to the max of the two. Only holes with nothing filling them become walls of their own.

#### `mergeMeshes` / `polygonData(mesh, fromBand)` (219-277)

Extracts "bands from `fromBand` up" as one drawable: only the vertices from `bandVertexStarts[fromBand]` on, indices rebased. In wgpu this is just a draw of an index sub-range with a base vertex; no copy is needed.

#### `simplifyLine` (759-787)

Iterative Douglas-Peucker; ends always kept; for a closed ring (chord of zero length) the distance is measured from the start point.

### 3.5 Drawing the radar

#### Lift factor (G:1160)

`liftFor(pitch) = round(clamp01((pitch - 10) / 30) * 10) / 10`: 0 at pitch <= 10 deg (really below 11.5, by rounding), 1 at pitch >= 40 deg, in steps of 0.1. Looking straight down there is no 3D stack at all.

Height scale: `ex = effectiveExaggeration(exaggeration, zoom)`; the radar uses `scale = round(ex * 10) / 10`. Layer height `z = elevation(km * 3280.84, scale)` = `km * 3280.84 * 0.3048 * scale` metres (= `km * 1000 * scale` m).

#### Common polygon layer (`radarPolygons`, G:1172-1203)

Indexed triangles, per-vertex RGBA, unlit, both faces, with a layer `opacity` multiplying alpha. Positions are lon/lat (z = 0) for slices; lon/lat/z for walls with z in [0, 1].

Model matrix (applied to lon, lat, metres before projection, so it is just an affine map of the height):
- slice at height `z`: `height = z` (translate z).
- wall between `bottom` and `z`: `height = bottom + zUnit * (z - bottom)` (column-major matrix `[1,0,0,0, 0,1,0,0, 0,0,z-bottom,0, 0,0,bottom,1]`).

In the port: pass `z_base` and `z_scale` uniforms and compute `height_m = z_base + pos.z * z_scale` in the vertex shader, then project (globe or Mercator).

#### RainViewer ground layer (G:1210-1215)

Per loaded tile with echo: draw `tile.ground` (all bands, flat on the ground), `opacity 0.55`, `SURFACE_NO_CULL` (no depth write, no culling). No lighting.

The set of tiles "in view" (`onViewportLoad`, G:1218-1225) is recorded as `radarTiles`; `radarVersion` is bumped only when that set changes (compared by tile identity).

#### RainViewer lifted layers (G:1278-1339)

Only when `lift > 0`. For each level index `i` (0..6) with any data:
- `km = RADAR_LEVELS_KM[i]`
- `opacity = max(0.16, 0.34 - i * 0.03) * lift` -> per level at lift 1: `0.34, 0.31, 0.28, 0.25, 0.22, 0.19, 0.16`
- slice layer `radar-lift-<km>` at height `z = elevation(km * FEET_PER_KM, scale)`, `RADAR_LIFTED` (no cull, no depth write).
- wall layer `radar-wall-<km>` with the same opacity, from `bottom` to `z`, where `bottom = i > 0 ? elevation(RADAR_LEVELS_KM[i-1] * FEET_PER_KM, scale) : 0`. So the lowest level's walls run down to the ground and the stack has no gaps.

Contents of level `i`: for every tile in view that has `tile.levels[i]`, the tile's slice mesh from `fromBand` up, and its wall mesh from `fromBand` up.

One layer per height **across all tiles** (not per tile), so the stack blends in height order across tile borders; one layer per tile would show a seam where two tiles' stacks meet (radar-arena.ts:3-5). Draw order: level 0 slices, level 0 walls, level 1 slices, level 1 walls, ...

Rebuilt only when `"{scale}/{lift}/{radarVersion}"` changes (G:1316).

#### GPU arenas (radar-arena.ts)

A performance device: per level, one growable GPU buffer set ("arena") for slices (2 floats per vertex) and one for walls (3 floats per vertex), edited in place as tiles come and go, to avoid re-uploading ~40 MB while panning.

- `MIN_VERTICES = 65_536`; minimum index capacity `3 * 65_536`; `GROWTH = 2`: `capacityFor(v, i) = (max(65536, ceil(2v)), max(196608, ceil(2i)))`.
- `update(pieces: Map<tile, {mesh, fromBand}>)`:
  - tiles no longer present: overwrite their index range with zeros (degenerate triangles on vertex 0), forget the slot, bump version.
  - compute live totals and what appending the new tiles would need.
  - repack (`pack`) when the buffers would overflow, or when `capacity > 2 * capacityFor(live)` (shrink), or when nothing is left (free the buffers); otherwise append the new tiles at the end.
- `append`: write the piece's vertex positions and colours at `vertexEnd`, its indices shifted by `(slot.vertex - piece.firstVertex)` at `indexCount`; advance; bump version.
- `pack`: recompute capacities; keep the existing buffers if `cap >= needed && cap <= 2 * needed` for both, else recreate (new buffers are zero-filled); re-append everything; if buffers were kept, zero the stale index tail.
- The layer draws `indexCount` indices (u32) from the arena; `version` tells the renderer something changed.
- Arenas are destroyed (`freeArenas`, G:1299) when weather is off, when there is no RainViewer URL, or when `lift == 0`.

In wgpu: same idea with `queue.write_buffer` on sub-ranges; or simpler, one vertex/index buffer pair per tile and a draw per (level, tile) ordered by level then tile. The latter keeps the required level-major draw order and is likely fast enough; the arena is an optimisation, not a behaviour.

#### Measured volume layers (G:1356-1384)

Per volume:
- Ground: `volume.ground`, `opacity 0.7`, `SURFACE_NO_CULL`, id `<source>-ground-<time>`.
- If `lift > 0`: for each level with `km > 1` (the lowest height is already in the ground composite), ascending: slices at `z = elevation(km * FEET_PER_KM, scale)` with `opacity = 0.18 * lift`, `RADAR_LIFTED`; walls (if any) with the same opacity from `bottom` to `z`, where `bottom` starts at 0 and becomes the previous drawn level's `z`.

Measured layers are drawn after the RainViewer layers.

### 3.6 Radar state summary

| State | Type | Updated |
|---|---|---|
| `radarUrl` | Option<String> | every 10 min while weather is on |
| `radarTiles`, `radarVersion` | tiles in view; counter | when the in-view tile set changes |
| `measured` | Vec<MeasuredVolume> | every 2 min (app only) |
| coverages | Vec<Coverage> | after `measured` changes |
| arenas | 7 x (slices, walls) | on `radarVersion` change while lifted |

---

## 4. Ships

Data shape `Ship` (api.ts:400-426): `{ mmsi: number, lat, lon, name: string (the MMSI as text when it never broadcast a name), kind: number, size: 0|1|2, heading: number|null (degrees clockwise from north; null when moored / not reported), old: bool, dims: [bow, stern, port, starboard] metres from the AIS reference point | null, frames: [lat, lon, unixMs][] }`.

### 4.1 Type colours and labels (ships.ts:12-31)

| kind | Label | RGB |
|---|---|---|
| 0 | Unknown type | 140, 152, 168 |
| 2 | Tug or special craft | 72, 198, 194 |
| 3 | Passenger | 98, 152, 240 |
| 4 | Cargo | 116, 188, 112 |
| 5 | Fishing | 236, 128, 156 |
| 6 | Tanker | 218, 92, 92 |
| 7 | Search and rescue or law enforcement | 228, 234, 242 |
| 8 | Pleasure craft or sailing | 192, 136, 236 |
| any other | Other | 140, 152, 168 (same as 0) |

`SHIP_COLOR_KEYS = [0, 2, 3, 4, 5, 6, 7, 8]` (JS object-key order: ascending integers). `shipKindKey(kind) = kind if listed else 0`.

Short labels for chips/rows (ship-list.ts:21): 2 -> "Tug or special", 7 -> "Rescue or law", 8 -> "Pleasure or sailing"; others use the full label.

### 4.2 Icon atlas (ships.ts:79-124)

Generated at runtime: a `128 x (64 * 8)` RGBA image; cell 64 px; one row per colour key in `SHIP_COLOR_KEYS` order; column 0 = hull, column 1 = dot. Icons are full colour (not masks); the instance colour's alpha fades them (and RGB multiplies, see 4.5).

Per row at `y = row * 64`, `fill = rgb(type colour)`, `stroke = INK = rgba(8, 12, 20, 0.9)`, line join round, **stroke first, then fill** (so only the outer half of the stroke shows):
- Hull (points up = north): polygon `(32, y+4) -> (47, y+24) -> (46, y+59) -> (18, y+59) -> (17, y+24)`, closed; stroke width 5 (visible border about 2.5 px at atlas scale). Mapping key `hull-<key>` = `{x: 0, y, w: 64, h: 64}`.
- Dot: circle centre `(64 + 32, y + 32)`, radius 20; stroke width 8 (visible about 4 px); same fill. Mapping key `dot-<key>` = `{x: 64, y, w: 64, h: 64}`.

Icon name: `shipIcon(ship) = (heading === null ? "dot" : "hull") + "-" + colorKey(kind)` (ships.ts:34).

In the port: rasterise once with tiny-skia (or bake to a PNG at build time), upload with linear filtering (mipmaps recommended: icons are drawn at 10-22 px from 64 px cells).

### 4.3 Icon size (ships.ts:37-44)

`shipIconSize(ship, zoom) = [15, 18, 22][size] * shipIconScale(zoom) * (heading === null ? 0.62 : 1)` pixels (the icon cell's height on screen; the cell is square).

`shipIconScale(zoom)`: `zoom < 3 -> 0.7`, `< 5 -> 0.85`, `< 7 -> 0.92`, else `1`.

### 4.4 Per-list data and motion (G:1596-1760)

Constants:

| Name | Value | Source |
|---|---|---|
| `SHIP_MESH_ZOOM` | 12.5 (always in the flat projection) | G:1596 |
| `SHIP_MESH_MIN_PX` | 14 | G:1598 |
| `SHIP_ALPHA` | 235 | G:1599 |
| `SHIP_OLD_ALPHA` | 105 | G:1601 |
| `SHIP_MESH_COLOR` | 255, 255, 255, 255 | G:1602 |
| `SHIP_MESH_OLD_COLOR` | 255, 255, 255, 120 | G:1603 |
| `SHIP_MESH_MATERIAL` | ambient 0.5, diffuse 0.48, shininess 24, specular colour 40, 40, 40 | G:1610 |
| `EASE_MS` | 2000 | G:1625 |

`shipsFor(list, nowMs)` (G:1705): rebuilt only when the list object changes. For each ship `i`:
- `color = [...shipColor(kind), old ? 105 : 235]`
- `blend = easeFrom(index of this mmsi in the previous data, previous positions, ship, nowMs)` (below)
- `moving` if `frames.length > 1` or it has a blend
- `position = [lon, lat, 0]`, `icon = shipIcon(ship)`
- If `heading !== null`: a model entry in its class group: `{ index, position (shared array with the icon), scale, heading, east, north, minZoom }` where `place = shipPlacement(ship)`, `[east, north] = offsetEastNorth(place.right, place.ahead, heading)`, and
  `minZoom = log2(metersPerPixel(0, lat) * SHIP_MESH_MIN_PX / place.length)` = the zoom at which the hull is 14 px long.
- A ship with `heading === null` never gets a model (no telling which way to draw it).

Motion along frames, shared with trains (trains.ts:80-134):
- `segment(frames, nowMs)`: none if empty; `[0,0,0]` if one frame or `now <= frames[0].t`; `[last,last,0]` if `now >= frames[last].t`; else the pair `i, i+1` around now with `f = (now - t0)/(t1 - t0)` (1 when `t1 <= t0`).
- `positionAt(v, nowMs)`: reported lat/lon when no frames; else linear interpolation between the two frames, longitude going the short way across the antimeridian and wrapped to [-180, 180]. **Held at the first frame before it starts and at the last after it ends; never extrapolated.**
- `headingAt(v, nowMs)`: initial great-circle bearing of the current leg (`i = min(segment.i, n-2)`), normalised to [0, 360); falls back to the reported heading when the leg has zero length or there are fewer than 2 frames.

Easing onto a new report (G:1624-1684), shared with trains:
- `aheadOf(p, from, headingDeg)` metres: `east = wrap180(p.lon - from.lon) * cos(from.lat) * 111320`, `north = (p.lat - from.lat) * 111320`, result `east * sin(h) + north * cos(h)`.
- `easeFrom(wasIndex, oldPositions, to, nowMs)`: null if the vehicle wasn't drawn before. `target = positionAt(to, now)`; `dLat = oldLat - target.lat`, `dLon = wrap180(oldLon - target.lon)`. Null if both are zero, or `|dLat| >= 0.05` deg or `|dLon| >= 0.1` deg (a jump). If `headingAt(to, now)` is known and the old drawn position is more than `0.05` m **ahead** of the target along that heading: return a *hold* `{dLat: 0, dLon: 0, from: now, hold: {lat, lon, heading}}` (never move backwards; wait there until the frames pass it). Otherwise `{dLat, dLon, from: now}`.
- `eased(v, blends, i, nowMs)`: base = `positionAt(v, now)`. With a hold: while the base is behind the hold point (`aheadOf(base, hold, hold.heading) < 0`) return the hold point; once past, delete the blend. With an offset blend: `left = 1 - min(1, (now - from) / 2000)`, `k = left^2 * (3 - 2*left)` (smoothstep), add `dLat*k`, `dLon*k`; delete when `left == 0`.

`moveShips` (G:1752): every frame, for each moving ship update `positions[i]` in place (icons and models share them); bump `shipMoves`.

### 4.5 Icon layer (G:1826-1876)

Layer `ship-icons`:
- Data: all ships below zoom 12.5; at zoom >= 12.5 only the ships not shown as models (see 4.6).
- Position: `positions[i]` (z = 0). Billboard (faces the screen), size in pixels = `shipIconSize(ship, zoom)`, icon anchored at its centre.
- Colour: `colors[i]` (type RGB, alpha 235 or 105). With `mask: false` deck.gl multiplies the texture by nothing in RGB and by `alpha/255` in alpha (the icon keeps the atlas colours; the instance alpha fades it). Note the atlas is already the type colour, so the instance RGB is redundant.
- Angle (degrees anticlockwise): 0 for heading-less ships and for ships beyond the visible radius (`cosAngle(ship, viewCentre) <= cos(visibleRadius(...))`); else `screenAngle(viewport, position, heading, step) ?? (bearing - heading)` with `step = ICON_HEADING_STEP_PX * mpp` metres (20 px at the view centre). Recomputed only when the camera (`zoom, bearing, pitch, longitude, latitude`) or the list changes (not while ships merely move along frames).
- Parameters: no cull, no depth write (depth test on, so the globe hides ships on the far side).
- Pickable. Hover highlight colour `[55, 142, 240, 200]`; the ship hovered in the sidebar list (`hoveredShipMmsi`) is highlighted the same way (`highlightedObjectIndex`).

### 4.6 3D models (G:1761-1812, ship-models.ts)

When `zoom >= 12.5`: `shipSplit(data, zoom)` with `step = floor(zoom * 4) / 4` (recomputed only when the quarter-zoom step changes): a ship with a model entry is drawn as a model when `step >= minZoom`; all others stay icons.

One instanced mesh layer per class (`ship-mesh-<class>`), each instance:
- position = the ship's (moving) position, z = 0
- orientation: yaw `yawOffset - heading` about the up axis where `yawOffset = flat ? 0 : 180` (always flat here in practice since 12.5 > 12); no pitch/roll. With the model's bow on +y and +x starboard, yaw `-heading` (anticlockwise positive) points the bow at compass `heading`.
- scale = `place.scale = [beam / model.beam, length, up]` (x across, y along, z up; metres per model unit)
- translation (metres, after rotation, in the local east/north frame) = `[sign * east, sign * north, 0]`, `sign = flat ? 1 : -1`. This moves the hull's centre so that the AIS reference point lands on the reported position.
- colour: `[255,255,255,255]`, or `[255,255,255,120]` for an old position; multiplied with the mesh's per-vertex colours.
- material: ambient 0.5, diffuse 0.48, shininess 24, specular `[40,40,40]`, lit by the scene's lights (sun, camera and ambient; described with the aircraft).
- no culling, depth test and **depth write on** (parameters `NO_CULL` only).
- pickable, auto-highlight on hover with `[55, 142, 240, 110]`; sidebar-hovered ship highlighted too.

#### Placement (ship-models.ts:540-649)

`usableDims(dims)`: null unless `3 <= length <= 500` and `1 <= beam <= 0.7 * length` (length = bow + stern, beam = port + starboard).

`shipClassOf(ship)`: with `length` = usable dims length or, when unknown, `[0, 0, +inf][size]` (so only "large" counts as long):
- kind 2: `length >= 55 ? generic : tug`
- kind 3: `passenger`
- kind 4: `length < 150 ? cargo : (mmsi % 3 == 0 ? bulk : container)`
- kind 5: `length >= 70 ? generic : fishing`
- kind 6: `tanker`
- kind 7: `length >= 45 ? generic : sar`
- kind 8: `length >= 30 ? passenger : pleasure`
- else `generic`

`TYPICAL_LENGTH` metres by size bucket [small, medium, large]:

| class | small | medium | large |
|---|---|---|---|
| container | 140 | 220 | 330 |
| tanker | 90 | 180 | 270 |
| bulk | 130 | 190 | 250 |
| cargo | 60 | 95 | 130 |
| passenger | 40 | 110 | 230 |
| tug | 20 | 28 | 36 |
| fishing | 14 | 24 | 45 |
| pleasure | 8 | 12 | 18 |
| sar | 10 | 16 | 24 |
| generic | 25 | 60 | 120 |

`shipPlacement(ship)`: with usable dims: `length = bow + stern`, `beam = port + stbd`, `ahead = (bow - stern)/2`, `right = (stbd - port)/2`. Without: `length = TYPICAL_LENGTH[cls][size]`, `beam = length * model.beam`, offsets 0. `up = min(length, MAX_SUPERSTRUCTURE_M / model.top)` with `MAX_SUPERSTRUCTURE_M = 40`. `scale = [beam / model.beam, length, up]`.

`offsetEastNorth(right, ahead, heading)`: `east = right*cos(h) + ahead*sin(h)`, `north = -right*sin(h) + ahead*cos(h)`.

#### Procedural meshes (ship-models.ts:82-506)

Model space: 1 unit long, bow along +y (y from -0.5 stern to +0.5 bow), +x starboard, +z up, z = 0 at the waterline. Flat shaded: 3 unique vertices per triangle with position, normal and RGB colour (0..1). Triangles are wound and their normal flipped so it faces away from a given "inside" point (`tri`, 88-103).

Paints (RGB): WHITE 236,238,240; OFF_WHITE 214,216,212; WINDOW 34,44,62; ANTIFOULING 112,34,30; BOOT_TOP 26,26,30; STEEL_DECK 104,108,106; RUST_DECK 128,70,56; TEAK 176,138,96; FUNNEL 44,46,52; CRANE 218,176,60; MAST 200,202,204; ORANGE 236,112,36. CONTAINERS (9): 178,58,48; 46,92,150; 64,130,84; 214,140,52; 150,150,146; 222,222,214; 92,70,120; 40,120,150; 190,160,60. `shade(c, k) = min(255, c*k)` per channel. `topsidesOf(kind) = shade(shipColor(kind), 0.82)`.

Builder primitives:
- `solid(corners[8], color, top = color)`: bottom 4 then top 4; six quads; the top face may have a different colour.
- `box(min, max, color, top)`; `block(halfWidth, y0, y1, z0, z1, color, top)` = box centred on the centreline.
- `rod(a, b, r, color)`: square prism of half-thickness `r` from a to b.
- `hull(spec)` (174-260): defaults `sheerBow = freeboard*0.5`, `sheerStern = freeboard*0.15`, `bowLength = 0.22`, `transom = 0.85`, `rake = 0.03`, `overhang = 0.02`, `bottom = 0.75`, `rail = topsides`, `railScale = 1`.
  - `half = beam/2`, `boot = min(freeboard*0.25, 0.006)`, `railHeight = min(freeboard*0.18, 0.006) * railScale`, `railWidth = beam*0.04*railScale`.
  - Stations `s` in [0,1]: `i/16` for i = 0..16 plus `1 - bowLength + bowLength*i/6` for i = 1..5, sorted.
  - Half-width factor `width(s)`: in the bow (`fromBow = (s - (1 - bowLength))/bowLength > 0`): `max(0, 1 - fromBow^1.7)^0.75`; for `s < 0.14`: `lerp(transom, 1, sqrt(s/0.14))`; else 1.
  - Deck height `deckZ(s) = freeboard + sheerBow * max(0, (s-0.55)/0.45)^2 + sheerStern * max(0, (0.15-s)/0.15)^2`.
  - Per station (starboard side, mirrored for port): `deck = [w, -0.5+s, deckZ]`, `boot = [0.97w, lerp(yWater, yDeck, boot/deckZ), boot]`, `water = [0.94w, yWater, 0]`, `keel = [w*bottom, yKeel, -draft]` with `w = width(s)*half`, `yWater = lerp(-0.5+overhang, 0.5-rake, s)`, `yKeel = lerp(-0.5+2*overhang, 0.5-1.6*rake, s)`.
  - Between consecutive stations, both sides: deck-to-boot quad (topsides colour), boot-to-water (BOOT_TOP), water-to-keel (ANTIFOULING); the deck quad across (deck colour); the keel quad across (ANTIFOULING); and a bulwark: outside face from deck edge up `railHeight`, top face inward `railWidth`, inside face back down (`shade(railColor, 0.8)`).
  - Transom at station 0: three quads across (topsides, BOOT_TOP, ANTIFOULING).
  - Returns `deckZ(y + 0.5)`.
- `aftBridge(m, {beam, y0, y1, z0, height, funnel = true})` (307-322): `hw = beam*0.4`, `top = z0 + height`; deckhouse block `hw, y0..y1, z0..top - 0.12h` WHITE; wheelhouse block `0.92hw, y0 + 0.25(y1-y0)..y1, top-0.12h..top` WHITE; window band box `x +/-0.93hw, y1-0.002..y1+0.001, top-0.1h..top-0.03h` WINDOW; bridge wings box `x +/-0.5beam, y1-0.35(y1-y0)..y1, top-0.13h..top-0.1h` OFF_WHITE; funnel block `0.1beam, fy..fy+0.4(y1-y0), top-0.3h..top+0.12h` FUNNEL with `fy = y0 - 0.05(y1-y0)`.
- `seeded(seed)`: LCG `s = (s*1664525 + 1013904223) mod 2^32`, returns `s / 2^32`.

Classes (exact calls; `fb` = freeboard):

- **container** (324-351): beam 0.14, fb 0.04. `hull{draft 0.02, topsides topsidesOf(4), deck STEEL_DECK, bowLength 0.25, sheerBow 0.02}`. Forecastle `block(0.32beam, 0.36, 0.43, fb, fb+0.022, OFF_WHITE)`. `rand = seeded(7)`; rows 5, bayLength 0.036, gap 0.005. `stack(y0, y1, narrow)`: for r in 0..5: `x0 = -0.46*beam*narrow + (0.92*beam*narrow/5)*r`, `x1 = x0 + 0.92*beam*narrow/5 - 0.0012`, `tiers = 3 + floor(rand()*4)`, `color = CONTAINERS[floor(rand()*9)]` (rand called in this order: tiers then colour), box from fb to `fb + tiers*0.0085`. Bays: `for (y = 0.34; y - 0.036 > -0.24; y -= 0.041) stack(y-0.036, y, y > 0.22 ? 0.7 : 1)`; then `stack(-0.42, -0.384, 0.85)`. `aftBridge{y0 -0.36, y1 -0.28, z0 fb, height 0.085}`. `top = fb + 0.085*1.12`.
- **tanker** (353-366): beam 0.17, fb 0.032. `hull{draft 0.022, topsides topsidesOf(6), deck RUST_DECK, bowLength 0.2, transom 0.9}`. Forecastle `block(0.3beam, 0.38, 0.45, fb, fb+0.018, OFF_WHITE)`. Pipe run `block(0.06beam, -0.3, 0.38, fb, fb+0.006, OFF_WHITE)`. Manifold `box(+/-0.42beam, -0.02..0.02, fb..fb+0.008, OFF_WHITE)`. `box(+/-0.1beam, 0.06..0.075, fb..fb+0.01, CRANE)`. Catwalk posts: `for (y = -0.25; y < 0.36; y += 0.08) block(0.02beam, y, y+0.008, fb, fb+0.012, OFF_WHITE)`. `aftBridge{y0 -0.42, y1 -0.33, height 0.08}`. `top = fb + 0.08*1.12`.
- **bulk** (`bulk(false)`) / **cargo** (`bulk(true)`, with cranes) (369-393): beam 0.16; fb 0.055 (cargo) or 0.045 (bulk). `hull{draft 0.022, topsides topsidesOf(4), deck cargo ? STEEL_DECK : RUST_DECK, bowLength 0.2}`. Forecastle `block(0.34beam, 0.38, 0.45, fb, fb+0.02, OFF_WHITE)`. Holds: 4 (cargo) or 7 (bulk); span 0.62 from start -0.27; `each = span/holds`; hatch `block(0.32beam, start + i*each + 0.08each, start + (i+1)*each - 0.08each, fb, fb+0.012, side, top)` with side/top = `[64,92,120]/[84,116,148]` (cargo) or `[150,104,70]/[170,120,80]` (bulk). Cargo only, between holds (i < holds-1): post `rod([x, y, fb], [x, y, fb+0.07], 0.005, CRANE)` and jib `rod([x, y, fb+0.055], [0.3x, y + 0.85each, fb+0.1], 0.0028, CRANE)` with `y = hatchEnd + 0.08each`, `x = (i odd ? -1 : 1) * 0.18beam`. `aftBridge{y0 -0.42, y1 -0.33, height 0.075}`. `top = fb + 0.075*1.12`.
- **passenger** (395-416): beam 0.15, fb 0.045. `hull{draft 0.02, topsides topsidesOf(3), deck TEAK, bowLength 0.26, sheerBow 0.015, transom 0.9}`. 6 tiers of height 0.021 starting at z = fb: tier i: `front = 0.3 - 0.022i`, `back = -0.43 + 0.012i`, `hw = beam*(0.49 - 0.012i)`; `block(hw, back, front, z, z+0.021, WHITE)`; side windows `box(+/-(hw+0.0012), back+0.01..front-0.008, z+0.35t..z+0.7t, WINDOW)`; front windows `box(+/-0.9hw, front-0.002..front+0.0012, z+0.35t..z+0.7t, WINDOW)`. Funnel `block(0.14beam, -0.27, -0.17, z, z+0.03, topsidesOf(3), FUNNEL)`. `top = z + 0.03` (= fb + 6*0.021 + 0.03).
- **tug** (418-434): beam 0.34, fb 0.07. `hull{draft 0.05, sheerBow 0.07, sheerStern 0.01, bowLength 0.32, transom 0.8, bottom 0.7, topsides topsidesOf(2), deck STEEL_DECK, rake 0.02, overhang 0.03, rail BOOT_TOP, railScale 2.5}`. Deckhouse `block(0.32beam, -0.08, 0.2, fb, fb+0.1, WHITE)`; wheelhouse `block(0.27beam, -0.02, 0.18, fb+0.1, fb+0.19, WHITE, top topsidesOf(2))`; windows `box(+/-0.275beam, -0.025..0.185, fb+0.13..fb+0.17, WINDOW)`; stacks `box(-0.12beam..-0.02beam, -0.17..-0.11, fb..fb+0.17, FUNNEL)` and mirrored `0.02beam..0.12beam`; tow hook `block(0.12beam, -0.36, -0.28, fb, fb+0.04, CRANE)`; mast `rod([0,0.08,fb+0.19],[0,0.08,fb+0.3],0.006,MAST)`. `top = fb + 0.19`.
- **fishing** (436-453): beam 0.28, fb 0.065. `hull{draft 0.045, sheerBow 0.06, sheerStern 0.01, bowLength 0.3, transom 0.75, bottom 0.6, topsides topsidesOf(5), deck STEEL_DECK}`. `block(0.34beam, 0.08, 0.3, fb, fb+0.08, WHITE)`; `block(0.3beam, 0.12, 0.28, fb+0.08, fb+0.15, WHITE)`; windows `box(+/-0.305beam, 0.2..0.282, fb+0.1..fb+0.135, WINDOW)`; mast `rod([0,0.06,fb],[0,0.06,fb+0.32],0.006,MAST)`; A-frame: `gz = fb+0.22`; legs for x = -1, 1: `rod([x*0.4beam,-0.45,fb],[x*0.36beam,-0.4,gz],0.008,ORANGE)`; crossbar `rod([-0.37beam,-0.4,gz],[0.37beam,-0.4,gz],0.008,ORANGE)`; winch `block(0.22beam, -0.2, -0.12, fb, fb+0.04, CRANE)`. `top = fb + 0.15`.
- **pleasure** (455-468): beam 0.3, fb 0.07. `hull{draft 0.04, sheerBow 0.03, bowLength 0.48, transom 0.8, bottom 0.35, rake 0.05, overhang 0.04, topsides topsidesOf(8), deck TEAK}`. Coachroof `block(0.28beam, -0.12, 0.2, fb, fb+0.04, WHITE)`; windows `box(+/-0.285beam, -0.08..0.16, fb+0.015..fb+0.03, WINDOW)`; cockpit `box(+/-0.3beam, -0.42..-0.2, fb..fb+0.005, topsidesOf(8))`; mast `rod([0,0.12,fb+0.04],[0,0.12,0.95],0.007,MAST)`; boom `rod([0,0.12,fb+0.1],[0,-0.3,fb+0.1],0.006,MAST)`. `top = fb + 0.04`.
- **sar** (470-482): beam 0.3, fb 0.06. `hull{draft 0.035, sheerBow 0.035, bowLength 0.4, transom 0.92, bottom 0.3, rake 0.04, topsides [44,62,104], deck STEEL_DECK}`. Cabin `block(0.36beam, -0.18, 0.2, fb, fb+0.1, ORANGE)`; screen `box(+/-0.365beam, -0.05..0.205, fb+0.055..fb+0.085, WINDOW)`; `block(0.28beam, -0.12, 0.08, fb+0.1, fb+0.14, ORANGE)`; mast `rod([0,-0.05,fb+0.14],[0,-0.05,fb+0.25],0.006,MAST)`; radar bar `box(+/-0.2beam, -0.065..-0.035, fb+0.23..fb+0.24, MAST)`. `top = fb + 0.14`.
- **generic** (484-493): beam 0.18, fb 0.06. `hull{draft 0.025, bowLength 0.25, topsides topsidesOf(0), deck STEEL_DECK}`. `block(0.3beam, 0.36, 0.44, fb, fb+0.025, OFF_WHITE)`; `aftBridge{y0 -0.38, y1 -0.22, height 0.09}`; `block(0.3beam, -0.1, 0.25, fb, fb+0.015, [120,124,120])`. `top = fb + 0.09*1.12`.

Each class returns `{positions, normals, colors, beam, top}`; `beam` and `top` feed `shipPlacement`.

### 4.7 True-scale hull outline (ships.ts:46-77)

`hullPolygon(lon, lat, dims, heading)`: 5 points `[lon, lat]`: bow tip `((stbd - port)/2, bow)`, starboard shoulder `(stbd, bow - 0.2*length)`, starboard stern `(stbd, -stern)`, port stern `(-port, -stern)`, port shoulder `(-port, shoulder)`; (right, ahead) metres rotated by heading, converted with 111 320 m/deg and `max(0.01, cos lat)`. (Exported helper; not used by the layers in this range. Check other documents before porting.)

### 4.8 Selection ring (G:3296-3321)

Only when a ship is selected, and (on the globe, zoom <= 12) only while `cosAngle(ship, viewCentre) > 0`. Position: the ship's **reported** `[lon, lat, 0]` (not the eased position). Radius px = `max(shipIconSize(ship, zoom)/2 + 5, hullPx + 6)` with `hullPx = dims ? max(dims[0], dims[1]) / mpp : 0` (note: max of bow and stern distances, not the full length). Two billboarded circles, depth compare always, no depth write: halo (filled `[55,142,240,56]`, drawn *under* the ships) and ring (stroke `[55,142,240,235]`, 2 px, drawn *over* the ships).

---

## 5. Trains

Data shape `Train` (api.ts:618-658): `{ id: string, source: "trafikverket", name, kind: "highSpeed"|"intercity"|"regional"|"suburban"|"other", lat, lon, heading: number|null, speedKmh: number|null, destination: string|null, operator: string|null, delayMinutes: number|null, positionUnix: seconds, frames: [lat, lon, unixMs][], reports?: Sighting[] (dev), predicted?: frames (dev), cars?: number|null, tail?: [lat, lon][] (track behind the start of frames, nearest first), structures?: Structure[], endedUnix?: number|null, shape?: "highSpeed"|"doubleDeck"|"regional"|"loco"|"railcar", model?: string }`.

A train is shown while `endedUnix == null || nowMs - endedUnix*1000 <= 30_000` (`TRAIN_ENDED_MS`, trains.ts:155-158; applied by the page, `+page.svelte:765`).

### 5.1 Colours (trains.ts:12-56)

Kind colours (display order = this order):

| kind | Label | RGB |
|---|---|---|
| highSpeed | High-speed | 226, 92, 196 |
| intercity | Intercity | 128, 132, 246 |
| regional | Regional | 96, 212, 160 |
| suburban | Suburban | 112, 190, 232 |
| other | Other | 158, 150, 140 |

Operator colours: first regex that matches the trimmed operator name, case-insensitive (index = atlas row `op<i>`):

| i | Pattern | RGB |
|---|---|---|
| 0 | `^sj\b` | 232, 232, 236 |
| 1 | `^sl\b` or `storstockholms` | 70, 146, 236 |
| 2 | `mälartåg` | 236, 84, 84 |
| 3 | `västtrafik` or `västtåg` | 62, 200, 236 |
| 4 | `skånetrafiken` or `pågatåg` | 196, 92, 222 |
| 5 | `öresundståg` | 150, 132, 246 |
| 6 | `arlanda express` | 250, 212, 64 |
| 7 | `snälltåget` | 120, 214, 92 |
| 8 | `norrtåg` | 58, 204, 176 |
| 9 | `^vy\b` | 240, 92, 150 |
| 10 | `bergslagen` | 244, 152, 56 |
| 11 | `upptåget` or `^ul\b` | 250, 120, 40 |
| 12 | `östgöta` | 214, 70, 130 |
| 13 | `x-trafik` or `x-tåget` | 150, 220, 60 |
| 14 | `krösatåg`, `kronoberg`, `jönköping`, `kalmar` or `blekinge` | 236, 120, 110 |
| 15 | `värmland` | 110, 180, 255 |

(JS `\b` and the `i` flag here are ASCII-oriented: `/i` without `/u` does not case-fold non-ASCII letters the same way in all engines; use Unicode-aware case-insensitive matching in Rust and keep word-boundary semantics for `sj`, `sl`, `vy`, `ul`.)

`trainColorOf(train)` = operator colour if any, else kind colour. Drawn alpha `TRAIN_ALPHA = 240` (G:1881); in a tunnel `TUNNEL_TRAIN_ALPHA = 90` (G:1884).

### 5.2 Icon atlas (trains.ts:195-258)

`128 x (64 * 21)` RGBA: one row per kind (5, in `TRAIN_KINDS` order) then one per operator (16). Column 0 = carriage (`car-<key>`), column 1 = rounded square (`box-<key>`). `key` = `op<i>` when the operator has a colour, else the kind (unknown kinds -> `other`). Stroke `INK = rgba(8, 12, 20, 0.9)`, line join round; stroke first then fill.

Per row at `y = row*64`, with `left = 19, right = 45, top = y + 4, bottom = y + 60`:
- Carriage path (26 wide, 56 long, pointing up): `moveTo(32, top)`; `bezierCurveTo(40, top+2, right, top+9, right, top+18)`; `lineTo(right, bottom-3)`; `quadraticCurveTo(right, bottom, right-3, bottom)`; `lineTo(left+3, bottom)`; `quadraticCurveTo(left, bottom, left, bottom-3)`; `lineTo(left, top+18)`; `bezierCurveTo(left, top+9, 24, top+2, 32, top)`; close. Stroke width 5, then fill with the row colour.
- Two joints across it, drawn after the fill in INK, line width 2.5: horizontal lines from `(left+1, j)` to `(right-1, j)` for `j = top+25` and `j = top+41`.
- Rounded square: `roundRect(64 + 14, y + 14, 36, 36, radius 9)`, stroke width 6, then fill.

Icon name `trainIcon(kind, heading, operator) = (heading === null ? "box" : "car") + "-" + key`.

### 5.3 Icon size (trains.ts:178-193)

`trainIconSize(kind, heading, zoom) = ICON_PX[kind] * trainIconScale(zoom) * (heading === null ? 0.8 : 1)` px (cell height), with `ICON_PX = { highSpeed: 24, intercity: 23, regional: 21, suburban: 20, other: 19 }` and `trainIconScale(zoom)`: `< 4 -> 0.7`, `< 6 -> 0.82`, `< 9 -> 0.92`, else `1`.

### 5.4 Per-list data and motion (G:1878-1949)

`trainsFor(list, nowMs)`: rebuilt when the list object changes. Per train: position `[lon, lat, 0]`, heading (reported), colour `[...trainColorOf, 240]`, `level = structures?.length ? levelAt([lon, lat], heading, structures) : ON_THE_GROUND` (see 9.3), `blend = easeFrom(...)` keyed by train id (same easing as ships, 4.4), `moving` if `frames.length > 1` or it has a blend.

`moveTrains` every frame for moving trains: `position = eased(...)`, `heading = headingAt(train, now)`, `level` recomputed when the train has structures. `trainMoves` bumped (everything position-dependent is re-uploaded).

### 5.5 Icons vs cars (G:2031-2071)

`mpp = metersPerPixel(zoom, view.latitude)`. Cars are drawn when `zoom > 12 && CAR_M / mpp >= CARS_MIN_PX`, i.e. `26 / mpp >= 3` (a car at least 3 px long; `mpp <= 8.67` m/px, which is about zoom 12.2 at 59 deg N and zoom 13.14 at the equator). Otherwise icons.

Nothing is drawn until the atlas exists (even cars: `if (!trainAtlasData) return []`).

Icon layer `train-icons`:
- All trains. Position `positions[i]`; size px as 5.3 using the *drawn* heading.
- **Not billboarded** (`billboard: false`): the icon lies flat on the ground, rotated in the ground plane: angle (anticlockwise degrees) = `yaw0 - heading` (heading null -> 0), `yaw0 = zoom > 12 ? 0 : 180` (the globe's local ground frame is turned half a turn from the flat one). So on a tilted view the icon is foreshortened with the ground; its size is still in pixels (the quad is sized in pixels but oriented in the ground plane).
- Colour: `colors[i]`, or with alpha 90 when `levels[i].tunnel`.
- No cull, no depth write.
- Pickable; hover highlight `[55, 142, 240, 200]`; `hoveredTrainId` (sidebar hover) highlights the same way.

### 5.6 Cars (train-cars.ts, train-models.ts, G:1950-2029)

Constants: `CAR_M = 26` (coach with coupling), `GAP_M = 0.6`, `DEFAULT_CARS = 4` (train-cars.ts:4-8); `CAR_WIDTH_M = 3.1`, `CAR_HEIGHT_M = 4.1` (train-models.ts:13-14); `CARS_MIN_PX = 3`, `MIN_CAR_WIDTH_PX = 3` (G:1956-1958); `MODEL_WHITE = [255,255,255,255]`; `CAR_MESH_MATERIAL = { ambient 0.45, diffuse 0.6, shininess 40, specular [70,70,70] }` (G:1960).

`drawnCars(data, nowMs)` (G:1970-1995):
- `viewBounds()` = the flat viewport's `[west, south, east, north]` (null on the globe). Trains whose drawn position is outside the bounds padded by 0.01 deg are skipped.
- `count = max(1, train.cars ?? 4)`.
- If the train has a loaded model (`modelCar(train.model, 0, count)` not null): each car's length is the model car's own `length`, and the gap is 0. Otherwise default length `CAR_M - GAP_M = 25.4` and gap 0.6.
- `places = carPlaces(carLines(frames, tail, count, nowMs, lengthOf, gap))`.
- Fallback when that doesn't yield exactly `count` cars (no frames, or cars too short): a straight row back along the heading: `behind = [lat - cos(h)/1000, lon - sin(h)/1000/cos(lat)]` (about 111 m back; `carLines` extrapolates further back along it), `carLines([[lat, lon, now]], [[lat, lon], behind], ...)`.
- Each car: `{ at: [lon, lat], heading, length, train: i, mesh: model(k) ?? carMesh(k == 0, train.shape), modelled, level: structures.length ? levelAt(place.at, place.heading, structures) : ground }`.

`carLines(frames, tail, cars, nowMs, lengthOf, gap)` (train-cars.ts:23-77):
1. `path` (lat/lon) = `tail.slice(1)` reversed (so it runs toward the train; `tail[0]` is the start of frames and is dropped), then `frames[0]`, then each later frame that differs from the last path point. `lead = index of frames[0] in path`.
2. Cumulative metres `at[]` along the path (flat metres, `cos` of the segment's first latitude).
3. Head distance: `head = at[lead]`; for each frame leg `(a, b)`: stop if `now <= a.t`; add `metres(a, b) * k` with `k = min(1, (now - a.t)/(b.t - a.t))` (1 when `b.t <= a.t`); stop if `now < b.t`.
4. `point(s)`: position at `s` metres along the path; for `s <= 0` extrapolate straight back along the first segment; beyond the end clamps to the last point.
5. For each car `c` from the front: `back = front - lengthOf(c)`; line = `[point(back), ...path vertices strictly between back and front, point(front)]`, emitted as `[lon, lat]`; then `front = back - gap`.

`carPlaces(lines)` (train-models.ts:193-204): for each line with >= 2 points: `back = first`, `front = last`; `east = (front.lon - back.lon) * cos(midLat) * 111320`, `north = (front.lat - back.lat) * 111320`; `length = hypot`; dropped when `< 0.5` m; `heading = (atan2(east, north) in degrees + 360) % 360`; `at` = midpoint of back and front. Note the car is the **chord** between its ends (length = chord length, slightly shorter on curves).

Car layers (`carLayers`, G:1998-2029): `width = max(3.1, 3 * mpp)` metres, `height = max(4.1, 3 * 1.3 * mpp)` metres (never thinner than 3 px, 3.9 px tall). For each distinct mesh key among the cars in view, two instanced mesh layers: `train-car-meshes-<key>` (cars with `level.tunnel == false`) and `train-car-meshes-<key>-tunnel` (cars in tunnels):
- position `[lon, lat, level.height]` (metres above ground: 0 or up on a bridge)
- orientation yaw `-heading` (flat projection only)
- scale `[width, car.length, height]` (mesh is a unit box: x in [-0.5, 0.5], y in [-0.5, 0.5] with the front at +0.5, z in [0, 1])
- colour: plain cars take the operator/kind colour `[r, g, b, 240]` (multiplied by the mesh's grey-scale vertex shades); model cars are `[255,255,255,255]` (their own livery in vertex colours). In a tunnel the alpha becomes 90.
- material as `CAR_MESH_MATERIAL`.
- parameters: normal layers no cull (depth test + write on); tunnel layers `THROUGH_GROUND` (no cull, no depth write, depth compare always: seen faintly through the ground and whatever stands on it).
- pickable, auto-highlight `[55, 142, 240, 110]`.

#### Plain car meshes (train-models.ts:16-132)

Unit box, flat shaded, vertex colour = a grey shade `s` (r = g = b = s) multiplied by the instance colour. `box(x, y, z ranges, {side, end = side, top = side})` emits 6 faces (two triangles each, indices 0,1,2,0,2,3) with normals +x, -x, +y, -y, +z, -z; the bottom face always has shade 0.2.

Shapes (`SHAPES`): `nose` = y where the leading car's nose starts; `windows` = list of `[z0, z1]` window rows:

| shape | nose | windows |
|---|---|---|
| highSpeed | 0.30 | [0.5, 0.72] |
| doubleDeck | 0.44 | [0.3, 0.46], [0.6, 0.76] |
| regional | 0.42 | [0.5, 0.72] |
| loco | 0.47 | none |
| railcar | 0.47 | [0.5, 0.72] |

Following cars use `coachShape(shape)` = `doubleDeck` for double-deckers, else `regional`. Mesh keys: lead `"<shape>-cab"`, others `"<coachShape>-coach"`. Default shape `regional`.

`carArrays(lead, shape)`:
1. `front = lead ? nose : 0.5`.
2. Underframe: `box(x [-0.4, 0.4], y [-0.47, 0.47], z [0, 0.2], side 0.22)`.
3. Body bands from `z = 0.2`: for each `[z0, z1]` in `windows ++ [[0.88, 0.88]]`: if `z0 > z`: body `box(x [-0.5, 0.5], y [-0.5, front], z [z, z0], side 1, end 0.85, top 0.9)`; if `z1 > z0`: window band `box(same x/y, z [z0, z1], side 0.16, end 0.3)`; `z = z1`.
4. Roof: `box(x [-0.42, 0.42], y [-0.48, front - 0.02], z [0.88, 1], side 0.8, top 0.75)`.
5. Lead only, the nose from `y0 = front` to `y1 = 0.5`: `yAt(z) = y1 - (y1 - y0) * ((z - 0.2)/0.68)`; three sloped quads across the full width (x -0.5..0.5) between heights: `[0.2, 0.5]` shade 1, `[0.5, 0.8]` shade 0.12 (windscreen), `[0.8, 0.88]` shade 1; normal `(0, 0.68, y1 - y0)` normalised; plus two side triangles `(x, y0, 0.2), (x, y1, 0.2), (x, y0, 0.88)` at x = +/-0.5 (reversed for x < 0), shade 1, normal `(sign x, 0, 0)`.

#### Operator models from `static/trains` (train-models.ts:134-183, scripts/trains/build.mjs)

Files: `static/trains/index.json` (2.2 kB) and `static/trains/meshes.bin` (1 938 232 bytes). Loaded once at startup (`loadTrainModels()`, G:3502); until loaded (or on failure) plain cars are drawn.

`index.json`: `{ "models": { "<name>": { "cars": [ { "length": metres, "vertices": n, "positions": byteOffset, "normals": byteOffset, "colors": byteOffset } x 3 ] } } }`.

`meshes.bin`, per car (unindexed triangle list, 3 vertices per triangle, flat shaded, sections 4-byte aligned):
- positions: `vertices * 3` little-endian `f32` at `positions`
- normals: `vertices * 3` `i8` at `normals`, value / 127
- colours: `vertices * 3` `u8` at `colors`, value / 255 (RGB)

Each car is already in the unit box used by the plain cars (x across / 3.1 m, y along / its own length with the front at +y, z up / 4.1 m). Normals were pre-divided by the draw scale `[3.1, length, 4.1]` so that after the instance scale is applied (inverse-transpose handling in deck.gl's mesh shader: normals are multiplied by the scale again and normalised) lighting is as on the car in metres. Note cars far out are drawn wider/taller than true (`width`/`height` above), which slightly skews those normals; acceptable.

Models and car lengths (metres) `[lead, middle, last]`:

| model | lead | middle | last | vertices |
|---|---|---|---|---|
| contessa | 26.2 | 26.6 | 26.3 | 4629, 4602, 4602 |
| x40 | 27.2 | 26.4 | 27.8 | 4416, 4506, 4710 |
| x60 | 20.35 | 16.4 | 20.75 | 4314, 4548, 4404 |
| x61-skane | 20 | 17 | 20.7 | 4569, 4509, 4677 |
| x61-vasttrafik | 20 | 17 | 20.7 | 4557, 4287, 4287 |
| sj-intercity | 15.1 | 26.7 | 26.7 | 4350, 4278, 4461 |
| x55 | 26.6 | 26.6 | 27.2 | 4392, 4698, 4488 |
| x2000 | 17.7 | 25 | 22.7 | 4425, 4329, 4638 |

`modelCar(model, k, count)`: car index `k == 0 -> cars[0]`, `k == count - 1 -> cars[2]`, else `cars[1]`; null when the train has no model or it isn't loaded. Mesh key `"<model>-<0|1|2>"`. (A one-car train with a model uses the lead car.) Which train takes which model is decided in the backend (`src-tauri/src/train_class.rs`, `model_of`).

Credit required wherever models are drawn: text "3D trains: Lardman", url `https://3dwarehouse.sketchup.com/user/1082279160696486794315506/Lardman` (`MODEL_CREDIT`, trains.ts:67-70); `trainCredits(trains)` = each source's credit once ("Trains in Sweden: Trafikverket, CC0", `https://data.trafikverket.se/`) plus the model credit if any train has a `model`.

### 5.7 Selection ring (G:3327-3354)

As for ships but at the train's **drawn** position (and drawn heading): radius px = `round(trainIconSize(kind, heading, zoom)/2 + 5)` (not enlarged for cars); hidden on the far side of the globe. Halo `[55,142,240,56]` under the trains, ring `[55,142,240,235]` 2 px over them; billboarded; depth compare always.

### 5.8 Text helpers (trains.ts)

- `delayText(min)`: `""` when null/non-finite; `"on time"` at 0; `"{n} min late"`; `"{n} min early"`.
- `delayTone(min)`: null; `>= 30 -> "bad"`; `>= 5 -> "warn"`; else `"good"`.
- `TRAIN_STALE_S = 300`; `trainPositionAge(positionUnix, nowMs)`: `age = max(0, now/1000 - positionUnix)`; stale when `age > 300`: `"Position {ageLabel} old"`, else `"Position {ageLabel} ago"`.
- `ageLabel(seconds)` (freshness.ts:12): `< 60 s -> "{s} s"`; `< 60 min -> "{m} min"`; else `"{h} h"` or `"{h} h {mm} min"` (mm zero-padded to 2).
- `TRAIN_SOURCE_NAMES = { trafikverket: "Trafikverket" }`.
- `stopView(stop)`: `t = departure ?? arrival`; `planned = t.planned`; `time = t.actual ?? t.estimated ?? planned`; `late = round((time - planned)/60)` when both known and different, else null; `passed = departure ? bool(departure.actual) : bool(arrival?.actual)`.
- `nextStop(stops)`: index after the last passed stop, or -1 when that is past the end.
- `trainProduct(train)`: for Trafikverket names not starting with `"Train "`: the name with its last whitespace-separated token removed (e.g. "Öresundståg 1042" -> "Öresundståg"); null if nothing is left or it equals the name.

---

## 6. Signal blocks and signals overlay (G:2164-2413, trains.ts:301-341)

Shown when the `trainBlocks` prop is on, in the flat projection, at `zoom >= BLOCKS_MIN_ZOOM = 13`.

Backend: `train_blocks({west, south, east, north}) -> { blocks: TrainBlock[], structures: Structure[], complete: bool }`; `TrainBlock = { id: string, signal: string | null, lines: [lon, lat][][] }` (a block from a main signal to the next ones; several stretches where the track branches; `lines[0][0]` is where the block starts = its signal).

### 6.1 Fetching (`refreshBlocks`, G:2219-2244)

Called every frame from `blockLayers()`. Skip when off, on the globe, zoom < 13, or a request is in flight. With `b = viewBounds()`:
- `inside` = the view lies within the fetched set's area.
- If inside and (`complete` or the last ask was < 2500 ms ago): nothing.
- If not inside and the last ask was < 700 ms ago: nothing.
- Else ask for the view padded by 15 % of its width/height on each side.
- On answer: if the answer is incomplete and has fewer blocks than what is on show, keep the old set and just mark it `complete = false` (prevents flicker while loading); else replace the set (`{area, list, structures, complete}`) and force the taken-blocks recomputation (`blocksTaken.at = 0`). Errors are ignored.

### 6.2 Which blocks are taken (`takenBlocks`, G:2247-2262; `blockAt`, trains.ts:332-341)

Recomputed at most every 500 ms. For each train whose drawn position is inside the view bounds: `block = blockAt([lon, lat], blocks, previousBlockOfThisTrain)`:
- distance to a block = min over its lines of the flat point-to-polyline distance in metres;
- candidates: blocks within `IN_BLOCK_M = 12` m; nearest wins;
- hysteresis: the train stays in its previous block if that block's distance `<= min(12, nearest.d + KEEP_BLOCK_M)` with `KEEP_BLOCK_M = 2`;
- none within 12 m -> not in a block.

Results: `by: blockId -> "name1, name2"` (train names joined with `", "`), `of: trainId -> blockId`.

### 6.3 Geometry

Computed once per block list: for every line of every block, `onStructures(line, structures)` (9.4) splits it into runs `{tunnel, path: [lon, lat, heightMetres][]}`: `above` (ground and bridges, with heights) and `below` (tunnels).

Signals (`signalsOf`, G:2390-2413): one per block at `b.lines[0][0]`, `name = b.signal`, `next` = ids of other blocks whose start lies within `BLOCK_JOIN_M = 6` m of the end of any of this block's lines (found via a hash of starts in cells of 0.001 deg, checking the 3 x 3 neighbouring cells).

Aspect: `stop` if the signal's own block is taken; else `caution` if any `next` block is taken; else `clear`.

### 6.4 Layers, in order

| Layer | What | Style |
|---|---|---|
| `train-blocks-casing` | `above` runs | colour `[16, 18, 24, 200]`; width px = block width + 3; round caps; no cull, no depth write; true depth (no polygon offset) |
| `train-blocks` | `above` runs | colour free `[60, 200, 110, 230]` / taken `[235, 64, 52, 255]`; width 3 px free, 5 px taken; round caps; no cull, no depth write; true depth (buildings and road bridges in front hide it) |
| `train-blocks-tunnels` | `below` runs | same colour with alpha `round(alpha * 0.5)` (free 115, taken 128); same width; dashed `[2.5, 2]` (dash, gap in units of line width), `dashJustified: true`; depth compare always (seen through the ground) |
| `train-signals` | signals | billboarded circles radius 6 px; fill by aspect: stop `[235, 64, 52, 255]`, caution `[250, 196, 40, 255]`, clear `[60, 200, 110, 255]`; stroke `[16, 18, 24, 200]` 2 px; depth always; **pickable** |
| `train-signal-names` | signals with a name, only at `zoom >= SIGNAL_NAMES_ZOOM = 15.5` | text 10 px monospace, colour `[235, 238, 245, 255]`, anchor start, pixel offset `[10, 0]`, background `[16, 18, 24, 200]` with padding `[3, 1]` px; billboard; depth always |
| `train-block-labels` | one per taken block, at the middle vertex of its first line (`lines[0][floor(len/2)]`), text = the trains' names | text 11 px monospace white `[255,255,255,255]`, background `[150, 30, 24, 230]`, padding `[4, 2]`; billboard; depth always |

Block paths are 3D (`[lon, lat, metres]`), so they rise over rail bridges.

The speed map is hidden while blocks are on: `railSpeeds = isTauri() && !trainBlocks` (G:2074). In the native app `isTauri()` is always true, so: rail map shows speeds unless blocks are on, in which case the plain OpenFreeMap network is drawn.

---

## 7. Selected train's route (G:2078-2162, train-route.ts)

Backend: `train_route(id) -> TrainRoute { segments: [lon, lat][][] (stretches of track; the first starts at the train; gaps where track is unknown), stops: RouteStop[] { name, lat, lon, track: string|null, stops: bool }, complete: bool }`.

Constants: `ON_TRACK_M = 40`, `OFF_ROUTE_M = 300`, `SEARCH_M = 20_000` (train-route.ts:7-11). `TRAIN_ROUTE_DASH = [0, 0, 0, 255]`, `TRAIN_ROUTE_CASING = [236, 238, 244, 215]`, `TRAIN_ROUTE_DASHES = [2.2, 1.6]`, `TRAIN_TO_STOP = [37, 99, 235, 255]` (G:2078-2082).

`trainRouteLayers(route, train)`:
- `drawn` = the train's drawn `[lon, lat]` (from `trainData`), else reported.
- `ahead` = its frames later than now, as `[lon, lat]`.
- `way = routeAhead(route, drawn, routeCursor, ahead)`; `routeCursor = way.cursor`; if `!way.on` call `ontrainrouteoff()` (the page then asks the backend for the route again).
- `toStop = wayToStop(drawn, frames, now)`.

Layers (all with cull none, no depth write, depth compare always):

| Layer | Data | Style |
|---|---|---|
| `train-route-dashes` | `way.rest` (polylines) | black `[0,0,0,255]`, 3 px, dashed `[2.2, 1.6]` (units of width, measured along the whole path with high-precision dashes), `dashJustified: false` |
| `train-route-head-dashes` | `[way.head]` if non-empty | same |
| `train-route-to-stop` | `[toStop]` if >= 2 points | `[37, 99, 235, 255]`, 4 px, round caps and joints |
| `train-route-stop-spot` | last point of `toStop` | circle radius 5 px, fill white, stroke `[37, 99, 235, 255]` 2.5 px (not billboarded: lies on the ground) |
| `train-route-stops` | `route.stops` where `stops == true` | circle radius 4.5 px, fill `[236, 238, 244, 215]`, stroke black 2 px |

Note: despite the name, `TRAIN_ROUTE_CASING` is only used as the station fill; no casing line is drawn under the dashes.

`routeAhead(route, drawn, cursor, ahead)` (train-route.ts:89-121):
- Start the search from the cursor (if it belongs to this route object), else from segment 0, leg 0.
- `locate`: walk legs forward from the cursor up to 20 km; track the nearest point overall; within 40 m, track the best of the *first run* of consecutive on-track legs and return it as soon as the run ends (a line that later doubles back past the train doesn't capture it); else return the nearest.
- `nearestOn(p, a, b)`: flat projection onto the leg, `t` clamped to [0,1], distance in metres.
- If not on track and found at the very start (`seg 0, leg 0, t 0`): the train is short of the route's start: `head = [drawn, ...ahead points]` until an ahead point is within 40 m of the route's first point (then `head += [line[0], line[1]]`) or projects onto the first leg with `t > 0` within 40 m (then `head += [q, line[1]]`); `on = true`. If none does: off.
- If `d > 300` m, or not on track and at the very end of the segment (`t == 1` on the last leg): off (`head` and `rest` empty, `on = false`).
- Else `head = [drawn, found.q, line[found.leg + 1]]`, `rest = [line.slice(leg + 1), ...later segments]` (only those with >= 2 points; cached in the cursor while the train stays on the same leg), `on = d <= 40`.

`wayToStop(drawn, frames, nowMs)` (train-route.ts:128-138): `way = [drawn]`; for each frame pair `(a, b)` with `b.t > now`: if `a` and `b` are the same place (it stands there): return `[]` if `a.t <= now` (already standing), else `way`; otherwise push `[b.lon, b.lat]`. If it never stops within its frames: `[]`.

---

## 8. Rail map

Enabled by the `railMap` prop. `railLayers(...)` (rail-layer.ts:518-542) in drawing order:

1. `zoom < 8`: the shipped overview (8.3).
2. `zoom >= 8`: the speed map (8.2) when `speeds`, else the OpenFreeMap network (8.1) when the tile URL is known.
3. OpenRailwayMap raster from zoom 10 (8.5): **off by default and not enabled by Globe.svelte**.
4. `zoom >= 12`: stations (8.4) (default on).

Colours: `RAIL_MAIN = [240, 112, 96, 245]`, `RAIL_MINOR = [214, 128, 112, 225]`, `RAIL_SERVICE = [168, 102, 94, 200]`, `RAIL_TRANSIT = [178, 146, 240, 225]`. Tunnels: same RGB with alpha `round(alpha * 0.55)` and dashed `[3, 2.5]` (units of line width), `dashJustified: true`.

Width scale by zoom: `railWidthScale(zoom) = round((0.8 + clamp01((zoom - 8)/7)) * 10) / 10` (x0.8 at zoom 8, x1.8 at zoom 15 and beyond). All rail lines: widths in pixels, round caps, cull none, no depth write, true depth (no polygon offset).

### 8.1 Network from OpenFreeMap vector tiles (rail-layer.ts:50-167)

Source: the same OpenMapTiles-schema vector tiles as the base map (`tileUrl`), layer `transportation`; tile zooms `8..14` (over-zoomed past 14).

`railKind(properties)`: not rail unless `class` is `rail` or `transit`; `service` set (yard, siding, spur, crossover) -> `service`; `class == transit` -> `transit`; `subclass` undefined or `rail` -> `main`; else `minor`.

| kind | colour | base width px |
|---|---|---|
| main | RAIL_MAIN | 1.9 |
| minor | RAIL_MINOR | 1.3 |
| service | RAIL_SERVICE | 0.8 |
| transit | RAIL_TRANSIT | 1.2 |

`brunnel == "tunnel"`: faint + dashed. In the flat projection, bridges (`brunnel == "bridge"`) are split out and raised to their decks (9.1) and drawn with `RAISED_NO_CULL` (depth compare always) in a second sub-layer; the rest on the ground.

### 8.2 Speed map (rail-layer.ts:296-481)

Source: the backend's own OpenRailwayMap vector tiles (today via the `railtiles://localhost/{z}/{x}/{y}` custom protocol; `http://railtiles.localhost/...` on Windows/Android). Layer `railway_line_high`; tile zooms `7..12` (`SPEED_TILE_ZOOMS`), over-zoomed beyond 12 ("a train on the track is on the line drawn"). In the native port call the backend's tile function directly instead of a URL.

`speedKind(p)`: null if `p.state` is defined and not `"present"`; `feature` in {tram, subway, light_rail, monorail} -> `transit`; `feature` not in {rail, narrow_gauge} -> null; `service` set -> `service`; else `line`.

| kind | base width px | colour |
|---|---|---|
| line | 2.2 | by speed |
| service | 0.9 | by speed |
| transit | 1.1 | RAIL_TRANSIT |

Speed bands (`maxspeed` in km/h; first band with `kmh <= upTo`):

| up to | RGBA | legend label |
|---|---|---|
| 40 | 104, 128, 255, 240 | 40 |
| 80 | 72, 186, 232, 240 | 80 |
| 120 | 86, 206, 128, 240 | 120 |
| 160 | 228, 212, 84, 245 | 160 |
| 200 | 246, 146, 62, 245 | 200 |
| infinity | 242, 78, 104, 250 | 250+ |
| not a positive number | 138, 142, 156, 190 (`SPEED_UNKNOWN`) | ? |

`p.tunnel` truthy -> alpha x0.55 and dashed `[3, 2.5]`. Bridges (`p.bridge === true`) raised to `RAIL_DECK_M = 6` m in the flat projection (9.2), drawn depth-always.

Speed labels from `zoom >= SPEED_LABEL_MIN_ZOOM = 12.5` (`speedLabels`, rail-layer.ts:387-410), per tile: candidates = every (multi)line part of kind `line` with a positive numeric `maxspeed` and length `>= 0.06 * tileSize`; sorted longest first; label at the line's midpoint (by length); skipped if a label with the same text is already within `0.18 * tileSize`. Text = `round(kmh)`. Style: 10 px, weight 650, font "Inter, system-ui, sans-serif", text colour `[12, 14, 20, 255]`, background = the speed colour, padding `[3, 1]` px, billboard, depth compare always.

Legend (G:3790-3798, CSS 3830-3858): shown when `railMap && railSpeeds && shownZoom >= 8` (`shownZoom` only updates when `floor(zoom * 2)` changes). A row centred horizontally at the bottom (`bottom = sheet height + 26 px`), `gap 2 px`, padding `3px 6px`, small radius, background `oklch(0.15 0 0 / 0.8)`, font `600 10px/1` data font, not interactive. Items: a "km/h" unit label (tertiary text colour, 4 px right margin), then one chip per band with text `40, 80, 120, 160, 200, 250+` and a final `?` chip (tooltip "Limit not mapped"); each chip `min-width 26 px`, padding `3px 4px`, radius 2 px, background = the band's RGB (opaque), text colour `oklch(0.16 0.01 260)`, centred. Accessible label "Track speed limits, km/h".

Attribution strings: `RAIL_SPEED_ATTRIBUTION = "Rail: OpenRailwayMap, © OpenStreetMap contributors"`; `OPENRAILWAYMAP_ATTRIBUTION = "Rail overlay: © OpenStreetMap contributors, style CC-BY-SA 2.0 OpenRailwayMap"`.

### 8.3 Overview below zoom 8 (rail-overview.ts)

File `static/rail/railroads.bin` (Natural Earth 1:10m railroads simplified to ~1 km; built by `scripts/build-rail-overview.py`). Little-endian:
```
"RRO1"                                  4 bytes magic
u32 paths, u32 points
per path:  u8 category, u16 point_count        (3 bytes each, packed)
per point: i16 lon*100, i16 lat*100            (4 bytes each)
```
File length must equal `12 + paths*3 + points*4` and the point counts must sum to `points`. Positions are `value / 100` degrees.

Drawn as open polylines: category 1 -> `RAIL_MAIN`, 1.5 px; others -> `RAIL_MINOR`, 1 px; cull none, no depth write. Loaded lazily on first need; silently absent on failure.

### 8.4 Stations (rail-layer.ts:100-187)

From the base tiles' `poi` layer, tile zooms 12..14: features with `class == "railway"` and `subclass` in {station, halt}. Circles: radius 3.5 px (station) / 2.5 px (halt), fill `[214, 220, 232, 255]`, stroke `[8, 12, 20, 230]` 1.5 px; cull none, no depth write.

### 8.5 OpenRailwayMap raster (rail-layer.ts:189-294)

Not used by the app today (the option defaults to false and Globe.svelte never sets it). If ported: `https://tiles.openrailwaymap.org/standard/{z}/{x}/{y}.png` (512 px images for 256 px tiles), zoom 10..18, opacity 0.85, at most 4 concurrent requests, cache 200 tiles, 5 s timeout per request, and a `Backoff`: after an HTTP 429 or 4 consecutive failures, no requests for 15 minutes (a cancelled request is not a failure). Requires a unique User-Agent.

### 8.6 Ground rails and platforms on the realistic basemap (realistic-ground.ts:338-402, 865-924)

Only in the app (`OWN_RAILS = isTauri()`, i.e. always in the native port), only with the realistic basemap; pushed at G:2663.

- `ground-rails-*`: the backend rail tiles (layer `railway_line_high`, tile zooms 7..12) drawn as part of the realistic ground (widths in **metres**, minimum 0.6 px, rounded joins/caps, not pickable). Line style: not drawn in tunnels; `speedKind == transit` -> not drawn for trams, else `ROADS.transit` = width 2.2 m, paint id 35, edge 0.6 m ballast; other kinds -> `ROADS.rail` = width 2.6 m, paint id 35, edge 1 m ballast (realistic-ground.ts:324-325, 392-396). Their bridges are raised with `ormDeck` (realistic-ground.ts:446). While these are on, the base map's own `rail`/`transit` lines are not drawn (except trams) (realistic-ground.ts:363, 373).
- `ground-platforms` (flat only): layer `standard_railway_platforms`, served at tile zoom 15 only (`PLATFORM_ZOOM = 15`), loaded only within 4000 m of the camera (`PLATFORM_LOD = { levels: [15], maxDistance: 4000 }`). Polygons extruded to `PLATFORM_M = 0.6` m, paint `AREAS.platform = paint(9)` (same surface paint as "commercial"/"bridge"), no deck.gl lighting (sides shaded by the realistic ground's own shading by facing), winding CCW, drawn with the `solid` parameters (`NO_CULL`, depth write on: hides what is behind). A platform mapped as a line is drawn flat as `PATHS.pedestrian`.

The paint ids and the realistic ground shader belong to the realistic-ground document; only the wiring and constants are recorded here.

---

## 9. Bridges and tunnels (bridges.ts)

Constants: `EARTH_CIRCUMFERENCE = 40_075_016.686`, `RAMP_M = 40`, `PIECE_M = 6`, `RAIL_DECK_M = 6`, `DECK_M = 6` (per layer), `ALONG_DEG = 30`, `M_PER_DEGREE = 111_320`.

### 9.1 Deck heights

- `omtDeck(p)` (OpenMapTiles): 0 unless `brunnel == "bridge"`; `class` rail/transit -> 6 m; else `6 * clamp(layer, 1, 4)` m (`layer` numeric property, default 1).
- `ormDeck(p)` (OpenRailwayMap): `bridge === true` -> 6 m, else 0.

`rise(d, total, deck, on: [bool, bool])`: `ramp = min(40, total * 0.3)`; if `ramp <= 0` return `deck`; `k = clamp01(min(on[0] ? 1 : d/ramp, on[1] ? 1 : (total - d)/ramp))`; height = `deck * k^2 * (3 - 2k)` (smoothstep ramps up from each end, unless the bridge continues past that end).

### 9.2 Raising tile lines (`bridgeParts` for binary tiles, `raiseFeatures` for GeoJSON features)

For tile-local coordinates (0..1 across the tile) in the flat projection:
- `lat = atan(sinh(PI * (1 - 2*(y + 0.5)/2^z)))` (tile centre); `tileMetres = EARTH_CIRCUMFERENCE * cos(lat) / 2^z`; heights converted to deck.gl common units with `commonPerMetre = 512 / (EARTH_CIRCUMFERENCE * cos(lat))` (in the port: just use metres for z).
- Each path with `deck > 0`: cumulative length along it in metres; an end within 0.001 of a tile edge (`x <= 0.001 || x >= 0.999 || ...`) counts as "goes on" (no ramp at that end: the rest of the bridge is the next tile's); each segment is subdivided into `ceil(len / 6 m)` pieces and each vertex gets `z = rise(...)`.
- Output: `ground` (everything else, unchanged), `raised` (3D), and for binary tiles also `under` (the bridges' flat copies, for shadows). `null` when the tile has no bridges.

### 9.3 Level of a point on the track (`levelAt`, 255-280)

`Structure = { tunnel: bool, line: [lon, lat][], joined: [bool, bool] }` (from the backend: with `Train.structures` and with the blocks). `Level = { tunnel: bool, height: metres }`; ground = `{false, 0}`.

`levelAt(p, heading | null, structures, within = 4)`: among structures whose bbox (padded by `within` metres) contains `p`, for each segment: flat distance `d` from `p`; skip if `d > 4` m or not better than the best so far; if `heading` is given and the segment is longer than 1 m: `turn = |((heading - segmentCourse) mod 180) + 180| mod 180`, skip when `min(turn, 180 - turn) > 30` deg (it crosses rather than runs along); best match wins: tunnel -> `{true, 0}`; bridge -> `{false, rise(distanceAlong, totalLength, 6, joined)}`.

### 9.4 A line on the track (`onStructures`, 291-316)

Splits a `[lon, lat]` polyline into runs `{tunnel, path: [lon, lat, metres][]}`. No structures or fewer than 2 points -> one ground run. Per segment: `course = courseOf(a, b)`; if the segment's bbox is near any bridge's bbox (padded 10 m in lat, 20 m-equivalent in lon) it is cut into `ceil(metres / 6)` pieces, else one piece. Per piece: `tunnel = levelAt(midpoint, course).tunnel`; point heights = `levelAt(point, course).height`; consecutive pieces with the same `tunnel` flag extend the current run, else a new run starts at the piece's start.

---

## 10. Aircraft route, airport and place markers in this line range (G:1386-1586)

These belong to the aircraft document functionally, but their definitions sit in the assigned range; recorded for completeness.

### 10.1 Route layers (`routeLayers`, G:1419-1478)

Colours: `ROUTE_AHEAD = [55, 142, 240, 210]`, `ROUTE_BEFORE = [241, 238, 231, 110]`, `ROUTE_FILED = [241, 238, 231, 150]`. Dashes are measured along the whole path (high precision), in units of line width.

| Layer | Data | Style |
|---|---|---|
| `route-filed` | `r.filed` (lon/lat waypoints, on the ground), if > 1 point | 1.5 px, `ROUTE_FILED`, solid; cull none, no depth write |
| `route-waypoints` | `r.filed` points | circles radius 2.5 px, fill `ROUTE_FILED`; cull none, no depth write |
| `route-before` | `drawnRoute(r.before)`, if > 1 point | 1.5 px, `ROUTE_BEFORE`, dashed `[3, 3]`; cull none |
| `route-ahead` | `drawnAhead(r.ahead, pos)`, if > 1 point | 2 px, `ROUTE_AHEAD`, dashed `[4, 3]`; cull none; recomputed every tick |
| `route-airports-*` | origin and destination airports | airport markers (10.2) |

`drawnRoute(points, id, ex)`: each `[lon, lat, altFt]` -> `[lon, lat, elevation(motion.drawAltitude(id, alt, lat, lon), ex)]` (airport-relative height like the aircraft, then exaggerated).

`drawnAhead(ahead, pos, id, ex)`: path = `drawnRoute(trimToPosition(ahead, [pos.lon, pos.lat]))`; `dz = pos.z - path[0].z`; walking along the path, for points within `AHEAD_BLEND_M = 15_000` m: `z = max(0, z + dz * max(0, 1 - walked/15000))`; finally `path[0] = pos`. (The estimate eases from the aircraft's drawn height into its own profile over 15 km.)

### 10.2 Airport markers (`airportMarkers`, G:1480-1517)

- Dots: radius 5 px, fill `MARKER_FILL = [55, 142, 240, 255]`, stroke `MARKER_LINE = [6, 12, 19, 230]` 2 px, billboard, cull none, depth compare always.
- Codes: text = `iata || icao`, 13 px, weight 700, `LABEL_FONT`, colour `[241, 238, 231, 255]`, pixel offset `[0, -16]`, outline 3 (SDF) in `LABEL_OUTLINE`, billboard, `LABEL_PARAMETERS`.

### 10.3 Searched place marker (`placeMarker`, G:1520-1556)

- Dot: radius 4 px, fill `[241, 238, 231, 255]`, stroke `MARKER_LINE` 2 px, billboard, depth always.
- Name: 12 px, weight 600, colour `[241, 238, 231, 230]`, offset `[0, -14]`, outline 3 `LABEL_OUTLINE`, any characters (`characterSet: "auto"`).

### 10.4 Small helpers

- `mix(a, b, t)`: per-channel lerp of RGB (G:1558).
- `at(pose, ex) = [pose.lon, pose.lat, elevation(pose.altFt, ex)]` (G:1564).
- `orientationOf(pose, yawOffset) = [pose.bank, yawOffset - pose.heading, pose.pitch]` in deck.gl's `[pitch, yaw, roll]` order: with the model's nose on +y and right wing on +x, deck's "pitch" (about the model's y axis) is the aircraft's bank and deck's "roll" (about x) is the aircraft's pitch; both act in the model frame before the yaw (about z) (G:1566-1576).
- `upAxis([p, y, r])`: the mesh's up axis after that orientation = third column of deck.gl's rotation: `[sy*sr + cy*sp*cr, -cy*sr + sy*sp*cr, cp*cr]` with s/c of the angles in degrees (G:1583-1586).

---

## 11. Picking, hover, tooltips, selection

### 11.1 Pickable things in this area

| Layer | Object | Resolves to |
|---|---|---|
| `ship-icons` | ship index (into the ship list) | ship |
| `ship-mesh-<class>` | `ShipModel` with `index` | ship |
| `train-icons` | train index | train |
| `train-car-meshes-*` | `DrawnCar` with `train` index | train |
| `train-signals` | `BlockSignal` | signal tooltip only |

Not pickable: radar, rail map, blocks' lines, labels in this area, route lines, selection rings (`ship-selection-*` is explicitly excluded in `pickedShip`, G:3368).

Pick resolution uses the current drawn data (`shipData.list` / `trainData.list`), falling back to the props (G:3357-3376).

Cursor: `grabbing` while dragging, `pointer` while hovering anything pickable, else `grab` (G:3714).

### 11.2 Click (G:3726-3736)

Priority: aircraft -> `onselect(flight)`; else ship -> `onshipselect(ship)`; else train -> `ontrainselect(train)`; else airport label -> `onairport(iata)`; else, when nothing at all was hit, `onselect(null)` (clears the aircraft selection). Clicking a signal does nothing.

### 11.3 Hover highlight

- Pointer hover: meshes tint with `[55,142,240,110]` (auto-highlight per instance); ship icons tint with `[55,142,240,200]`. `train-icons` has no `autoHighlight`, so pointer hover does not tint train icons; only the sidebar-hover does.
- Sidebar hover: `hoveredShipMmsi` / `hoveredTrainId` props highlight the matching icon (and the matching ship mesh instance). Train **cars** are not highlighted from the sidebar.

### 11.4 Tooltips (G:3381-3422)

A small HTML tooltip near the pointer (class `aircraft-tooltip`: raised surface background, primary text, 1 px line border, small radius, padding `space-2` x `space-3`, small UI font with line height 1.45, shadow; `.tt-head` base size weight 650; `.tt-sub` secondary text colour; `.tt-warn` warning colour) (CSS G:3884-3902).

- **Ship**: head = `ship.name` (raw; may be the MMSI as text). Sub 1 = `shipKindLabel(kind)` + (dims ? ` · {bow + stern} m` : ""). Sub 2 = `MMSI {mmsi}` + (old ? ` · old position` : "").
- **Train**: head = `train.name`. Sub 1 = `[trainLabel(kind), operator]` joined by ` · `. Sub 2 (only if non-empty) = `["to {destination}", delayText(delayMinutes)]` joined by ` · `. Sub 3 = position age label (`trainPositionAge`), in the warning colour when stale.
- **Signal**: head = `Signal {name}` or `Main signal`, then ` · Stop | Caution | Clear`. Sub 1: stop -> `{trains} in the block after it`; caution -> `{trains joined by ", "} in the next block`; clear -> `No train in the block after it`. Sub 2: `From where the trains are, not the signal itself`.
- (Airport labels: head = name, sub `Open departures and arrivals`; aircraft use the HoverCard instead: other documents.)

### 11.5 Selection

Selection state lives in the page (`selectedShip`, `selectedTrain` props); the globe only draws the halo + ring (4.8, 5.7) and the selected train's route (7). Camera follow of a ship/train ("vehicle lock", G:3142-3183, 3708-3709) keeps the vehicle's drawn position at the view centre during gestures; it is outside this range but depends on `trainData.positions` / `shipData.positions`.

---

## 12. Sidebar list logic

### 12.1 Ships (ship-list.ts, ship-details.ts)

- `ShipFilters { hiddenKinds: number[] (type groups by shipKindKey), motion: "all"|"moving"|"moored", hiddenSizes: (0|1|2)[], hideOld: bool, sort: "size"|"name"|"distance" }`; defaults: nothing hidden, `motion "all"`, `hideOld false`, `sort "size"`. Stored filters are merged over defaults with validation (`mergeShipFilters`).
- `shipFilterCount` = number of active groups among (hidden kinds, motion != all, hidden sizes, hideOld).
- `SHIP_SIZES` labels: 0 "Small", 1 "Medium", 2 "Large".
- `SHIP_COVERAGE = "Ships in view from VesselFinder; zoom in to see more."`
- `shipName(s)` = name unless empty or equal to the MMSI as text (then "").
- `shipMatches(ship, query)`: empty -> true; all digits -> MMSI starts with the query or the folded name contains it; else every whitespace-separated folded word is contained in the folded `shipName`. `fold` = NFD, strip combining marks (`\p{M}`), lower case.
- `shipFilter(f)` (what the **map** draws too): excludes hidden type groups, hidden sizes, old positions when `hideOld`, `moving` requires a heading, `moored` requires no heading.
- `shipLength(s) = dims ? dims[0] + dims[1] : null`.
- `sortShips(list, sort, centre)`: `name` comparator: named before unnamed, then locale collation (numeric, base sensitivity), then MMSI. `distance`: by `cosAngle` to the view centre descending, then name. `name`: by name. `size` (default): size class descending; within it known lengths before unknown, longer first; ties by name.
- Card helpers: `SHIP_STALE_S = 900`; `positionAge` -> `"Position {age} ago"` / stale `"Position {age} old"`; `navTone(status)` = `"warn"` for AIS status codes 2, 6, 14, else `"neutral"`; `dimensions(length, beam)` -> `"366 × 48 m"`, `"366 m"`, `"48 m beam"` or null (rounded, en-GB thousands separators); `draught(current, max)` -> `"9.3 / 15.5 m"`, `"9.3 m"`, `"max 15.5 m"` or null (1 decimal); `latLon(lat, lon)` -> `"51.9213° N, 4.4812° E"` (4 decimals); `isLocode(dest)` = matches `^[A-Z]{2} ?[A-Z2-9]{3}$`.

### 12.2 Trains (train-list.ts)

- `TrainFilters { hiddenKinds: TrainKind[], hiddenOperators: string[] ("" = unknown operator), delayedOnly: bool, sort: "delay"|"speed"|"name"|"distance" }`; defaults: nothing hidden, `delayedOnly false`, `sort "delay"`. Hidden operators are also sent to the backend (`hide_train_operators`) so they are not streamed.
- `trainFilterCount` = active groups among (hidden kinds, hidden operators, delayedOnly).
- `TRAIN_COVERAGE = "Live trains are shown in Sweden, from Trafikverket."`; `DELAYED_MINUTES = 5`; `isDelayed` = finite `delayMinutes >= 5`.
- `trainMatches(train, query)`: every folded word contained in folded `"{name} {destination} {operator}"`.
- `trainFilter(f)`: not a hidden kind, not a hidden operator (`operator ?? ""`), and delayed if `delayedOnly`.
- `sortTrains`: `distance` by `cosAngle` descending; `delay` by `delayMinutes` descending, unknowns last; `speed` by `speedKmh` descending, unknowns last; `name`; ties by name (numeric collation) then id.
- `distanceKm(lat, lon, centre) = acos(clamp(cosAngle)) * 6371`; `distanceLabel(km)`: `< 1 km -> "{round(km*100)*10} m"`, else `"{round(km)} km"` with en-GB thousands separators.
- `trainOperators(trains)`: `{name, count}` sorted: unknown ("") last, then by count descending, then name.

---

## 13. Dev-only layers

### 13.1 Sightings (`sightingLayers`, G:2420-2518)

Only in dev builds (`import.meta.env.DEV`); gate behind a debug flag in the port. Uses `train.reports` (`Sighting { lat, lon, unixMs, speedKmh|null, latest: bool }`) and `train.predicted` frames. All layers depth-always, no depth write.

- `dev-predicted`: unfilled circles radius 7 px, stroke `[80, 220, 120, 255]` 2 px, at `positionAt({frames: predicted}, now)` for every sighted train with predicted frames.
- `dev-predicted-label`: for the selected train: text "predicted", 11 px monospace, colour `[80,220,120,255]`, offset `[0, 16]`, background `[12, 14, 20, 225]`, padding `[4, 2]`.
- `dev-sightings`: latest report of each train: circle radius 4 px, fill `[240, 60, 60, 255]`, stroke white 2 px.
- `dev-sighting-labels`: for the selected train's latest report: text `live {age} s · {speed} km/h | no speed{versus}`, colour `[240,60,60,255]`, offset `[0, -14]`, same background; `versus` = ` · drawn {±n} m` (how far the drawn train is ahead (+) or behind (−, U+2212) of the predicted real position along the heading) and, when a speed was reported, `, {±n} km/h` (drawn speed over the next second minus reported). Text refreshed once a second.

### 13.2 Demo trains (demo-trains.ts)

Dev builds without the backend (`#demo` in the address): six made-up trains through the view centre, one per shape. Per run `{heading, cars, kind, operator, offset m (perpendicular), speed m/s, shape}`:
```
70,  3,  regional,  "Skånetrafiken", 0,    14, regional
250, 6,  highSpeed, "SJ",            40,   22, highSpeed
160, 12, intercity, "Öresundståg",   -60,  0,  regional
340, 4,  regional,  "SJ",            90,   0,  doubleDeck
20,  8,  intercity, "Snälltåget",    -110, 0,  loco
110, 1,  regional,  "Inlandsbanan",  140,  0,  railcar
```
Start = centre moved `offset` m at `heading + 90`, then 40 m back; frames every 2 s for 120 s at `speed`; tail at 0, 100, 200, 300, 400 m behind; id `trafikverket:demo:{n}`, name `Demo {operator} {100+n}`, `speedKmh = speed*3.6`, `delayMinutes 0`. Useful as a visual test fixture for the car meshes in the port.

---

## 14. Porting notes, open questions, risks

### 14.1 Suggested native structure

- **Radar**: a `radar` module with `palette`, `contour` (blur, iso-rings, even-odd, grid cut, earcut), `mesh` (buildMesh/buildWalls/polygonWalls), `tiles` (RainViewer fetch + LRU cache + coverage) and a render pass drawing indexed, unlit, alpha-blended triangles with a per-draw `{opacity, z_base, z_scale}` uniform. Crates: `earcutr` (earcut), `image`/`png` for decoding, `reqwest` for tiles. The backend's `radar.rs` already contours the measured volumes; check whether its contouring/triangulation code can be reused for RainViewer tiles instead of porting `IsoRings` (the results must still clip exactly at tile edges so tiles join without seams, and must give wall lines that skip tile edges).
- **Ships/trains**: instanced icon pipeline (atlas texture, per-instance position, size px, angle, colour, flags: billboard vs ground-plane), instanced lit mesh pipeline (per-instance position, yaw, scale xyz, translation metres, colour; per-vertex position, normal, colour), circle/ring pipeline, path pipeline (width px, dash array, round caps, 3D paths), text pipeline with background boxes.
- **Picking**: an id buffer pass or CPU hit-testing (ships/trains: screen-space distance to projected icon centres; meshes: oriented-box test) is enough; picked objects only need to resolve to (kind, index).

### 14.2 Behaviours easy to miss

1. RainViewer tiles are blanked under measured coverage, and tiles are re-contoured only when the coverage over *that* tile changes (cache key includes the intersecting coverage keys).
2. Lifted radar must be drawn level-major across tiles (all tiles' level 0, then level 1, ...), not tile-major.
3. `lift` is quantised to 0.1 and height scale to 0.1; lifted layers disappear entirely at pitch <= ~11 deg.
4. Ships: models only for ships with a heading, at zoom >= 12.5, once 14 px long; the model/icon split is evaluated at quarter-zoom steps.
5. Ship model colour is white (mesh's own colours); the type shows via the hull topsides. Old positions fade (icon alpha 105, model alpha 120).
6. The ship selection ring uses the reported position and `max(bow, stern)` (not length); the train ring uses the drawn position.
7. Train icons lie on the ground (not billboarded) with a 180 deg yaw offset on the globe; ship icons are billboarded and use the projected screen angle.
8. Vehicles never move backwards onto a new report: they hold until their frames pass them; otherwise a 2 s smoothstep ease. No easing for jumps >= 0.05 deg lat or 0.1 deg lon.
9. Positions are never extrapolated past the last frame.
10. Trains in tunnels: icons at alpha 90; cars in separate depth-always layers at alpha 90.
11. Cars: model cars use their own lengths and no gap; plain cars 25.4 m + 0.6 m gap; minimum on-screen width 3 px (so far-out cars are fatter than true scale).
12. Blocks: answers that come back short while loading must not replace a fuller set; taken blocks recomputed every 500 ms with 2 m hysteresis.
13. `railSpeeds` depends on the blocks toggle.
14. Block and train layers read `viewBounds()` which is null on the globe: none of the close-in train features (cars, blocks) exist at zoom <= 12.

### 14.3 Open questions / risks

1. **`cutPolygonByGrid`** (`@math.gl/polygon`) is used both for clipping contours to the tile (grid 1/8) and for splitting wide polygons (4 deg). There is no drop-in Rust equivalent; it needs a port (it is a Sutherland-Hodgman-style recursive bisect that keeps holes by bridging) or a replacement (e.g. clip with `geo`/`i_overlay` boolean ops against grid cells). Its exact output affects only triangulation, not appearance, as long as pieces are clipped at the same lines.
2. **Exact marching-squares parity**: d3-contour semantics (>= threshold, half-cell offsets, linear smoothing) should be matched or neighbouring tiles could show hairline seams; the padding scheme (2 cells: repeat edge, then zero) is the part that makes tile edges join.
3. **Radar tile zoom selection**: the note in 3.2 about `round(zoom) + 1` clamped to 7 should be verified against deck.gl's TileLayer (`tileSize 256`, `zoomOffset 0`) or simply chosen to match the visual density of the current app.
4. **RainViewer palette inversion** relies on the "Universal Blue" scheme (`/2/`); if RainViewer changes its palette or API, `reflectivity()` breaks. The API response shape (`host`, `radar.past[].path`) is assumed stable.
5. **Mesh lighting**: `SHIP_MESH_MATERIAL` / `CAR_MESH_MATERIAL` are deck.gl Phong materials evaluated against the scene's light rig (ambient + sun/key + camera light), defined outside this range. The port needs that rig's exact colours/intensities (see the aircraft/lighting document) for the ships and cars to look the same.
6. **Train model normals** are pre-divided by the car's draw scale; the native mesh shader must transform normals by the instance scale (not its inverse) and renormalise to get the intended lighting, mirroring deck.gl's SimpleMeshLayer behaviour. Worth a visual check against the web app with the demo trains.
7. **Icon atlases** are drawn with Canvas 2D (anti-aliased strokes, bezier curves, round joins). tiny-skia reproduces these closely but not bit-identically; consider baking PNGs from the current app to guarantee identical glyphs.
8. **deck.gl IconLayer on the ground (`billboard: false`) with pixel sizes**: the icon is sized in pixels at its anchor but oriented in the ground plane; under perspective/tilt it foreshortens. The port must replicate "pixel size converted to world size at the icon's depth", not a constant world size.
9. **Dashes**: deck.gl's PathStyleExtension measures dashes in units of line width along the path; `dashJustified` stretches the pattern to fit the path length exactly; `highPrecisionDash` accumulates along the whole path. The path shader needs a cumulative-distance attribute in screen-independent units (deck uses common-space distance scaled by width); expect small differences in dash phase.
10. **Polygon offset**: deck.gl offsets each layer slightly toward the camera by layer index; several layers here opt out (`getPolygonOffset: null`). A native renderer with explicit draw order and depth-write-off ground layers probably doesn't need the offset at all, but the "hidden by buildings/road bridges" behaviour of rail lines and blocks must be kept (depth test on against buildings, off against the ground).
11. **Regex operator matching** uses JS `\b` and case-insensitivity on names with Swedish letters; confirm the Rust `regex` (Unicode `(?i)`) classification gives the same rows for real operator names (e.g. "SJ Götalandståg" must match row 0, "Vy" row 9).
12. **`hullPolygon`** (ships.ts) and the OpenRailwayMap raster layer appear unused by the current UI; confirm before spending time on them.
13. **Web-only fallbacks** (`isTauri()` false: no measured radar, no speed map, no ground rails/platforms) disappear in the native app; the native app always has the backend, so `railSpeeds = !trainBlocks` and measured radar polling is always on.
14. **Train `tail` orientation**: `carLines` assumes `tail[0]` equals the first frame's position and later entries run backward along the track; confirm with the backend (`Train.tail` doc says "back from the start of frames").
15. **Performance**: `drawnCars` and `levelAt` run every frame for all trains in view; `takenBlocks` is O(trains x blocks x segments) every 500 ms. Fine in Rust, but worth a spatial index if block counts grow.
