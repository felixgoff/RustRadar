# 06 — UI specification (everything except the globe)

Source of truth: `src/routes/+page.svelte`, `src/routes/+layout.svelte`, `src/routes/+layout.ts`, `src/app.css`,
`src/app.html` and every `src/lib/*.svelte` except `Globe.svelte` (documented in 01–04). File:line references are to
the tree on branch `rust-port` before the port. All sizes are CSS px (logical points: multiply by the window scale
factor). "px" in egui terms = `points`.

Reading guide: sections 1–3 are global (tokens, shell, state machine); sections 4+ are one per component.

Contents (sections are numbered by component, and appear in this order in the file):
1 Design tokens · 2 App shell and layouts · 3 App-level state machine · 4 Detail panels: shared anatomy ·
7 FlightPanel · 8 TrainPanel / ShipPanel / AirportPanel · 5 SearchBox · 6 ModeBar + Segmented · 9 AltitudeChart ·
10 ExportMenu · 11 FilterPanel + TokenInput · 12 TrainFilterPanel + FilterChips + ToggleRow · 13 ShipFilterPanel ·
17 BottomSheet · 14 TopFlights · 15 TrainList · 16 ShipList · 18 HoverCard + RainOverlay · 19 MapControls +
LayersMenu + SettingsMenu · 20 KeysDialog · 21 ShortcutsHelp · 22 UpdateToast (+ updater splash) · 23 Persisted
settings · 24 Helper functions and strings · 25 DevPanel · 26 Icons · 27 Animations · 28 Z-order · 29 egui notes ·
30 Open questions and risks.

---

## 1. Design tokens (`src/app.css`)

Theme intent (app.css:1-7): Photoshop dark UI / Adobe Spectrum. Grey panels on a darker canvas, 4 px corners, white
aircraft, Spectrum blue for selection and primary actions, orange for delays, green only for "live", red only for
failures. Overpass for UI, Overpass Mono for measured values (altitudes, speeds, codes, times).

### 1.1 Colours (app.css:18-42)

| Token | CSS | RGBA (0-255, alpha 0-1) | Use |
|---|---|---|---|
| `--ink` | `#1e1e1e` | 30,30,30,1 | window background, canvas behind globe, "filed route" code box, toggle knob (on) |
| `--surface` | `#323232` | 50,50,50,1 | every floating panel |
| `--surface-raised` | `#3e3e3e` | 62,62,62,1 | inputs, neutral badge, skeleton base, photo placeholder, kbd |
| `--surface-hover` | `#4a4a4a` | 74,74,74,1 | hover of rows/buttons, skeleton highlight |
| `--line` | `#444444` | 68,68,68,1 | panel borders, dividers |
| `--line-control` | `#6e6e6e` | 110,110,110,1 | control borders (button, input, toggle off track), scrollbar thumb |
| `--text-1` | `#e3e3e3` | 227,227,227,1 | primary text |
| `--text-2` | `#c4c4c4` | 196,196,196,1 | secondary text, default icon colour |
| `--text-3` | `#a8a8a8` | 168,168,168,1 | tertiary text, labels (dt), placeholders |
| `--amber` | `#378ef0` | 55,142,240,1 | **the accent (Spectrum blue, despite the name)**: selection, focus ring, primary buttons, caret |
| `--amber-ink` | `#ffffff` | 255,255,255,1 | text on accent |
| `--amber-soft` | `rgb(55 142 240 / 0.18)` | 55,142,240,0.18 | pressed/selected background |
| `--live` | `#33ab84` | 51,171,132,1 | "Live", early, good |
| `--live-soft` | `rgb(51 171 132 / 0.16)` | 51,171,132,0.16 | good badge bg |
| `--danger` | `#ec5b62` | 236,91,98,1 | failures |
| `--danger-soft` | `rgb(236 91 98 / 0.14)` | 236,91,98,0.14 | error alert / bad badge bg |
| `--warn` | `#e68619` | 230,134,25,1 | delays, stale data |
| `--warn-soft` | `rgb(230 134 25 / 0.16)` | 230,134,25,0.16 | warn alert / badge bg |
| `--series-1` | `#2680eb` | 38,128,235,1 | altitude chart line |

Other literal colours used by components (all `oklch` in source; sRGB approximations):

| Source | Approx RGBA | Where |
|---|---|---|
| `oklch(0.15 0 0 / 0.75)` | 11,11,11,0.75 | dev readout bg (+page:1905) |
| `oklch(0.15 0 0 / 0.6)` | 11,11,11,0.60 | attribution strip bg (+page:1918) |
| `oklch(0.15 0 0 / 0.7)` | 11,11,11,0.70 | photo prev/next round buttons (FlightPanel:826) |
| `oklch(0 0 0 / 0.9)` | 0,0,0,0.9 | text-shadow `0 1px 2px` on photo credit/count |
| `oklch(1 0 0 / 0.25)` | 255,255,255,0.25 | attribution link underline |
| `oklch(0.95 0.01 85 / 0.6)` | ~241,238,230,0.6 | legend swatch "Filed route" |
| `oklch(0.95 0.01 85 / 0.45)` | ~241,238,230,0.45 | legend swatch "estimated departure" dashes |
| `oklch(0.96 0 0)` | 242,242,242,1 | light tile behind train operator logos |
| `oklch(0.8 0.15 152 / 0.6 → 0)` | ~108,214,140 | "Live" dot pulse ring |

Root: `color-scheme: dark`, `accent-color` and `caret-color` = accent; text selection = accent bg with white text
(app.css:73-89).

### 1.2 Typography (app.css:44-53, 68-70, 124-128)

- UI font `--font-ui`: `"Middle Dot", "Overpass Variable", "Segoe UI", system-ui, sans-serif`. Fonts come from npm
  packages `@fontsource-variable/overpass` and `@fontsource-variable/overpass-mono` (+layout.svelte:2-3): **variable**
  fonts, weight axis 100–900. Weights actually used: 400 (default), 500, 550, 600, 650, 700, 750. In egui there are
  no variable-weight fonts: ship static instances (suggest 400, 500/550→Medium, 600/650→SemiBold, 700/750→Bold) of
  Overpass and Overpass Mono (400, 600/650, 700).
- "Middle Dot" (app.css:8-16): a `@font-face` mapping only U+00B7 `·` to a local system font, because Overpass's
  middle dot has no advance width so `" · "` separators collide. In the port: add a fallback font for U+00B7, or draw
  separators with explicit spacing. The `" · "` separator is used all over (attribution, details, rows).
- Data font `--font-data` (class `.data`): `"Overpass Mono Variable", ui-monospace, "Cascadia Mono", monospace`, with
  `font-variant-numeric: tabular-nums` and `letter-spacing: -0.01em`.
- Root font size 16 px, line-height 1.4; `body` font-size 14 px (`--text-base`). `-webkit-font-smoothing: antialiased`.

Role scale:

| Token | px | Use |
|---|---|---|
| `--text-xs` | 11 | captions, badges, notes |
| `--text-sm` | 12 | labels, secondary |
| `--text-md` | 13 | list rows, data, buttons |
| `--text-base` | 14 | inputs, body |
| `--text-lg` | 17 | wordmark |
| `--text-xl` | 22 | callsign, airport codes |

Outliers: attribution 8 px (7 px on phones), dev readout 10 px/1.35 monospace, altitude-chart axis text (see §9).

### 1.3 Spacing, radii, shadow, easing (app.css:55-66)

- Spacing: `--space-1..6` = 4, 8, 12, 16, 20, 24 px.
- Radius: `--radius` = `--radius-sm` = **4 px** everywhere (exceptions: round dots/knobs 50 %, active-count pill 9 px,
  tab underline 1 px, legend swatch 2 px, train swatch 2 px, train logo tile 3 px, bottom-sheet top corners — see §17).
- `--shadow: none`. Panels have **no drop shadow and no backdrop blur**; they are opaque `--surface` with a 1 px
  `--line` border (`.surface`, app.css:140-145). (The few `box-shadow`s that exist are component-local: see each.)
- `--ease-out: cubic-bezier(0.16, 1, 0.3, 1)` — every UI transition uses it, mostly at 150 ms.
- Focus ring (`:focus-visible`): 2 px solid accent, offset 2 px (app.css:91-94).
- Scrollbars: thin, thumb `--line-control`, track transparent (app.css:96-99).
- `prefers-reduced-motion: reduce`: all animations/transitions collapse to 0.01 ms, 1 iteration (app.css:242-250).

### 1.4 Shared control classes (app.css:147-240)

- **`.surface`**: bg `--surface`, 1 px border `--line`, radius 4.
- **`.icon-button`**: 32×32, no border, radius 4, transparent, icon colour `--text-2`. Hover: bg `--surface-hover`,
  colour `--text-1`. Pressed (`aria-pressed=true`): bg `--amber-soft`, colour accent. Disabled: `--text-3`, opacity
  0.5. Transitions 150 ms on bg + colour.
- **`.button`**: inline-flex, gap 8, height 32, padding 0 12, 1 px border `--line-control`, radius 4, transparent bg,
  13 px weight 550, text `--text-1`. Hover bg `--surface-hover`. Pressed toggle: border accent, bg `--amber-soft`,
  text accent.
- **`.badge`**: padding 1 px 6 px, radius 4, 11 px weight 650, line-height 1.5, nowrap. Tones: `good` (`--live-soft`
  bg / `--live` text), `warn` (`--warn-soft` / `--warn`), `bad` (`--danger-soft` / `--danger`), `neutral`
  (`--surface-raised` / `--text-2`).
- **`.skeleton`**: radius 4; horizontal gradient `--surface-raised` → `--surface-hover` (50 %) → `--surface-raised`,
  `background-size: 200% 100%`, animated by shifting background-position from 0 to −200 % over **1.4 s linear
  infinite** (a shimmer sweeping right-to-left).
- **`.visually-hidden`**: screen-reader-only live regions; not needed visually (egui: feed AccessKit if desired).

---

## 2. App shell

### 2.1 `app.html` / `+layout.*`

- Title "RustRadar"; favicon `static/favicon.png`. Viewport never zooms (pinch only zooms the map); iOS
  `gesturestart/change/end` are cancelled (app.html:7-16). Port: only the map handles pinch.
- `+layout.ts`: `ssr = false` (SPA) — irrelevant to the port.
- Main window (`src-tauri/tauri.conf.json`): title `RustRadar`, 1440×900, **min 900×600**, starts maximized, created
  hidden and shown once ready, background `#04060b` (4,6,11) before the first frame. Note that the minimum width
  900 means the desktop window never reaches the phone breakpoint (≤ 899) by resizing; the phone layout is for
  Android/iOS (and for short windows ≤ 500 px high, which the 600 minimum also prevents on desktop).
- `+layout.svelte`: imports fonts + `app.css`, renders the page and `<UpdateToast />` (§22). The browser context menu
  is suppressed everywhere except inside `input, textarea, [contenteditable]` (so copy/paste works there);
  right-drag on the map rotates/tilts (+layout.svelte:9-15).

### 2.2 Phone breakpoint (`src/lib/phone.svelte.ts`)

`PHONE_QUERY = "(max-width: 899px), (max-height: 500px)"` → `layout.phone`. Live (re-evaluated on resize). Every
component's CSS uses the same query. A third variant: **phone landscape** = `(max-height: 500px) and
(orientation: landscape)`.

### 2.3 Desktop layout (window ≥ 900 px wide and > 500 px tall)

`main` is `position: fixed; inset: 0; background: --ink` (+page:1762-1766). The globe fills it. Everything else
floats above, absolutely positioned:

```
+--------------------------------------------------------------------------------------+
| 16px                  [ ModeBar: top 16, centred between x=352 and   [dev readout,    |
| +-- sidebar 320px --+   right=352; one horizontal row ]               top 8 right 8]  |
| | primary card      |                                                                |
| |  brand / status   |                                                                |
| |  search           |                                                                |
| |  Filters | counts |                                                                |
| |  alert/note       |                                                                |
| +-------------------+                                                +- detail 360 -+ |
| 12px gap                                                             | flight/train/ | |
| +-------------------+                                                | ship/airport  | |
| | filter panel (opt)|                                                | panel         | |
| +-------------------+               [MapControls: right 16+inset,    | right 16      | |
| | list card         |                bottom 28, column]              | bottom 28     | |
| +-------------------+                                                +---------------+ |
| bottom 40px                                 [attribution strip, right 0 bottom 0]      |
+--------------------------------------------------------------------------------------+
```

- **Sidebar** (+page:1767-1797): `top 16; left 16; bottom 40; width 320`; flex column, gap 12; `pointer-events: none`
  (children sections take events, so the map is draggable through the gaps).
  - `.primary` card: `flex: none`, column, gap 12, padding 16.
  - filter panel (`.filters`, when open): `flex-shrink: 1` (see §11-13 for its own max-height).
  - `.list` card: `flex: 0 3 auto` (shrinks three times as fast as the filter panel: "with filters open, the list
    gives way first"), `min-height: 96`, `overflow-y: auto`, padding `12px 8px`.
- **ModeBar** (§6): a wrapper at `top 16; left 352; right 352` (`320 + 2·16`), `justify-content: center`, no pointer
  events; the bar itself is a horizontal pill-row centred in it.
- **Detail panel** (Flight/Train/Ship/Airport): `right 16; bottom 28; width 360; max-height: calc(100% - 16 - 28)`;
  anchored to the bottom, grows upward; scrolls internally. Enter animation: 200 ms ease-out from
  `opacity 0, translateX(8px)`.
- **MapControls** (§19): `right: 16 + inset; bottom: 28`, where `inset = 372` (`PANEL_INSET = 360 + 12`, +page:109)
  when any detail panel is open on desktop, else 0; `right` transitions 200 ms ease-out.
- **Attribution** (§2.6): `right 0; bottom 0; max-width: calc(100% - 352px)`.
- **Dev readout** (dev builds only): `top 8; right 8` (§2.7).
- Dialogs (KeysDialog, ShortcutsHelp) are centred modals (§20, §21). UpdateToast §22.

### 2.4 Phone layout (portrait)

- `.sidebar` becomes a **top bar**: `top: max(8, safe-area-top); left: max(8, safe-area-left); right: max(8,
  safe-area-right); bottom: auto; width: auto; z-index 3` (+page:1938-1949).
- `.primary` becomes one wrapping row: `flex-flow: row wrap; align-items: center; padding 4; gap 4`. Contents: search
  (flex 1, min-width 0) + ModeBar (rendered **inside** the card after the search, horizontal). The `.brand` header
  (wordmark + status) is hidden. Alerts/notes take a full row below (`flex-basis: 100%`).
- When a detail is open (`main.detail`), the top bar slides away: `transform: translateY(calc(-100% - 8px));
  opacity 0; visibility hidden`, 220 ms ease-out (visibility flips after 220 ms). It slides back the same way.
- A single **BottomSheet** (§17) holds either the list view (filter panel if open + the list, with the toolbar as the
  sheet "head") or the open detail panel(s). Sheet label: `"Details"` when a detail is open, else `"Flights"` /
  `"Trains"` / `"Ships"`. `clear` prop: 12 when detail open, 64 otherwise. `head` = toolbar snippet only for the list.
- Toolbar on phone: `flex-direction: row-reverse` (count on the left, Filters button on the right), `.count` 14 px,
  the noun ("flights"/"trains"/"ships") is shown (it is `display: none` on desktop).
- `.list` loses its card chrome (no border/radius/bg, `overflow: visible`, `flex: 1 1 auto`); filter panel inside
  the sheet: `flex: none; margin: 0 8px 8px; no shadow`.
- Detail panels lose their chrome (static position, no border/bg/radius, no enter animation, `flex: 1; min-height:
  0`) and reorder so the summary (`data-peek` parts) comes first — see each panel.
- MapControls: see §19 (moves above the sheet, right edge).
- Attribution: `bottom: var(--sheet-h)` (sits just above the sheet; `--sheet-h` is the sheet's current height in px,
  set on `main`), `left 0`, full width, no radius, **7 px**, z-index 2.

### 2.5 Phone landscape (`max-height: 500px` and landscape)

Sidebar: `right: auto; width: 320` (down the left). Attribution: `bottom 0; left auto; max-width: calc(100% - 360px)`.
BottomSheet and MapControls have their own landscape rules (§17, §19).

### 2.6 Attribution strip (+page:1269-1299, 1753-1758, 1911-1934)

One line, `white-space: nowrap; overflow: hidden; text-overflow: ellipsis`; padding `1px 4px`; top-left radius 4;
bg rgba(11,11,11,0.6); 8 px, line-height 1.3, colour `--text-3`. Tooltip (`title`) = the full text. Items joined by
`" · "`. **Train credits come first** (so the ellipsis cannot hide them), as links (colour `--text-2`, underline
white 25 %, offset 2 px; hover `--text-1`; click opens the URL in the system browser). Train credits =
`trainCredits(trainsOnMap)` when trains are on the map (see §24.4).

Then, in this order, each included only if its condition holds:

| # | Condition | Text |
|---|---|---|
| 1 | aircraft on map | `Flight data: Flightradar24 (unofficial API)` or `…(unofficial API, signed in)` when authenticated |
| 2 | `settings.weather` | `Radar: NOAA MRMS, Deutscher Wetterdienst, RainViewer` |
| 3 | basemap = satellite | `Imagery: Esri, Maxar, Earthstar Geographics` |
| 4 | OpenSky points merged into the trail (`merged.added > 0`) | `Track: OpenSky Network` |
| 5 | adsb.lol measured data on show | `Aircraft data: © adsb.lol contributors, ODbL` |
| 6 | ships on map | `Ships © VesselFinder` |
| 7 | `settings.railMap` (and running in the app) | `RAIL_SPEED_ATTRIBUTION` (rail-layer.ts) |
| 8 | `settings.terrain` | `TERRAIN_ATTRIBUTION` (terrain-layer.ts) |
| 9 | basemap = realistic | `Textures: Streets GL (MIT)` |
| 10 | realistic and zoom ≥ `SGL_ZOOM − 0.5` | `SGL_ATTRIBUTION` (streets-gl.ts) |
| 11 | realistic and zoom ≥ `SGL_ZOOM − 0.5` | `OVERTURE_ATTRIBUTION` (overture.ts) |
| 12 | realistic and zoom ≥ 12 | `LANDCOVER_ATTRIBUTION` (landcover.ts) |
| 13 | realistic and zoom ≥ 14.5 | `` `${CANOPY_ATTRIBUTION} · ${STREET_TREES_ATTRIBUTION}` `` |
| 14 | realistic and zoom ≥ 14 (app) | `Road widths: Trafikverket NVDB` |
| 15 | realistic | `Earth: NASA Blue Marble, Black Marble` if `settings.daylight` else `Earth: NASA Blue Marble` |
| 16 | realistic and zoom < 11 | `CLOUDS_ATTRIBUTION` (clouds.ts) |
| 17 | live sky (realistic + sky = live) and weather loaded | `Weather: MET Norway` |
| 18 | always | `Map, names and buildings: OpenFreeMap, © OpenMapTiles, © OpenStreetMap contributors` |

The exact strings of the imported constants are listed in §24.5.

### 2.7 Dev readout (dev builds only, +page:1745-1751, 1898-1910)

`<pre>` top 8 / right 8, padding `4px 6px`, radius 4, bg rgba(11,11,11,0.75), `--text-2`, 10 px / 1.35 monospace,
no pointer events, z 5. Four lines:

```
z {zoom.toFixed(2)}  pitch {pitch.toFixed(0)}°  bearing {bearing.toFixed(0)}°
{lat.toFixed(5)}, {lon.toFixed(5)}
{basemap}[ · Streets GL if realistic and zoom ≥ SGL_ZOOM−0.5]  {fps} fps
{flights} aircraft · {trains} trains · {ships} ships        (counts with thousands separators)
```

FPS = frames counted per ≥1000 ms window, rounded.

### 2.8 Sidebar primary card (+page:1432-1519)

1. **Brand header** (desktop only): flex row, space-between, gap 12.
   - `h1`: icon `Radar` (20 px, stroke 1.75, accent colour) + text `RustRadar`; 17 px, weight 750, letter-spacing
     −0.01em, gap 8.
   - Status `p.status` (12 px, tabular-nums, `--text-3`, nowrap), right-aligned:
     - focus = aircraft:
       - snapshot present and `freshness.stale` → `<badge warn>{freshness.label}</badge>` (badge at 12 px here);
         tooltip `Last update at {HH:MM}` + `; updating now` if loading.
       - snapshot present, not stale → `Updating…` while a request is in flight, else `freshness.label`;
         tooltip `{snapshot.requests} requests in {snapshot.elapsedMs} ms`.
       - no snapshot and no `feedError` → skeleton 90×12.
     - focus = trains/ships: `listFreshness` → warn badge if stale, else plain label; if no answer yet (and in the
       app) → skeleton 90×12.
     - Freshness labels come from `feedFreshness(ageMs, intervalMs)` (§24.1).
2. **SearchBox** (§5) — one instance per focus (aircraft: remote only; trains/ships: with a local section and
   two-way bound query).
3. Desktop: **toolbar** row (flex, space-between, gap 12): Filters button + counts. Phone: the ModeBar here instead
   (toolbar moves to the sheet head).
   - Filters button: `.button`, icon `Funnel` 14/1.75, text `Filters`, then if N active filter groups > 0 a pill
     `.active-count` (min-width 18, height 18, padding 0 4, radius 9, accent bg, white 11 px weight 700). Pressed
     style when the filter panel is open. Tooltip `Filters (F)`. N = `activeCount(filters)` / `trainFilterCount` /
     `shipFilterCount` by focus.
   - Counts `p.counts` (12 px, tabular, `--text-2`, nowrap); the number `.count` is 13 px weight 650 `--text-1`:
     - aircraft (once a snapshot exists): `{filtered}` [`of {total}` when different] [`flights` phone only]
       `in view` if the snapshot was regional else `worldwide`.
     - trains (once `trainsAt`): `{listed}` [`of {total}`] [`trains`] `in view`.
     - ships (once `shipsAt`): `{listed}` [`of {total}`] [`ships`] `in view`.
     - Numbers use locale thousands separators.
4. **Alerts** (only one):
   - Not in the app (browser dev only): error alert `No live {flight|train|ship} data in a plain browser.` + detail
     ``RustRadar needs its Rust backend. Start it with `bun tauri dev`.`` — not applicable to the native port.
   - focus ≠ aircraft: nothing (the lists show their own failures).
   - `rateLimited`: **warn** alert `Flightradar24 is limiting requests. Slowing down; next update in {retryIn} s.`
   - `feedError`: **error** alert `Can't reach Flightradar24. Trying again in {retryIn} s.`, detail = the error
     string, and a `.button` `Retry now` (icon `RefreshCw` 14/1.75; disabled while loading) that calls `refresh()`.
   - else if `snapshot.errors.length = n > 0`: note (12 px, `--text-2`)
     `{n} region{s} didn't load this round; some aircraft may be missing.`
   - `retryIn = max(0, ceil((nextRefreshAt − now)/1000))`.
   - `.alert` style: flex gap 8, padding 12, radius 4, bg `--danger-soft`, icon colour `--danger` (`TriangleAlert`
     16/1.75, margin-top 1); `.warn` variant uses warn-soft/warn. Inner column gap 8, left-aligned; main text
     `--text-1` 13 px; `.detail` 12 px `--text-2`, word-break.

