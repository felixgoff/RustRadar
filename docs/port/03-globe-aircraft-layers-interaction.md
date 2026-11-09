# 03 — Globe: layer assembly, aircraft drawing, labels, trails, picking, render loop

Scope: `src/lib/Globe.svelte` lines 2520–3903 (`buildLayers()` and everything after it), the
aircraft-related definitions earlier in the file that `buildLayers()` depends on (G:335-415,
754-969, 1235-1251, 1386-1586), and `src/lib/aircraft.ts`, `models.ts`, `liveries.ts`,
`aircraft-labels.ts`, `aircraft-readings.ts`, `hover-details.ts` (+ `HoverCard.svelte`),
`routes.ts`, `flightpath.ts`, `taxinet.ts`, `taxiways.ts`, `adsb.ts`, `freshness.ts`,
plus the bits of `shadow.ts`, `geo.ts`, `realistic.ts` (`skyCss`) they call.

Purpose: enough detail to re-implement aircraft rendering (icons + 3D meshes + gear + shadows +
liveries), aircraft labels, the selected flight's trail/route, selection/hover/picking, the frame
loop and the per-frame layer order in winit + wgpu + egui without re-reading the TypeScript.

Conventions (same as `01-globe-camera-basemap.md`):

- `G:123` = `src/lib/Globe.svelte` line 123. Other files are named in full.
- Colours are `[R, G, B, A]` 0–255, straight alpha, written as-is to a non-sRGB 8-bit target
  (see doc 01 "Conventions" and its Risks about `*Srgb` surfaces).
- "zoom" is deck.gl zoom (512 px world at zoom 0). `flat` = `zoom > 12` (Web Mercator
  projection); otherwise the globe projection. `mpp` = metres per CSS pixel at the view centre
  = `40_075_016.686 · cos(lat_centre) / (512 · 2^zoom)` (`geo.ts:60-61`).
- "px" are CSS pixels (deck.gl `sizeUnits: "pixels"`); multiply by the device pixel ratio in use
  (section 13) for framebuffer pixels.
- Camera, limits, controller, gestures, `lockedView`, base layers, place labels are owned by
  doc 01; they are summarised here only where the code lives in this line range.

---

## 1. Data shapes used in this document

### 1.1 `Pose` (`motion.ts:110-135`) — where and how an aircraft is drawn *now*

Produced by `motion.pose(flight, now)` once per visible aircraft per frame.

| Field | Unit | Meaning |
|---|---|---|
| `lon`, `lat` | degrees | drawn position (smoothed, a few seconds in the past) |
| `altFt` | feet | height to draw at: airport-relative near airports, exactly 0 on the ground |
| `heading` | deg clockwise from north | where the nose points (track, or measured true heading) |
| `track` | deg | direction of motion |
| `pitch` | deg, nose up + | |
| `bank` | deg, right wing down + | |
| `gear` | 0..1 | 0 retracted, 1 fully down, animated |
| `phase` | enum | `parked`, `taxi`, `takeoffRoll`, `initialClimb`, `climb`, `cruise`, … (`motion.ts:98`) |

Other `motion` calls used here:

- `motion.now()` = `Date.now() + clockOffset` (server-synced clock, `motion.ts:1030`). **All
  aircraft timing, the sun for aircraft shadows and the night layer use this clock**; ships,
  trains, radar refresh timers and camera transitions use the local `Date.now()`.
- `motion.drawnTime(id, now)` → ms: the time the aircraft is drawn at (2–12 s behind `now`).
- `motion.drawAltitude(id, altFt, lat, lon)` → feet: the same airport-relative correction for a
  trail/route point, so paths meet the model (`motion.ts:1172-1178`).
- `motion.groundField(id)` → the airport an aircraft is on the ground at (for taxiways).

### 1.2 World position of an aircraft

```
at(pose, ex) = [pose.lon, pose.lat, elevation(pose.altFt, ex)]            (G:1564)
elevation(altFt, ex) = max(0, altFt) · 0.3048 · ex            metres       (geo.ts:47-48)
ex = effectiveExaggeration(exaggeration, zoom)
   = 1 + (exaggeration − 1) · clamp((11 − zoom)/5, 0, 1)                   (geo.ts:54-57)
```

`exaggeration` is the component prop (altitude exaggeration setting); it fades to true scale
(1×) between zoom 6 and 11.

### 1.3 `LiveFlight` fields read in this range

`id` (number, the selection/hover/label key), `lat`, `lon`, `alt` (ft), `speed` (kt), `track`,
`onGround`, `icon` (Flightradar24 icon class, e.g. `B738`), `typecode` (ICAO type, e.g. `A20N`),
`callsign`, `flight`, `reg`, `origin`, `destination` (IATA codes), `timestampMs`.

### 1.4 Component state touched in this range

| Name | Init | Meaning |
|---|---|---|
| `view` | `{longitude: 10, latitude: 35, zoom: 1.9, bearing: 0, pitch: 0}` (G:314) | current camera, optional `position: [0,0,z]` (flat projection target height) |
| `tick` | 0 | incremented once per `renderFrame`; "per-frame data changed" marker |
| `lastTick`, `frame` | 0 | rAF timestamp of the last render; rAF handle |
| `transitionUntil` | 0 | `Date.now()` ms until which a programmatic camera transition runs |
| `raised` | false | camera target lifted to an aircraft's height (flat projection lock) |
| `hoveredId` | null | flight id under the pointer (for the highlight) |
| `hoverFlight`, `pointer`, `areaWidth/Height`, `hoverVersion` | | hover card state (section 10) |
| `fontsReady` | false | label font loaded; gates all text layers |
| `tileUrl` | undefined | OpenFreeMap vector tile URL template, from its TileJSON |
| `zoomAround` | `"pointer"` | current wheel-zoom anchor mode |
| `lockedMaxPitch` | 85 | pitch limit while locked on an aircraft |
| `wasLocked`, `wasTerrain`, `vehicleLocked` | false | edge detectors / train-ship follow state |
| `shownSky` | `""` | last sky CSS written |
| `rainShown` | `{0,0,0}` | quantised rain overlay inputs |
| `labelSet`, `labelPositions` | empty | aircraft label layout cache (section 7) |
| `mapError` | null | first render error; while set, nothing renders |

---

## 2. `buildLayers()` — preamble (G:2520-2636)

Called once per rendered frame (`renderFrame`, section 11). Steps, in order:

1. `updateLook()` (section 12) — also called again by deck's `onBeforeRender`.
2. `now = motion.now()`; destructure `view` → `cLon, cLat, zoom, bearing, pitch`.
3. `flat = zoom > 12`; `ex` as in 1.2; `mpp = metersPerPixel(zoom, cLat)`.
4. Size floor `minPx = minPxAt(zoom) = 8 + 16 · clamp((zoom − 3)/7, 0, 1)` px (G:337-339):
   8 px at zoom ≤ 3, 24 px at zoom ≥ 10.
5. **`screenPx(metres)`** — on-screen size (px, largest dimension) of an aircraft (G:2531-2537):

   ```
   floor = minPx · clamp((metres / 38)^0.8, 0.6, 2)        // 38 = MODEL_SIZE.narrow
   screenPx = max(floor, metres / mpp)                     // never smaller than true size, no ceiling
   ```

   Zoomed out everything sits at its floor: A380 (79.8 m) ≈ 1.81× an A320-class, a PA-28
   (10.6 m) hits the 0.6× clamp.
6. `depth = depthAt(zoom) = round(clamp((zoom − 6)/3, 0, 1) · 10) / 10` (G:343): 0 at zoom ≤ 6,
   1 at zoom ≥ 9, in steps of 0.1. Mesh z-scale is `max(0.04, depth)` (flat silhouettes when
   zoomed out).
7. `material = meshMaterial(depth)` (G:399-415), cached per depth:
   `ambient = 1 − 0.28·depth`, `diffuse = 0.42·depth`, `shininess = 48`,
   `specularColor = round([255, 236, 200] · depth)`. At depth 0 this is pure flat colour.
8. `yawOffset = flat ? 0 : 180` — deck.gl's globe-projection local frame for mesh instances is
   the flat one turned half a turn about "up". **Port note:** this is a deck.gl artefact; a
   native renderer that builds a true east/north/up frame at each aircraft uses yaw = −heading
   in both projections and drops `yawOffset` (and the matching sign flip in shadows, 5.4).
9. Cull radius: `radius = visibleRadius(zoom, pitch, w, h)` degrees
   `= min(100, 1.3 · (hypot(w, h)/2) · 360/(512·2^zoom) · (1 + 4·sin(pitch)))` (`geo.ts:92-98`);
   `minCos = cos(radius)`.
10. `liveryAmount = round(clamp((zoom − 6)/2, 0, 1) · 10) / 10` — airline colours fade in over
    zoom 6–8 in 0.1 steps.
11. `useIcons = pitch <= 5 && zoom < 11 && aircraftModels.loaded` (`FLAT_PITCH = 5`,
    `CLOSE_ZOOM = 11`).
12. `detail = zoom >= 11 ? 1 : 0` — LOD index for the published meshes.
13. `lockedOn = follow ? selected?.id : undefined`.
14. `taxiways = zoom >= 12` (`TAXIWAY_ZOOM`, `taxiways.ts:9`).
15. `labelling = aircraftLabels && fontsReady`; `labelAll = labelling && zoom >= 4`.

### 2.1 Per-flight loop (G:2584-2621)

Iterates `showAircraft ? flights : []` in feed order:

```
facing = cosAngle(f.lon, f.lat, cLon, cLat)            // reported position, not the pose
if f.id != lockedOn && facing <= minCos: skip           // off-screen / behind the globe
real = aircraftModels.forFlight(f.typecode, f.icon)     // published model or null (4.3)
pose = motion.pose(f, now)
if taxiways: taxiwaysFor(f.id, pose.phase)              // lazy-load airport taxi network (section 16)
position = at(pose, ex)

// label candidate
if labelling && (labelAll || f.id == selected?.id)
   && (zoom > 6 || facing > 0.42 || f.id == lockedOn)   // zoomed out: only well short of the horizon (~65°)
   && labelTextOf(f) != "":
     labelled.push(f); labelledAt.push(position); labelledSizes.push(real?.size ?? sizeOf(f.icon))

if useIcons && real:                                     // → icon lists
     iconIndex[f.id] = icons.len; icons.push(f); iconPoses.push(pose)
     iconPositions.push(position); iconNames.push(real.name); iconSizes.push(real.size)
     continue
// → mesh groups (also: anything without a published model even when useIcons)
[kind] = modelFor(f.icon)
groupId = real ? "t-" + real.name : "k-" + kind
group = groups[groupId] or new {real, kind, flights:[], poses:[], positions:[], liveries:[], index:{}}
group.index[f.id] = group.flights.len
group.flights.push(f); group.poses.push(pose); group.positions.push(position)
if liveryAmount > 0: group.liveries.push(liveries.forFlight(f))
```

Notes:

- The aircraft the camera is locked to is always drawn (never culled).
- The **same `position` array object** is pushed to `labelledAt` and `group.positions`; it is
  mutated in 2.2, so mesh-drawn aircraft's labels follow the raised position. Icon positions are
  not raised.
- With `useIcons` true, aircraft with no published model (balloons, ground vehicles, satellites,
  anything unmapped) are still drawn as built-in 3D meshes, with the usual depth for the zoom
  (4 % thick unlit silhouettes at zoom ≤ 6, full depth from zoom 9).
- Groups are keyed by published model name or built-in kind → one instanced draw per group (per
  livery part), in Map insertion order (first appearance in the feed).

### 2.2 Standing on the wheels (G:2623-2636)

For each group:

```
mesh   = real?.gear?.geometry ?? real?.lods[min(detail, lods.len−1)].combined ?? models()[kind].combined
wheels = −lowestZ(mesh)                 // lowestZ = min(0, min z over all vertices), cached per mesh (G:376-387)
divisor = real?.size ?? MODEL_SIZE[kind]
for each instance i:
   s = screenPx(real?.size ?? sizeOf(flights[i].icon)) · mpp / divisor
   positions[i][2] += wheels · s · max(0.04, depth)
```

Published models are centred on their bounding box, so their gear hangs below z = 0; built-in
models already have their lowest point at z ≈ 0 (raise ≈ 0).

---

## 3. `buildLayers()` — full layer order

Layers are pushed in this order; deck.gl draws them in array order (later = on top, subject to
depth testing where a layer tests depth). Everything is rebuilt as descriptors each frame and
matched by `id`. Layers owned by other port documents are listed with their id/condition only.

