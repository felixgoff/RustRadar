# 04 — The realistic basemap: light, weather, ground, buildings, trees, shadows, clouds, LOD tiles, 3D terrain, textures

Scope: everything that makes `basemap === "realistic"` (and the 3D terrain / hillshade shared with the other basemaps).
Source files (all under `src/lib/` unless noted), abbreviations used for `file:line` references:

| Abbrev. | File | What it holds |
|---|---|---|
| `R` | `realistic.ts` | `Look` (light model), shared `rlight` shader module, building / roof / tree / tree-shadow / cloud / haze-fade / foot-shade / terrain-air shader injections, building style picker |
| `RG` | `realistic-ground.ts` | ground surfaces table, feature → paint classification, ground shader, ground tile layers |
| `RT` | `realistic-textures.ts` | texture array layer lists, stand-ins, loading |
| `TL` | `tree-layer.ts` | tree planting per tile, seasonal colours, tree shadows, tile cache |
| `TM` | `tree-models.ts` | the crossed-cards tree mesh |
| `CN` | `canopy.ts` | Meta/WRI canopy height GeoTIFF reader |
| `ST` | `street-trees.ts` | registered town trees (`static/street-trees/*.bin`) |
| `BL` | `building-layer.ts` | building tile layer: heights, per-tile shadows, foot shade, pitched roof meshes |
| `BS` | `building-shadow.ts` | shadow parallelogram mesh |
| `SH` | `shadow.ts` | sun direction, local sun, `sunlight`, aircraft shadow matrix |
| `RF` | `roofs.ts` | pitched roof geometry |
| `SG` | `streets-gl.ts` | Streets GL vector tile schema helpers |
| `OV` | `overture.ts` | Overture building heights (PMTiles) |
| `RW` | `road-widths.ts` | Trafikverket NVDB road widths |
| `LC` | `landcover.ts` | NMD2023 / ESA WorldCover land-cover rasters |
| `LG` | `landcover-ground.ts` | land cover → ground polygons |
| `W` | `weather.ts` | `Conditions`, presets, MET Norway mapping |
| `CL` | `clouds.ts` | EUMETSAT cloud picture, strips |
| `RO` | `RainOverlay.svelte` | 2D rain streaks |
| `LT` | `lod-tileset.ts` | distance LOD tile selection, look-ahead, refinement |
| `TR` | `terrain-layer.ts` | hillshade, terrarium decode, 3D terrain surface, `groundHeight` |
| `K` | `ktx2.ts`, `KT` `ktx2-transcode.ts` | KTX2 → GPU textures |
| `CT` | `scripts/compress-textures.mjs` | encoder settings |
| `G` | `Globe.svelte` | how all of the above is wired (parameters, order, thresholds) |

Conventions in this document:

- **Local frame** = east, north, up (ENU), metres, at the point being shaded.
- **Common space** = deck.gl's Web-Mercator world: 512 units round the world, y **up** (north), z in the same units
  (`commonUnitsPerMeter = 512 / (40 075 016.686 · cos φ)` at latitude φ). On the globe, common space is a sphere and
  its axes relate to Earth-centred axes as (x, y, z)_common = (y, −x, z)_earth (`R:524,539`).
- Colours given as "linear" are linear-light floats; "sRGB" are 0-255 bytes. The pipeline shades in linear light and the
  **shader itself applies exposure and gamma 1/2.2 at the end** (`rl_display`), writing to a non-sRGB (`rgba8unorm`) target.
  In wgpu: render the realistic passes to an `Rgba8Unorm`/`Bgra8Unorm` (non-sRGB) surface view, or strip the gamma and let an sRGB target encode (not bit-identical: pow 2.2 vs the piecewise sRGB curve; keep pow for parity).
- Texture images are stored top row first and uploaded unflipped; shaders negate `v` (`vec3(uv.x, -uv.y, layer)`) and rely on `repeat` addressing.

---

## 1. Frame overview: what the realistic map draws, in order

`G:2636-2720`. `flat` = zoom > 12 (`FLAT_ABOVE_ZOOM = 12`, `G:266`), where the view is Web Mercator; otherwise the globe. `tileUrl` = the OpenFreeMap/OpenMapTiles vector tile template (see doc 01).

| # | Layer id | Condition | Source | GPU params | Extension(s) |
|---|---|---|---|---|---|
| 0 | CSS background of the map container | realistic | `skyCss(look, …)` (`R:189`, `G:3680`) | – | – |
| 1 | `earth` | always | sphere (doc 01) | – | – |
| 2 | `terrain-surface` | `terrain` setting on | Mapterhorn terrarium tiles → mesh (`TR:430`) | `operation: "terrain+draw"` | `SeaLevelExtension`, `TERRAIN_AIR` (realistic only) |
| 3 | `ground-land` | realistic | one polygon ±180, ±85.06, paint `LAND` (`RG:937`) | `SURFACE` | `GROUND_SURFACES` |
| 4 | `ground-land-cover` | realistic && flat | land-cover rasters traced to polygons, tile zoom 10-14 (`LG:98`) | `SURFACE_NO_CULL` | `GROUND_SURFACES` |
| 5a | `ground-city` | realistic && flat | LOD vector tiles: OpenMapTiles z9-14 + Streets GL z16 (`RG:842`) | `SURFACE_NO_CULL` | `GROUND_SURFACES` |
| 5b | `ground-globe` / `ground-flat` | realistic && !flat | OpenMapTiles z0-14 MVT (`RG:792`) | `SURFACE_NO_CULL` | `GROUND_SURFACES` |
| 6 | `ground-rails-flat/globe`, `ground-platforms` | realistic, app (Tauri) only | OpenRailwayMap tiles (`RG:878`) | rails `SURFACE_NO_CULL`; platforms `NO_CULL` (depth-writing, extruded 0.6 m) | `GROUND_SURFACES` |
| 7 | `terrain-hillshade` | realistic: always; other basemaps: when `terrain` | Mapzen terrarium PNG → CPU hillshade (`TR:322`) | `SURFACE` | `HAZE_FADE` (realistic) |
| 8 | `clouds-flat-N` / `clouds-globe-0` | realistic && zoom < 6.5 | EUMETSAT IR picture (`CL`, `G:3619`) | `CLOUD_PARAMETERS` | `CLOUDS` |
| … | rail map, radar, etc. (other docs) | | | | |
| 9 | `buildings-city-realistic` | `buildings` setting && flat | LOD tiles: OpenMapTiles z13,14 + Streets GL z16 (`G:1056`) | `NO_CULL` | `BUILDINGS`; per tile sub-layers below |
| 9.1 | `…-foot` | tile z ≥ 14 | wall outlines as 5 m paths | `SHADOW_PARAMETERS` | `FOOT_SHADE` |
| 9.2 | `…-shadow` | tile z ≥ 14, sun up | shadow parallelograms | `SHADOW_PARAMETERS` | `HAZE_FADE` |
| 9.3 | buildings (extruded polygons) | | | `NO_CULL` | `BUILDINGS` |
| 9.4 | `…-roofs` | Streets GL tile with pitched roofs | roof mesh, metre offsets | `NO_CULL` | `ROOFS` |
| 10 | `trees-z/x/y-shadows`, `-conifers`, `-broadleaves` | realistic && flat && zoom ≥ 14 (`TREES_ZOOM`) | planted per tile (`TL:690`) | shadows `SHADOW_PARAMETERS`; trees default (depth test + write, back-face culling off is not set: default deck params) | `TREE_SHADOWS`; `TREES` |
| 11 | RainOverlay canvas (DOM, z-index 1 above the map) | realistic, raining | `RO` | – | – |

When `terrain` is on, every layer except ids matching `/^(earth$|terrain-surface$|clouds-)/` gets deck.gl's `TerrainExtension` appended (`G:3059-3067`): flat layers are draped (rendered top-down into per-terrain-tile textures and sampled by the terrain mesh), 3D ones (buildings, trees, aircraft…) are offset vertically by the terrain height under them. See §14.

Shared GPU state (`G:325-375`):

```text
NO_CULL            = { cullMode: "none" }
SURFACE            = { depthWriteEnabled: false }          // depth test stays on (default less-equal) so the globe's far side hides it
SURFACE_NO_CULL    = NO_CULL + SURFACE
CLOUD_PARAMETERS   = { cullMode: "none", depthWriteEnabled: false, depthCompare: "less-equal" }   (G:3618)
SHADOW_PARAMETERS  = { cullMode: "none", depthWriteEnabled: false, depthCompare: "less-equal",
                       stencilWriteMask: 0xff, stencilCompare: "equal",
                       stencilPassOperation: "increment-clamp",
                       stencilFailOperation: "keep", stencilDepthFailOperation: "keep" }           (G:366-375)
```

The WebGL context is created with a stencil buffer (`deviceProps: { webgl: { stencil: true } }`, `G:3694`); deck.gl clears it to 0 every frame; the stencil reference value is the default 0. Blending everywhere is deck.gl's default: `src-alpha, one-minus-src-alpha` for colour, `one, one-minus-src-alpha` for alpha.

Per-frame driver (`G:3647-3682`, `onBeforeRender: updateLook`):

```text
sun          = sceneSun(daylight, localSun(sunDirection(now), lat, lon))            // ENU at view centre
weatherHere  = mixConditions(sky ?? PRESETS.sunny, PRESETS.sunny,
                             smoothstep(3000, 10000, cameraAltitude))               // weather only under the clouds
cameraMetres = 1.5 * viewportHeightPx * metersPerPixel(zoom, lat)                    // CAMERA_HEIGHTS = 1.5
cameraAltitude = cameraMetres * cos(pitch)
setLook(lookFor(weatherHere, sun, cameraMetres))
showRain(weatherHere.rain, pitch, look.night)    // quantised: amount to 0.05, pitch to 10°, night to 0.1
background = skyCss(look, top = max(1,pitch) + atan(1/3)° − 90, horizonAt, sunAt, space = clamp01((11.5 − zoom)/2.5))
```

`sky` (a `Conditions` or null) is `PRESETS[settings.sky]`, or for `"live"` `conditionsOf(liveWeather)` (null until fetched → sunny) (`+page.svelte:243`). Live weather is refetched every 10 min, or after the view centre has moved 15 km; after a failure not for 2 min (`+page.svelte:210-225`).

---

## 2. Weather: `Conditions`, presets, live mapping (`W`)

```ts
type Conditions = { sun, cloud, haze /* extinction per metre */, wet, snow, cloudShadows, rain }   // all 0..1 except haze
type Sky = "live" | "sunny" | "cloudy" | "rain" | "fog" | "snow"     // SKIES order is the UI order
```

Presets (`W:44-50`), exact:

| preset | sun | cloud | haze (1/m) | wet | snow | cloudShadows | rain |
|---|---|---|---|---|---|---|---|
| sunny | 1 | 0.05 | 2e-5 | 0 | 0 | 0 | 0 |
| cloudy | 0 | 1 | 7e-5 | 0 | 0 | 0 | 0 |
| rain | 0 | 1 | 1.8e-4 | 1 | 0 | 0 | 1 |
| fog | 0.04 | 1 | 4.5e-3 | 0.35 | 0 | 0 | 0 |
| snow | 0 | 1 | 1.4e-4 | 0 | 1 | 0 | 0 |

`mixConditions(a, b, t)`: per-field linear interpolation with t clamped to 0..1 (`W:55`).

Live weather record (`Weather`, from the Rust backend command `weather_at(lat, lon)`; MET Norway locationforecast/2.0 `complete`, first timeseries entry): `cloud` = cloud_area_fraction/100, `cloudLow` = cloud_area_fraction_low/100, `fog` = fog_area_fraction/100, `precipitation` = next_1_hours precipitation_amount (mm, ≥ 0), `symbol` = next_1_hours symbol_code, `temperature` = air_temperature (default 10) (`W:87-103`).

`conditionsOf(w)` (`W:61-84`):

```text
snowing = symbol matches /snow|sleet/  OR (symbol matches /rain/ AND temperature <= 0)
raining = !snowing AND (symbol matches /rain|thunder/ OR precipitation >= 0.1)
foggy   = max(w.fog, symbol matches /fog/ ? 0.8 : 0)
cover   = max(w.cloud * 0.85, w.cloudLow)
sun     = clamp01((0.92 − cover) / 0.6) * (1 − foggy)
rain    = clamp01(precipitation / 2)
haze    = 2e-5 + 5e-5*w.cloud + (raining ? 6e-5 + 1.2e-4*rain : 0) + (snowing ? 1.2e-4 : 0)
haze    = max(haze, foggy * 4.5e-3)
return { sun,
         cloud: clamp01(max(cover, foggy)),
         haze,
         wet:  raining ? 0.55 + 0.45*rain : foggy*0.35,
         snow: snowing ? 1 : 0,
         cloudShadows: clamp01(4*sun*(1−sun)) * clamp01(w.cloud*2),
         rain: raining ? 0.35 + 0.65*rain : 0 }
```

---

## 3. The light model: `Look` and `lookFor` (`R:33-229`)

### 3.1 Constants (`R:68-98`)

| Name | Value | Meaning |
|---|---|---|
| `SUN` | 3.2 | sun irradiance above the atmosphere, shading units |
| `TAU` | (0.13, 0.19, 0.33) | clear-sky zenith optical depth R,G,B |
| `DAY_ZENITH` | (0.07, 0.17, 0.48) | linear |
| `DAY_HORIZON` | (0.42, 0.55, 0.75) | |
| `DUSK_HORIZON` | (0.75, 0.38, 0.16) | |
| `NIGHT_ZENITH` | (0.0048, 0.008, 0.019) | |
| `NIGHT_HORIZON` | (0.014, 0.018, 0.032) | |
| `OVERCAST_ZENITH` | (0.5, 0.52, 0.56) | |
| `OVERCAST_HORIZON` | (0.62, 0.63, 0.66) | |
| `CITY_GLOW` | (0.012, 0.008, 0.005) | town light off the cloud base at night |
| `CLEAR_DAYLIGHT` | 1.84 | reference luminance for exposure adaptation |
| `CLEAR_AIR` | 2.2e-5 /m | clear air extinction (cap for non-fog haze) |
| `RAIN_MIST` | 0.2 | |
| luminance weights | (0.2126, 0.7152, 0.0722) | Rec. 709 |

Helpers: `smooth(a,b,x)` = smoothstep; `mix(a,b,t)`; `scale(a,k)`; `clamp01`.

### 3.2 Sun transmittance (Kasten & Young air mass), verbatim

`src/lib/realistic.ts:82-89`

```ts
/** The share of each colour of the sun's light that comes through the air at `altDeg`, by Kasten and Young's air mass. */
export function transmittance(altDeg: number, haze: number): RGB {
  const alt = Math.max(altDeg, -1);
  const airMass = 1 / (Math.sin(Math.max(alt, 0) * DEG) + 0.50572 * (alt + 6.07995) ** -1.6364);
  // hazier air dims and reddens it more
  const aerosol = Math.min(1.2, haze * 2500);
  return TAU.map((tau) => Math.exp(-(tau + aerosol * 0.4) * (airMass - 1) * 0.9)) as RGB;
}
```

### 3.3 `lookFor(conditions, sunENU, cameraMetres = 1000)`, verbatim

`src/lib/realistic.ts:108-169`

```ts
export function lookFor(c: Conditions, sun: readonly number[], cameraMetres = 1000): Look {
  const sunUp = Math.max(-1, Math.min(1, sun[2]));
  // from far out each place on the globe has its own day (the shaders light it): the light's
  // colours are the day's, as with the sun well up, whatever the time in the middle of the view
  const globe = smooth(150_000, 600_000, cameraMetres);
  const realAlt = Math.asin(sunUp) / DEG;
  const altDeg = realAlt + (Math.max(realAlt, 35) - realAlt) * globe;
  // daylight from 6° below the horizon (civil twilight) to full from 15° up
  const day = smooth(-6, 15, altDeg);
  const sunColor = scale(transmittance(altDeg, c.haze), SUN * c.sun * smooth(-0.8, 3, altDeg));
  // the clear sky, warming at its horizon around sunset
  const dusk = clamp01(1 - Math.abs(altDeg - 1) / 9) * (1 - c.cloud);
  const clearZenith = mix(NIGHT_ZENITH, DAY_ZENITH, day);
  const clearHorizon = mix(mix(NIGHT_HORIZON, DAY_HORIZON, day), scale(DUSK_HORIZON, Math.max(day, 0.15)), dusk * 0.75);
  // an overcast sky: grey, darker in rain, as bright as the day; at night lit a little by the towns below
  const dim = (0.12 + 0.88 * day) * (1 - 0.3 * c.wet) * day;
  const glow = scale(CITY_GLOW, 1 - day);
  const greyZenith = scale(OVERCAST_ZENITH, dim).map((v, i) => v + glow[i] * 0.5) as RGB;
  const greyHorizon = scale(OVERCAST_HORIZON, dim).map((v, i) => v + glow[i]) as RGB;
  const skyZenith = mix(clearZenith, greyZenith, c.cloud);
  const skyHorizon = mix(clearHorizon, greyHorizon, c.cloud);
  const skyLight = mix(skyHorizon, skyZenith, 0.5);
  const night = 1 - smooth(-7, 1, altDeg);
  // Streets GL's light from the sky: grey and the same from every side, a fifth of the sun's
  // on a clear day; under cloud the sky gives what the sun doesn't; at night a little, so the
  // streets are dim but there to see, not black
  const overcast = luminance(mix(OVERCAST_ZENITH, OVERCAST_HORIZON, 0.5)) * (1 - 0.3 * c.wet);
  const ambient = (0.2 + (overcast * 1.1 - 0.2) * c.cloud) * (0.15 + 0.85 * day) + 0.08 * night;
  // the ground throws back about a fifth of what falls on it
  const groundColor = skyLight.map((s, i) => 0.2 * (s + sunColor[i] * Math.max(sunUp, 0))) as RGB;
  // fog is the air itself lit: whiter than a hazy sky
  const fog = clamp01((c.haze - 3e-4) / 2e-3);
  const fogColour = scale([0.7, 0.71, 0.72], Math.max(day, 0.02));
  const hazeColor = mix(skyHorizon, mix(skyLight, fogColour, 0.5), fog);
  const camera = Math.max(cameraMetres, 50);
  // falling rain: a grey veil from some way out, thickening with the distance (lighter than fog)
  const rainMist = c.rain * (1 - fog);
  // a clear day as Streets GL shows it; a grey one taken a little brighter, as a camera would (more so at night)
  const daylight = luminance(sunColor) * Math.max(sunUp, 0) + ambient;
  const adapt = Math.min(1.8, Math.max(1, Math.sqrt(CLEAR_DAYLIGHT / Math.max(daylight, 1e-3))));
  return {
    sunDir: [sun[0], sun[1], sunUp],
    sunUp,
    sunColor,
    skyZenith,
    skyHorizon,
    groundColor,
    ambient,
    hazeColor,
    hazeDensity: Math.max(
      Math.min(Math.min(c.haze, CLEAR_AIR) * (1 - fog) + c.haze * fog, (0.22 + 0.64 * fog) / camera),
      (rainMist * RAIN_MIST) / camera,
    ),
    hazeStart: Math.max(fog * 0.5, rainMist * 0.3) * camera,
    wet: c.wet,
    snow: c.snow,
    cloudShadow: c.cloudShadows * 0.85,
    night,
    exposure: adapt * (1 + 2.2 * (1 - day)),
    globe,
  };
}
```

Notes for the port:

- `globe` (0 below 150 km camera distance, 1 above 600 km) lifts the *effective* sun altitude to at least 35° so that far out the uniform colours are "daytime"; per-fragment day/night is then done in the shader from `rl_sun.z` (§4).
- Haze density is capped so the air never eats more than `1 − exp(−0.22)` ≈ 20 % at the view centre in clear/overcast weather and `1 − exp(−0.86)` ≈ 58 % in full fog (`(0.22 + 0.64·fog)/camera`), with fog starting half-way (`hazeStart = 0.5·fog·camera`). Rain adds a floor `0.2·rainMist/camera` starting at `0.3·rainMist·camera`.
- `exposure = adapt · (1 + 2.2·(1 − day))`, `adapt = clamp(sqrt(1.84 / daylight), 1, 1.8)`.
- `cloudShadow = conditions.cloudShadows · 0.85`.
- Initial `look` before the first frame: `lookFor(sunny, [0.4, −0.55, 0.73])` (`R:172`).

### 3.4 The scene sun (`R:214-218`, `SH:15-41`)

- With the *daylight* setting on: the real sun, `localSun(sunDirection(now), lat, lon)` (ENU at the view centre).
- Off: a fixed afternoon sun, azimuth 215° (clockwise from north), altitude 35°: `[sin(az)cos(alt), cos(az)cos(alt), sin(alt)]` = (−0.4698, −0.6710, 0.5736).
- `sunDirection(t)` = unit vector from the Earth's centre to the subsolar point: `[cosφ cosλ, cosφ sinλ, sinφ]` (x through 0°E on the equator, z north).
- `localSun(sun, lat, lon)`: `e = −sinλ·x + cosλ·y; n = −sinφ cosλ·x − sinφ sinλ·y + cosφ·z; u = cosφ cosλ·x + cosφ sinλ·y + sinφ·z`.
- `fromLocal(enu, lat, lon)` is the inverse (`R:221-229`).

### 3.5 Display transform for CPU-side colours

`displayRgb(linear, exposure)` = `round(255 · clamp01(v·exposure)^(1/2.2))` per channel (`R:179`). Used for the sky gradient so it matches the shaders exactly.

### 3.6 The sky behind the map (`skyCss`), verbatim

`src/lib/realistic.ts:189-207`