### 2.9 List card (+page:1567-1610)

One of TopFlights (§14), TrainList (§15), ShipList (§16) by focus. "Show all" in the train/ship lists clears the
list's search query and resets that kind's filters to defaults **keeping the current sort**.

### 2.10 Detail panel stacking (+page:1641-1699)

Rendered in this order (all at the same desktop position, so at most one is visible):

1. AirportPanel when `boardCode` is set — **kept mounted but hidden** while a flight is selected (so that going
   back keeps its tab and loaded pages).
2. FlightPanel when a flight is selected. Its `back` prop is `{ label: "{boardCode} departures and arrivals" }` when
   the flight was opened from the board and the board is still set; back = deselect the flight (board reappears).
3. ShipPanel when a ship is selected.
4. TrainPanel when a train is selected.

Selecting a flight closes ship+train; selecting a ship closes flight+train+board; selecting a train closes
flight+ship+board; opening an airport closes flight+ship+train. So really: one panel at a time, plus the
board-under-flight case.

---

## 3. App-level state machine (`+page.svelte` script)

### 3.1 Constants

| Name | Value | Meaning | Line |
|---|---|---|---|
| `WORLD_REFRESH_MS` | 15 000 | aircraft feed interval when the view covers the world (area = null) | 102 |
| `AREA_REFRESH_MS` | 8 000 | aircraft feed interval for a regional view | 103 |
| `MIN_GAP_MS` | 4 000 | minimum spacing between aircraft feed requests | 104 |
| `MAX_BACKOFF` | 6 | max multiplier of the interval under rate limiting (doubles: 1,2,4,6) | 105 |
| `TOP_REFRESH_MS` | 60 000 | most-tracked list | 106 |
| `TILT` | 55 | pitch for the `T` shortcut | 107 |
| `PANEL_INSET` | 372 | map-controls inset when a detail panel is open | 109 |
| `WEATHER_REFRESH_MS` | 600 000 | live weather refresh | 210 |
| `WEATHER_RETRY_MS` | 120 000 | after a weather failure | 211 |
| `WEATHER_MOVE_KM` | 15 | re-ask weather after moving this far | 212 |
| `OPENSKY_REFRESH_MS` | 300 000 | OpenSky track for the selected aircraft | 336 |
| `ADSB_POLL_MS` | 3 000 | adsb.lol poll for the selected aircraft; × min(2^failures, 16) | 358-359 |
| `ADSB_SHOW_MS` | 60 000 | measured data shown this long after the aircraft's last report | 361 |
| `SHIPS_SETTLE_MS` | 600 | camera-settle debounce before asking for ships | 420 |
| `SHIPS_REFRESH_MS` | 60 000 | ships refresh | 421 |
| `SHIP_DETAILS_REFRESH_MS` | 60 000 | selected ship's card | 483 |
| `TRAINS_SETTLE_MS` | 600 | camera-settle debounce for trains | 542 |
| `TRAINS_REFRESH_MS` | 10 000 | trains refresh | 543 |
| `LIVE_ZOOM` | 9 | at/above this zoom, trains in view are "watched" live | 545 |
| `MAX_LIVE` | 40 | max watched trains (nearest the centre first) | 547 |
| `TRAINS_LIVE_MS` | 1 500 | min spacing of reloads caused by `trains-changed` events | 610 |
| `ROUTE_LOADING_MS` | 2 500 | re-ask train route while incomplete | 687 |
| `ROUTE_REFRESH_MS` | 30 000 | re-ask train route when complete (and /3 = 10 000 after an error) | 688 |
| `ROUTE_OFF_MS` | 3 000 | min spacing of "train is off its route" re-asks | 690 |
| clock tick | 1 000 | `now` (drives ages, retry countdowns) | 1348 |
| Trafikverket key retry | 30 000 | when the status check itself fails | 174 |

### 3.2 State

Persisted (see §23): `settings`, `filters`, `trainFilters`, `shipFilters` — written to storage on every change.