| # | Layer id(s) | Condition | Source | Notes |
|---|---|---|---|---|
| 1 | `earth` | always | G:2638, doc 01 §5.4 | sphere; exempt from terrain wrapping |
| 2 | `terrain-surface` | `terrain` | G:2643 | 3D ground; realistic adds `TERRAIN_AIR` extension; realistic also calls `aimMapEye(viewportOf(view))`; exempt from wrapping |
| 3a | `imagery` | `basemap == "satellite"` | G:2650, doc 01 §5.8 | |
| 3b | shaded relief (`terrainLayers`) | satellite && `terrain` | G:2651 | |
| 3c | realistic land (`realisticLandLayer`) | realistic | G:2654 | `GROUNDED` ext, `SURFACE` params |
| 3d | land-cover ground | realistic && flat | G:2656 | |
| — | (side effect) `wantRoadWidths(cLon, cLat, render)` | realistic && flat && zoom ≥ 14 | G:2658 | |
| 3e | city ground (`cityGroundLayer`, `GROUND_LOD`) | realistic && tileUrl && flat | G:2660 | |
| 3e' | ground (`groundLayer`) | realistic && tileUrl && !flat | G:2661 | |
| 3f | rail ground (`railGroundLayers`) | realistic | G:2663 | |
| 3g | shaded relief (`terrainLayers`, opacity `terrain ? 1.4 : 1`, `HAZE_FADE`) | realistic | G:2665 | always, even with `terrain` off |
| 3h | `clouds-globe-0` / `clouds-flat-{i}` | realistic && `clouds > 0` && cloud picture loaded | G:2666-2668, 3615-3638 | `clouds = 1 − smoothstep(5.5, 6.5, zoom)` as opacity; BitmapLayer at `CLOUD_HEIGHT_M`, params `{cull none, depthWrite false, depthCompare "less-equal"}`, not pickable; globe: one quad over `CLOUD_BOUNDS`; flat: `cloudStrips(picture, cLat)`; exempt from wrapping |
| 3i | `land` | dark | G:2670, doc 01 §5.5 | |
| 3j | shaded relief | dark && `terrain` | G:2671 | |
| 3k | `map-globe`/`map-flat` | dark && tileUrl | G:2674, doc 01 §5.6 | `trueAeroways = 45/mpp >= 4` |
| 3l | `aeroway-true-*` | dark && tileUrl && trueAeroways | G:2675, doc 01 §5.7 | |
| 4 | rail network (`railLayers`) | `railMap` | G:2679 | |
| 5a | radar tiles + lifted radar (`radarLayer`, `radarLifted(ex, lift)`) | `weather` && `radarUrl` | G:2680-2683 | `lift = liftFor(pitch)`; `refreshRadar()` if last check > 600 000 ms ago; if no `radarUrl` → `freeArenas()` |
| 5b | measured radar volumes (`measuredLayers`) | `weather`, per `measured` volume | G:2686-2689 | `refreshMeasured(viewArea(...))` when in the Tauri app and last check > 120 000 ms ago |
| — | `freeArenas()` | `!weather` | G:2690 | |
| 6 | `night` | `daylight && !realistic` | G:2692, doc 01 §5.10 | uses `now` (feed clock) |
| 7 | city buildings (`cityBuildingLayer`) | `buildings && flat && tileUrl` | G:2694 | sun: `realisticShadowSun(now, cLat, cLon)` or `shadowSun(now)` |
| 8 | trees (`treeLayers`) | realistic && tileUrl && flat && zoom ≥ 14 | G:2695-2720 | radius `min(sgl ? 2400 : 4800, max(900, 1.6·distance))`, `distance = 1.5·h·mpp`; shadow when `sunStrength() > 0.05` with alpha `round(min(0.6, 0.55·sunStrength())·255)`; `sgl = realistic && flat && zoom >= SGL_FROM_ZOOM` |
| — | (side effect) `aimKeyLight(localSun(sunDirection(now), cLat, cLon), buildings && flat ? sunlight(up)·clamp(zoom−12,0,1) : 0)` | always | G:2723-2724 | steers the scene's directional light (doc 01 §9.1) |
| 9 | `ship-selection-halo` | `selectedShip` and visible (11.4) | G:2726-2727 | |
| 10 | ship layers (`shipLayers(ships, flat, mpp, Date.now())`) | always (may be empty) | G:2728 | |
| 11 | `ship-selection-ring` | as 9 | G:2729 | |
| 12 | signal blocks (`blockLayers()`) | internal conditions | G:2735 | |
| 13 | train route (`trainRouteLayers`) | `trainRoute && selectedTrain` | G:2737 | |
| 14 | `train-selection-halo` | `selectedTrain` and visible | G:2738 | |
| 15 | train layers (`trainLayers(trains, Date.now())`) | always | G:2739 | computed before 12 so its data is fresh |
| 16 | sightings (`sightingLayers`) | dev builds only | G:2741 | |
| 17 | `train-selection-ring` | as 14 | G:2742 | |
| 18 | `label-tiles` + `labels` (place names / airport codes) | `tileUrl && labels && fontsReady` | G:2743-2754, doc 01 §5.9 | labels of the focused airport and (with a selection + route) the route's origin/destination IATA codes are removed (`withoutMarked`) because markers 22/24 draw them |
| 19 | `trail-shadow` | selected && showAircraft (section 8) | G:2775-2783 | |
| 20 | `trail` | same | G:2784-2795 | |
| 21 | `route-filed`, `route-waypoints`, `route-before`, `route-ahead` | same && `route` (each when it has > 1 point) | G:1419-1474 | |
| 22 | `route-airports-dots`, `route-airports-codes` | same && route has origin or destination | G:1475-1476 | |
| 23 | `focus-airport-dots`, `focus-airport-codes` | `airport` prop set | G:2799 | |
| 24 | `place-dot`, `place-name` | `place` prop set | G:2800 | |
| 25 | `shadow-{groupId}-{detail}` per mesh group | `shadowFade > 0` (zoom > 9) and ≥ 1 casting instance | G:2805-2868 | all shadows before any aircraft |
| 26 | per mesh group, in group order: `gear-{groupId}` then `aircraft-{groupId}-{detail}` (or `…-{part}` × parts) | group exists; gear: section 5.3 | G:2870-2955 | |
| 27 | `aircraft-icon-outlines` | `icons.length` && both atlases loaded | G:2974-2995 | |
| 28 | `aircraft-icons` | `icons.length` && icon atlas loaded | G:2997-3024 | |
| 29 | `aircraft-label-names`, `aircraft-label-heights` | `labelling` && label set non-empty; heights when zoom > 7 | G:3028-3030 | else the label set is dropped (G:3031-3035) |
| 30 | `selection-ring` | selected && showAircraft && `cosAngle(pose, centre) > 0` | G:3038-3055 | |

Finally: `return terrain ? onTerrain(layers) : layers` (section 9).

"With the aircraft off (`showAircraft == false`), nothing of the selected one either: no trail,
route or ring" (G:2756): `selectedPose = selected && showAircraft ? motion.pose(selected, now) : null`.
Airport/place markers (23, 24) do not depend on `showAircraft`.

### 3.1 Shared GPU parameters appearing in this range

| Name | Value |
|---|---|
| deck default (unless overridden) | depth test `less-equal`, depth write on, alpha blending (src-alpha, one-minus-src-alpha), GlobeView default `cullMode: "back"` |
| `NO_CULL` | `{cullMode: "none"}` |
| `LABEL_PARAMETERS` | `{cullMode: "none", depthCompare: "always", depthWriteEnabled: false}` (G:724) |
| `SHADOW_PARAMETERS` | no cull, depth write off, depth `less-equal`, stencil: write mask `0xff`, compare `equal` (reference 0), pass op `increment-clamp`, fail and depth-fail `keep` (G:366-375). The stencil buffer is requested at device creation (`stencil: true`) and cleared each frame; every shadow (aircraft, building, tree) marks the pixels it darkens so no pixel is darkened twice. |
| `CLOUD_PARAMETERS` | no cull, depth write off, depth `less-equal` |

---

## 4. Aircraft model assets

### 4.1 `static/aircraft/index.json` (`aircraft.ts:14-60`)

```jsonc
{
  "models": { "<name>": PackedModel, ... },         // 40 models
  "types":  { "<ICAO type designator, upper case>": "<model name>", ... },   // 138 entries
  "icons":  { "<Flightradar24 icon class>": "<model name>", ... },           // 24 entries
  "lods":   [220, 2600],                            // triangle budgets, informational
  "icon":   { "cell": 128, "columns": 7, "names": ["a318", ...40 names...],
              "outline": { "cell": 152, "pad": 12 } }
}
PackedModel = { size, span, length,                 // metres; size = max(span, length)
                lods: [PackedLod, PackedLod],       // coarsest first; always 2
                gear?: PackedGear }                 // absent for "ask21" and "heli"
PackedLod  = { positions, normals, indices,         // byte offsets into meshes.bin
               vertices, triangles,                 // counts
               parts: { top|belly|wings|tail|engines: [firstTriangle, triangleCount] } }
PackedGear = { positions, normals, indices, vertices, triangles, attachZ }   // attachZ metres, negative
```

Facts about the shipped file (22 536 bytes): model names `a318 a319 a320 a321 a332 a333 a343 a346
a359 a380 an225 ask21 atr42 b736 b737 b738 b739 b744 b748 b752 b753 b762 b763 b764 b772 b773
b788 b789 bae146 beluga citation crj700 crj900 cs100 cs300 e170 e190 heli pa28 q400`; sizes
range 10.55 m (`pa28`) to 87.92 m (`an225`); parts actually present are `top`, `belly`, `wings`,
`tail` (no `engines` part in any published model); max 1 579 vertices per LOD (fits `u16`
indices). Example: `a318` = size 33.91, LOD0 95 vertices / 215 triangles, LOD1 1 092 / 2 695,
gear 204 / 162, `attachZ −3.064`.

`icons` map (FR24 icon class → model): `A320→a320, B738→b738, B736→b736, B757→b752, B777→b772,
B747→b744, A330→a332, A340→a343, A343→a343, A380→a380, A3ST→beluga, A225→an225, MD11→b763,
Q300→q400, LJ60→citation, C206→pa28, C303→pa28, EC→heli, F100→bae146, FOKKER100→bae146,
RJ85→bae146, AS20→ask21, ASW20→ask21, GLID→ask21`.

### 4.2 `static/aircraft/meshes.bin` (1 703 272 bytes; `aircraft.ts:92-121`, `scripts/aircraft/build.mjs:707-726`)

Headerless concatenation of mesh records; all offsets come from `index.json`. Little-endian.
Each mesh (a LOD or a gear) is three sections, each starting 4-byte aligned:

| Section | Offset field | Element type | Count | Notes |
|---|---|---|---|---|
| positions | `positions` | `f32` × 3 | `vertices` | metres; nose +y, right wing +x, up +z; centred on the bounding box |
| normals | `normals` | `i8` × 3 | `vertices` | unit normal × 127 (clamped ≥ −127); decode `n = byte / 127`; padded to 4 bytes |
| indices | `indices` | `u16` × 3 | `triangles` | triangle list into this mesh's vertices; padded to 4 bytes |

(`normals == positions + 12·vertices`; `indices == normals + pad4(3·vertices)`.)

A LOD's index buffer is ordered by livery part: part `p` occupies indices
`[3·first, 3·(first+count))`. The "combined" mesh is the whole index buffer; a part mesh shares
the same vertex buffers with a sub-range of the indices (`aircraft.ts:104-115`). In wgpu: one
vertex buffer + one index buffer per LOD and draw `first_index/ index_count` ranges per part.

Gear mesh: nose and main gear extended, in the same frame; lies entirely below the plane
`z = attachZ` (struts hang from it).

### 4.3 Model lookup (`aircraft.ts:183-186`)

```
forFlight(typecode, icon) = models[ types[typecode.toUpperCase()] ?? icons[icon] ] ?? null
```

Returns `{name, size, lods: [ModelLod; 2], gear: ModelGear | null}`. Loading
(`aircraftModels.load()`, called on mount) fetches index + bin in parallel; on failure it only
logs a warning and `loaded` stays false, so the built-in procedural models are used for
everything and icons are never used. `LOD_COUNT = 2`.

### 4.4 Icon atlases (`aircraft.ts:163-178`, G:1235-1251, `build.mjs:593-669`)

- `static/aircraft/icons.ktx2`: KTX2, 896 × 768 px (7 columns × 6 rows of 128 px cells), 10 mip
  levels, Basis Universal payload (vkFormat 0, supercompression scheme 2). White RGB with the
  silhouette in alpha ("mask" icons: the instance colour tints them).
- `static/aircraft/icons-outline.ktx2`: 1 064 × 912 px (7 × 6 cells of 152 px = 128 + 2·12 pad),
  11 mip levels. The silhouette grown by 9 cell-pixels (anti-aliased over the last pixel).
- Cell of model index `i` (order of `icon.names`): `x = (i mod columns)·cell`,
  `y = floor(i / columns)·cell`, `width = height = cell`.
- Inside a cell the silhouette is top-down, **nose up (north)**, centred, scaled so the
  model's `size` spans 94 % of the cell (`build.mjs:595`), 3×3 supersampled coverage.
- `outlineScale = outline.cell / cell = 152/128 = 1.1875`.
- Textures: not sRGB, 2D (not array), sampler clamp-to-edge both axes, linear mag/min/mip
  (G:1241-1242). The web build transcodes to BC7 / ASTC 4×4 / RGBA8 depending on GPU features
  (`ktx2.ts`); the port can do the same with a Basis transcoder crate or re-bake the atlases to
  plain PNG/RGBA at build time (they are single-channel masks).
