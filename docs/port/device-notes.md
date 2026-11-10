# Device notes: the phone shells (E19)

Status: the shells and the platform glue of E19 build, but **nothing has run on a phone yet**.
Nobody on the port has an Android phone or an iPhone. The checks below are for the owner (plan section 9,
question 2). Record what you see in the result columns: ☐ not run, ✓ as expected, ✗ wrong (with a note).

Plan references: section 5, risks K3 (surface lifecycle) and K4 (text input, safe areas, back); doc 07 §6,
§11.1, §13.4–13.5; doc 06 §2.4, §29.

## 1. What exists

| Piece | Where | What it does |
|---|---|---|
| Android project | `platform/android` | The Tauri Gradle project (`src-tauri/gen/android`) with Tauri taken out: no `buildSrc` rust plugin, no `tauri.*` files, no WebView or FileProvider. `MainActivity` extends `GameActivity` (`androidx.games:games-activity:4.4.0`, the version android-activity 0.6 glues). The manifest's `android.app.lib_name` is `rustradar_app`. Version from `-PversionName` / `-PversionCode` (default 1.0 / 1). The window starts in the map's ink colour. |
| Android Rust build | `platform/android/build-rust.sh [--release] [ABI…]` | cargo-ndk 4: `librustradar_app.so` into `app/src/main/jniLibs/<abi>/`, API level 24. GameActivity's C++ and the Basis transcoder link libc++ statically, so no `libc++_shared.so` is packaged. |
| iOS project | `platform/ios/project.yml` | xcodegen spec: one app target, bundle id `com.felix.rustradar`, iOS 15, `Info.plist` with `ITSAppUsesNonExemptEncryption = false` and a launch screen in the map's ink, the 1024 px icon (no alpha) from `src-tauri/icons/ios`. `Sources/main.m` calls `rustradar_ios_main`. A pre-build phase runs `build-rust.sh`, which builds `librustradar_app.a` with cargo for the platform being built. |
| Android glue | `crates/app/src/platform/android.rs` | The Tauri build's folders; the window insets; the text bridge between egui and GameTextInput; back and the keyboard's Done key; logcat logging. |
| iOS glue | `crates/app/src/platform/ios.rs` | The Tauri build's folders and the log. The safe area, the keyboard and typed text come from winit and egui-winit on iOS. |
| CI | `.github/workflows/phone-shells.yml` | Builds a debug APK (arm64-v8a) and an unsigned simulator app, as artifacts. It also builds the iPhone library, but nothing is signed or packaged for devices (that is P8 and P9). |

## 2. Before the checks: wiring the app loop

E19 does not own the app loop (`app.rs`, `lib.rs` and `platform/mod.rs` belong to E17). The glue is written and
tested, but **nothing calls it yet**. Until the six calls below are in, an APK or simulator app does the following:

- **Android:** it opens the globe without the backend, because it has no folders. Nothing is logged to logcat. There is no safe area and no typing. Back leaves the app at once, because `nativeBack` answers that the app does not take it.
- **iOS:** it opens the globe with the backend (`Dirs::desktop()` already resolves inside the container). There is no log.

The wiring (one item, a few lines):

```rust
// app.rs, run_with, right after `let wake = …`:
#[cfg(target_os = "android")]
platform::android::init(winit::platform::android::EventLoopExtAndroid::android_app(&event_loop), wake.clone());

// platform/mod.rs, dirs():
#[cfg(target_os = "android")]
return android::dirs();
#[cfg(target_os = "ios")]
return ios::dirs();
#[cfg(not(any(target_os = "android", target_os = "ios")))]
Dirs::desktop()

// lib.rs, first line of rustradar_ios_main:
platform::ios::init();

// app.rs, View::redraw, around the frame:
let mut input = self.egui_input.take_egui_input(&self.window);
#[cfg(target_os = "android")]
let back = platform::android::before_frame(&self.engine.egui, &mut input, self.window.scale_factor() as f32);
let (platform, next) = self.engine.frame(&self.gpu, &target, size, self.window.scale_factor(), input, local_ms);
#[cfg(target_os = "android")]
{
    platform::android::after_frame(&self.engine.egui, &platform);
    if back && !self.engine.state.apply(Action::CloseTop, local_ms) {
        platform::android::leave();
    }
}
self.egui_input.handle_platform_output(&self.window, platform);
```

