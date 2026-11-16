# RustRadar

A live flight radar on a 3D globe, also showing trains and ships, written in Rust and drawn
with wgpu (Vulkan, Metal, DX12). One code base runs on Linux, Windows, macOS, Android and iOS.

History: RustRadar began as a Tauri + SvelteKit + deck.gl app and was ported to a native
winit + wgpu + egui app in 2026 (the plan and notes are in `docs/port/`).

## What it does

- **Every live aircraft** (~20k worldwide), drawn from the real shape of its type: about 99.7% of
  traffic resolves to one of 40 aircraft models, the rest keep built-in low-poly shapes. Flat and
  wide you see top-down silhouettes; tilted, the real mesh; from zoom 11, a finer one. Aircraft are
  true to scale within sensible on-screen limits and dead-reckoned between feed refreshes.
- **Airline liveries** up close: about 60 hand-made schemes, every other airline coloured from its
  logo (cached on disk). Military aircraft are grey.
- **Follow a flight**: click an aircraft (or pick one from search, "Most tracked" or an airport
  board). The panel shows airline, origin and destination with local times and delays, an
  altitude chart, the route, aircraft details and photos, and recent flights; the 3D trail is
  coloured by altitude. `C` makes the camera follow it.
- **Routes**: with a Flightradar24 subscription that includes flight plans, the filed route of the
  current AIRAC cycle is drawn. Without one, the track flown so far (filled in from the OpenSky
  Network where it saw more) and the great circle to the destination, labelled as an estimate.
- **Filters**: categories (filtered by the server), altitude, airborne or on ground, airlines,
  types, airports and free text. Filters and map settings persist.
- **Airport boards**: live departures and arrivals in local time.
- **Trains** (Sweden, Trafikverket) with 3D cars, routes and the rail map, and **ships**
  (VesselFinder) with hull models.
- **Weather radar** in 3D (`W`): measured volumes over the US and southern Canada (NOAA MRMS) and
  central Europe (DWD), heights estimated from intensity elsewhere (RainViewer).
- **Three basemaps**: a dark vector map (OpenFreeMap), satellite imagery (Esri), and a realistic
  3D map after Streets GL with textured ground, buildings with façades and roofs, trees, terrain,
  sky, sun shadows, clouds and live or chosen weather.
- **Day and night** shading with the three twilights, and 3D buildings from zoom 13.

Keyboard (`?` lists them): `P` / `I` / `S` aircraft, trains or ships (with `Shift`, also show
them), `/` search, `F` filters, `C` follow, `Esc` close, `+` / `-` zoom, `T` tilt, `N` north up,
`1` / `2` / `3` dark, satellite or realistic, `E` terrain, `B` buildings, `L` place names, `R`
rail map, `K` aircraft labels, `W` weather radar, `D` day and night.

Optional: a Flightradar24 account raises the flight limit and, depending on the plan, unlocks
filed flight plans, squawk, vertical speed and airspace. Enter it on the API keys page (the key
button in the map controls); the same page holds the Trafiklab and Trafikverket keys and exports
or imports them as a file. Or set `fr24_username` / `fr24_password` (or `fr24_subscription_key` /
`fr24_token`) in the environment (see `crates/fr24/README.md`).

## Layout

```
crates/fr24/     Rust port of the fr24 Python library: gRPC-web + protobuf, JSON API, auth
crates/core/     pure logic: geometry, settings, filters, formats, trains, labels, assets
crates/backend/  network and disk: feeds, tiles, radar, weather, images, keys, caches
crates/scene/    what to draw: per-frame lists for every layer, tessellation, picking
crates/map/      wgpu renderer: pipelines, shaders, tile runtime, textures, headless render
crates/app/      the app (package rustradar): window, input, state, egui UI, updater, platforms
crates/xtask/    developer and release tooling (`cargo xtask`)
assets/          fonts, icons, textures, aircraft/train meshes, rail and tree data
platform/        Android (Gradle), iOS (xcodegen), Linux/macOS/Windows packaging, app icons
scripts/         asset pipelines, release version, CI helpers
docs/port/       architecture and port notes
```

Build output goes to `./target`. fr24 is ported rather than wrapped with PyO3 so that the app
stays a single native binary; its `.proto` files are compiled with `protox` (no `protoc` needed).

## Build and run

### Desktop (Linux, Windows, macOS)