- While an atlas is loading (or failed) `atlas(url)` returns null and the layer that needs it is
  not pushed (aircraft simply don't show as icons until it arrives; a `render()` is triggered
  on load).

### 4.5 Built-in procedural models (`models.ts`)

Fallback for types with no published model and for everything until the assets load. Units
metres; nose +y, right wing +x, up +z; lowest point at z ≈ 0. Flat-shaded (per-triangle
normals, unindexed, 3 vertices per triangle), triangles wound so the normal faces away from an
"inside" point. Each triangle belongs to a livery part; `build()` emits a `combined` mesh
(parts concatenated in `PARTS` order `top, belly, wings, tail, engines`) and one mesh per
present part.

Builder primitives (`models.ts:19-159`):

- `tri(a,b,c,inside)`: skip if degenerate (|cross| < 1e-9); flip winding if normal faces
  `inside`.
- `quad(a,b,c,d)` = `tri(a,b,c)` + `tri(a,c,d)`.
- `loft(sections[{y,w,h,z,x?}], sides, belly=false)`: elliptical rings
  `[x + w·cos t, y, z + h·sin t]`, `t = 2π·i/sides`; quads between consecutive rings; with
  `belly`, quads whose mid-angle has `sin < −0.3` go to part `belly`; blunt ends (w ≥ 1e-6)
  are capped with a fan.
- `lathe(profile[[r,z]], sides, at)`: body of revolution about the vertical axis.
- `slab(outline[[u,v]], place(u,v,side))`: convex plate, two faces (side = ±1) fan-triangulated
  plus edge quads. `box(min,max)` = slab. `mirrored` = slab on both sides of x = 0.

Models (`models.ts:426-439`): the exact section tables are in `models.ts:161-409` (copy the
numbers verbatim when porting; they are short):

| Kind | Generator | Parameters | `MODEL_SIZE` (m) |
|---|---|---|---|
| `narrow` | `airliner` | length 38, radius 2, span 35, 2 engines | 38 |
| `wide2` | `airliner` | length 64, radius 3.1, span 61, 2 engines | 64 |
| `wide4` | `airliner` | length 70, radius 3.3, span 64, 4 engines | 70 |
| `turboprop` | `propPlane` | length 25, span 27, twin | 27 |
| `twin` | `propPlane` | length 9.5, span 12, twin | 12 |
| `light` | `propPlane` | length 8.6, span 11, single prop | 11 |
| `glider` | `propPlane` | length 6.8, span 15, no prop | 15 |
| `heli` | `helicopter` | fixed | 11 |
| `balloon` | `balloon` | fixed (envelope is part `tail`, basket `engines`) | 13.2 |
| `ground` | `groundVehicle` | two boxes | 6 |

`airliner` (`models.ts:162-249`): fuselage loft of 8 sections with 12 sides and belly split,
centreline height `cz = R + 0.6`; swept wings (part `wings`, sweep factor 0.5, tip chord 32 %
of root, thickness 0.12 R at `cz − 0.55R`); engines (part `engines`, radius `0.48R`, 10-sided
4-section lofts at span stations `[0.34]` or `[0.30, 0.56]`); tailplane (`wings`); fin (part
`tail`, height `0.19 L`).

FR24 icon class → `[kind, factor]` (`models.ts:456-492`; unknown → `["narrow", 1]`):

```
B738 narrow 1      A320 narrow 1      B736 narrow 0.85   B757 narrow 1.25   RJ85 narrow 0.75
FOKKER100 narrow 0.8   LJ60 narrow 0.45   FGTR narrow 0.45   CONC narrow 1.5
B767 wide2 0.85    A330 wide2 0.95    B777 wide2 1.05    MD11 wide2 0.9     A3ST wide2 0.85
A343 wide4 0.95    A346 wide4 1.05    B747 wide4 1       A380 wide4 1.1     A225 wide4 1.2
SAT wide4 0.5      ISS wide4 1.4      Q300 turboprop 1   C303 twin 1        C206 light 1
SLEI light 1       ASW20 glider 1     SI2 glider 1.6     EC heli 1          DRON heli 0.25
BALL balloon 1     GRND ground 1
```

`sizeOf(icon) = MODEL_SIZE[kind] · factor` — the real-world size (m) used when there is no
published model (`models.ts:509-512`).

---

## 5. Drawing aircraft as 3D meshes

### 5.1 Instance transform (G:2877-2893, 1566-1576; deck `mesh-layers/utils/matrix.ts`)

Per instance, model-space vertex `p` (metres) → local offset (metres, in the instance's local
frame at `position`), then added to the world position:

```
orientation = [bank, yawOffset − heading, pitch]      // deck's [pitch°, yaw°, roll°]
s      = screenPx(real?.size ?? sizeOf(f.icon)) · mpp / divisor       // divisor = real?.size ?? MODEL_SIZE[kind]
scale  = [s, s, s · max(0.04, depth)]

M = R(orientation) · diag(scale)                       // scale in model axes, then rotate
R columns (P = orientation[0], Y = orientation[1], Ro = orientation[2]; s*/c* = sin/cos):
  col0 = [ cY·cP,              sY·cP,             −sP    ]
  col1 = [ −sY·cRo + cY·sP·sRo, cY·cRo + sY·sP·sRo, cP·sRo ]
  col2 = [ sY·sRo + cY·sP·cRo, −cY·sRo + sY·sP·cRo, cP·cRo ]
offset_local = M · p  (+ translation, gear only)
```

i.e. `R = Rz(yaw) · Ry(bank) · Rx(pitch)`: pitch about the model's x (wing) axis (nose up
positive), then bank about the model's y (nose) axis (right wing down positive), then yaw about
up. With yaw = −heading the nose (+y = north) turns clockwise to the heading. In the flat
projection the local frame is x east, y north, z up, in metres at the instance. In deck's globe
projection the frame is the same turned 180° about up, hence `yawOffset = 180` (see 2 step 8).

For published models `divisor == size`, so `s = max(floorPx · mpp / size, 1)`: exactly true
scale (1.0) once the aircraft is larger on screen than its floor. For built-ins
`s = max(floor·mpp, MODEL_SIZE·factor) / MODEL_SIZE`.

Note `mpp` is taken at the *view centre* latitude for every aircraft.

Normals: `R·diag(scale)·n` (deck does not use the inverse-transpose), normalised in the shader.

### 5.2 Mesh layers per group (G:2870-2955)

For each group (`shape = real LOD min(detail, lods.len−1)` or the built-in model):

- Common: instanced over `group.flights`; position `group.positions[i]` (raised, 2.2);
  material from 2 step 7; `pickable: true`; `parameters: NO_CULL` (depth test + write on);
  highlight: instance `group.index[hoveredId]` gets `highlightColor [55,142,240,150]`
  (section 10.2).
- `liveryAmount == 0` (zoom ≤ 6): **one** layer `aircraft-{groupId}-{detail}` with the
  `combined` mesh; colour `SELECTED_COLOR [55,142,240]` for the selected flight, else
  `AIRCRAFT_COLOR [255,255,255]` (alpha 255).
- `liveryAmount > 0`: one layer per part `aircraft-{groupId}-{detail}-{part}` with that part's
  mesh; colour `mix(AIRCRAFT_COLOR, group.liveries[i][part], liveryAmount)` (linear per
  channel, not rounded). **The selected aircraft is not tinted blue in this mode** — selection
  shows through the ring, the label colour and the trail only.

Lighting (deck `LightingEffect`, G:417-427; Phong material): ambient light white intensity 1;
camera light colour `[255,248,235]` intensity 0.55 (a directional light along the view
direction); key light `[255,244,225]` intensity 0.7, direction `[-1,-2,-3]` normalised (steered
toward the real sun over buildings by `aimKeyLight`). Material per 2 step 7. deck's Phong:
`color = surfaceColor·ambient·ambientLight + Σ_lights (lambert·diffuse·surfaceColor + specular·specularColor)·lightColor`,
with `specular = pow(max(dot(N, H), 0), shininess)`; colours are 0..1 (light colour ×
intensity / 255). At depth 0 the result is exactly the surface colour.

### 5.3 Landing gear (G:2894-2931)