Each single-line text field (U8 search, U9 filter text, token inputs, U17 keys dialog) also calls
`platform::android::text::report(ui.ctx(), id, &text)` every frame it is shown. That call is not behind a
`cfg`, because it only writes egui memory.

## 3. Getting a build

**From CI.** Open the run of "Phone shells" for the commit (Actions tab). It runs on pushes to `rust-port` and
`main` that touch `platform/**`, `crates/app/Cargo.toml`, `crates/app/src/lib.rs` or `crates/app/src/platform/**`,
or by hand with "Run workflow". It has two artifacts:

- `android-native-debug-apk`: `RustRadar_native_debug.apk`, arm64-v8a, a debug build signed with the CI runner's debug key.
  - Install it with `adb install -r RustRadar_native_debug.apk`.
  - It cannot install over a release-signed Tauri install, because the signatures differ. Uninstall that first, which deletes its `keys.json`. Export the keys from the Tauri app beforehand.
- `ios-native-simulator-app`: `RustRadar_native_simulator.app.zip`, for an Apple-silicon simulator. On a Mac:
  1. `unzip`;
  2. `xcrun simctl boot "iPhone 16"`;
  3. `xcrun simctl install booted RustRadar.app`;
  4. `xcrun simctl launch --console booted com.felix.rustradar`.

**On an iPhone.** Build from Xcode:

1. Run `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`.
2. Run `cd platform/ios && xcodegen generate` and open `RustRadar.xcodeproj`.
3. Set your team under Signing & Capabilities (a free personal team signs for 7 days).
4. Pick the phone and Run. The first build compiles the whole workspace for iOS, which takes several minutes.

**Android, locally.** You need cargo-ndk 4, NDK 27 (`ANDROID_NDK_HOME`), JDK 17 and the SDK:

```sh
rustup target add aarch64-linux-android
platform/android/build-rust.sh            # debug, arm64-v8a
cd platform/android && ./gradlew assembleDebug && adb install -r app/build/outputs/apk/debug/app-debug.apk
adb logcat -s rustradar                   # the app's log (after the wiring of section 2)
```

## 4. Checklist K3: surface lifecycle

Run this on one Android phone and one iPhone after the wiring of section 2. Leave `adb logcat -s rustradar` running on Android, or the Xcode console on iOS.

Expected throughout:
- no crash;
- no frame that stays black or stretched;
- the log has no "cannot remake the window surface" and no "surface texture validation error".