Session state: `snapshot` (aircraft feed), `top`/`topFailed`, `details` (follow stream's `FlightDetails`),
`openskyPoints`, `selectedId: number|null`, `hint: LiveFlight|null`, `followState: "connecting"|"live"|
"reconnecting"|"ended"`, `endedSeenMs`, `relockOnResume`, `feedError`, `rateLimited`, `loading`, `nextRefreshAt`,
`shownFeed {at, interval}`, `regional`, `backoff`, `lastRequestAt`, `refreshAgain`, `fetchedArea`, `now`, `view
{longitude, latitude, zoom, bearing, pitch}` (initial `0,0,2,0,0`), `trafikverketKey: "none"|"invalid"|"ok"|null`,
`filtersOpen`, `helpOpen`, `keysOpen`, `menu: "layers"|"settings"|null`, `sheetDetent: "peek"|"half"|"full"`,
`sheetHeight`, `keysPrompt`, `session: SessionInfo|null`, `following` (camera follows aircraft), `followingVehicle`
(camera follows train/ship), `liveWeather`, `boardCode`, `fromBoard`, `flyPending`, `flyZoom`, `place`, `ships`,
`shipsAt`, `shipsFailed`, `pickedShip`, `shipCard {mmsi, details, error}`, `trains`, `trainsAt`, `trainsFailed`,
`pickedTrain`, `liveTrain`, `liveStops`, `shownRoute`, `trainQuery`, `shipQuery`, `hoveredTrainId`,
`hoveredShipMmsi`, `devOpen`, `adsb {id, aircraft, seenAt}`.

Derived: `kinds = shownKinds(settings)` → `{aircraft, trains, ships}` = focus OR in `alsoShow`; `focus =
settings.mode`; `trainsOffered = trafikverketKey === "ok"`; `authenticated = session?.authenticated ??
snapshot?.authenticated ?? false`; `near = zoom ≥ 5 ? {lat, lon of the view centre} : null` (search ranking);
`detailOpen = selected || boardCode || selectedShip || selectedTrain`.

### 3.3 Modes

- `settings.mode` ∈ `aircraft | trains | ships` — the **focus**: the sidebar's search, filters and list are for it,
  and it is always on the map. `settings.alsoShow` lists other kinds also fetched and drawn. `toggleShown(kind)`
  flips membership (no-op for the focus itself).
- Trains are only offered when the Trafikverket key check returned `"ok"`. The check (`trafikverket_key_status`)
  runs on mount and whenever the keys dialog closes; a failure of the check retries after 30 s. If it returns
  `"none"` or `"invalid"`: mode `trains` is forced to `aircraft` and `trains` is removed from `alsoShow`. While the
  result is still unknown (`null`), a persisted trains mode stays. ModeBar's `offered.trains = trainsOffered ||
  mode === "trains"`.
- When a kind leaves the map:
  - aircraft off: cancel the refresh timer, `nextRefreshAt = 0`, deselect the flight, close the board, drop
    `snapshot` and `shownFeed`. Back on: schedule a refresh at `max(0, lastRequestAt + MIN_GAP − now)`.
  - ships off: stop timers, bump the request counter, clear `ships`, `shipsAt`, `shipsFailed`; deselect the ship.
  - trains off: same for trains; `watch_trains([])`; deselect the train.
- Changing focus clears both list hovers.

### 3.4 Aircraft feed

- `refresh()`: skipped if already loading or aircraft are off. `area = viewArea(view, w, h)` (null = whole world);
  calls `liveFlights(area, serverCategories(filters))` (`serverCategories` = null when all or none of the 12
  categories are selected, else the list). On success: store snapshot; if `snapshot.authenticated` differs from
  `session.authenticated`, re-read `sessionInfo()`; `motion.syncClock(snapshot.serverTimeMs)`; `regional = area !==
  null`; `feedError = null`; `limited = snapshot.errors.some(isRateLimited)`. On failure: `feedError = String(e)`;
  `limited = isRateLimited(feedError)`. Finally: `rateLimited = limited`; `backoff = limited ? min(backoff*2, 6) :
  1`; `interval = (area ? 8000 : 15000) * backoff`; `shownFeed = afterRound(shownFeed, {arrived, at: now,
  interval})` (only a successful round moves the freshness clock); schedule the next refresh in `interval`, or in
  `MIN_GAP_MS` if another refresh was requested meanwhile and not limited.
- `refreshSoon()` (after sign-in/out, category filter change): no-op when backing off or aircraft are off; if
  loading, set `refreshAgain`; else schedule at `max(300, lastRequestAt + MIN_GAP − now)` ms.
- View-change trigger (+page:924-938): if not backing off and the view moved > 3° of arc from the fetched view's
  centre, or zoom changed by > 1.5, then after an 800 ms debounce schedule a refresh at
  `max(0, lastRequestAt + MIN_GAP − now)` if that is sooner than the pending one.
- Category filter change → `refreshSoon()` (categories are filtered server-side; all other filters client-side via
  `matcher(filters)`).
- `shown` (what the globe draws) = filtered flights with the selected flight substituted in (or appended when
  filtered out).
- Top flights: `topFlights()` whenever the focus becomes aircraft, and every 60 s while it is.
- On mount (app): `unfollowFlight()`, `reference.load()`, `sessionInfo()`, `refresh()`, start timers.

### 3.5 Flight selection and follow stream

`select(id, {hint?, fly?, follow?, fromBoard?})` (+page:1034-1068):

- Same id and no `fly`: if `followState === "ended"`, restart the follow stream; otherwise nothing.
- Otherwise: set `selectedId`; when non-null close ship and train; `details = null`; `hint`; `flyPending = false`;
  `following = opts.follow ?? false`; `relockOnResume = false`; `fromBoard`. If `id === null`:
  `unfollowFlight()` and stop. If `fly`: fly the camera to the known position now (`globe.flyTo(lon, lat)`), or set
  `flyPending` to fly when the first details arrive. Then `startFollow(id)`.
- **A click on the map selects without moving the camera**; picks from lists/search/history/board fly in. Only the
  panel's Follow button or `C` locks the camera.
- `startFollow(id, resuming)`: `followState = resuming ? "reconnecting" : "connecting"`; `followFlight(id, cb)`.
  Events (ignored if the selection changed): `details` → store, `followState = "live"`, re-lock the camera if
  `relockOnResume`, do the pending fly-to; `error` → `"reconnecting"`; anything else (ended) or a rejected call →
  `followEnded()`.
- `followEnded()`: `followState = "ended"`; `endedSeenMs = selected.timestampMs`; `relockOnResume ||= following`;
  `following = false`.
- Auto-resume: while ended, if the feed shows a newer `timestampMs` for the selected id than `endedSeenMs`,
  `startFollow(id, true)`.
- `toggleFollow()`: flips `following`, clears `relockOnResume`.
- `selected` (the `LiveFlight` shown): the feed's row (or the hint) unless the follow stream's
  `details.flight.timestamp_ms` is newer, in which case a row built from the stream (fields: lat, lon, track, alt,
  speed = ground_speed, onGround, timestampMs, callsign, and feed fallbacks for flight/reg/typecode/origin/
  destination/icon) (+page:277-299).
- OpenSky: once `details.aircraft.icao_address` is known (hex, 6 digits, lower case), `openskyTrack(hex)` now and
  every 5 min; merged into the trail by `mergeTrail`; `merged.added` drives a credit.
- adsb.lol: when `settings.adsb` and a flight is selected with a hex or callsign: poll `adsbHex(hex)` (or
  `adsbCallsign(callsign)`) every 3 s × backoff while the window is visible; match with `matchAdsb`; store with
  `seenAt = now − max(0, snap.nowMs − match.seenMs)`; `motion.measure(id, match)`. `measured` is shown while
  `now − seenAt < 60 s`. Deselecting / turning the setting off calls `motion.measure(id, null)`.
- Sign-in (`handleSignIn(email, password)` → `signIn`) and sign-out (`signOut`): store the session, `refreshSoon()`,
  restart the follow stream for the selected flight (to get the flight plan).

### 3.6 Airport board

`openAirport(code)`: `boardCode = code.toUpperCase()`; deselect flight, ship, train. Opened from: the globe's
airport click, FlightPanel's origin/destination codes, and a search result of type `airport` (which also flies to
`lon, lat` at zoom 11, and only opens the board if the id matches `^[A-Z0-9]{3}$`).

### 3.7 Search picks

- `pickSearch(entry)`: `airport` as above; `live` → `select(parseInt(entry.id, 16), {fly: true, hint: {id, lat,
  lon, callsign, everything else zero/empty}})`.
- `pickPlace(place)`: `following = false`; `place = p` (marked on the globe until dismissed);
  `globe.flyTo(placeView(p, w, h))`. A new query (`onquery`) clears the place marker. Any map selection clears it.
- `pickTrain(train)`: clear place, `following = false`, `selectTrain`, `flyTo(lon, lat, max(zoom, 10))`.
- `pickShip(ship)`: same with `max(zoom, 11)`.

### 3.8 Ships

- On every view change while ships are on: debounce 600 ms then `loadShips()`. `loadShips` re-arms a 60 s refresh
  timer first (kept apart from the settle timer so a never-settling camera still refreshes), skips when the window
  is hidden, calls `shipsInView(area, zoom)` with `area = viewArea(...)` or the whole world. The answer is dropped
  if a newer request was made, ships went off, the feed zoom step changed (`shipZoom(z) = clamp(floor(z + 1), 2,
  16)`), or the view centre moved away by more than half the visible radius
  (`cosAngle(...) < cos(radius·π/360)`). Failure: `shipsFailed = true`, ships on show stay.
- `shipSizesKnown = shipZoom(zoom) > 6`.
- Selected ship: `selectedShip` = the list's row for `pickedShip.mmsi`, else the last one seen. Card:
  `shipDetails(mmsi)` on selection and every 60 s while visible. A refresh keeps the card on show; only a new ship
  or a retry after an error shows skeletons; a failed refresh keeps the previous card.
- `shipsFiltered = ships.filter(shipFilter(shipFilters))`; `shipsListed` = + query + sort; `shipsOnMap` =
  filtered + the selection if filtered out.

### 3.9 Trains

- Same settle/refresh scheme with 600 ms / 10 s, `trainsInView(area, zoom)`, same staleness drop (without the zoom
  step). After each answer `watchInView(list, view)`: when trains are on and zoom ≥ 9, take the Trafikverket
  trains, sort nearest the view centre, take 40, sort ids, and call `watchTrains(ids)` only if the joined id key
  changed (else `[]`).
- Event `trains-changed` from the backend → `loadTrains()` at most every 1.5 s.
- `hideTrainOperators(hiddenOperators)` whenever that filter changes.
- `trainsFiltered = trains.filter(trainFilter(filters) && trainRunning(t, now))`.
- Selected train: `followTrain(id, cb)` while selected (events `{kind: "train", train}` → `liveTrain`, `{kind:
  "stops", stops}` → `liveStops`), `unfollowTrain()` on deselect. `selectedTrain` = `liveTrain` if same id and its
  `positionUnix` ≥ the list's, else the list's row, else the last seen.
- Route: for ids starting `trafikverket:`, `trainRoute(id)`; re-ask after 2.5 s while `!complete`, 30 s when
  complete, 10 s after an error; the globe's `ontrainrouteoff` callback re-asks at most every 3 s.
- `followingVehicle` resets to false whenever the selected train id or ship mmsi changes.
- Dev-only `#demo` hash → fake trains (not needed).

### 3.10 Live weather (realistic basemap, sky = live)

`weatherLive = basemap === "realistic" && sky === "live"`. On each view change and every 60 s tick call
`refreshWeather(lat, lon)`, which really fetches only when not busy and (no previous ask, or the last ask is older
than 10 min (2 min if it failed), or it succeeded and the view moved ≥ 15 km). `skyConditions` = live conditions
(null until loaded) or `PRESETS[sky]`.

### 3.11 "Close the top thing" (Escape, Android back) (+page:1133-1147)

In order, the first that applies: keys dialog → help → open map-controls menu → place marker → selected flight →
selected ship → selected train → airport board → filter panel → (phone) sheet not at peek → set to peek. Android:
a history entry is kept while anything is closable so the back gesture closes it instead of leaving the app; in the
port, handle the Android back button/gesture by calling `closeTop()` and only exit when it returns false.

### 3.12 Phone sheet detent rule (+page:1117-1131)

When a detail opens: remember the list's detent, set the sheet to `half` if it is an airport board without a
selected flight, else `peek`. When all details close: restore the list's detent.

### 3.13 Keyboard shortcuts (+page:1202-1265)

Ignored while typing in a text field, while the keys dialog is open, or with Ctrl/Cmd/Alt held. While the help
dialog is open only `?` (and Escape) work. Letters: Shift+letter is a distinct shortcut; Caps Lock alone counts as
the plain letter.

| Key | Action |
|---|---|
| `Esc` | `closeTop()` |
| `/` | focus the search box |
| `?` | toggle the shortcuts dialog |
| `f` | toggle the filter panel |
| `c` | flight selected: toggle camera follow; else train/ship selected: toggle `followingVehicle` |
| `t` | `setPitch(pitch > 5 ? 0 : 55)` |
| `n` | reset orientation (north up, no tilt) |
| `+` / `=` | `zoomBy(+1)` |
| `-` | `zoomBy(−1)` |
| `1` / `2` / `3` | basemap dark / satellite / realistic |
| `l` | toggle place labels |
| `b` | toggle 3D buildings |
| `p` / `i` / `s` | focus aircraft / trains (only if offered) / ships |
| `P` / `I` / `S` | toggle that kind also shown on the map (`I` only if trains offered) |
| `k` | toggle aircraft labels (only when aircraft are on the map) |
| `d` | toggle day and night |
| `w` | toggle weather radar |
| `r` | toggle rail map |
| `e` | toggle terrain |
| Ctrl/Cmd+Shift+D | dev builds: toggle DevPanel |
| Ctrl/Cmd+Alt+I | open the web inspector (`open_devtools`) — drop in the port, or map to an egui debug window |

### 3.14 Backend commands used by the UI (names as in `src/lib/api.ts`; see §24.6 for the wire names)

`liveFlights`, `topFlights`, `followFlight`/`unfollowFlight`, `openskyTrack`, `adsbHex`, `adsbCallsign`,
`sessionInfo`, `signIn`, `signOut`, `shipsInView`, `shipDetails`, `trainsInView`, `watchTrains`,
`hideTrainOperators`, `followTrain`/`unfollowTrain`, `trainRoute`, `trafikverketKeyStatus`, `open_devtools`;
components add `find`/`searchPlaces` (SearchBox), `airportBoard` (AirportPanel), `aircraftHistory` (FlightPanel),
`trainStops`, `trainMedia` (TrainPanel), key storage commands (KeysDialog), updater commands (UpdateToast), API
log (DevPanel). Event: `trains-changed`.

### 3.15 URL hash handling

Dev builds only: `#@lat,lon,zoom[,pitch[,bearing]]` (regex
`^#@(-?[\d.]+),(-?[\d.]+),([\d.]+)(?:,([\d.]+))?(?:,(-?[\d.]+))?`) jumps the map there on mount; `#demo` (anywhere
in the hash, browser-only) shows demo trains. Nothing in release builds. Suggested port equivalent: CLI args
`--at lat,lon,zoom,pitch,bearing`.

---

## 4. Detail panels — shared anatomy

All four (Flight, Train, Ship, Airport) are `<aside class="panel surface">`: 360 px wide, column flex, anchored
bottom-right (§2.3), internal vertical scroll (Airport scrolls only its board), sticky footer (Flight/Train/Ship:
`position: sticky; bottom: 0`, bg `--surface`, top border). Parts marked **[peek]** (`data-peek`) are what the phone
bottom sheet shows at its lowest detent.

**Header strip** ("ATC flight-progress strip"): CSS grid with cells separated by 1 px `--line` vertical rules,
bottom border 1 px `--line`. Cell: column, centred vertically, gap 2, padding 12 (first cell: padding-left 16, no
left rule). Last column is 40 px wide and holds the close `.icon-button` (icon `X` 18/1.75, tooltip `Close (Esc)`),
centred. Title `h2`: 22 px weight 700, line-height 1.1. Sub lines: 12 px `--text-2`, single line with ellipsis.
`.value`: 14 px weight 650 (13 px in the airport panel).

**Facts grid** (`dl.facts`): CSS grid, 3 equal columns (2 in the train panel), gap 12 (`.compact`: 8 row / 12 col);
`.wide` spans all columns. `dt`: 12 px `--text-3`. `dd`: margin-top 2, 13 px, wraps anywhere; `.data` values in the
mono font.

**Photo block**: full-width, aspect 2:1 (`.short` 5:2), bg `--surface-raised`, image `object-fit: cover`, clickable.
Credit text bottom-right (right 8, bottom 4), 11 px `--text-1` with shadow `0 1px 2px rgba(0,0,0,0.9)`.

**Footer**: flex row wrap, space-between, gap 8, padding `12px 16px`, top border. Left: status/report. Right
(`.actions`, margin-left auto, gap 8): `.button`s.

Follow button (all three vehicle panels): `.button`, icon `LocateFixed` 14/1.75 + ` Follow`; pressed style when
following; tooltip `Stop following (C)` when following else `Follow with the camera (C)`.

---

## 7. FlightPanel (`src/lib/FlightPanel.svelte`)

`aria-label="Selected flight"`. Props: `flight: LiveFlight`, `details: FlightDetails|null`, `route: Route|null`,
`status: FollowState`, `lastSeenMs`, `authenticated`, `openskyAdded`, `measured: AdsbAircraft|null`, `following`,
`back`, callbacks `onfollow`, `onselect(flightId)`, `onairport(code)`, `onsignin(returnFocus)`, `onclose`.

Derived: `info = details.flight`, `aircraft = details.aircraft`, `schedule = details.schedule`, `progress =
details.progress`; `loading = !details && status !== "ended"`; `ended = status === "ended"`.

Top to bottom:

1. **Back row** [peek] (only with `back`): full-width button, icon `ArrowLeft` 14/2 + label
   (`"{IATA} departures and arrivals"`); padding `4px 16px`, bottom border, 12 px `--text-2`; hover `--text-1` on
   `--surface-hover`.
2. **Strip** [peek], grid `1fr auto auto 40px`:
   - id cell: `h2.data` = `flight.callsign || flight.flight || "No callsign"`; below `flight.flight ||
     schedule.flight_number || "No flight number"`.
   - cell 2: `.data.value` = `aircraft.typecode || flight.typecode || "—"`; `.data.sub` = `aircraft.reg ||
     flight.reg || "—"`.
   - cell 3 (dimmed to `--text-3` when ended, tooltip `As last reported`): `.data.value.level` in **accent** =
     `altitudeLabel(altitude, onGround)` (§24.2); `.data.sub` = `{info.ground_speed ?? flight.speed} kt`.
   - close button, `aria-label="Close flight details"`.
3. **Photo**: while `loading` a skeleton block (2:1, or 5:2 when there is a back row or the flight is diverted).
   Else if the aircraft has images: the current one (`image.medium || image.thumbnail`), click opens `image.url`
   (tooltip `View this photo on JetPhotos`); credit `© {copyright}` bottom-right; with >1 image: round prev/next
   buttons (28×28, bg rgba(11,11,11,0.7), icons `ChevronLeft`/`ChevronRight` 18/2, vertically centred at left 8 /
   right 8, **opacity 0 until the photo is hovered** or the button focused, 150 ms) and a counter `{n}/{total}`
   bottom-left (mono, 11 px). Photo index resets to 0 when the flight changes; wraps around.
4. **Airline row** (if the airline is known from the callsign's ICAO prefix, or a logo id exists): padding `12px
   16px 0`, gap 12, 13 px weight 600. Logo (`airlineLogoUrl(schedule.painted_as_id || schedule.operated_by_id)`)
   on a light tile: height 26, padding `2px 6px`, radius 4, bg `--text-1`; image max 22 high, 110 wide; hidden if it
   fails to load. Then the airline name.
5. **Route section** [peek] (`aria-label="Route"`): padding 16, column gap 8, bottom border.
   - Two columns (grid `1fr 1fr`, gap 12): origin (left-aligned) and destination (right-aligned). Each, column gap 2:
     - code: `origin.iata || flight.origin` (resp. destination); a text button, mono 22 px weight 700; hover
       underlines in accent (offset 3 px); tooltip `Departures and arrivals at {airport name or code}`; click →
       `onairport(code)`. If no code: `—`.
     - city: `airport.city || airport.name || ""`, 12 px `--text-1`, ellipsis.
     - when: `{label} {clock}` 12 px `--text-2`; the clock is `<Clock unix timeZone=airport.timezone>` (§24.3).
       Departure label/time: no schedule → `Departure` / none; `actual_departure` → `Departed`; else `Scheduled`
       (scheduled_departure). Arrival: no schedule → `Arrival`; `actual_arrival` → `Arrived`; `progress.eta` →
       `Expected`; else `Scheduled`.
     - delay badge when known: `punctuality(delayMinutes(scheduled, actual-or-eta))` → `{label, tone}` (§24.3),
       margin-top 2. When the flight ended and the arrival is still an estimate ("stale"), the when-line dims and
       the badge becomes `neutral`.
   - Progress track: height 2, margin `4px 7px`, bg `--line-control`. Fill: accent, `scaleX(pct/100)` from the left,
     600 ms ease-out. Marker: icon `Plane` 14/2, accent, centred on the track at `pct %`, rotated 45° (points
     right), slides 600 ms ease-out. `pct = clamp(progress.progress_pct, 0, 100)`. Ended: fill and marker
     `--text-3`. Without progress data: a plain 2 px `--line` rule (no marker).
   - Distances (12 px tabular `--text-2`, dimmed when ended): `{km(traversed)} flown · {km(remaining)} to go`
     + `` · {duration(remaining_time)} left`` when `remaining_time > 0`. `km(m) = round(m/1000)` with thousands
     separators + ` km`. While loading: skeleton line 12 px high, 70 % wide.
   - Diverted (`schedule.diverted_id` set and ≠ `destination_id`): `<badge bad>Diverted to {destination.name ??
     "another airport"}`.
6. **Tabs** (`Overview`, `Route`, `Aircraft`, `History`): row, gap 4, padding `0 12px`, bottom border. Tab: padding
   `12px 8px 8px`, 13 px weight 550, `--text-3`; hover/selected `--text-1`; selected has a 2 px accent underline
   inset 8 px left/right at the row's bottom border (radius 1). Left/Right arrows cycle (wrapping) and move focus.
   The tab is **not** reset when the flight changes (component state), but the panel is re-created per mount.
7. **Tab panel**: padding 16, column gap 16.

### 7.1 Overview tab

- Facts (3 columns). `.dim` = `--text-3` when ended (FR24 values only):
  - **Vertical speed**: if FR24's is unavailable (not signed in) but adsb.lol has one: `{signed(measuredVs)}` +
    small `adsb.lol` tag (`.via`: UI font 11 px `--text-3`), cell tooltip `Reported by the aircraft, via adsb.lol`.
    Else unless gated: `signed(info.vertical_speed)` (only when authenticated). `signed(v)` = `—` if undefined, else
    `+1,234 fpm` / `-800 fpm` / `0 fpm`.
  - **Track**: `degrees(info.track ?? flight.track)` (§24.2).
  - **Squawk**: if FR24's is 0/unavailable and adsb.lol has one: `{measured.squawk}` + `adsb.lol` tag. Else unless
    gated: 4 octal digits zero-padded (`code.toString(8).padStart(4,"0")`), `—` when 0.
  - **Phase**: `sentence(progress.flight_stage)` (first char kept, rest lower-cased, `_` → space) or `—`.
  - **Airspace** (only when `info.airspace`; spans 2 columns).
- Gated note (when not signed in and some of vertical speed / squawk / airspace have nothing to show): one line, 12
  px `--text-3`: icon `LockKeyhole` 12/2, the text `signInNote(gated)` (§24.2), and an inline text button `Sign in`
  (weight 600, `--text-1`, underlined in `--line-control`, hover underline accent) → opens the keys dialog with the
  login focused.
- **AltitudeChart** (§9) with `trail = details.trail`, `timeZone = origin.timezone`, `place = origin.iata ||
  flight.origin` (only when the zone is known), `ended`, and an **ExportMenu** (§10) in its tools slot.
- **"Reported by the aircraft"** section (only with `measured`): top border, padding-top 16, margin-top 8, column
  gap 8. Header row: `h3` `Reported by the aircraft` (12 px weight 600 `--text-2`) and a link button `adsb.lol` +
  icon `ExternalLink` 11/2 (opens `ADSB_LOL_URL`; tooltip `Community ADS-B data from adsb.lol, under the Open
  Database Licence`). Then facts grid of `readings.first`; then a disclosure `More from the aircraft ({n})`
  (chevron `ChevronRight` 14/2 rotating 90° in 150 ms when open; 12 px weight 550 `--text-2`; open state remembered
  for the session across flights) containing the `readings.more` grid and, if any, the note `Not reported by this
  aircraft: {listing(unreported)}.`; when opened, scroll so the opened block clears the sticky footer (+12 px). If
  `measured.mlat`: note `Position by multilateration: less precise than the aircraft's own.` Notes: 11 px
  `--text-3`. Readings are defined in `aircraft-readings.ts` (§24.2).

### 7.2 Route tab

Column, gap 12, left-aligned.

- Row (space-between): badge — `Filed flight plan` (good) when `route.kind === "filed"`, `Estimated` (neutral) when
  `"estimated"`, else `No route known` (neutral); right: `AIRAC {ident}` (12 px `--text-3`, ident mono `--text-1`),
  tooltip `Effective {d MMM} to {d MMM}, 00:00 UTC`.
- Filed with `details.flight_plan`: the route string (`plan.route || "Direct"`) in a mono box: full width,
  max-height 132 with scroll, padding `8px 12px`, radius 4, bg `--ink`, 12 px, line-height 1.5, word-spacing 0.2em.
  Compact facts: `Waypoints` = count; `Alternates` (wide, comma-joined) if any. Note: `Flight plans are filed against
  the current AIRAC cycle, {ident}, effective {d MMM} to {d MMM}.`
- Otherwise an explanation paragraph (13 px `--text-2`), composed as:
  - if `route.ahead.length > 1` and an arrival runway is known: `The path ahead is an estimate: the great circle to
    {dest}, then a turn onto a straight final to runway {runway} down a 3° glide path.` + one of `The aircraft is
    lined up with that runway.` (reason `aligned`) / `Other aircraft are using that runway now.` (`traffic`) / `The
    runway in use depends on the wind, which isn't known here, so it's a guess.` + `Controllers vector arrivals, so
    the turns are a guess too.`
  - if ahead but no runway: `The path ahead is an estimate: the great circle to {dest}, descending at 3°. Its
    runways aren't known, so the path ends at the airport.`
  - else: `The map shows the track flown so far.`
  - always appended: `The filed route follows airways and waypoints of AIRAC cycle {ident}; Flightradar24 shares
    flight plans only with signed-in subscribers.`
  - `dest` = destination IATA, else ICAO, else `the destination`.
  - If not authenticated: `.button` `Sign in to Flightradar24`.
- If `progress.great_circle_distance`: compact facts `Great circle` (km), `Flown` (km), `Typical time`
  (`duration(mean_flight_time)` or `—`).
- **Map key** (list, 12 px `--text-2`, gap 8; each row: swatch 28×3 radius 2, gap 12, text):
  - gradient swatch (the altitude colour ramp `ALTITUDE_STOPS` from geo.ts, left→right, stops positioned at
    `alt / maxAlt`) — `Track flown, coloured by altitude` + mono `0 – 45,000 ft` (`--text-3`); if OpenSky points
    were added, a second small line `Start of the track from the OpenSky Network` (11 px `--text-3`).
  - filed: solid swatch rgba(241,238,230,0.6) — `Filed route`.
  - else, if `route.ahead.length > 1`: dashed accent swatch (6 on / 4 off) — `Estimated path to runway {runway}` or
    `Estimated path to the airport`.
  - if `route.before.length > 1`: dashed swatch rgba(241,238,230,0.45) (4 on / 4 off) — `Estimated take-off from
    runway {runway} to the first tracked position` or `Estimated departure to the first tracked position`.
  - if a runway (arrival or departure) is used: small credit `Runways from OurAirports`, indented 40 px.

### 7.3 Aircraft tab

Facts (3 columns): `Type` (wide; `aircraft.full_description || flight.typecode || "—"`), `Type code`,
`Registration`, `Mode S` (hex upper-case, 6 digits), `Serial number` (`aircraft.msn`), `Age` (`{n} year(s)`),
`First flight` (`aircraft.birth_date`), `Owner` (wide; `aircraft.registered_owners`). Missing → `—`.

### 7.4 History tab

Loaded on first opening of the tab per registration: `aircraftHistory(reg)`.

- no registration: `This aircraft's registration isn't known, so its other flights can't be looked up.`
- failed: `Couldn't load the flights of {reg}.` + `.button` `Try again` (icon `RefreshCw` 14/1.75).
- loading: 5 skeleton rows (12 px high, widths `150 + (i*37 % 90)` px).
- empty: `No recent flights recorded for {reg}.`
- rows (buttons; disabled unless `r.live && r.flight_id` and not the current flight; hover `--surface-hover`; the
  current flight has `--amber-soft` bg). Grid `74px 1fr auto`, two lines, gap `2px 8px`, padding 8, 12 px:
  - line 1: day (`dayAt(stod, originZone)`, `--text-2`) | number (`r.number || r.callsign || "—"`, mono weight 650)
    | legs (`{origin_iata||"?"} – {destination_iata||"?"}`, mono).
  - line 2: times (`{dep clock} – {arr clock}`, `--text-3`; dep = `atod || etod || stod` in the origin's zone, arr =
    `atoa || etoa || stoa` in the destination's) | status text right-aligned (`This flight` for the current one,
    else `r.status`), coloured by `toneOf(r.status_color)` (good → `--live`, warn → `--warn`, bad → `--danger`,
    else `--text-2`).
  - click → `onselect(flight_id)` (selects and flies there).
- note under the list: `Live flights open on the map.`

### 7.5 Footer [peek]

- Left status (13 px):
  - `live`: green pulsing dot (8 px circle `--live`; ring animation: box-shadow spread 0 → 6 px fading from 60 %
    to 0 % over 2 s ease-out, infinite) + `Live` (weight 600, `--live`).
  - `ended`: hollow dot (8 px, 1.5 px inset ring `--text-3`, no animation) + `Last seen {clock}` (in the departure
    airport's zone, else the viewer's) or `Not live` when no time is known; `--text-2`; tooltip `The last report, in
    {IATA time | your time zone}`.
  - `connecting` / `reconnecting`: spinner icon `LoaderCircle` 14/2 rotating 360° per 900 ms linear + `Connecting…`
    / `Reconnecting…`.
- Right: Follow button — **icon only** (32×32, `.compact`) unless status is `live`, when it also shows ` Follow`;
  and `.button` `Flightradar24` + icon `ExternalLink` 14/1.75 opening
  `https://www.flightradar24.com/{callsign || "flight"}/{id as lower-case hex}`.

### 7.6 Phone

Chrome removed (§2.4). Order: back, strip, route, then (order 1) photo, airline, tabs, tab panel, then (order 2) the
footer. At the `peek` detent only the `data-peek` parts show (back row, strip, route, footer).

---

## 8. TrainPanel, ShipPanel, AirportPanel

### 8.1 TrainPanel (`src/lib/TrainPanel.svelte`)

`aria-label="Selected train"`. Props: `train`, `now`, `following`, `onfollow`, `onclose`, `liveStops`.

1. **Strip** [peek]: grid `minmax(0,1fr) auto 40px` (`.bare` without the middle cell: `minmax(0,1fr) 40px`).
   - id: `h2` (UI font, 22/700, single line ellipsis; when `name.length > 16`: 17 px, line-height 1.25, wraps to at
     most 2 lines) = `train.name`; below: an 8×8 swatch (radius 2, the train's map colour `trainColorOf(train)`,
     margin-right 6) + `{trainLabel(kind)} train`.
   - if moving (`speedKmh !== null || heading !== null`), tooltip `Speed and direction of travel`: value in accent
     `{round(speedKmh)} km/h` or `—`; sub `degrees(heading)` or empty.
   - close (`aria-label="Close train details"`).
2. **Photo** (if `trainMedia(source, product, operator)` returned one and it loads): image `photo.url`, click opens
   `photo.pageUrl` (tooltip `{title} on Wikipedia`), credit `Photo: Wikipedia, {title}`.
3. **Journey** [peek] (if destination or delay): grid `minmax(0,1fr) auto`, gap 12, padding 16, bottom border.
   - left: label `Destination` (12 px `--text-3`); value 22/700 single-line ellipsis (14 px/1.5 when longer than 14
     chars); or `Not given` (12 px `--text-2`).
   - right (if a delay text exists; tooltip `At the last or next stop, as the operator reports it`): label
     `Running`, badge with tone `delayTone(delayMinutes) ?? neutral` and text = `delayText(...)` capitalised; badge
     at 12 px.
4. **Stops** (Trafikverket trains only): padding `12px 16px 0`. Heading `Stops` (13 px weight 650) + count (12 px
   `--text-3`). Source: `liveStops` if present else `trainStops(id)` fetched on selection and every 60 s.
   - each stop row: grid `14px 1fr auto`, gap 8, padding `5px 0`, 13 px. A 2 px vertical line at x = 6 px runs
     through all rows (`--line-control`; `--text-3` on passed rows; starts at the middle of the first row and ends
     at the middle of the last). Dot: 10 px circle, 2 px border `--line-control`, fill `--surface`, margin-left 2;
     passed: border+fill `--text-3`; next stop: border+fill accent.
   - name (wraps); passed: `--text-3`; next: weight 650; cancelled: line-through + badge `Cancelled` (bad); each
     `stop.notes` entry on its own line, 12 px `--warn`.
   - right (mono 12 px, gap 8, nowrap): if `view.late` is non-null and non-zero: planned time struck through
     (`--text-3`) then the actual/estimated time in `--warn` (late) or `--live` (early), tooltip `{n} min late|
     early`; else just the time. Then `Tr {track}` (`--text-3`, tooltip `Track`) if any. `stopView(stop)` and
     `nextStop(stops)` are in trains.ts (§24.4).
   - empty list: `No timetable for this train: it isn't announced to travellers.`; failed: `Couldn't load the stops.
     Trying again in a minute.`; loading: `Loading stops…` (12 px `--text-2`).
5. **Body** (padding 16, gap 12): facts in **2 columns**: `Operator` (optional logo: max 56×22, padding `2px 4px`,
   radius 3, bg #f2f2f2, contain; then `train.operator ?? "Not given"`), `Source`
   (`TRAIN_SOURCE_NAMES[source] ?? source`), `Position` (wide, mono, `latLon(lat, lon)` of `positionAt(train,
   now)`; tooltip `Moving along the source's expected positions` when `frames.length > 1` else `As last reported`).
   Then, if the source has a credit, a note link (11 px `--text-3`, underlined) with `credit.text` → opens
   `credit.url`.
6. **Footer** [peek]: left: `trainPositionAge(positionUnix, now)` → `{label, stale}`: warn badge (12 px) if stale
   else plain 13 px `--text-2`; tooltip `Reported at {clockWithZone(positionUnix)}`. Right: Follow button (always
   with text), and, if the source has a credit, `.button` `{sourceName}` + `ExternalLink` 14/1.75 opening
   `credit.url` (hidden on phones).

Phone order: strip, journey, stops, body, then photo (order 1), footer (order 2).

### 8.2 ShipPanel (`src/lib/ShipPanel.svelte`)

`aria-label="Selected ship"`. Props: `ship`, `details: ShipDetails|null`, `error`, `now`, `onretry`, `following`,
`onfollow`, `onclose`. `loading = !details && !error`.

1. **Strip** [peek] (grid as the train's; `.bare` when not moving and not loading):
   - `h2.data` (mono; 17 px/1.3 when the name is longer than 13 chars) = `details.name || ship.name (unless it is
     just the MMSI) || "Unnamed ship"`; subtitle = `[details.typeLabel || shipKindLabel(ship.kind), details.flag]`
     joined by ` · `; while loading and no subtitle: skeleton 140×12.
   - moving (`speedKn` or `courseDeg` present; tooltip `Speed and course over ground`): accent `{speedKn.toFixed(1)}
     kn` or `—`; sub `degrees(courseDeg)`. While loading: two skeletons (56×14 and 36×12).
   - close (`aria-label="Close ship details"`).
2. **Photo**: skeleton 2:1 while loading; else if `details.photoId` loads: `shipPhotoUrl(photoId)`, click opens the
   ship's VesselFinder page (`shipPageUrl(mmsi)`; tooltip `View this ship on VesselFinder`), credit `Photo:
   VesselFinder`.
3. **Voyage** [peek]: while loading: two 84×22 skeletons + a 70 % line. Else if destination/ETA/status/SAR:
   - legs grid `minmax(0,1fr) auto`: left label `Destination` + value (mono 22/700 if it is a UN/LOCODE
     (`isLocode`), else 14 px/1.5; tooltip `UN/LOCODE {dest}, as entered by the crew` or `As entered by the crew`)
     or `Not given`; right (if ETA): label `ETA` or `ETA, passed`, value `clockWithZone(eta, "UTC")` (mono 14 px),
     and `dayAt(eta, "UTC", now)` (12 px); dimmed `--text-3` when passed; tooltip `This ETA has passed; the crew
     hasn't updated it` / `As entered by the crew; the port's time zone isn't known, so in UTC`.
   - badges row: nav status badge (`status.label`, tone `navTone(status)`, tooltip `AIS navigational status, as set
     by the crew`); `Search and rescue aircraft` (neutral) when `details.sar`.
4. **Body** (padding 16, gap 12):
   - error: `Couldn't load this ship's details from VesselFinder.` (13 px `--text-2`), the error string (note), and
     `.button` `Try again` (`RefreshCw`).
   - facts (3 columns); loading: 6 skeleton pairs (label 52×10, value `44 + (i*23 % 40)` px × 12). Loaded, in
     order, each only when present: `IMO`, `MMSI` (always), `Flag` (tooltip = flag code), `Length × beam`
     (`dimensions(lengthM, beamM)`), `Draught / max` or `Draught` (`draught(draughtM, maxDraughtM)`; tooltip `The
     current draught as entered by the crew, and the ship's maximum`), `Built`, `Gross tonnage` (thousands
     separators, en-GB), `Deadweight` (`{n} t`), `Position` (wide; `latLon(ship.lat, ship.lon)`; tooltip `As the map
     last showed it`). All but Flag in mono.
   - notes (11 px `--text-3`, gap 2), once details exist: `{Status[, destination] and ETA} is|are entered by the
     crew.` (lists whichever of status / destination / ETA are shown, capitalised, `is` for one, `are` for more)
     and `Data: VesselFinder`.
5. **Footer** [peek]: left column: if `details.positionUnix`: `positionAge(positionUnix, now)` as warn badge (stale)
   or plain 13 px text, tooltip `Reported at {clockWithZone(positionUnix)}`; plus `via satellite AIS` (11 px
   `--text-3`) when `details.satellite`; loading: skeleton 120×12; else if `ship.old`: warn badge `Old position`.
   Right: Follow button and `.button` `VesselFinder` + `ExternalLink` opening the ship's page.

Phone order: strip, voyage, then photo and body (order 1), footer (order 2).

### 8.3 AirportPanel (`src/lib/AirportPanel.svelte`)

`aria-label="{airport.name ?? code} departures and arrivals"`. Props: `code` (IATA), `airport` (reference data, may
be undefined while loading), `selectedId`, `onselect(flightId)`, `onlocate`, `onclose`. Panel does not scroll as a
whole: the board does (`flex: 1; min-height: 0; overflow-y: auto`).

State: `mode: "departures"|"arrivals"` (default departures), `board {key, rows, page, pages}`, `failed`, `busy`,
`updatedAt`, own `now` ticking every 15 s. `key = "{code}:{mode}"`. On key change: clear, load page 1; every 60 s
reload pages `1..board.page` (sequentially, `airportBoard(code, mode, p)`); rows de-duplicated by
`{flightId ?? number}-{scheduled}`. "More" exists while `board.page < board.pages`.

1. **Strip** [peek], grid `auto 1fr auto 40px`:
   - id: `h2.data` = code; `p.data` = `airport.icao ?? ""`.
   - name cell: `.value` (13 px/650, ellipsis; tooltip full name) = `airport.name ?? "Airport"`; sub = `city,
     country`.
   - clock cell (tooltip `Local time at {code}` or `Your time: the airport's time zone isn't known yet`): mono
     `clockAt(now, timeZone)` and `zoneName(now, timeZone)`.
   - close (`aria-label="Close airport"`).
2. **Tabs** [peek]: `Departures` (icon `PlaneTakeoff` 15/1.75) and `Arrivals` (`PlaneLanding`), styled as the
   flight panel's tabs with icon gap 8; Left/Right toggles.
3. **Board** (padding 8; opacity 0.75 while busy, 150 ms):
   - failed with no rows: `Couldn't load the {mode} board.` + `.button` `Try again`.
   - loading: 7 skeleton rows (flex gap 12, padding `12px 8px`; widths 38, `90 + (i*31 % 60)`, `60 + (i*17 % 30)`
     px; 12 high).
   - empty: `No {mode} listed right now.`
   - rows, with **day headings** inserted before a row whose day label differs from the previous row's (for the
     first row: only when it is not `Today`): 12 px weight 600 `--text-2`, padding `12px 8px 4px`.
   - row button (disabled unless `r.live && r.flightId`; tooltip `Show this flight` / `Not airborne or not tracked
     right now`; hover `--surface-hover`; selected flight `--amber-soft`): grid `44px 1fr auto`, three lines, gap
     `1px 8px`, padding 8, 12 px:
     - line 1: time (mono 13/650, `clockAt(scheduled, timeZone)`) | number (mono 13/650, `number || callsign ||
       "—"`) + airline name (`--text-3`, ellipsis) | status (`r.status`, coloured by `toneOf(statusColor)`,
       preceded by a 6 px `--live` dot when live).
     - line 2 (under the flight column): `To|From {airportCity || airportName || "?"}` + mono `({IATA})`;
       `--text-2`, ellipsis.
     - line 3 (hidden when empty; mono 11 px `--text-3`): `[aircraft, registration, T{terminal}, Gate {gate}]`
       joined by ` · `.
   - `.button` `Show more {mode}` (margin 8; disabled while busy) loads one more page.
4. **Footer** [peek] (not sticky; padding `12px 16px`, top border): left 11 px `--text-3`: `Updated {HH:MM} · times
   at {code}` (or `· your time` when the zone is unknown); right `.button` `Show on map` (icon `LocateFixed`;
   disabled until the airport is known) → fly to the airport at zoom 11.


---

## 5. SearchBox (`src/lib/SearchBox.svelte`)

Props: `onpick(FindEntry)`, `onplace(Place)`, `onquery()`, `near`, `local: LocalSearch|null`, two-way `query`. Exposes
`focus()` (focus + select all), used by the `/` shortcut.

### 5.1 Field

- Height 36, padding `0 12`, gap 8, 1 px border `--line-control`, radius 4, bg `--ink`; icon colour `--text-3`
  (`--text-2` and **accent border** while focused; 150 ms).
- Left icon `Search` 16/1.75. Input: 14 px `--text-1`, placeholder `--text-3`. No native clear button.
- Placeholder: `Flight, airport or address` (aircraft); trains: `Train, destination or address` (phone: `Train or
  address`); ships: `Ship name, MMSI or address` (phone: `Ship or address`).
- Right adornment (exactly one):
  - searching → spinner `LoaderCircle` 14/2 (900 ms/turn);
  - empty query → `kbd` `/` (min 18×18, 1 px border `--line-control`, radius 4, mono 11 px weight 600 `--text-3`;
    tooltip `Press / to search`; hidden on phones);
  - local mode with text → clear button (22×22, icon `X` 14/1.75, `--text-3`; hover bg `--surface-hover`,
    `--text-1`; tooltip `Clear the search`) → resets and calls `onquery`.

### 5.2 Behaviour

- Two independent searches run as the user types:
  - **Flights/airports** (`find(query)`, backend search; only entries of type `live` or `airport` kept): needs ≥ 2
    chars, debounce **250 ms**. Not run in local mode.
  - **Places** (`geocode(query, near, placeLanguage())`, Photon): needs ≥ **3** chars, debounce **350 ms**.
  - Each has a phase: `idle | searching | done(results) | error`, with a request counter so late answers are
    dropped.
- Local mode (`local` set; trains and ships): the first group is `local.search(query)` capped at **6** rows
  (computed synchronously from the loaded list: the first 6 of the sidebar's already-filtered-and-sorted list). The
  query is bound to the page (`trainQuery` / `shipQuery`) and narrows the sidebar list as well.
- Every keystroke: open the dropdown, clear the highlighted row, call `onquery` (clears the place marker).
- Dropdown visible when the field is focused and (local: the query is non-empty; else: at least one of the two
  searches has settled (done or error)). Blur closes after 120 ms.
- Keys: Down/Up move the highlight through all rows of both groups (wrapping; scrolled into view); Enter picks the
  highlighted row (default: the first); Escape closes the dropdown and blurs (and does **not** propagate to the
  app's Escape handler). Mouse hover highlights.
- Picking: reset (clear the query, both phases idle, close, blur), then `onpick(entry)` / `local.onpick(key)` /
  `onplace(place)`.

### 5.3 Dropdown

Positioned under the field: `top: 100% + 8px`, same width (on phones the `.search` wrapper is static so the
dropdown spans the **whole top bar's width**); z 10; `max-height: min(440px, 70vh)`, scrolls; padding 4; **bg
`--surface-raised`**, 1 px `--line` border, radius 4.

- Group heading (12 px weight 600 `--text-3`; padding `8px 8px 4px`): first group — local: `local.heading` (`Trains
  in view` / `Ships in view`); otherwise `Flights and airports`, shown only when the places group is shown. Second
  group `Places` (shown whenever the place search is not idle), separated by a 1 px `--line` top border + 4 px
  margin, with a right-aligned credit `© OpenStreetMap contributors, via Photon` (11 px weight 400 `--text-3`,
  ellipsis) when there are results.
- Messages (13 px; padding `8px 8px 12px`; `quiet` = `--text-3`, else `--text-2`):
  - `Searching flights…` (quiet, with a 12 px spinner), `Flight search failed. Check your connection and try
    again.`, `No live flights or airports match “{q}”.`
  - local: `No {noun} in view match “{q}”.` (noun = `trains` / `ships`)
  - `Looking up places…` (quiet, spinner), `Places unavailable` (quiet), `No places match “{q}”.`
- Row: flex gap 12, padding 8, radius 4; highlighted bg `--surface-hover`. Leading 26×26 tile (radius 4, bg
  `--surface`) with a 14/1.75 icon; then two lines, both single-line ellipsis: label 13 px, detail 12 px `--text-3`.
  - live flight: icon `Plane` in accent; label `entry.label`; detail `entry.detail.route ?? entry.detail.ac_type`.
  - airport: icon `MapPin` `--text-2`; detail `Airport`.
  - local: the mode's icon (`TrainFront` / `Ship`) coloured with the item's map colour; label/detail from the
    page: trains — label `train.name`, detail `[To {destination}, delayText(delay) || trainLabel(kind)]` joined by
    ` · `; ships — label `shipName(s) || "MMSI {mmsi}"`, detail `{shipKindLabel(kind)} · MMSI {mmsi}`.
  - place: icon by `place.kind` — address `House`, street `Signpost`, poi `Landmark`, airport `PlaneLanding`,
    district/city/town `Building`, postcode `Mailbox`, region `Map`, country `Flag`, fallback `MapPin`; label
    `place.name`; detail `placeDetail(place)` (places.ts), tooltip `place.label`.

---

## 6. ModeBar + Segmented (`ModeBar.svelte`, `Segmented.svelte`)

### 6.1 Segmented (generic radio group)

Props: `legend`, `hideLegend`, `options: [value, label, key?][]`, `value`, `disabled`, `columns`.

- Optional legend above: 13 px `--text-1`, margin-bottom 8.
- Options box: CSS grid of `columns` (default: one row with all options) equal columns, gap 2, padding 2, 1 px
  border `--line-control`, radius 4.
- Option: centred, gap 6, padding `5px 0`, radius 4, 13 px `--text-2`; hover bg `--surface-hover` (150 ms);
  **selected: bg accent, text white, weight 650**. Keyboard focus: 2 px accent outline, offset 1. Optional key hint
  after the label: mono 11 px weight 600, opacity 0.7; option tooltip `{label} ({key})`.
- Disabled group: opacity 0.4, no hover.

### 6.2 ModeBar

A `.surface` with padding 3 holding a Segmented (legend "Show", hidden) laid out as a flex row with no inner
border/padding. Options in order (only those `offered`): `Aircraft` (icon `Plane`, key P), `Trains` (`TrainFront`,
key I), `Ships` (`Ship`, key S). Each option: icon 15/1.75, gap 7, name, and — when its count is non-null — the
count (mono 12 px weight 500 `--text-3`; in the selected option it inherits white at opacity 0.75). Padding `6px
12px`. No key hints shown (they are in tooltips: `Aircraft (P)` etc.).

Count format: `Intl.NumberFormat("en-GB", {useGrouping: "min2"})` — group digits only from five digits up (`1443`,
`12,345`). Counts: aircraft = `flights.length` when aircraft are on and a snapshot exists; trains = `trains.length`
when on and first answer in; ships likewise; else no count.

**Eye toggle** after each non-focused option: button with icon `Eye` 14/1.75, tooltip `Also show {aircraft|trains|
ships} on the map (Shift+{P|I|S})`. It is **collapsed (width 0, opacity 0)** until the bar is hovered or has
keyboard focus inside, or the toggle is on; then width 24, margin-left −4, margin-right 2, opacity 1 (150 ms on
width/margin/opacity/bg). Colour `--text-3`; hover bg `--surface-hover` + `--text-1`; on: accent colour. When a
non-focused option is "also shown", its label text is `--text-1` rather than `--text-2`.

Phone: static position inside the top bar; no surface chrome; each option is a 44×36 icon-only cell (name and count
hidden).

---

## 9. AltitudeChart (`src/lib/AltitudeChart.svelte`)

Props: `trail: TrailPoint[]` (`timestamp` unix s, `altitude` ft, `ground_speed` kt, …), `timeZone`, `place`,
`ended`, `tools` slot.

Header (baseline-aligned row, space-between): `Altitude` (12 px weight 600 `--text-2`) + ` feet` (weight 400
`--text-3`). Right side, only when drawable, gap 12: zone note (11 px `--text-3`): `{place} time` or `Your time,
{zone}` (tooltip `Times are local to {place} ({zone})` / `Times are in your time zone ({zone}); the departure
airport's isn't known`); the tools slot (ExportMenu); a text toggle `Table` / `Chart` (12 px `--text-3`, hover
`--text-1`).

Data: points with `timestamp > 0`, sorted ascending. Not enough (fewer than 2 points or no time span): the message
`The altitude profile appears once the aircraft has reported a few positions.` (12 px `--text-3`).

Geometry (px): `LEFT 46`, `RIGHT 8`, `TOP 18`, `PLOT 96`, `AXIS 18`; total height **132**; width = container width
(panel 360 − 2×16 padding = 328 on desktop).

- `x(t) = LEFT + (t − t0)/(t1 − t0) · (width − LEFT − RIGHT)`; `y(alt) = TOP + PLOT − max(0, alt)/scale.top · PLOT`.
- Y scale: `niceScale(max(1000, max altitude) · 1.12)`: the first step in `[250, 500, 1000, 2000, 2500, 5000,
  10000, 15000, 20000]` for which `top = max(step, ceil(max/step)·step)` has `top/step ≤ 4`; fallback step 20 000.
  Ticks at every step from 0 to top: a 1 px `--line` horizontal grid line (at y + 0.5) and a right-aligned label at
  `x = LEFT − 6`, baseline `y + 4`, with thousands separators.
- X ticks: interval = the first of `[5, 10, 15, 30, 60, 120, 180, 360, 720]` minutes giving ≤ 4 intervals over the
  span; ticks on round **local** times (in `timeZone`) from the first round time ≥ t0; a tick is dropped if its
  label would overhang (`x − 18 < LEFT − 6` or `x + 18 > width`). Label `HH:MM`, centred, baseline `TOP + PLOT + 13`.
- Tick/end labels: mono 11 px, tabular; ticks `--text-3`, end label `--text-2` (`--text-3` when ended).
- Area: the line closed down to the baseline, fill `--series-1` at 10 % opacity. Line: `--series-1`, 2 px, round
  joins and caps.
- End dot: circle r 5, fill `--series-1` (`--text-3` when ended), 2 px `--surface` ring. End label
  `{altitude} ft` right-aligned at `(ex − 9, ey − 8)` when the line arrives level or climbing (the point ~64 px
  earlier is not more than 300 ft higher); when descending, at `(ex − 9, ey + 16)` if `ey + 18 ≤ TOP + PLOT`, else
  no label.
- Hover (pointer move over the plot; touch `pan-y` so vertical scrolling still works): nearest point by time
  (binary search); a 1 px `--line-control` vertical crosshair from TOP to TOP+PLOT, the dot at that point, and a
  tooltip pinned to the top of the plot at `left = x`, shifted 10 px right — or flipped fully to the left
  (`translateX(-100% − 10px)`) when `x > width/2`. Tooltip: padding `4px 8px`, 1 px `--line`, radius 4, bg
  `--surface-raised`, nowrap; line 1 (mono 13 px weight 650): a 10×2 `--series-1` key + `{alt} ft`; line 2 (11 px
  `--text-2`): `{clock} · {ground_speed} kt`. While hovering, the end dot/label are hidden.
- Keyboard (plot focusable): Left/Right step one point (Shift: a tenth of the points), Home/End; focus selects the
  last point; blur clears.
- Table view (max-height 180, scroll): columns `Time`, `Altitude`, `Speed` (headers 12 px weight 600 `--text-3`,
  sticky, right-aligned except the first); at most ~30 rows (every `ceil(n/30)`-th point plus the last); rows mono
  12 px, 1 px `--line` top border, padding `2px 0`: `HH:MM`, `{alt} ft`, `{speed} kt`.

---

## 10. ExportMenu (`src/lib/ExportMenu.svelte`)

A text button in the chart header (12 px `--text-3`; hover/open `--text-1`; disabled opacity 0.5): label `Export`;
`Saving…` while busy; after an outcome, for 5 s (success) or 8 s (error): `Saved` / `Not saved` (error in
`--danger`). Disabled when fewer than 2 exportable points or busy. Tooltip: the last status text, else `Nothing to
export until the aircraft has reported a few positions` / `Save this flight's path as a file`.

Menu (opens under the button, right-aligned; `top: 100% + 8`; width 240; padding 4; radius 4; bg `--surface-raised`
+ 1 px `--line`; z 5). Items (column, gap 2, padding 8, radius 2; hover/focus bg `--surface-hover`): label 13 px
weight 600, hint 12 px `--text-3`:

| Format | Label | Hint |
|---|---|---|
| kml | `KML` | `Google Earth, with a 3D track that plays back in time` |
| gpx | `GPX` | `Mapping and GPS apps` |
| csv | `CSV` | `Spreadsheets: time, position, altitude, speed, track` |
| geojson | `GeoJSON` | `GIS tools and web maps` |

Footer note (top border, 12 px `--text-3`): `{count} positions, saved to Downloads`.

Behaviour: opening focuses the first item; Up/Down cycle; Escape closes (not propagated) and refocuses the button;
click outside or focus leaving closes. Choosing: `exportPath(trail, meta, format)` → contents;
`exportFileName(meta, points, format)` = `{callsign||flightNumber||registration||"flight"}[ {origin}-{destination}][
{YYYY-MM-DD of first point}].{ext}`; `saveExport(name, contents, format)` (backend writes to Downloads and returns
the path). Status: `Saved {file name}` or `Couldn't save the path: {error without "Error: "}`. The file builders
are in `src/lib/export.ts` (port to Rust verbatim; tests in `export.test.ts`).

---

## 11. FilterPanel — aircraft (`src/lib/FilterPanel.svelte`)

`.filters.surface`, column; in the desktop sidebar it sits between the primary card and the list (`flex: 0 1 auto`,
`min-height: 0`; its body scrolls).

**Header** (shared by all three filter panels): row gap 8, padding `8px 8px 8px 16px`, bottom border: `Filters`
(14 px weight 650); `{shown} of {total} shown` (flex 1, 12 px tabular `--text-3`); text button `Reset` (disabled
when no filter is active); close icon button (`X` 16/1.75, tooltip `Close filters (F)`).

Text button style (Reset / All / None): padding `2px 4px`, radius 4, 12 px weight 600, accent text; hover bg
`--amber-soft`; disabled `--text-3`.

**Body**: column gap 16, padding 16, scrolls. Group labels: 12 px weight 600 `--text-2`.

1. `Callsign, flight or registration` — text input (height 34, padding `0 12`, 1 px `--line-control`, radius 4, bg
   `--ink`, 13 px; focus border accent), placeholder `e.g. BAW, LH400, D-AIM`; bound to `filters.text`.
2. `Categories` with `All` / `None` links on the right; chips (wrap, gap 4) for the 12 categories (filters.ts:10-23)
   in order: `Passenger`(0), `Cargo`(1), `Military and government`(2), `Business jets`(3), `General aviation`(4),
   `Helicopters`(5), `Lighter than air`(6), `Gliders`(7), `Drones`(8), `Ground vehicles`(9), `Other`(10),
   `Uncategorised`(11). Chip: height 26, padding `0 8`, 1 px `--line-control`, radius 13 (pill), 12 px, gap 3; off:
   transparent, `--text-3`; on: bg `--surface-raised`, `--text-1`, with a leading `Check` icon 12/2.5 in accent;
   hover bg `--surface-hover`. `filters.categories` is kept sorted ascending.
3. `Altitude` with the current value on the right (mono 12 px `--text-1`): `Any altitude` (0 to max), `Above {low}
   ft`, `Below {high} ft`, or `{low} – {high} ft` (thousands separators). A **dual-thumb range slider**: 20 px
   high; rail 2 px at top 9, inset 8 px left/right, `--line-control`; the selected span in accent; thumbs 16 px
   circles, 2 px accent border, fill `--ink`. Range 0–50 000 ft (`MAX_ALTITUDE`; the top means "no upper limit"),
   step 1 000; the thumbs are kept at least one step apart. Below: two checkboxes, gap 16, 13 px: `In the air`
   (`filters.airborne`), `On the ground` (`filters.onGround`) (native checkboxes tinted accent).
4. TokenInput `Airlines` — placeholder `Name or code`; chip text `{ICAO} {airline name}`; free entry: 3 letters →
   upper-cased. Suggestions (max 6): airlines with an ICAO code not already picked; with a query: ICAO starts with
   it, or IATA equals it, or the name contains it; without: those with live flights. Sorted: exact code match
   first, then live count descending, then name. Row: label = name; detail = `{ICAO} · {n} live`.
5. TokenInput `Aircraft types` — placeholder `e.g. A388 or B77`; hint `A type code, or its start: B77 matches every
   Boeing 777.`; free entry: 2–4 alphanumerics → upper-cased. Suggestions: type codes present in the feed starting
   with the query, by count descending; detail `{n} live`.
6. TokenInput `Airports` — placeholder `City, name or code`; hint `Flights departing from or flying to any of
   them.`; chip `{IATA} {city}`; free entry: 3 letters. Suggestions: airports with an IATA code; query matches IATA
   prefix, ICAO prefix, name or city substring; without a query those with live flights; sorted exact, live count,
   then airport size; label `{city} · {name}` (or name); detail `{IATA} · {n} live`.

`Reset` restores `DEFAULT_FILTERS`. Filter semantics: `matcher()` in filters.ts:79-99; badge count
`activeCount()` filters.ts:66-76 (7 groups: categories, altitude, airborne/ground, airlines, types, airports, text).

### 11.1 TokenInput (`src/lib/TokenInput.svelte`)

Label (12 px weight 600 `--text-2`), then a field (flex wrap, gap 4, min-height 34, padding `3px 8px`, 1 px
`--line-control`, radius 4, bg `--ink`; focus-within border accent) holding the chosen chips followed by a
borderless input (min-width 80, height 26, 13 px). Chip: height 24, padding-left 8, radius 4, bg `--amber-soft`,
accent text 12 px weight 600; text max-width 150 with ellipsis (tooltip full text); a 22×22 remove button (`X`
12/2).

Suggestions list appears **in the flow under the field** (not floating — the panel scrolls), `.surface` with bg
`--surface-raised`, padding 4; rows padding `6px 8px`, radius 4, 13 px, label left (ellipsis), detail right (mono
11 px `--text-3`); active row bg `--surface-hover`. Shown while focused (`suggest(query)` is called even with an
empty query). Keys: Down/Up cycle; Enter adds the active suggestion, or else the `free(query)` value if valid;
Backspace on an empty input removes the last chip; Escape closes the list (not propagated). Mouse-down on a row
adds it. After adding: clear the query. Hint below: 11 px `--text-3`.

---

## 12. TrainFilterPanel (`src/lib/TrainFilterPanel.svelte`)

Same shell/header as §11. Body:

1. FilterChips `Kind`: one chip per `TRAIN_KINDS` (trains.ts) with label `trainLabel(kind)`, the kind's map colour
   as swatch, and the count of loaded trains of that kind.
2. FilterChips `Operator`: operators of the loaded trains (most trains first; unknown operator `""` last, labelled
   `Unknown`), plus any hidden operators not currently running (count 0, so they can be put back); swatch =
   `trainColorOf({kind: "other", operator})` (none for unknown). Hint: `Each operator's trains come live on their
   own stream; one left out isn't streamed.`
3. ToggleRow `Delayed only`, hint `5 min or more late, as the operator reports`.

`Reset` clears `hiddenKinds`, `hiddenOperators`, `delayedOnly` (sort is kept). Badge count: 3 groups.

### 12.1 FilterChips (`src/lib/FilterChips.svelte`)

Legend row: label (12 px weight 600 `--text-2`) and `All` (disabled when nothing hidden) / `None` (disabled when
everything hidden) text buttons. Chips as the category chips (height 26, pill radius 13, 1 px `--line-control`, 12
px, gap 5, padding `0 8`): on → bg `--surface-raised`, `--text-1`, leading `Check` 12/2.5 accent (margin-right −2);
off → `--text-3`; then an 8×8 colour swatch (radius 2; opacity 0.35 when off), the label, and the count (mono 11 px
`--text-3`, en-GB grouping; omitted when 0/undefined). The model stores the **hidden** values. Optional hint (12 px
`--text-3`). Disabled: legend and chips at opacity 0.45, no hover; the hint stays readable.

### 12.2 ToggleRow (`src/lib/ToggleRow.svelte`)

Row grid `1fr auto auto`, gap 12, min-height 32, 13 px; the whole row toggles. Left: label and an optional hint
line (12 px `--text-3`, single-line ellipsis). Middle: optional key cap (min 20×20, padding `0 4`, 1 px `--line`
border with a 2 px bottom, radius 4, mono 11 px weight 600 `--text-3`). Right: the **switch**: 34×20 track, radius
10, bg `--line-control` (accent when on); knob 14 px circle at 3 px inset, `--text-1` (white when on), slides 14 px
in 150 ms.

---

## 13. ShipFilterPanel (`src/lib/ShipFilterPanel.svelte`)

Same shell/header. Body:

1. FilterChips `Type`: one chip per ship colour key (`SHIP_COLOR_KEYS` without 0, then 0 = unknown last), label
   `shipKindShort(key)` (short names: key 2 `Tug or special`, 7 `Rescue or law`, 8 `Pleasure or sailing`, else
   `shipKindLabel`), swatch `shipColor(key)`, count.
2. Group `Movement` (label 12 px weight 600) with a Segmented: `All` / `Under way` / `Moored`
   (`filters.motion`: `all | moving | moored`); hint `Moored includes ships reporting no heading.`
3. FilterChips `Size`: `Small`(0) / `Medium`(1) / `Large`(2) with counts only when sizes are known; **disabled**
   when `!sizesKnown` with hint `Zoom in to tell sizes apart: zoomed out, VesselFinder lists every ship as medium.`
4. ToggleRow `Hide old positions`, hint `Ships that haven't reported lately`.

`Reset` restores `DEFAULT_SHIP_FILTERS` keeping the sort. Badge count: 4 groups (kinds, motion ≠ all, sizes,
hideOld). Predicate: `shipFilter()` (ship-list.ts): moving = `heading !== null`.

---

## 17. BottomSheet (`src/lib/BottomSheet.svelte`) — phones only

`position: fixed; left 0; right 0; bottom 0; z 4`; `.surface` without left/right/bottom borders; **top corners
radius 12**; `padding-bottom: safe-area-inset-bottom`; `overflow: hidden`; column: grab handle, optional head,
scrolling body.

- Props: `detent: "peek"|"half"|"full"` (two-way), `height` (out: the measured outer height incl. the home-bar
  padding — the page publishes it as `--sheet-h` for the attribution strip and map controls), `label`, `clear` (px
  left free at the top at full height; 64 with the search bar on show, 12 with a detail open), `head`.
- Grab handle: full-width button 22 px high containing a 36×4 pill (radius 2, `--line-control`; accent on keyboard
  focus). `touch-action: none`.
- Head: padding `0 16px 4px` (respecting side safe areas) — holds the toolbar (count + Filters) for the list.
- Body: flex 1, column, `overflow-y: auto`, `overscroll-behavior: contain`.
- Heights:
  - `peek` = if the content marks parts with `data-peek`: `grabHeight + headHeight (if any) + Σ offsetHeight of all
    [data-peek] elements` (re-measured on resize/mutation); otherwise **90**.
  - `half` = `max(peek, round(viewportHeight · 0.5))`.
  - `full` = `max(peek, viewportHeight − clear)`; additionally capped by CSS `max-height: 100dvh − clear −
    safe-top − safe-bottom`.
- Height changes animate 220 ms ease-out (none while dragging or with reduced motion).
- Gestures on the handle: drag follows the finger (`height = clamp(from + dy, peek − 40, full)`); a movement
  > 6 px counts as a drag; on release snap to the nearest detent; a tap (no drag) steps `peek → half → full →
  peek`. Keyboard: Up raises a detent, Down lowers.
- Landscape phone: `left: max(8, safe-left); right: auto; width: 320` (a column down the left, under the search).

Because detail panels put their `data-peek` parts first on phones (and the rest gets `order: 1/2`), the peek
detent shows exactly: [back row], header strip, key line (route / journey / voyage; the board's tabs), and the
footer with the actions.

---

## 14. TopFlights (`src/lib/TopFlights.svelte`) — the aircraft list

Header (baseline row, space-between, padding `0 8px 8px`): `Most tracked` (14 px weight 650); right: icon `Eye`
13/1.75 + `Watching` (12 px `--text-3`; tooltip `People watching each flight on Flightradar24 right now`).

- failed and nothing to show: `Couldn't load the most tracked flights.` + `.button` `Try again` (`RefreshCw`).
- loading (`flights === null`): 6 skeleton rows — grid `1fr auto`: skeleton `58 + (i*17 % 30)` px × 12, skeleton
  34×12, and a full-width second line `120 + (i*29 % 70)` px × 10.
- rows (buttons): grid `1fr auto`, gap `2px 8px`, padding 8, radius 4; hover `--surface-hover` (150 ms); selected
  flight: bg `--amber-soft` and the callsign in accent.
  - line 1: callsign (mono 13/650; `callsign || flight_number || "No callsign"`) | watchers (mono 13 px `--text-2`,
    right-aligned; `live_clicks` formatted with grouping only from 5 digits: `1443`, `12,345`).
  - line 2 (12 px `--text-2`, gap 4): `{from_iata||"?"}` `ArrowRight` 11/2 (`--text-3`) `{to_iata||"?"}` in mono —
    or `No route filed` when neither is known — then the aircraft type (`full_description || type`, `--text-3`,
    margin-left 4, ellipsis).
  - click → `select(flight_id, {fly: true})`.

Refresh: on becoming the focus and every 60 s (§3.4). The header stays on phones.

## 15. TrainList (`src/lib/TrainList.svelte`)

Props: `trains` (filtered, searched, sorted), `total`, `loaded`, `failed`, `sort` (two-way), `centre`, `selectedId`,
`onpick`, `onhover(id|null)`, `onshowall`.

- Header (hidden on phones): `Trains in view` (14/650) and, once loaded with `total > 0`, the listed count (mono 12
  px `--text-3`, en-GB grouping).
- Sort Segmented (padding `0 8px 8px`; tooltip `Distance is from the centre of the map`): `Delay` / `Speed` /
  `Name` / `Distance` (`delay|speed|name|distance`; default `delay`). Sorting rules: `sortTrains` (train-list.ts):
  delay and speed descending with unknowns last; ties and `name` by natural-order, case-insensitive name then id;
  distance nearest first (centre snapped to 0.05° steps, §3.2 `listCentre`).
- States:
  - not loaded and not failed: 6 skeleton rows (`70 + (i*23 % 50)` × 12, 52 × 12, wide `110 + (i*31 % 80)` × 10).
  - `total === 0`: `Couldn't reach the train feeds. Trying again shortly.` (failed) or `No trains in view.`; then
    the coverage line `Live trains are shown in Sweden, from Trafikverket.` (12 px `--text-3`).
  - nothing matches: `No trains in view match the search and filters.` + `.button` `Show all trains`.
- Rows (at most **150**; buttons; hover/focus also report `onhover(id)` so the map highlights the train, leave/blur
  report null): grid `minmax(0,1fr) auto`, 2 lines, gap `2px 8px`, padding 8, radius 4; hover `--surface-hover`;
  selected `--amber-soft` + name in accent.
  - line 1: 8×8 swatch (radius 2, `trainColorOf`, tooltip `{kind label} train`) + name (13/650, ellipsis) | delay
    badge (`delayText` capitalised: `On time`, `5 min late`, `2 min early`; tone `delayTone ?? neutral`) or empty.
  - line 2 (padding-left 16, 12 px `--text-2`): `ArrowRight` 11/2 (`--text-3`) + destination (ellipsis) — or the
    kind label in `--text-3` when there is no destination | measure (mono 12 px `--text-2`, right): when sorted by
    distance `distanceLabel(distanceKm(...))` (`850 m`, `12 km`, `1,240 km`), else `{round(speedKmh)} km/h` if known.
- Footer notes (11 px `--text-3`, padding `8px 8px 4px`): if more than 150: `{n} more; search or filter to narrow
  the list.`; always the coverage line.

## 16. ShipList (`src/lib/ShipList.svelte`)

As TrainList, with:

- Header `Ships in view` + count. Sort: `Size` / `Name` / `Distance` (`size|name|distance`; default `size`).
  `sortShips` (ship-list.ts): size class descending, then measured length descending (known before unknown), then
  name; unnamed ships after named; ties by MMSI.
- Skeleton widths: `80 + (i*23 % 60)`, 40, wide `100 + (i*31 % 70)`.
- Empty: `Couldn't reach VesselFinder. Trying again in a minute.` / `No ships in view.` + coverage `Ships in view
  from VesselFinder; zoom in to see more.`; no match: `No ships in view match the search and filters.` + `Show all
  ships`.
- Row line 1: 8 px **round** swatch (`shipColor(kind)`) + name (mono 13/650; if unnamed: `MMSI ` + mono number,
  weight 500 `--text-2`) | measure: distance when sorted by distance, else `{length} m` (`dims[0] + dims[1]`) or,
  when sizes are known, the size class label (`Small`/`Medium`/`Large`).
- Row line 2 (padding-left 16, gap 8): kind (`shipKindShort`, ellipsis) + `Under way` (heading known) / `Moored`
  (`--text-3`) | `Old position` warn badge (tooltip `This ship hasn't reported its position lately`) when `s.old`.
- More than 150: `{n} more; search or filter to narrow the list.`

---

## 18. Map overlays owned by the globe: HoverCard, RainOverlay

Both are mounted by `Globe.svelte` (Globe.svelte:3786-3788) but are plain UI.

### 18.1 HoverCard (`src/lib/HoverCard.svelte`)

The card that follows the pointer over an **aircraft** (airport labels, ships and trains use a plain tooltip — see
doc 03). Non-interactive, width **272**, positioned `16 px` right and below the pointer; flipped to the left/top
when it would cross `area − 8 px`; never closer than 8 px to the left/top edge.

- Photo slot above the body: 16:9, radius 4, bg `--surface-raised`, margin-bottom 6; a shimmer skeleton until the
  image has loaded, then the image fades in over 160 ms; credit `© {copyright}` bottom-left (11 px with shadow).
  The slot is open while details are `loading` or (`ready` and a photo exists); it collapses/expands its height
  over 180 ms ease-out. Image = first image's `thumbnail || medium`.
- Body (`.surface`, padding `8px 12px 12px`, column gap 2):
  - head row (min-height 26, gap 8): callsign (mono 14/700, ellipsis; `callsign || flight || "No callsign"`); type
    code chip (mono 11/600 `--text-2`, 1 px `--line-control` border, radius 4, padding `0 5`, line-height 16);
    registration (mono 11 px `--text-3`); airline logo right-aligned on a light tile (height 22, padding `2px 5px`,
    radius 4, bg `--text-1`; image max 18×72).
  - route (13 px `--text-2`, ellipsis): `{from} to {to}` / `From {from}` / `To {to}` / nothing.
  - progress bar (if known): 2 px, margin `8px 0 4px`, bg `--line`, fill accent scaled by `clamp(value, 0, 1)` (400
    ms); `--text-3` fill when it is a great-circle estimate rather than FR24's figure.
  - numbers (mono 12 px `--text-2`): `{alt} ft · {speed} kt` or `On ground · {speed} kt`.
- Data loading/caching (`HoverLoader`, `hoverInfo`, `routeProgress`) is in `src/lib/hover-details.ts`.

### 18.2 RainOverlay (`src/lib/RainOverlay.svelte`)

A full-window, non-interactive layer above the map (z 1) drawing rain streaks while `amount ≥ 0.02` and reduced
motion is off. Props: `amount` 0–1, `pitch` (deg), `night` 0–1.

- Three depths `{per, length, speed, line, alpha}`: `{300, 12, 900, 0.8, 0.22}`, `{190, 22, 1400, 1.0, 0.30}`,
  `{80, 38, 2100, 1.4, 0.40}` (drops per 10⁶ px² at full rain; px; px/s; line width px; opacity).
- Drop count per depth = `round(per · amount · width · height / 1e6)`; each drop has random x, y and a factor
  `k ∈ [0.75, 1.25)`.
- `lengthen = 0.3 + 0.7 · sin(clamp(pitch, 0, 85)°)`; `SLANT = 0.14` (px sideways per px down).
- Colour: `shade = round(215 − 120·night)`; `rgba(shade, shade+8, shade+20, alpha · (0.6 + 0.4·amount) · (1 −
  0.5·night))`.
- Per frame (`dt = min(0.05 s, elapsed)`): `y += speed·k·dt; x += that·SLANT`; when `y − length > height`: respawn at
  `y = −rand·40`, `x = rand·(width + height·SLANT) − height·SLANT`. Each drop is a line from `(x, y)` to `(x −
  length·SLANT·k, y − length·k)`.
- Port: a wgpu line/quad batch or `egui::Painter::line_segment` calls on a background layer.

---

## 19. MapControls, LayersMenu, SettingsMenu

### 19.1 MapControls (`src/lib/MapControls.svelte`)

A column (right-aligned, gap 8) of three small `.surface` stacks (each: column, padding 2, 32×32 icon buttons with
18/1.75 icons), bottom-right (§2.3):

1. Stack A:
   - **Map layers** (`Layers`; tooltip `Map layers`; pressed style while its popover is open).
   - **Settings** (`Settings`; tooltip `Settings`; pressed while open).
   - **API keys** (`KeyRound`; tooltip `API keys and Flightradar24 login`, or `API keys (signed in to
     Flightradar24)` with the icon in `--live` when signed in) → opens the KeysDialog (closing any popover).
     Hidden on phones (the entry is in the Settings menu instead).
   - **Keyboard shortcuts** (`Keyboard`; tooltip `Keyboard shortcuts (?)`) → opens ShortcutsHelp. Hidden on phones.
2. Stack B (hidden on phones): **Zoom in** (`Plus`; tooltip `Zoom in (+)`; disabled at zoom ≥ 18) and **Zoom out**
   (`Minus`; tooltip `Zoom out (−)`; disabled at zoom ≤ 0.5) → `zoomBy(±1)`.
3. Stack C:
   - **Tilt** (`Rotate3d`; pressed when `pitch > 5`; tooltip `Tilt to 3D (T, or drag with the right mouse button)`
     / `Look straight down (T)`) → `setPitch(tilted ? 0 : 55)`.
   - **Compass** (`Navigation2`, drawn in accent, rotated by `−bearing` degrees with a 150 ms ease; tooltip `Reset
     to north up (N)`; disabled — and grey — unless `|bearing| > 0.5` or tilted; on phones hidden while disabled)
     → `resetOrientation()`.

**Popovers** (one at a time): `.surface`, anchored to the left of stack A (`right: 100% + 8px; bottom: 0` relative
to that stack), width **300**, `max-height: calc(100vh − 60px)`, scrolls, padding 16, column gap 12. Enter
animation 180 ms ease-out from `opacity 0, translateX(6px)`. Closed by: the same button, a pointer-down anywhere
outside the controls, Escape (captured **before** the app's Escape handler; focus returns to the trigger), or
choosing the keys entry. The open menu is bound to the page (`menu`) so `closeTop()` can close it.

Phones: the controls sit at `right: max(8, safe-right); bottom: sheetHeight + 24` (transition 220 ms), showing only
Layers, Settings, Tilt and (when rotated) the compass. Popovers become bottom sheets of their own: fixed, full
width, `bottom: 0`, z 6 (above the BottomSheet), `max-height: 70dvh`, top radius 12, padding 16 (+ safe areas),
entering from `translateY(24px), opacity 0`. Phone landscape: `bottom: 24`.

### 19.2 LayersMenu (`src/lib/LayersMenu.svelte`) — "what is drawn"

Sections (column gap 6; each after the first has a 1 px `--line` top border and padding-top 12; section heading 12
px weight 600 `--text-3`, margin-bottom 4):

**Map**
- Segmented `Basemap` (legend hidden): `Dark` (key `1`) / `Satellite` (`2`) / `Realistic` (`3`) — the key hint is
  shown after each label; margin-bottom 4.
- Only when basemap = realistic:
  - Segmented with visible legend `Weather`, 3 columns × 2 rows: `Live` / `Sun` / `Clouds` / `Rain` / `Fog` /
    `Snow` (values `live|sunny|cloudy|rain|fog|snow` → `settings.sky`).
  - ToggleRow `Tilt-shift` — hint `Top and bottom blurred, like a miniature`.
- ToggleRow `Terrain` — hint `Mountains and valleys in relief` — key `E`.
- ToggleRow `3D buildings` — hint `Appear at street level` — key `B`.
- ToggleRow `Place names` — key `L`.
- ToggleRow `Rail map` — hint `Railway lines from OpenStreetMap` — key `R`.
- When the rail map is on: Segmented (legend hidden) `Speed limits` / `Signal blocks` (`speeds|blocks` →
  `settings.railShows`) and a small line (12 px `--text-3`): `Each stretch coloured by its limit` / `Free or taken,
  and by which train; at street level`.

**Aircraft** (only while aircraft are on the map)
- ToggleRow `Aircraft labels` — hint `Callsign next to each aircraft` — key `K`.

**Sky**
- ToggleRow `Weather radar` — hint `Rain and snow, every 10 min` — key `W`.
- ToggleRow `Day and night` — hint `Shades where the sun has set` — key `D`.

### 19.3 SettingsMenu (`src/lib/SettingsMenu.svelte`) — "how the app behaves"

Sections with column gap 12 (heading as above, pulled 4 px closer to its content).

**Display**
- Segmented with visible legend `Place name size`, 2 columns × 2 rows: `Small` (0.85) / `Medium` (1) / `Large`
  (1.25) / `Extra large` (1.5) → `settings.labelSize`. Disabled (opacity 0.4) when place names are off, with the
  note `Place names are off in Layers` (12 px `--text-3`).
- Slider: label row `Altitude exaggeration` … `×{n}` (mono, accent); range 1–40, step 1 → `settings.exaggeration`;
  note `Applies when zoomed out; true scale up close`. (A native range input tinted with the accent.)

**Data**
- ToggleRow `Aircraft-reported data` — hint `Heading, bank and autopilot, from adsb.lol` → `settings.adsb`.
- Phones only: `.button` with icon `KeyRound` 14/1.75: `API keys and Flightradar24 login`.

---

## 20. KeysDialog (`src/lib/KeysDialog.svelte`) — API keys and sign-in

A **modal** dialog: `.surface`, width 440 (`max-width: 100vw − 32`, `max-height: 100dvh − 32`), centred; backdrop
`oklch(0.1 0 0 / 0.5)` ≈ rgba(3,3,3,0.5); enter animation 180 ms from `opacity 0, translateY(6px)`. Clicking the
backdrop or pressing Escape closes it. While open, the app's shortcuts are off.

Content (column gap 12, padding `12px 16px 16px`):

1. Header: `API keys` (14 px weight 650) and a close icon button (`X` 16/1.75).
2. Group **Flightradar24** (top border `--line`, padding-top 12, column gap 8; legend 13 px weight 650) with a
   `Signed in` good badge when authenticated.
   - note (12 px `--text-2`): `Optional. A subscription unlocks filed flight plans, vertical speed, squawk and
     airspace, depending on your plan.`
   - field `Email` (email input), field `Password` (password input). Field label 12 px weight 600 `--text-2`;
     input height 34, padding `0 12`, 1 px `--line-control`, radius 4, bg `--ink`, 13 px; focus border accent.
   - when signed in: `.button` `Sign out`.
3. Group **Trafikverket**: note `Every train in Sweden, SJ included. Free key from api.trafikinfo.trafikverket.se.`
   (the host is an accent link to `https://api.trafikinfo.trafikverket.se/`, opened in the system browser); field
   `API key`.
4. Status line (12 px, min-height 1.2em; `--live` for success, `--danger` for errors).
5. Actions row (wrap, gap 8): primary button (accent bg, white text, no border; disabled → opacity 0.5) labelled
   `Save and sign in` when the form has both email and password and either differs from what is saved, else
   `Save`; a spinner (`LoaderCircle` 14/2) inside it while busy; disabled when busy or nothing changed. Spacer.
   `.button` `Import…`, `.button` `Export`.
6. Checkbox: `Include the Flightradar24 login in the export`.
7. Fine print (11 px `--text-3`): `Kept on this device in plain text. An exported file holds the keys as they are:
   share it only with people you'd give the keys to.`

Data (`src/lib/keys.ts`): `Keys = { fr24Email, fr24Password, trafiklabGtfs, trafiklabGtfsStatic, trafiklabOxyfi,
trafikverket }` (all strings; only email, password and trafikverket have fields). Backend commands: `get_keys`,
`save_keys({keys})` (returns the keys as stored, trimmed), `save_export({fileName, contents})`.

Behaviour:

- On open: clear the status, load the keys into `saved` and `form`; if opened from a "Sign in" prompt
  (`focusLogin`), focus the email field. On close (page's `closeKeys`): re-check the Trafikverket key status, and
  return focus to the prompt that opened it.
- **Save**: `save_keys`; if the login changed and both parts are set, call `onsignin(email, password)` (backend
  `sign_in`) and append `, signed in to Flightradar24` to the status. Status `Saved` (or the import message). Any
  error: its message (without a leading `Error: `) in red.
- **Sign out**: `onsignout()` (backend `sign_out`), then save the keys with an **empty password** (so the next
  launch does not sign in again); status `Signed out`.
- **Export**: file `rustradar-keys.json` = `{ "format": "rustradar-keys", "version": 1, "keys": { …non-empty
  fields… } }` pretty-printed with 2 spaces and a trailing newline; without the checkbox, `fr24Email` and
  `fr24Password` are left out. Saved through `save_export` (Downloads), revealed in the file manager; status
  `Saved {file} to Downloads`; failure `Couldn't export: {message}`.
- **Import**: pick a `.json` file; `readKeysFile`: must parse, have `format === "rustradar-keys"` and an object
  `keys`, else `That file isn't a RustRadar keys file.`; non-empty string fields replace the current ones, others
  stay; none → `That file has no keys in it.`. Then save immediately with status `Imported {n} key|keys` (plus the
  sign-in suffix if the login changed).

The launch sign-in (saved credentials) is done by the backend; the page learns of it through `session_info` and
`snapshot.authenticated` (§3.4).

---

## 21. ShortcutsHelp (`src/lib/ShortcutsHelp.svelte`)

Modal dialog, width 360 (`max-width: 100vw − 32`), same backdrop and enter animation as the keys dialog; padding
`12px 16px 16px`. Header `Keyboard shortcuts` (14/650) + close icon button; margin-bottom 8. Then rows (grid `72px
1fr`, gap 12, padding `5px 0`, 1 px `--line` top border): key caps on the left (gap 4; cap: min-width 22, height 22,
padding `0 5`, 1 px `--line-control` border with a 2 px bottom, radius 4, mono 12 px weight 600 `--text-1`),
description on the right (13 px `--text-2`).

| Keys | Description |
|---|---|
| `P` | Aircraft mode |
| `I` | Trains mode |
| `S` | Ships mode |
| `Shift+P` | Also show aircraft on the map, or not |
| `Shift+I` | Also show trains on the map, or not |
| `Shift+S` | Also show ships on the map, or not |
| `/` | Search the mode's traffic, or a place |
| `F` | Show or hide filters |
| `C` | Follow the selected aircraft with the camera |
| `Esc` | Close the open panel |
| `+` `−` | Zoom in or out |
| `T` | Tilt to 3D, or look straight down |
| `N` | North up |
| `1` `2` `3` | Dark map, satellite imagery or realistic 3D |
| `E` | Terrain |
| `B` | 3D buildings |
| `L` | Place names |
| `R` | Rail map |
| `K` | Aircraft labels (with the aircraft on the map) |
| `W` | Weather radar |
| `D` | Day and night |
| `?` | This list |
| `Ctrl` `Alt` `I` | Developer tools, for bug reports (⌘⌥I on a Mac) |
| `Ctrl` `Shift` `D` (dev builds) | API calls and the selected train's data (dev builds, ⌘⇧D on a Mac) |

(The "Developer tools" row only makes sense with a webview; drop or repurpose it.)

---

## 22. UpdateToast (`src/lib/UpdateToast.svelte`)

Mounted by the layout, above everything (z 30). `.surface`, fixed `left 16; bottom 40`, `max-width: min(440px,
100vw − 32px)`, row gap 12, padding `12px 12px 12px 16px`.

- On mount: backend `pending_update` → version string or null; event `update-ready` (payload: version) sets it.
- Shown while a version is known and not dismissed for that version.
- Contents: icon `RefreshCw` 16/1.75 (accent); title `RustRadar {version} is ready` (13 px weight 600); sub line
  (12 px `--text-3`): `Restart to start using it. It takes a few seconds.` — on Android `Download it, then open the
  file to install.` — or the error text; primary `.button` (accent bg and border, white text; hover slightly
  lighter (+0.05 OKLCH lightness ≈ #4c9bf5); disabled opacity 0.7): `Restart` → `Restarting…` (Android:
  `Download`) → backend `restart_to_update`; on failure show the error and re-enable; close icon button (`X`
  16/1.75; `aria-label` `Later`; tooltip `Later: it installs the next time RustRadar opens`, Android `Later`) which
  dismisses this version for the session.

### 22.1 Updater splash (`static/updater.html`, shown by `src-tauri/src/updater.rs`)

Not part of the Svelte app but a second piece of UI: a small window shown at launch **while an update installs,
before the main window opens**. It uses its own (older) palette, not the app tokens:

- Window (`open_splash`, updater.rs:153-165): 380×150, centred, not resizable, no decorations, always on top. Background `#04060b`, 1 px border `#1b2230`, text
  `#e9edf3`, system UI font 14 px/1.4, not selectable. Content centred vertically, padding `0 32px`, gap 14.
- `RustRadar` (15 px weight 650, letter-spacing 0.02em); status line (`#9aa5b8`); progress bar 3 px high, radius 2,
  track `#141a26`, fill **`#fcb442`** (the old amber accent), width transitions 150 ms ease-out.
- Indeterminate (size unknown): a 30 %-wide sliver sweeping from `translateX(-100%)` to `translateX(340%)` every
  1.2 s ease-in-out; with reduced motion a static full bar at 40 % opacity.
- Status texts sent from Rust: `Updating RustRadar…` (indeterminate), `Downloading update…` (with a fraction),
  `Installing…` (fraction 1).

Port: a small winit window (or the main window's first frames) drawing the same three elements.

---

## 23. Persisted settings

Storage key **`rustradar.v1`** (web `localStorage`), one JSON object written on every change of any part
(`filters.ts:200-230`):

```jsonc
{
  "settings": {
    "mode": "aircraft",          // "aircraft" | "trains" | "ships" — the focus
    "alsoShow": [],              // subset of the modes, also drawn on the map
    "basemap": "dark",           // "dark" | "satellite" | "realistic"
    "labels": true,              // place names
    "labelSize": 1,              // 0.85 | 1 | 1.25 | 1.5
    "buildings": true,
    "aircraftLabels": false,
    "daylight": true,            // day/night shading
    "weather": false,            // weather radar
    "adsb": true,                // adsb.lol data for the selected aircraft
    "railMap": false,
    "railShows": "speeds",       // "speeds" | "blocks"
    "sky": "live",               // "live" | "sunny" | "cloudy" | "rain" | "fog" | "snow"
    "tiltShift": false,
    "terrain": false,
    "exaggeration": 10           // 1..40
  },
  "filters": {                   // aircraft
    "categories": [0,1,2,3,4,5,6,7,8,9,10,11],
    "altitude": [0, 50000],      // feet; 50000 = no upper limit
    "airborne": true,
    "onGround": true,
    "airlines": [],              // ICAO designators
    "types": [],                 // type-code prefixes
    "airports": [],              // IATA codes
    "text": ""
  },
  "trainFilters": {
    "hiddenKinds": [],           // "highSpeed" | "intercity" | "regional" | "suburban" | "other"
    "hiddenOperators": [],       // operator names; "" = unknown operator
    "delayedOnly": false,
    "sort": "delay"              // "delay" | "speed" | "name" | "distance"
  },
  "shipFilters": {
    "hiddenKinds": [],           // ship colour keys: 0,2,3,4,5,6,7,8
    "motion": "all",             // "all" | "moving" | "moored"
    "hiddenSizes": [],           // 0 | 1 | 2
    "hideOld": false,
    "sort": "size"               // "size" | "name" | "distance"
  }
}
```

Loading rules (must be kept so existing users' settings survive if the port imports them; otherwise just use the
defaults):

- Unreadable JSON → all defaults.
- `settings`: start from the defaults; copy each stored key that exists in the defaults **and has the same JS
  type**; `alsoShow` = the known modes present in the stored array (else `[]`); unknown `sky` / `railShows` → the
  default. Legacy: `trainBlocks === true` without `railShows` → `railMap = true, railShows = "blocks"`; a missing/
  unknown `mode` → `aircraft` unless legacy `aircraft === false`, then `ships` unless `ships === false`, then
  `trains` unless `trains === false`.
- `filters`: defaults shallow-overlaid with the stored object (no validation).
- `trainFilters` / `shipFilters`: validated field by field (`mergeTrainFilters`, `mergeShipFilters`): unknown kinds,
  sizes, sorts, motions are dropped.

Other storage: `rustradar.keys` in localStorage is the **browser-only** fallback for the keys; the app keeps keys
in its config folder through the backend (`keys.rs`).

Not persisted: the camera view, selection, open panels, the phone sheet's detent, the FlightPanel's tab and its
"More from the aircraft" disclosure (session only), filter-panel open state, the dismissed update version.

Port recommendation: keep the same JSON schema in a `settings.json` in the app's config dir (serde with
`#[serde(default)]` per field gives the same "unknown/missing takes the default" behaviour). On first run of the
native build the old webview localStorage is not reachable in a portable way — see open questions.

---

## 24. Helper functions the UI text depends on

### 24.1 Freshness (`src/lib/freshness.ts`)

- `ageLabel(seconds)`: `< 60` → `{s} s`; `< 60 min` → `{m} min`; else `{h} h` or `{h} h {mm} min` (2-digit minutes).
- `feedFreshness(ageMs, intervalMs)`: stale when `ageMs > max(30 000, 2·intervalMs)`. Labels: `Updated {age} ago`
  (quiet) / `Data {age} old` (stale → warn badge).
- `afterRound`: only a round that brought data restarts the clock (and adopts that round's interval).

### 24.2 Aircraft wording

- `altitudeLabel(ft, onGround)`: `GND` on the ground; `≥ 18 000 ft` → `FL` + 3 digits of hundreds (`FL350`); else
  `{ft} ft` with thousands separators.
- `degrees(v)`: 3 digits + `°` (`007°`), rounded, wrapped to 0–359.
- `signInNote(missing)`: all three → `3 values need a Flightradar24 sign-in`; one → `Airspace needs a Flightradar24
  sign-in`; two → `Squawk and airspace need a Flightradar24 sign-in` (names: `vertical speed`, `squawk`,
  `airspace`; first letter capitalised). `missingWithoutSignIn`: empty when authenticated, else the values with
  nothing to show.
- `aircraftReadings(measured)` (`aircraft-readings.ts`):
  - always shown (`—` when absent): `Heading` (`degrees(trueHeading)`), `IAS` (`{n} kt`), `Mach` (3 decimals),
    `Wind` (wide; `{dir}° at {speed} kt`; tooltip `Derived from the aircraft's airspeed, heading and ground track`).
  - behind "More" (only those reported): `Bank` (`{abs deg}° R|L`, R if roll > 0.5, L if < −0.5), `GNSS altitude`
    (`{n} ft`), `TAS` (`{n} kt`), `Selected alt.` (`navAltitudeMcp ?? navAltitudeFms`, ft; tooltip `Set on the
    autopilot (or by the flight management system)`), `Selected hdg` (degrees; tooltip `Set on the autopilot;
    magnetic`), `OAT` (`{n} °C`), `Autopilot modes` (wide, UI font; modes joined by ` · `: `autopilot` →
    `Autopilot`, `vnav` → `VNAV`, `althold` → `Altitude hold`, `approach` → `Approach`, `lnav` → `LNAV`, `tcas` →
    `TCAS`, others upper-cased).
  - unreported names for the sentence: `bank`, `GNSS altitude`, `TAS`, `selected altitude`, `selected heading`,
    `OAT`, `autopilot modes`; `listing()` joins with commas and a final ` and `.
- Altitude colour ramp `ALTITUDE_STOPS` (geo.ts:22-30), ft → RGB: 0 → (255,98,0); 4 000 → (255,196,0); 8 000 →
  (160,230,40); 15 000 → (40,220,130); 25 000 → (0,200,230); 35 000 → (70,125,255); 45 000 → (205,85,255); linear
  between stops.
- `isRateLimited(message)`: regex `/status 8|too many requests/i`.
- Links: `ADSB_LOL_URL = https://adsb.lol`; airline logo
  `https://cdn.flightradar24.com/assets/airlines/logotypes/{id}.png`; ship photo
  `https://static.vesselfinder.net/ship-photo/{photoId}/1` (`/0` = thumbnail); ship page
  `https://www.vesselfinder.com/vessels/details/{mmsi}`.

### 24.3 Time (`src/lib/time.ts`) and `Clock.svelte`

- `clockAt(unix, tz)`: `HH:MM` 24-hour in the IANA zone (viewer's zone when absent/unknown); `—` when no time.
- `clockWithZone` = `{clock} {zoneName}`.
- `<Clock>`: the time in the mono face, a normal word space, then the zone name in the UI face in `--text-3`
  (`13:25 BST`); `—` without a time.
- `zoneName(unix, tz)`: the zone's common abbreviation at that moment (`BST`, `CEST`, `EDT`, `JST`…), from the
  browser's locale data via a list of English-ish locales, with fix-ups (`SAME_CLOCK`, `OWNED`, `COMMON_NAMES`
  tables, time.ts:101-151), else `UTC`, `UTC+3`, `UTC+5:45`, `UTC−4` (true minus sign). **Port risk**: Rust has no
  Intl; needs `chrono-tz`/`jiff` plus an abbreviation table (tz database abbreviations are close but not identical:
  see open questions).
- `dayAt(unix, tz, now)`: `Today` / `Yesterday` / `Tomorrow`, else `Mon 6 Oct` style (viewer's locale).
- `duration(seconds)`: `{h} h {mm} min` or `{m} min`.
- `delayMinutes(scheduled, actual)`: rounded minutes, null unless both known.
- `punctuality(min)`: `≥ 45` → `{duration} late` (bad); `≥ 15` → `{n} min late` (warn); `≤ −5` → `{n} min early`
  (good); else `On time` (good).
- `toneOf(color)`: `green` → good; `yellow`/`orange` → warn; `red` → bad; else none.
- AIRAC (`airacCycle()` in routes.ts): `{ident, effective, expires}`; dates shown as `d MMM` in UTC.

### 24.4 Trains (`src/lib/trains.ts`, `train-list.ts`)

- Kinds, labels, colours (RGB): `highSpeed` `High-speed` (226,92,196); `intercity` `Intercity` (128,132,246);
  `regional` `Regional` (96,212,160); `suburban` `Suburban` (112,190,232); `other` `Other` (158,150,140).
- Operator colours override the kind's (`trainColorOf`; first regex match on the trimmed operator name,
  trains.ts:30-47): `^sj\b` (232,232,236); `^sl\b|storstockholms` (70,146,236); `mälartåg` (236,84,84);
  `västtrafik|västtåg` (62,200,236); `skånetrafiken|pågatåg` (196,92,222); `öresundståg` (150,132,246); `arlanda
  express` (250,212,64); `snälltåget` (120,214,92); `norrtåg` (58,204,176); `^vy\b` (240,92,150); `bergslagen`
  (244,152,56); `upptåget|^ul\b` (250,120,40); `östgöta` (214,70,130); `x-trafik|x-tåget` (150,220,60);
  `krösatåg|kronoberg|jönköping|kalmar|blekinge` (236,120,110); `värmland` (110,180,255). All case-insensitive.
- `delayText(min)`: `` (unknown), `on time`, `{n} min late`, `{n} min early`. `delayTone`: `≥ 30` bad, `≥ 5` warn,
  else good; null when unknown. "Delayed only" = `delayMinutes ≥ 5`.
- `trainPositionAge`: `Position {age} ago`; stale past 300 s → `Position {age} old`.
- `trainRunning`: hidden 30 s after `endedUnix`.
- Credits: `TRAIN_CREDITS.trafikverket = { text: "Trains in Sweden: Trafikverket, CC0", url:
  "https://data.trafikverket.se/" }`; `MODEL_CREDIT = { text: "3D trains: Lardman", url:
  "https://3dwarehouse.sketchup.com/user/1082279160696486794315506/Lardman" }` (added when any train on the map has
  a 3D model). `TRAIN_SOURCE_NAMES.trafikverket = "Trafikverket"`.
- `stopView`, `nextStop`, `trainProduct`, `positionAt`, `headingAt`: trains.ts:98-135, 269-300 (port verbatim).
- Search match (`trainMatches`): every word (accent- and case-folded) appears in `name + destination + operator`.

### 24.5 Ships (`src/lib/ships.ts`, `ship-list.ts`, `ship-details.ts`) and attribution strings

- Kind keys, labels, colours: 0 `Unknown type` (140,152,168); 2 `Tug or special craft` (72,198,194); 3 `Passenger`
  (98,152,240); 4 `Cargo` (116,188,112); 5 `Fishing` (236,128,156); 6 `Tanker` (218,92,92); 7 `Search and rescue or
  law enforcement` (228,234,242); 8 `Pleasure craft or sailing` (192,136,236); any other kind: label `Other`, the
  unknown's colour, grouped under key 0.
- `positionAge`: `Position {age} ago`; stale past 900 s → `Position {age} old`. `navTone`: AIS status codes 2, 6,
  14 → warn, else neutral.
- `dimensions`: `{L} × {B} m` / `{L} m` / `{B} m beam`; `draught`: `{cur} / {max} m` / `{cur} m` / `max {max} m` (1
  decimal). `latLon`: `51.9213° N, 4.4812° E` (4 decimals). `isLocode`: `^[A-Z]{2} ?[A-Z2-9]{3}$`.
- Search match (`shipMatches`): all-digit query → MMSI prefix (or name contains); else every folded word in the
  name.
- Attribution constants: `RAIL_SPEED_ATTRIBUTION = "Rail: OpenRailwayMap, © OpenStreetMap contributors"`;
  `TERRAIN_ATTRIBUTION = "Terrain: © Mapterhorn (Lantmäteriet, Kartverket, Copernicus GLO-30 and others); Mapzen
  Terrain Tiles (AWS Open Data): USGS 3DEP, SRTM, GMTED2010; NOAA ETOPO1; Copernicus EU-DEM; and others"`;
  `SGL_ATTRIBUTION = "Map data close in: Streets GL (tiles.streets.gl), © OpenStreetMap contributors"` with
  `SGL_ZOOM = 16`; `OVERTURE_ATTRIBUTION = "Building heights: © Overture Maps Foundation, © OpenStreetMap (ODbL)"`;
  `LANDCOVER_ATTRIBUTION = "Land cover: NMD2023 Naturvårdsverket · ESA WorldCover 2021"`; `CANOPY_ATTRIBUTION =
  "Canopy height: Meta & WRI (CC BY 4.0), via TGE Labs"`; `STREET_TREES_ATTRIBUTION = "Street trees: Malmö stad,
  Umeå kommun (CC0)"`; `CLOUDS_ATTRIBUTION = "Clouds: © EUMETSAT"`.

### 24.6 Backend command names (Tauri `invoke`) used from the UI

`live_flights {area, services}`, `flight_details {flightId}`, `top_flights`, `search {query}`, `geocode {query,
lat, lon, lang}`, `reference_data`, `airport_board {code, mode, page}`, `aircraft_history {registration}`,
`ships_in_view {west, south, east, north, zoom}`, `ship_details {mmsi}`, `opensky_track {icao24}`, `adsb_hex
{hex}`, `adsb_callsign {callsign}`, `session_info`, `sign_in {email, password}`, `sign_out`, `follow_flight
{flightId, onEvent: channel}`, `unfollow_flight`, `trains_in_view {west, south, east, north, zoom}`, `train_media
{source, product, operator}`, `follow_train {id, onEvent: channel}`, `unfollow_train`, `train_route {id}`,
`watch_trains {ids}`, `hide_train_operators {names}`, `train_stops {id}`, `trafikverket_key_status`, `get_keys`,
`save_keys {keys}`, `save_export {fileName, contents}`, `pending_update`, `restart_to_update`, `open_devtools`,
`dev_log {since}`, `dev_log_clear`. Events: `trains-changed`, `update-ready`. In the port these become direct
async Rust calls / channels; the semantics in §3 (request counters, drop-stale-answers, debounce) must be kept in
the UI's controller.

Place search: `placeLanguage()` = the UI language's first two letters if one of `en, de, fr, it`, else none.
`placeDetail(place)` = `{category ?? kind word}` + ` · {label without the leading "name, "}`; kind words: `Address`,
`Street`, `Place`, `Airport`, `District`, `City`, `Town`, `Postcode`, `Region`, `Country`. `placeView`: fly-to zoom
by kind — address 17.5, poi 16.5, street 16, district 14, postcode 13, airport 13, town 12, city 11, region 7,
country 5 — or fitted to the bbox (70 % of the view) clamped to `[kind − 2, kind + 1.5]`, then to `[2, 17.5]`
(places.ts:1-51).

---

## 25. DevPanel (`src/lib/DevPanel.svelte`) — dev builds only

Toggled with Ctrl/Cmd+Shift+D. `.surface` docked to the right edge (`top 0; right 0; bottom 0; width min(560px,
100%)`, z 50), mono 12 px/1.4.

- Header: **Dev**, tabs `API calls` / `Selected train` (active: bg `--text-1`, text `--surface`), close `×`.
- **API calls**: toolbar — source select (`All sources` + each source seen), search input (`Search URL or data`),
  `Pause`/`Resume`, `Clear` (also `dev_log_clear`). Polls `dev_log {since: lastId}` every 1 s while open and not
  paused; keeps the last 400 entries; newest first. Entry `{id, atMs, source, method, url, request, status, ms,
  size, body, error}`. Row: time (`HH:MM:SS`), source (bold), status (or `failed`; `--warn` when error or status
  ≥ 400), `{size} · {ms} ms` (size as `B` / `kB` / `MB` with 1 decimal), then `{method} {url}` on a second line
  (ellipsis). Click expands: error text, `Request` (+ `Copy`) and `Response` (+ `Copy`) in `<pre>` blocks (bg
  ≈ #030303, max-height 50vh, pretty-printed JSON, XML one tag per line). Empty: `No calls yet. They show up as the
  app makes them (dev builds only).`
- **Selected train**: `Copy` + the train id (or `Click a train on the map`), then a JSON dump of `{train (with
  frames summarised), stops, route {complete, segments summary, stops}}`.

Low priority for the port; an egui window is fine.

---

## 26. Icon inventory (`@lucide/svelte`)

All icons are Lucide outline icons (24×24 viewBox, `stroke="currentColor"`, round caps/joins). Sizes/stroke widths
as used:

| Icon | Where (size / stroke) |
|---|---|
| `Radar` | wordmark (20 / 1.75) |
| `Funnel` | Filters button (14 / 1.75) |
| `RefreshCw` | Retry / Try again buttons (14 / 1.75), update toast (16 / 1.75) |
| `TriangleAlert` | sidebar alerts (16 / 1.75) |
| `Plane` | ModeBar aircraft (15 / 1.75), search live row (14 / 1.75), progress marker (14 / 2, rotated 45°) |
| `TrainFront` | ModeBar trains (15 / 1.75), search local rows (14 / 1.75) |
| `Ship` | ModeBar ships (15 / 1.75), search local rows (14 / 1.75) |
| `Eye` | ModeBar "also show" (14 / 1.75), TopFlights "Watching" (13 / 1.75) |
| `Search` | search field (16 / 1.75) |
| `LoaderCircle` | spinners (14 / 2; 12 / 2 in search messages) |
| `X` | close buttons (18 / 1.75 panels; 16 / 1.75 filters, dialogs, toast; 14 / 1.75 search clear; 12 / 2 token chip) |
| `MapPin` | search airport row / fallback (14 / 1.75) |
| `House`, `Signpost`, `Landmark`, `PlaneLanding`, `Building`, `Mailbox`, `Map`, `Flag` | place kinds (14 / 1.75) |
| `ArrowLeft` | flight panel back row (14 / 2) |
| `ChevronLeft`, `ChevronRight` | photo nav (18 / 2); disclosure chevron (`ChevronRight` 14 / 2) |
| `ExternalLink` | external buttons (14 / 1.75), adsb.lol link (11 / 2) |
| `LocateFixed` | Follow, Show on map (14 / 1.75) |
| `LockKeyhole` | sign-in note (12 / 2) |
| `PlaneTakeoff`, `PlaneLanding` | airport board tabs (15 / 1.75) |
| `ArrowRight` | list rows (11 / 2) |
| `Check` | chips (12 / 2.5) |
| `Layers`, `Settings`, `KeyRound`, `Keyboard`, `Plus`, `Minus`, `Rotate3d`, `Navigation2` | map controls (18 / 1.75); `KeyRound` also 14 / 1.75 in the settings menu |

Port: embed the ~45 Lucide SVGs and render them as tinted textures (e.g. `egui_extras` SVG loader / `resvg` at the
needed pixel sizes × scale factor), or use an icon font built from Lucide (`lucide-icons` crates exist); stroke
widths 1.75 vs 2 vs 2.5 differ per use, which favours SVG rasterisation with a stroke-width parameter.

---

## 27. Animation and transition inventory

| What | Spec |
|---|---|
| Easing everywhere | `cubic-bezier(0.16, 1, 0.3, 1)` ("ease-out expo-like") |
| Hover/pressed colour changes (buttons, rows, chips, inputs' borders) | 150 ms |
| Detail panel enter (desktop) | 200 ms, from opacity 0 + 8 px to the right |
| Popover enter (layers/settings) | 180 ms, from opacity 0 + 6 px right (phones: 24 px below) |
| Dialog enter | 180 ms, from opacity 0 + 6 px below |
| MapControls shift when a panel opens | `right` 200 ms (phones: `bottom` 220 ms) |
| Phone top bar hide/show | 220 ms translate + fade |
| Bottom sheet height | 220 ms (none while dragging) |
| Flight progress fill + marker | 600 ms |
| Hover-card progress fill | 400 ms; photo fade 160 ms; photo slot open/close 180 ms |
| Switch knob | 150 ms slide 14 px |
| ModeBar eye toggle reveal | 150 ms width/margin/opacity |
| Disclosure chevron | 150 ms rotate 90° |
| Compass needle | 150 ms rotate |
| Board busy dim | opacity 0.75, 150 ms |
| Skeleton shimmer | 1.4 s linear infinite |
| Spinner | 900 ms per turn, linear, infinite |
| Live dot pulse | 2 s ease-out infinite ring 0 → 6 px |
| Reduced motion | everything effectively instant; rain overlay off |

While any of the infinite animations is on screen (skeleton, spinner, live dot) egui must keep repainting
(`ctx.request_repaint()`); otherwise repaint on input, data arrival and the 1 s clock tick.

---

## 28. Z-order (top to bottom)

DevPanel (50) → UpdateToast (30) → modal dialogs (browser top layer) → search dropdown (10, within the sidebar) →
phone popover sheets (6) → dev readout (5), ExportMenu menu (5, within the panel) → BottomSheet (4) → phone top bar
(3) → phone attribution (2), HoverCard (2, within the globe) → RainOverlay (1) → globe canvas. Desktop sidebar,
ModeBar, detail panels, MapControls and attribution have no explicit z-index: they stack in document order (globe,
ModeBar, sidebar, panels, MapControls, …, attribution).

---

## 29. Notes for the egui implementation

- **Pointer pass-through**: the sidebar wrapper and the ModeBar wrapper are click-through; only the cards take
  input. In egui: separate `Area`s per card, and treat `ctx.is_pointer_over_area()` / `wants_pointer_input()` as the
  switch between UI and map gestures.
- **Layout**: fixed widths (sidebar 320, detail 360, popover 300, dialogs 440/360, hover card 272) make absolute
  `Area` placement straightforward. The sidebar's shrink priority (list shrinks 3× faster than the filter panel,
  list min 96) needs a small manual height solver: `available = window_h − 16 − 40`; give the primary card its
  natural height, then split the rest between filter panel and list by their natural heights with shrink weights
  1:3, each scrolling internally.
- **Fonts**: weights 550/650/750 do not exist as static instances; pick the nearest (Medium/SemiBold/Bold) or
  generate instances from the variable fonts with `fonttools` at build time and register each as its own egui font
  family (`overpass-400`, `overpass-550`, …). Letter-spacing −0.01em on mono and tabular figures: Overpass Mono is
  already tabular; egui has no letter-spacing (ignore, or pre-kern at 13 px ≈ −0.13 px: invisible).
- **Text ellipsis** is used heavily (`white-space: nowrap; text-overflow: ellipsis`): use `Label::truncate()`.
- **Tooltips**: every `title` attribute above is a tooltip (`on_hover_text`). They carry real information (units,
  time zones, shortcut hints): keep them.
- **Images** (aircraft photos, airline/operator logos, ship/train photos) load from the network: use an async
  texture cache keyed by URL with failure memo (the `failedPhoto`/`logoFailed` states), `object-fit: cover`
  cropping, and the skeleton until loaded.
- **Text input** needs IME/soft-keyboard handling on Android/iOS (winit + egui support is uneven): the search box,
  the filter text, token inputs and the keys dialog are the only text fields.
- **Safe areas** (notch, home indicator) are used on phones for the top bar, the sheet's bottom padding, the
  controls' right inset and the popover sheets: needs per-platform insets from winit/the activity.
- **Accessibility**: the source has ARIA roles, live regions and full keyboard operation (tabs with arrow keys,
  listbox, slider chart, focus return after dialogs). egui + AccessKit covers part of this; parity is a stretch
  goal after features.

---

## 30. Open questions and risks

1. **Time-zone abbreviations** (`zoneName`): the web build leans on the browser's Intl locale data plus hand
   tables to print `BST`, `CEST`, `JST`, `KST`, `UTC+5:45`. Rust needs `chrono-tz`/`jiff` and its own table; tz
   database abbreviations differ in places (e.g. numeric `+03` for many zones). Decide: port the tables and accept
   small differences, or bundle ICU (`icu_datetime`) — heavier, especially on mobile. `time.test.ts` lists the
   expected outputs.
2. **Locale-dependent formatting**: several numbers use the viewer's locale (`toLocaleString()` without a locale:
   counts in the toolbar, feet, km, fpm; `dayAt`'s `Mon 6 Oct`), others are pinned to `en-GB`. A native build has
   no free locale formatter: propose pinning everything to en-GB style (`12,345`, `Mon 6 Oct`) unless the owner
   wants OS-locale formatting.
3. **Variable font weights** 550/650/750 are not available in egui (no variable-font axis support): static
   instances must be generated, or weights rounded to 500/600/700 — a small but visible difference in headings and
   buttons. Also the "Middle Dot" fallback glyph must be reproduced by a fallback font.
4. **Existing users' settings** live in the webview's localStorage (`rustradar.v1`), which the native app cannot
   read portably (WebView2/WKWebView/WebKitGTK each store it differently). Options: (a) ship one last Tauri release
   that copies the settings into a JSON file in the config dir, (b) accept a reset to defaults. Keys are already
   in the backend's config folder, so sign-in and the Trafikverket key survive either way.
5. **Web-only affordances to redefine**: Ctrl/Cmd+Alt+I (web inspector) and its row in the shortcuts list; the
   "plain browser" alert (`No live … data in a plain browser.`); `#@lat,lon,…` and `#demo` URL hashes (dev only);
   the context-menu suppression. Suggested: drop the first two, replace the hashes with CLI flags.
6. **File import/export dialogs**: keys import uses a browser file picker; exports go to Downloads via
   `save_export` and are revealed in the file manager. Native equivalents (`rfd` on desktop; Android SAF / iOS
   document picker on mobile) are needed; `rfd` does not cover mobile.
7. **Opening URLs** (`openUrl`: JetPhotos, Flightradar24, VesselFinder, Wikipedia, Trafikverket, adsb.lol, train
   credits) needs a per-platform opener (`open`/`webbrowser` crates on desktop; intents on Android; `UIApplication`
   on iOS).
8. **Mobile text input and safe areas** with winit + egui are the least mature part of the stack (soft keyboard
   show/hide, IME composition, insets). The phone layout depends on both (search bar, keys dialog, bottom sheet).
9. **Android back gesture**: today done with a history entry; natively it must be routed from the activity to
   `closeTop()`.
10. **Update flow on mobile/desktop**: UpdateToast relies on backend commands `pending_update` /
    `restart_to_update` and the `update-ready` event from `tauri-plugin-updater`; without Tauri the updater itself
    needs replacing (self-update crate or store updates), which changes or removes this toast and the splash.
11. **Remote images** in panels and the hover card (JetPhotos, FR24 logos, VesselFinder, Wikimedia): need an async
    HTTP + decode + texture cache; some hosts may require a Referer/User-Agent that the webview used to send
    implicitly — to be verified per host.
12. **Trail gradient stops in the Route tab legend** use `ALTITUDE_STOPS` positions (0, 8.9, 17.8, 33.3, 55.6,
    77.8, 100 %): egui has no multi-stop gradient primitive; draw as a small mesh with per-vertex colours.
13. **Phone breakpoint on desktop**: with the 900×600 minimum window the phone layout is unreachable on desktop
    today. Should the native desktop build keep that minimum (simplest), or allow narrow windows and switch
    layouts live?
14. **Scroll-under-sticky-footer behaviour** in the detail panels (sticky footer, `scroll-padding-bottom`, the
    "reveal More" auto-scroll): in egui the natural equivalent is a fixed bottom bar + a `ScrollArea` above it,
    which changes nothing visually but should be confirmed as acceptable.
15. **Pixel parity of text**: egui's text layout (no kerning tweaks, different hinting/gamma, line-height handling)
    will not match Chromium exactly; "looks the same" should be judged on layout, colours and sizes rather than
    glyph-level identity.