Only when the group has a published model with gear and `depth >= 0.8` (`GEAR_DEPTH`, about
zoom ≥ 8.4). Instances = indices `i` with `poses[i].gear > 0`. Layer `gear-{groupId}` (pushed
*before* the group's airframe layers), mesh `gear.geometry`:

```
s        = screenPx(size) · mpp / size                 // same as the airframe
flatten  = max(0.04, depth)
position = group.positions[i];  orientation = orientationOf(pose, yawOffset)
scale    = [s, s, s · flatten · pose.gear]             // squashed up toward the airframe
lift     = gear.attachZ · (1 − pose.gear) · s · flatten
translation = upAxis(orientation) · lift               // keeps the attach plane fixed while retracting
upAxis([P, Y, Ro]) = [ sY·sRo + cY·sP·cRo, −cY·sRo + sY·sP·cRo, cP·cRo ]   // = R col2 (G:1583-1586)
colour = GEAR_COLOR [178, 182, 190]; same material; NO_CULL; not pickable
```

(Scaling z by `g = pose.gear` about the plane `z = attachZ`: `z' = attachZ + (z − attachZ)·g
= z·g + attachZ·(1 − g)`; the second term is the translation along the rotated up axis.)

### 5.4 Shadows on the ground (G:2805-2868, `shadow.ts`)

`shadowFade = clamp((zoom − 9)/(11 − 9), 0, 1)` (`SHADOW_ZOOM = [9, 11]`). When > 0, for each
group (before any airframe layer):

```
sun = sunDirection(now)                                 // unit vector from Earth's centre toward the sun
for each instance i:
   [e, n, up] = localSun(sun, pose.lat, pose.lon)       // toward the sun as east, north, up; up = sin(sun altitude)
   a = shadowOpacity(pose.altFt, metres ?? sizeOf(icon), up) · shadowFade
   if a < 0.05 (SHADOW_MIN_ALPHA): not cast
   toSun_i = flat ? [e, n, up] : [−e, −n, up]           // deck globe-frame flip, see 2 step 8
   alpha_i = a
```

`localSun` (`shadow.ts:26-41`): `e = −sinLon·x + cosLon·y`;
`n = −sinLat·cosLon·x − sinLat·sinLon·y + cosLat·z`; `up = cosLat·cosLon·x + cosLat·sinLon·y + sinLat·z`.
`sunDirection(t)`: from `subsolarPoint(t)` (`daynight.ts`): `[cosLat·cosLon, cosLat·sinLon, sinLat]`.

`shadowOpacity(altFt, sizeM, sinSun)` (`shadow.ts:102-146`):

```
light  = sinSun > 0 ? min(1, asin(min(1, sinSun))° / 6) : 0        // LOW_SUN_DEG = 6
if light == 0: 0
slant  = max(0, altFt) · 0.3048 / sinSun
crisp  = 0.1 · sizeM / (0.53° in radians)                           // ≈ 10.81 · sizeM
contrast = slant <= crisp ? 1 : (crisp / slant)^1.5
alpha  = 0.5 · contrast · light ;  < 0.05 → 0                       // SHADOW_MAX_ALPHA = 0.5
```

Layer `shadow-{groupId}-{detail}`: instanced over the casting indices; mesh `shape.combined`;
instance position `[lon, lat, 0]` (ground); `material: false` (unlit); not pickable;
`SHADOW_PARAMETERS` (3.1); colour `[4, 8, 16, round(alpha_i · 255)]` (`SHADOW_COLOR`).
Instance matrix = `shadowMatrix(orientation, scale, toSun_i, height = group.positions[i][2])`
with the aircraft's own `orientation` and `scale` (5.1) (`shadow.ts:55-100`):

```
m = R(orientation) · diag(scale)                       // m[row][col] as in 5.1
kx = toSun.x / toSun.z ;  ky = toSun.y / toSun.z
// column-major 4×4 `out`:
col0 = [ m00 − kx·m20, m10 − ky·m20, 0, 0 ]
col1 = [ m01 − kx·m21, m11 − ky·m21, 0, 0 ]
col2 = [ m02 − kx·m22, m12 − ky·m22, 0, 0 ]
col3 = [ −kx·height,   −ky·height,   0, 1 ]            // groundZ = 0
```

i.e. each vertex of the posed mesh at `(0,0,height) + m·p` slides along the sun's rays to
z = 0: an oblique projection. The shadow lands `height / tan(sun altitude)` away from the
point below the aircraft, on the side away from the sun. `height` is the *drawn* height
(exaggerated metres, including the wheel raise), while the opacity uses the true `altFt`.

With `terrain` on, shadow layers get the terrain extension like the rest (the shadow's anchor
is raised by the ground height).

---

## 6. Drawing aircraft as icons (G:2957-3025)

Used for flights with a published model when `useIcons` (pitch ≤ 5°, zoom < 11, assets
loaded). Icons only show below zoom 11, where the projection is always the globe.

Heading on screen (`screen.ts`, doc 01 §3.14): a `GlobeViewport` is built from the current view
state and container size (not asked of deck); `step = 20 px · mpp` metres
(`ICON_HEADING_STEP_PX = 20`);

```
angle_i = screenAngle(viewport, iconPositions[i], pose.heading, step) ?? (bearing − pose.heading)
```

= the anticlockwise screen rotation (degrees) that turns a north-up icon so it points from the
aircraft's projected position toward the projection of a point `step` metres ahead along its
heading. (Away from the screen centre on a globe this differs from `bearing − heading`.)

Two billboard quad layers, both instanced over `icons`, position `iconPositions[i]` (at
altitude, not wheel-raised), `sizeUnits: "pixels"`, `billboard: true` (screen-facing, constant
pixel size), `NO_CULL`, default depth test/write, deck `alphaCutoff` 0.05 (fragments with
alpha < 0.05 discarded):

| Layer | Atlas / cell | Size (px, quad height = width) | Colour | Pickable |
|---|---|---|---|---|
| `aircraft-icon-outlines` (first, underneath) | outline atlas, `iconNames[i]` | `screenPx(iconSizes[i]) · 1.06 · 1.1875` | `OUTLINE_COLOR [8, 12, 20, 215]` | no |
| `aircraft-icons` | icon atlas, `iconNames[i]` | `screenPx(iconSizes[i]) · 1.06` | selected: `[55,142,240]`, else `[255,255,255]` | yes; highlight `iconIndex[hoveredId]`, `[55,142,240,150]` |

`1.06 ≈ 1/0.94` undoes the silhouette's 94 % fill so the silhouette itself spans `screenPx`.
Fragment colour = instance RGB, alpha = instance alpha × atlas alpha (mask mode). The outline
layer requires *both* atlases; the icon layer only its own.

Because both layers write depth at the same position, the outline (drawn first) and icon
(drawn second, `less-equal`) stack correctly; neighbouring icons at different heights occlude
by depth, not by draw order.

---

## 7. Aircraft labels (G:754-969, `aircraft-labels.ts`)

### 7.1 Text (`aircraft-labels.ts`)

- `aircraftLabel(f) = f.callsign.trim() || f.flight.trim() || f.reg.trim()` (cached per flight
  object in a WeakMap, G:806-811). Empty → the aircraft gets no label.
- `altitudeLabel(altFt, onGround)`: `"GND"` on the ground; else `ft = max(0, round(altFt))`;
  `ft >= 18 000` → `"FL" + zero-pad-3(round(ft/100))` (e.g. `FL350`); else
  `ft.toLocaleString() + " ft"` (locale thousands separator, e.g. `4,500 ft`).
- `labelPriority(sizeM, altFt, onGround)`:
  `size = clamp(sizeM/80, 0, 1)`; `low = onGround ? 1 : 1 − clamp(altFt/45 000, 0, 1)`;
  `round(600·size + 300·low) − 450` → range −450..450; larger and lower aircraft win.
- `SELECTED_LABEL_PRIORITY = 1000` for the selected aircraft.

### 7.2 Which aircraft get a label

From the loop in 2.1: label setting on and font loaded; zoom ≥ 4 for everyone
(`AIRCRAFT_LABEL_MIN_ZOOM`), below that only the selected aircraft; at zoom ≤ 6 only aircraft
with `facing > 0.42` (or the locked one); non-empty text. The height line appears when
`zoom > 7` (`AIRCRAFT_LABEL_HEIGHT_ZOOM`).

### 7.3 Label set cache (G:813-871)

The text layout is kept while `(feed array identity, withHeight, same set of ids)` is
unchanged; then only positions move each frame. Otherwise a new set (version + 1) is built
with per-aircraft `texts`, `heights` (`altitudeLabel(f.alt, f.onGround)` from the *reported*
altitude, or `""`), `priorities`, `sizes` and `largest` size. When labelling goes off the set
is emptied (G:3031-3035). In a native port this is just "re-shape glyph runs only when the
feed or membership changes".

### 7.4 Layout and style (G:878-969)

Per label, anchored at the aircraft's projected drawn position (wheel-raised for meshes),
billboard text, font "Overpass Variable" weight 600 rendered as SDF (atlas font size 64,
buffer 6), outline width 3 (SDF units) colour `LABEL_OUTLINE [6, 12, 19, 220]`,
`LABEL_PARAMETERS` (no depth test at all, no depth write), text anchor **start** (left edge at
the anchor + offset), baseline **center**. `scale = labelScale` prop (user label size).

```
gap(i)  = screenPx(set.sizes[i]) / 2 + 5          px, to the right        (AIRCRAFT_LABEL_GAP_PX = 5)
line    = withHeight ? 6 · scale : 0              px                      (AIRCRAFT_LABEL_LINE_PX = 6)
name line:    pixel offset [gap(i), −line], size 11 · scale px            (AIRCRAFT_LABEL_SIZE)
height line:  pixel offset [gap(i), +line], size 10 · scale px            (AIRCRAFT_LABEL_HEIGHT_SIZE)
```

(Screen y grows downward: the callsign sits above, the height below; with no height line the
callsign is vertically centred on the aircraft.)

Colours:

| | normal | selected |
|---|---|---|
| callsign | `[241, 238, 231, 235]` | `[55, 142, 240, 255]` |
| height | `[162, 172, 183, 220]` | `[55, 142, 240, 205]` |

Layer ids `aircraft-label-names`, `aircraft-label-heights`; not pickable.

### 7.5 Collision / decluttering

deck's `CollisionFilterExtension`, group `"aircraft-labels"`: all labels' *collision footprints*
are rendered to an off-screen map in priority order; a label is shown in proportion to how much
of a small patch around its anchor it "owns". Both lines share data (same picking colour) so
they show and hide together. Modifications:

- Footprint: the name layer has a transparent background rectangle
  (`getBackgroundColor [0,0,0,0]`) with padding `[left, top, right, bottom]`:

  ```
  half    = 11 · scale · 0.5
  spread  = 1 + 0.6 · clamp((7 − zoom)/3, 0, 1)        // labels keep further apart zoomed out
  padY    = (half + 2·line + 4·scale) · spread
  reserve = 60 · scale · spread                         // AIRCRAFT_LABEL_RESERVE_PX = 60
  padding = [ gap(largest) + reserve, padY, 4 + reserve − 60·scale, padY ]
  ```

  so each label claims a box reaching about one callsign-width to the *left* of its aircraft
  too, and its own height above and below.
- Collision test pass draws at `sizeScale = scale`.
- Priority: selected → 1000, else `set.priorities[i]`.
- "Crisp" fade: alpha `*= smoothstep(0.3, 0.65, collision_fade)` instead of the linear fade
  (G:788-803).

Port guidance: a CPU greedy declutter gives the same result: sort labels by priority
descending; for each, compute its claimed screen rectangle (text box extended by `padding`);
keep it if it intersects no kept rectangle; fade alpha in/out over a few frames (e.g. 150 ms)
to mimic the ease. Exact parity with deck's GPU occupancy test is not required.

---

## 8. Selected flight: trail, route, markers, ring

All gated on `selected && showAircraft` (`selectedPose` non-null), G:2757-2798.

### 8.1 Trail (G:2758-2796)

```
pos    = at(selectedPose, ex)                           // the drawn aircraft position (not wheel-raised)
drawnS = motion.drawnTime(selected.id, now) / 1000
trail  = (details?.trail ?? []).filter(p => (p.latitude || p.longitude) && !(p.timestamp > drawnS))
path   = trail.map(p => [p.longitude, p.latitude,
                         elevation(motion.drawAltitude(selected.id, p.altitude, p.latitude, p.longitude), ex)])
path.push(pos)
colors = trail.map(p => [...altitudeColor(p.altitude), 255]);  colors.push([...altitudeColor(selected.alt), 255])
```

`TrailPoint = {timestamp (s), latitude, longitude, altitude (ft), ground_speed, track,
vertical_speed, source}`. Points newer than the drawn time are dropped so the trail never
runs past the (delayed) aircraft; points at exactly (0, 0) are dropped.

`altitudeColor(ft)` (`geo.ts:22-44`): piecewise-linear, channels rounded, clamped at the ends:
0 `[255,98,0]`; 4 000 `[255,196,0]`; 8 000 `[160,230,40]`; 15 000 `[40,220,130]`;
25 000 `[0,200,230]`; 35 000 `[70,125,255]`; 45 000 `[205,85,255]`.

| Layer | Geometry | Style | Parameters |
|---|---|---|---|
| `trail-shadow` | the path with every z = 0 | colour `[241, 238, 231, 60]`, width 1.5 px, butt caps/miter joints (deck defaults) | no cull, depth write off (depth-tested) |
| `trail` | the 3D path | per-vertex colours (interpolated along segments), width 3 px, rounded joints and caps | no cull; depth test + write on |

Paths are screen-space-width polylines (`widthUnits: "pixels"`).

### 8.2 Route (G:1386-1478; data from `routes.ts`, section 15)

`Route = {origin?, destination?, kind, filed: LonLat[], before: PathPoint[], ahead: PathPoint[],
departure, arrival}`; `PathPoint = [lon, lat, altFt MSL]`.

| Layer | Condition | Geometry | Style |
|---|---|---|---|
| `route-filed` | `filed.length > 1` | filed waypoints, z = 0 (2D points) | `ROUTE_FILED [241,238,231,150]`, 1.5 px, solid; no cull, depth write off |
| `route-waypoints` | same | a dot per filed waypoint | radius 2.5 px, fill `[241,238,231,150]`; no cull, depth write off (not billboarded: lies in the ground plane) |
| `route-before` | `before.length > 1` | `drawnRoute(before)` | `ROUTE_BEFORE [241,238,231,110]`, 1.5 px, dash `[3, 3]`; no cull |
| `route-ahead` | `ahead.length > 1` | `drawnAhead(ahead, pos)` | `ROUTE_AHEAD [55,142,240,210]`, 2 px, dash `[4, 3]`; no cull; rebuilt every frame |
| `route-airports-*` | origin or destination known | `airportMarkers` (8.3) | |

Dashes: `[dash, gap]` in units of the line width (deck `PathStyleExtension`), i.e. 4.5 px on /
4.5 px off for `before`, 8 px on / 6 px off for `ahead`, measured **along the whole path**
(`highPrecisionDash`), not restarted per segment.

```
drawnRoute(points, id, ex) = points.map(([lon, lat, alt]) =>
        [lon, lat, elevation(motion.drawAltitude(id, alt, lat, lon), ex)])

drawnAhead(ahead, pos, id, ex):                         // AHEAD_BLEND_M = 15 000
   path = drawnRoute(trimToPosition(ahead, [pos.lon, pos.lat]), id, ex)     // 15.3
   dz = pos.z − path[0].z ; walked = 0
   for i = 1.. while walked < 15 000:
       walked += distance(path[i−1], path[i]) in metres
       path[i].z = max(0, path[i].z + dz · max(0, 1 − walked/15 000))
   path[0] = pos
```

so the estimate starts exactly at the drawn aircraft and eases from its drawn height into its
own profile over 15 km.

### 8.3 Airport and place markers (G:1480-1556)

`airportMarkers(id, airports)` — used as `route-airports` (route ends) and `focus-airport`
(the `airport` prop):

- `{id}-dots`: circle at `[lon, lat]` (z 0), radius 5 px, fill `MARKER_FILL [55,142,240,255]`,
  stroke `MARKER_LINE [6,12,19,230]` 2 px, billboard, `{no cull, depthCompare: "always"}`
  (draws through everything, including the globe).
- `{id}-codes`: text `iata || icao`, size 13 px, colour `[241,238,231,255]`, pixel offset
  `[0, −16]` (centred above the dot), font weight 700, SDF outline 3 `[6,12,19,220]`,
  billboard, `LABEL_PARAMETERS`. (Not scaled by `labelScale`.)

`placeMarker(place)` (`place` prop `{lon, lat, name}`):

- `place-dot`: radius 4 px, fill `[241,238,231,255]`, stroke `[6,12,19,230]` 2 px, billboard,
  depth always.
- `place-name`: the name, 12 px, `[241,238,231,230]`, offset `[0, −14]`, weight 600, outline as
  above, `LABEL_PARAMETERS`.

Neither is culled when on the far side of the globe (depth "always", no CPU check) — existing
behaviour.

### 8.4 Selection ring (G:3037-3055)

Only when the selected aircraft is on the near hemisphere
(`cosAngle(pose.lon, pose.lat, cLon, cLat) > 0`). Layer `selection-ring`: one billboard circle
at `at(selectedPose, ex)`, radius `screenPx(sizeOf(selected.icon)) · 0.6 + 4` px, not filled,
stroke `[55, 142, 240, 230]` 1.5 px, `{no cull, depthCompare: "always"}` — drawn last, over
neighbouring aircraft. Note it uses `sizeOf(icon)` (the built-in class size), **not** the
published model's size, and the un-raised position.

Ship/train selection (G:3291-3354), for reference: halo (filled `[55,142,240,56]`) under and
ring (stroke `[55,142,240,235]`, 2 px) over the vehicle, billboard, no depth test/write;
hidden when `zoom <= 12` and the vehicle is on the far hemisphere. Ship radius
`max(shipIconSize/2 + 5, hullPx + 6)` with `hullPx = max(dims[0], dims[1]) / mpp`; train radius
`round(trainIconSize/2 + 5)` at its currently drawn position.

---

## 9. `onTerrain` wrapping (G:3059-3067)

When the `terrain` prop is on, every layer whose id does **not** match
`/^(earth$|terrain-surface$|clouds-)/` is cloned with deck's `TerrainExtension` appended to its
extensions. Effect (deck `terrain-extension.js`): a layer is drawn in one of two modes —

- **offset**: the layer has per-instance anchor positions (`instancePositions`: mesh, icon,
  scatterplot, text layers) or `extruded` — each object is raised by the terrain height under
  its anchor;
- **drape**: everything else (polygon fills, bitmaps, vector map lines, and in WebGL the
  `PathLayer`, whose attribute is `vertexPositions`) is rendered into a texture that covers the
  terrain surface.

So aircraft, gear, shadows, icons, labels, markers and rings are lifted by the ground height
below them; flat map content is painted onto the relief. See Risks: by this rule the trail and
route `PathLayer`s are draped (flattened onto the ground) while terrain is on.

When terrain is switched off, `renderFrame` clears the leftover `terrainDrawMode` state from
all deck layers (G:3224-3229) — a deck workaround with no native equivalent.

Related: with terrain on in the flat projection and not following an aircraft, the camera
target height eases to the ground height each frame: `z += (ground − z) · 0.3` when
`|ground − z| > 0.5` m and no transition is running (`GROUND_EASE`, G:3204-3212; doc 01 §3.11).

---

## 10. Picking, hover, selection, tooltip

### 10.1 What is pickable

deck colour-picking with `pickingRadius: 6` px (the nearest pickable object within 6 px of the
pointer; topmost drawn wins at a pixel). Pickable layers:

| Layer id pattern | Object | Handled as |
|---|---|---|
| `aircraft-{groupId}-{detail}[-{part}]` | `LiveFlight` | aircraft |
| `aircraft-icons` | `LiveFlight` | aircraft |
| `labels` (place/airport labels, doc 01) | label feature | airport tooltip/click when `layerName == "aerodrome_label"` |
| `ship-*` (not `ship-selection*`) | index, `Ship`, or `{index}` | ship |
| `train-icons` (index) / `train-car-*` (`DrawnCar.train` index) | | train |
| `train-signals` | `BlockSignal` | tooltip only |

Not pickable: gear, shadows, icon outlines, aircraft labels, trail/route, markers, rings. The
code tests `layer.id.startsWith("aircraft-")`; the non-pickable `aircraft-icon-outlines` /
`aircraft-label-*` layers never reach it.

Port: an id buffer (R32Uint) rendered for pickable layers and read back around the pointer, or
CPU picking: project each visible aircraft's anchor and test distance against
`max(screenPx/2, 6)` px, preferring the nearest to the camera. Either satisfies the behaviour.

### 10.2 Hover (G:3715-3725, 3424-3474)

```
onHover(info):
   flight = info.layer?.id startsWith "aircraft-" ? info.object : null
   setHover(flight, info.x, info.y)
   if (flight?.id ?? null) != hoveredId: hoveredId = id; render()     // immediate re-render for the highlight
onDragStart: setHover(null)                                             // no card while dragging

setHover(flight, x, y):
   if flight: pointer = {x, y}                         // card follows the pointer
   if flight?.id == hoverFlight?.id: return
   hoverLoader.hover(flight && !ownDetails(flight.id) ? flight.id : null)
   hoverFlight = flight
ownDetails(id) = (id == selected?.id && details) ? details : null      // selected flight's details are already streaming
```

Highlight: the hovered instance in every aircraft layer is blended with `[55,142,240,150]`:
`a = 150/255`; `outA = a + colour.a·(1 − a)`; `rgb = mix(colour.rgb, highlight.rgb, a/outA)`
(for opaque aircraft: 59 % blue over the lit colour).

Cursor: `grabbing` while dragging, `pointer` while over any pickable object, else `grab`.

Hover card data (`hoverCard`, G:3451-3474), recomputed when the hovered flight, the loader
(`hoverVersion`) or the reference data change:

```
hovered = flights.find(id == hoverFlight.id) ?? hoverFlight       // latest report under a resting pointer
own     = ownDetails(hovered.id)
info    = own ? hoverInfo(own) : hoverLoader.info(hovered.id)
status  = own ? "ready" : hoverLoader.status(hovered.id)
origin, destination = reference.airport(hovered.origin / .destination)
progress = info?.progressPct != null ? {value: pct/100, measured: true}
         : routeProgress(hovered, origin, destination) != null ? {value, measured: false} : null
card = {flight: hovered, from: origin?.city || hovered.origin, to: destination?.city || hovered.destination,
        progress, info, status}
```

### 10.3 `hover-details.ts`

- `HoverInfo = {images: AircraftImage[], logoId: number, progressPct: number | null}`;
  `hoverInfo(details)`: `images = details.aircraft?.images ?? []`;
  `logoId = schedule.painted_as_id || schedule.operated_by_id || 0`;
  `progressPct = progress.progress_pct` only if `traversed_distance > 0 || remaining_distance > 0`.
- `HoverLoader` (debounced, serialised, cached fetch of `flight_details`):
  - defaults: `dwellMs 300`, `capacity 100` (LRU), `ttlMs 600 000`, `failureTtlMs 120 000`,
    `cooldownMs 60 000`.
  - `hover(id | null)`: on change, cancel the dwell timer; if id is set, not cached-fresh and
    not cooling down, start a 300 ms timer; when it fires and the pointer is still on that
    flight → `request(id)`.
  - `request`: only one request in flight; a second becomes the single `pending` one (replacing
    any earlier). On success store `hoverInfo`; on error: if `isRateLimited(message)` start a
    60 s cooldown (nothing stored), else store a failure (`null`). Then, if the pending flight
    is still hovered, not cached and not cooling down, request it; always call `onchange`.
  - `info(id)`: fresh cached info or null. `status(id)`: cached → `ready` / `unavailable`
    (failure entry); in flight, pending or dwell timer running → `loading`; else cooling down
    → `unavailable`, otherwise `loading`.
  - Cache entries expire after `ttlMs` (successes) or `failureTtlMs` (failures); reading
    refreshes recency; over capacity evicts least recently used.
  - `dispose()` on teardown.
- `routeProgress(position, origin, destination)`: null if either end unknown or the ends are
  < 1e-4 rad apart; else `flown / (flown + remaining)` with great-circle angles.

### 10.4 Hover card UI (`HoverCard.svelte`) — to rebuild in egui

Non-interactive (pointer-events none), width 272 px, positioned at `pointer + 16 px` right and
below; flipped to the left/above when it would cross `area − 8 px`; never closer than 8 px to
the top/left edge:

```
left = x + 16; if left + 272 > width − 8: left = x − 16 − 272; left = max(8, left)
top  = y + 16; if top + cardHeight > height − 8: top = y − 16 − cardHeight; top = max(8, top)
```

Content top to bottom:

1. Photo slot (16:9, radius 4 px, 6 px gap below, background `--surface-raised #3e3e3e`):
   open while `status == "loading"` or (`ready` and a photo URL that hasn't failed); closes
   with a 180 ms ease when no photo is coming. Image `images[0].thumbnail || .medium`,
   cover-fit, fades in over 160 ms; skeleton until loaded; credit `© {copyright}` bottom-left
   (11 px, text-1, dark text shadow) once loaded.
2. Body (surface panel, padding 8/12/12 px, 2 px row gap):
   - head row (min height 26 px, gap 8 px): callsign (`flight.callsign || flight.flight ||
     "No callsign"`, data font 14 px weight 700, ellipsis); type code chip (11 px, weight 600,
     1 px border `--line-control #6e6e6e`, radius 4, padding 0 5 px, line height 16 px, colour
     text-2); registration (11 px, text-3); airline logo right-aligned (if `logoId`, on a
     light chip: background `--text-1 #e3e3e3`, height 22 px, padding 2×5 px, radius 4; image
     max 18 × 72 px; hidden if it fails to load).
   - route line (13 px, text-2, ellipsis): `"{from} to {to}"`, `"From {from}"`, `"To {to}"`,
     or omitted.
   - progress bar if `progress`: track 2 px high, radius 1, `--line #444444`, margin 8/4 px;
     fill `--amber #378ef0` for measured, `--text-3 #a8a8a8` for an estimate; width
     `clamp(value, 0, 1)`, animated 400 ms ease-out (`cubic-bezier(0.16, 1, 0.3, 1)`).
   - numbers (12 px, text-2, data font): `"On ground"` or `"{alt.toLocaleString()} ft"`,
     then ` · {speed} kt`.

### 10.5 Click (G:3726-3736)

Precedence on a click (not a drag):

1. aircraft layer with an object → `onselect(flight)`;
2. else a ship → `onshipselect(ship)`;
3. else a train → `ontrainselect(train)`;
4. else the `labels` layer: if the feature is an `aerodrome_label` with an `iata` →
   `onairport(iata)`; any other label: nothing;
5. else if nothing pickable was hit (`!info.layer`) → `onselect(null)` (deselect).

A click on a pickable-but-unhandled object (e.g. a signal) does nothing. `pickedShip` /
`pickedTrain` (G:3356-3376) map layer objects back to the ship/train lists.

### 10.6 deck tooltip (G:3378-3422, styles G:3884-3902)

Ships, trains, signals and airport labels get a small HTML tooltip at the pointer (aircraft get
the hover card instead). It is hidden when the camera moves. Style (`.aircraft-tooltip`):
background `--surface-raised #3e3e3e`, text `--text-1 #e3e3e3`, 1 px border `--line #444444`,
radius 4 px, padding 8 × 12 px, font 12 px / 1.45 UI face; `.tt-head` 14 px weight 650;
`.tt-sub` colour `--text-2 #c4c4c4`; `.tt-warn` colour `--warn`. Top-left corner at the pointer
position.

| Hit | Head | Sub lines |
|---|---|---|
| ship | `ship.name` | `shipKindLabel(kind)` + (dims: ` · {dims[0]+dims[1]} m`); `MMSI {mmsi}` + (` · old position` if `ship.old`) |
| train | `train.name` | `trainLabel(kind) · operator`; optional `to {destination} · {delayText}`; position age label (`.tt-warn` if stale) |
| `train-signals` | `Signal {name}` or `Main signal`, then ` · Stop|Caution|Clear` | Stop: `{train} in the block after it`; Caution: `{trains, …} in the next block`; Clear: `No train in the block after it`; then `From where the trains are, not the signal itself` |
| `labels`, aerodrome | airport name (`nameOf(p)`) | `Open departures and arrivals` |

Text is HTML-escaped (`& < > "`).

---

## 11. Render loop and frame (G:3131-3243)

### 11.1 `loop(t)` — pacing

Runs every display frame (`requestAnimationFrame`), renders when at least `interval` ms have
passed since the last render:

```
locked   = ((follow && selected) || vehicleLocked) && Date.now() > transitionUntil
interval = locked && zoom > 7 ? 0          // every frame: camera and aircraft must move together
         : zoom > 7 ? 50                   // 20 fps
         : zoom > 4 ? 150                  // ~6.7 fps
         : 500                             // 2 fps
```

Independently of the loop, deck.gl itself redraws every animation frame while the camera
moves or a transition runs (using the last layer set), and `render()` is called immediately:

- when any of these props change (G:3771-3777): `flights, ships, selected, details, route,
  airport, place, exaggeration, basemap, labels, labelScale, buildings, sky, showAircraft,
  aircraftLabels, daylight, weather, follow, selectedShip, terrain, railMap, trains,
  selectedTrain, trainRoute, trainBlocks, hoveredTrainId, hoveredShipMmsi`;
- when the hovered aircraft changes; on `jumpTo`; on our own gesture moves (`moveView`);
- when async resources land: label font, aircraft models, train models, tile URL, icon
  atlases, tree/cloud/terrain data callbacks, realistic textures (`deck.redraw`).

Port: a winit `ControlFlow::WaitUntil`-style scheduler — redraw at the interval above for
animation, plus on-demand redraws for input, camera transitions and data arrival. The camera
itself must stay at display rate while moving.

### 11.2 `render()` and errors

`render()` does nothing while `mapError` is set; otherwise runs `renderFrame()` and on an
exception calls `reportMapError(e)` (first error only; shows the error card with Retry, doc 01
§9.5).

### 11.3 `renderFrame()` in order

1. `tick++`.
2. **Follow train/ship**: if `vehicleLocked` and no transition is running and `vehicleAt()` is
   known → set `view.longitude/latitude` to it, emit `onviewchange(view)`, push the view to the
   camera. (`vehicleAt()` returns null unless `followVehicle` and not following an aircraft;
   it reads the currently drawn train/ship position.) `vehicleLocked` is set by an effect when
   `followVehicle` turns on: fly to the vehicle with `zoom = max(view.zoom, 13)` over 800 ms
   (G:3160-3169).
3. **Zoom anchor**: `wanted = (follow && selected) || vehicleLocked ? "center" : "pointer"`;
   when it changes, swap the controller config (`controllerFor(locked)`: locked → zoom about
   the centre, drag rotates, no two-finger pan; free → zoom about the pointer, drag pans,
   two-finger drag pans).
4. **Aircraft lock**: if `follow && selected` and no transition is running →
   `view = lockedView(view, selected)`, emit `onviewchange`, `raised = Boolean(view.position)`,
   push the view with `limits()`. Else if `raised` and no longer following → `raised = false`,
   `view.position = undefined`, push the view with `position: [0,0,0]` and the plain `LIMITS`.
5. **Terrain aim** (when not following an aircraft): 9 above.
6. **Lock released beyond free limits**: if `wasLocked && !locked` and (`zoom > 20` or
   `pitch > 60`) → `goTo({zoom: min(zoom, 20), pitch: min(pitch, 60)}, 600 ms, ["zoom","pitch"])`.
   `wasLocked = locked`.
7. `deck.setProps({layers: buildLayers()})`.
8. Terrain-off cleanup (9).

### 11.4 `lockedView(v, flight)` (G:3089-3129; full discussion in doc 01 §3.9)

```
pose = motion.pose(flight, motion.now());  altitude = elevation(pose.altFt, effectiveExaggeration(exaggeration, v.zoom))
if v.zoom > 12:                                        // flat projection: raise the camera target
    lockedMaxPitch = 85
    return {...v, longitude: pose.lon, latitude: pose.lat, position: [0, 0, altitude]}
camera = 1.5 · containerHeight · metersPerPixel(v.zoom, pose.lat)             // CAMERA_HEIGHTS = 1.5
lockedMaxPitch = min(85, acos(min(1, altitude / (0.9 · camera))) in degrees)   // LOCK_CAMERA_ABOVE = 0.9
tilt   = min(v.pitch, lockedMaxPitch)
height = min(altitude, 0.9 · camera · cos(tilt))
lift   = height · tan(tilt)
latitude  = clamp(pose.lat + lift·cos(bearing) / 111 320, −85, 85)
longitude = pose.lon + lift·sin(bearing) / (111 320 · max(0.05, cos(pose.lat)))
return {...v, longitude, latitude, pitch: tilt, position: undefined}
```

Also applied inside `onViewStateChange` and `moveView` so a gesture while locked is re-aimed
in the same frame.

### 11.5 `goTo(target, duration, props)` (G:3245-3256)

`transitionUntil = Date.now() + duration`; the camera interpolates **linearly** (no easing)
the listed properties from the current view to `target` over `duration` ms, within `limits()`.
While `Date.now() <= transitionUntil`, following/terrain-aim/trackpad-pan do not touch the view.

---

## 12. `updateLook()` and the sky background (G:3594-3682, `realistic.ts:183-207`)

Called at the top of `buildLayers()` and again from deck's `onBeforeRender` (before every
frame deck draws, including camera-only redraws), so light and sky keep up with the camera.

```
if basemap != "realistic":
    clear the container background if one was set ("" → the page's own background shows)
    showRain(0, 0, 0); return
sun = sceneSun(daylight, localSun(sunDirection(Date.now()), latitude, longitude))
      // daylight on: the real sun as [east, north, up] at the view centre (local clock);
      // off: fixed azimuth 215°, altitude 35°: [sin(az)·cos(alt), cos(az)·cos(alt), sin(alt)]
cameraAltitude = 1.5 · h · mpp(zoom, latitude) · cos(pitch)                    // metres; ∞ without a container
weatherHere = mixConditions(sky ?? PRESETS.sunny, PRESETS.sunny,
                            smoothstep(0.3 · CLOUD_HEIGHT_M, CLOUD_HEIGHT_M, cameraAltitude))
      // the weather's look only under the clouds; above them clear air and sun
setLook(lookFor(weatherHere, sun, 1.5 · h · mpp))      // haze set against the camera–target distance
showRain(weatherHere.rain, pitch, look.night)
```

`smoothstep(a, b, x)`: `t = clamp((x − a)/(b − a), 0, 1)`; `t²(3 − 2t)`.

`showRain(amount, pitch, night)` quantises to avoid restarting the drops: `amount` to 0.05
steps, `pitch` to 10° steps, `night` to 0.1 steps; the `RainOverlay` component (a DOM overlay
above the map, below panels) only updates when one of them changes.

Sky geometry (deck's perspective camera sees `2·atan(1/3)` ≈ 36.87° top to bottom; the camera
is 1.5 viewport heights from the target):

```
halfFov   = atan(1/3)° ≈ 18.435
tilt      = max(1, pitch)
horizonAt = (1 − 3 / tan(tilt)) / 2            // fraction down the screen (0 top, 1 bottom); negative = above the top
top       = tilt + halfFov − 90                // elevation (deg above horizon) at the top edge
space     = clamp((11.5 − zoom) / 2.5, 0, 1)   // 1 at zoom ≤ 9 (globe hangs in space), 0 at zoom ≥ 11.5

sunAt = null
if zoom > 12 && sun.up > −0.02:
    forward = [sinB·sinP, cosB·sinP, −cosP];  right = [cosB, −sinB, 0];  up = [sinB·cosP, cosB·cosP, sinP]
    ahead = forward · sun
    if ahead > 0.05:
        f = 3 · (h/2)                                                // focal length in px
        sunAt = [ w/2 + (right·sun / ahead)·f,  h/2 − (up·sun / ahead)·f ]
```

(`B` = bearing, `P` = pitch; vectors as east, north, up.)

`skyCss(look, top, horizonAt, sunAt, space)` → drawn **behind the map** (the container's CSS
background; the map canvas is transparent where nothing is drawn, i.e. above the horizon and
around the globe):

```
displayRgb(linear, exposure) = round(255 · clamp(v · exposure, 0, 1)^(1/2.2))      per channel
elevationColour(deg) = mix(look.skyHorizon, look.skyZenith, sqrt(clamp(sin(deg), 0, 1)))    // linear RGB
atTop     = round(mix(displayRgb(elevationColour(max(top, 0))), [4, 7, 16],   space))
atHorizon = round(mix(displayRgb(look.hazeColor),               [12, 18, 34], space))
gradient: vertical, atTop at 0 %  →  atHorizon at clamp(horizonAt, 0, 1)·100 % (rounded to whole %)  →  atHorizon to 100 %

sun glow (in front of the gradient) when sunAt && max(look.sunColor) > 0.05 && space < 1:
    glow  = displayRgb(look.sunColor·0.5 + look.skyHorizon)
    alpha = min(0.85, max(look.sunColor) / 3.2) · (1 − space)                       // SUN = 3.2
    radial gradient centred on round(sunAt) px:
        0 px    rgb(255 252 240) at alpha
        14 px   rgb(255 250 235) at alpha
        40 px   glow at alpha · 0.55
        320 px  glow at 0
```

The CSS string is only written when it changes. Port: draw this as a full-screen pass before
the scene (vertical gradient + radial sun glow, both in display space with straight CSS-like
interpolation in gamma-encoded RGB), or clear to it. For dark and satellite basemaps the
background is the page background (see the UI/theme document) — nothing is drawn here.

The `Look` fields used (`skyHorizon`, `skyZenith`, `hazeColor`, `sunColor`, `exposure`,
`night`) and `lookFor`/`mixConditions`/`PRESETS` belong to the realistic-map document.

Also in this range: `CLOUD_*` constants and `cloudLayers` (3 row 3h), `WEATHER_UP_TO_M =
0.3 · CLOUD_HEIGHT_M`, `CLOUDS_FULL_ZOOM = 5.5`, `CLOUDS_GONE_ZOOM = 6.5`.

---

## 13. Deck creation, device pixel ratio, mount/teardown

### 13.1 `createDeck()` options (G:3684-3739)

| Option | Value | Port meaning |
|---|---|---|
| `parent` | the `.globe` div | the map fills its area |
| `views` | `GlobeView({resolution: 5})` | globe projection ≤ zoom 12, Web Mercator above (doc 01 §2); `resolution` = globe tessellation step in degrees for deck's generated geometry |
| `initialViewState` | `{...view, ...LIMITS}` | start view (1.4), limits `minZoom 0.5, maxZoom 20, maxPitch 60` |
| `useDevicePixels` | `pixelRatio()` | 13.2 |
| `controller` | `controllerFor(false)` | `NorthUpGlobeController`, zoom about pointer, drag pans, two-finger drag pans (doc 01 §3) |
| `effects` | `[lighting]` | ambient + camera + key light (5.2) |
| `deviceProps` | WebGL with `stencil: true`; dev: shader error debugging | depth-stencil attachment needed (`Depth24PlusStencil8`), stencil cleared each frame |
| `pickingRadius` | 6 | px |
| `onDeviceInitialized` | stores the GPU device | used for atlases / radar arenas |
| `onBeforeRender` | `updateLook` | section 12 |
| `onError` | `reportMapError` | a failing layer is surfaced, not just logged |
| `onViewStateChange` | below | |
| `getTooltip` | `tooltip` | 10.6 |
| `getCursor` | grabbing / pointer / grab | 10.2 |
| `onHover`, `onDragStart`, `onClick` | 10.2, 10.5 | |
| `layers` | `[earthLayer(basemap)]` | until the first render |

`onViewStateChange({viewState})` (user gestures and transitions report the new camera):

```
view = {longitude, latitude, zoom, bearing ?? 0, pitch ?? 0, position}
if floor(view.zoom·2) != floor(shownZoom·2): shownZoom = view.zoom       // reactive zoom in half steps (rail legend)
locked = follow && selected && Date.now() > transitionUntil
if locked: view = lockedView(view, selected)
vehicle = !locked && vehicleLocked && Date.now() > transitionUntil ? vehicleAt() : null
if vehicle: view.longitude/latitude = vehicle
onviewchange(view)
return (locked || vehicle) ? merged view : viewState                     // the corrected camera is what gets applied
```

### 13.2 Device pixel ratio (G:3741-3759)

`pixelRatio() = basemap == "realistic" ? min(devicePixelRatio, 1.5) : true` (`true` = the full
device pixel ratio). Re-applied when `basemap` changes; the same effect then eases the pitch
back within the new basemap's limit (`goTo({pitch: maxPitch}, 600 ms)` if over, unless
following an aircraft) or re-pushes the limits. Known shortcut in the source: the ratio is read
only when the basemap is set, so moving the window to a display with another scale factor keeps
the old ratio. Port: render-target scale = `min(scale_factor, 1.5)` for realistic, else
`scale_factor`; handle winit `ScaleFactorChanged` properly; all "px" constants in this document
are logical pixels.

