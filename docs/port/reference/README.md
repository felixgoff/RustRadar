# Reference pictures of today's app

Pictures of the Svelte + deck.gl app as it is before the port (plan item S1.33), for the drawing and UI items to
compare their shots with by eye (plan section 8, "Visual review"). Twelve views across the three basemaps, each at
1440×900 (desktop layout) and 390×844 (phone layout), made by `scripts/port/reference-shots.mjs`.

Without the Rust backend there is no live data: no aircraft, ships or trains, and the sidebar shows the
"No live … data in a plain browser" alert. These are references for the **basemaps and the shell**, not for the
vehicle layers or the detail panels.

## The views

`V1`…`V8` are the named views of the plan's section 8 (`lat, lon, zoom, pitch, bearing`). The hash is what the
script puts after `http://127.0.0.1:1421/`; `basemap` and `mode` are what it stores in the settings
(`rustradar.v1` in `localStorage`, doc 06 §23) before the page loads. Files are `<basemap>-<view>-<w>x<h>.png`.

| Files | Hash | Basemap | Mode | Shows |
|---|---|---|---|---|
| `dark-v1-1440x900.png`, `dark-v1-390x844.png` | `#@35,10,1.9,0,0` | dark | aircraft | V1, the initial view: the whole globe on the space colour, borders, country and city names |
| `dark-v2-1440x900.png`, `dark-v2-390x844.png` | `#@52,10,4.5,0,0` | dark | aircraft | V2, Europe: land and sea, borders, roads, city and country names |
| `dark-v3-1440x900.png`, `dark-v3-390x844.png` | `#@59.33,18.07,9,0,0&demo` | dark | trains | V3, the Stockholm region: water, roads, place names, the airport code `BMA`. `#demo` draws nothing today (see below) |
| `dark-v4-1440x900.png`, `dark-v4-390x844.png` | `#@59.6519,17.9186,13.5,45,30` | dark | aircraft | V4, Arlanda pitched 45° and turned 30°: amber runways, taxiway lines, flat-colour buildings, `ARN` |
| `dark-v8-1440x900.png`, `dark-v8-390x844.png` | `#@0,179.5,3,0,0` | dark | aircraft | V8, the Pacific across ±180° at about 22:00 local time: the night shading over the globe, names on both sides of the antimeridian |
| `satellite-v2-1440x900.png`, `satellite-v2-390x844.png` | `#@52,10,4.5,0,0` | satellite | aircraft | V2 on Esri imagery: names only, no borders or roads (doc 01 §10, note 15) |
| `satellite-v6-1440x900.png`, `satellite-v6-390x844.png` | `#@57.7,11.93,13,0,0` | satellite | ships | V6, Gothenburg harbour on imagery, with the ships mode's shell: search placeholder, the "Ships in view" card with its sort control and "Couldn't reach VesselFinder" |
| `satellite-v7-1440x900.png`, `satellite-v7-390x844.png` | `#@78.2,15.6,6,0,0` | satellite | aircraft | V7, Svalbard: imagery far north, the globe's horizon at the top edge |
| `realistic-v1-1440x900.png`, `realistic-v1-390x844.png` | `#@35,10,1.9,0,0` | realistic | aircraft | V1 on the realistic map: the Blue Marble globe on the space colour, the sun's glint on the sea, the pale cap over the pole, names with dark outlines |
| `realistic-v3-1440x900.png`, `realistic-v3-390x844.png` | `#@59.33,18.07,9,0,0` | realistic | aircraft | V3 on the realistic map: water, woods, farmland, towns and roads on the ground, outlined names |
| `realistic-v4-1440x900.png`, `realistic-v4-390x844.png` | `#@59.6519,17.9186,13.5,45,30` | realistic | aircraft | V4 on the realistic map: fields, woods and lakes from the land cover, runways and aprons, lit buildings |
| `realistic-v5-1440x900.png`, `realistic-v5-390x844.png` | `#@59.3293,18.0686,15.5,60,20&demo` | realistic | trains | V5, Stockholm city pitched 60° at noon: buildings with roofs and façades, trees, parks, water, Streets GL ground |

## How they were made

