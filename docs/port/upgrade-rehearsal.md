# Upgrade rehearsal (plan stage 11, row P11)

Status: procedure written (item Pk11), **not yet run**. This is a person check: the owner runs it on real
machines and fills in the results table (section 9). Until every required row there passes, no native build may
reach `releases/latest` (section 10).

What it proves: copies of today's Tauri app, installed the ways users installed them, update themselves to the
native app through their own unchanged updater, keep their keys and sign-in, and end up as one clean native
install (risk K10 of the plan, doc 07 §12.1 and §17). The plan row names Windows, macOS, the AppImage and the
APK; this procedure also covers the MSI, the `.deb` (through `pkexec`), the `.rpm` and iOS through TestFlight.

Placeholders used below (replace them everywhere):

| | Meaning | Example |
|---|---|---|
| `O` | the newest real release, what installed copies run | `0.3.16` |
| `P` | the real release before it (for the control runs) | `0.3.15` |
| `V` | the native dry-run build's version: the next patch after `O`, as a push to `main` would publish | `0.3.17` |
| `REPO` | the test repository, `OWNER/NAME` | `felixgoff/RustRadar-rehearsal` |
| `RUN` | the id of the green "Build" run on `rust-port` whose packages are rehearsed | `43734850065` |

---

## 1. What is being rehearsed (read off today's code)

### 1.1 The updater every installed copy runs

