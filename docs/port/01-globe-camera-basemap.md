# 01 — Globe component: props, camera model, input, basemap layers

Scope: `src/lib/Globe.svelte` lines 1–1350 (plus the camera/input/assembly code further down the
same file that those lines depend on), `src/lib/controller.ts`, `src/lib/trackpad.ts`,
`src/lib/geo.ts`, `src/lib/screen.ts`, `src/lib/daynight.ts`, `src/lib/places.ts`.

Purpose: enough detail to re-implement the camera, the input handling and every *base* layer
(earth sphere, land, vector map, true-width runways, satellite imagery, place/airport labels,
night shading, and the radar tile layer's skeleton) in winit + wgpu + egui without re-reading
the TypeScript.

Conventions in this document:

- `G:123` = `src/lib/Globe.svelte` line 123. Other files are named in full.
- Colours are `[R, G, B, A]`, 0–255, straight (non-premultiplied) alpha, sRGB values as written
  in the source. deck.gl does **no** gamma handling: the values are written to an 8-bit
  non-sRGB framebuffer as-is, and blending happens in that (gamma-encoded) space. A wgpu port
  that renders to an `*Srgb` surface must either convert these to linear and accept slightly
  different blend results, or render to a non-sRGB (`Bgra8Unorm`) target to match exactly. See
  "Risks".
- "zoom" is always deck.gl's zoom: the world is 512 px wide at zoom 0 at the equator
  (512-px tile convention — one more than the 256-px "slippy" zoom for the same scale).
- Stack: deck.gl 9.4.0, luma.gl 9.4.x, loaders.gl 4.5.x (`package.json:21-36`). WebGL2 backend.

Things in lines 1–1350 that are **not** basemap (aircraft label layers G:754-969, aircraft
shadow/material constants G:335-415, city buildings G:971-1092, radar lifted arenas
G:1111-1354, map-error handling G:1252-1277) are summarised briefly in section 9 so nothing in
the range is undocumented, but they belong to other port documents.

---

## 1. Component interface

### 1.1 `ViewState` (module export, G:2-10)

```ts
type ViewState = {
  longitude: number;   // degrees, kept in [-180, 180] by the controller
  latitude: number;    // degrees, clamped to [-85, 85] by our pan code (deck allows ±90)
  zoom: number;        // deck.gl zoom, see conventions
  bearing: number;     // degrees clockwise from north, wrapped to [-180, 180)
  pitch: number;       // degrees from straight-down, 0 = nadir
  position?: [number, number, number]; // metres; only [0,0,z] is ever used: the camera
                                       // target raised z metres. Only honoured in the flat
                                       // projection (zoom > 12).
};
```

Initial view (G:314): `{ longitude: 10, latitude: 35, zoom: 1.9, bearing: 0, pitch: 0 }`.

### 1.2 Props (G:154-255) and what the page passes (`src/routes/+page.svelte:1363-1414`)

| Prop | Type | Default | Page passes | Meaning |
|---|---|---|---|---|
| `flights` | `LiveFlight[]` | required | `shown` (filtered feed) | Aircraft to draw |
| `ships` | `Ship[]` | `[]` | `shipsOnMap` | Ships, under aircraft |
| `selected` | `LiveFlight \| null` | required | `selected` | Selected aircraft |
| `details` | `FlightDetails \| null` | required | `shownDetails` | Selected flight's details (trail) |
| `route` | `Route \| null` | required | `route` | Selected flight's route (origin/destination) |
| `airport` | `Airport \| null` | required | `boardAirport ?? null` | Airport to mark (board open) |
| `place` | `{lon,lat,name} \| null` | `null` | `place` | Searched place marker |
| `exaggeration` | `number` | required | `settings.exaggeration` | Altitude exaggeration when zoomed out (1 = true) |
| `basemap` | `"dark" \| "satellite" \| "realistic"` | required | `settings.basemap` | See section 5 |
| `labels` | `boolean` | required | `settings.labels` | Place/airport name labels |
| `labelScale` | `number` | required | `settings.labelSize` | Multiplier on label px sizes |
| `buildings` | `boolean` | required | `settings.buildings` | 3D buildings (flat projection only) |
| `sky` | `Conditions \| null` | `null` | `skyConditions` | Realistic map's weather look |
| `aircraft` (local name `showAircraft`) | `boolean` | `true` | `aircraftOn` | Draw aircraft at all |
| `aircraftLabels` | `boolean` | `false` | `settings.aircraftLabels` | Callsign labels |
| `daylight` | `boolean` | required | `settings.daylight` | Night shading |
| `weather` | `boolean` | required | `settings.weather` | Precipitation radar |
| `terrain` | `boolean` | `false` | `settings.terrain` | Shaded/3D relief |
| `tiltShift` | `boolean` | `false` | `settings.tiltShift` | Realistic map: blur top/bottom |
| `railMap` | `boolean` | `false` | `settings.railMap` | Railway network overlay |
| `trains` | `Train[]` | `[]` | `trainsOnMap` | |
| `selectedTrain` | `Train \| null` | `null` | `selectedTrain` | |
| `trainRoute` | `TrainRoute \| null` | `null` | `shownRoute` | |
| `trainBlocks` | `boolean` | `false` | `settings.railMap && settings.railShows === "blocks" && isTauri()` | |
| `ontrainrouteoff` | `() => void` | – | `() => reloadRoute?.()` | |
| `ontrainselect` | `(Train) => void` | – | clears `place`, `selectTrain(t)` | |
| `hoveredTrainId` | `string \| null` | `null` | `hoveredTrainId` | Sidebar hover highlight |
| `hoveredShipMmsi` | `number \| null` | `null` | `hoveredShipMmsi` | |
| `follow` | `boolean` | required | `following` | Lock camera on selected aircraft |
| `followVehicle` | `boolean` | `false` | `followingVehicle` | Lock camera on selected train/ship |
| `onselect` | `(LiveFlight \| null) => void` | required | clears `place`; on `null` also deselects ship and train; `select(f?.id ?? null)` | Aircraft clicked / empty map clicked |
| `selectedShip` | `Ship \| null` | `null` | `selectedShip` | |
| `onshipselect` | `(Ship) => void` | – | clears `place`, `selectShip(s)` | |
| `onairport` | `(iata: string) => void` | – | (opens the airport board) | Airport label clicked |
| `onviewchange` | `(ViewState) => void` | – | `(v) => (view = v)` | Every camera change, ours or the user's |

Any prop change triggers an immediate re-render rather than waiting for the frame timer
(G:3771-3777).

### 1.3 Exported methods (called through `bind:this={globe}`)

| Method | Source | Behaviour | Page call sites |
|---|---|---|---|
| `flyTo(lon, lat, zoom?)` | G:3259-3267 | 1500 ms linear transition of longitude/latitude/zoom; see 3.6 | train select `Math.max(view.zoom, 10)` (`+page:806`), ship select `Math.max(view.zoom, 11)` (`:812`), flight select `flyZoom` (`:999`, `:1064`), history entry `11` (`:1079`), searched place `placeView(...)` (`:1101`), airport board locate `11` (`:1650`) |
| `jumpTo(partialView)` | G:3270-3275 | Instant; dev-only `#@lat,lon,zoom,pitch,bearing` URL hash (`+page:1332-1336`) | |
| `zoomBy(delta)` | G:3277-3280 | 250 ms zoom transition, clamped | keys `+`/`=` → `+1`, `-` → `-1` (`+page:1212-1214`), map controls `onzoom` (`:1724`) |
| `resetOrientation()` | G:3283-3285 | 400 ms to bearing 0, pitch 0 | key `n` (`:1211`), map controls `onreset` (`:1725`) |
| `setPitch(pitch)` | G:3287-3289 | 400 ms to that pitch | key `t`: `view.pitch > 5 ? 0 : 55` (`TILT = 55`, `+page:107,1210`), map controls `ontilt` (`:1726`) |

### 1.4 DOM the component owns (G:3780-3809)

- A `div.globe`, `position:absolute; inset:0; isolation:isolate`. The deck.gl canvas fills it.
  Page background behind the canvas is `--ink` = `#1e1e1e` (`src/app.css:19,72`): this is the
  "space" colour around the globe on the dark and satellite basemaps (the canvas is cleared
  transparent). On the realistic basemap the container gets a CSS sky gradient instead
  (`updateLook`, G:3647-3682), cleared to `""` on the others.
- Optional tilt-shift blur overlays (realistic only): two full-size layers with
  `backdrop-filter: blur(2px)` reaching 42% from top and bottom, and `blur(5px)` reaching 22%,
  each masked by `linear-gradient(black, transparent R, transparent calc(100% - R), black)`
  (G:3781-3785, 3818-3828).
- `RainOverlay`, `HoverCard`, rail speed legend, and the map-error card (section 9.5).
- Cursor: `grabbing` while dragging, `pointer` while hovering a pickable object, else `grab`
  (G:3714).

---

## 2. Projection and camera model

deck.gl's `GlobeView` is used with `resolution: 5` (G:3687). Two different projections are
used depending on zoom, exactly at the threshold `FLAT_ABOVE_ZOOM = 12` (G:266; deck's
`GlobeView.getViewportType`: `viewState.zoom > 12 ? WebMercatorViewport : GlobeViewport`).

- `zoom <= 12`: **globe** (3D sphere, perspective camera).
- `zoom > 12`: **flat** Web Mercator (perspective camera over a plane, like MapLibre/Mapbox).

The two are built to agree in scale at the view centre, so the switch is seamless there.
Everywhere in the code `const flat = zoom > FLAT_ABOVE_ZOOM` (G:2525).

`resolution: 5` is the globe tessellation step in degrees: deck.gl subdivides polygon/bitmap
geometry so no edge spans more than ~5° when drawn on the sphere (default is 10). A native port
must tessellate large quads/polygons (land rectangle, night quads, imagery tiles at low zoom,
cloud bitmap) at 5° or finer on the globe.

### 2.1 Shared camera constants

| Constant | Value | Source |
|---|---|---|
| Camera distance from target | `CAMERA_HEIGHTS = 1.5` viewport heights | G:3072; deck `altitude = 1.5` |
| Vertical field of view | `2·atan(0.5/1.5) = 2·atan(1/3)` ≈ 36.87° | G:3661-3662 |
| Focal length in px | `1.5 · viewportHeightPx` (so `f = 3·(h/2)`) | G:3676 |
| Earth radius (globe viewport) | 6 370 972 m | deck `globe-viewport.js` |
| Earth circumference (scale maths) | 40 075 016.686 m | `geo.ts:10` |
| World size at zoom 0 | 512 px | `geo.ts:61`, `places.ts:26` |

Metres per pixel at the view centre, both projections (`geo.ts:60-61`):

```
metersPerPixel(zoom, lat) = 40_075_016.686 · cos(lat) / (512 · 2^zoom)
```

Distance from the camera to its target in metres (used all over: `followMaxZoom`, haze,
`lockedView`, trees radius):

```
cameraDistanceM = 1.5 · viewportHeightPx · metersPerPixel(zoom, lat)          (G:3111, 2697)
cameraAltitudeM = cameraDistanceM · cos(pitch)                                  (G:3604-3605)
```

### 2.2 Globe viewport (zoom ≤ 12) — exact maths from deck.gl 9.4 `globe-viewport.js`

Common ("world") space: a sphere of radius `GLOBE_RADIUS = 256` units centred at the origin.

```
projectPosition([lng, lat, zMetres]):
  D = (z / 6_370_972 + 1) · 256
  x =  sin(λ)·cos(φ)·D
  y = -cos(λ)·cos(φ)·D
  z =  sin(φ)·D                      (λ = lng in rad, φ = lat in rad)
```

So lon 0/lat 0 is at `(0, -256, 0)`, the north pole at `(0, 0, 256)`, lon 90°E at `(256, 0, 0)`.
`unitsPerMeter = 256 / 6_370_972`.

Scale:

```
zoomAdjust(lat) = log2(π · cos(lat))            // lat clamped to ±85.051129 for this
scale = 2^(zoom − zoomAdjust(lat))  =  2^zoom / (π · cos(lat))
```

With that, one common unit is `scale` px at the view centre, so the globe's circumference on
screen is `2π·256·scale = 512·2^zoom / cos(lat)` px: the globe is enlarged by `1/cos(lat)` as
the view centre moves away from the equator, to match Web Mercator's local scale. Consequence:
**the globe visibly grows as you pan toward the poles at constant zoom.**

View matrix (column-vector convention, applied right to left to a common-space point, after
the base Viewport translates the view centre's surface point to the origin):

```
V = lookAt(eye = [0, −1.5, 0], center = [0,0,0], up = [0, 0, 1])
    · Rx(−pitch) · Ry(−bearing) · Rx(latitude) · Rz(−longitude) · S(scale / height)
```

Projection: perspective, `fovy = 2·atan(1/3)`, aspect = width/height,
`near = 0.5` (`nearZMultiplier`), 
`far = 1.5 + (256·2·scale / height) / max(cos(pitch), 0.1)` (`farZMultiplier = 1`).
Units here are viewport heights.

Because near is 0.5 viewport heights in front of the camera (one third of the way to the
target), anything standing more than `cameraDistanceM · 2/3` toward the camera from the target
is clipped — relevant to the "earth sphere is clipped up close" remark at G:567.

`unproject(screen xy)` on the globe = ray/sphere intersection with radius `256` (or
`(targetZ/6_370_972 + 1)·256`); when the ray misses the sphere the maths yields the closest
limb point (the `sqrt(max(0, …))`), i.e. it does **not** return null for off-globe pixels.
`mapAt` (G:3518-3521) only rejects non-finite results.

Default GlobeView GPU parameter: `cullMode: "back"` for every layer unless the layer overrides
it (deck `globe-view.js`). See section 4.

### 2.3 Flat viewport (zoom > 12)

Standard deck.gl/MapLibre `WebMercatorViewport`: world is a 512×512-unit square
(`x = 512·(lng+180)/360`, `y = 512·(1/2 − ln(tan(π/4 + φ/2))/(2π))` measured north-up in deck's
common space), scaled by `2^zoom`, camera 1.5 viewport heights from the target along the
pitched/rotated view axis, same `fovy`. `position: [0, 0, z]` raises the camera target `z`
metres above the plane (the camera rises with it; used for locking on an aircraft at altitude
and for aiming at 3D terrain). No back-face culling difference: GlobeView's `cullMode:"back"`
default still applies to layers in this mode.

Local frame orientation differs between the projections: models placed on the globe need an
extra 180° yaw (`yawOffset = flat ? 0 : 180`, G:2540-2541, 2042). The key light direction is
only re-aimed in the flat projection, whose common space is east/north/up (G:428-445).

deck.gl 9.4 bug worked around at G:257-261: `GlobeController` calls
`viewport.getZoomAnchorStrength()` (a GlobeViewport-only method) while wheel-zooming even when
the viewport is the Mercator one, and throws. The code patches
`WebMercatorViewport.prototype.getZoomAnchorStrength = () => 1` (full anchoring at the cursor
in flat mode). The port simply uses anchor strength 1 in flat mode.

### 2.4 Limits (G:265-306)

| Name | Value | When |
|---|---|---|
| `LIMITS` | `minZoom 0.5`, `maxZoom 20`, `maxPitch 60` | dark and satellite basemaps, not locked |
| `REALISTIC_LIMITS` | same but `maxPitch 75` | `basemap === "realistic"`, not locked |
| `FOLLOW_MAX_PITCH` | 85° | locked on an aircraft (`follow && selected`) |
| `FOLLOW_CLOSEST` | 1.3 aircraft lengths | closest camera distance when locked |
| `FOLLOW_MAX_ZOOM` | 23 | absolute cap when locked |
| `minPitch` | 0 | deck default |
| latitude | ±85 for our own pans (`controller.ts:10`, G:3530, 3125); deck itself allows ±90 | |

`limits()` (G:294-300):

```
if !(follow && selected):  basemap == realistic ? REALISTIC_LIMITS : LIMITS
else:
   shift = log2(max(0.05, cos(selected.lat)))
   { minZoom: 0.5, maxZoom: followMaxZoom(selected) − shift, maxPitch: lockedMaxPitch }
```

The `− shift` exists only because deck's `GlobeController._constrainZoom` clamps zoom to
`[minZoom + a, maxZoom + a]` with `a = zoomAdjust(lat) − zoomAdjust(0) = log2(cos(lat))`.
**This latitude shift applies to all zoom limits passed to deck**, i.e. in the current app the
effective free-camera range is `[0.5 + log2(cos lat), 20 + log2(cos lat)]` (e.g. at 60°N:
−0.5 … 19). A native port should reproduce this (parity) or deliberately drop it; note that
`zoomBy` and `moveView` clamp with the *unshifted* `LIMITS.minZoom`/`maxZoomNow()` /
`limits()` values, so the two paths are already slightly inconsistent in the source.

`followMaxZoom(flight)` (G:281-287):

```
size    = aircraftModels.forFlight(typecode, icon)?.size ?? sizeOf(icon)     // metres
closest = (1.3 · size) / (1.5 · max(1, containerHeightPx ?? 900))            // m per px
zoom    = log2(metersPerPixel(0, flight.lat) / closest)
return min(23, max(20, zoom))
```

`maxZoomNow() = follow && selected ? followMaxZoom(selected) : 20` (G:302).

When the basemap changes (G:3750-3759) and the camera isn't locked: if `view.pitch > maxPitch`
ease to `maxPitch` over 600 ms (`["pitch"]`), else just re-apply limits. When a lock ends with
`zoom > 20` or `pitch > 60`: ease back to `min(zoom, 20)`, `min(pitch, 60)` over 600 ms
(`["zoom","pitch"]`, G:3213-3221; note it uses `LIMITS`, not the realistic 75°).

### 2.5 Device pixel ratio (G:3741-3747)

`useDevicePixels`: `true` (full DPR) on dark/satellite; on realistic `min(devicePixelRatio, 1.5)`.
Re-evaluated only when `basemap` changes.

### 2.6 Frame pacing (G:3232-3243)

A `requestAnimationFrame` loop calls `render()` at most every:

| Condition | Interval |
|---|---|
| locked (aircraft or vehicle, and no transition running) and zoom > 7 | every frame (0 ms) |
| zoom > 7 | 50 ms |
| zoom > 4 | 150 ms |
| else | 500 ms |

`render()` rebuilds the full layer list (`buildLayers`) and re-applies the camera. deck.gl
itself still redraws at display rate during gestures and transitions; this timer only governs
how often *data* (aircraft positions, night polygons, etc.) is refreshed. Prop changes, tile
loads, hover changes etc. call `render()` immediately.

---

## 3. Camera control and input

### 3.1 Controller configuration (G:3081-3087)

```
controllerFor(locked) = {
  type: NorthUpGlobeController,
  zoomAround:     locked ? "center" : "pointer",
  dragMode:       locked ? "rotate" : "pan",
  multiTouchDrag: locked ? null     : "pan",
}
```

`locked` = `(follow && selected) || vehicleLocked` (switched in `renderFrame`, G:3180-3187).
Everything else is deck.gl's default: `scrollZoom: true` (speed 0.01, not smooth),
`dragPan: true`, `dragRotate: true`, `doubleClickZoom: true`, `touchZoom: true`,
`touchRotate: false`, `keyboard: true`, `inertia: 0` (no inertia/fling), `pickingRadius: 6` px
(G:3695).

### 3.2 Gesture table (free camera)

| Input | Result |
|---|---|
| Left-drag | Pan (north-up; see 3.3) |
| Right-drag, or left-drag with Ctrl/Shift/Alt/Meta | Rotate: bearing and pitch (3.4) |
| Mouse wheel | Zoom about the pointer (3.5) |
| Shift + wheel | Same, at quarter speed (mjolnir multiplies delta by 0.25) |
| Double-click | Zoom in ×2 (one level) about the pointer, 300 ms linear transition; with a modifier key: zoom out ×0.5 |
| Trackpad two-finger scroll | Pan (our handler, 3.7) |
| Trackpad pinch (WebKit `gesture*`) | Zoom about the fingers + twist rotates bearing (3.8) |
| Trackpad pinch elsewhere (arrives as `ctrlKey` wheel) | Falls through to deck's wheel zoom |
| Touch: one finger drag | Pan |
| Touch: two-finger pinch | Zoom about the pinch centre (`touchZoom`), no twist-rotate (`touchRotate: false`) |
| Touch: two fingers dragged together | Pan (`multiTouchDrag: "pan"`) |
| Keyboard arrows | Pan 100 px per press; with modifier: Left/Right bearing ∓15°, Up/Down pitch ±10° |
| Keyboard `-` / `=` | Zoom out/in ×2 about the centre (twice with modifier), 300 ms transition (deck). The page *also* binds `+`,`=`,`-` to `zoomBy(±1)` (`+page:1212-1214`); see open questions |

Locked camera (aircraft or vehicle):

| Input | Result |
|---|---|
| Any drag (either button, any modifier) | Rotate/orbit (`dragMode:"rotate"`; `controller.ts:28-43` forces `isFunctionKeyPressed → false` and `rightButton → false`) |
| Wheel / pinch | Zoom about the screen centre |
| Two-finger trackpad scroll | Swallowed (no pan) (G:3558) |
| Two-finger touch drag | Disabled (`multiTouchDrag: null`) |

### 3.3 North-up pan (`controller.ts`)

deck 9.4's stock globe pan is a trackball that also changes bearing; this subclass replaces it.

On pan start (deck `GlobeState.panStart`) it records the start pointer position, the start
camera frame `{longitude, latitude, bearing}` and

```
angularRate = (0.25 / 2^(zoom − zoomAdjust(lat, clampToPoles))) · π/180   // radians of arc per px
            = 0.25° · π·cos(lat) / 2^zoom  per px
```

(0.25°/px at scale 1 means the globe surface tracks the pointer approximately; it is exact at
the view centre only.)

On each move (`controller.ts:55-73`), with `start`, `frame`, `rate`:

```
right = (start.x − pos.x) · rate
up    = (pos.y − start.y) · rate
b     = frame.bearing (rad)
north = up·cos(b) − right·sin(b)
east  = up·sin(b) + right·cos(b)
latitude  = clamp(frame.latitude + north (in degrees), −85, 85)
longitude = frame.longitude + east (in degrees) / max(0.15, cos(frame.latitude))
bearing   = frame.bearing                     // unchanged
zoom      = stock trackball pan's zoom        // = startZoom + zoomAdjust(newLat) − zoomAdjust(frame.lat)
```

The zoom term keeps the *visual* scale constant while panning north/south: since the globe is
drawn `1/cos(lat)` larger, deck adds `log2(cos(newLat)/cos(startLat))` to zoom as latitude
changes (latitudes clamped to ±85.051129 for this). (Strictly the code takes the zoom from the
trackball result, whose latitude can differ slightly from the north-up one; reproduce with the
north-up latitude — the difference is imperceptible.) **This means panning changes the zoom
value**, which matters at the zoom-12 projection switch and for zoom-dependent styling.

This same pan runs in the flat projection too (the controller is the globe one at all zooms).

`applyConstraints` override (`controller.ts:75-84`): after deck's constraint pass (which, for
pointer-anchored zoom, rotates the camera frame and may change bearing), bearing is put back to
its pre-constraint value wrapped to `[-180, 180)`: `((b % 360) + 540) % 360 − 180`. So
wheel-zooming never turns the map.

deck's `applyConstraints` otherwise: clamp zoom (latitude-shifted limits, 2.4); wrap longitude
to ±180; clamp latitude ±90; clamp pitch to `[0, maxPitch]`; and if latitude changed, add
`zoomAdjust(newLat) − zoomAdjust(oldLat)` to zoom.

### 3.4 Drag-rotate (deck `MapState._getNewRotation`)

With drag start `(sx, sy)`, current `(x, y)`, viewport `w × h`, start bearing/pitch:

```
bearing = startBearing + 180 · (x − sx) / w
dy = y − sy
if dy > 0 (dragging down) and |h − sy| > 5:   s = (dy / (sy − h)) · 1.2      // negative
if dy < 0 (dragging up)   and sy > 5:         s = 1 − y / sy                 // positive
s = clamp(s, −1, 1)
if s > 0: pitch = startPitch + s · (maxPitch − startPitch)
if s < 0: pitch = startPitch − s · (minPitch − startPitch)                   // minPitch = 0
```

So a full-width drag turns the map 180°, and dragging up to the top of the window reaches
`maxPitch`. Rotation is about the view centre (globe) — deck's pivot-point rotation only applies
when a controller provides `getAltitude`, which this one doesn't.

### 3.5 Wheel zoom (deck `Controller._onWheel`)

```
delta = −deltaY                 (× 40 if deltaMode is LINE; ÷ devicePixelRatio on Firefox pixel mode;
                                 × 0.25 with Shift)
s     = 2 / (1 + exp(−|delta · 0.01|))          // 1 … 2
scale = delta < 0 ? 1/s : s
zoom += log2(scale)                              // no animation
```

(A typical 100-px mouse notch: `s = 2/(1+e^−1) ≈ 1.462`, i.e. ≈ 0.548 zoom levels.)

Anchor: `zoomAround: "pointer"` keeps the geographic point under the cursor fixed;
`"center"` zooms about the view centre. On the globe the anchoring is damped near the limb
(deck `getZoomAnchorStrength`): let `r` = (distance of the pointer ray from the globe centre) /
(globe radius). Strength is 1 for `r ≤ 0.75`, falls with a smoothstep to 0 at `r ≥ 1.15`
(`1 − smoothstep((r − 0.75)/0.40)`), and the lon/lat correction is multiplied by it; 0 = zoom
about centre. With north-up bearing (|bearing| < 1°) the correction is abandoned (centre zoom)
if the anchor is beyond ±85.05° latitude or its longitude differs by more than 90°, and scaled
down so latitude never passes ±85.05°. In flat mode strength is always 1.

### 3.6 Programmatic moves (G:3245-3289)

All go through `goTo(target, durationMs, props)`:

```
transitionUntil = now + duration
view state := { ...view, ...target, ...limits() }, interpolated linearly (LinearInterpolator,
              easing t => t, i.e. NO easing curve) over `duration` on exactly the listed props
```

`transitionUntil` gates everything that would fight a transition: follow re-aiming, vehicle
centring, terrain aim easing, trackpad pan (G:3174, 3188, 3208, 3558).

- `flyTo(lon, lat, zoom?)`: 
  `near = cosAngle(target, viewCentre) > cos(2°)`;
  `longitude = unwrapLongitude(view.longitude, lon)` (shortest way round, may leave ±180 during
  the flight; `geo.ts:121-126`);
  `zoom = min(zoom ?? (near ? max(view.zoom, 8) : 8), maxZoomNow())`;
  1500 ms on `["longitude","latitude","zoom"]`. Longitude, latitude and zoom are each
  interpolated linearly — it is **not** a great-circle or "zoom-out-then-in" flight. A flight
  that crosses zoom 12 switches projection mid-way.
- `jumpTo(partial)`: `view = {...view, ...partial}`, notify `onviewchange`, apply, render. No
  transition, no clamping.
- `zoomBy(delta)`: `zoom = min(maxZoomNow(), max(0.5, view.zoom + delta))`, 250 ms, `["zoom"]`,
  about the view centre.
- `resetOrientation()`: `{bearing: 0, pitch: 0}`, 400 ms, `["bearing","pitch"]`.
- `setPitch(p)`: `{pitch: p}`, 400 ms, `["pitch"]`.
- Follow-vehicle engage (G:3160-3169): `goTo({lon, lat, zoom: max(view.zoom, 13)}, 800,
  ["longitude","latitude","zoom"])`, then `vehicleLocked = true`.

`placeView(place, w, h)` (`places.ts`), used by the page for searched places:

```
KIND_ZOOM = { address 17.5, poi 16.5, street 16, district 14, postcode 13, airport 13,
              town 12, city 11, region 7, country 5 }           (fallback: poi)
fitZoom([west,south,east,north], w, h):
   lonSpan = max(east >= west ? east − west : east + 360 − west, 1e-6)
   midLat  = min(|(south+north)/2|, 85)
   latSpan = max(north − south, 1e-6) / cos(midLat)
   return log2( min( w·0.7·360 / (512·lonSpan),  h·0.7·360 / (512·latSpan) ) )
zoom = bbox ? clamp(fitZoom, kindZoom − 2, kindZoom + 1.5) : kindZoom
zoom = clamp(zoom, 2, 17.5)
```

(w, h are `window.innerWidth/innerHeight`.) `placeDetail` and `placeLanguage` in the same file
are UI text helpers: second line = `category ?? KindWord` + ` · ` + label-without-name;
languages offered to Photon: `en, de, fr, it`, else local names.

### 3.7 Trackpad two-finger scroll = pan (G:3547-3563, `trackpad.ts`)

A capture-phase, non-passive `wheel` listener on the container runs before deck.gl:

1. If `e.ctrlKey` and a WebKit pinch is in progress: swallow the event (WebKit sends both).
2. `isTrackpadScroll(e)` (`trackpad.ts:13-19`):
   - `ctrlKey` → false (that is a pinch-zoom; let deck zoom);
   - `deltaX !== 0` → true;
   - if legacy `wheelDeltaY` exists and is non-zero → `wheelDeltaY === −3·deltaY` (trackpads;
     mouse wheels report multiples of 120);
   - else `deltaMode === 0 (pixels) && !Number.isInteger(deltaY)`.
   Not a trackpad → return, deck zooms.
3. Swallow the event. If the camera is locked or a transition is running: do nothing.
4. `[dx, dy] = wheelPixels(e, containerHeight)`: deltas × 16 for line mode, × page height for
   page mode, × 1 for pixels.
5. New centre = the map point currently under screen point
   `(w/2 + dx, h/2 + dy)` (`mapAt` = viewport unproject); `moveView({...view, lon, lat})`.

A native port gets real device information from winit (`MouseScrollDelta::PixelDelta` vs
`LineDelta`, and `TouchpadMagnify`/`TouchpadRotate` on macOS), so the heuristic is unnecessary:
`PixelDelta` → pan by those pixels, `LineDelta` → wheel zoom.

`moveView(next)` (G:3524-3538): clamp zoom to `limits()`, latitude to ±85, wrap bearing; if
locked on an aircraft re-aim with `lockedView`; notify `onviewchange`; apply; render.

### 3.8 Pinch and twist (WebKit `gesturestart/change/end`, G:3564-3592)

- start: remember `{zoom, bearing, at: pointer position within the container}`.
- change: `pinched(start, e)` (`trackpad.ts:36-39`):
  `zoom = start.zoom + log2(max(0.01, e.scale))`,
  `bearing = wrapBearing(start.bearing − e.rotation)` (rotation in degrees, clockwise positive:
  the map turns with the fingers). Then `zoomAbout(zoom, bearing, at)` where `at` is the
  start point, or the screen centre when locked.
- `zoomAbout` (G:3539-3546): `before = mapAt(view, at)`, `after = mapAt(next, at)`; shift
  `longitude += before.lon − after.lon`, `latitude += before.lat − after.lat` (skipped when
  locked), then `moveView`.
- end: clear.

### 3.9 Follow / lock camera on an aircraft (G:3089-3129, 3171-3223, 3700-3712)

Active when `follow && selected` and no transition is running. Applied every rendered frame
(`renderFrame`) **and** synchronously inside the view-state-change callback during gestures
(otherwise the aircraft jumps during a zoom).

`lockedView(v, flight)`:

```
pose     = motion.pose(flight, motion.now())            // the same pose the model is drawn at
altitude = elevation(pose.altFt, effectiveExaggeration(exaggeration, v.zoom))    // metres
```

with (`geo.ts:47-57`)

```
elevation(altFt, ex)            = max(0, altFt) · 0.3048 · ex
effectiveExaggeration(ex, zoom) = 1 + (ex − 1) · clamp((11 − zoom)/5, 0, 1)   // full ≤ z6, true scale ≥ z11
```

- **Flat (zoom > 12)**: `lockedMaxPitch = 85`; view = `{...v, longitude: pose.lon,
  latitude: pose.lat, position: [0, 0, altitude]}` — the camera target is raised to the
  aircraft, so it sits dead centre at any tilt/bearing/zoom.
- **Globe (zoom ≤ 12)**: the camera can only aim at the ground, so the ground aim point is
  pushed forward along the bearing until the aircraft lies on the camera axis:

  ```
  camera         = 1.5 · containerHeight · metersPerPixel(v.zoom, pose.lat)
  lockedMaxPitch = min(85, acos(min(1, altitude / (0.9 · camera))) in degrees)   // LOCK_CAMERA_ABOVE = 0.9
  tilt   = min(v.pitch, lockedMaxPitch)
  height = min(altitude, 0.9 · camera · cos(tilt))
  lift   = height · tan(tilt)                                   // metres ahead
  latitude  = clamp(pose.lat + lift·cos(bearing) / 111_320, −85, 85)
  longitude = pose.lon + lift·sin(bearing) / (111_320 · max(0.05, cos(pose.lat)))
  view = {...v, longitude, latitude, pitch: tilt, position: undefined}
  ```

While locked: limits become the follow limits (2.4), controller becomes orbit-only with
centre zoom (3.1), the frame timer runs every frame above zoom 7 (2.6), the locked aircraft is
exempt from off-screen culling (G:2563-2565, 2586), and `onviewchange` is called with the
re-aimed view each frame.

When the lock ends with the target raised (`raised`): `position` is reset to `[0,0,0]` with
plain `LIMITS` (G:3196-3203), then the 600 ms ease back inside limits (2.4) if needed.

### 3.10 Follow a train or ship (G:3141-3179, 3708-3709)

`vehicleAt()` = drawn position `[lon, lat]` of the selected train (else ship), only when
`followVehicle` and not following an aircraft. After the 800 ms fly-in (3.6), every frame and
every gesture sets `view.longitude/latitude` to it; zoom, bearing, pitch stay the user's.
Controller is in locked mode.

### 3.11 Terrain aim (G:3204-3212)

When not locked on an aircraft: `ground = terrain && zoom > 12 ? groundHeight(lon, lat) : 0`
(may be `null` while loading); if `|ground − position.z| > 0.5` m and no transition is running,
`position.z += (ground − position.z) · 0.3` per rendered frame (`GROUND_EASE = 0.3`).

### 3.12 `onviewchange` contract

Called: once on mount with the initial view (G:3493); on every deck view-state change (user
gesture or transition frame, G:3700-3711); on every follow/vehicle re-aim; from `jumpTo` and
`moveView`. The page stores it and derives the feed's fetch area from it via
`viewArea(view, window.innerWidth, window.innerHeight)` (`+page:444,590,889`).

### 3.13 Visibility helpers (`geo.ts`)

```
cosAngle(lon1, lat1, lon2, lat2) = sin a·sin b + cos a·cos b·cos(lon1 − lon2)     // great-circle cos

visibleRadius(zoom, pitch, w, h)      // degrees around the view centre worth drawing/fetching
   = min(100, 1.3 · (hypot(w, h)/2) · (360 / (512·2^zoom)) · (1 + 4·sin(pitch)))

viewArea(view, w, h):
   r = visibleRadius(...);  if r >= 50 → null (fetch the whole world)
   south = max(−90, lat − r); north = min(90, lat + r)
   dLon  = r / max(0.05, cos(max(|south|, |north|)))
   if dLon >= 180 → { south, north, west: −180, east: 180 }
   else { south, north, west: wrap(lon − dLon), east: wrap(lon + dLon) }   // may cross the antimeridian (west > east)
```

Aircraft are skipped when `cosAngle(aircraft, centre) <= cos(visibleRadius)` (G:2543-2586).

Other `geo.ts` exports recorded for completeness:

- `AIRCRAFT_COLOR = [255,255,255]`, `SELECTED_COLOR = [55,142,240]` (the UI's `--amber`, which is
  in fact a blue `#378ef0`).
- `ALTITUDE_STOPS` (feet → RGB, linear interpolation between, clamped at the ends, channels
  rounded): 0 `[255,98,0]`; 4 000 `[255,196,0]`; 8 000 `[160,230,40]`; 15 000 `[40,220,130]`;
  25 000 `[0,200,230]`; 35 000 `[70,125,255]`; 45 000 `[205,85,255]`.
- `extrapolate(f, nowMs)`: dead reckoning. If `onGround` or `speed < 40` kt → unchanged. Else
  `dt = clamp((now − timestampMs)/1000, 0, 90)` s; angular distance
  `d = speed·0.514444·dt / 6_371_000`; standard great-circle destination formula from
  `(lat, lon)` along `track`; longitude wrapped to ±180.

### 3.14 `screen.ts` — `screenAngle(viewport, [lon,lat,z], heading, stepM)`

Angle (degrees, anticlockwise — deck IconLayer's `getAngle`) that turns a north-pointing icon
to face `heading` as seen on screen:

```
ahead = [ lon + stepM·sin(h) / (111_320 · max(0.01, cos(lat))),  lat + stepM·cos(h) / 111_320,  z ]
(x0,y0) = project([lon,lat,z]);  (x1,y1) = project(ahead)
if non-finite or hypot(dx,dy) < 1e-3 → null (caller falls back)
return −atan2(dx, −dy) in degrees
```

`stepM` comes from `ICON_HEADING_STEP_PX = 20` px at the view centre (G:394).

---

## 4. Shared GPU state for base layers (G:325-333, 724)

| Name | Value | Meaning |
|---|---|---|
| (GlobeView default) | `cullMode: "back"` | applies to any layer without `NO_CULL`, in both projections |
| `NO_CULL` | `{ cullMode: "none" }` | camera-facing quads (text, rings, paths) and geometry with inconsistent winding |
| `SURFACE` | `{ depthWriteEnabled: false }` | ground layers: depth-*tested* (default `less-equal`) against the earth sphere so the far side is hidden, but they write no depth and so paint strictly in layer order (no z-fighting) |
| `SURFACE_NO_CULL` | `{ cullMode: "none", depthWriteEnabled: false }` | |
| `LABEL_PARAMETERS` | `{ cullMode: "none", depthCompare: "always", depthWriteEnabled: false }` | text ignores depth entirely; far-side labels are filtered on the CPU instead |

Blending for everything in this document is deck.gl's default: enabled,
colour `src-alpha, one-minus-src-alpha`, alpha `one, one-minus-src-alpha`. Depth compare
default `less-equal`, depth write default on. The WebGL context is created with a stencil
buffer (`deviceProps.webgl.stencil = true`, G:3694), used only by shadows.

Because the ground layers write no depth, the only depth written beneath vehicles is the
earth sphere (globe mode; nothing in flat mode) plus 3D things (terrain, buildings).

Lighting effect (G:417-427) — affects only lit meshes (aircraft, buildings), not base layers
(`material: false` on the earth and radar): ambient white intensity 1; camera light
`[255,248,235]` intensity 0.55; directional key light `[255,244,225]` intensity 0.7 direction
`[-1,-2,-3]` (normalised; altitude ≈ 53°).

---

## 5. Base layers and draw order

`buildLayers()` (G:2520-3057) assembles the list every render; deck.gl draws in list order.
The list always starts with the earth sphere. Per basemap:

### 5.1 Order, dark basemap (`basemap === "dark"`)

1. `earth` — sphere, colour `[50,50,50]`
2. `terrain-surface` (only if `terrain`; 3D relief mesh, other doc)
3. `land` — world rectangle `[50,50,50,255]`
4. shaded-relief `terrainLayers` (only if `terrain`)
5. `map-globe` or `map-flat` — vector map (needs `tileUrl`)
6. `aeroway-true-globe` / `aeroway-true-flat` — true-width runways (only when
   `45 / metersPerPixel >= 4`)
7. rail network layers (if `railMap`)
8. radar: `radar-…` tile layer, lifted stacks, measured volumes (if `weather`)
9. `night` (if `daylight`)
10. `buildings-city` (if `buildings && flat && tileUrl`)
11. ships (halo, ships, ring), signal blocks, train route, trains (halo, trains, ring)
12. `label-tiles` + `labels` (if `tileUrl && labels && fontsReady`)
13. selected-flight trail/route/airport markers, aircraft, aircraft labels, selection ring
    (other docs)

### 5.2 Order, satellite basemap

1. `earth` — `[6,16,34]`
2. `terrain-surface` (if `terrain`)
3. `imagery` — Esri World Imagery tiles
4. shaded-relief `terrainLayers` (if `terrain`)
5. (no vector map, no land polygon, **no runways/roads/borders**)
6. onward as dark from step 7 (rail, radar, night, buildings, vehicles, labels, aircraft)

### 5.3 Order, realistic basemap (detail in the realistic-map document)

1. `earth` — `POLES = [226,234,240]` (`realistic-ground.ts:96`)
2. `terrain-surface` (if `terrain`, with the `TERRAIN_AIR` haze extension)
3. `realisticLandLayer`, `landCoverGroundLayer` (flat only), `cityGroundLayer` (flat) or
   `groundLayer` (globe), `railGroundLayers`, `terrainLayers` (always; opacity 1.4 with
   terrain on, else 1; haze-faded), cloud bitmaps (opacity `1 − smoothstep(5.5, 6.5, zoom)`)
4. rail, radar as above; **no `night` layer** (the realistic shaders light the night side)
5. buildings, trees (zoom ≥ 14), vehicles, labels, aircraft

If `terrain` is on, every layer except ids matching `/^(earth$|terrain-surface$|clouds-)/` gets
deck's `TerrainExtension` appended (draped or raised onto the terrain, G:3059-3067).

### 5.4 `earth` — the sphere (G:451-464)

- Geometry: UV sphere, radius **6.3e6 m** (deliberately ~71 km *below* the 6 370 972 m surface so
  surface layers always pass the depth test against it), 36 latitude × 72 longitude segments,
  centred at `[0,0,0]` in **Cartesian** (earth-centred) coordinates.
- Colour by basemap (`GROUND`, G:453): dark `[50,50,50]`, satellite `[6,16,34]`, realistic
  `[226,234,240]`. Opaque, unlit (`material: false`), flat colour.
- Parameters: all defaults → back-face culled, depth test `less-equal`, **depth write ON**.
  This is the occluder that hides the far side of the globe for every `SURFACE` layer and the
  vector tiles.
- In flat mode (zoom > 12) a Cartesian sphere has no meaning; per the comment at G:567 it is
  "absent in the flat projection" and "clipped up close" on the globe (near plane). The port
  should draw it only in globe mode. It is why an explicit `land` polygon exists.
- Role per basemap: on dark it *is* the land colour seen beyond ±85.06° and through any gaps;
  on satellite/realistic it only shows at the poles (beyond the Mercator tiles) and while
  tiles load.

### 5.5 `land` — land rectangle (dark only) (G:567-585)

- One polygon: `[-180,-85.06] → [180,-85.06] → [180,85.06] → [-180,85.06]`.
- Fill `LAND = [50,50,50,255]`. Parameters `SURFACE` (back-face culled, depth-tested, no depth
  write). Not extruded, unlit.
- On the globe it must be tessellated (≤ 5° steps) to follow the sphere.
- Water is then painted over it from the vector tiles (the tiles carry water polygons, not
  land).

### 5.6 `map-globe` / `map-flat` — vector map (dark only) (G:492-541, 587-608)

Data source:

- TileJSON fetched once on mount from `https://tiles.openfreemap.org/planet` (G:308,
  3503-3509); the tile URL template is `tilejson.tiles[0]` (it changes with each planet
  build, e.g. `https://tiles.openfreemap.org/planet/<build>/{z}/{x}/{y}.pbf`, so it must be
  read at runtime, never hard-coded). If the fetch fails, the map simply has no vector
  layer, labels or buildings (silently ignored).
- Format: Mapbox Vector Tiles, **OpenMapTiles schema**. `minZoom 0`, `maxZoom 14` (tiles
  above 14 are over-zoomed from z14). deck's MVTLayer default `tileSize` is 512, so the
  tile z requested ≈ `floor(view zoom)` (clamped 0…14).
- Only these source layers are decoded: `water`, `boundary`, `transportation`, `aeroway`
  (G:539-541). Decoding runs in a bundled worker.
- Two layer ids (`map-flat` when `zoom > 12`, else `map-globe`) because tiles are decoded per
  projection; each has its own tile cache. Crossing zoom 12 therefore re-fetches/re-decodes
  (HTTP cache permitting) and can flash the map empty. A native port with one tile cache in
  lon/lat does not need this.

Rendering config: `filled: true`, `stroked: false` (no polygon outlines), line widths in
**screen pixels**, points not drawn (`getPointRadius: 0`), parameters `SURFACE_NO_CULL`.
Lines use deck PathLayer defaults: butt caps, miter joints (miter limit 4), no dashes,
`lineWidthMinPixels 0`.

Palette (G:497-508):

| Name | RGBA |
|---|---|
| `NONE` | `[0,0,0,0]` |
| `LAND` | `[50,50,50,255]` |
| `WATER` | `[30,30,30,255]` |
| `BORDER` (country) | `[118,118,118,255]` |
| `BORDER_STATE` | `[84,84,84,255]` |
| `ROAD_MAJOR` | `[88,88,88,255]` |
| `ROAD_MINOR` | `[66,66,66,255]` |
| `RUNWAY` | `[163,119,52,255]` |
| `TAXIWAY` | `[105,80,46,255]` |
| `APRON` | `[60,60,60,255]` |

Polygon fill (`vectorFill`, G:510-514) by source layer:

| Source layer | Condition | Fill |
|---|---|---|
| `water` | any | `WATER` |
| `aeroway` | `class == "runway"` | `RUNWAY` |
| `aeroway` | `class == "taxiway"` | `TAXIWAY` |
| `aeroway` | anything else (apron, helipad, …) | `APRON` |
| others | | `NONE` (transparent) |

Line colour (`vectorLine`, G:515-526) and width in px (`vectorLineWidth`, G:527-538):

| Source layer | Condition | Colour | Width px |
|---|---|---|---|
| `boundary` | `maritime` truthy | `NONE` | (1 or 0.6, invisible) |
| `boundary` | `admin_level <= 2` | `BORDER` | 1 |
| `boundary` | `admin_level > 2` | `BORDER_STATE` | 0.6 |
| `transportation` | `class` ∈ {motorway, trunk} | `ROAD_MAJOR` | 1.4 |
| `transportation` | `class == primary` | `ROAD_MAJOR` | 1 |
| `transportation` | `class` ∈ {secondary, tertiary, minor, service} | `ROAD_MINOR` | 0.6 |
| `transportation` | any other class (rail, path, track, ferry, …) | `NONE` | 0.6 (invisible) |
| `aeroway` | `class == runway` | `RUNWAY` | `RUNWAY_PX = 4`, or **0** when the true-width layer is active |
| `aeroway` | `class == taxiway` | `TAXIWAY` | 1.5 |
| `aeroway` | other | `NONE` | 1.5 (invisible) |
| others | | `NONE` | 0 |

Notes:

- There are no per-zoom style rules in the code; what appears at each zoom is whatever
  OpenMapTiles puts in the tile at that zoom (e.g. minor roads only from ~z11-13, aeroways
  from ~z10, state boundaries from ~z3-4). Widths are constant in pixels at every zoom.
- Feature draw order *within* a tile is deck's GeoJsonLayer order: all polygon fills, then all
  lines, in tile feature order. So water/apron/runway polygons are under every line; among
  lines, order follows the tile's layer order (`water`, …, `boundary`, `transportation`,
  `aeroway` as the tile stores them).
- `water` polygons include oceans, lakes and wide rivers. Line-type waterways (`waterway`
  layer) are not loaded.
- Road polygons (e.g. pedestrian areas, bridges as polygons in `transportation`) get fill
  `NONE`.
- Runways appear both as polygons (where OSM maps the area) and as centre lines.

### 5.7 `aeroway-true-globe` / `aeroway-true-flat` — runways at real width (dark only) (G:543-565, 2672-2676)

Close in, a 4-px centre line would be thinner than the runway. Switch condition:

```
trueAeroways = RUNWAY_WIDTH_M / metersPerPixel(zoom, centreLat) >= RUNWAY_PX      // 45 m / mpp >= 4 px
```

i.e. `mpp <= 11.25` m/px — about zoom ≥ 12.8 at the equator, ≥ 12.2 at 50°, ≥ 11.8 at 60°
(so it can engage in either projection, hence two ids). At the switch the pixel line is
exactly as wide as the metre line, so nothing jumps.

- Same tile URL, `minZoom 0`, `maxZoom 14`, only the `aeroway` source layer.
- `filled: false`, `stroked: false`; lines only: colour `RUNWAY` for `class == "runway"`,
  `NONE` otherwise; width **45 metres** (`lineWidthUnits: "meters"`) for runways, 0 otherwise.
- Parameters `SURFACE_NO_CULL`. Drawn immediately after the vector map layer.
- While active, the vector map layer's runway *line* width is 0 (polygons and taxiways still
  drawn by the map layer). Taxiways stay 1.5-px centre lines at every zoom.