### 13.3 Mount (G:3476-3510) and teardown (G:3761-3769)

Mount: create deck (errors → error card) → subscribe to realistic textures (redraw on
arrival) → capture-phase `wheel` listener (trackpad pan, doc 01 §3.7) and WebKit
`gesturestart/change/end` (pinch/twist, doc 01 §3.8) → WebGL context-lost listener (reports
"The graphics context was lost (WebGL context lost)"; port: handle wgpu surface/device loss by
re-creating and continuing) → `onviewchange(view)` → start the rAF loop → load the label font
("600 16px Overpass Variable"), then `fontsReady = true; render()` → `aircraftModels.load()` →
`loadTrainModels()` → fetch OpenFreeMap TileJSON (`https://tiles.openfreemap.org/planet`),
`tileUrl = tiles[0]`, render (failure ignored: labels/buildings are optional).

Teardown: stop the texture subscription, remove listeners, cancel the rAF, `freeArenas()`,
finalize deck; `hoverLoader.dispose()`.

### 13.4 Own gesture helpers in this range (G:3512-3592; detailed in doc 01 §3.7-3.8)

- `viewportOf(v)`: Web Mercator viewport above zoom 12, else globe viewport, at the container
  size. `mapAt(v, [x, y])` → `[lon, lat]` under a screen point or null off the globe.