Install Rust with [rustup](https://rustup.rs) (CI uses 1.97.0). The build compiles C and C++
(Basis Universal, aws-lc) and runs bindgen, so it needs a C/C++ toolchain, CMake and libclang.

- **Linux** (Ubuntu/Debian):
  `sudo apt install build-essential clang libclang-dev cmake pkg-config libxkbcommon-x11-0 libvulkan1 mesa-vulkan-drivers`
  (the last three are needed at run time: winit loads xkbcommon-x11, wgpu needs a Vulkan driver).
- **macOS**: the Xcode command line tools and `brew install cmake`.
- **Windows**: Visual Studio Build Tools (C++), CMake and LLVM (for libclang). Without NASM, set
  `AWS_LC_SYS_PREBUILT_NASM=1`.

```sh
cargo run -p rustradar                      # debug build, opens the window
cargo run -p rustradar --release            # much faster rendering
cargo run -p rustradar -- --help            # every option
```

Options worth knowing: `--view LAT,LON,ZOOM[,PITCH[,BEARING]]`, `--basemap dark|satellite|realistic`,
`--demo` (demo trains), `--set KEY=VALUE` (any setting for this run only, not saved).

### Android

Needs JDK 17, the Android SDK (`ANDROID_HOME`), **NDK r28c** (`28.2.13676358`; r27's clang
crashes on the Basis transcoder) as `ANDROID_NDK_HOME`, and cargo-ndk 4:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
platform/android/build-rust.sh              # debug, arm64-v8a: the .so and the APK's assets
cd platform/android && ./gradlew assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb logcat -s rustradar
```

`platform/android/build-rust.sh --release armeabi-v7a x86_64` builds other ABIs. The release APK
(four ABIs, checked and named, signed when the keystore secrets are set) is
`cargo xtask android-release --out dist`.

### iOS

On a Mac with Xcode and [xcodegen](https://github.com/yonaskolb/XcodeGen) (`brew install xcodegen`):

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cd platform/ios && xcodegen generate && open RustRadar.xcodeproj
```

Set your team under Signing & Capabilities and Run. A build phase runs
`platform/ios/build-rust.sh`, which builds the Rust library for the selected device or simulator.
The release IPA is `cargo xtask ios-release --unsigned --out dist`. More device notes:
`docs/port/device-notes.md`.

## Test

```sh
cargo test --workspace                      # everything (the map's GPU tests are slow on a CPU driver)
cargo test -p rustradar-core                # one crate
cargo clippy --all-targets --workspace -- -D warnings
cargo run -p fr24 --example smoke           # every fr24 endpoint against the live API
```

GPU tests need a Vulkan adapter; without one they print `skipped:` and pass. CI runs them on
SwiftShader (`VK_ICD_FILENAMES` pointing at Chrome's `vk_swiftshader_icd.json`); lavapipe
(`mesa-vulkan-drivers`) works too. Shots the tests draw land in `target/shots/`.

### Headless screenshots

The app binary renders one frame without a window and exits:

```sh
cargo run -p rustradar -- --screenshot out.png --size 1440x900 \
  --view 59.6519,17.9186,13.5,45,30 --basemap dark --now 1760000000000 \
  --fixture crates/map/tests/data --demo --ui
```

`--fixture DIR` reads tiles and feeds from a folder instead of the network (the committed
fixtures in `crates/map/tests/data`, refreshed by `cargo xtask fixtures`, hold tiles, radar,
clouds and weather but no live flights, so the example shows the demo trains); `--ui` draws the
interface over the map; `--mode`, `--select ID`, `--scale`, `--wait SECONDS` and `--set KEY=VALUE`
set up the rest. Named views V1 to V8 and the reference pictures are in `docs/port/reference/`.

## Releases and versions

Everything is in `.github/workflows/`:

- `rust.yml` ("Build") runs on every push and pull request: tests and clippy, cross checks for
  Windows, macOS, iOS and Android, and every release package (deb, rpm, AppImage, NSIS, MSI, `.app`,
  dmg, APK, IPA). On `rust-port`, pull requests and manual runs it is a dry run: packages are
  workflow artifacts, signed with a throwaway key. **A push to `main` or a `v*` tag publishes**: a
  GitHub release with the packages, signed updater files and `latest.json`, the APK, and the iOS
  build to TestFlight.
- The version lives in `crates/app/Cargo.toml` and `crates/backend/Cargo.toml` (and their lock
  entries). `cargo xtask version` prints it, `cargo xtask bump patch|minor|major|x.y.z` changes it;
  `bump.yml` ("Bump version", run by hand) does the same and commits it. If `main` is pushed
  without a bump, `scripts/release-version.sh` publishes the next patch after the latest release,
  since installed apps only update to a newer version.
- `testflight-refresh.yml` rebuilds the latest release for TestFlight before its build expires.

Desktop installs update themselves from `latest.json` (minisign-signed). Android shows an update
toast that downloads the new APK; or use
[Obtainium](https://github.com/ImranR98/Obtainium) with `https://github.com/felixgoff/RustRadar`.
iOS updates through TestFlight.

Repository secrets (only the publishing runs see them): `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (the updater key; the name is kept so installed copies keep
updating), `ANDROID_KEYSTORE` (base64 release keystore; keep the file, every update must use the
same key), `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, and `APPLE_DEVELOPMENT_TEAM`,
`APPLE_API_ISSUER`, `APPLE_API_KEY_ID`, `APPLE_API_KEY` (an App Store Connect API key). Without the
Android ones a release has no signed APK; without the Apple ones the IPA is unsigned (sideload
only). The app's identity (bundle id `com.felix.rustradar`, folders, update channel) must not
change: see `docs/port/07-backend-decoupling-and-packaging.md`.

## Being a polite client

Flightradar24's API is unofficial and rate-limited. RustRadar fetches only the slices covering the
visible area when zoomed in, refreshes the world every 15 s and a region every 8 s, and backs off
up to 6× when told to slow down.

## Licences and credits

The project's code is MIT. `assets/aircraft/` is derived from Flightradar24's GPL-2.0 aircraft
models and carries that licence. Fonts (Overpass, OFL), icons (Lucide, ISC), Streets GL textures
(MIT), Basis Universal (Apache-2.0), the data sources the app credits, and every Rust crate it
links are listed in `THIRD-PARTY-NOTICES.md`; regenerate the crate list with
`python3 scripts/rust-licenses.py`.

Asset pipelines (rerun only to change the assets): `node scripts/aircraft/build.mjs` (aircraft
meshes), `node scripts/trains/build.mjs` (train cars), `scripts/realistic-textures.py`,
`scripts/earth-texture.py`, `scripts/night-texture.py`, `scripts/build-rail-overview.py`.