| # | Step | Expected | Android | iPhone |
|---|---|---|---|---|
| 1 | Cold start | The globe (view V1) within a few seconds. The screen is the map's ink before the first frame, not white. The log names the adapter and the backend (Vulkan or GL on Android, Metal on iOS), then "map MSAA x4". | ☐ | ☐ |
| 2 | Home, then back from recents, 20 times | The globe each time within a second, at the same view. Android drops and remakes the surface each time; iOS keeps its view. | ☐ | ☐ |
| 3 | Rotate portrait, landscape, portrait, 10 times | The globe fills the screen at once, with the view centre kept and nothing stretched. | ☐ | ☐ |
| 4 | Lock with the power button, then unlock, while open | As 2. | ☐ | ☐ |
| 5 | Notification shade or Control Centre over the app, then away | It keeps drawing, with no remade surface. | ☐ | ☐ |
| 6 | Split screen (Android), Slide Over or Stage Manager (iPad, if at hand); drag the divider | It draws at each new size. | ☐ | ☐ |
| 7 | Android: Developer options, then "Don't keep activities"; home, then return | `android_main` runs again for the new activity and the app starts again (no crash on the second `init`). | ☐ | – |
| 8 | Background, then kill (`adb shell am kill com.felix.rustradar`, or iOS's app switcher), then open | A clean start. | ☐ | ☐ |
| 9 | Five minutes in the background, then return | As 2. Note whether the feeds catch up: pausing them on suspend is not built yet (E17's note). | ☐ | ☐ |
| 10 | Android only: drag and rotate the globe continuously for a minute | Smooth. Note the frame rate if the dev readout exists. On a device without working Vulkan, note the model and the GPU (risk K1). | ☐ | – |

If remaking the surface is unreliable on some driver, the fallback of K3 applies: rebuild the whole `MapRenderer` on resume.

## 5. Checklist K4: text, safe area, back

The typing rows need a text field, which comes with stage 7 (U8 search, U17 keys dialog). Until then, unit tests in
`platform/android.rs` (`text::tests`) cover the bridge with a real egui `TextEdit`: Swedish letters, held
backspace, autocorrect, a moved cursor, an app-side clear, Enter and the Done key.

**Typing.** Android goes through GameTextInput and the bridge. iOS goes through winit's key and IME events. Try Gboard on Android, plus the Samsung keyboard if at hand.

| # | Step | Expected | Android | iPhone |
|---|---|---|---|---|
| 1 | Tap the search field | The keyboard shows; its key at the bottom right reads Done. | ☐ | ☐ |
| 2 | Type "Göteborg", then "Åre" (long-press for å and ö, and the Swedish layout) | Exactly that text, with the cursor after the last letter. | ☐ | ☐ |
| 3 | Type "teh " where autocorrect turns it into "the " | The field shows the corrected word, with the cursor after it, not before its last letters. | ☐ | ☐ |
| 4 | Tap a suggestion chip in the middle of a word | The word is replaced once, with no doubled letters. | ☐ | ☐ |
| 5 | Hold backspace over "Gothenburg Göteborg" | It deletes one character at a time, then words, and leaves nothing behind. | ☐ | ☐ |
| 6 | Move the cursor with Gboard's space-bar swipe, or the iOS space-bar drag; type in the middle | The field's cursor follows, and the text goes in there. | ☐ | ☐ |
| 7 | An emoji in the middle of the text, then typing after it | Cursor positions stay right. Java counts an emoji as two units; the bridge converts. | ☐ | ☐ |
| 8 | Paste through the keyboard's clipboard chip | The pasted text is in the field once. | ☐ | ☐ |
| 9 | Done | The search runs (or the field gives up focus) and the keyboard hides. | ☐ | ☐ |
| 10 | Hide the keyboard (back, or its own hide key), then tap the field again | It shows again. | ☐ | ☐ |
| 11 | Rotate with the keyboard open | The text and cursor are kept, and the keyboard is still usable. | ☐ | ☐ |
| 12 | Bluetooth or DeX hardware keyboard (optional) | One character per key, never two. | ☐ | ☐ |
| 13 | While typing, look at the field after each letter | Each letter shows at once. Android draws on the keyboard's own nudge (`nativeTextChanged`). | ☐ | ☐ |

**Safe area.** Until U19 pads by the safe area, read the values from the log. Android logs a line "window insets
Insets { left, top, right, bottom, keyboard }" in device pixels each time they change.

| # | Step | Expected | Android | iPhone |
|---|---|---|---|---|
| 14 | Portrait: compare the logged top and bottom with the status bar and the gesture bar or home indicator | Top = the status bar (plus the camera cutout), bottom = the navigation or gesture bar. | ☐ | ☐ |
| 15 | Landscape both ways | The cutout's side has the larger left or right inset. | ☐ | ☐ |
| 16 | Keyboard open, then closed | Android: `keyboard` is about the keyboard's height when open and 0 when closed. The bar insets do not change. | ☐ | – |
| 17 | After U19: the top bar, the sheet's bottom and the controls clear the notch, the cutout and the home indicator | doc 06 §2.4. | ☐ | ☐ |
| 18 | Status bar icons on the dark map, with the system in light theme, then dark | Readable. The app keeps the Tauri build's `enableEdgeToEdge()` default, so the icons follow the system theme; note it if they vanish. | ☐ | ☐ |

**Back.** Android only. iOS has no system back.

| # | Step | Expected | Android |
|---|---|---|---|
| 19 | Back gesture or key with nothing open | The app leaves, as a plain app does: to the background on Android 12 and later, closed before. | ☐ |
| 20 | Back with the keyboard open | The first back only hides the keyboard. | ☐ |
| 21 | After stage 7: back with a panel open, then again | The first closes the top panel or dialog; the next leaves. | ☐ |
| 22 | Android 14 and later: start the back gesture slowly | No back-to-home preview animation, because the app always takes back (see section 7). | ☐ |

## 6. Folders and keys (doc 07 §6 "verify")

The app looks in the same places the Tauri build did, so `keys.json` carries over on an in-place upgrade. The in-place
upgrade itself needs the release signature (P8 for Android, P11 for the rehearsal).

| # | Check | Expected | Result |
|---|---|---|---|
| 1 | Tauri build (a debug APK of it, so `run-as` works): enter keys, then `adb shell run-as com.felix.rustradar ls -la . cache` | `keys.json` right in `/data/user/0/com.felix.rustradar` (`Context.getDataDir()`), caches in `…/cache`. This is what tauri 2.12's `PathPlugin.kt` says. | ☐ |
| 2 | Native debug build, after keys are entered (stage 7): the same listing | The same files and places. | ☐ |
| 3 | iOS simulator: `xcrun simctl get_app_container booted com.felix.rustradar data` | `Library/Application Support/com.felix.rustradar/keys.json` and `Library/Caches/com.felix.rustradar/`. `HOME` is the container, as the Tauri build relied on. | ☐ |

## 7. Known gaps of the prototype

These are deliberate, or left for a later item.

- **Back is always taken, so Android 14+ shows no back-to-home preview.** The app decides only after the press whether back closes something (`Action::CloseTop`), then calls `MainActivity.leave()`. That method replays back without the app. The fix: once the state can say whether something is open, Rust switches the callback on and off ahead of time.
- **The safe area comes from the window insets, not `content_rect`.** GameActivity's content rectangle is its surface view's own rectangle in the window, so it never leaves an inset against the surface (edge to edge, enforced from target SDK 35, makes it the whole window). `MainActivity.onApplyWindowInsets` reports the system bars and the display cutout, and separately the keyboard's height.
- **Composing text is shown as typed text.** The underlined word an IME is composing has no underline.
- **The Done key is a soft Enter.** The keyboard is set to plain text with a Done key; each field decides what Enter does.
- **Exports are not available on phones**, so there is no Downloads folder (plan section 7).
- **The CI build is limited.**
  - The debug APK holds arm64-v8a only. P8 builds all four ABIs, signed with the release key.
  - The simulator app is for Apple silicon only.
  - Nothing is signed for an iPhone (P9).
- **No local Android build was made here.** No item has built this Gradle project outside CI.
  - This container has no Android SDK or NDK.
  - Installing the SDK means accepting Google's SDK licence, which is for the owner to do.
  - The Rust side was checked with `cargo check` and clippy for `aarch64-linux-android` and `aarch64-apple-ios`, through stand-in C compilers (E17's recipe). The CI run is the first real build.
- **The first CI run** (run 42865363833, commit `314b0d0`) built both artifacts:
  - **The debug APK: 155 MB.** It holds a debug `librustradar_app.so` of 149 MB: unoptimised code, line tables, and the 27 MB of embedded assets. A release library (optimised, LTO, stripped) is far smaller; risk K14 measures that in P8.
  - **The simulator app** (39 MB zipped), with the iPhone library alongside it. The app's `Info.plist` came out as specified: `com.felix.rustradar`, iOS 15, the ink launch screen, `ITSAppUsesNonExemptEncryption = false`.
- **Still to check on a downloaded APK:**
  - `unzip -l` shows only `lib/arm64-v8a/librustradar_app.so` (no `libc++_shared.so`).
  - For Play only, 16 KB pages: `zipalign -c -P 16 -v 4` passes and `llvm-readelf -l librustradar_app.so` shows LOAD alignment 0x4000.

## 8. Results

| Date | Device, OS | Build (commit) | Who | Notes |
|---|---|---|---|---|
| | | | | |