Related but outside this range: from `TAXIWAY_ZOOM = 12` (`taxiways.ts:9`) the app fetches
OSM taxiway geometry for airports with aircraft on the ground, to move taxiing aircraft along
them (G:2574-2589).

### 5.8 `imagery` — satellite imagery (satellite basemap) (G:466-490)

- URL: `https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}`
  (note `{z}/{y}/{x}` order; standard XYZ/Web-Mercator scheme, JPEG, no API key in the code).
- `minZoom 0`, `maxZoom 19`, `tileSize 256` → tile z ≈ `floor(view zoom) + 1` (clamped to 19;
  beyond that tiles are stretched). `maxRequests: 12` concurrent.
- Each tile is a bitmap over its lon/lat bounding box, reprojected from Mercator onto the globe
  (i.e. texture v must follow Mercator y within the tile, not latitude linearly; on the globe the
  quad is tessellated).
- Tint: `tintColor [150,158,180]` — texture RGB is multiplied by `(150/255, 158/255, 180/255)`
  = `(0.588, 0.620, 0.706)` — a darkening with a slight blue cast so traffic stands out. Opaque.
- Parameters `SURFACE` (culled back faces, depth-tested, no depth write). Default linear
  filtering, no mipmaps beyond deck's default texture settings.
- The imagery has no text; place labels come from the label layer (5.9) when enabled.
- Tile refinement is deck TileLayer's default `best-available`: while a tile loads, already
  loaded ancestors/descendants are shown in its place.