```ts
export function skyCss(l: Look, top: number, horizon: number, sunAt: [number, number] | null, space: number): string {
  const elevationColour = (deg: number) => mix(l.skyHorizon, l.skyZenith, Math.sqrt(clamp01(Math.sin(deg * DEG))));
  const css = (c: readonly number[]) => `rgb(${c.join(" ")})`;
  const atTop = mix(displayRgb(elevationColour(Math.max(top, 0)), l.exposure), [4, 7, 16], space).map(Math.round);
  const atHorizon = mix(displayRgb(l.hazeColor, l.exposure), [12, 18, 34], space).map(Math.round);
  const at = Math.round(clamp01(horizon) * 100);
  const layers = [`linear-gradient(to bottom, ${css(atTop)} 0%, ${css(atHorizon)} ${at}%, ${css(atHorizon)} 100%)`];
  const sun = Math.max(...l.sunColor);
  if (sunAt && sun > 0.05 && space < 1) {
    const glow = displayRgb(l.sunColor.map((v, i) => v * 0.5 + l.skyHorizon[i]), l.exposure);
    const alpha = Math.min(0.85, sun / SUN) * (1 - space);
    const [x, y] = [Math.round(sunAt[0]), Math.round(sunAt[1])];
    layers.unshift(
      `radial-gradient(circle at ${x}px ${y}px, rgb(255 252 240 / ${alpha.toFixed(2)}) 0px, rgb(255 250 235 / ${alpha.toFixed(2)}) 14px, ` +
        `rgb(${glow.join(" ")} / ${(alpha * 0.55).toFixed(2)}) 40px, rgb(${glow.join(" ")} / 0) 320px)`,
    );
  }
  return layers.join(", ");
}
```

Inputs from `G:3661-3681`:

```text
halfFov   = atan(1/3) in degrees (≈ 18.43°; deck.gl's camera sees 2·atan(1/3) ≈ 36.87° top to bottom)
tilt      = max(1, pitch)
top       = tilt + halfFov − 90                 // elevation (deg) of the top edge of the screen; clamped ≥ 0 inside
horizonAt = (1 − 3 / tan(tilt)) / 2             // 0 = top, 1 = bottom; clamped to 0..1 inside (so pitch ≤ ~71.6° ⇒ horizon above the screen ⇒ 0)
space     = clamp01((11.5 − zoom) / 2.5)        // 0 from zoom 11.5 in, 1 at zoom ≤ 9: space colours
sunAt     : only when zoom > 12 and sun.z > −0.02:
            forward = [sinB·sinP, cosB·sinP, −cosP]; right = [cosB, −sinB, 0]; up = [sinB·cosP, cosB·cosP, sinP]   (B bearing, P pitch)
            ahead = forward·sun; if ahead > 0.05:
              f = 3 · (height/2);  sunAt = [width/2 + (right·sun/ahead)·f,  height/2 − (up·sun/ahead)·f]    (CSS px)
```

Rendering to reproduce in wgpu (a full-screen pass drawn first, or a cleared background + quad):

1. Vertical linear gradient: colour `atTop` at y = 0 → `atHorizon` at y = `horizon·100 %`, constant `atHorizon` below.
   `atTop = mix(displayRgb(mix(skyHorizon, skyZenith, sqrt(clamp01(sin(max(top,0)))))), [4,7,16], space)`; `atHorizon = mix(displayRgb(hazeColor), [12,18,34], space)` (both rounded to bytes; interpolation is in sRGB byte space as CSS does).
2. Sun glow (only if `sunAt` given, `max(sunColor) > 0.05`, `space < 1`): radial gradient centred at `sunAt`, `alpha = min(0.85, max(sunColor)/3.2)·(1 − space)`: rgba(255,252,240,alpha) at r = 0 → rgba(255,250,235,alpha) at 14 px → `glow` with alpha·0.55 at 40 px → `glow` with alpha 0 at 320 px; `glow = displayRgb(sunColor·0.5 + skyHorizon)`. Composited over the linear gradient with normal alpha blending.

---

## 4. The shared light shader module `rlight` (`R:231-557`)

Every realistic layer's shader includes this module. It supplies: a uniform block, four varyings computed in the vertex shader, and a library of fragment functions.

### 4.1 Uniform block (std140), verbatim

`src/lib/realistic.ts:238-267`

```glsl
const rlightBlock = /* glsl */ `\
layout(std140) uniform rlightUniforms {
  vec3 sunDir;
  float sunUp;
  vec3 sunColor;
  float ambient;
  vec3 skyZenith;
  float exposure;
  vec3 skyHorizon;
  float hazeDensity;
  vec3 hazeColor;
  float wet;
  vec3 groundColor;
  float snow;
  vec3 sunGlobe;
  float hazeStart;
  vec2 origin;
  float period;
  float periodMetres;
  vec2 cloudOrigin;
  float cloudPeriod;
  float cloudShadow;
  vec2 anchor;
  float night;
  float time;
  float globe;
  float cover;
  vec3 eye;
} rlight;
`;
```

The same layout works as a WGSL struct (each `vec3<f32>` followed by an `f32` packs to 16 bytes; `vec2 + f32 + f32` to 16; the trailing `eye: vec3<f32>` needs 4 bytes of padding ⇒ `globe` at offset 160, `cover` 164, `eye` 176, total size 192 bytes).

Where every value comes from (`rlightProps`, verbatim):

`src/lib/realistic.ts:493-554`

```ts
/** A place on web mercator's map, as deck.gl's flat map has it (512 units round the world). */
function mercator(lng: number, lat: number): [number, number] {
  const phi = Math.max(-89.9, Math.min(89.9, lat)) * DEG;
  return [(lng + 180) * MERCATOR_PER_DEGREE, (Math.PI + Math.log(Math.tan(Math.PI / 4 + phi / 2))) * (512 / (2 * Math.PI))];
}

/**
 * The light's props for a layer: the look, and where the layer is, for what
 * lies fixed to the world (cut to the textures' and the clouds' periods, so
 * the numbers stay small enough for the GPU's precision).
 */
function rlightProps(params: DrawParams) {
  const projectProps = params.shaderModuleProps.project;
  const viewport = projectProps.viewport;
  const [lng, lat] = [Math.fround(viewport.longitude ?? 0), Math.fround(viewport.latitude ?? 0)];
  let at: [number, number];
  // deck.gl drawing what is draped on the 3D terrain, from straight above (its own orthographic view)
  const cover = Boolean(viewport.orthographic);
  let eye = [0, 0, 0];
  if (viewport.resolution) {
    // the globe: from the view's centre (the shader measures from it)
    at = mercator(lng, lat);
  } else {
    // the flat map: from deck.gl's origin for the layer
    const { commonOrigin } = project.getUniforms(projectProps as never) as { commonOrigin: number[] };
    at = [commonOrigin[0], commonOrigin[1]];
    if (cover) {
      const p = viewport.projectPosition(mapEye);
      eye = [p[0] - commonOrigin[0], p[1] - commonOrigin[1], p[2] - (commonOrigin[2] ?? 0)];
    }
  }
  // towards the sun in the globe's common space (x, y and z are the Earth's y, −x and z)
  const sun = fromLocal(look.sunDir, lat, lng);
  return {
    sunDir: look.sunDir,
    sunUp: look.sunUp,
    sunColor: look.sunColor,
    ambient: look.ambient,
    skyZenith: look.skyZenith,
    exposure: look.exposure,
    skyHorizon: look.skyHorizon,
    hazeDensity: look.hazeDensity,
    hazeColor: look.hazeColor,
    wet: look.wet,
    groundColor: look.groundColor,
    snow: look.snow,
    sunGlobe: [sun[1], -sun[0], sun[2]],
    hazeStart: look.hazeStart,
    origin: [mod(at[0], PERIOD), mod(at[1], PERIOD)],
    period: PERIOD,
    periodMetres: (PERIOD * 40_075_016.686 * Math.cos(lat * DEG)) / 512,
    cloudOrigin: [mod(at[0], CLOUD_PERIOD), mod(at[1], CLOUD_PERIOD)],
    cloudPeriod: CLOUD_PERIOD,
    cloudShadow: look.cloudShadow,
    anchor: [lng, lat],
    night: look.night,
    time: (performance.now() / 1000) % 100_000,
    globe: look.globe,
    cover: cover ? 1 : 0,
    eye,
  };
}
```

Resolved meanings:

| Uniform | Value |
|---|---|
| `sunDir` | ENU sun at the view centre (flat map uses this directly for every fragment) |
| `sunGlobe` | the same sun as a direction in the globe's common space: `fromLocal(sunDir, lat, lon)` → (y, −x, z) |
| `sunUp`, `sunColor`, `ambient`, `skyZenith`, `skyHorizon`, `groundColor`, `hazeColor`, `hazeDensity`, `hazeStart`, `wet`, `snow`, `cloudShadow`, `night`, `exposure`, `globe` | straight from `look` |
| `period` | `PERIOD = 1/64` common unit (≈ 1223 m · cos φ: "about 600 m at 60°N") |
| `periodMetres` | `PERIOD · 40 075 016.686 · cos(lat) / 512` (lat = view centre) |
| `origin` | where the layer's coordinate origin sits within the texture period: `mod(at, PERIOD)`, with `at` = the layer's common-space origin on the flat map (deck's `commonOrigin`, i.e. what vertex positions are relative to), or the Web-Mercator common position of the view centre on the globe |
| `cloudPeriod` | `CLOUD_PERIOD = 1/4` common unit (≈ 19.6 km · cos φ) |
| `cloudOrigin` | `mod(at, CLOUD_PERIOD)` |
| `anchor` | view centre (lng, lat) in degrees, as float32 (`Math.fround`) |
| `time` | seconds, `(performance.now()/1000) mod 100 000` |
| `cover` | 1 when this draw is deck.gl's top-down orthographic "drape onto terrain" pass, else 0 |
| `eye` | in the cover pass on the flat map: the real map camera's common position minus `commonOrigin` (so lighting is still computed for the real camera); else 0 |

`mercator(lng, lat)` = `[(lng+180)·512/360, (π + ln tan(π/4 + φ/2))·512/(2π)]`, φ clamped to ±89.9° (`R:494`).

**Rust equivalent of `origin`**: each tile/mesh is drawn relative to some double-precision origin `O` (common units). Pass `origin = (O.x mod PERIOD, O.y mod PERIOD)` computed in f64; the shader adds the f32 local offset. This keeps texture coordinates precise at any zoom.

### 4.2 Varyings and vertex part, verbatim

`src/lib/realistic.ts:269-287`

```glsl
const rlightVaryings = (dir: "in" | "out") => /* glsl */ `\
${dir} vec3 rl_eye;
${dir} vec3 rl_sun;
${dir} vec2 rl_t;
${dir} vec2 rl_c;
`;

const rlightVs = /* glsl */ `\
${rlightBlock}
${rlightVaryings("out")}
// east, north and up at a point of deck.gl's globe (its common space there);
// the flat map's common space is east, north and up already
mat3 rl_frame(vec3 p, bool globe) {
  if (!globe) return mat3(1.0);
  vec3 up = normalize(p);
  vec3 east = normalize(vec3(-up.y, up.x, 0.0) + vec3(1e-9, 0.0, 0.0));
  return mat3(east, cross(up, east), up);
}
`;
```

Injected at the end of every vertex shader (`vs:DECKGL_FILTER_GL_POSITION`, after deck has computed `geometry.position` = common-space position (relative to the layer origin on the flat map), `geometry.worldPosition` = lng/lat/z):

`src/lib/realistic.ts:417-442`

```glsl
    "vs:DECKGL_FILTER_GL_POSITION": /* glsl */ `
  {
    bool rl_globe = project.projectionMode == PROJECTION_MODE_GLOBE;
    mat3 rl_enu = rl_frame(geometry.position.xyz, rl_globe);
    // to the camera and to the sun from here, as metres east, north and up
    // (draped on the 3D terrain, it is drawn from above: still seen from the map's own camera)
    vec3 rl_cam = rlight.cover > 0.5 ? rlight.eye : project.cameraPosition;
    rl_eye = ((rl_cam - geometry.position.xyz) / project.commonUnitsPerMeter.x) * rl_enu;
    rl_sun = rl_globe ? rlight.sunGlobe * rl_enu : rlight.sunDir;
    // where it is on the flat map, for what lies fixed to the world (textures, the clouds' shadows):
    // on the globe, web mercator's units from the view's centre, so they fit the flat map's
    vec2 rl_m = geometry.position.xy;
    if (rl_globe) {
      vec2 rl_deg = geometry.worldPosition.xy - rlight.anchor;
      float rl_cos = max(cos(radians(rlight.anchor.y + 0.5 * rl_deg.y)), 0.01);
      rl_m = vec2(rl_deg.x, rl_deg.y / rl_cos) * ${MERCATOR_PER_DEGREE.toFixed(10)};
    #ifdef RL_LINE
      // out across a line's width from its middle
      vec3 rl_side = ((geometry.position.xyz - project_position(geometry.worldPosition.xyz)) / project.commonUnitsPerMeter.x) * rl_enu;
      rl_m += rl_side.xy * (${MERCATOR_PER_METRE.toExponential(10)} / rl_cos);
    #endif
    }
    rl_t = (rlight.origin + rl_m) / rlight.period;
    rl_c = (rlight.cloudOrigin + rl_m) / rlight.cloudPeriod;
  }
    `,
```

with `${MERCATOR_PER_DEGREE.toFixed(10)}` = `1.4222222222` and `${MERCATOR_PER_METRE.toExponential(10)}` = `1.2776039596e-5`.

In plain terms, for the port:

- `rl_eye` (vec3, metres, ENU) = vector **from the fragment to the camera**. Flat map: `(cameraCommon − positionCommon) / commonUnitsPerMeter.x`. (`normalize(rl_eye)` is the view vector `v`; `length(rl_eye)` is the distance in metres.)
- `rl_sun` (vec3, ENU) = direction to the sun *at this vertex*: constant `sunDir` on the flat map; on the globe `sunGlobe` rotated into the vertex's own ENU frame, so `rl_sun.z` is the sine of the local sun altitude (per-place day/night).
- `rl_t` (vec2) = world-fixed texture coordinate in units of `PERIOD`: `(origin + position.xy) / period`. One unit = one period ≈ 1223·cosφ m. x east, y north.
- `rl_c` (vec2) = same in units of `CLOUD_PERIOD`.
- On the globe, `rl_m` is a locally-flattened Mercator offset from the view centre so the textures line up with the flat map at the zoom-12 switch; with `RL_LINE` defined (path layers) the extrusion offset across the line's width is added too.

### 4.3 Fragment library, verbatim

`src/lib/realistic.ts:289-405`

```glsl
const rlightFs = /* glsl */ `\
${rlightBlock}
${rlightVaryings("in")}

#define RL_PI 3.14159265

vec3 rl_linear(vec3 c) {
  return pow(max(c, vec3(0.0)), vec3(2.2));
}
// the screen's gamma, as Streets GL shows its colours: no tone curve to dull them
vec3 rl_display(vec3 c) {
  return pow(clamp(c * rlight.exposure, 0.0, 1.0), vec3(1.0 / 2.2));
}
// value noise on a lattice \`n\` cells to the period, repeating with it
float rl_hash(vec2 cell, float n) {
  cell = mod(cell, n);
  return fract(sin(dot(cell, vec2(127.1, 311.7))) * 43758.5453);
}
float rl_noise(vec2 t, float n) {
  vec2 p = t * n;
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(rl_hash(i, n), rl_hash(i + vec2(1.0, 0.0), n), f.x),
             mix(rl_hash(i + vec2(0.0, 1.0), n), rl_hash(i + vec2(1.0, 1.0), n), f.x), f.y);
}
// how many cells of a lattice in texture coordinates \`t\` one pixel spans (more than one: too fine to show)
float rl_cells(vec2 t) {
  vec2 d = fwidth(t);
  return max(d.x, d.y);
}
// towards the sun from here
vec3 rl_toSun() {
  return normalize(rl_sun);
}
// the sun's light here: none where the globe has turned away from it (from far out, dimming
// and reddening through the evening as its light comes through more air)
vec3 rl_sunLight() {
  float up = rl_sun.z;
  float far = rlight.globe;
  vec3 dusk = mix(vec3(1.0), vec3(1.0, 0.62, 0.38), far * (1.0 - smoothstep(0.0, 0.3, up)));
  return rlight.sunColor * dusk * smoothstep(mix(-0.02, -0.03, far), mix(0.02, 0.22, far), up);
}
// how much of the day's sky light there is here: all of it, but from far over the globe only where the
// sun is up here, fading through twilight to a little at night
float rl_dayHere() {
  return mix(1.0, mix(0.015, 1.0, smoothstep(-0.12, 0.12, rl_sun.z)), rlight.globe);
}
// the sky's light from a direction, with the sun's glow round it; below the horizon, the ground's
vec3 rl_sky(vec3 dir) {
  float up = clamp(dir.z, -1.0, 1.0);
  vec3 sky = mix(rlight.skyHorizon, rlight.skyZenith, sqrt(max(up, 0.0)));
  float toSun = max(dot(dir, rl_toSun()), 0.0);
  sky += rl_sunLight() * (0.04 * pow(toSun, 6.0) + 0.5 * pow(toSun, 900.0));
  return mix(sky, rlight.groundColor, smoothstep(0.0, -0.2, up)) * rl_dayHere();
}
// all the light a surface facing \`n\` gets from the sky: grey, as Streets GL has it (a little less facing down)
vec3 rl_irradiance(vec3 n) {
  return vec3(rlight.ambient * rl_dayHere()) * mix(0.6, 1.0, clamp(0.5 + 0.5 * n.z, 0.0, 1.0));
}
// the split-sum environment BRDF (Karis's fit for mobile, as Streets GL has it)
vec2 rl_envBRDF(float NdotV, float roughness) {
  const vec4 c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
  const vec4 c1 = vec4(1.0, 0.0425, 1.04, -0.04);
  vec4 r = roughness * c0 + c1;
  float a004 = min(r.x * r.x, exp2(-9.28 * NdotV)) * r.x + r.y;
  return vec2(-1.04, 1.04) * a004 + r.zw;
}
// how much of the sun's own reflection a surface shows (less where it is broken up too finely to draw)
float rl_glint = 1.0;
// a surface lit by the sun (where \`sunVisible\`) and the sky: GGX, Smith-Schlick, Schlick's Fresnel
vec3 rl_shade(vec3 albedo, vec3 n, vec3 v, float roughness, float metalness, float ao, float sunVisible) {
  vec3 l = rl_toSun();
  roughness = clamp(roughness, 0.04, 1.0);
  float NdotL = max(dot(n, l), 0.0);
  float NdotV = max(dot(n, v), 1e-3);
  vec3 h = normalize(l + v);
  float NdotH = max(dot(n, h), 0.0);
  float VdotH = max(dot(v, h), 0.0);
  vec3 f0 = mix(vec3(0.04), albedo, metalness);
  vec3 F = f0 + (1.0 - f0) * pow(1.0 - VdotH, 5.0);
  float a = roughness * roughness;
  float a2 = a * a;
  float d = NdotH * NdotH * (a2 - 1.0) + 1.0;
  float D = a2 / (RL_PI * d * d);
  float k = a * 0.5;
  float vis = 0.25 / max((NdotL * (1.0 - k) + k) * (NdotV * (1.0 - k) + k), 1e-4);
  vec3 diffuse = albedo * (1.0 - metalness);
  vec3 direct = ((1.0 - F) * diffuse + F * min(D * vis * RL_PI, 60.0) * rl_glint) * rl_sunLight() * NdotL * sunVisible;
  vec2 brdf = rl_envBRDF(NdotV, roughness);
  vec3 r = reflect(-v, n);
  vec3 env = mix(rl_sky(r), rl_irradiance(r) / max(rlight.ambient, 1e-3), roughness);
  vec3 specular = env * (f0 * brdf.x + brdf.y) * (1.0 - 0.5 * roughness);
  return direct + (diffuse * rl_irradiance(n) + specular) * ao;
}
// how much of what is here gets through the air to the camera (fog only from some way out, so the map stays readable)
float rl_clear() {
  // (draped on the 3D terrain: its surface takes the air, as it is drawn)
  if (rlight.cover > 0.5) return 1.0;
  return exp(-max(length(rl_eye) - rlight.hazeStart, 0.0) * rlight.hazeDensity);
}
// the air between here and the camera: dimming and adding its own light (brighter towards the sun)
vec3 rl_aerial(vec3 c) {
  float t = rl_clear();
  vec3 toHere = -rl_eye / max(length(rl_eye), 1e-3);
  float forward = pow(max(dot(toHere, rl_toSun()), 0.0), 8.0);
  vec3 air = rlight.hazeColor * rl_dayHere() + rl_sunLight() * 0.06 * forward;
  return c * t + air * (1.0 - t);
}
// how much of the sun the clouds let through here, drifting with the wind
float rl_sunVisible() {
  if (rlight.cloudShadow <= 0.0) return 1.0;
  vec2 c = rl_c + vec2(rlight.time * 0.0006, rlight.time * 0.00025);
  float n = 0.55 * rl_noise(c, 4.0) + 0.3 * rl_noise(c, 9.0) + 0.15 * rl_noise(c, 23.0);
  return 1.0 - rlight.cloudShadow * smoothstep(0.46, 0.6, n);
}
`;
```

Summary of the model (for a WGSL rewrite):