- `node scripts/port/reference-shots.mjs` starts `bunx vite dev --port 1421 --strictPort`, opens each view in
  Playwright's Chromium (headless shell, `--use-angle=swiftshader --enable-unsafe-swiftshader`, so WebGL runs on
  SwiftShader), and stops the server at the end. `--url` uses a server already running; `--only dark-v1,…` and
  `--sizes desktop|phone` redo a few. The whole set takes about half an hour here. The dev server is needed because
  the hashes only work in dev builds (`import.meta.env.DEV`, doc 06 §3.15); the dev readout (doc 06 §2.7) is
  therefore in every picture, top right.
- Device scale factor 1 for both sizes; the phone context is a touch device (`isMobile`, `hasTouch`). The layout
  follows the width only (`PHONE_QUERY`, doc 06 §2.2), so 390×844 shows the phone layout: the top bar with search
  and mode bar, the bottom sheet at its peek height, the map controls above it.
- Settings: the defaults (doc 06 §23) with `basemap` and `mode` from the table and `sky: "sunny"`, so the realistic
  map's weather is a preset and not the day's forecast from MET Norway.
- Clock: the page's `Date` runs from **2026-06-21 10:00 UTC** (midsummer, noon in Stockholm), so the sun, the
  day/night shading, the season of the trees and the realistic map's light are the same on every run.
  `performance.now` and the animation frames run at their own pace. Time zone `Europe/Stockholm`, locale `en-US`.
- `https://view.eumetsat.int/**` is blocked, so the EUMETSAT cloud picture does not hide the realistic globe (its
  credit still shows in the attribution). All other tile hosts are fetched live (OpenFreeMap, Esri, Streets GL,
  Overture, Mapzen terrarium, Meta canopy, Naturvårdsverket, Planetary Computer), so a later run can differ where
  the data has changed.
- A view is taken once the page has loaded and the map canvas is in `<main>`, at least 15 s have passed, the
  network has been quiet for 5 s and four more frames have been drawn (or nothing has been redrawn for 20 s), at
  most 150 s. Then the page's `requestAnimationFrame` is parked and the frames already queued are let finish before
  the capture: SwiftShader draws a frame in one to three seconds, and a capture taken while the map is still
  drawing waits behind every queued frame (it timed out after 120 s on V5). CSS animations (the skeleton shimmer)
  are stopped for the capture. The dev server now and then answers a module with an error and serves its "500
  Internal Error" page instead of the app; the script then tries the view again, up to three times.
- Pictures whose PNG came out larger than 800 kB are stored as 256-colour PNGs with Floyd–Steinberg dithering
  (ImageMagick), to keep the repository small. That is the 1440×900 pictures of `satellite-v2`, `satellite-v6`,
  `satellite-v7`, `realistic-v3`, `realistic-v4` and `realistic-v5`: compare hues and shapes in those, not exact
  pixel values. All the others, and every phone picture, are exact.

## What differs from the app on a desktop or a phone

- No backend: no vehicles, skeleton rows under "Most tracked", the alert in the sidebar, `0 aircraft · 0 trains ·
  0 ships` in the readout.
- **`#demo` draws nothing in today's app.** The demo trains (`src/lib/demo-trains.ts`) need the trains mode, but in a
  plain browser the Trafikverket key check answers "none" at start and turns the trains mode back to aircraft
  (`+page.svelte`, `checkTrafikverketKey` and the effect after it), and the mode bar then offers no trains mode. So
  `dark-v3` and `realistic-v5` keep `&demo` in their hash and `trains` in their settings, and show the aircraft
  shell without trains. There are no reference pictures of trains.
- The realistic ground draws the vector map's own railways (`OWN_RAILS` in `realistic-ground.ts` is false outside
  the app) and has no road widths from Trafikverket NVDB (`road-widths.ts` asks the backend). Land cover comes live
  from Naturvårdsverket; a request that fails (seen once during these runs, as a CORS error in the console) falls
  back to ESA WorldCover for that tile, as in the app.
- The fps figure in the readout is SwiftShader's (0 to about 60 depending on view and size) and says nothing about
  the app's speed.
- Labels are placed by deck.gl's collision pass as it settles; a few overlap or are half faded where the capture
  caught them.