- `cameraLocked()` = `(follow && selected) || vehicleLocked`.
- `moveView(next)`: clamp zoom to `limits()`, latitude to ±85, wrap bearing; re-aim if
  following an aircraft; update `shownZoom`; emit `onviewchange`; apply; `render()`.
- `zoomAbout(zoom, bearing, at)`: sets zoom/bearing and (when not locked) shifts
  longitude/latitude by `before − after` of `mapAt(at)` so the place under `at` stays put.
- `onWheel`: a ctrl-wheel during a WebKit pinch is swallowed; a trackpad scroll (not a mouse
  wheel) pans: new centre = `mapAt(view, [w/2 + dx, h/2 + dy])`; ignored while locked or during
  a transition.
- Pinch: `gesturestart` records `{zoom, bearing, at}`; `gesturechange` →
  `zoomAbout(pinched(...), at)` (about the screen centre when locked); `gestureend` clears.

---

## 14. Exported component methods (G:3258-3289)

| Method | Behaviour |
|---|---|
| `flyTo(longitude, latitude, zoom?)` | `near = cosAngle(target, view centre) > cos(2°)`; target zoom = `min(zoom ?? (near ? max(view.zoom, 8) : 8), maxZoomNow())`; longitude unwrapped to the shortest way round (`unwrapLongitude`); linear transition of longitude, latitude, zoom over **1500 ms** |
| `jumpTo(partialView)` | merge into `view`, emit `onviewchange`, apply immediately, `render()` (dev `#@lat,lon,zoom,pitch,bearing`) |
| `zoomBy(delta)` | zoom = `clamp(view.zoom + delta, 0.5, maxZoomNow())`, **250 ms** |
| `resetOrientation()` | bearing 0, pitch 0 over **400 ms** |
| `setPitch(pitch)` | pitch over **400 ms** |

