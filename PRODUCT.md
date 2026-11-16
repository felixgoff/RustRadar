# Product

<!-- impeccable:product-schema 1 -->

## Platform

native (desktop: Linux, Windows, macOS; phones: Android, iOS)

## Users

Aviation enthusiasts: planespotters and flight fans watching live traffic for the interest of it. They follow a flight, look closely at the aircraft and its airline, check an airport's board, and watch weather and routes. Most sit at a desktop with a mouse and keyboard, usually with the app open for long stretches; some check it on a phone.

## Product Purpose

RustRadar is a live flight radar on a 3D globe, with trains (Sweden) and ships as further modes. It shows every tracked aircraft worldwide (about 20,000), in motion, and lets the user follow one in detail: position, altitude profile, route, aircraft, and recent flights. Success is that watching real air traffic feels like looking at the real sky from above, and that what the user reads is trustworthy.

## Positioning

A real 3D globe where each aircraft is a true-scale 3D model in its airline's colours, altitude is shown as real height, and the map has 3D buildings, terrain and measured 3D weather (US, central Europe), drawn by a native Rust and wgpu renderer on desktop and phone. A browser-based flat tracker could not truthfully claim this.

## Operating Context

- Native window (winit + wgpu + egui), used with mouse and keyboard; right-drag tilts and rotates the camera, `/` searches, many single-key shortcuts. On phones, touch gestures and a bottom sheet.
- Live data from Flightradar24's unofficial gRPC and JSON APIs; airport boards, aircraft history and flight details come from the same source. Reference data (airlines, airports) is cached weekly.
- Zoom spans the whole globe to street level at an airport with 3D buildings.

## Capabilities and Constraints

- Features: live aircraft with per-type 3D models and airline liveries; filters (category, altitude, airline, type, airport, text); flight panel with altitude chart, route, aircraft and history tabs; airport departure and arrival boards; follow-camera; day and night shading; 3D weather radar (NOAA MRMS over the US, DWD over central Europe, RainViewer elsewhere); dark, satellite and realistic 3D basemaps with terrain; trains and ships modes; optional Flightradar24 sign-in.
- Desktop first; below 900 px wide the phone layout takes over (full-screen map, bottom sheet).
- The data source is unofficial and rate-limited, so the interface must degrade gracefully when requests are limited or offline, and credit its sources.
- A route is shown as filed only when Flightradar24 supplies a flight plan (subscribers); otherwise it is a clearly labelled estimate. A guessed route must never read as a filed one. The AIRAC cycle in force is computed, not fetched.
- Flightradar24 sign-in is optional; the login is kept in the app's config folder (`keys.json`) and signs in at launch.

## Evidence on Hand

- A working app with live data; no testimonials, customers, usage figures or press, and none should be invented.
- Airline logos come from Flightradar24 and pics.avs.io; photos come from Flightradar24's JetPhotos links with credit.

## Product Principles

1. Show real things at real scale: models, heights, weather. Decoration that implies data the app does not have is out.
2. Be honest about certainty: filed versus estimated, live versus stale, measured versus inferred.
3. The globe is the product; panels serve it and get out of the way.
4. Built for long sessions by people who love the subject: dense, calm, and fast to scan.