- **Colour in**: `rl_linear(c) = pow(max(c,0), 2.2)` for sRGB tints carried in vertex colours. Texture colour arrays are sRGB formats, so sampling returns linear.
- **Colour out**: `rl_display(c) = pow(clamp(c·exposure, 0, 1), 1/2.2)`. No tone curve.
- **Value noise** `rl_noise(t, n)`: lattice of `n` cells per period, hash `fract(sin(dot(mod(cell,n), (127.1, 311.7))) · 43758.5453)`, smoothstep-interpolated; tiles with period 1 in `t`.
- **`rl_cells(t)`** = `max(fwidth(t).x, fwidth(t).y)`: lattice cells per pixel; used for all distance fades.
- **Sun at fragment** `rl_sunLight()`: `sunColor · dusk · smoothstep(mix(−0.02,−0.03,globe), mix(0.02,0.22,globe), rl_sun.z)`, `dusk = mix(1, (1, 0.62, 0.38), globe·(1 − smoothstep(0, 0.3, rl_sun.z)))`.
- **`rl_dayHere()`** = `mix(1, mix(0.015, 1, smoothstep(−0.12, 0.12, rl_sun.z)), globe)`.
- **Sky radiance** `rl_sky(dir)`: `mix(skyHorizon, skyZenith, sqrt(max(dir.z,0)))` + sun glow `sunLight·(0.04·toSun^6 + 0.5·toSun^900)`; below the horizon blends to `groundColor` over dir.z 0 → −0.2; times `rl_dayHere()`.
- **Irradiance** `rl_irradiance(n)` = grey `ambient · dayHere · mix(0.6, 1, clamp(0.5 + 0.5·n.z))`.
- **BRDF** `rl_shade(albedo, n, v, roughness, metalness, ao, sunVisible)`: GGX D, Smith-Schlick visibility with `k = a/2`, Schlick Fresnel with `f0 = mix(0.04, albedo, metalness)`; direct specular clamped `min(D·vis·π, 60) · rl_glint`; ambient specular = `env · (f0·brdf.x + brdf.y) · (1 − 0.5·roughness)` where `env = mix(rl_sky(reflect(−v,n)), rl_irradiance(r)/ambient, roughness)` and `brdf` is Karis's mobile env-BRDF fit; diffuse ambient = `albedo·(1−metal)·rl_irradiance(n)`; ambient terms times `ao`. Roughness clamped to [0.04, 1].
- **`rl_glint`** is a *mutable global* (default 1) the water branch lowers before calling `rl_shade`. In WGSL make it a parameter.
- **Aerial perspective** `rl_aerial(c)`: `t = exp(−max(dist − hazeStart, 0)·hazeDensity)` (1 when `cover > 0.5`), `air = hazeColor·dayHere + sunLight·0.06·pow(max(dot(viewRayDir, toSun),0), 8)`, result `c·t + air·(1−t)`.
- **Cloud shadows** `rl_sunVisible()`: 1 if `cloudShadow ≤ 0`; else three octaves of noise on `rl_c + time·(0.0006, 0.00025)` (lattices 4, 9, 23 per cloud period, weights 0.55/0.3/0.15), `1 − cloudShadow·smoothstep(0.46, 0.6, n)`.

---

## 5. Textures (`RT`, `K`, `KT`, `CT`)

### 5.1 Files in `static/realistic/` (served at `/realistic/…`)

Measured from the KTX2 headers in the repo:

| File | Kind | Size | Layers | Mips | Encoding | sRGB | Sampler |
|---|---|---|---|---|---|---|---|
| `building.color.ktx2` | 2D array | 512×512 | 16 | 10 | UASTC + Zstd | yes | repeat/repeat |
| `building.normal.ktx2` | 2D array | 512×512 | 16 | 10 | UASTC + Zstd (normal-map mode, mips renormalised) | no | repeat |
| `building.mask.ktx2` | 2D array | 512×512 | 16 | 10 | UASTC + Zstd | no | repeat |
| `building.glow.ktx2` | 2D array | 128×128 | 16 | 8 | UASTC + Zstd | no | repeat |
| `ground.color.ktx2` | 2D array | 512×512 | 23 | 10 | UASTC + Zstd | yes | repeat |
| `ground.normal.ktx2` | 2D array | 512×512 | 23 | 10 | UASTC + Zstd | no | repeat |
| `tree.color.ktx2` | 2D array | 512×512 | 5 | 10 | UASTC + Zstd | yes | repeat |
| `tree.normal.ktx2` | 2D array | 512×512 | 5 | 10 | UASTC + Zstd | no | repeat |
| `tree.mask.ktx2` | 2D array | 512×512 | 5 | 10 | UASTC + Zstd | no | repeat |
| `water.normal.ktx2` | 2D | 256×256 | – | 9 | UASTC + Zstd | no | repeat |
| `tree-volume.normal.ktx2` | 2D | 512×512 | – | 10 | UASTC + Zstd | no | clamp-to-edge both |
| `earth.color.ktx2` | 2D | 4096×2048 | – | 13 | ETC1S (BasisLZ), quality 255, compression 2 | yes | repeat U, clamp V |
| `earth.night.ktx2` | 2D | 4096×2048 | – | 13 | ETC1S (BasisLZ) | yes | repeat U, clamp V |

(KTX2 header: `vkFormat = 0` (Basis Universal payload), supercompression scheme 2 = Zstandard for UASTC files, 1 = BasisLZ for ETC1S.) Also `static/realistic/LICENSE-streets-gl.txt` (MIT, © StrandedKitty) must ship with the app. Aircraft icon atlases (`static/aircraft/icons.ktx2`, `icons-outline.ktx2`) are made by the same script (kind `data`: UASTC, not sRGB) — see the aircraft document.

Encoder settings (`CT:43-93`): `color` = UASTC, perceptual + sRGB mips + sRGB transfer function; `normal` = UASTC, `setNormalMap()`, `setMipRenormalize(true)`; `data` = UASTC linear; `earth` = ETC1S sRGB, quality 255, compression level 2. All: full mip chains down to 1 px (`setMipGen(true)`, `setMipSmallestDimension(1)`); UASTC pack flags 2, UASTC Zstd supercompression on. Source images are `textures/realistic/<name>.<kind>.webp`, each array = one file with layers in the list order below.

Common sampler (`RT:91-98`): address repeat U and V, mag linear, min linear, mipmap linear (trilinear), max anisotropy 8.

### 5.2 Array layer orders (the shaders index by these)