`maxZoomNow() = follow && selected ? followMaxZoom(selected) : 20`;
`followMaxZoom(flight) = clamp(log2(metersPerPixel(0, flight.lat) / (1.3·size / (1.5·max(1, h)))), 20, 23)`
with `size = published model size ?? sizeOf(icon)` (G:281-287). These are the only exported
methods; everything else is driven by props and callbacks (`onselect`, `onshipselect`,
`ontrainselect`, `onairport`, `onviewchange`; see doc 01 §1.2-1.3 for call sites).

DOM owned by the component in this range (G:3780-3809; doc 01 §1.4): two tilt-shift blur
overlays (realistic + `tiltShift`: blur 2 px reaching 42 % from top/bottom, and 5 px reaching
22 %, masked by a vertical gradient), `RainOverlay`, the hover card, the rail speed legend
(`railMap && railSpeeds && shownZoom >= RAIL_MIN_ZOOM`; centred, 26 px above the bottom sheet,
10 px weight-600 data font, band chips min-width 26 px), the map error card.

---

## 15. Routes and estimated flight paths (`routes.ts`, `flightpath.ts`)

Pure geometry; no rendering. Port 1:1 into a Rust module with the same unit tests
(`*.test.ts` exist beside the sources).

### 15.1 `routes.ts` public API

- `decodeWaypoints(plan, departure?) → LonLat[]`: flight-plan waypoints are integer
  `[lat, lon]` pairs of unknown scale. Try divisors `1, 1e4, 1e5, 1e6, 1e7`; keep those where
  every point is a valid coordinate; with a departure airport choose the candidate whose first
  point is nearest to it, else the smallest valid divisor.
- `mergeTrail(trail, opensky) → {trail, added}`: prepend OpenSky points older than
  (first FR24 timestamp − 30 s) when there are ≥ 2; their speed is derived from the move to the
  next point (`km/1.852/dt·3600` kt, capped 700, rounded), `vertical_speed 0`, `source −1`.
- `routeFor(flight, details, traffic) → Route`: origin = airport by `schedule.origin_id` ??
  `flight.origin` ?? `plan.departure`; destination = by `schedule.diverted_id ||
  destination_id` ?? `flight.destination` ?? `plan.destination`. If the decoded filed plan has
  > 1 point → `kind "filed"` with only `filed`. Else `estimateRoute` with the first non-zero
  trail point, both airports' runways (`reference.runways`) and the live traffic.
- `airacCycle(now) → {ident, effective, expires}`: 28-day cycles from 2024-01-25 UTC (cycle
  2401); `ident = YY + two-digit cycle number within the year of the cycle's start`.
- Re-exports `distanceKm, greatCircle, trimToPosition, LonLat, PathPoint, RunwayGuess`.

### 15.2 `flightpath.ts` — constants

`EARTH_RADIUS_M 6 371 000`, `NM_M 1852`, `FT_M 0.3048`, `KT_MS 0.514444`, `GRAVITY 9.80665`,
`GLIDE_DEG 3`, threshold crossing 50 ft → `AIM_M = 50·0.3048/tan 3° ≈ 290.8 m`,
`FINAL_NM 10`, `MIN_FINAL_NM 5`, `MAX_FINAL_NM 25`, `MIN_FINAL_NM_CLOSE 2`,
`FINAL_STEP_NM 0.25`, `SHORT_FINAL_PENALTY 3`, `LONG_FINAL_PENALTY 0.2`, `TOO_HIGH_PENALTY 4`,
`BANK_DEG 25`, `TERMINAL_KT [140, 230]`, `ENROUTE_KT [140, 480]`, `STEEP_FT_PER_NM 500`,
`PLANNED_FT_PER_NM 350`, `CLIMB_FT_PER_NM 600`, `LOCAL_M 150 000`, `ENTRY_M 60 000`,
`JOIN_M 60 000`, `CLIMB_OUT_M 6 000`, `ARC_STEP 3°`, `LINE_STEP_M 2 000`,
`GREAT_CIRCLE_STEP_KM 25`, `ROLLOUT_M 1 400`, `TRAFFIC_KM 30`.

### 15.3 `flightpath.ts` — public API and algorithms

Spherical helpers: `distanceM/distanceKm` (haversine), `bearing(a, b)` (initial course, deg),
`destination(a, course, m)`, `greatCircle(a, b, stepKm = 80)` (n = clamp(ceil(km/step), 2, 256)
segments, longitudes unwrapped across the antimeridian), `turnRadius(kt) = v²/(g·tan 25°)`.
Local frames are azimuthal-equidistant around a centre (x east, y north, metres).

- `dubins(x0, y0, h0, x1, y1, h1, r)`: shortest path between two poses with turn radius `r`
  (the six words LSL, RSR, LSR, RSL, RLR, LRL from the standard closed forms, each candidate
  verified by integrating it — position error ≤ max(1 m, r·1e-4), heading error ≤ 1e-3 rad;
  shortest valid wins; none → straight line). Sampled every 3° on arcs and 2 km on straights.