### 5.9 `label-tiles` + `labels` — place names and airport codes (all basemaps) (G:610-752, 2743-2754)

Drawn when `tileUrl && labels && fontsReady`. Two layers:

**`label-tiles`** (data only, renders nothing): a tile layer over the same OpenFreeMap URL,
`minZoom 0`, `maxZoom 14`, `tileSize 512`. Each tile is decoded on the main thread to GeoJSON in
WGS84, source layers `place` and `aerodrome_label` only, then filtered by `isLabel`:

- geometry must be a `Point`;
- `aerodrome_label`: kept only if it has an `iata` property;
- `place`: kept only if `class` ∈ {`country`, `city`, `town`, `village`, `suburb`}.

Whenever the set of visible tiles finishes loading (`onViewportLoad`), `labelFeatures` = the
concatenation of every visible tile's features (no de-duplication across tiles; none needed
since each point lives in one tile at a given zoom).

**Horizon filter** (`labelsInView`, G:694-707): at `zoom > 6` all features pass. At `zoom <= 6`
keep only features with `cosAngle(feature, viewCentre) > 0.42` (within ≈ 65.2° of the centre).
Cached by `round(cLon), round(cLat), featureCount`.

**Marked airports removed** (`withoutMarked`, G:709-720, 2744-2746): `aerodrome_label`
features whose `iata` equals the marked airport's (`airport` prop) or, when a flight is selected
with a route, the route's origin/destination IATA are dropped (those have their own markers).