Every release from v0.2.0 to v0.3.16 carries `tauri-plugin-updater` 2.13.2, `tauri` 2.12.1 and `reqwest` 0.13.5
(each tag's `Cargo.lock`), so one description covers all installed desktop copies:

- **Launch check** (`src-tauri/src/updater.rs`): in release builds, unless started with `--updated`, GET the
  endpoint compiled in from `src-tauri/tauri.conf.json`,
  `https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json`, with an 8 s timeout. Any
  failure (no answer, 404, a manifest it cannot read, no entry for this install) opens the installed version.
- **The request**: a fresh reqwest client (`ClientBuilder::new()`): system proxy settings, **the environment's
  `HTTPS_PROXY` first** (hyper-util's `Matcher::from_system`), the platform's certificate store
  (rustls-platform-verifier), up to 10 redirects; user agent `tauri-plugin-updater/2.13.2`; `Accept:
  application/json` for the manifest, `Accept: application/octet-stream` for the package.
- **The entry it takes**: `<os>-<arch>-<installer>`, else `<os>-<arch>`. The installer is the kind patched into
  the old binary when it was packed (`__TAURI_BUNDLE_TYPE_VAR_NSS`, `_MSI`, `_DEB`, `_RPM`, `_APP` for the
  AppImage); on macOS it is always `app`.
- **The checks**: the version must be greater (semver); the minisign signature must verify with the public key
  compiled in (key id `28F06F796981B11D`, the same value as `crates/app/src/update/mod.rs`'s `PUBKEY`); and when
  the signature's trusted comment names a version (`version:x.y.z`, which `cargo xtask sign` writes), it must be
  the announced one.
- **The install, per kind**, then the relaunch:

| Installed as | Package taken | Install (plugin 2.13.2) | Relaunch |
|---|---|---|---|
| NSIS, per user, `%LOCALAPPDATA%\RustRadar` | `RustRadar_V_x64-setup.exe` | written to `%TEMP%`, run with `/S /UPDATE /R /ARGS` (installMode `quiet`); the old app exits at once | by the installer (`RunAsUser`), with the old process's arguments: **no `--updated`** |
| MSI, per machine, `C:\Program Files\RustRadar` | `RustRadar_V_x64_en-US.msi` | `msiexec /i <msi> /quiet /promptrestart AUTOLAUNCHAPP=True LAUNCHAPPARGS=""`; the old app exits at once. The plugin's own docs: quiet mode "requires admin privileges if the installer does", and this MSI is per machine | by the MSI's `LaunchApplication` action, no `--updated` |
| `.app` (macOS, both architectures) | `RustRadar_V_<arch>.app.tar.gz` | unpacked into a temp folder on the same volume, each path's first part dropped, swapped with the installed bundle; if the swap is refused for permissions, an administrator prompt (`osascript … with administrator privileges`) | the old executable path, `RustRadar.app/Contents/MacOS/rustradar`, with `--updated` |
| AppImage | `RustRadar_V_amd64.AppImage` | written over `$APPIMAGE` (same file name, permissions kept) | `$APPIMAGE` with `--updated` |
| `.deb` (`rust-radar`) | `RustRadar_V_amd64.deb` | `pkexec dpkg -i <file>`, else a zenity/kdialog password and `sudo -S`, else terminal `sudo` | `/usr/bin/rustradar --updated` |
| `.rpm` (`rust-radar`) | `RustRadar-V-1.x86_64.rpm` | `pkexec rpm -U <file>`, same fallbacks | `/usr/bin/rustradar --updated` |

Phones: Android checks `https://api.github.com/repos/felixgoff/RustRadar/releases/latest` hourly and opens the
release's `.apk` in the browser (`src-tauri/src/mobile_update.rs`); Android installs it over the old one only if
it is signed with the same key and its `versionCode` is not lower (`V` 0.3.17 is 3017, `O` 0.3.16 is 3016).
iOS updates through TestFlight only.

### 1.2 What the native release gives it

The native packages (Pk5–Pk7, built by `cargo xtask package` in `rust.yml`'s `package` job) keep today's names,
identity and install locations (doc 07 §13.6); `cargo xtask sign` signs them as the Tauri CLI did, and
`cargo xtask latest-json` writes `latest.json` with today's eleven keys. One deliberate difference (Pk1, gap 7 of
the stages 8-10 status): the URLs are the release's download URLs,
`https://github.com/<repo>/releases/download/vV/<name>`, where tauri-action wrote GitHub's API asset URLs
(`https://api.github.com/repos/felixgoff/RustRadar/releases/assets/<id>`). Checked while writing this
procedure, with the updater's headers: both forms of v0.3.16's `setup.exe` answer one `302` to the same
`release-assets.githubusercontent.com` object, and the API form answers JSON metadata, not the file, without
`Accept: application/octet-stream` (the plugin always sends it). The download URLs are also not GitHub API calls,
so the API's rate limit for anonymous clients does not apply to them. That installed copies really follow them
is what rows G0–G2 and every desktop row confirm.

### 1.3 What carries over

| | Tauri app | Native app | Expected after the upgrade |
|---|---|---|---|
| Keys and FR24 sign-in | `keys.json` in the config folder (`%APPDATA%\com.felix.rustradar`, `~/Library/Application Support/com.felix.rustradar`, `~/.config/com.felix.rustradar`, Android's app data folder) | the same file, same folder | kept, byte for byte |
| Settings, filters, mode, basemap | the web view's `localStorage` (`rustradar.v1`) | `settings.json` beside `keys.json` | **reset once** to the defaults (aircraft mode, dark basemap, the initial globe view, no filters), then kept: the plan's question 1 default |
| Caches | `com.felix.rustradar` cache folder | the same folder | reused |
| The web view's own data | e.g. `%LOCALAPPDATA%\com.felix.rustradar\EBWebView` | not used | left behind, unused (not a failure) |

---

## 2. The approach, and why

**Chosen: the released apps themselves, unchanged, with their one request for `latest.json` redirected by a
local HTTPS proxy that only the app's own process uses.** On each desktop the owner installs the real release `O`
exactly as users did, then starts it from a terminal with `HTTPS_PROXY` pointing at
[mitmproxy](https://mitmproxy.org) on `127.0.0.1:8080`. mitmproxy rewrites that single URL to the same path in
the test repository and passes everything else through: the test repository's own redirects, the download URLs of
section 1.2 and GitHub's asset servers are real. The native packages are signed with the real updater key, so
the old app checks them with the key it really holds.

Why not the alternatives:

- **A hosts-file override with a local server** would point `github.com` at the machine for every program on it,
  needs a local server presenting a `github.com` certificate from a locally trusted CA anyway, and would serve the
  files itself, so GitHub's redirects (gap 7) would go untested. The per-process proxy needs the same trusted CA
  but changes nothing for any other program and keeps every real redirect.
- **A one-off Tauri build pointing at the test repository** needs no proxy or CA, but it is a rebuilt app, not
  the binary users run, and with a rehearsal key in place of the real one it cannot show that the real key's
  signatures, made by `cargo xtask sign`, verify in installed copies. It is the fallback when the updater key file
  is not at hand (section 8).

The updater reads the environment's `HTTPS_PROXY` before the system's settings, and trusts the platform's
certificate store (section 1.1), so the proxy needs no change to the app. Checked while writing this: mitmproxy
12.3.1 with the flags of section 6 (plus this container's own upstream CA) rewrote the endpoint for a reqwest 0.13.5 client with the same TLS and
proxy code (`cargo xtask rehearsal-check`, which uses that client), trusting the proxy's CA through
`SSL_CERT_FILE`; a host outside `--allow-hosts` was passed through untouched.

**Deviation from the item text: the test repository is public, and holds no code.** Installed copies download
anonymously (as they do from felixgoff/RustRadar, a public repository); GitHub answers `404` to anonymous requests
for a private repository's release files. The repository holds only the rehearsal's release (a README is enough),
so it has no workflows and no secrets that could run or leak. The one part that needs a private copy of this
repository with workflows and secrets is the iOS row (7.10).

---

## 3. Safety rules (read before starting)

1. **Nothing is published to felixgoff/RustRadar.** Every `gh` command below names `--repo REPO` explicitly.
   `cargo xtask latest-json --rehearsal` refuses felixgoff/RustRadar in any spelling, and prints the `gh` command
   with the test repository written out.
2. **The rehearsal release starts as a pre-release** (`--prerelease`): `releases/latest` does not see it (row G1).
   It is marked latest **in the test repository only**, for the platform rows (row G2).
3. **No secret is written to a file in a repository, pasted into a terminal's history, the results table or an
   issue.** The updater key is read from its key file by path; passwords are typed at a prompt (`read -rs`). The
   Android keystore stays where it is. Record hashes of `keys.json`, never its contents.
4. **The proxy's CA is trusted only for the rehearsal** and removed afterwards (section 11). On Linux it is not
   installed at all (`SSL_CERT_FILE` for the one process).
5. **Never push a `v*` tag to felixgoff/RustRadar**, and never merge stage 12's D2 into `main`, until section 10
   allows it. The iOS row pushes a tag only to its private copy, from a clone that has no remote for
   felixgoff/RustRadar.

---

## 4. One-time setup

### 4.1 The test repository

```sh
gh repo create REPO --public --add-readme --description "RustRadar upgrade rehearsal: test releases only"
```

No workflows, no secrets, no collaborators needed.

### 4.2 Tools

- On the machine that publishes (any OS with bash): `gh` (logged in, with read access to felixgoff/RustRadar's
  Actions artifacts and write access to `REPO`), Rust 1.97.0 and a clone of this repository.
- On each desktop under test: mitmproxy (Windows: the installer from mitmproxy.org; macOS: `brew install
  mitmproxy`; Linux: `pipx install mitmproxy`). Run `mitmdump` once and stop it with Ctrl-C: it writes its CA to
  `~/.mitmproxy/` (`mitmproxy-ca-cert.pem`, `.cer`, `.p12`).
- For the Android row: Android SDK build-tools (`zipalign`, `apksigner`, `aapt2`), `adb`.

### 4.3 The keys

- The updater key: the key file behind `TAURI_SIGNING_PRIVATE_KEY` (the one-line file the Tauri CLI wrote, e.g.
  `~/.tauri/rustradar.key`) and its password. GitHub never shows a secret's value again; if the file is gone, use
  section 8.
- The Android release keystore behind `ANDROID_KEYSTORE`, its password and alias, for row A1 only.

---

## 5. Publish the native dry-run build in the test repository

Every push to `rust-port` runs the release pipeline as a dry run (Pk10): every package at the version a push to
`main` would publish, kept as workflow artifacts and signed with a throwaway key. The rehearsal takes those
packages, signs them with the real key and publishes them in `REPO`.

### 5.1 Pick the build

```sh
gh run list --repo felixgoff/RustRadar --workflow Build --branch rust-port --status success --limit 5
```

Take the newest green run (`RUN`) whose commit is the one to rehearse. Its "Release version" job's summary gives
`V` (the artifact names carry it too). Write down the commit; the rehearsal holds for it (section 10).

### 5.2 Download its packages

In an empty folder:

```sh
mkdir rehearsal && cd rehearsal
for a in desktop-windows-x86_64 desktop-macos-aarch64 desktop-macos-x86_64 desktop-linux-x86_64; do
  gh run download RUN --repo felixgoff/RustRadar -n "$a" -D dist
done
gh run download RUN --repo felixgoff/RustRadar -n native-android-apk -D apk
ls dist   # exactly the nine packages: setup.exe, msi, two .app.tar.gz, two dmg, AppImage, deb, rpm
```

Do not download the `updater` artifact: its signatures are the throwaway key's.

### 5.3 Sign them with the real key

From a worktree of the rehearsed commit (the version change below stays local and is never committed):

```sh
git -C ~/RustRadar worktree add ~/rr-rehearsal <commit>
cd ~/rr-rehearsal
cargo build -p xtask --locked
target/debug/xtask bump V                      # the signatures' trusted comment names the files' version
export TAURI_SIGNING_PRIVATE_KEY="$HOME/.tauri/rustradar.key"     # the key file's path
read -rs TAURI_SIGNING_PRIVATE_KEY_PASSWORD && export TAURI_SIGNING_PRIVATE_KEY_PASSWORD
XTASK=target/debug/xtask bash scripts/ci/sign-updater-artifacts.sh ~/rehearsal/dist V
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

The script refuses a folder that is not exactly the nine packages, and writes seven `.sig` files (not the dmgs).

### 5.4 Write `latest.json` for the test repository

```sh
target/debug/xtask latest-json ~/rehearsal/dist --version V --rehearsal REPO --out ~/rehearsal/dist/latest.json
```

It checks every signature against the key installed copies hold and that each was signed for `V`, writes the
manifest with `https://github.com/REPO/releases/download/vV/<name>` URLs, and prints the next commands with
`REPO` filled in.

### 5.5 Create the pre-release

Run the `gh release create` command `latest-json` printed. Its shape:

```sh
gh release create vV --repo REPO --prerelease --title "RustRadar vV" \
  --notes "Installed copies update to this version automatically the next time they open." \
  <the seven packages> <their seven .sig files> ~/rehearsal/dist/latest.json
```

### 5.6 Row G0: read it as installed copies will

```sh
target/debug/xtask rehearsal-check https://github.com/REPO/releases/download/vV/latest.json --current O
```

For each kind of install it prints the key the old updater takes, every redirect, the size, the signature check
with the installed key, the signed version, and whether the bytes are what that kind installs (an NSIS installer
for NSIS copies, an MSI for MSI copies, a `.app.tar.gz` with one `.app` folder holding
`Contents/MacOS/rustradar` and today's bundle id and version, a type 2 AppImage, a deb, an rpm). It ends with
"every kind of installed copy would install this release", or fails naming each problem. The same command against
today's real release (`https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json --current P`)
passes; run it once as a control.

### 5.7 Rows G1 and G2: the pre-release is not seen, then the test repository's latest is

G1: `target/debug/xtask rehearsal-check https://github.com/REPO/releases/latest/download/latest.json` must fail
with "the manifest answers 404: installed copies find no update there" (`REPO` has no full release), and on one
desktop (any row's machine, after section 6) the old app started through the proxy must open as version `O`,
with no "Updating RustRadar…" window. This is what a native pre-release in felixgoff/RustRadar would do to every
installed copy: nothing.

G2: mark it latest **in the test repository**, then check the whole chain installed copies will follow:

```sh
gh release edit vV --repo REPO --prerelease=false --latest
target/debug/xtask rehearsal-check https://github.com/REPO/releases/latest/download/latest.json --current O
```

### 5.8 The APK for row A1

The dry run's APK is unsigned (`apk/RustRadar_V_unsigned.apk`); sign it with the release keystore:

```sh
zipalign -c -P 16 -v 4 apk/RustRadar_V_unsigned.apk | tail -1          # "Verification successful"
apksigner sign --ks /path/to/release.jks --ks-key-alias <alias> --out apk/RustRadar_V.apk apk/RustRadar_V_unsigned.apk
apksigner verify --print-certs apk/RustRadar_V.apk | grep SHA-256
# a77966fb7f61d9465bf7c7d63918b13bf270cfb7d2d973b4b4f18b68b23c63a4 (v0.3.16's certificate, Pk8)
aapt2 dump badging apk/RustRadar_V.apk | head -1                     # com.felix.rustradar, versionCode V's code, versionName V
gh release upload vV apk/RustRadar_V.apk --repo REPO
```

(`apksigner` asks for the password; if `zipalign -c` fails, run `zipalign -P 16 -f 4` into a new file and sign
that.)

---

## 6. On each desktop: the proxy

Start it in its own terminal and leave it running (single quotes as written; in `cmd.exe` use double quotes):

```sh
mitmdump --listen-host 127.0.0.1 --listen-port 8080 --allow-hosts '^github\.com(:443)?$' \
  --map-remote '|^https://github\.com/felixgoff/RustRadar/releases/latest/download/latest\.json$|https://github.com/REPO/releases/latest/download/latest.json'
```

Only `github.com` is intercepted; only the endpoint is rewritten (to the same path in `REPO`); everything else,
GitHub's asset servers included, passes through. Its log shows each request, so you can see the old app ask.

Trust its CA for the rehearsal:

| OS | Trust | Start the old app through it |
|---|---|---|
| Windows (PowerShell) | `certutil -user -addstore Root "$env:USERPROFILE\.mitmproxy\mitmproxy-ca-cert.cer"` (confirm the dialog) | `$env:HTTPS_PROXY = 'http://127.0.0.1:8080'` then `& "$env:LOCALAPPDATA\RustRadar\rustradar.exe"` (NSIS) or `& "C:\Program Files\RustRadar\rustradar.exe"` (MSI) |
| macOS | `sudo security add-trusted-cert -d -p ssl -p basic -k /Library/Keychains/System.keychain ~/.mitmproxy/mitmproxy-ca-cert.pem` | `HTTPS_PROXY=http://127.0.0.1:8080 /Applications/RustRadar.app/Contents/MacOS/rustradar` |
| Linux | nothing installed: `cat /etc/ssl/certs/ca-certificates.crt ~/.mitmproxy/mitmproxy-ca-cert.pem > /tmp/rehearsal-ca.pem` (Fedora: `/etc/pki/tls/certs/ca-bundle.crt`) | `HTTPS_PROXY=http://127.0.0.1:8080 SSL_CERT_FILE=/tmp/rehearsal-ca.pem <the AppImage, or rustradar>` |

Quit any running RustRadar first. Start the app only this way for the upgrade itself; every other start (the
second launch below) is the normal one, without the proxy.

---

## 7. Per platform

### 7.0 Every row, the same way

**Control first (C), through the real endpoint, no proxy:** install `P` from felixgoff/RustRadar's release as a
user would, open it once, and let it update itself to `O`. This records today's behaviour on that machine (what
it shows, which prompts, how it relaunches) for comparison, and leaves the machine on `O`, the version users have.
If the control does not update, record that and rehearse from `P`.

**A machine used for a second row** (W2 after W1, M2 after M1, L2 after L1): put the old version back first, and
delete the native app's `settings.json` beside `keys.json`, so that the upgrade is a first one again.

**Prepare** in the old app (`O`): open the keys dialog and enter the FR24 sign-in and the Trafiklab and
Trafikverket keys (the "Signed in" badge shows); choose settings that differ from the defaults (e.g. the satellite
basemap, trains mode, a filter); quit. Record the hash of `keys.json` (Windows `Get-FileHash
"$env:APPDATA\com.felix.rustradar\keys.json"`, macOS `shasum -a 256 ~/Library/Application\
Support/com.felix.rustradar/keys.json`, Linux `sha256sum ~/.config/com.felix.rustradar/keys.json`).

**Rehearse (R),** once G2 has passed: start the old app through the proxy (section 6). Expected: the small window "Updating
RustRadar…", then "Downloading update…" with a filling bar, "Installing…", then the native app's window.

**Check, and write into the table:**

1. *Updated*: what showed, any prompt, how long. A failure anywhere must end with the old version opening.
2. *Relaunch*: the native app opened by itself. Its command line (macOS/Linux `ps -o args= -p "$(pgrep -n
   rustradar)"`, Windows `Get-CimInstance Win32_Process -Filter "Name='rustradar.exe'" | Select-Object
   CommandLine`) ends in `--updated` on macOS and Linux; on Windows the installer starts it without (as today).
3. *Keys and sign-in*: the `keys.json` hash is unchanged; the keys dialog shows the keys and "Signed in"; trains
   mode shows trains.
4. *Settings reset once*: the native app opened with the defaults; change the basemap, quit, start it normally:
   the change is kept.
5. *Install entry*: one install, at `V` (per row below).
6. *Version shown*: where the OS shows it (per row below). The app shows its version only on the map error
   card (today's shows none).
7. *Second launch, no proxy*: start it the normal way (Start menu, Finder, launcher): it opens at once, with no
   "Updating RustRadar…" window (felixgoff/RustRadar's latest is `O`, older than `V`; a native build that does not
   carry its release version would update itself back to `O`: gap 6).

### 7.1 W1: Windows, NSIS installer

Install `RustRadar_P_x64-setup.exe` (SmartScreen: More info, Run anyway), control, prepare, rehearse.

- Expected: no administrator prompt (per user); the app closes, the installer runs silently, the native app opens.
- Install entry: Settings → Apps lists one RustRadar, version `V`, publisher felix:
  `Get-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*,
  HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\* -ErrorAction SilentlyContinue | Where-Object
  DisplayName -like 'RustRadar*' | Format-Table DisplayName, DisplayVersion, Publisher, InstallLocation` prints
  one row. The Start menu and desktop shortcuts start the native app.
- Version shown: that entry; `rustradar.exe` → Properties → Details → Product version `V`.
- Last: uninstall from Settings → Apps, leaving "Delete the application data" unticked: the folder and shortcuts
  go, `keys.json` stays.

### 7.2 W2: Windows, MSI

On a machine (or after uninstalling W1) with no RustRadar: install `RustRadar_P_x64_en-US.msi`, control,
prepare, rehearse.

- Expected, from the plugin's code (1.1): `msiexec /quiet` on a per-machine package. It may need the app to run
  as administrator to succeed; the control shows what today's MSI updates do. If after the old app closes nothing
  opens within a minute, look in Event Viewer → Windows Logs → Application, source MsiInstaller, and record the
  message. If the control fails the same way, write "fails as today" and the owner decides (K10's fallback, or
  accept it; plan question 4).
- Install entry: one RustRadar at `V` (the MSI's major upgrade removes `O`'s entry; the UpgradeCode is
  `{C73F46FF-44DC-5F3E-AB93-388B09B92DEC}`), under `C:\Program Files\RustRadar`.

### 7.3 M1: macOS, `.app`, writable (Apple silicon)

Install from `RustRadar_P_aarch64.dmg` into /Applications as an administrator user, open it once with right-click
→ Open (unsigned), control, prepare, rehearse.

- Expected: no prompt; the bundle is swapped in place and the native app opens.
- Install entry and version: `defaults read /Applications/RustRadar.app/Contents/Info CFBundleShortVersionString`
  is `V`, `codesign --verify --deep --strict /Applications/RustRadar.app` succeeds (ad hoc), no
  `com.apple.quarantine` on it (`xattr -l`); one RustRadar.app.
- Second launch from Finder or Launchpad: opens with no "damaged" or "unidentified developer" dialog.

### 7.4 M2: macOS, `.app` that needs the administrator prompt

As M1 (on a machine that ran M1: replace /Applications/RustRadar.app with `O`'s from `RustRadar_O_aarch64.dmg`,
open it once, and delete `settings.json`), then make the installed bundle unwritable for the user, as for a copy
an administrator installed for a standard user: `sudo chown -R root:wheel /Applications/RustRadar.app`. Rehearse
twice:

- First press **Cancel** at "RustRadar wants to make changes": the old version (`O`) must open, unchanged.
- Then again, and enter an administrator's password: the native app opens. Checks as M1.

### 7.5 M3 (optional): macOS Intel

As M1 on an Intel Mac (`RustRadar_P_x64.dmg`): exercises `darwin-x86_64-app`.

### 7.6 L1: Linux, AppImage

On Ubuntu 22.04 or newer (`sudo apt install libfuse2` if AppImages do not start): download
`RustRadar_P_amd64.AppImage`, `chmod +x`, control, prepare, rehearse.

- Expected: no prompt; the file is overwritten in place and keeps its name (still `…_P_…` or `…_O_…`: the
  updater does not rename it) and its executable bit; the native app opens with `--updated`.
- Install entry: none (a file); `ls -l` shows the new size.

### 7.7 L2: Linux, `.deb` through `pkexec`

On a desktop install set up as a user's would be (e.g. fresh Ubuntu 22.04 desktop), install only the old package:
`sudo apt install ./RustRadar_P_amd64.deb`, control, prepare, rehearse (from a terminal in the desktop session, so
that polkit's agent can ask).

- Expected: polkit's password dialog (`pkexec dpkg -i`), then the native app with `--updated`.
- Install entry: `dpkg -s rust-radar` shows `Status: install ok installed` and `Version: V`; `dpkg --audit`
  prints nothing. `dpkg -i` does not fetch new dependencies: the native package depends on libraries the Tauri one
  did not name (xkbcommon-x11, xcb-xkb, Vulkan and EGL loaders, D-Bus, root certificates; `Packager.toml`
  `[deb]`). If dpkg reports unmet dependencies, the row fails: record them.
- The menu entry (RustRadar.desktop) starts the native app.

### 7.8 L3 (optional): Linux, `.rpm`

On Fedora: `sudo dnf install ./RustRadar-P-1.x86_64.rpm`, control, prepare, rehearse. Expected `pkexec rpm -U`;
`rpm -q rust-radar` is `rust-radar-V-1.x86_64`, `rpm -V rust-radar` prints nothing.

### 7.9 A1: Android, the native APK over the Tauri APK

- Control: install `RustRadar_P.apk` from felixgoff/RustRadar's release, prepare keys and settings; within a
  minute its "update ready" toast offers `O`: tap it, the browser downloads `RustRadar_O.apk`, install it. Keys
  kept: this is today's path.
- Rehearse the same path with the native APK: on the phone's browser open
  `https://github.com/REPO/releases/download/vV/RustRadar_V.apk` (from 5.8), open the download, Update.
  (`adb install -r apk/RustRadar_V.apk` installs the same way; the browser is what the notice does.)
- Expected: "App installed". "App not installed as package conflicts with an existing package" means another
  signing key; a refusal to downgrade means a lower `versionCode`: both fail the row.
- Checks: keys and "Signed in" in the keys dialog (no file hash on a phone), settings reset once, Settings → Apps
  → RustRadar shows version `V`, one RustRadar on the home screen; reopen: no update toast.

### 7.10 I1: iOS, TestFlight on a tag

The native App Store build and its TestFlight upload run only on a `v*` tag of a tree without
`src-tauri/tauri.conf.json` (`rust.yml`, `tree.publish`): this row needs stage 12's D2 on a branch (not merged).

1. Make a **private** copy for it, e.g. `OWNER/RustRadar-ios-rehearsal`, and in its Settings → Actions disable the
   "Refresh TestFlight" and "Bump version" workflows before adding any secret. Add only the four App Store Connect
   secrets by today's names (`APPLE_API_ISSUER`, `APPLE_API_KEY_ID`, `APPLE_API_KEY`, `APPLE_DEVELOPMENT_TEAM`):
   without `TAURI_SIGNING_PRIVATE_KEY` the copy's `updater` job stops at "Check signing key" and it publishes no
   desktop release; without the `ANDROID_*` secrets its APK is unsigned.
2. In a clone of the D2 branch whose only remote is the copy (`git remote set-url origin
   https://github.com/OWNER/RustRadar-ios-rehearsal`; no remote for felixgoff/RustRadar): `cargo xtask bump V`,
   commit, `git tag vV`, push the branch and the tag to the copy.
3. The copy's run builds the App Store IPA (`ios-release`) and uploads it (`testflight`) once the native tests
   pass. After App Store Connect has processed it, on an iPhone that has the Tauri build from TestFlight (keys and
   sign-in set): TestFlight → RustRadar → install `V`.
4. Checks: the app replaces the Tauri build (one icon), keys and "Signed in" kept, settings reset once, TestFlight
   and Settings → General → iPhone Storage show `V`.

Mind: the build goes to the app's TestFlight testers like any upload; and App Store Connect refuses a second
upload of version `V` with the same or a lower build number, so after this row felixgoff/RustRadar's next release
must carry a version above `V` (put `cargo xtask bump <V + 1 patch>` in the next change that reaches `main`).
Remove the four secrets from the copy afterwards.

---

## 8. Fallback when the updater key file is not at hand: a one-off Tauri build

Use this only if section 4.3's key file is gone. It changes the old app instead of its network:

1. Make a rehearsal key: `bash scripts/ci/throwaway-updater-key.sh ~/rehearsal-key` (needs `minisign`; keep the
   folder until the rehearsal ends, then delete it).
2. In a private copy of this repository with no other secrets, on a branch from tag `vO`, change two values in
   `src-tauri/tauri.conf.json`: `plugins.updater.endpoints` to
   `["https://github.com/REPO/releases/latest/download/latest.json"]` and `plugins.updater.pubkey` to the content
   of `~/rehearsal-key/updater.key.pub`. Set the copy's `TAURI_SIGNING_PRIVATE_KEY` (the content of
   `updater.key`) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, push a `v*` tag of that commit to the copy: the tag's
   own workflow (today's, at `vO`) builds and releases today's app with these two values, at the version its files
   hold (0.3.0 at v0.3.16's tag: older than `V`, which is all the rehearsal needs). Install those packages on the
   desktops instead of `O` (download them while logged in to GitHub).
3. Sign the native packages with the rehearsal key in 5.3 (`TAURI_SIGNING_PRIVATE_KEY=~/rehearsal-key/updater.key`,
   its password from `password`), give 5.4 and 5.6 `--pubkey "$(cat ~/rehearsal-key/updater.key.pub)"`, and skip
   section 6: the one-off app asks `REPO` itself.
4. Record in the table that route B was used: it does not show that the real key's signatures verify in installed
   copies, and the old app is a rebuild. Rows A1 and I1 are the same in both routes.

---

## 9. Results

One line per row; "Control" is the `P` → `O` run of 7.0. Keys: `hash =` when the `keys.json` hash is unchanged.
Required for the gate: G0, G1, G2, W1, W2, M1, M2, L1, L2, A1, I1. Route: A (section 2) unless noted.

Rehearsed commit: `________`  Build run: `RUN = ________`  `V = ______`  `O = ______`  `P = ______`  Route: `__`

| Row | Install | Machine (OS, arch) | Control P→O | Updated (what showed, prompts) | Relaunch | Keys, sign-in | Settings reset once | Install entry | Version shown | 2nd launch: no update | Result | Date, who, notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| G0 | `rehearsal-check` of the pre-release | – | – | | – | – | – | – | – | – | | |
| G1 | pre-release not seen | | – | | – | – | – | – | – | – | | |
| G2 | `rehearsal-check` of REPO's latest | – | – | | – | – | – | – | – | – | | |
| W1 | Windows, NSIS | | | | | | | | | | | |
| W2 | Windows, MSI | | | | | | | | | | | |
| M1 | macOS, .app, writable | | | | | | | | | | | |
| M2 | macOS, .app, admin prompt (Cancel, then OK) | | | | | | | | | | | |
| M3 | macOS Intel (optional) | | | | | | | | | | | |
| L1 | Linux, AppImage | | | | | | | | | | | |
| L2 | Linux, .deb (pkexec) | | | | | | | | | | | |
| L3 | Linux, .rpm (optional) | | | | | | | | | | | |
| A1 | Android, APK over APK | | | | | | | | | | | |
| I1 | iOS, TestFlight on a tag (needs D2's branch) | | | | | | | | | | | |

Result is **pass**, **fail** (with what failed), or for W2 only **as control** (fails exactly as today's MSI
update does, with the owner's decision in the notes).

---

## 10. The gate

Only when every required row of section 9 is **pass** (W2: pass, or as control with a written decision) may a
native build reach `releases/latest`. Until then:

- stage 12's D2 (the tree without `src-tauri/tauri.conf.json`, which makes a push to `main` publish the native
  release) is not merged into `main`;
- no `v*` tag is pushed to felixgoff/RustRadar for a native tree;
- no native release in felixgoff/RustRadar is created, or edited into a full or latest release, by hand.

A rehearsal holds for the commit it rehearsed. Rerun the rows a change touches if, after that commit, anything
changes in: `Packager.toml`, `platform/windows/**`, `platform/macos/**`, `platform/linux/**`,
`platform/android/**`, `platform/ios/**`, `crates/app/build.rs`, `crates/xtask/src/{package,sign,manifest,
android,ios}.rs`, `scripts/ci/sign-updater-artifacts.sh`, `.github/workflows/rust.yml`'s `package`, `updater` and
`release` jobs, the app's identity (doc 07 §13.6), or where it keeps `keys.json`. If a row fails: fix the native
package (or, as K10 says, ship one more Tauri release whose only change is an updater that can install it), make a
new dry run, and rehearse the failed rows again, with any row that shares the changed package.

The plan's stage 12 entry condition ("P11 has passed") refers to this section.

---

## 11. Afterwards

- Remove the proxy's CA: Windows `certutil -user -delstore Root mitmproxy`; macOS `sudo security
  delete-certificate -c mitmproxy /Library/Keychains/System.keychain`; Linux `rm /tmp/rehearsal-ca.pem`.
  Optionally delete `~/.mitmproxy`.
- Delete the worktree (`git -C ~/RustRadar worktree remove ~/rr-rehearsal`) and `~/rehearsal`, and, for route B,
  `~/rehearsal-key` and the copy's secrets.
- The rehearsal machines may stay on the native `V`: they update from felixgoff/RustRadar once its latest release
  is newer than `V`. To return one to the Tauri app now: Windows, uninstall and install `O`; macOS, replace the
  bundle with `O`'s; Linux `sudo apt install --allow-downgrades ./RustRadar_O_amd64.deb` or the `O` AppImage;
  Android only by uninstalling (keys are lost, re-enter them); iOS, an older build in TestFlight.
- Delete the test repository's release, or the repository, when the stage 12 switch is done.

---

## 12. Checked while writing this (Pk11), and what is left open

- `cargo xtask rehearsal-check https://github.com/felixgoff/RustRadar/releases/latest/download/latest.json
  --current 0.3.15`, run against today's real release v0.3.16 (Tauri, its API asset URLs): the manifest after two
  redirects (`releases/latest/download` → `releases/download/v0.3.16` → `release-assets.githubusercontent.com`),
  all seven kinds of install found their key (`windows-x86_64-nsis`, `-msi`, `darwin-aarch64-app`,
  `darwin-x86_64-app`, `linux-x86_64-appimage`, `-deb`, `-rpm`), every package downloaded after one redirect with
  its `Content-Length`, every signature verified with the key installed copies hold, signed for 0.3.16, and every
  package is of its kind (the macOS archives with `RustRadar.app/Contents/MacOS/rustradar` and today's
  `Info.plist`). So the check finds nothing wrong with a release installed copies do update from.
- The proxy rule of section 6 with mitmproxy 12.3.1, for a reqwest 0.13.5 client with rustls-platform-verifier
  0.7.1 (the plugin's versions) and `SSL_CERT_FILE` on Linux: the endpoint was rewritten (that client got
  v0.3.15's manifest when it asked for the endpoint), the other host passed through.
- Not done here (no machine, no key): any install, any OS but Linux, anything signed with the real key.
- Open points the rows will settle: whether MSI copies can update at all in quiet mode (W2's control); whether
  the native `.deb`'s new dependencies are present on users' machines (L2); the Windows relaunch carries no
  `--updated` (today's behaviour, so the native app checks once more at its first start: harmless while `V` is the
  newest release); the iOS row needs D2's branch and moves felixgoff/RustRadar's next version above `V`.

Tools: `crates/xtask/src/rehearsal.rs` (module docs; tests against a local server cover every check above) and
`latest-json --rehearsal` (`crates/xtask/src/manifest.rs`).