- `turnThenStraight(x0, y0, h0, x1, y1, r)`: one turn (left or right, whichever is shorter and
  whose circle doesn't contain the target) then a tangent straight; fallback straight line.
- `plan(a: Pose, b, endHeading | null, radii)` (internal): if the ends are ≤ 150 km apart, one
  local plan around `b` (Dubins to `endHeading`, or turn-then-straight). Otherwise: great
  circle toward `b` (or toward an entry point 60 km short of `b` when an end heading is
  required); if the aircraft's heading is > 1° off that course, a local Dubins joins the great
  circle within `min(60 km, half the distance)`; then the great-circle leg (sampled every
  25 km); then, with an end heading, a local Dubins from the entry onto the end pose.
- `runwayEnds(runways, fieldElevationFt) → RunwayEndInfo[]`: both directions of each runway;
  only hard surfaces (`/ASP|CON|PEM|BIT|TAR|PAV|SEAL|MAC|CEM/i`) when the airport has any;
  heading = bearing start→far end; threshold moved in by `displacedFt`; drop ends shorter than
  `min(0.7·longest, 1800 m)`.
- `centrelineOffset(end, p) → {along, cross}`: metres before the threshold along the extended
  centreline (negative past it) and to the right of an approaching aircraft.
- `runwayUse(ends, traffic, ignoreId?)`: counts arrivals/departures per end from other flights
  whose track is within 15° of the end: on the ground at ≥ 40 kt on the runway (|cross| ≤ 60 m)
  → departure; airborne ≤ 3 000 ft AGL: on final (0 ≤ along ≤ 20 km, |cross| ≤ 150 + 0.06·along)
  → arrival; climbing out (≤ 12 km past the far end, |cross| ≤ 300 + 0.08·past) → departure;
  each flight counts for its best-aligned (smallest |cross|) end.
- `chooseArrivalRunway(ends, ac, traffic, ignoreId) → {end, reason}`: (1) an end the aircraft
  is landing on (`along ≤ 300`, still over the runway, |cross| < 150 m, track within 30°,
  < 1 000 ft AGL) or (2) lined up with (`300 < along < 30 NM`, |cross| < 300 + 0.08·along,
  track within 30°, < 6 000 ft AGL) → `"aligned"` (smallest cross); else (3) the direction most
  traffic uses (ends within 15° grouped, score = 2·arrivals + departures), preferring the end
  with more arrivals then the nearer centreline → `"traffic"`; else (4) the end with the
  shortest planned arrival → `"guessed"`.
- `chooseDepartureRunway(ends, to, traffic)`: among the flow in use (or all ends), the shortest
  departure plan → `"traffic"` / `"guessed"`.
- `arrivalPath(ac, end) → PathPoint[]`: ground track = `arrivalPlan`:
  - landing on the runway: straight on to a stop point
    `min(runway remaining − 100 m, AIM_M + 1 400 m)` past the threshold;
  - aligned: merge onto the centreline `lead = min(0.9·along, max(1 500, 1.2·(2·sqrt(r·|cross|) +
    r·offHeading)))` ahead (straight to threshold if < 200 m of final would remain), then the
    final;
  - otherwise: try final-approach fixes every 0.25 NM from 2 to 25 NM out; cost = path length
    to the fix + fix distance + penalty for a final shorter (3 ×) or longer (0.2 ×) than the
    usual `clamp(AGL / (tan 3° · NM/ft), 5, 10)` NM + 4 × the distance by which the path is too
    short to lose the height at 350 ft/NM; cheapest wins; then straight final to the threshold
    and on to the aiming point.
  Heights (`descend`): `elevation + min(start, max(glide(toGo), start − 500 ft/NM · flown))`
  with `glide(s) = s·tan 3°` in feet — level until the glide path is met from below, steep
  descent when above it.
- `arrivalToPoint(ac, airport, elevationFt)`: no runways — turn toward the airport, fly
  straight there, same descent rule.
- `departurePath(end, to)`: roll `min(0.6·length, 2 200 m)` to lift-off, climb straight ahead
  6 km, then `plan` to the first tracked position arriving on its track (radii from
  0.6·speed clamped to terminal speeds, and speed clamped to en-route speeds); heights
  (`climb`): steady gradient `max(600 ft/NM, top/total)` from lift-off, levelling at the
  target altitude; last point forced to `to.altFt`.
- `departureFromPoint(airport, elevationFt, to)`: no runways — the reverse of an arrival flown
  backwards from the first tracked position, climbing steadily.
- `trimToPosition(path, here, searchM = 20 000, nearM = 3 000)`: within the first 20 km of the
  path find the point nearest `here` (segment projection in a local frame); if nearer than
  3 km, return `[here at the interpolated altitude, …rest after that segment]`; otherwise just
  replace the first point's position by `here`. Longitudes unwrapped.
- `estimateRoute(input) → {kind, before, ahead, departure, arrival}`:
  `kind = origin || destination ? "estimated" : "none"`. *Ahead*: only when a destination is
  known and the flight is airborne: runway ends at field elevation `destination.alt ?? 0`,
  traffic within 30 km of the airport; with a runway choice → `arrivalPath` and
  `arrival = {runway: ident, reason}`, else `arrivalToPoint`. *Before*: only when the origin is
  known and the recorded trail starts > 15 km from it: `to` = first trail point (speed
  `ground_speed || 250`); with a runway → `departurePath` + `departure`, else
  `departureFromPoint`.

Types: `LonLat = [lon, lat]`; `PathPoint = [lon, lat, altFt]`;
`RunwayEndInfo = {ident, heading, start, threshold, elevationFt, lengthM}`;
`Traffic = {id, lat, lon, track, alt, speed, onGround}`;
`Aircraft = {lon, lat, track, altFt, speedKt}`; `RunwayGuess = {runway, reason}`;
`RunwayReason = "aligned" | "traffic" | "guessed"`.

---

## 16. Taxiways (`taxiways.ts`, `taxinet.ts`)

Not drawn by this code (the map's own aeroway lines are); they feed the motion model so taxiing
aircraft stay on the centrelines.

- `TAXIWAY_ZOOM = 12`. In the per-flight loop, at zoom ≥ 12, `taxiwaysFor(id, phase)`: when the
  phase is `parked`, `taxi`, `takeoffRoll` or `landingRoll`, and `motion.groundField(id)` gives
  an airport → `wantTaxiways(airport)`.
- `wantTaxiways(airport)`: needs a 4-letter ICAO code and the native backend. If loaded,
  refresh its last-wanted time. Else, unless pending or tried within the last 5 minutes
  (`RETRY_MS`), call the backend `airportTaxiways(icao, lat, lon)`; non-empty result →
  `motion.setTaxiways(code, lines)` and mark loaded; then evict least-recently-wanted networks
  beyond `MAX_LOADED = 6` (`motion.setTaxiways(code, null)`). Empty/failed → retried later.
- `TaxiNetwork` (`taxinet.ts`): built from `TaxiLine[]` (`{points: [lon, lat][]}`): local
  frame at the mean point (`M_PER_DEG = 6 371 000·π/180`, x scaled by `max(cos lat0, 1e-3)`,
  antimeridian-safe); every polyline segment longer than 0.1 m stored as start `(ax, ay)`,
  unit direction `(ux, uy)`, `len` (Float64 arrays); a hash grid of 50 m cells
  (`key = (cx + 32768)·65536 + (cy + 32768)`) lists each segment in every cell within one cell
  of the cells it passes through (sampled every 25 m). API: `x(lon)`, `y(lat)`,
  `covers(x, y)` (inside the bounding box grown by 100 m), `near(x, y)` → candidate segment
  ids, `metresPerDegLon`; `project(net, s, x, y)` → nearest point on segment `s` and its
  distance (module-level `projX, projY, projDist` outputs to avoid allocation; return a struct
  in Rust).

---

## 17. Liveries (`liveries.ts`)

`Part = "top" | "belly" | "wings" | "tail" | "engines"`; `Livery = Record<Part, RGB>`.

Base colours: `WHITE #F5F5F3`, `WING_GREY #E3E6EA`, `ENGINE_GREY #D6DAE0`.
`livery({tail, top?, belly?, wings?, engines?})`: `top = top ?? WHITE`; `belly = belly ?? top`;
`wings = wings ?? WING_GREY`; `engines = engines ?? ENGINE_GREY`.

- `DEFAULT_LIVERY` = top/belly/tail WHITE, wings WING_GREY, engines ENGINE_GREY.
- `MILITARY_LIVERY` = top `#8C9298`, tail `#8C9298`, wings `#80868C`, engines `#7A8086`
  (belly = top), for ICAO designators `RCH RRR GAF IAM BAF CTM CFC HKY ASY PAT CNV NAF`.
- `CURATED` (ICAO designator → spec; unspecified fields default as above):

```
RYR tail 073590 belly 073590 engines 073590      EZY tail FF6600 engines FF6600
WZZ tail C6007E engines 3E1A61                   DLH tail 05164D engines 05164D
EWG tail A6005E                                  BAW tail 1B2A59 belly 1B2A59 engines 1B2A59
AFR tail 002157                                  KLM top 00A1DE belly F5F5F3 tail 00A1DE engines 00A1DE
IBE tail D7192D                                  VLG tail FFCC00 engines 57585A
SWR tail E2001A        AUA tail EE2E24           AEE tail 00539F        THY tail C8102E
FIN tail 0B1560        NOZ tail D81939           EIN tail 006D60 engines 006D60
LOT tail 0D2C6C        VIR top E6E7EA tail DA0530   EXS tail E2231A     AFL top E5E7EA tail 0055A5
UAE tail D71921        QTR top F0F0F0 belly B9BBBE tail 5C0632 engines B9BBBE
ETD top E1D9C9 belly C5B597 tail B7924D engines C5B597      SVA belly D9C59B tail 0F5A99
MSR tail 003366        ETH tail 078930           RAM tail C1272D        ELY tail 0C2E66
DAL belly 0B2A5B tail 0B2A5B
AAL top C9CED4 belly A9B0B8 wings AEB4BA tail 0B5AB0 engines C9CED4
UAL belly 0C2C6B tail 0C2C6B engines 0C2C6B      SWA top 304CB2 tail 304CB2 engines 304CB2
JBU belly 0033A0 tail 0033A0 engines 0033A0      ASA tail 01426A
NKS top FFE600 tail FFE600 engines FFE600        ACA belly 1A1A1A tail 1A1A1A engines 1A1A1A
AMX tail 0B2343        LAN tail 1B0088           GLO tail FF6600 engines FF6600
AZU tail 1E4FA3 engines 1E4FA3                   CMP tail 002F6C
QFA tail E40000        VOZ tail CE0E2D           JST tail FF5C00        ANZ tail 111111 engines 111111
SIA tail 0B2363        CPA tail 006564           CSN tail 005BAC        CHH tail C8102E
AXM tail E31F26 engines E31F26                   THA tail 5F2D86        GIA tail 00758D
KAL top 7FB8E6 belly DDE0E3 tail F2F2F2 engines DDE0E3      JAL tail F5F5F3 engines F5F5F3
IGO belly 1F2A72 tail 1F2A72 engines 1F2A72      FDX belly 9DA3A9 tail 4D148C
UPS top F1EFEA belly 351C15 tail 351C15 engines 351C15      DHK top FFCC00 tail FFCC00 engines FFCC00
```

- `ALIASES` (subsidiary → parent): `RUK MAY→RYR; EJU EZS→EZY; WMT WAZ WUK→WZZ; CLH→DLH;
  SHT CFE EFW→BAW; HOP→AFR; KLC→KLM; IBS→IBE; NSZ NAX→NOZ; ENY PDT JIA→AAL; LPE TAM→LAN;
  QLK→QFA; AIQ AWQ→AXM; BCS DHX→DHK`.

`liveries.forFlight(f)` (`liveries.ts:177-189`):

1. `icao = airlineIcao(f.callsign)` (from `reference.svelte.ts`); none → default.
2. `CURATED[ALIASES[icao] ?? icao]` → that.
3. military set → `MILITARY_LIVERY`.
4. `iata = reference.airline(icao)?.iata || /^([A-Z0-9]{2})\d/` of `f.flight`; none → default.
5. logo-derived cache by IATA: known → it (or default if the lookup found nothing); unknown →
   queue a request and return default for now.

Logo colours: requests are batched — a 400 ms timer, up to 40 codes per batch, to the backend
`airlineColors(codes) → {code: {primary: RGB, secondary?: RGB}}`; result →
`{...DEFAULT, tail: primary, engines: secondary ?? ENGINE_GREY}` (or "none" cached as null);
`liveries.version++` (forces the colour attributes to refresh); on failure nothing is cached
(retried on a later sighting); remaining queue re-arms the timer.

Published models have no `engines` part, so `engines` colours only show on the built-in models.

---

## 18. Small pure modules consumed by the UI

### 18.1 `aircraft-readings.ts` (flight panel, "from the aircraft" values)

`aircraftReadings(m: AdsbAircraft) → {first: Reading[], more: Reading[], unreported: string[]}`;
`Reading = {label, value: string | null, title?, wide?, words?}`.

- `degrees(v)` = three-digit rounded heading + `°` (`007°`), wrapped 0–359.
- `number(v, unit, digits)` = locale-formatted with fixed fraction digits, or null if absent.
- `first` (always shown, `—` for null): `Heading` (`trueHeading`), `IAS` (` kt`), `Mach`
  (3 decimals), `Wind` (`"{degrees(windDir)} at {windSpeed} kt"`, wide, title "Derived from the
  aircraft's airspeed, heading and ground track").
- `more` (only reported ones; the rest go to `unreported` by their sentence names): `Bank`
  (`"{|roll| to 0 dp}° R|L"`, side only when |roll| > 0.5; "bank"), `GNSS altitude` (ft),
  `TAS` (kt), `Selected alt.` (`navAltitudeMcp ?? navAltitudeFms`, ft, title "Set on the
  autopilot (or by the flight management system)"; "selected altitude"), `Selected hdg`
  (title "Set on the autopilot; magnetic"; "selected heading"), `OAT` (` °C`),
  `Autopilot modes` (joined with ` · `, mapped `autopilot→Autopilot, vnav→VNAV,
  althold→Altitude hold, approach→Approach, lnav→LNAV, tcas→TCAS`, others upper-cased; wide,
  words).
- `listing(names)` = `"a, b and c"`.

### 18.2 `adsb.ts`

`matchAdsb(list, {hex, callsign, reg, lat, lon}) → AdsbAircraft | null`: with a Mode S hex,
exact (lower-case) match only. Otherwise candidates whose normalised (upper-case alphanumerics)
callsign matches and whose registration doesn't contradict, or whose registration matches;
the candidate must have a position; nearest within 150 km wins (equirectangular distance,
111.195 km/deg). `verticalRate(a) = a.baroRate ?? a.geomRate` (ft/min).

### 18.3 `freshness.ts`

`STALE_MIN_MS = 30 000`; `staleAfter(interval) = max(30 000, 2·interval)`;
`ageLabel(seconds)`: `"45 s"`, `"3 min"`, `"2 h"`, `"2 h 05 min"`;
`feedFreshness(ageMs, intervalMs)` → `{stale: true, label: "Data {age} old"}` when
`ageMs > staleAfter`, else `{stale: false, label: "Updated {age} ago"}`;
`afterRound(shown, {arrived, at, interval})` → a new `{at, interval}` clock only when a
snapshot arrived.

---

## 19. Zoom / pitch thresholds in this range (quick reference)

| Threshold | Effect |
|---|---|
| zoom ≤ 3 → 10 | aircraft size floor 8 → 24 px |
| zoom < 4 | labels only for the selected aircraft |
| zoom ≤ 4 / ≤ 7 / > 7 | render interval 500 / 150 / 50 ms (0 when locked and zoom > 7) |
| zoom 5.5 → 6.5 | clouds fade out (realistic) |
| zoom ≤ 6 | labels only where `facing > 0.42`; liveries off; meshes 4 % thick, unlit |
| zoom 6 → 8 | liveries fade in (0.1 steps) |
| zoom 6 → 9 | mesh depth and lighting fade in (0.1 steps) |
| zoom 6 → 11 | altitude exaggeration fades to 1× |
| zoom > 7 | label height line; label spread factor reaches 1 |
| depth ≥ 0.8 (zoom ≥ 8.4) | landing gear drawn |
| zoom 9 → 11 | aircraft shadows fade in |
| zoom ≤ 9 → 11.5 | realistic sky fades from space to atmosphere |
| zoom < 11 and pitch ≤ 5° | published models drawn as icons; otherwise meshes |
| zoom ≥ 11 | LOD 1 (detailed) meshes |
| zoom > 12 | flat projection; sun glow possible; lock raises the camera target; `yawOffset 0` |
| zoom ≥ 12 | taxiway networks requested for ground traffic |
| zoom ≥ 14 | trees, road widths (realistic) |
| follow | max zoom up to 23, pitch up to 85° |

---

## 20. Port notes, risks and open questions

1. **Selected aircraft colour with liveries on.** At zoom > 6 the per-part livery colours
   replace the blue selection tint (5.2); only icons and the zoom ≤ 6 single-colour meshes turn
   blue. Parity means reproducing this; confirm it is intended rather than an oversight.
2. **Trail/route with 3D terrain.** By deck's terrain rule (9) the `PathLayer`s (trail, trail
   shadow, route lines) have no instance anchor in WebGL and so are *draped* onto the ground
   when `terrain` is on, losing their height. This is inferred from the deck.gl 9.4 source, not
   observed in the running app; verify and decide whether the port should keep the trail at
   altitude (probably better) — a deliberate deviation.
3. **Globe-frame half turn.** `yawOffset = 180` and the `−e, −n` sun flip are deck.gl
   artefacts; use a real ENU frame per aircraft and drop both. Verify headings and shadow
   directions against the web build at zoom < 12 and > 12.
4. **`mpp` at the view centre** is used to size every aircraft; on the globe far from the
   centre the true-scale size is slightly off. Reproduce as-is for parity.
5. **Selection ring size** uses the icon-class size, not the published model's size, and the
   un-raised position (8.4). Reproduce or fix consciously.
6. **Label decluttering** relies on deck's GPU collision filter; a CPU greedy rectangle
   declutter with a short fade is the proposed replacement (7.5). Results will differ in
   detail (which of two equal-priority neighbours wins).
7. **SDF text** (font size 64, buffer 6, outline width 3) for labels/markers needs a wgpu text
   path with outlines (e.g. an SDF/MSDF atlas or glyph outlines stroked on CPU); egui's text
   has no outline. Shared with doc 01 §6.
8. **KTX2/Basis atlases**: need a Basis transcoder in Rust or a build step that converts the
   two aircraft atlases to plain single-channel textures with mips. They are alpha masks, so R8
   + mip chain is sufficient.
9. **Stencil shadows** require a depth-stencil target and consistent draw order (all shadows
   before aircraft; building/tree shadows share the same stencil rule).
10. **Lighting parity**: deck's Phong (ambient/diffuse/specular, camera light + key light) must
    be reproduced in the mesh shader, including the depth-dependent material; colours are
    blended in gamma space (doc 01 Risks).
11. **Picking**: choose GPU id-buffer or CPU picking (10.1). The 6 px radius and "topmost
    wins" matter for dense traffic; hover must trigger an immediate redraw for the highlight.
12. **Two clocks**: aircraft use the server-synced `motion.now()`; ships/trains/camera/sky use
    the local clock (`updateLook` uses `Date.now()` for the sun while aircraft shadows use the
    feed clock). Keep the distinction.
13. **Frame pacing**: the web app renders layers at 2–20 fps except when locked, but camera
    motion is always at display rate. On mobile the same intervals save battery; make sure
    camera transitions and gestures are not tied to the layer interval.
14. **Locale formatting**: `toLocaleString()` in altitude labels, hover card and readings uses
    the system locale's digit grouping; pick a Rust equivalent (e.g. `num-format` with the
    system locale) or fix to `en` grouping — owner's call.
15. **Hover on touch devices** (Android/iOS): there is no pointer hover; the hover card and
    highlight only exist on desktop today. Decide whether a long-press should show it.
16. **Device pixel ratio cap** (1.5 on realistic) is a performance choice for WebGL; on native
    it may be revisited per platform, but parity keeps it.
17. **Tilt-shift blur and rain overlay** are DOM/CSS effects over the canvas (backdrop blur,
    masked); natively they become post-process passes. Owned by the realistic-map/UI documents
    but created here.
18. The tooltip for ships/trains/signals/airports is deck's DOM tooltip; rebuild as an egui
    tooltip with the styles in 10.6. `--warn` and other theme tokens are defined in
    `src/app.css` (UI/theme document).