**Buildings** (`BUILDING_TEXTURES`, `RT:18-35`) — 4 arrays: `color` (sRGB), `normal`, `mask` (R = roughness, G = metalness, B = how much the building's own colour tints the texel), `glow` (R = window emission):

| Layer | Name | | Layer | Name |
|---|---|---|---|---|
| 0 | plaster-wall | | 8 | glass |
| 1 | plaster-window | | 9 | roof-tar |
| 2 | brick-wall | | 10 | roof-concrete |
| 3 | brick-window | | 11 | roof-metal |
| 4 | block-wall | | 12 | roof-tiles |
| 5 | block-window | | 13 | roof-eternit |
| 6 | wood-wall | | 14 | roof-generic1 |
| 7 | wood-window | | 15 | roof-generic3 |

So wall layer = `facade·2 + (hasBays ? 1 : 0)` for façade 0-3, 8 for glass (façade 4); roof layer = `9 + roof`.

**Ground** (`GROUND_TEXTURES`, `RT:38-62`) — 2 arrays: `color` (sRGB), `normal`:

`0 grass, 1 park, 2 forest, 3 farmland0, 4 farmland1, 5 farmland2, 6 garden, 7 sand, 8 rock, 9 soil, 10 pavement, 11 asphalt, 12 road, 13 road-unmarked, 14 gravel, 15 railway, 16 cobblestone, 17 dirt, 18 pitch, 19 cycleway, 20 wood, 21 tram, 22 terrain`

**Trees** (`TREE_TEXTURES`, `RT:65`) — 3 arrays: `color` (sRGB), `normal`, `mask` (R ≥ 0.5 = inside the tree's silhouette), plus the single 2D `tree-volume.normal` shared by all:

`0 tree-beech, 1 tree-fir, 2 tree-linden0, 3 tree-linden1, 4 tree-oak`

### 5.3 Stand-ins until loaded (`RT:102-177`)

1×1 textures so the map renders immediately: colour (255,255,255,255) sRGB; normal (128,128,255,255); mask (235,0,255,255) = rough, non-metal, fully tinted; glow (0,0,0,255); tree mask (0,0,0,0) ⇒ every tree fragment is discarded until the textures arrive; tree volume = flat normal; water = flat normal; `earth` = (0,0,0,**0**) — alpha 0 means "not loaded, don't use" in the ground shader; `night` = (0,0,0,255). On load all are swapped at once, `ready = true`, and listeners fire (`onTexturesReady`) to redraw. On failure the stand-ins stay.

### 5.4 Loading and transcoding today (`K`, `KT`)

1. `fetch` the file; verify the 12-byte KTX2 magic `AB 4B 54 58 20 32 30 BB 0D 0A 1A 0A` (Tauri returns the HTML page for missing files).
2. Choose target by device features: BC7 if available (`bc7-rgba-unorm[-srgb]`, Basis transcoder format 6) — preferred first because some Linux drivers advertise ASTC but decode it on the CPU; else ASTC 4×4 (`astc-4x4-unorm[-srgb]`, format 10); else RGBA8 (`rgba8unorm[-srgb]`, format 13). ETC2 deliberately unused (a luma.gl bug, not a port concern).
3. Transcode in a pool of 2 web workers with Basis Universal's `KTX2File`: for each mip level, for each layer, `transcodeImage(level, layer, face 0, target)`; a level's data is all layers concatenated.
4. Create the texture (`2d-array` or `2d`, `mipLevels = levels.length`), upload each level, set `TEXTURE_MAX_LEVEL = levels − 1`.

**Port**: wgpu supports all three targets (`Features::TEXTURE_COMPRESSION_BC` → `Bc7RgbaUnorm[Srgb]`; `TEXTURE_COMPRESSION_ASTC` → `Astc { block: B4x4, channel: Unorm|UnormSrgb }`; fallback `Rgba8Unorm[Srgb]`; on Android without ASTC, `TEXTURE_COMPRESSION_ETC2` → `Etc2Rgba8Unorm[Srgb]` is a valid extra target, Basis format 1). Options, in order of preference:

- (a) Keep the `.ktx2` files and transcode at runtime in Rust on a worker thread: parse the container with the `ktx2` crate, inflate levels with `zstd` (UASTC files) and transcode with a Basis Universal binding (`basis-universal` crate's low-level transcoder for UASTC blocks; ETC1S/BasisLZ files need the full KTX2 transcoder — verify the chosen crate exposes KTX2+BasisLZ, otherwise re-encode the two Earth pictures as UASTC or ship them as JPEG/WebP and build mips on the GPU).
- (b) Pre-transcode at build time into per-target KTX2 (BC7 for desktop, ASTC for mobile/Apple) and load with the `ktx2` crate only. Bigger downloads per platform but no transcoder dependency.
- Keep the upload contract: array textures (`D2Array` view) with the file's own mips; colour arrays in sRGB formats; `mip_level_count` exact.

---

## 6. The ground (`RG`)

### 6.1 Encoding: "paint" = RGBA vertex colour

Every ground feature carries an RGBA colour: RGB = sRGB **tint** multiplying the texture (white = unchanged), **alpha = 255 − surface id** (`paint(id, rgb)`, `RG:90`). `NONE = [0,0,0,0]` ⇒ id 255 ⇒ ≥ 42 ⇒ fragment alpha 0 (not drawn). The fragment shader reads `id = floor((1 − a)·255 + 0.5)`. In the port, pass the id as a separate integer vertex/instance attribute instead (keep tint as RGB).

Special cases: id 17 (`FIELD`) re-uses RGB as data (R = row angle 0..180° as 0..255, G = crop index·100 → 0/100/200, B = shade seed), see `fieldPaint` below.

### 6.2 Surface table (`SURFACES`, `RG:48-84`); `SURFACE_COUNT = 42`

GLSL const arrays `GS_LAYER`, `GS_METRES`, `GS_ROUGH`, `GS_SNOW`, `GS_PAVED`, `GS_FLAT_R/G/B` are generated from this table (unused ids take id 0's row):

| id | use | texture | GS_LAYER | GS_METRES | GS_ROUGH | GS_SNOW | GS_PAVED | GS_FLAT (linear RGB) |
|---|---|---|---|---|---|---|---|---|
| 0 | unmapped land (Streets GL terrain detail) | terrain | 22 | 76 | 0.9 | 1 | 0 | −1 (none) |
| 1 | park | park | 1 | 20 | 0.9 | 1 | 0 | −1 (none) |
| 2 | woods | forest | 2 | 15 | 0.95 | 1 | 0 | −1 (none) |
| 3 | farmland (procedural fields) | farmland0 | 3 | 50 | 0.95 | 1 | 0 | −1 (none) |
| 4 | grass | grass | 0 | 25 | 0.95 | 1 | 0 | −1 (none) |
| 5 | garden / residential | garden | 6 | 16 | 0.92 | 1 | 0 | −1 (none) |
| 6 | sand | sand | 7 | 12 | 0.9 | 1 | 0 | −1 (none) |
| 7 | rock | rock | 8 | 32 | 0.85 | 1 | 0 | −1 (none) |
| 8 | soil | soil | 9 | 25 | 0.95 | 1 | 0 | −1 (none) |
| 9 | pavement | pavement | 10 | 10 | 0.8 | 0.5 | 1 | −1 (none) |
| 10 | asphalt areas | asphalt | 11 | 20 | 0.85 | 0.5 | 1 | −1 (none) |
| 11 | water (procedural) | water | -1 | 0 | 0.05 | 0 | 0 | −1 (none) |
| 12 | gravel | gravel | 14 | 8 | 0.9 | 1 | 0 | −1 (none) |
| 13 | cobblestone | cobblestone | 16 | 6 | 0.75 | 0.5 | 1 | −1 (none) |
| 14 | ice / glacier (always snow) | sand | 7 | 10 | 0.6 | 1 | 0 | −1 (none) |
| 15 | sports pitch | pitch | 18 | 20 | 0.9 | 1 | 0 | −1 (none) |
| 16 | wooden planks (area) | wood | 20 | 4 | 0.8 | 1 | 0 | −1 (none) |
| 17 | mapped field (crop/rows/shade in RGB) | farmland0 | 3 | 50 | 0.95 | 1 | 0 | −1 (none) |
| 32 | marked road (line) | road | 12 | 0 | 0.8 | 0.5 | 1 | (0.075, 0.075, 0.08) |
| 33 | unmarked road (line) | road-unmarked | 13 | 0 | 0.8 | 0.5 | 1 | (0.075, 0.075, 0.08) |
| 34 | dirt track (line) | dirt | 17 | 0 | 0.95 | 1 | 0 | (0.26, 0.2, 0.13) |
| 35 | railway track (line, textured) | railway | 15 | 0 | 0.8 | 0.5 | 0 | −1 (none) |
| 36 | pavement / footway (line) | pavement | 10 | 10 | 0.8 | 0.5 | 1 | (0.3, 0.3, 0.29) |
| 37 | asphalt line (shoulder, runway) | asphalt | 11 | 20 | 0.8 | 0.5 | 1 | (0.075, 0.075, 0.08) |
| 38 | gravel / ballast / path (line) | gravel | 14 | 8 | 0.9 | 1 | 0 | (0.32, 0.29, 0.25) |
| 39 | tram rails in street (line) | tram | 21 | 0 | 0.8 | 0.5 | 1 | (0.075, 0.075, 0.08) |
| 40 | pier boards (line, textured) | wood | 20 | 0 | 0.8 | 1 | 0 | −1 (none) |
| 41 | cycleway (line) | cycleway | 19 | 8 | 0.8 | 0.5 | 1 | (0.2, 0.1, 0.085) |
| 18-31 | unused | = id 0's row (terrain, 22, 76, 0.9, 1, 0, −1) | | | | | | |

`metres` = how many metres one texture repeat covers when laid fixed to the world; `0` = laid along a line (u across the width, v along). `flat` = plain linear colour used instead of the texture (roads and paths are drawn in flat colours; their textures are only referenced for layer bookkeeping). Road asphalt `ROAD = (0.075, 0.075, 0.08)` linear.

Named constants: `FARMLAND = 3`, `FIELD = 17`, `WATER_ID = 11`, `ICE = 14`.

`LAND = paint(0, [58, 81, 29])` — base ground everywhere (Streets GL's temperate biome green). `POLES = [226, 234, 240]` sRGB — the globe sphere's own colour on the realistic map (visible past ±85.06°).

### 6.3 Area paints (`AREAS`, `RG:99-135`)

| key | id | tint sRGB | | key | id | tint sRGB |
|---|---|---|---|---|---|---|
| ocean | 11 | 200,214,222 | | institutional | 5 | 236,236,226 |
| water | 11 | white | | commercial | 9 | white |
| pool | 11 | 150,255,255 | | industrial | 10 | 236,236,236 |
| wood | 2 | white | | railway | 12 | white |
| scrub | 2 | 230,236,214 | | cemetery | 1 | 228,236,222 |
| grass | 4 | white | | pitch | 15 | white |
| park | 1 | white | | athletics | 8 | 255,150,120 |
| meadow | 4 | 255,246,206 | | playground | 6 | white |
| golf | 1 | 236,255,228 | | quarry | 8 | 236,228,214 |
| heath | 4 | 236,220,196 | | aerodrome | 4 | 236,244,222 |
| farmland | 3 | white | | apron | 10 | white |
| orchard | 5 | 232,240,214 | | runway | 10 | 180,180,186 |
| allotments | 8 | 226,236,206 | | taxiway | 10 | 214,214,218 |
| wetland | 4 | 196,214,200 | | square | 13 | white |
| sand | 6 | white | | platform | 9 | white |
| rock | 7 | white | | pier | 16 | white |
| ice | 14 | white | | bridge | 9 | white |
| residential | 5 | white | | | | |

### 6.4 Polygon classification `areaPaint(feature)` (`RG:160-258`)

By `layerName`, `class`, `subclass` (OpenMapTiles schema) or `type` (Streets GL schema):

- **`water`** (OMT): class `ocean` → ocean; `swimming_pool` → pool; else water.
- **`landcover`**: `wood` → wood; `farmland` → orchard if subclass ∈ {orchard, vineyard, plant_nursery} else farmland; `grass` → scrub {scrub, shrubbery} / heath {heath, fell} / golf {golf_course} / meadow {meadow, grassland} / allotments {allotments} / park {park, garden, village_green, recreation_ground} / else grass; `wetland`, `sand`, `rock`, `ice` → same-named; else NONE.
- **`landuse`**: residential → residential; {school, university, college, kindergarten, hospital, library, stadium} → institutional; {commercial, retail, garages, bus_station, dam} → commercial; industrial; railway; cemetery; pitch; track → athletics; playground; quarry; {theme_park, zoo} → park; else NONE.
- **`aeroway`**: aerodrome, apron, runway, taxiway → same-named; else NONE.
- **`transportation`** polygons: class pier → pier; bridge → bridge; subclass platform → platform; else square.
- Streets GL **`common`** / **`natural`**: `type == "farmland"` → `fieldPaint(p)`; else `SGL_AREAS[type]`: forest→wood, scrub→scrub, grass→grass, garden→residential, farmland→farmland, fairway→golf, pitch/playground/dogPark→pitch, sand, rock, construction/brownfield→quarry, parking/apron/helipad→apron, bridge, pier; else NONE.
- Streets GL **`highways`** polygons: `pathType` ∈ {pedestrian, footway, platform} → platform; else apron.
- **`standard_railway_platforms`** (OpenRailwayMap) → platform.

`fieldPaint(p)` (`RG:266-278`): from the Streets GL oriented bounding box properties `@ombb00,@ombb01,@ombb10,@ombb11,@ombb20,@ombb21`: `a = p1 − p0`, `b = p2 − p1`; take the longer; `angle = ((atan2(dy,dx) mod π) + π) mod π / π`; `id = osmId`; `h(salt) = fract(sin(id·12.9898 + salt·78.233)·43758.5453)`; paint = `[round(angle·255), floor(h(1)·3)·100, round(h(2)·255), 255 − 17]`.

`LAND_COVER_PAINTS` (land-cover class → paint, `RG:142-155`): CONIFERS/MIXED/BROADLEAVES/FOREST → wood; CLEARCUT → heath; WETLAND → wetland; ARABLE → farmland; GRASS → meadow; SHRUB → scrub; HEATH → heath; BARE → rock; SNOW → ice. (BUILT, WATER, NONE: nothing.)

### 6.5 Lines: `lineOf(feature)` → `{ width (m, full), paint, edge (m each side), edgePaint }` (`RG:301-413`)

Paint constants: `PAVEMENT = paint(36)`, `SHOULDER = paint(37, [210,210,210])`, `BALLAST = paint(38)`.

`ROADS` (OMT `transportation.class`):

| class | width m | paint id | edge m | edge paint |
|---|---|---|---|---|
| motorway | 11 | 32 (marked) | 1.5 | SHOULDER |
| trunk | 10 | 32 | 1.5 | SHOULDER |
| primary | 9 | 32 | 2 | PAVEMENT |
| secondary | 8 | 32 | 2 | PAVEMENT |
| tertiary | 7 | 33 (unmarked) | 1.8 | PAVEMENT |
| minor | 6 | 33 | 1.6 | PAVEMENT |
| busway | 7 | 33 | 1.8 | PAVEMENT |
| service | 4 | 33 | 0 | – |
| raceway | 10 | 32 | 0 | – |
| track | 3 | 34 | 0 | – |
| rail | 2.6 | 35 | 1 | BALLAST |
| transit | 2.2 | 35 | 0.6 | BALLAST |
| pier | 3 | 40 | 0 | – |

`PATHS` (class `path`, by subclass): footway 2 m id 36; cycleway 2.5 m id 41; pedestrian 5 m id 36; path 1.6 m `paint(38, [236,226,206])`. Subclass `steps`/`platform` → footway; unknown → path.
`WATERWAYS` widths (m): river 18, canal 12, stream 3, drain 1.5, ditch 1.5, default 2; paint = water.
`AEROWAYS`: runway 45 m `paint(37,[180,180,186])`; taxiway 18 m `paint(37,[214,214,218])`.
`TRAM` = 2.2 m id 39, no edge.

Rules:

- `transportation`: skip (`NO_LINE`) if `brunnel == "tunnel"`, class `ferry`, or class ends with `_construction`. class `path` → PATHS. class `transit` & subclass `tram` → TRAM. In the app (`OWN_RAILS = isTauri()`), class `rail`/`transit` → skipped (OpenRailwayMap's are drawn instead). Else `asSurveyed(ROADS[class] ?? NO_LINE, p, false)`.
- `waterway`: skip tunnels; width from `WATERWAYS`.
- `aeroway`: `AEROWAYS[class]`.
- Streets GL `highways`: `type == "railway"` → tram if `railwayType == "tram"`, else (app) nothing / (web) `ROADS.rail`. Otherwise `sglPath(p)` (§6.6) gives kind/width/surface; base = `AEROWAYS[kind] ?? PATHS[kind] ?? ROADS[kind]`; width = `path.width ?? base.width`; paint = `SGL_SURFACES[surface]` if the surface isn't asphalt or the kind is a path, else the base paint; if `sidewalkSide` is set: `edge = max(base.edge, 2)`, `edgePaint = PAVEMENT`; then `asSurveyed(..., mapped = width mapped || surface mapped)`.
  `SGL_SURFACES`: asphalt → paint(37); concrete → paint(37,[226,226,222]); cobblestone → paint(13); dirt → paint(34); sand → paint(34,[255,236,200]); wood → paint(40).
- Streets GL `water` lines: only `type == "waterway"`; width = `p.width || WATERWAYS[waterwayType] || 2`.
- `railway_line_high` (OpenRailwayMap; app only): not in tunnels; `speedKind(p)` (rail-layer doc): `"transit"` → `ROADS.transit` unless `feature == "tram"` (skipped); any other kind → `ROADS.rail`; null → nothing.
- `standard_railway_platforms` lines → `PATHS.pedestrian`.
- `asSurveyed(line, p, mapped)`: if `p.nvdbWidth > 0` and not `mapped`: `width = nvdbWidth`, and paint = `paint(34)` (dirt) if `nvdbGravel > 0`.

Accessors: line width = `lineOf(f).width`; edge width = `width + 2·edge` (0 if no edge); edge colour = `edgePaint`. Width units metres; `lineWidthMinPixels = 0.6` (hairline far out) except Streets GL z16 tiles in the city layer (0); rounded joints and caps.

### 6.6 Streets GL tiles (`SG`)

- URL `https://tiles.streets.gl/vector/{z}/{x}/{y}` — **only z = 16 is served** (`SGL_ZOOM = 16`); used from view zoom ≥ 15.5 (`SGL_FROM_ZOOM`). MVT, layers: `buildings`, `common`, `natural`, `water`, `highways`, `barriers`, `point`; each feature's kind in `type`. Requests for other zooms must not be sent (`sglFetch` resolves null).
- Attribution string: "Map data close in: Streets GL (tiles.streets.gl), © OpenStreetMap contributors".
- Numeric helper semantics: `num(v)` = finite number or null; `some(v)` = `num(v)` but 0 → null (binary-decoded tiles give 0 for "absent").
- `hexColour(v)`: `v & 0xffffff`; a table maps Streets GL's CSS colour values back to names (0xffffff white, 0x808080 grey, 0xd3d3d3 lightgrey, 0xa9a9a9 darkgrey, 0xc0c0c0 silver, 0xff0000 red, 0x800000 maroon, 0xa52a2a & 0xa0522d brown, 0xd2b48c tan, 0xf5f5dc beige, 0xffff00 yellow, 0xffd700 gold, 0xffa500 orange, 0xffc0cb pink, 0x008000 green, 0x0000ff blue); otherwise `#rrggbb`; 0/absent → undefined.
- `sglBuilding(p)` (`SG:120-145`), `LEVEL = 4 m`:

```text
roofType   = p.roofType if in {flat,hipped,gabled,gambrel,pyramidal,onion,dome,round,skillion,mansard,quadruple_saltbox,saltbox} else "flat"
levelsOfRoof = some(p.roofLevels) ?? (roofType == "flat" ? 0 : 1)
roofHeight = some(p.roofHeight) ?? levelsOfRoof*4
height     = some(p.height);  minHeight = num(p.minHeight);  levels = some(p.levels);  minLevel = num(p.minLevel)
if height != null: roofHeight = min(roofHeight, height − (minHeight ?? 0))
if height == null: height = (levels ?? (minLevel ? minLevel+1 : 1))*4 + roofHeight
if minHeight == null: minHeight = min((minLevel ?? 0)*4, height)
if p.buildingType == "roof": minHeight = max(minHeight, height − max(roofHeight, 1))
return { height, minHeight, roofHeight: max(0, roofHeight), roofType,
         colour: hexColour(p.color), facade: FACADES[p.material], roof: ROOFS[p.roofMaterial], roofColour: hexColour(p.roofColor) }
```
  `FACADES`: brick→brick; cement_block, block→cementBlock; wood→wood; plaster, plastered, concrete, hard→plaster; glass, mirror→glass.
  `ROOFS`: tile, tiles, roof_tiles, slate→tiles; metal, metal_sheet, "metal sheet", tin, copper, zinc→metal; concrete, asphalt→concrete; eternit, asbestos→eternit; thatch; grass; glass; tar_paper→tar.
- `sglPath(p)` (`SG:171-212`): `KINDS[pathType]` → `{kind, lanes, width?}`:
  motorway(2), motorway_link(1)→motorway; trunk(2)/trunk_link(1)→trunk; primary(2)/_link(1); secondary(2)/_link(1); tertiary(2)/_link(1); residential, unclassified, living_street→minor(2); service(2); track(1); busway(1); raceway(1, width 6); footway(1); path(1); steps→footway; platform→pedestrian; pedestrian(1, width 8); cycleway(1); runway(1, width 45); taxiway(1, width 20). Unknown → null (not drawn).
  `surface = SURFACES[p.surface] ?? (foot kind or track ? "dirt" : "asphalt")` with `SURFACES`: asphalt, paved→asphalt; unpaved, ground, gravel, dirt, grass, compacted, fine_gravel, earth→dirt; concrete→concrete; paving_stones, sett, cobblestone, pebblestone→cobblestone; sand→sand; wood→wood. Foot kinds = {footway, path, pedestrian, cycleway}.
  If `width` mapped, or the kind has a fixed width, or foot: `{kind, width: p.width ?? kindWidth ?? null, surface}`. Else lanes = `some(p.lanes) ?? (oneway ? max(1, floor(kindLanes/2)) : kindLanes)`; width = `lanes == 1 ? 4 : lanes·3`.
- `sglTree(p)`: `leafType == "needleleaved"` or genus ∈ {abies, picea, pinus} → fir; genus fagus → beech; tilia → linden; quercus → oak; else broadleaf.

### 6.7 Road widths from Trafikverket NVDB (`RW`)

- Only in the app, only within lon 10.5-24.5, lat 55-69.2 (Sweden), only when flat and zoom ≥ 14 (`ROAD_WIDTHS_ZOOM`, `G:2658`).
- Backend command `roadWidths(lon, lat)` (Rust `roads.rs`) returns `RoadPiece { line: [lon,lat][], width: metres, paved: bool }` for a 2000 m square (`CELL_M`). Cell key: `stepLat = (2000/6 371 008.8)·180/π`; `j = floor(lat/stepLat)`; `midLat = (j+0.5)·stepLat`; `stepLon = stepLat / max(0.05, cos(midLat))`; `i = floor(lon/stepLon)`; centre = `((i+0.5)·stepLon, midLat)`.
- `wantRoadWidths`: asks (once per cell) for the centre's cell and the four neighbours probed at ±0.6 step, skipping neighbours whose centre is > 1.1 cells away. On arrival segments go into a spatial hash (0.002° cells) and a global `version` is bumped; ground tiles re-derive their line widths when the version changes.
- Matching (`stretchAt`): for each road polyline of a tile (only car roads: OMT classes in `ROADS` except rail/transit/pier/track; Streets GL kinds in `ROADS` except track), take the midpoint of its middle segment and that segment's bearing; nearest NVDB segment within `MATCH_M = 10` m (search the 3×3 hash cells) whose bearing is within `MATCH_DEG = 30°` (modulo 180°). The whole polyline gets `nvdbWidth = piece.width`, `nvdbGravel = paved ? 0 : 1`.

### 6.8 Ground shader

Extra uniforms/varyings and generated tables (verbatim; tables as in §6.2):

`src/lib/realistic-ground.ts:463-479`

```glsl
const groundFs = /* glsl */ `\
uniform mediump sampler2DArray rground_color;
uniform mediump sampler2DArray rground_normal;
uniform sampler2D rground_water;
uniform sampler2D rground_earth;
uniform sampler2D rground_night;
in vec2 rg_e;
in vec3 rg_wall;
${table("GS_LAYER", (s) => (s.texture === "water" ? -1 : textureLayer.ground(s.texture)))}
${table("GS_METRES", (s) => s.metres)}
${table("GS_ROUGH", (s) => s.rough)}
${table("GS_SNOW", (s) => s.snow)}
${table("GS_PAVED", (s) => (s.paved ? 1 : 0))}
${table("GS_FLAT_R", (s) => s.flat?.[0] ?? -1)}
${table("GS_FLAT_G", (s) => s.flat?.[1] ?? -1)}
${table("GS_FLAT_B", (s) => s.flat?.[2] ?? -1)}
`;
```

Samplers: `rground_color`/`rground_normal` = the ground arrays; `rground_water` = `water.normal`; `rground_earth` = `earth.color`; `rground_night` = `earth.night`.

Vertex + fragment injections (verbatim; `${SURFACE_COUNT}` = 42, `${FARMLAND}` = 3, `${FIELD}` = 17, `${ICE}` = 14). `color` on entry is the feature's paint (0..1), `geometry.uv` is the path layer's across/along coordinate for lines:

`src/lib/realistic-ground.ts:481-624`

```glsl
const groundInject = {
  "vs:DECKGL_FILTER_GL_POSITION": /* glsl */ `
  {
    // where it is on the picture of the Earth: on the globe, its own place; on the flat map (close in) the view's centre is near enough
    vec2 rg_ll = project.projectionMode == PROJECTION_MODE_GLOBE ? geometry.worldPosition.xy : rlight.anchor;
    rg_e = vec2((rg_ll.x + 180.0) / 360.0, (90.0 - rg_ll.y) / 180.0);
  }
  `,
  // an extruded area's sides (a platform's edge) face their own way; all else up (the flat map's)
  "vs:#main-end": /* glsl */ `
    rg_wall = vec3(0.0);
  #ifdef IS_SIDE_VERTEX
    rg_wall = geometry.normal;
  #endif
  `,
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
  {
    float id = floor((1.0 - color.a) * 255.0 + 0.5);
    if (id >= ${SURFACE_COUNT}.0) {
      color.a = 0.0;
    } else {
      int i = int(id);
      vec3 tint = rl_linear(color.rgb);
      float layer = GS_LAYER[i];
      vec3 v = normalize(rl_eye);
      vec3 n = vec3(0.0, 0.0, 1.0);
      vec3 albedo;
      float rough = GS_ROUGH[i];
      // texture coordinates: fixed to the world, whole repeats to the period; or along a line, across its width
      vec2 uv = GS_METRES[i] > 0.0
        ? rl_t * max(1.0, floor(rlight.periodMetres / GS_METRES[i] + 0.5))
        : vec2(geometry.uv.x * 0.5 + 0.5, geometry.uv.y * 0.5);
      if (layer < 0.0) {
        // water: Streets GL's ripples, two of them drifting across each other, under a mirror of the sky
        vec2 w = rl_t * max(1.0, floor(rlight.periodMetres / 18.0 + 0.5));
        vec3 n1 = texture(rground_water, w + vec2(rlight.time * 0.011, rlight.time * 0.007)).xyz * 2.0 - 1.0;
        vec3 n2 = texture(rground_water, w * 1.7 - vec2(rlight.time * 0.008, -rlight.time * 0.012)).xyz * 2.0 - 1.0;
        n = normalize(vec3(n1.xy + n2.xy, n1.z * n2.z) * vec3(0.6, 0.6, 1.0));
        albedo = vec3(0.012, 0.025, 0.035) * tint * 2.0;
        // ripples too small to see from up here still break up the sun's reflection: a soft sheen, not a mirror's spot
        float unseen = smoothstep(0.04, 0.5, rl_cells(w));
        rough = mix(rough, 0.25, unseen);
        rl_glint = 1.0 - 0.8 * unseen;
      } else {
        // Fields: the land round points scattered over it (40 to the clouds' period, about 250 m, numbered
        // the world over so the pattern repeats only every 11 km or so), so each field has straight edges
        // and its own shape, size and lie; to each its crop (one of Streets GL's three) and its rows turned
        // its own way. The crops' colours are drawn closer together than in the pictures, the more so the
        // smaller the fields look (side by side, far off, the soil's brown turns purple in the air), and
        // fields too small to see take the crops' mean, not a flicker.
        // A field mapped as one (Streets GL's tiles) has its crop, rows and shade in its colour.
        vec3 c3 = vec3(0.0);
        float crops = 0.0, even = 0.0, shade = 1.0;
        bool field = i == ${FARMLAND} || i == ${FIELD};
        if (field) {
          vec2 p = rl_c * 40.0;
          float seed, a;
          if (i == ${FIELD}) {
            seed = color.b;
            layer += floor(color.g * 2.999);
            a = 3.14159 * color.r;
            tint = vec3(1.0);
          } else {
            vec2 cell = floor(p);
            float best = 1e9;
            vec2 near = cell;
            for (int y = -1; y <= 1; y++) {
              for (int x = -1; x <= 1; x++) {
                vec2 o = cell + vec2(float(x), float(y));
                vec2 q = o + 0.1 + 0.8 * vec2(rl_hash(o, 40.0), rl_hash(o + 17.0, 40.0));
                float d = dot(p - q, p - q);
                if (d < best) {
                  best = d;
                  near = o;
                }
              }
            }
            seed = rl_hash(near + 29.0, 40.0);
            layer += floor(seed * 2.999);
            a = 6.2832 * fract(seed * 7.31);
          }
          uv = mat2(cos(a), sin(a), -sin(a), cos(a)) * uv;
          float first = GS_LAYER[${FARMLAND}];
          c3 = (textureLod(rground_color, vec3(0.5, 0.5, first), 16.0).rgb
            + textureLod(rground_color, vec3(0.5, 0.5, first + 1.0), 16.0).rgb
            + textureLod(rground_color, vec3(0.5, 0.5, first + 2.0), 16.0).rgb) / 3.0;
          even = mix(0.5, 0.85, smoothstep(0.004, 0.04, rl_cells(p)));
          shade = 0.93 + 0.14 * fract(seed * 3.7);
          crops = smoothstep(0.35, 0.9, rl_cells(rl_t * 3.0));
        }
        // (the images are stored top row first: v runs up them)
        vec3 at = vec3(uv.x, -uv.y, layer);
        vec4 c = texture(rground_color, at);
        if (field) {
          vec3 own = textureLod(rground_color, vec3(0.5, 0.5, layer), 16.0).rgb;
          c.rgb *= mix(vec3(1.0), c3 / max(own, vec3(1e-3)), even) * shade;
          c.rgb = mix(c.rgb, c3, crops);
        }
        vec3 nm = texture(rground_normal, at).xyz * 2.0 - 1.0;
        n = dot(rg_wall, rg_wall) > 0.0 ? normalize(rg_wall) : normalize(vec3(nm.xy, max(nm.z, 0.3)));
        // a little unevenness over tens of metres, so the repeats don't show (gone where it would only speckle)
        float grain = 1.0 - smoothstep(0.25, 1.0, rl_cells(rl_t * 32.0));
        albedo = c.rgb * tint * (1.0 + 0.28 * grain * (rl_noise(rl_t, 32.0) - 0.5));
        // a road or path: its plain colour, flat
        if (GS_FLAT_R[i] >= 0.0) {
          albedo = vec3(GS_FLAT_R[i], GS_FLAT_G[i], GS_FLAT_B[i]) * tint;
          n = vec3(0.0, 0.0, 1.0);
        }
        // Zoomed out the map's data hardly says what the land is (desert, savanna and tundra would all be
        // grass): there its colour is the Earth's own, from the picture of it, taking over as a pixel grows
        // past 100 m or so. Closer in, land nothing is mapped on keeps the Earth's colour where that is
        // dry (sand, rock), in the terrain's detail; where it is green, Streets GL's biome green.
        vec4 earth = texture(rground_earth, rg_e);
        float far = earth.a * smoothstep(120.0, 1200.0, rl_cells(rl_t) * rlight.periodMetres);
        if (i == 0) {
          const vec3 LUM = vec3(0.2126, 0.7152, 0.0722);
          // (the detail's own unevenness, around its tinted mean, on the Earth's colour)
          vec3 mean = textureLod(rground_color, vec3(0.5, 0.5, layer), 16.0).rgb * tint;
          float dry = earth.a * smoothstep(0.03, 0.15, (earth.r - earth.g) / (earth.r + earth.g + 1e-4));
          albedo = mix(albedo, earth.rgb * dot(albedo, LUM) / max(dot(mean, LUM), 1e-4), dry);
        }
        if (i != ${ICE}) albedo = mix(albedo, earth.rgb, far);
        // snow lies on the land, slush on the roads; rain darkens what is paved and makes it shine
        // (the weather where the camera is: not the whole world's, zoomed out)
        float snow = i == ${ICE} ? 1.0 : rlight.snow * GS_SNOW[i] * (1.0 - far);
        albedo = mix(albedo, vec3(0.82, 0.84, 0.88) * mix(0.75, 1.0, GS_SNOW[i]), snow);
        float wet = rlight.wet * mix(0.35, 1.0, GS_PAVED[i]) * (1.0 - far);
        albedo *= 1.0 - 0.45 * wet;
        rough = mix(rough, 0.12, wet * GS_PAVED[i] * (1.0 - rlight.snow));
        n = normalize(mix(n, vec3(0.0, 0.0, 1.0), wet * 0.6));
      }
      vec3 lit = rl_shade(albedo, n, v, rough, 0.0, 1.0, rl_sunVisible());
    #ifndef RL_LINE
      // from far over the globe, its towns' lights where the sun has set (NASA's Black Marble)
      if (rlight.globe > 0.0) {
        float dark = 1.0 - smoothstep(-0.12, 0.03, rl_sun.z);
        lit += rl_linear(texture(rground_night, rg_e).rgb) * 0.55 * rlight.globe * dark;
      }
    #endif
      color = vec4(rl_display(rl_aerial(lit)), 1.0);
    }
  }
  `,
};
```

Things to know to port it:

- `rg_e` = equirectangular UV into the whole-Earth pictures: `((lon+180)/360, (90−lat)/180)`; on the flat map it is constant = the view centre.
- `rg_wall` = the side normal for extruded polygons' side faces (station platforms), zero otherwise.
- `RL_LINE` is defined when the layer is a path layer (lines). Lines skip the night-lights term.
- Line UV: deck.gl's path layer gives `geometry.uv = (x across in −1..+1, distance along the current segment from its start in half-widths)`, so `uv = (x·0.5+0.5, y·0.5)`: u 0..1 across the full width, v advancing 1 per full width of travel (restarting at each polyline vertex). Only ids 35 (railway: rails + sleepers) and 40 (pier boards) actually sample their textures this way; all other line surfaces have `flat` colours. **Verify against deck.gl 9.4 `path-layer-vertex.glsl` when matching sleeper spacing** (texture v-period = one track width = 2.6 m).
- World-fixed UV: `rl_t · max(1, floor(periodMetres/metres + 0.5))` — a whole number of repeats per period so tiles meet at period boundaries.
- Water: albedo `(0.012, 0.025, 0.035)·tint·2`, two scrolling normal-map samples at 18 m repeat (second at ×1.7 scale), roughness 0.05 → 0.25 and glint 1 → 0.2 as ripples become sub-pixel.
- Fields (id 3: procedural Voronoi cells on a lattice of 40 cells per cloud period, `rl_c·40`, ≈ 490 m·cos φ per cell; id 17: mapped fields, parameters from the paint): crop layer = farmland0 + {0,1,2}; rows rotated by `a`; colours pulled towards the three crops' mean (`c3`, sampled with `textureLod(..., 16.0)` = the 1×1 mip = texture mean).
- `textureLod(x, vec3(0.5,0.5,layer), 16.0)` is used throughout as "mean colour of layer". In the port precompute these means on the CPU once (read the last mip after transcoding to RGBA, or average the base level) and pass them as a small uniform array — cheaper and avoids depending on LOD clamping.
- Far-out Earth colour: `far = earth.a · smoothstep(120, 1200, metresPerPixel)` where metres per pixel = `rl_cells(rl_t)·periodMetres`; albedo → `earth.rgb` (except ice). Snow and wetness are scaled by `(1 − far)`.
- Unmapped land (id 0) takes the Earth picture's hue where it is "dry" (`(r−g)/(r+g)` 0.03 → 0.15), keeping the terrain texture's luminance detail.
- Snow colour `(0.82, 0.84, 0.88)·mix(0.75, 1, GS_SNOW)`; wet: albedo ×(1 − 0.45·wet), paved roughness → 0.12 (unless snowing), normal flattened by 0.6·wet. `wet` for unpaved = 0.35·`rlight.wet`.
- Night lights (not lines, `globe > 0`): `+ rl_linear(night.rgb)·0.55·globe·(1 − smoothstep(−0.12, 0.03, rl_sun.z))`.
- Output alpha 1 (opaque), or 0 for unknown ids.

### 6.9 Ground tile layers and the three-pass order (`RG:661-945`)

Each ground tile is drawn as **three passes, in this order, over the whole tile**: (1) filled areas; (2) the *edging* of lines (wider strokes in `edgePaint`, width `width + 2·edge`); (3) the lines themselves. So one road's pavement never covers another's asphalt at junctions. No depth writes; painter's order.

Close in on the flat map (binary tiles), lines on bridges are split out (`bridgeParts`, see the bridges/rail document) and four more passes follow: (4) `bridge-shade`: under-bridge lines in `BRIDGE_SHADE = [16, 20, 14, 70]` RGBA (plain, no ground shader), width `max(width, edgeWidth) + 2` m; (5) bridge edges (raised); (6) bridge lines (raised); (7) `bridge-decks`: invisible (colour NONE) strokes with **depth write enabled** for road bridges only (rail bridges width 0), so later layers pass under them.

Sources and zooms:

| Layer | Tiles | Zooms | MVT layers read |
|---|---|---|---|
| `ground-globe` / `ground-flat` (globe view) | OpenMapTiles (`tileUrl`) | 0-14, standard deck selection | `landuse, landcover, aeroway, water, waterway, transportation` |
| `ground-city` (flat view) | OpenMapTiles z 9,10,11,12,13,14 + Streets GL z16 | distance LOD `GROUND_LOD = { levels: [9,10,11,12,13,14,16], bias: 0.5, maxDistance: 80 000 m }` (`G:1041`) | OMT as above; SGL: `common, natural, water, highways` |
| `ground-rails-*` | OpenRailwayMap via the app's `railtiles:` protocol (`railTilesUrl()`), `SPEED_TILE_ZOOMS.min..max` | standard | `railway_line_high` |
| `ground-platforms` | same protocol, z15 only | LOD `{ levels: [15], maxDistance: 4000 m }` | `standard_railway_platforms`; polygons extruded `PLATFORM_M = 0.6` m, sides lit by their normals (`rg_wall`), winding CCW, depth-writing |
| `ground-land-cover` | computed (§11) | 10-14, 256 px tile size, max 4 requests | – |
| `ground-land` | single rectangle | – | – |

---

## 7. Buildings (`BL`, `R:559-763`, `R:1003-1169`, `G:1032-1092`)

### 7.1 Sources, LOD, heights

- `CITY_LOD = { levels: [13, 14, 16], bias: 0.5, maxDistance: 25 000 m }`; sources: OpenMapTiles layer `building` at z13 and z14; on the realistic map additionally Streets GL layer `buildings` at z16 through `sglBuildingFetch` (Overture heights filled in, §7.7). Layer `maxZoom` 16 (realistic) or 14. Max 8 concurrent requests. Only on the flat map (zoom > 12) and when the *buildings* setting is on.
- `CITY_CLOSE_ZOOM = 14`: tiles with z < 14 (the z13 ones, far away) are drawn **flat** (not extruded; no shadows, foot shade or roofs). Tiles z ≥ 14 are extruded.
- Height (`buildingHeight`, `BL:36-43`): OpenMapTiles: `hide_3d ? 0 : (render_height ?? 6)` m. Streets GL: `buildingType == "roof"` → 0; else `b.height − (pitched(roofType) ? roofHeight : roofHeight/2)` (walls stop at the eaves when a pitched roof mesh is built; otherwise half-way up the roof). Buildings always start at the ground (`minHeight` is not used for geometry).
- Shadow cast height (`castHeight`): OMT = same as drawn; SGL = `height − roofHeight/2` (0 for `roof` type).
- Polygon winding: outlines are treated as counter-clockwise in tile coordinates (`_windingOrder: "CCW"`), walls get outward normals; culling off.
- Non-realistic basemaps: flat colour `BUILDING_COLOR = [58, 68, 84]` with deck's Phong material `{ambient 0.42, diffuse 0.6, shininess 16, specular [60,66,80]}`, shadow colour `[4, 8, 16]`.

### 7.2 Per-building style (`buildingStyle`, `R:1128-1169`)

Seed: mean of the footprint's outline vertices in world tile units at zoom 23 precision (`scale = 2^(23 − z)`; `gx = round((tile.x + meanX)·scale)`, `gy` likewise), `seed = imul(gx, 73 856 093) XOR imul(gy, 19 349 663)` (32-bit wrapping multiply) (`BL:219-231`). The same building therefore gets the same look in a z14 and a z16 tile.

`hash(seed, salt)` (`R:1078-1082`), 32-bit:
```text
h = imul(seed ^ imul(salt + 1, 0x9e3779b9), 0x85ebca6b) >>> 0
h = imul(h ^ (h >>> 13), 0xc2b2ae35) >>> 0
return ((h ^ (h >>> 16)) >>> 0) / 2^32
```
`pickWeighted(r, [[value, weight]…])`: walk the list subtracting weights from `r·total`.

- **Façade** (0 plaster, 1 brick, 2 block, 3 wood, 4 glass): mapped (`look.facade`) if given; else if `height > 30` and `hash(seed,1) < 0.55` → glass; else weighted by `hash(seed,2)`: plaster 0.42, brick 0.28, block `height > 12 ? 0.25 : 0.1`, wood `height < 9 ? 0.22 : 0.02`.
- **Roof** (0 tar, 1 concrete, 2 metal, 3 tiles, 4 eternit, 5 flat1 (generic1), 6 flat3 (generic3)): mapped if given; else by `hash(seed,3)`: `height < 10`: tiles 0.55, metal 0.2, eternit 0.15, tar 0.1; otherwise tar 0.35, concrete 0.25, flat1 0.2, flat3 0.15, metal 0.05.
- Streets GL mapped materials (`BL:52-53`): façade plaster 0, brick 1, cementBlock 2, wood 3, glass 4; roof tar 0, concrete 1, metal 2, glass 2, tiles 3, thatch 3, eternit 4, grass 6.
- **Wall colour**: if the feature has a colour (`colour` in OMT, `color` in SGL as name or `#rrggbb`; names resolved through `NAMED`): each channel softened `round(c·0.75 + 200·0.25)`. Else `WALL_COLOURS[facade][floor(hash(seed,4)·len)]` with jitter `j = round((hash(seed,5) − 0.5)·14)` added to all channels (clamped 0..255).
- Returned RGBA = `[r, g, b, 255 − (facade + 5·roof)]`. The shader decodes `code = floor((1 − a)·255 + 0.5)`, `facade = code mod 5`, `roof = floor(code/5)`.

`NAMED` (`R:1053-1075`): white #f2f0ea, black #3a3a3a, grey/gray #9a9a9a, lightgrey/lightgray #c8c8c8, darkgrey/darkgray #6a6a6a, silver #c0c0c0, red #b04a3a, maroon #7a3028, brown #8a5a40, tan #d2b48c, beige #e6dcc0, cream #f0e6c8, yellow #e8d27a, gold #d8b860, orange #d88a50, pink #e0b0b0, green #8aa080, blue #8098b0.

`WALL_COLOURS` sRGB (`R:1012-1047`):
- plaster: (238,234,222) (232,220,196) (226,208,172) (214,214,208) (236,214,196) (206,220,214) (240,228,180)
- brick: (176,92,70) (150,78,60) (190,130,96) (128,72,58) (206,176,136)
- block: (212,210,204) (196,194,188) (222,216,204)
- wood: (150,44,32) (150,44,32) (214,172,92) (236,234,226) (168,170,166)
- glass: (200,214,222) (190,206,200) (214,206,190)

`roofColour(seed, height)` (`R:1100-1120`; an alternative "roof colour as seen from afar" fill, sRGB): tiles → `hash(seed,6) < 0.75 ? (190,117,93) : (131,131,141)`; metal → `< 0.8 ? (169,181,190) : (140,174,163)`; tar (64,64,64); concrete (139,139,139); eternit (88,86,84); flat1 (100,99,100); flat3 (142,144,144). The `roofColours` layer prop that uses it is **not enabled anywhere in `Globe.svelte`**; today far (z13) tiles are flat polygons filled with the wall-style RGBA (alpha 255 − code, i.e. very slightly translucent) under the `UNLIT` material with the building shader disabled (it requires `extruded`). For the port, drawing far tiles as flat polygons in `roofColour` through the light/haze functions is the evident intent — see open questions.

### 7.3 Building shader

Varyings and samplers (verbatim):

`src/lib/realistic.ts:561-579`

```glsl
const rbuildVaryings = (dir: "in" | "out") => /* glsl */ `\
${dir} vec2 rb_uv;
${dir} vec3 rb_n;
${dir} vec3 rb_tangent;
${dir} float rb_z;
flat ${dir} vec3 rb_tint;
flat ${dir} vec4 rb_info;
`;

const rbuildModule = {
  name: "rbuild",
  vs: rbuildVaryings("out"),
  fs: /* glsl */ `\
uniform mediump sampler2DArray rbuild_color;
uniform mediump sampler2DArray rbuild_normal;
uniform mediump sampler2DArray rbuild_mask;
uniform mediump sampler2DArray rbuild_glow;
${rbuildVaryings("in")}`,
} as const;
```

`BAY_M = 4` (a window bay, metres), `STOREY_M = 3.3`, `METRES_PER_DEGREE = 6 370 972·π/180 = 111194.438` (written with `.toFixed(3)`).

Vertex and fragment injections for extruded polygons (verbatim). deck.gl's solid-polygon vertex shader names used here: `fillColors` (per-polygon RGBA 0..1), `pos`/`nextPos` (this wall's two base corners, in the layer's coordinates), `positions` (per-vertex of the side quad: `x` 0 at `pos` → 1 at `nextPos`, `y` 0 bottom → 1 top), `elevations` (metres), `solidPolygon.elevationScale` (1), `IS_SIDE_VERTEX` (defined for the wall program; the roof/top program is a separate draw without it):

`src/lib/realistic.ts:588-705`

```glsl
const rbuildInject = {
  // at the end of main, where the layer's attributes and main's own variables
  // can be read: the wall's length and height, how far along and up it this vertex is
  "vs:#main-end": /* glsl */ `
    rb_tint = fillColors.rgb;
    float rb_code = floor((1.0 - fillColors.a) * 255.0 + 0.5);
    bool rb_globe = project.projectionMode == PROJECTION_MODE_GLOBE;
  #ifdef IS_SIDE_VERTEX
    {
      // the wall from this corner to the next, metres east and north, and facing out (deck.gl's side)
      vec2 rb_along;
      if (rb_globe) {
        rb_along = vec2((nextPos.x - pos.x) * cos(radians(0.5 * (pos.y + nextPos.y))), nextPos.y - pos.y) * ${METRES_PER_DEGREE.toFixed(3)};
        rb_n = normalize(vec3(-rb_along.y, rb_along.x, 0.0) + vec3(1e-9, 0.0, 0.0));
      } else {
        vec4 rb_a = project_position(vec4(pos, 1.0), pos64Low);
        vec4 rb_b = project_position(vec4(nextPos, 1.0), nextPos64Low);
        rb_along = (rb_b.xy - rb_a.xy) / project.commonUnitsPerMeter.xy;
        rb_n = geometry.normal;
      }
      float rb_length = length(rb_along);
      float rb_height = elevations * solidPolygon.elevationScale;
      // whole bays of windows to the wall, whole storeys to the building (Streets GL's way)
      float rb_bays = floor(rb_length / ${BAY_M.toFixed(1)} + 0.5);
      float rb_levels = max(1.0, floor(rb_height / ${STOREY_M.toFixed(1)} + 0.5));
      rb_uv = vec2(positions.x * (rb_bays > 0.5 ? rb_bays : rb_length / ${BAY_M.toFixed(1)}), positions.y * rb_levels);
      rb_tangent = vec3(rb_along / max(rb_length, 1e-6), 0.0);
      rb_z = positions.y * rb_height;
      rb_info = vec4(rb_code, rb_bays, rb_levels, 1.0);
    }
  #else
    // a roof: its textures lie fixed to the world (rl_t), east along them
    rb_n = rb_globe ? vec3(0.0, 0.0, 1.0) : geometry.normal;
    rb_uv = vec2(0.0);
    rb_tangent = vec3(1.0, 0.0, 0.0);
    rb_z = elevations * solidPolygon.elevationScale;
    rb_info = vec4(rb_code, 0.0, 0.0, 0.0);
  #endif
  `,
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
  {
    float code = rb_info.x;
    float facade = mod(code, 5.0);
    float roof = floor(code / 5.0 + 0.01);
    bool wall = rb_info.w > 0.5;
    // the building's own choices from its colour: the same all round it
    float which = fract(dot(rb_tint, vec3(71.23, 43.17, 36.19)));
    vec3 tint = rl_linear(rb_tint);
    float layer;
    vec2 uv;
    if (wall) {
      // plaster, brick, block, wood: the wall alone, or a window to each bay; glass towers all glass
      layer = facade > 3.5 ? 8.0 : facade * 2.0 + step(0.5, rb_info.y);
      uv = rb_uv;
    } else {
      // tar, concrete, metal, tiles, eternit and two of Streets GL's flat roofs, each its metres a tile
      // (whole tiles to the textures' period, so they meet where it repeats)
      layer = 9.0 + roof;
      float metres = roof < 0.5 ? 4.0 : roof < 1.5 ? 10.0 : roof < 2.5 ? 4.0 : roof < 3.5 ? 3.0 : roof < 4.5 ? 5.0 : 18.0;
      // a pitched roof's (RoofExtension) lie along its own face, in metres
      uv = rb_info.z > 0.5 ? rb_uv / metres : rl_t * max(1.0, floor(rlight.periodMetres / metres + 0.5));
      // Streets GL's roof colours: tiles terracotta (or slate), metal pale grey-blue (or copper),
      // concrete grey; its flat roofs a little under their textures' white, so the sun doesn't burn them out;
      // tar and eternit darker
      tint = roof > 2.5 && roof < 3.5 ? (which < 0.75 ? vec3(0.597, 0.205, 0.127) : vec3(0.262, 0.262, 0.309))
        : roof > 1.5 && roof < 2.5 ? (which < 0.8 ? vec3(0.546, 0.644, 0.723) : vec3(0.366, 0.59, 0.509))
        : roof > 0.5 && roof < 1.5 ? vec3(0.497)
        : roof > 4.5 ? vec3(0.8)
        : vec3(0.55);
      // one with its own colour mapped
      if (rb_info.z > 1.5) tint = rl_linear(rb_tint);
    }
    // (the images are stored top row first: v runs up them)
    vec3 at = vec3(uv.x, -uv.y, layer);
    // the textures coarser further off (past a kilometre and a half, a mip level each doubling)
    float detail = clamp(log2(length(rl_eye) / 1500.0), 0.0, 4.0);
    // how big a bay and a storey are on screen: the side, in pixels, of a square of the same area
    // (here, where the wall and roof branches have joined: the derivatives hold)
    vec2 du = dFdx(uv);
    vec2 dv = dFdy(uv);
    float cellPx = inversesqrt(max(abs(du.x * dv.y - du.y * dv.x), 1e-12));
    // windows from 10 px a bay, fading out under that, gone under 5 px: there the plain wall
    float near = smoothstep(5.0, 10.0, cellPx);
    vec3 c = texture(rbuild_color, at, detail).rgb;
    vec3 nm = texture(rbuild_normal, at, detail).xyz * 2.0 - 1.0;
    vec3 m = texture(rbuild_mask, at, detail).rgb;
    // a wall with windows (the same all over each wall: the samples' derivatives hold in here too)
    if (wall && facade < 3.5 && rb_info.y > 0.5) {
      vec3 plain = vec3(at.xy, facade * 2.0);
      c = mix(texture(rbuild_color, plain, detail).rgb, c, near);
      nm = mix(texture(rbuild_normal, plain, detail).xyz * 2.0 - 1.0, nm, near);
      m = mix(texture(rbuild_mask, plain, detail).rgb, m, near);
    }
    vec3 albedo = c * mix(vec3(1.0), tint, m.b);
    if (!wall) albedo = mix(albedo, vec3(0.8), rlight.snow * 0.92);
    // the texture's u runs along the wall (or east), its v up the wall (or north)
    vec3 N = normalize(rb_n);
    vec3 T = normalize(rb_tangent - N * dot(rb_tangent, N) + vec3(1e-6));
    vec3 B = wall ? vec3(0.0, 0.0, 1.0) : normalize(cross(N, T));
    vec3 n = normalize(T * nm.x + B * nm.y + N * max(nm.z, 0.2));
    vec3 v = normalize(rl_eye);
    // the ground shades the foot of a wall; rain makes everything shinier
    float ao = wall ? mix(0.55, 1.0, smoothstep(0.0, 2.5, rb_z)) : 1.0;
    float roughness = mix(m.r, 0.18, rlight.wet * 0.5);
    vec3 lit = rl_shade(albedo, n, v, roughness, m.g, ao, rl_sunVisible());
    // at night about half the windows are lit, warm, some brighter; where they are too small to tell
    // apart, their light evened out over the wall (the windows' share of it, lit on average)
    if (rlight.night > 0.0) {
      float pick = rl_hash(floor(uv) + rb_tint.xy * 41.0, 1021.0);
      float glow = texture(rbuild_glow, at, detail).r;
      vec3 each = glow * step(0.48, pick) * mix(vec3(1.0, 0.62, 0.3), vec3(1.0, 0.84, 0.62), step(0.85, pick));
      vec3 evened = textureLod(rbuild_glow, vec3(0.5, 0.5, layer), 16.0).r * vec3(0.52, 0.355, 0.204);
      lit += mix(evened, each, near) * rlight.night * 0.28;
    }
    color = vec4(rl_display(rl_aerial(lit)), 1.0);
  }
  `,
};
```

What it does:

- **Walls**: `rb_uv = (along · bays, up · levels)` where `bays = round(length/4)` (if 0 bays the wall uses the plain-wall layer and `u = length/4`), `levels = max(1, round(height/3.3))`: whole window bays per wall and whole storeys per building. `rb_tangent` along the wall, `rb_z` height above the ground in metres, `rb_info = (code, bays, levels, 1)`.
- **Flat roofs** (top faces): `rb_info = (code, 0, 0, 0)`, UV from the world-fixed `rl_t` with metres per repeat by roof kind: tar 4, concrete 10, metal 4, tiles 3, eternit 5, flat1/flat3 18.
- **Pitched roof meshes** (§7.5) arrive with `rb_info.z = 1` (roof face; UV in metres along the face / `metres`) or `2` (roof with its own mapped colour → `tint = rl_linear(rb_tint)`), and gables as walls `(code, 0, 0, 1)`.
- Roof tints (linear): tiles `which < 0.75 ? (0.597, 0.205, 0.127) : (0.262, 0.262, 0.309)`; metal `which < 0.8 ? (0.546, 0.644, 0.723) : (0.366, 0.59, 0.509)`; concrete 0.497; flat1/flat3 0.8; tar/eternit 0.55. `which = fract(dot(rb_tint, (71.23, 43.17, 36.19)))` (per-building, from its sRGB colour 0..1).
- Mip bias: `detail = clamp(log2(dist/1500), 0, 4)` passed as the **bias** argument of `texture()`.
- Window LOD: `cellPx` = on-screen size in px of one UV cell (`1/sqrt(|det J|)`); `near = smoothstep(5, 10, cellPx)` cross-fades the window layer with the plain wall layer (`facade·2`).
- `albedo = texColor · mix(1, tint, mask.b)`; roofs get snow: `mix(albedo, 0.8, snow·0.92)`.
- TBN: N = face normal, T = tangent orthogonalised, B = up for walls / `cross(N,T)` for roofs; `n = normalize(T·nm.x + B·nm.y + N·max(nm.z, 0.2))`.
- Wall AO: `mix(0.55, 1, smoothstep(0, 2.5 m, rb_z))`. Roughness `mix(mask.r, 0.18, wet·0.5)`, metalness `mask.g`.
- **Window glow at night** (`night > 0`): per window cell `pick = rl_hash(floor(uv) + rb_tint.xy·41, 1021)`; lit when `pick ≥ 0.48` (≈ 52 % of windows), colour warm `(1, 0.62, 0.3)` or brighter `(1, 0.84, 0.62)` when `pick ≥ 0.85`; `each = glowTex.r · lit · colour`. When windows are too small, `evened = meanGlow(layer) · (0.52, 0.355, 0.204)`. `lit += mix(evened, each, near) · night · 0.28`.
- Output `vec4(rl_display(rl_aerial(lit)), 1)`.

### 7.4 Foot shade (`BL:384-410`, `R:947-965`)

A 5 m wide (`FOOT_SHADE_M`) path along every footprint ring (open path type, centred on the wall so half is under the building), colour = shadow colour with alpha `FOOT_SHADE_ALPHA = 0.42` (realistic; 0 = not drawn on other basemaps), drawn with `SHADOW_PARAMETERS` (stencil-exclusive with shadows!) before the buildings. Fragment (verbatim):

`src/lib/realistic.ts:954-957`

```glsl
        "fs:DECKGL_FILTER_COLOR": /* glsl */ `
    float across = 1.0 - min(abs(geometry.uv.x), 1.0);
    color.a *= across * rl_clear();
  `,
```

i.e. alpha falls linearly from the centre line to the edges, times haze transmittance.

### 7.5 Pitched roofs (`RF`, `BL:233-310`)

Only for Streets GL tiles (first polygon's `layerName == "buildings"`), for buildings with `pitched(roofType)` and `roofHeight > 0` and not `buildingType == "roof"`. One mesh per tile, vertices in **metres east/north/up of the tile's north-west corner** (deck `METER_OFFSETS` with origin `[west, north, 0]`); ring vertices are `(x·tileMetres, −y·tileMetres)` with `tileMetres = 2π·6 378 137·cos(midLat)/2^z`.

Shape mapping (`SHAPES`): gabled, gambrel, saltbox, round → **gabled**; hipped, mansard, quadrupleSaltbox, quadruple_saltbox → **hipped**; pyramidal, dome, onion → **pyramidal**; skillion → **skillion**. Flat: none.

Algorithm (`roofParts`):

1. Open the ring (drop a repeated last point); need ≥ 3 points, `height > 0.05`, |area| ≥ 1 m². Make the outline anticlockwise, holes clockwise.
2. Minimum-area bounding rectangle of the convex hull (rotating edges; Andrew's monotone chain): centre, unit axes `u, v`, half sizes. Ridge runs along the longer side unless `roofOrientation == "across"`. `a` = half length along the ridge, `b` = half width across; if `b < 0.05` no roof.
3. Planes, as height fraction `h(x,y) = c + gx·x + gy·y` (1 at the ridge, 0 at the eaves), in (along `U`, across `V`) from the centre:
   - gabled: `1 − V/b`, `1 + V/b`
   - hipped: those two plus `a/b − U/b`, `a/b + U/b` (ends as steep as the sides)
   - pyramidal: `1 ∓ V/b`, `1 ∓ U/a`
   - skillion: one plane falling along `down` = (`sin d`, `cos d`) for `roofDirection` d degrees from north (else the across axis): `h = (hi − p·down)/(hi − lo)` over the ring's extent (needs `hi − lo ≥ 0.05`).
   The roof is the **minimum** of the planes, clamped at 0.
4. Roof faces: for each plane, clip the outline (and each hole) by Sutherland-Hodgman to the region where this plane ≤ every other plane; triangulate with earcut (holes supported); vertex z = `base + height·max(0, h)` where `base = b.height − b.roofHeight`. Normal = normalize(−gx·H, −gy·H, 1) (H = roof height). UV in metres on the face: `level` = horizontal unit vector across the slope, `up = normal × level`; `uv = (q·level, q·up.xy + (z − base)·up.z)`.
5. Gable walls: for each footprint edge, split at parameters where two planes cross; for each piece with roof height above eaves > 0.05 m at either end, emit a quad from `base` up to `base + h` (two triangles; outward normal `(dy, −dx, 0)/len`); UV = (metres along the edge, metres up).
6. Mesh attributes per vertex: position, normal, `texCoords` (walls: `(u/4, v/3.3)`; roofs: metres), `colors` (wall colour 0..1, or the mapped roof colour for roof faces when `roofColour` is set — passed through `buildingStyle(0,0,colour)`, i.e. softened), `rr_info = (code, 0, isWall ? 0 : (roofColour ? 2 : 1), isWall ? 1 : 0)`.

Roof mesh vertex injection (verbatim):

`src/lib/realistic.ts:712-723`

```glsl
const roofInject = {
  "vs:#decl": "in vec4 rr_info;\n",
  "vs:#main-end": /* glsl */ `
    rb_tint = colors;
    rb_n = geometry.normal;
    rb_uv = texCoords;
    rb_tangent = normalize(cross(vec3(0.0, 0.0, 1.0), rb_n) + vec3(1e-6, 0.0, 0.0));
    rb_z = 10.0;
    rb_info = rr_info;
  `,
  "fs:DECKGL_FILTER_COLOR": rbuildInject["fs:DECKGL_FILTER_COLOR"],
};
```

### 7.6 Building shadows on the ground — the stencil technique (`BS`, `BL:95-129`, `BL:323-365`, `G:356-375`)

- `shadowStep(toSunENU, cap = 10)`: where the shadow of a point 1 m up lands, `[−e/across, −n/across]·min(across/up, 10)` (null if `up ≤ 0`; `[0,0]` if the sun is at the zenith). `SHADOW_LENGTH_CAP = 10`.
- Per tile, per sun: for every footprint ring, for every edge whose outward normal faces away from the sun (`sign·(ey·dx − ex·dy) > 0`, `sign` = +1 for CCW outlines, flipped for holes), emit the parallelogram `a, b, b + d, a + d` with `d = step·height` (2 triangles). The footprint itself is not drawn (it is under the building). Step is converted to tile units: `[step.e/tileMetres, −step.n/tileMetres]` (tile y runs south).
- Alpha: `BUILDING_SHADOW_ALPHA (0.35) · sunlight(sinAlt)`, where `sunlight` = `min(1, altitudeDeg/6)` (0 at/below horizon); not drawn below `SHADOW_MIN_ALPHA = 0.05`. Realistic map: colour `[6, 12, 28]`, final alpha `min(1, alpha · shadowStrength)` with `shadowStrength = round(sunStrength·1.55·20)/20`, `sunStrength = max(look.sunColor)/3.2`; fragment alpha additionally × `rl_clear()` (`HAZE_FADE`). Dark basemap: colour `[4, 8, 16]`, strength 1.
- Sun used: the real sun (or, with daylight off on the realistic map, the fixed scene sun converted to Earth-centred), **held until it has moved 0.25°** (`SHADOW_SUN_STEP = cos 0.25°`, about once a minute). Per tile the local sun is `localSun(sun, tileMidLat, tileMidLon)`.
- Recast budget: when the sun changes, tiles are recast lazily: at most `RECAST_BUDGET_MS = 4` ms per frame (at least one tile per frame); stale tiles keep their old shadows meanwhile. New tiles always get shadows immediately.
- **Stencil**: all shadow geometry (building parallelograms, foot shade bands, tree shadow cards, aircraft shadows) is drawn with: depth test `less-equal`, no depth write, no culling, stencil compare `equal` against reference 0, pass op `increment-clamp`, fail/depth-fail `keep`, write mask 0xFF. The stencil is cleared to 0 each frame. Result: a pixel takes at most one shadow layer per frame — overlapping parallelograms / neighbouring buildings / tree-on-building shadows never darken twice. Shadows are drawn *before* their tile's buildings so the buildings' depth doesn't matter for the tile's own shadows, and depth-tested so earlier buildings in front hide them.
- wgpu: depth-stencil format `Depth24PlusStencil8`; pipeline `stencil: StencilState { front = back = StencilFaceState { compare: Equal, fail_op: Keep, depth_fail_op: Keep, pass_op: IncrementClamp }, read_mask: 0xff, write_mask: 0xff }`, `set_stencil_reference(0)`, `depth_write_enabled: false`, `depth_compare: LessEqual`, `cull_mode: None`. Because foot shade uses the same stencil rule and is drawn first, a pixel within 2.5 m of a wall gets the foot shade and *not* the cast shadow (keep this order for parity).

### 7.7 Overture heights for Streets GL buildings without one (`OV`)

- Catalogue `https://stac.overturemaps.org/catalog.json` → `latest` release (fallback `"2026-09-23.1"`); archive `https://tiles.overturemaps.org/<release>/buildings.pmtiles` read with HTTP range requests; z14 tiles, MVT layers `building`, `building_part`; keep polygons with `height > 0` or `num_floors > 0` (outer ring, bbox). Cache 48 tiles.
- For each Streets GL building with neither `height` nor `levels`: test its vertex-mean point (lon/lat) against the Overture polygons of its z14 tile (bbox then even-odd point-in-ring); take the tallest (by height, else by floors); write `height` if > 0 else `levels = floors` into the building's properties before styling/extrusion.
- Attribution: "Building heights: © Overture Maps Foundation, © OpenStreetMap (ODbL)".

---

## 8. Trees (`TL`, `TM`, `CN`, `ST`, `R:765-890`)

### 8.1 When and where

Flat map, realistic, zoom ≥ 14. Tiles within `radius = min(sgl ? 2400 : 4800, max(900, 1.6·cameraMetres))` metres of the view centre (`tilesAround`: all tiles whose nearest point is within the radius, nearest first), at z14 from OpenMapTiles (layers `landcover, landuse, water, building, transportation`) or, when zoom ≥ 15.5, at z16 from Streets GL (layers `natural, common, water, buildings, highways, point`). At most 4 tiles loading at once (`MAX_LOADING`); LRU cache of 256 planted tiles (`MAX_CACHED`); a failed tile is retried only after eviction. Features are decoded in tile-local coordinates (0..1, y down).

### 8.2 Planting a tile (`plantTile`, `TL:321-555`) — deterministic

Constants: `GRID = 512` mask cells per tile side; `MAX_TREES = 24 000` per tile; `EARTH_CIRCUMFERENCE = 40 075 016.686`.

1. `tileMetres = C·cos(midLat)/2^z`; `cellMetres = tileMetres/512`.
2. **Cover mask** (u8 512×512): polygons rasterised (even-odd scanline at cell centres) with their cover code, lower codes first so higher codes win. `coverOf(properties)`:
   - SGL `natural`: forest → 11 (leafType broadleaved) / 12 (needleleaved) / 10; scrub → 9. SGL `common`: garden → 4.
   - OMT `landcover`: wood 10; wetland 8; farmland+orchard 7; grass: scrub/shrubbery/heath 9, park/recreation_ground/village_green 5, garden 4, allotments 3.
   - OMT `landuse`: cemetery 6; residential 1; school/university/college/hospital 2.
3. **Blocked mask**: `blockOf`: polygons of layers `building`/`buildings`/`water` → filled; SGL `common` polygons of type ∈ {parking, pitch, playground, dogPark, bridge, construction, brownfield, apron, helipad, pier, sand, rock}; SGL `highways` polygons filled, railway lines stroked 3.5 m, other paths stroked `(width ?? 2)/2 + 1.5` m; `landuse` polygons railway/pitch/playground; `transportation` polygons filled, lines stroked by `CLEARANCE[class] ?? 3` m (motorway 12, trunk 9, primary 8, secondary 7, tertiary 6, minor 5, service 3.5, busway 5, raceway 6, track 2.5, path 1.5, rail 3.5, transit 3, pier 2, bridge 5; `_construction` and ferry ignored). Stroke = every cell centre within radius (m/cellMetres cells) of a segment.
4. **Covers** (`COVERS`, index = code): `{spacing m, chance, conifer share (null = by latitude), height [min,max] m, width factor [min,max]}`:

| code | what | spacing | chance | conifers | height | width |
|---|---|---|---|---|---|---|
| 1 | residential gardens | 15 | 0.2 | 0.25 | 6-13 | 0.9-1.25 |
| 2 | school/hospital grounds | 16 | 0.16 | 0.2 | 7-14 | 0.9-1.3 |
| 3 | allotments | 10 | 0.18 | 0.05 | 3-6 | 1-1.3 |
| 4 | gardens | 12 | 0.32 | 0.2 | 5-11 | 0.9-1.3 |
| 5 | parks | 15 | 0.42 | 0.18 | 9-18 | 0.95-1.35 |
| 6 | cemeteries | 11 | 0.55 | 0.3 | 8-16 | 0.85-1.2 |
| 7 | orchards (rows; jitter 0.08) | 5 | 0.95 | 0 | 3.2-4.8 | 1.1-1.3 |
| 8 | wetland | 10 | 0.22 | 0.1 | 4-10 | 0.9-1.2 |
| 9 | scrub/heath | 5.5 | 0.42 | 0.2 | 2.2-4.5 | 1.1-1.5 |
| 10 | woods | 6.2 | 0.92 | by latitude / land cover | 13-24 | 0.85-1.15 |
| 11 | broadleaved woods | 6.2 | 0.92 | 0.05 | 13-24 | 0.85-1.15 |
| 12 | needleleaved woods | 6.2 | 0.92 | 0.97 | 13-24 | 0.85-1.15 |

5. For each cover code present: a jittered grid **in world tile units** (so it is continuous across tiles): `step = spacing/tileMetres`; cells `i ∈ [floor(tile.x/step), ceil((tile.x+1)/step))`, same for j; `h(salt) = hash2(i, j, code·16 + salt)`; skip if `h(0) ≥ chance`; position `x = (i + 0.5 + jitter·(h(1) − 0.5))·step − tile.x` (jitter 0.7, orchards 0.08), y likewise with `h(2)`; must be inside the tile, on a cell whose cover == code and not blocked.
   `hash2(x, y, salt)` (`TL:230-235`): `h = imul(x, 0x27d4eb2d) ^ imul(y, 0x165667b1) ^ imul(salt, 0x9e3779b1); h = imul(h ^ (h >>> 15), 0x85ebca6b); h = imul(h ^ (h >>> 13), 0xc2b2ae35); return ((h ^ (h >>> 16)) >>> 0) / 2^32`.
6. **Canopy heights** (optional, §8.5): for covers {1,2,3,4,5,6,10,11,12} when a canopy grid exists: non-wood covers use step 7 m (`CANOPY_STEP_M`) and chance 1, and a tree is kept only where the canopy (tallest within ~2 m) is ≥ 3 m (`CANOPY_TREE_M`; woods ≥ 2 m, `CANOPY_WOOD_M`). Height then = `min(45, top·(0.86 + 0.14·h(4)))`.
7. **Unmapped land** (cover 0, not blocked): with a canopy grid: 7 m grid (salt 20), tree where canopy ≥ 4 m (`CANOPY_UNMAPPED_M`), not on land-cover WATER or ARABLE; conifer share from land cover if wooded else 0.2; widths wood's or [0.9, 1.25]. Without canopy but with land cover: wood grid (salt 21), only where the land cover is wooded.
8. **Conifer share** in plain woods: `clamp(0.35 + (|lat| − 56)·0.12, 0.15, 0.8)`; overridden by land cover at the spot: CONIFERS 0.95, MIXED 0.5, BROADLEAVES 0.05.
9. **Registered trees** (§8.6) first: each at its place; kind → species (CONIFER → fir; BEECH → 0; LINDEN → 2 or 3 by `h(12) < 0.5`; OAK → 4; BROADLEAF → random broadleaf); height from canopy else `clamp(crown·1.5 + 1, 3, 28)` (or 7-14 if crown unknown); width factor `clamp(crown/(height·0.75), 0.75, 1.2)` (or 0.9-1.2); hash `hash2(round((tile.x + x)·65536), round((tile.y + y)·65536), 1301 + salt)`; rank −1 − k·1e-9 (never thinned first). No planted tree within 4.5 m (`REGISTERED_CLEAR_M`) of a registered one; no mapped tree within 3 m.
10. **Mapped trees** (SGL `point` layer `type == "tree"`, and `treeRow` lines every 10 m with carry-over between segments): species from `sglTree`; height = mapped `height` or canopy or 7-14; width 0.9-1.2; rank −1; hash salt `977 + k·31 + salt` on the 65536-grid position.
11. `grow(...)` per tree: conifer = `h(3) < conifers` (or by kind); species = fir (1) for conifers else `BROADLEAVES[floor(h(11)·4)]` from `[0, 2, 3, 4]`; width = lerp(wide, h(5)); height = lerp(height, h(4)); yaw = `h(10)·2π`; brightness `v = 0.88 + 0.24·h(8)`.
    Season for broadleaves: `r = h(7)`; with `leafPhase` shares: `r < bare` → colour from `BARE`, width ×0.82; `< bare + autumn` → `AUTUMN`; `< bare + autumn + spring` → `SPRING`; else summer (picture's own colour: `SUMMER_AS_DRAWN = (220,220,220)`). List pick index `floor(h(6)·len)`. `retint` = not summer. Final colour = `min(255, round(c·v))`.
    Palettes sRGB — SPRING: (104,146,62) (118,158,70) (96,136,58); AUTUMN: (196,150,46) (204,118,40) (170,72,38) (150,138,52) (104,108,46) (214,168,60); BARE: (112,98,86) (98,88,80) (122,108,94).
12. Sort by rank (the grid trees' rank is `h(9)`: a deterministic shuffle), truncate to 24 000.
13. Output two instance lists (conifers, broadleaves — drawn identically; the split is historical), per tree:
    - position `[lon, lat, 0]`;
    - 3×3 matrix (column-major, + zero translation): `tall = height·CARD_SCALE[species]` with `CARD_SCALE = [1.35, 1.06, 1.19, 1.02, 1.43]`; `across = tall·width`; columns `(c·across, s·across, 0)`, `(−s·across, c·across, 0)`, `(0, 0, tall)` (s, c = sin/cos yaw);
    - colour RGBA = `[r, g, b, 200 + species·10 + (retint ? 5 : 0)]`.

`leafPhase(date, lat)` (`TL:238-256`): |lat| < 23 → all summer. `day` = day of year (UTC, fractional); southern hemisphere `(day + 182.5) mod 365`; `shift = (|lat| − 56)·4`; `leafOut = 118 + shift`; `turn = 262 − shift`; `fall = 300 − 0.8·shift`; `ramp(x,a,b) = clamp01((x−a)/(b−a))`; `leaves = ramp(day, leafOut−8, leafOut+8)·(1 − ramp(day, fall, fall+18))`; `autumn = ramp(day, turn−10, turn+14)·leaves`; `spring = (1 − ramp(day, leafOut+10, leafOut+34))·leaves·(1 − autumn)`; `summer = max(0, leaves − autumn − spring)`; `bare = 1 − leaves`.

### 8.3 Mesh (`TM`)

Two unit quads crossed at right angles, standing on z = 0, 12 vertices (non-indexed triangle list): card 1 across x (east-west) with normal (0,−1,0); card 2 across y with normal (1,0,0). Per card the vertices `(u, z)` = (0,0),(1,0),(1,1),(0,0),(1,1),(0,1): position = `across·(u − 0.5)` + z up; UV = `(u, 1 − z)` (v = 0 at the top of the tree). No culling, so each quad serves both sides. Instanced with the 3×3 matrix above, at the instance's lon/lat (in metres ENU around it).

### 8.4 Tree shader

Module (verbatim):

`src/lib/realistic.ts:767-780`

```glsl
const rtreeModule = {
  name: "rtree",
  vs: "out vec3 rt_n;\n",
  fs: /* glsl */ `\
in vec3 rt_n;
uniform mediump sampler2DArray rtree_color;
uniform mediump sampler2DArray rtree_normal;
uniform mediump sampler2DArray rtree_mask;
uniform sampler2D rtree_volume;
`,
  inject: {
    "vs:DECKGL_FILTER_GL_POSITION": "    rt_n = geometry.normal * rl_frame(geometry.position.xyz, project.projectionMode == PROJECTION_MODE_GLOBE);\n",
  },
} as const;
```

`rt_n` = the card's normal in ENU. Fragment (verbatim; `color` on entry = instance colour/255 with alpha carrying the code, `geometry.uv` = the card UV):

`src/lib/realistic.ts:784-829`

```glsl
const rtreeInject = {
  "fs:DECKGL_FILTER_COLOR": /* glsl */ `
  {
    // the instance's alpha: which picture, and whether its colour is the season's (tree-layer.ts)
    float code = floor(color.a * 255.0 + 0.5) - 200.0;
    float layer = floor(code / 10.0 + 0.01);
    bool retint = mod(code, 10.0) > 2.5;
    vec3 at = vec3(geometry.uv, layer);
    if (texture(rtree_mask, at).r < 0.5) discard;
    vec3 leaf = texture(rtree_color, at).rgb;
    vec3 albedo = retint
      ? leaf * rl_linear(color.rgb) / max(textureLod(rtree_color, vec3(0.5, 0.5, layer), 16.0).rgb, vec3(1e-3))
      : leaf * pow(color.r / 0.8627, 2.2);
    vec3 v = normalize(rl_eye);
    // the card's face towards the camera, and the way the picture runs across and up it there, from how
    // its texture coordinates change from pixel to pixel (Streets GL's getTBN): the picture's left is on
    // the same side of the tree from either side of the card, as its top is
    vec3 card = normalize(rt_n);
    if (dot(card, v) < 0.0) card = -card;
    vec3 dp1 = dFdx(-rl_eye);
    vec3 dp2 = dFdy(-rl_eye);
    vec2 duv1 = dFdx(geometry.uv);
    vec2 duv2 = dFdy(geometry.uv);
    vec3 dp2perp = cross(dp2, card);
    vec3 dp1perp = cross(card, dp1);
    float facing = sign(dot(card, cross(dp1, dp2)));
    vec3 across = (dp2perp * duv1.x + dp1perp * duv2.x) * facing;
    // (v runs down the picture)
    vec3 up = -(dp2perp * duv1.y + dp1perp * duv2.y) * facing;
    float scale = inversesqrt(max(max(dot(across, across), dot(up, up)), 1e-20));
    // reoriented normal mapping: the leaves' normals on the crown's
    vec3 t = texture(rtree_volume, geometry.uv).xyz * 2.0 - 1.0 + vec3(0.0, 0.0, 1.0);
    vec3 u = (texture(rtree_normal, at).xyz * 2.0 - 1.0) * vec3(-1.0, -1.0, 1.0);
    vec3 m = normalize(t * dot(t, u) - u * t.z);
    vec3 n = normalize((across * m.x + up * m.y) * scale + card * max(m.z, 0.05));
    float sun = rl_sunVisible();
    // snow on the tops of the crowns; the inside and underneath shaded
    albedo = mix(albedo, vec3(0.75), rlight.snow * smoothstep(0.2, 0.7, n.z) * 0.8);
    float ao = mix(0.6, 1.0, smoothstep(0.95, 0.35, geometry.uv.y));
    vec3 lit = rl_shade(albedo, n, v, 0.9, 0.0, ao, sun);
    // the sun through the leaves, seen from the shaded side
    lit += albedo * rl_sunLight() * 0.3 * pow(max(dot(-v, rl_toSun()), 0.0), 3.0) * sun;
    color = vec4(rl_display(rl_aerial(lit)), 1.0);
  }
  `,
};
```

Points:

- `code = alpha·255 − 200`; `layer = floor(code/10)`; `retint = (code mod 10) > 2.5`.
- Alpha test against `tree.mask` (R < 0.5 → discard); no blending, depth-written.
- Albedo: not retinted: `leaf · (color.r/0.8627)^2.2` (0.8627 = 220/255: brightness variation only). Retinted: `leaf · rl_linear(color.rgb) / meanColour(layer)` (texture luminance detail on the seasonal colour).
- Normal: screen-space-derivative TBN on the card (Streets GL's `getTBN`) so both sides of a card show a consistent picture; "reoriented normal mapping" of the leaf normal map (`tree.normal`, xy flipped) onto the shared crown-volume normal map (`tree-volume.normal`, sampled with the card UV, clamp-to-edge); `n = (across·m.x + up·m.y)·scale + card·max(m.z, 0.05)`.
- Snow on upward-facing crown parts: `mix(albedo, 0.75, snow·smoothstep(0.2, 0.7, n.z)·0.8)`; AO darkens the lower part: `mix(0.6, 1, smoothstep(0.95, 0.35, uv.y))`; roughness 0.9, metal 0.
- Translucency: `+ albedo·sunLight·0.3·pow(max(dot(−v, toSun),0), 3)·sunVisible`.

### 8.5 Tree shadows (`TL:633-659`, `R:848-890`)

Only when `sunStrength > 0.05` and `sun.z > 0.02`. Each tree's two cards are sheared flat onto the ground along the sun: with `step = shadowStep(look.sunDir)`, each matrix column `(mx, my, mz)` → `(mx + mz·step.e, my + mz·step.n, 0)`, translation z = 0.05 m. Cached per tile per sun direction quantised to 0.5° in azimuth and altitude. Drawn before the tile's trees with `SHADOW_PARAMETERS` (stencil), colour `[6, 12, 28, round(min(0.6, 0.55·sunStrength)·255)]`. Fragment (verbatim):

`src/lib/realistic.ts:848-856`

```glsl
const rtshadowModule = {
  name: "rtshadow",
  fs: /* glsl */ `\
layout(std140) uniform rtshadowUniforms {
  vec4 color;
} rtshadow;
`,
  uniformTypes: { color: "vec4<f32>" },
} as const;
```
`src/lib/realistic.ts:872-876`

```glsl
        "fs:DECKGL_FILTER_COLOR": /* glsl */ `
    float code = floor(color.a * 255.0 + 0.5) - 200.0;
    if (texture(rtree_mask, vec3(geometry.uv, floor(code / 10.0 + 0.01))).r < 0.5) discard;
    color = vec4(rtshadow.color.rgb, rtshadow.color.a * rl_clear());
  `,
```

(cut out by the same tree mask; alpha fades with haze).

### 8.6 Canopy heights (`CN`)

Meta/WRI canopy height map v2 (CC BY 4.0) mirrored at `https://data.source.coop/tge-labs/meta-chm-v2/chm/<quadkey>.tif`: one Cloud-Optimised GeoTIFF per **zoom-10** Web-Mercator tile (Bing quadkey: per zoom digit = xbit + 2·ybit), 32768 px across (≈ 1.2 m), u8 metres (0 = none), with overviews. For a tile (z ≥ 10): locate its window in the file (`span = 1/2^(z−10)`), choose the smallest overview with ≥ 512 px (`WANT`) across the window, read that window with HTTP range requests (skip images whose `NewSubfileType` has bit 4 = masks). Result `{size, heights: u8[size²]}`. Caches: 24 open files, 96 tiles. `tallestNear(g, x, y, r)` = max in a (2r+1)² pixel box. Attribution "Canopy height: Meta & WRI (CC BY 4.0), via TGE Labs". Port: a COG reader with range requests (`tiff` crate + manual IFD/tile-offset reads, or do it in the backend).

### 8.7 Registered street trees (`ST`)

`static/street-trees/malmo.bin` (bounds W 12.8, S 55.45, E 13.3, N 55.7) and `umea.bin` (19.6, 63.5, 20.9, 64.2), CC0. Format: magic `"RST1"`, `u32` LE count n, then n records of 10 bytes: `i32` LE lon·1e6, `i32` LE lat·1e6, `u8` kind (0 broadleaf, 1 conifer, 2 beech, 3 linden, 4 oak), `u8` crown diameter in half-metres (0 = unknown; `crown = byte/2` m). Records sorted by latitude (binary search `lowerBound` for the tile's south edge). A town's file is loaded only when a tile intersects its bounds. Attribution "Street trees: Malmö stad, Umeå kommun (CC0)".

---

## 9. Clouds (`CL`, `R:892-916`, `G:3606-3638`)

- Picture: EUMETSAT world cloud map IR 10.8 µm, one JPEG 4096×1934 covering lon −180..180, lat −85..85 (EPSG:4326 WMS 1.3.0, bbox order lat,lon):
  `https://view.eumetsat.int/geoserver/wms?service=WMS&version=1.3.0&request=GetMap&layers=mumi:worldcloudmap_ir108&styles=&crs=EPSG:4326&bbox=-85,-180,85,180&width=4096&height=1934&format=image/jpeg`
  Refetched every 60 min; after a failure retried after 5 min. Attribution "Clouds: © EUMETSAT".
- Shown only zoomed out: opacity `1 − smoothstep(5.5, 6.5, zoom)`.
- Geometry at altitude `CLOUD_HEIGHT_M = 10 000` m: on the globe one quad over the bounds (tessellated by the globe projection); on the flat map (not reachable in practice since flat means zoom > 12, but implemented) 1°-latitude strips for `floor(lat) ± 3`, each a sub-image of the picture rows `[floor((north − b)·rows), ceil((north − a)·rows))`, `rows = height/170`.
- Params: no cull, no depth write, depth test less-equal. Fragment (verbatim; `color` = picture texel × opacity in alpha):

`src/lib/realistic.ts:904-908`

```glsl
        "fs:DECKGL_FILTER_COLOR": /* glsl */ `
    float cover = smoothstep(0.33, 0.62, color.r) * color.a;
    vec3 lit = vec3(0.9) * (rl_sunLight() * max(rl_toSun().z, 0.0) + vec3(rlight.ambient * rl_dayHere()));
    color = vec4(rl_display(rl_aerial(lit)), cover);
  `,
```

  i.e. coverage = `smoothstep(0.33, 0.62, grey)·opacity`; colour white 0.9 lit by sun (·sin altitude) + ambient, through haze and display transform.
- Cloud **shadows** on the ground are not from this picture: they are the procedural noise in `rl_sunVisible()` (§4.3), driven by `Conditions.cloudShadows`.

## 10. Rain overlay (`RO`)

A 2D canvas over the map (not GL). Port as an egui painter layer or a tiny line-list pipeline. Shown when `amount ≥ 0.02` and the OS doesn't ask for reduced motion.

- Three depths, drops per 10⁶ px² at full rain / streak length px / speed px·s⁻¹ / line width px / alpha: (300, 12, 900, 0.8, 0.22), (190, 22, 1400, 1.0, 0.30), (80, 38, 2100, 1.4, 0.40).
- Drop count per depth = `round(per·amount·width·height/1e6)`; each drop: random x, y, speed factor `k ∈ [0.75, 1.25)`.
- `SLANT = 0.14` (x per y). `lengthen = 0.3 + 0.7·sin(clamp(pitch, 0, 85)°)`.
- Colour `rgba(s, s+8, s+20, alpha·(0.6 + 0.4·amount)·(1 − 0.5·night))`, `s = round(215 − 120·night)`.
- Each frame (`dt = min(0.05 s, elapsed)`): `fall = speed·k·dt`; `y += fall; x += fall·0.14`; when `y − length > height`: `y = −rand·40`, `x = rand·(width + height·0.14) − height·0.14`. Streak from `(x, y)` to `(x − length·0.14·k, y − length·k)` with `length = depth.length·lengthen`.
- Backing store scale `min(2, devicePixelRatio)`. Inputs are quantised (amount 0.05, pitch 10°, night 0.1) so the drop set isn't rebuilt every frame.

## 11. Land cover (`LC`, `LG`)

Classes: NONE 0, CONIFERS 1, MIXED 2, BROADLEAVES 3, FOREST 4, CLEARCUT 5, WETLAND 6, ARABLE 7, GRASS 8, SHRUB 9, HEATH 10, BARE 11, BUILT 12, WATER 13, SNOW 14. `wooded(c)` = 1..4.

Per map tile (z ≥ 10; 256×256 classes, row 0 north), cached (96 tiles LRU; failures forgotten):

1. **Sweden** (tile centre lon 10.5-24.5, lat 55-69.2): Naturvårdsverket NMD2023 WMS, PNG, exact legend colours decoded back to classes (pixels with alpha < 128 or unknown colours → NONE):
   `https://geodata.naturvardsverket.se/geoserver/wms?service=WMS&version=1.3.0&request=GetMap&layers=lc-nmd:LC.LandCoverRaster.Bas.2023.v2.x&styles=&crs=EPSG:3857&bbox=<w,s,e,n in EPSG:3857 metres, 3 decimals>&width=256&height=256&format=image/png&transparent=true`
   Colour → class (`LC:51-64`):
   - CONIFERS: #6E8C05 #2D5F00 #4E7000 #598C55 #305E50 #23735A
   - MIXED: #38A800 #438870
   - BROADLEAVES: #4CE600 #AAFF00 #97E600 #89CD9B #A5F578 #ABCD78 #00E6A9 #55FF00
   - CLEARCUT: #CDCD66 #898944
   - WETLAND: #C29ED7 #894465 #CD6699 #F57AB6 #D69DBC #73004C #A80084 #E600A9 #FF00C5 #704489 #AA66CD #CA7AF5 #4C0073 #8400A8 #A900E6 #C500FF
   - ARABLE: #FFFFBE; BARE: #E1E1E1 #800080; SHRUB: #CCD79E #ABC9A6 #9EB591; HEATH: #D7C29E #CDAA66 #897044; GRASS: #FFEBAF #FFD37F #FFBF7F; BUILT: #5A1414 #E5464B #191919; WATER: #6699CD #8ACCFA
2. **Elsewhere, and to fill NONE pixels**: ESA WorldCover 2021 via Microsoft Planetary Computer (z ≥ 6). Register a mosaic once: `POST https://planetarycomputer.microsoft.com/api/data/v1/mosaic/register` with JSON `{"collections":["esa-worldcover"],"filter-lang":"cql2-json","filter":{"op":"anyinteracts","args":[{"property":"datetime"},{"interval":["2021-01-01T23:59:59Z","2021-12-31T23:59:59Z"]}]}}` → `searchid`. Tiles: `…/mosaic/<searchid>/tiles/WebMercatorQuad/{z}/{x}/{y}@1x?assets=map&collection=esa-worldcover&format=png`; the red channel is the class code: 10 FOREST, 20 SHRUB, 30 GRASS, 40 ARABLE, 50 BUILT, 60 BARE, 70 SNOW, 80 WATER, 90 WETLAND, 95 BROADLEAVES (mangroves), 100 HEATH (moss/lichen).
   Images must be decoded without colour management or alpha premultiplication and resampled nearest-neighbour to 256².
3. Attribution "Land cover: NMD2023 Naturvårdsverket · ESA WorldCover 2021".

Ground polygons from it (`LG`): tile layer zoom 10-14 (256 px tiles, max 4 requests; deeper views overzoom z14). Per tile: group classes by their paint (`LAND_COVER_PAINTS`); for each group build a 0/1 float mask, skip if < 6 cells (`MIN_CELLS`), box-blur 3×3 twice (edge-aware average), trace the 0.5 iso-contour (d3-contour marching squares, with holes) → polygons in cell coordinates → lon/lat (`lon = ((tx + x/256)/2^z)·360 − 180`, `lat = atan(sinh(π(1 − 2(ty + y/256)/2^z)))`); drop polygons whose outer ring area < 6 cells. Drawn as filled polygons with the ground shader, after `ground-land` and before the vector ground, so it only shows where nothing is mapped. Used also by the tree planter (conifer share, unmapped woods).

---

## 12. Haze on overlays and the terrain surface (`R:921-991`)

- `HAZE_FADE` (hillshade, building shadows): fragment `color.a *= rl_clear();` (verbatim: `R:932`).
- `FOOT_SHADE`: §7.4.
- `TERRAIN_AIR` (the 3D terrain mesh, which shows the draped ground texture; the draped passes are rendered with `cover = 1` so they contain *no* haze): at the end of the terrain fragment shader (verbatim):

`src/lib/realistic.ts:979-983`

```glsl
        "fs:#main-end": /* glsl */ `
    float rl_t = rl_clear();
    vec3 rl_air = rl_aerial(vec3(0.0)) / max(1.0 - rl_t, 1e-4);
    fragColor.rgb = fragColor.rgb * rl_t + rl_display(rl_air) * (1.0 - rl_t) * fragColor.a;
  `,
```

  i.e. `out.rgb = out.rgb·t + display(airColour)·(1 − t)·out.a` — haze applied in display space on top of the already display-encoded draped texture (an approximation of `rl_display(rl_aerial(lit))`).
- `UNLIT` material `{ambient 1, diffuse 0, shininess 1, specular 0}` just turns deck.gl's own lighting off for these layers; nothing to port.

---

## 13. Distance LOD tile selection (`LT`)

deck.gl normally picks one zoom for the whole view (except on the globe / pitch > 60°). The realistic ground, buildings, platforms and the 3D terrain instead pick **each tile's zoom from its distance to the camera**, restricted to the zoom levels a source actually has.

### 13.1 `Lod`

```ts
type Lod = { levels: readonly number[];   // zooms with tiles, ascending
             bias?: number;               // zoom levels finer than deck.gl's rule (default 0)
             maxDistance?: number }       // metres; nothing farther from the camera (default ∞)
```

In use: `GROUND_LOD {levels [9,10,11,12,13,14,16], bias 0.5, maxDistance 80 000}`; `CITY_LOD {levels [13,14,16], bias 0.5, maxDistance 25 000}`; `PLATFORM_LOD {levels [15], maxDistance 4000}`; `SURFACE_LOD {levels 0..16, bias 0.5}` (terrain).

### 13.2 Selection (`distanceTileIndices`), verbatim

`src/lib/lod-tileset.ts:35-105`

```ts
function tileLngLat(x: number, y: number, z: number): [number, number] {
  const scale = 2 ** z;
  const n = Math.PI - (2 * Math.PI * y) / scale;
  return [(x / scale) * 360 - 180, (180 / Math.PI) * Math.atan(0.5 * (Math.exp(n) - Math.exp(-n)))];
}

function boundingVolume(x: number, y: number, z: number, elevation: [number, number], project: ((p: number[]) => number[]) | null) {
  if (project) {
    const points: number[][] = [];
    for (const [px, py] of z < 1 ? REF_11 : z < 2 ? REF_9 : REF_5) {
      const [lng, lat] = tileLngLat(x + px, y + py, z);
      points.push(project([lng, lat, elevation[0]]));
      if (elevation[0] !== elevation[1]) points.push(project([lng, lat, elevation[1]]));
    }
    return makeOrientedBoundingBoxFromPoints(points);
  }
  // web mercator's common space, y up
  const extent = TILE_SIZE / 2 ** z;
  const [ox, oy] = [x * extent, TILE_SIZE - (y + 1) * extent];
  return new AxisAlignedBoundingBox([ox, oy, elevation[0]], [ox + extent, oy + extent, elevation[1]]);
}

/** Whether a tile on the globe is all on the far side of it from the camera. */
function beyondHorizon(x: number, y: number, z: number, camera: number[], project: (p: number[]) => number[], elevation: number) {
  const [cx, cy, cz] = camera;
  const camLng = (Math.atan2(cx, -cy) * 180) / Math.PI;
  const camLat = (Math.asin(cz / Math.hypot(cx, cy, cz)) * 180) / Math.PI;
  const [west, north] = tileLngLat(x, y, z);
  const [east, south] = tileLngLat(x + 1, y + 1, z);
  const mid = (west + east) / 2;
  const lng = Math.max(west, Math.min(mid + (((camLng - mid + 540) % 360) - 180), east));
  const lat = Math.max(south, Math.min(camLat, north));
  const p = project([lng, lat, elevation]);
  return p[0] * cx + p[1] * cy + p[2] * cz <= p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
}

/** The tiles in view, each at the zoom its distance from the camera calls for. */
export function distanceTileIndices(viewport: Viewport, lod: Lod, zRange: number[] | null = null, offset = 0): Index[] {
  const { levels, bias = 0, maxDistance = Infinity } = lod;
  const project = viewport instanceof GlobeViewport ? (p: number[]) => viewport.projectPosition(p) : null;
  const planes = Object.values(viewport.getFrustumPlanes()).map(({ normal, distance }) => new Plane(normal.clone().negate(), distance));
  const culling = new CullingVolume(planes);
  const upm = viewport.distanceScales.unitsPerMeter[2];
  const elevation: [number, number] = [zRange ? zRange[0] * upm : 0, zRange ? zRange[1] * upm : 0];
  const camera = viewport.cameraPosition;
  // deck.gl's distance is in view heights at the view's scale; without the zoom
  const k = viewport.scale / 2 ** viewport.zoom / viewport.height;
  const farthest = maxDistance * upm;
  const [lowest, highest] = [levels[0], levels[levels.length - 1]];
  const allowed = new Set(levels);
  const levelFor = (want: number) => (want >= highest ? highest : want <= lowest ? lowest : levels.find((l) => l >= want)!);
  const out: Index[] = [];
  const visit = (x: number, y: number, z: number) => {
    const volume = boundingVolume(x, y, z, elevation, project);
    if (culling.computeVisibility(volume) < 0) return;
    if (project && beyondHorizon(x, y, z, camera, project, elevation[1])) return;
    const distance = volume.distanceTo(camera);
    if (distance > farthest) return;
    if (allowed.has(z)) {
      const want = offset - Math.floor(Math.log2(Math.max(distance * k, 1e-12)) - bias);
      if (z >= levelFor(want)) {
        out.push({ x, y, z });
        return;
      }
    }
    if (z >= highest) return;
    for (const [dx, dy] of [[0, 0], [0, 1], [1, 0], [1, 1]]) visit(x * 2 + dx, y * 2 + dy, z + 1);
  };
  visit(0, 0, 0);
  return out;
}
```

In words:

1. Recursive quadtree descent from tile 0/0/0.
2. A tile's bounding volume: flat map → axis-aligned box in common space, `extent = 512/2^z`, min `(x·extent, 512 − (y+1)·extent, zmin)`, max `(+extent, +extent, zmax)`; z range from the layer's known elevation range in metres × `unitsPerMeter` (0 without terrain). Globe → oriented box around 5 sample points (centre + corners; 9 at z1, 11 at z0) projected to the sphere (at both elevation bounds if they differ).
3. Cull against the 6 frustum planes (skip if fully outside). On the globe also skip tiles wholly beyond the horizon: take the tile's nearest point to the camera's sub-point (clamped lng/lat), project to `p`; hidden if `p·camera ≤ p·p`.
4. `distance` = box-to-camera distance in common units; skip if `distance > maxDistance·unitsPerMeter`.
5. If z is one of `levels`: `want = offset − floor(log2(max(distance·k, 1e-12)) − bias)`, with `k = viewport.scale / 2^zoom / viewportHeightPx` (flat map: `scale = 2^zoom` ⇒ `k = 1/height`; on the globe `scale/2^zoom` is the globe viewport's latitude-dependent factor) and `offset = log2(512/tileSize) + zoomOffset` (0 for these layers). `levelFor(want)` = lowest level ≥ want, clamped to the lowest/highest level. If `z ≥ levelFor(want)` → **select** this tile, stop descending.
6. Otherwise, if `z < highest` recurse into the four children (order (0,0), (0,1), (1,0), (1,1) in (dx, dy)).

Worked example (flat map, 1000 px tall viewport, lat 60°, `unitsPerMeter = 512/(40 075 017·0.5) = 2.555e-5`): a tile 1000 m from the camera has `distance·k = 0.02555/1000 = 2.555e-5`, `log2 = −15.26`, minus bias 0.5 → floor = −16 → want 16. At 2000-4000 m want is 15 or 14 → the ground uses 16 if ≥ 15 is wanted (the next level ≥ 15 is 16), 14 for want 14, etc. The camera itself is 1.5 viewport heights from the view centre, so at view zoom Z the tile at the view centre has `distance·k = 1.5/2^Z` and wants `−floor(0.585 − Z − 0.5)` = `ceil(Z − 0.085)`: the view's zoom rounded up (to the next available level).

If the viewport's zoom is below the layer's `minZoom` nothing is selected. Without a `lod` the default deck behaviour applies.

### 13.3 Look-ahead preloading while zooming (`LodTileset.loadAhead`, `LT:107-191`)

Every time tiles are selected, compare the camera with the previous call: `dz = zoom_now − zoom_last`. If `|dz| ≥ 1e-3`:

- Extrapolate a virtual camera one zoom level further in the same direction: `k = min(1/|dz|, 50)`; `longitude += (lon_now − lon_last)·k`; `latitude` likewise clamped to ±85; `zoom += sign(dz)·1` (`LOOK_AHEAD_ZOOM = 1`); same size, pitch, bearing.
- Run the same distance selection for that camera; take up to `MAX_AHEAD = 32` tiles not already cached (or already in the previous ahead set) and create/request them.
- Their request priority is `AHEAD_PRIORITY (2e8) + distance priority`, i.e. after every in-view tile (deck's in-view priorities are < 2e8). deck normally cancels requests for tiles that are neither selected nor visible; ahead tiles are exempt while they are in the current ahead set, and dropped when the set changes (the camera turned another way).

### 13.4 Refinement: `keepCoarserUntilIn` (verbatim)

`src/lib/lod-tileset.ts:193-237`

```ts
type RefineTile = {
  state: number;
  isSelected: boolean;
  isVisible: boolean;
  zoom: number;
  content: unknown;
  parent: RefineTile | null;
  children: RefineTile[] | null;
};
const VISITED = 1;
const VISIBLE = 2;
/** A tile with something to draw (one that failed, or isn't there, doesn't count: what is over it is drawn instead). */
const ready = (tile: RefineTile) => tile.content != null;

/**
 * deck.gl's 'no-overlap' refinement (a coarser tile is drawn in place of finer
 * ones until they are all in, never with them: 3D buildings would stand
 * twice), but a tile that came back empty or failed keeps the one over it.
 */
export function keepCoarserUntilIn(allTiles: RefineTile[]) {
  for (const tile of allTiles) tile.state = 0;
  for (const tile of allTiles) {
    if (!tile.isSelected) continue;
    for (let t: RefineTile | null = tile; t; t = t.parent) {
      if (ready(t)) {
        t.state |= VISIBLE;
        break;
      }
    }
  }
  const finer = (tile: RefineTile) => {
    for (const child of tile.children ?? []) {
      if (ready(child)) child.state |= VISIBLE;
      else finer(child);
    }
  };
  for (const tile of [...allTiles].sort((a, b) => a.zoom - b.zoom)) {
    tile.isVisible = Boolean(tile.state & VISIBLE);
    if (tile.children && (tile.isVisible || tile.state & VISITED)) {
      for (const child of tile.children) child.state = VISITED;
    } else if (tile.isSelected) {
      finer(tile);
    }
  }
}
```

A variant of deck.gl's "no-overlap" strategy. Inputs: all cached tiles with `isSelected`, `content` (non-null = loaded with something to draw; a failed/empty tile counts as *not ready*), parent/children links. Result: `isVisible` per tile.

1. For each selected tile, walk up from itself to the first ancestor-or-self that is ready and mark it VISIBLE (so a coarser loaded tile stands in while the fine one loads — and stays for good if the fine one failed or is empty).
2. Process tiles in ascending zoom: `isVisible = VISIBLE bit`. If a tile is visible (or was marked VISITED by an ancestor), mark all its children VISITED (state reset to VISITED: they are covered and must not draw, so nothing is drawn twice — important for 3D buildings). Otherwise, if the tile is selected but not visible (nothing ready at or above it), mark ready descendants visible instead (`finer`: recursive to the first ready descendant on each branch).

### 13.5 Per-zoom sources (`LodMVTLayer.getTileData`)

Each tile request picks the source whose `zooms` include the tile's z (`{zooms, url, layers, fetch?}`); no source → null content. MVT is decoded to flat binary arrays in tile-local coordinates (0..1, y south) on the flat map, or to WGS84 GeoJSON on the globe; features carry `layerName`.

**Port**: implement one generic `LodTileset` in Rust (quadtree select → request queue with priorities and cancellation → cache → refinement), parameterised by `Lod` + sources; it serves ground, buildings, platforms and terrain.

---

## 14. Terrain (`TR`)

### 14.1 Elevation sources and encoding

| Use | URL | Format | Zooms |
|---|---|---|---|
| Hillshade | `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png` (Mapzen/Tilezen, AWS Open Data, keyless) | 256 px PNG, terrarium | 0-15 exist; hillshade uses 0-12 (`HILLSHADE_MAX_ZOOM`), overzoomed beyond |
| 3D surface + camera ground height | `https://tiles.mapterhorn.com/{z}/{x}/{y}.webp` | 512 px WebP (lossless), terrarium | 0-12 everywhere (Copernicus GLO-30), up to 16 where national lidar exists (Sweden 1 m, Norway, Denmark, Finland, …); missing deeper tiles return **404** |
| 3D surface fallback | same Mapzen bucket, `.png`, same z/x/y | 256 px | when Mapterhorn answers 404 |

Terrarium: `height_m = R·256 + G + B/256 − 32768` (`decodeTerrarium`). Pixels must be read exactly (no colour management, no premultiplied alpha) — trivially true with the `image`/`png`/`image-webp` crates.

Attribution (one line, `TERRAIN_ATTRIBUTION`): "Terrain: © Mapterhorn (Lantmäteriet, Kartverket, Copernicus GLO-30 and others); Mapzen Terrain Tiles (AWS Open Data): USGS 3DEP, SRTM, GMTED2010; NOAA ETOPO1; Copernicus EU-DEM; and others". The long list (`TERRAIN_ATTRIBUTION_FULL`, `TR:48-61`, 12 lines) belongs in the credits view; copy it verbatim from the source.

### 14.2 Hillshade (2D shaded relief), all basemaps

- Tile layer: zoom 0-12, 256 px tiles, max 8 requests, cache 300 tiles; both projections; drawn as a bitmap over each tile's bounds (Web-Mercator image), no depth write.
- Opacity = `HILLSHADE_OPACITY[basemap] · opacity · fade`: dark 1, satellite 0.45, realistic 0.6; `opacity` = 1.4 on the realistic map with 3D terrain on, else 1; `fade = round(hillshadeFade(zoom)·20)/20`, `hillshadeFade(zoom) = 1 − 0.4·clamp01((zoom − 11)/3)`.
- On the realistic map it is always drawn (even without the terrain setting) and its alpha is multiplied by haze transmittance (`HAZE_FADE`). On dark/satellite only when `terrain` is on.
- Per tile: decode heights; `zFactor = reliefExaggeration(z) = clamp(2^((11 − z)/3), 1, 8)`; ground metres per pixel per row: `tileRowMetersPerPixel(z, y, row, size) = 2π·6 378 137·cos(lat(row centre)) / (size·2^z)`.
- Style (`HILLSHADE_STYLE`): light azimuth 315°, altitude 45°, shadow colour (4, 8, 16) max alpha 135, highlight (255, 244, 225) max alpha 40, gain 0.55.
- Algorithm (verbatim; Horn 3×3 gradient with edge clamping; heights below 0 count as 0; output is non-premultiplied RGBA8 with flat ground fully transparent):

`src/lib/terrain-layer.ts:136-192`

```ts
export function hillshade(
  heights: ArrayLike<number>,
  width: number,
  height: number,
  metersPerPixel: (row: number) => number,
  style: Partial<HillshadeStyle> = {},
  out?: Uint8ClampedArray,
): Uint8ClampedArray {
  const s = { ...HILLSHADE_STYLE, ...style };
  const rgba = out && out.length >= width * height * 4 ? out : new Uint8ClampedArray(width * height * 4);
  const az = s.azimuth * DEG;
  const alt = s.altitude * DEG;
  // towards the light, in east, north, up
  const lx = Math.sin(az) * Math.cos(alt);
  const ly = Math.cos(az) * Math.cos(alt);
  const lz = Math.sin(alt);
  const flat = lz;
  // below sea level counts as sea level
  const n = width * height;
  if (!seaLevel || seaLevel.length < n) seaLevel = new Float32Array(n);
  const z = seaLevel;
  for (let i = 0; i < n; i++) z[i] = Math.max(0, heights[i]);
  const [sr, sg, sb] = s.shadow;
  const [hr, hg, hb] = s.highlight;
  for (let y = 0; y < height; y++) {
    const rn = (y > 0 ? y - 1 : y) * width;
    const rc = y * width;
    const rs = (y < height - 1 ? y + 1 : y) * width;
    const ySpan = (y > 0 ? 1 : 0) + (y < height - 1 ? 1 : 0);
    const mpp = metersPerPixel(y) / s.zFactor;
    const yScale = 1 / (4 * ySpan * mpp);
    for (let x = 0; x < width; x++) {
      const xw = x > 0 ? x - 1 : x;
      const xe = x < width - 1 ? x + 1 : x;
      // Horn: weighted differences across the 3x3 neighbourhood
      const dzdx = (z[rn + xe] + 2 * z[rc + xe] + z[rs + xe] - z[rn + xw] - 2 * z[rc + xw] - z[rs + xw]) / (4 * (xe - xw) * mpp);
      // rows run south: the gradient towards the north is the negative of the row's
      const dzdy = (z[rn + xw] + 2 * z[rn + x] + z[rn + xe] - z[rs + xw] - 2 * z[rs + x] - z[rs + xe]) * yScale;
      // the surface normal (-dz/dx, -dz/dy, 1), normalised, against the light
      const shade = (-dzdx * lx - dzdy * ly + lz) / Math.sqrt(dzdx * dzdx + dzdy * dzdy + 1);
      const d = (shade - flat) / s.gain;
      const i = (y * width + x) * 4;
      if (d < 0) {
        rgba[i] = sr;
        rgba[i + 1] = sg;
        rgba[i + 2] = sb;
        rgba[i + 3] = Math.min(1, -d) * s.shadowAlpha;
      } else {
        rgba[i] = hr;
        rgba[i + 1] = hg;
        rgba[i + 2] = hb;
        rgba[i + 3] = Math.min(1, d) * s.highlightAlpha;
      }
    }
  }
  return rgba;
}
```

  Port as-is on a worker thread (≈ 1 ms per 256² tile), or as a fragment shader sampling an R16/R32F height texture (then the edge handling needs a 1 px border or clamp addressing; tiles won't match across seams exactly unless neighbours are sampled — the CPU version has the same limitation and it's accepted).

### 14.3 The 3D surface

- deck.gl `TerrainLayer` (Martini RTIN mesh from each 512 px height tile; `meshMaxError = 2` metres; mesh vertices are lng/lat + metres), tiles chosen by `SURFACE_LOD` (every zoom 0-16, bias 0.5), `tileSize 512`, max 8 requests, fetch = Mapterhorn with Mapzen fallback on 404.
- Mesh colour `MESH_COLOR = [34, 39, 48]` (only visible where nothing is draped), no lighting material.
- **Sea-level clamp** (`SeaLevelExtension`): vertex shader `size.z = max(size.z, 0.0);` — vertex heights below 0 m are raised to 0 so the sea floor doesn't sink under the water surface.
- Elevation range (`zRange`) used for tile bounding boxes widens as each tile loads (min/max of the mesh bounding boxes) instead of waiting for all tiles in view; until any tile has loaded, tiles are assumed at sea level.
- `operation: "terrain+draw"`: the mesh is both drawn and registered as the terrain other layers stand on.

How other layers "stand on" it today (deck.gl `TerrainExtension`, applied to every layer except `earth`, `terrain-surface`, `clouds-*`):

- **Drape** mode (layers without 3D extent: ground polygons/lines, hillshade, shadows, foot shade, rail map…): deck renders those layers top-down with an orthographic camera into a texture per terrain tile, and the terrain mesh samples that texture. This is why `rlight` has `cover`/`eye`: in that pass the viewport is orthographic, so the shader takes the real camera position from `eye` for `rl_eye`, and `rl_clear()` returns 1 (no haze baked in); haze is added when the terrain mesh is drawn (`TERRAIN_AIR`). Draped textures are cached across frames ("drawn once for many frames"), so time-varying effects (water ripples, cloud shadows) only update when the cover is re-rendered.
- **Offset** mode (3D layers: buildings, trees, aircraft, trains, labels): deck renders the terrain's height into an offscreen height map and adds the sampled height to each vertex/instance z in the vertex shader.

### 14.4 Camera ground height (`groundHeight`, `TR:216-249`; `G:3204-3212`)

With terrain on and zoom > 12 (and not following an aircraft), each frame: `ground = groundHeight(view.lon, view.lat)`; if it differs from the camera target's current z by > 0.5 m and no programmatic transition is running, move the target's z 30 % of the way (`GROUND_EASE = 0.3`) per frame. `groundHeight`: Mapterhorn tile at `GROUND_ZOOM = 12` containing the point (lat clamped ±85); nearest pixel; `max(0, height)`; returns null (no change) until the tile has loaded, then triggers a redraw; keeps the last 16 tiles.

### 14.5 Port approach for terrain

- Meshing: `martini`-style RTIN in Rust (port of mapbox/martini, ~150 lines; error 2 m, 513² grid needs a 1 px backfill of the last row/column as loaders.gl does), or a fixed regular grid with skirts for a first version.
- Draping: the native renderer owns all shaders, so prefer **direct displacement**: give the ground/overlay vertex shaders access to a height texture (per terrain tile, or a clip-map around the camera) and tessellate ground polygons/lines finely enough — *or* reproduce deck's approach: render ground passes into a per-terrain-tile colour texture (ortho, `cover = 1`) and texture the terrain mesh with it. The second is simpler, matches today's look (including haze-on-top), and reuses the 2D ground renderer unchanged; cost: texture memory (deck uses ≤ 2048² per tile) and re-render on LOD change.
- Offsets for 3D objects: sample heights on the CPU from the decoded height tiles (bilinear, at building centroid / tree position / vehicle position) when building instance buffers, instead of a GPU height-map pass. Tiles needed are already cached by the terrain tileset.

---

## 15. Other data sources referenced by the realistic map (summary)

| Data | URL / origin | Zooms | Notes |
|---|---|---|---|
| OpenMapTiles vector tiles | `tileUrl` (OpenFreeMap; see doc 01) | 0-14 | ground layers `landuse, landcover, aeroway, water, waterway, transportation`; buildings layer `building` (`render_height`, `hide_3d`, `colour`) |
| Streets GL vector tiles | `https://tiles.streets.gl/vector/16/{x}/{y}` | 16 only | ground, buildings, trees |
| Overture buildings | `https://tiles.overturemaps.org/<release>/buildings.pmtiles` | 14 | heights only |
| OpenRailwayMap (app) | `railTilesUrl()` custom protocol | per rail doc; platforms z15 | tracks + platforms |
| NVDB road widths (app, Sweden) | backend `roadWidths` | – | key held by backend |
| Land cover | NMD WMS; Planetary Computer WorldCover | tile z10-14 | 256² class rasters |
| Canopy height | source.coop COGs | file z10 | range requests |
| Street trees | `static/street-trees/{malmo,umea}.bin` | – | bundled |
| Elevation | Mapterhorn WebP, Mapzen PNG | 0-16 / 0-15 | terrarium |
| Clouds | EUMETView WMS JPEG | – | hourly |
| Weather | backend `weather_at` (MET Norway) | – | 10 min |
| Textures | `static/realistic/*.ktx2` | – | bundled |

---

## 16. Porting to wgpu / WGSL

### 16.1 Suggested structure

- One WGSL "library" source for `rlight` (struct + functions) concatenated (or `naga_oil`/`wesl` `#import`) into each pipeline's shader: `ground.wgsl` (areas + lines variants via a `const IS_LINE` override or two entry points), `building.wgsl` (walls, flat roofs, roof meshes — one fragment function, three vertex entry points), `tree.wgsl`, `tree_shadow.wgsl`, `overlay.wgsl` (haze fade / foot shade / plain shadow), `cloud.wgsl`, `terrain.wgsl`, `sky.wgsl`.
- Bind group 0: `rlight` uniform (per frame; per-draw `origin`/`cloudOrigin` go in a small per-tile uniform or instance data, since they depend on each tile's origin). Bind group 1: texture arrays + samplers for the pipeline. Bind group 2: per-tile transform.
- Vertex shader outputs replace deck's hooks: compute `rl_eye`, `rl_sun`, `rl_t`, `rl_c` from the tile-local position, tile origin and camera exactly as §4.2.
- Encode per-feature data explicitly instead of in colour alpha: ground `surface_id: u32` + `tint: vec3`; buildings `facade: u32, roof: u32, tint`; trees `layer: u32, retint: u32`. Keep `flat` interpolation (`@interpolate(flat)`).
- Coordinates: build tile meshes in f32 relative to a per-tile f64 origin (Mercator metres or common units), pass `view_proj · translate(origin − camera)` computed in f64 on the CPU. This replaces deck's `commonOrigin`/fp64 handling.

### 16.2 GLSL → WGSL translation notes

| GLSL | WGSL | Catch |
|---|---|---|
| `texture(arr, vec3(uv, layer), bias)` | `textureSampleBias(t, s, uv, i32(layer), bias)` | array index is a separate integer argument |
| `textureLod(arr, vec3(.5,.5,layer), 16.0)` | `textureSampleLevel(t, s, vec2(.5), i32(layer), 16.0)` — or better a uniform array of precomputed mean colours | level 16 > mip count relies on clamping; BC7/ASTC last mips are 4×4 blocks of a 1×1 image: fine, but a CPU-side mean is exact and cheaper |
| `dFdx/dFdy/fwidth` | `dpdx/dpdy/fwidth` | **must be called in uniform control flow** |
| implicit-LOD `texture()` inside `if` | not allowed in non-uniform control flow | the ground and building shaders sample inside branches on per-fragment ids. Fix: compute UVs for all branches, take derivatives (`dpdx(uv)`, `dpdy(uv)`) before branching and use `textureSampleGrad`, or sample unconditionally and `select()`; naga rejects/warns otherwise. This is the largest mechanical change |
| `mod(x, y)` | `x - y * floor(x / y)` | WGSL `%` truncates (differs for negatives; `rl_hash` uses `mod(cell, n)` on possibly negative cells) |
| mutable global `rl_glint` | function parameter | no mutable module-scope vars shared that way in fragment stage without `var<private>`; a parameter is cleaner |
| `const float A[42] = float[42](…)` | `const A = array<f32, 42>(…)`; index with `u32` | or pack the surface table into a uniform/storage buffer (`array<SurfaceParams, 42>`) so Rust owns the table |
| `mat2(cos a, sin a, −sin a, cos a) * uv` | `mat2x2<f32>(vec2(cos a, sin a), vec2(−sin a, cos a)) * uv` | column-major in both |
| `discard` | `discard` | fine; derivative use after discard is OK in WGSL (demote-to-helper semantics) |
| `inversesqrt`, `smoothstep`, `reflect`, `exp2` | same names (`inverseSqrt`) | `smoothstep(e0, e1, x)` with `e0 > e1` (used: `smoothstep(0.0, -0.2, up)`, `smoothstep(0.95, 0.35, uv.y)`) is **undefined in WGSL** (and formally in GLSL, though drivers do the obvious thing). Rewrite as `1.0 - smoothstep(e1, e0, x)` |
| `pow(x, 900.0)`, `pow(max(c,0), 2.2)` | same | keep the `max` guards; `pow` of negative base is undefined |
| `layout(std140) uniform` | `struct` + `var<uniform>` | offsets match as written; struct size 192 bytes (`globe` @160, `cover` @164, `eye` @176) — use `encase`/`bytemuck` with explicit padding |
| sampler2DArray + sampler2D with shared sampler state | separate `texture_2d_array<f32>` and `sampler` bindings | anisotropy 8 requires linear min/mag/mip in wgpu |
| sRGB handling | sRGB texture formats for colour arrays; output gamma applied manually | render target for these passes must be non-sRGB, or drop `pow(…, 1/2.2)` and accept the small curve difference |

### 16.3 Hardest parts, ranked

1. **Terrain draping + offsets** (§14.3/14.5): deck.gl's TerrainExtension does a lot implicitly (cover FBOs per terrain tile, height-map pass, cache invalidation). Needs an explicit design in the native renderer; affects every other layer.
2. **Ground shader under WGSL's uniformity rules**: branches on surface id with texture samples, Voronoi loop with hashes, many `fwidth`-driven fades. Restructure with `textureSampleGrad` and pre-branch derivatives; test on all backends (Metal, Vulkan, DX12, GLES on Android).
3. **Vector tile geometry pipeline that deck.gl provided for free**: MVT decode, polygon triangulation (earcut), wide-line tessellation with round joins/caps and metre widths + min pixel widths, extruded polygon walls/roofs with per-wall `pos/nextPos` (walls should be generated as explicit quads with per-vertex `(along, up)`, wall length and building height — simpler than deck's instanced sides), bridges split. All per tile on worker threads.
4. **LOD tileset** with multi-source zooms, refinement without overlap, look-ahead and request prioritisation (§13).
5. **Tree normal reconstruction** from screen derivatives and reoriented normal mapping — direct to translate, but sensitive to handedness/UV-flip conventions; compare against screenshots.
6. **KTX2/Basis transcoding in Rust** on all five platforms (§5.4), including ETC1S for the Earth pictures.
7. **Globe mode parity** (`rl_frame`, per-fragment day/night, Mercator-matched `rl_t` from lng/lat, night lights): depends on how the native globe is built (doc 01); keep `globe` uniform semantics.
8. **Stencil shadows**: easy in wgpu, but requires a stencil-capable depth attachment shared by all passes and the exact draw order (foot → shadow → building per tile; tree shadows; aircraft shadows).
9. **Sky background and rain** move from CSS/canvas to GPU/egui: straightforward.
10. External rasters: COG range reads (canopy), WMS PNG class decoding, PMTiles — all doable in Rust (`reqwest` range requests, `pmtiles` crate, `tiff`), best placed in the existing backend with caching.

### 16.4 Suggested parity order

1. `Look` + `rlight` WGSL + sky pass + flat untextured ground (LAND rectangle) → verify colours against `displayRgb`.
2. Ground areas + lines from OpenMapTiles with textures; water; weather presets.
3. LOD tileset; Streets GL ground; buildings (flat roofs, walls, windows, night glow); stencil shadows + foot shade.
4. Trees + shadows; pitched roofs; Overture heights; land cover; NVDB widths; canopy/registered trees.
5. Hillshade; 3D terrain + draping; camera ground follow.
6. Globe-mode realistic ground (Earth picture, night lights, clouds); rain overlay.

---

## 17. Open questions / risks

1. **Far building tiles (z13)**: `roofColours` is implemented (`BL:153,197-210`, `R:1100`) but never switched on in `Globe.svelte`; today far tiles are flat polygons in the *wall* colour with alpha `255 − code` and no realistic shading (the building extension is disabled for non-extruded layers). Port as-is, or implement the evident intent (flat, roof-coloured, hazed)? Recommend the latter; needs the owner's nod if strict pixel parity is wanted.
2. **Path UV convention** for railway sleepers/pier boards (`geometry.uv.y` units and restart per segment) should be verified visually against the running app when the line tessellator is written.
3. **Draped-cover caching**: with 3D terrain on, animated effects (water ripples, cloud shadows) are frozen between cover re-renders today. A native renderer that drapes by displacement would animate them — a visible (better) difference; acceptable?
4. **`rl_display` vs sRGB targets**: egui expects to render into either linear or sRGB framebuffers depending on backend config; the realistic passes output display-encoded values. Decide the surface format once (suggest non-sRGB surface view for map passes, egui configured accordingly).
5. **Basis/KTX2 in Rust**: confirm a crate that transcodes KTX2 UASTC+Zstd *and* ETC1S/BasisLZ to BC7/ASTC/ETC2/RGBA on Windows/macOS/Linux/Android/iOS (C++ build via `cc` for mobile targets), else pre-transcode at build time.
6. **Third-party endpoints with CORS-era assumptions** (Planetary Computer mosaic registration POST, EUMETView WMS, source.coop COGs, tiles.streets.gl, Mapterhorn 404 fallback): natively these should go through the Rust HTTP client with on-disk caching, polite User-Agent and rate limits; `tiles.streets.gl` is the Streets GL author's personal server (only z16) — availability risk, already true today.
7. **Mobile GPU limits**: 23-layer 512² BC7/ASTC arrays ×2, 16-layer ×3 (+glow), Earth 4096×2048 ×2 ≈ fine with compression, but ~4× larger in the RGBA8 fallback (≈ 110 MB with mips). `max_texture_array_layers` ≥ 256 everywhere; anisotropy 8 may be clamped on GLES.
8. **`smoothstep` with reversed edges** and implicit-derivative sampling in branches behave "as expected" on WebGL drivers today but are undefined/illegal in WGSL — every occurrence must be rewritten (§16.2); screenshots should be compared after.
9. **MVT decode parity**: several rules depend on loaders.gl's *binary* decoding quirk that absent numeric properties read as 0 (`some()`); a Rust MVT decoder gives real absence — implement `some()` as "absent or 0 → none" and keep `minLevel` handling (`SG:130-131`).
10. **Time uniform**: `time` wraps at 100 000 s; water/cloud drift jumps once every ~27.8 h (existing behaviour).
11. **Tree count**: up to 24 000 instances per tile × tiles within 4.8 km, plus an equal number of shadow instances, all alpha-tested quads with heavy fragment shading — check fill-rate on mobile; may need a distance cut or impostor LOD (not present today).