**`labels`** text layer (G:722-752):

| Property | Value |
|---|---|
| Position | the feature's `[lon, lat]`, altitude 0 |
| Font | `"Overpass Variable"`, weight 600 (bundled web font; must be loaded before the glyph atlas is built — `fontsReady`, G:3495-3499) |
| Glyph rendering | SDF atlas, 64 px glyphs, buffer 6, character set built from the data (`"auto"`) |
| Size | `labelSize(f)` px × `sizeScale = labelScale` (the user's setting) |
| Outline | width 3 (SDF outline units), colour `LABEL_OUTLINE = [6,12,19,220]` |
| Anchor | deck defaults: horizontally centred (`middle`), vertically centred on the point |
| Billboard | yes (always faces the screen, upright regardless of bearing/pitch) |
| Pickable | yes — click on an airport label fires `onairport(iata)` (G:3732-3734) |
| GPU parameters | `LABEL_PARAMETERS` (no cull, depth always, no depth write) |
| Collision | deck `CollisionFilterExtension`, group `"labels"`, priority `labelPriority(f)`, collision test drawn with `sizeScale: 1.3` (labels reserve 1.3× their own size, independent of `labelScale`) |

Text (`labelText`, G:624-628): airports → the IATA code; countries → name upper-cased;
others → name. Name = `name_en || name:latin || name || ""` (G:621).

Size in px before `labelScale` (`labelSize`, G:630-643), colour (`labelColor`, G:645-651) and
collision priority (`labelPriority`, G:653-658; higher wins), with `rank = Number(p.rank ?? 10)`:

| Feature | Size px | Colour RGBA | Priority |
|---|---|---|---|
| airport (`aerodrome_label`) | 12 | `[55,142,240,255]` (selection blue) | 600 |
| `country` | 12 | `[162,172,183,255]` | `900 − 10·rank` |
| `city`, rank ≤ 3 | 14 | `[241,238,231,245]` | `800 − 10·rank` |
| `city`, rank > 3 | 13 | `[241,238,231,245]` | `800 − 10·rank` |
| `town` | 12 | `[179,191,204,235]` | `400 − 10·rank` |
| `village`, `suburb` | 11 | `[179,191,204,235]` | `100 − 10·rank` |

Collision behaviour to reproduce: deck's CollisionFilterExtension renders all labels of a
group to an off-screen "collision map" in priority order and hides (fades out) any label whose
anchor area is covered by a higher-priority label. It is GPU/screen-space, re-evaluated as the
camera moves, with a short fade. A native port will most likely do CPU greedy placement:
sort by priority descending, place a label if its box (inflated ×1.3) doesn't intersect an
already-placed one. Collision groups are independent: `"labels"` (places/airports) and
`"aircraft-labels"` never hide one another.

### 5.10 `night` — night shading (dark and satellite, when `daylight`) (G:1094-1109, `daynight.ts`)

- Recomputed once per minute (`floor(now / 60_000)` changes); `now` is the motion model's clock
  (`motion.now()`, the feed's clock, G:2522-2523).
- Geometry: quads from `nightPolygons(now)`; every quad is filled with
  `NIGHT_COLOR = [2,6,14,46]` (alpha 46/255 ≈ 0.18). Parameters `SURFACE_NO_CULL`.
- Four **overlapping** sets of quads, for sun altitude below `0°, −6°, −12°, −18°`
  (`TWILIGHT_STEPS`: sunset, civil, nautical, astronomical). Overlap makes the darkening
  cumulative: opacity after n layers = `1 − (1 − 46/255)^n` → 0.180, 0.328, 0.449, 0.549.
  So full night darkens the map by ≈ 55% toward `[2,6,14]`.
- Drawn after the radar and before buildings, vehicles and labels: it darkens map, rail and
  radar, but not buildings, ships, trains, aircraft or text.
- Not drawn on the realistic basemap.

Sun position (`subsolarPoint(timeMs)`, `daynight.ts:10-23`), low-precision almanac:

```
d  = time/86_400_000 − 10_957.5                       // days since J2000.0
g  = 357.529 + 0.98560028·d                            (deg)  mean anomaly
q  = 280.459 + 0.98564736·d                            (deg)  mean longitude
L  = q + 1.915·sin g + 0.020·sin 2g                    (deg)  ecliptic longitude
e  = 23.439 − 0.00000036·d                             (deg)  obliquity
RA = atan2(cos e · sin L, cos L)                       (deg)
declination = asin(sin e · sin L)                      (deg)
eqTime = wrap180(q − RA)                               (deg)
utcHours = (time / 3_600_000) mod 24
sun.lat = declination
sun.lon = wrap180(−15·(utcHours − 12) − eqTime)
wrap180(x) = ((x mod 360) + 540) mod 360 − 180
```

Sun altitude at a point: `asin(clamp(sin φ·sin δ + cos φ·cos δ·cos(λ − λsun), −1, 1))`.

Band geometry (`nightBelow(altitude, sun)`, `daynight.ts:53-73`), per threshold:

- For each meridian `lon = −180, −178, …, 180` (2° step): scan latitudes `−85 … 85` in 1° steps
  and take `[lo, hi]` = first and last latitude where the sun altitude is `< threshold`
  (`belowOn`; assumes the night part of a meridian is one interval), or none.
- For each 2° strip between consecutive meridians, if either side has an interval, emit the
  quad `[lon, a.lo] → [lon+2, b.lo] → [lon+2, b.hi] → [lon, a.hi]`, where a missing side is
  replaced by a zero-height interval at the other side's midpoint (tapers the band to a point).
- Up to 180 quads per threshold, 720 total. Latitude resolution is 1°, so the terminator is a
  slightly stepped polyline; band edges stop at ±85°.
- On the globe each quad (2° wide, up to 170° tall) must be tessellated ≤ 5° in latitude.

### 5.11 Radar tile layer skeleton (when `weather`) (G:1111-1124, 1152-1153, 1205-1225, 1340-1354)

Recorded here because it sits in the base stack between the map and the night shading; the
contouring/mesh pipeline (`radar.ts`, `radar-mesh.ts`, `radar-arena.ts`, `radar-pool.ts`) and
the NOAA MRMS measured volumes belong to the weather document.

- Frame discovery: `GET https://api.rainviewer.com/public/weather-maps.json`, refreshed when
  older than 600 000 ms (10 min) (G:2681). Latest past frame =
  `maps.radar.past[last]`; tile URL template
  `${maps.host}${latest.path}/256/{z}/{x}/{y}/2/1_1.png`. On failure: no radar.
- Tile layer: id `radar-${url}-${radarCoverageKey()}`, `minZoom 0`, `maxZoom 7`,
  `tileSize 256`, parameters `SURFACE`. Each tile's PNG is turned into vector polygons off-thread
  (`loadRadarTile`); the ground picture is drawn as flat-colour polygons with layer opacity
  **0.55**, parameters `SURFACE_NO_CULL`, unlit.
- Lifted 3D stack (only when the camera is tilted): `liftFor(pitch) =
  round(clamp((pitch − 10)/30, 0, 1)·10)/10` — zero up to 10° pitch, full at 40°, in 0.1 steps.
  For each level `i` in `RADAR_LEVELS_KM`: slice at
  `z = elevation(km·3280.84, round(ex·10)/10)` metres with opacity
  `max(0.16, 0.34 − 0.03·i) · lift`, plus walls from the level below (or the ground) up to it,
  same opacity. Parameters `RADAR_LIFTED = { cullMode: none, depthWriteEnabled: false }`
  (depth-tested, unlike surface layers they are raised geometry). Raised by a model matrix:
  slices translate z by `z`; walls scale z by `(z − bottom)` and translate by `bottom`.
- Measured volumes: checked every 120 000 ms when running in the app (G:2686-2688), for the
  area `viewArea(view, w, h)`.

---

## 6. Label font and atlas notes for the port

- Family "Overpass Variable" at weight 600 for all map text (place labels, aircraft labels).
  The UI font stack is `"Middle Dot", "Overpass Variable", "Segoe UI", system-ui, sans-serif`
  (`app.css:44`); data font "Overpass Mono Variable".
- deck renders text from a 64-px SDF atlas (`buffer: 6`, default `radius 12`, `cutoff 0.25`)
  with `outlineWidth: 3` in the dark ink `[6,12,19,220]`. Visually: light text with a soft dark
  halo roughly 1–1.5 px wide at 12 px size. In egui/wgpu this needs either an SDF/MSDF text
  pipeline for map labels (recommended, also gives cheap outlines and thousands of glyph quads
  in one draw) or stroked text; plain egui text has no outline.
- Text is not rotated with the map and not scaled by zoom (`sizeUnits: "pixels"`).

---

## 7. Mount / teardown sequence (G:3476-3510, 3684-3739, 3761-3769)

1. Create the renderer with the initial view and `LIMITS`, free-camera controller, lighting
   effect, stencil buffer, picking radius 6, initial layers `[earth]`.
2. Install the capture-phase wheel handler and WebKit gesture handlers.
3. Listen for context loss → map-error card (9.5).
4. `onviewchange(view)`; start the frame loop.
5. Load the label font, then `fontsReady = true` and render.
6. Load aircraft models and train models (render when each lands).
7. Fetch the OpenFreeMap TileJSON → `tileUrl` → render.
8. On destroy: remove listeners, cancel the loop, free radar arenas, finalize the renderer.

Click handling (G:3726-3736): aircraft layer object → `onselect(flight)`; else ship →
`onshipselect`; else train → `ontrainselect`; else the `labels` layer with an
`aerodrome_label` that has an IATA → `onairport(iata)`; else if nothing at all was picked →
`onselect(null)`. (Clicking a non-airport label does nothing, and does not deselect.)

Hover (G:3715-3725): only objects on layers whose id starts with `aircraft-` produce a hover
card; hover is cleared on drag start.

---

## 8. Zoom thresholds referenced in this range (quick reference)

| Zoom | What changes | Source |
|---|---|---|
| 0.5 | min zoom (latitude-shifted, 2.4) | G:267 |
| 1.9 | initial zoom | G:314 |
| 3 → 10 | aircraft minimum size 8 px → 24 px (`minPxAt`) | G:337-339 |
| 4 | aircraft labels for all (below: selected only) | G:759 |
| > 4 / > 7 | data refresh 150 ms / 50 ms (else 500 ms) | G:3237 |
| 5.5 → 6.5 | realistic clouds fade out | G:3609-3610 |
| ≤ 6 | far-side filter for place labels (`cos > 0.42`) and aircraft labels | G:696, 2594 |
| 6 → 8 | liveries fade in | G:2547 |
| 6 → 9 | aircraft gain depth/shading (`depthAt`, 0.1 steps) | G:343 |
| 6 → 11 | altitude exaggeration fades to true scale | `geo.ts:54-57` |
| 7 | aircraft label height line appears | G:761 |
| 8 | default `flyTo` zoom; rail map min zoom | G:3264; `rail-layer.ts:44` |
| 9 → 11 | aircraft shadows fade in | G:356 |
| 11 | detailed aircraft meshes; icons stop (`CLOSE_ZOOM`) | G:347, 2550 |
| > 12 | flat Web Mercator; buildings possible; raised camera target; sun-aimed key light fades in over 12→13 | G:266, 2694, 2724 |
| 12 | taxiway fetching (`TAXIWAY_ZOOM`) | `taxiways.ts:9` |
| ~11.8–12.8 | true-width runways (latitude dependent: `mpp <= 11.25`) | G:2673 |
| 13 | min zoom when engaging vehicle follow | G:3167 |
| 14 | vector/label tile max zoom; trees; surveyed road widths; buildings stand up | G:593, 1003-1005, 1043 |
| 15.5 | Streets GL tiles (`SGL_FROM_ZOOM = 16 − 0.5`) | `streets-gl.ts:17,24` |
| 19 | imagery max tile zoom | G:485 |
| 20 | max zoom, free camera | G:267 |
| 23 | max zoom, locked on an aircraft | G:275 |

Pitch: ≤ 5° counts as "flat view" for icon silhouettes (`FLAT_PITCH`, G:345); 10°→40° radar
stack fades in; 55° = the `t` key's tilt; 60 / 75 / 85 = max pitch (free / realistic / locked).

---

## 9. Other definitions inside lines 1–1350 (owned by other port documents)

Recorded briefly so the range is fully covered.

### 9.1 Aircraft drawing constants (G:335-415)

- Size floor: `minPxAt(zoom) = 8 + 16·clamp((zoom − 3)/7, 0, 1)` px.
- `depthAt(zoom) = round(clamp((zoom − 6)/3, 0, 1)·10)/10`.
- `GEAR_DEPTH = 0.8`; `GEAR_COLOR = [178,182,190]`.
- `SHADOW_ZOOM = [9, 11]`; `SHADOW_COLOR = [4,8,16]`.
- `SHADOW_PARAMETERS`: no cull, no depth write, depth `less-equal`, stencil write mask `0xff`,
  compare `equal` (against reference 0), pass op `increment-clamp`, fail/depth-fail `keep` —
  each pixel takes at most one shadow.
- `ICON_HEADING_STEP_PX = 20`; `OUTLINE_COLOR = [8,12,20,215]`.
- Mesh material at depth `d`: `ambient = 1 + (0.72 − 1)·d`, `diffuse = 0.42·d`,
  `shininess = 48`, `specularColor = round([255,236,200]·d)`.
- `aimKeyLight(toSun, weight)` (G:434-445): `up = min(toSun.z, sin(53°))`;
  `flat = sqrt(1 − up²)/hypot(toSun.x, toSun.y)`; `ray = [−toSun.x·flat, −toSun.y·flat, −up]`;
  direction = normalise(lerp(usualDirection, ray, weight)). Weight used:
  `buildings && flat ? sunlight(toSun.z)·clamp(zoom − 12, 0, 1) : 0` (G:2724).

### 9.2 Aircraft labels (G:754-969)

- Sizes: callsign 11 px, height line 10 px (× `labelScale`); colours: callsign
  `[241,238,231,235]`, selected `[55,142,240,255]`; height `[162,172,183,220]`, selected
  `[55,142,240,205]`. Same font/outline/parameters as place labels. Anchor: start (left),
  vertically centred.
- Offset right of the aircraft: `screenPx(size)/2 + 5` px; with the height line the two lines
  sit at `∓6·scale` px vertically.
- Collision group `"aircraft-labels"` with a "crisp" fade: alpha ×
  `smoothstep(0.3, 0.65, collision_fade)`. Each label claims an invisible background box:
  with `half = 11·scale·0.5`, `line = withHeight ? 6·scale : 0`,
  `spread = 1 + 0.6·clamp((7 − zoom)/3, 0, 1)`, `padY = (half + 2·line + 4·scale)·spread`,
  `reserve = 60·scale·spread`: padding `[left, top, right, bottom] =
  [gapFor(largest) + reserve, padY, 4 + reserve − 60·scale, padY]`.
- Priority: selected aircraft `SELECTED_LABEL_PRIORITY`, others
  `labelPriority(size, alt, onGround)` (`aircraft-labels.ts`).
- Label set is laid out once per (feed, withHeight, membership) and only positions update per
  frame.

### 9.3 Buildings (G:971-1092)

`BUILDING_COLOR = [58,68,84]`; material ambient 0.42, diffuse 0.6, shininess 16, specular
`[60,66,80]`. Shadow sun recomputed only after moving 0.25°. `CITY_LOD = {levels:[13,14,16],
bias 0.5, maxDistance 25 000 m}`; `GROUND_LOD = {levels:[9..14,16], bias 0.5, maxDistance
80 000 m}`; `CITY_CLOSE_ZOOM = 14`; sources: OpenFreeMap `building` at z13–14, plus Streets GL
`buildings` at z16 on realistic; `maxRequests 8`. Realistic extras: shadow colour `[6,12,28]`,
strength `round(sunStrength·1.55·20)/20`, foot shade 0.42; `sunStrength = max(look.sunColor)/3.2`.
`TREES_ZOOM = 14`, `ROAD_WIDTHS_ZOOM = 14`.

### 9.4 Aircraft icon atlases (G:1235-1251)

KTX2 textures loaded per URL, not sRGB, clamp-to-edge, linear min/mag/mip filtering.

### 9.5 Map error card (G:1252-1277, 3799-3808)

First rendering error is captured once; drawing pauses. Card (centred, width
`min(420px, 100% − 2·space-4)`): title "The map couldn't draw", the message in the data font,
buttons "Copy details" (copies message, GPU vendor/renderer/version, user agent, stack) and
"Retry" (clears the error and re-renders; after a lost context, reloads the page). Context
loss message: "The graphics context was lost (WebGL context lost)".

---

## 10. Port notes, risks and open questions

1. **Colour space.** All colours here are authored for blending in gamma space on an 8-bit
   canvas. The night shading (4 stacked alpha layers), radar opacity and label halos will look
   different if blended in linear space. Recommendation: render the map to a non-sRGB target
   (or do the blend maths in gamma space) for visual parity.
2. **Zoom/latitude coupling.** deck's globe couples zoom with latitude in three places
   (scale = `2^zoom/(π cos lat)`, pan adds `log2(cos newLat / cos oldLat)` to zoom, limits are
   shifted by `log2(cos lat)`). Parity requires reproducing all three; "fixing" them changes
   how zoom thresholds feel at high latitudes. Decide deliberately.
3. **Projection switch at zoom 12.** The globe and Mercator views only match at the view centre;
   deck simply swaps. With pitch, or at high latitude, the swap is visible. The port can keep
   the hard swap (parity) or blend. MVT layers currently re-load at the switch (two ids); a
   native tile cache should avoid that.
4. **Globe unproject off-limb** returns the nearest limb point rather than "nothing";
   trackpad pan and pinch anchoring rely on getting *some* coordinate.
5. **Label collision** is GPU/screen-space with fading in deck. A CPU greedy placement gives
   similar but not identical results (deck tests the anchor region, not the full box
   intersection); expect differences in dense areas.
6. **Text rendering.** Outlined SDF text is needed for map labels; egui has no text outline.
   Budget for a dedicated wgpu text pipeline (e.g. an SDF atlas of Overpass 600).
7. **Globe tessellation.** Large lon/lat primitives (land rectangle, night quads, low-zoom
   imagery tiles) must be subdivided (≤ 5°) and imagery must be sampled with Mercator-correct v.
8. **Vector tile line rendering.** Widths go down to 0.6 px. As far as I know deck's PathLayer
   draws plain triangles with no edge smoothing of its own, so today's look comes from the
   WebGL canvas's multisampling (not verified in the deck source). The port should enable
   MSAA ×4 or implement anti-aliased lines, or the 0.6-px borders and roads will shimmer.
9. **Keyboard zoom double-binding.** deck's controller handles `Minus`/`Equal` (×2 zoom, 300 ms)
   and the page also binds `+`/`=`/`-` to `zoomBy(±1)` (250 ms). Whether both fire today (i.e.
   whether the canvas has keyboard focus) was not verified; the port should implement a single
   one-level zoom.
10. **Touch input on Android/iOS.** Today touch goes through deck/mjolnir defaults: one-finger
    pan, two-finger pinch zoom, two-finger drag pan, no twist rotation, and **no touch gesture
    for pitch or bearing** (only the on-screen controls). The WebKit `gesture*` twist path is
    desktop Safari/macOS webview only. Confirm whether mobile should gain twist-to-rotate and
    two-finger-vertical-drag-to-tilt.
11. **No inertia.** Pans stop dead when released (deck `inertia: 0`). Native users may expect
    fling; parity says none.
12. **Linear camera transitions.** `flyTo` etc. use linear interpolation with no easing. Parity
    says keep; it is cheap to improve later.
13. **Esri World Imagery and OpenFreeMap terms / attribution.** No attribution is drawn in this
    component; check where (if anywhere) the app shows it and carry it over. Esri imagery is
    requested without a key.
14. **`resolution: 5`** and the 36×72 earth sphere are deck-specific; any equivalent or finer
    tessellation is fine.
15. The satellite basemap draws no borders/roads/runways at all (only labels). That is current
    behaviour, not an omission in this document.
