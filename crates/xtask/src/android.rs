//! `cargo xtask android-release`: the native app's release APK (plan stage 11, row P8; doc 07 §8.1,
//! §9.1, §13.4, §13.6).
//!
//! ```text
//! cargo xtask android-release [--version X.Y.Z] [--abi ABI]... [--out DIR] [--any-key] [--dry-run]
//! ```
//!
//! 1. **Version.** The one the version files agree on (`cargo xtask version`; [`bump`]), which is the
//!    app crate's `CARGO_PKG_VERSION`, the version its update check compares with the releases'
//!    (`update::android::VERSION`). CI passes the release version as `--version`: it must equal the
//!    files', or the APK would offer its own release as an update on every launch (F5's trap), so run
//!    `cargo xtask bump <version>` first.
//! 2. **versionCode** = major·1 000 000 + minor·1000 + patch ([`version_code`]): the code the Tauri CLI
//!    (2.12.1) derived from `tauri.conf.json`'s version and wrote into `tauri.properties`, which
//!    `src-tauri/gen/android/app/build.gradle.kts` read. v0.3.16's released APK carries 3016 (read
//!    from its manifest for this item), and the plan's rule is the same formula, so both agree:
//!    for the same version the native APK has the code the Tauri APK had, and any later version a
//!    higher one, so Android installs it over the Tauri app (it refuses a lower code). The code is
//!    left for Gradle to derive from `-PversionName` (`platform/android/app/build.gradle.kts`, the same
//!    formula) and then checked here against [`version_code`], so the two cannot drift apart.
//! 3. **Signing**, with the secrets today's `android` job reads, by the same names
//!    ([`SECRETS`]: `ANDROID_KEYSTORE`, base64 of the keystore; `ANDROID_KEYSTORE_PASSWORD`, the store's
//!    and the key's password; `ANDROID_KEY_ALIAS`), materialised as that job does: the keystore
//!    decoded to `$RUNNER_TEMP/release.jks`, and `keystore.properties` (`storeFile`, `password`,
//!    `keyAlias`, [`keystore_properties`]) written into the Gradle project, which signs the release
//!    build type with it. Both files are removed again once Gradle is done. With none of the secrets
//!    (forks, pull requests) an existing `platform/android/keystore.properties` is used, else the APK
//!    is unsigned; with only some of them, a warning and an unsigned APK. A signed APK must be signed
//!    with today's key ([`RELEASE_CERT_SHA256`], the certificate of v0.3.16's APK): Android installs an
//!    update only over a copy signed with the same key, so another key is refused unless `--any-key`.
//! 4. **Native libraries**: `platform/android/build-rust.sh --release --locked <ABI>...` (cargo-ndk 4,
//!    API level 24) for the four ABIs today's release has ([`ABIS`], `RustPlugin.kt`'s
//!    `defaultAbiList`: one universal APK), after any `.so` left in `jniLibs` is removed.
//! 5. **Gradle**: `./gradlew assembleRelease --no-daemon -PversionName=<version>` in `platform/android`.
//! 6. **Checks**, with the SDK's build tools (`$ANDROID_HOME`, `$ANDROID_SDK_ROOT` or `local.properties`'
//!    `sdk.dir`; Gradle installs the build tools there): `aapt2 dump badging` must show
//!    `com.felix.rustradar`, the version, its code, minSdk 24, the INTERNET permission, the launcher
//!    activity `com.felix.rustradar.MainActivity` and exactly the ABIs built ([`check_badging`]); a
//!    signed APK must pass `apksigner verify` with today's certificate.
//! 7. **Name**: copied to `--out` (default `<target dir>/android`) as `RustRadar_{version}.apk`, today's
//!    asset name (the app's update check takes a release's first `.apk`), or, unsigned,
//!    `RustRadar_{version}_unsigned.apk` ([`apk_name`]): Android installs no unsigned APK, so a release
//!    must never carry one (today's workflow skips `*_debug.apk` for the same reason). Under GitHub
//!    Actions the path goes to `$GITHUB_OUTPUT` (`apk`, `name`, `signed`) and a line to the summary.
//!
//! `--abi` builds fewer ABIs (a local try; the checks then expect those). `--dry-run` prints the plan
//! and builds nothing. Needs, as the phone shells' job: JDK 17, the Android SDK, NDK 27
//! (`ANDROID_NDK_HOME`), cargo-ndk 4 and the Rust targets of the ABIs (`aarch64-linux-android`,
//! `armv7-linux-androideabi`, `i686-linux-android`, `x86_64-linux-android`).
//!
//! What else keeps the native APK installable over today's and the user's data in place (doc 07
//! §13.6, plan risk K10), checked against `src-tauri/gen/android` by the tests while it exists: the
//! applicationId and namespace, minSdk 24, targetSdk and compileSdk 37, the INTERNET permission only,
//! the launcher activity `.MainActivity` with today's launch mode and config changes (home-screen
//! shortcuts name it), the launcher icons byte for byte, the app's name. The app's files stay in
//! `/data/user/0/com.felix.rustradar` (`platform::android`, the Tauri build's folders).

use crate::bump;
use base64::Engine;
use regex::Regex;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The application id (and Gradle namespace) of today's APK.
pub const APPLICATION_ID: &str = "com.felix.rustradar";
/// The launcher activity, as the merged manifest names it.
pub const ACTIVITY: &str = "com.felix.rustradar.MainActivity";
/// minSdk (plan section 9, question 5).
pub const MIN_SDK: u32 = 24;
/// The ABIs of today's universal APK, in `RustPlugin.kt`'s order.
pub const ABIS: [&str; 4] = ["arm64-v8a", "armeabi-v7a", "x86", "x86_64"];
/// The permissions today's APK asks for (besides androidx's own `DYNAMIC_RECEIVER_NOT_EXPORTED`).
pub const PERMISSIONS: [&str; 1] = ["android.permission.INTERNET"];
/// The Gradle project, relative to the repository root.
pub const PROJECT: &str = "platform/android";
/// The signing secrets, by today's names (doc 07 §9.2): keystore (base64), password, alias.
pub const SECRETS: [&str; 3] = ["ANDROID_KEYSTORE", "ANDROID_KEYSTORE_PASSWORD", "ANDROID_KEY_ALIAS"];
/// SHA-256 of the certificate today's APKs are signed with (v0.3.16's, read from its APK Signature
/// Scheme v2 block): the key behind `ANDROID_KEYSTORE`, which must not change (doc 07 §13.6).
pub const RELEASE_CERT_SHA256: &str = "a77966fb7f61d9465bf7c7d63918b13bf270cfb7d2d973b4b4f18b68b23c63a4";
/// The library GameActivity loads (the manifest's `android.app.lib_name`): the app crate's `[lib]`.
pub const LIBRARY: &str = "librustradar_app.so";
/// The largest versionCode Android (and the Tauri CLI) accepts.
const MAX_VERSION_CODE: u64 = 2_100_000_000;

/// The versionCode of `version`: major·1 000 000 + minor·1000 + patch, the Tauri CLI's rule, between 1
/// and 2 100 000 000 as it requires. Only a plain `x.y.z` (the release tags' scheme) is taken.
pub fn version_code(version: &str) -> Result<u32, String> {
    let numbers: Vec<u64> = version
        .split('.')
        .map(|p| if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) { p.parse().ok() } else { None })
        .collect::<Option<_>>()
        .unwrap_or_default();
    let &[major, minor, patch] = numbers.as_slice() else {
        return Err(format!("the version {version} is not x.y.z"));
    };
    let code = major
        .checked_mul(1_000_000)
        .and_then(|c| c.checked_add(minor.checked_mul(1000)?))
        .and_then(|c| c.checked_add(patch))
        .filter(|c| (1..=MAX_VERSION_CODE).contains(c))
        .ok_or_else(|| format!("the version {version} gives no versionCode from 1 to {MAX_VERSION_CODE}"))?;
    Ok(code as u32)
}

/// The APK's file name: today's release asset name, or with `_unsigned` when there was no key.
pub fn apk_name(version: &str, signed: bool) -> String {
    if signed { format!("RustRadar_{version}.apk") } else { format!("RustRadar_{version}_unsigned.apk") }
}

/// `keystore.properties` as today's `android` job writes it (`storeFile`, `password`, `keyAlias`,
/// one per line; Gradle uses the password for the store and the key).
pub fn keystore_properties(store_file: &Path, password: &str, alias: &str) -> String {
    format!("storeFile={}\npassword={password}\nkeyAlias={alias}\n", store_file.display())
}

/// The keystore from `ANDROID_KEYSTORE`'s base64 (as `base64 --decode` takes it: line breaks and other
/// white space ignored; missing padding is taken too).
pub fn decode_keystore(text: &str) -> Result<Vec<u8>, String> {
    let compact: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    match engine.decode(compact) {
        Ok(bytes) if !bytes.is_empty() => Ok(bytes),
        _ => Err(format!("{} is not a base64 keystore", SECRETS[0])),
    }
}

/// How the release build is signed.
#[derive(Debug, PartialEq)]
pub enum Signing {
    /// with the three secrets, materialised for the build
    Secrets { keystore: Vec<u8>, password: String, alias: String },
    /// with a `keystore.properties` already in the Gradle project
    File,
    /// not at all, and why; `warning` when the secrets are only partly set (a mistake, not a fork)
    Unsigned { why: String, warning: bool },
}

impl Signing {
    pub fn signed(&self) -> bool {
        !matches!(self, Signing::Unsigned { .. })
    }
}

/// The signing that `env` (a variable's value, `None` or empty when unset) and an existing
/// `keystore.properties` give. Messages name the secrets, never their values.
pub fn signing(env: impl Fn(&str) -> Option<String>, properties_exist: bool) -> Result<Signing, String> {
    let values: Vec<Option<String>> = SECRETS.iter().map(|name| env(name).filter(|v| !v.is_empty())).collect();
    let missing: Vec<&str> = SECRETS.iter().zip(&values).filter(|(_, v)| v.is_none()).map(|(n, _)| *n).collect();
    match (&values[..], missing.len()) {
        ([Some(keystore), Some(password), Some(alias)], 0) => {
            if properties_exist {
                return Err(format!(
                    "{PROJECT}/keystore.properties exists and the signing secrets are set: remove the file or unset {}",
                    SECRETS.join(", ")
                ));
            }
            Ok(Signing::Secrets { keystore: decode_keystore(keystore)?, password: password.clone(), alias: alias.clone() })
        }
        (_, 3) if properties_exist => Ok(Signing::File),
        (_, 3) => Ok(Signing::Unsigned { why: format!("no signing secrets ({})", SECRETS.join(", ")), warning: false }),
        _ => Ok(Signing::Unsigned {
            why: format!("not all the signing secrets are set (missing: {})", missing.join(", ")),
            warning: true,
        }),
    }
}

/// What `aapt2 dump badging` says about an APK, the parts that are checked.
#[derive(Debug, Default, PartialEq)]
pub struct Badging {
    pub package: String,
    pub version_code: String,
    pub version_name: String,
    pub min_sdk: String,
    pub target_sdk: String,
    pub permissions: Vec<String>,
    pub launchable: String,
    pub native_code: Vec<String>,
}

/// The `key='value'` pairs of one badging line.
fn quoted(line: &str) -> Vec<(String, String)> {
    let pair = Regex::new(r"([A-Za-z][\w-]*)='([^']*)'").expect("a valid pattern");
    pair.captures_iter(line).map(|c| (c[1].to_owned(), c[2].to_owned())).collect()
}

/// The values of a line like `native-code: 'arm64-v8a' 'x86'`.
fn listed(rest: &str) -> Vec<String> {
    rest.split('\'').skip(1).step_by(2).map(str::to_owned).collect()
}

/// Reads `aapt2 dump badging` (or aapt's: `sdkVersion` there for `minSdkVersion`).
pub fn parse_badging(text: &str) -> Result<Badging, String> {
    let mut b = Badging::default();
    let mut has_package = false;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else { continue };
        let get = |name: &str| quoted(rest).into_iter().find(|(k, _)| k == name).map(|(_, v)| v).unwrap_or_default();
        let single = || listed(rest).into_iter().next().unwrap_or_default();
        match key {
            "package" => {
                has_package = true;
                b.package = get("name");
                b.version_code = get("versionCode");
                b.version_name = get("versionName");
            }
            "minSdkVersion" | "sdkVersion" => b.min_sdk = single(),
            "targetSdkVersion" => b.target_sdk = single(),
            "uses-permission" => b.permissions.push(get("name")),
            "launchable-activity" => b.launchable = get("name"),
            "native-code" => b.native_code = listed(rest),
            _ => {}
        }
    }
    if has_package { Ok(b) } else { Err("aapt2 printed no package line".to_owned()) }
}

/// Whether the APK's badging is what this release must be.
pub fn check_badging(b: &Badging, version: &str, code: u32, abis: &[&str]) -> Result<(), String> {
    let mut wrong = Vec::new();
    let mut expect = |what: &str, got: &str, want: &str| {
        if got != want {
            wrong.push(format!("{what} is '{got}', not '{want}'"));
        }
    };
    expect("the package", &b.package, APPLICATION_ID);
    expect("versionCode", &b.version_code, &code.to_string());
    expect("versionName", &b.version_name, version);
    expect("minSdkVersion", &b.min_sdk, &MIN_SDK.to_string());
    expect("the launcher activity", &b.launchable, ACTIVITY);
    let mut got: Vec<&str> = b.native_code.iter().map(String::as_str).collect();
    let mut want = abis.to_vec();
    got.sort_unstable();
    want.sort_unstable();
    if got != want {
        wrong.push(format!("native-code is {got:?}, not {want:?}"));
    }
    for permission in PERMISSIONS {
        if !b.permissions.iter().any(|p| p == permission) {
            wrong.push(format!("{permission} is not asked for"));
        }
    }
    if wrong.is_empty() { Ok(()) } else { Err(format!("the APK is not the release: {}", wrong.join("; "))) }
}

/// The signer's certificate digest from `apksigner verify --print-certs` (exactly one signer).
pub fn signer_digest(text: &str) -> Result<String, String> {
    let line = Regex::new(r"(?m)^Signer #\d+ certificate SHA-256 digest: ([0-9a-fA-F]+)\s*$").expect("a valid pattern");
    let digests: Vec<String> = line.captures_iter(text).map(|c| c[1].to_ascii_lowercase()).collect();
    match &digests[..] {
        [one] => Ok(one.clone()),
        [] => Err("apksigner printed no signer certificate".to_owned()),
        _ => Err(format!("the APK has {} signers, today's one", digests.len())),
    }
}

/// The Android SDK: `ANDROID_HOME`, `ANDROID_SDK_ROOT`, or `sdk.dir` in the project's `local.properties`.
fn sdk_root(project: &Path) -> Option<PathBuf> {
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir));
        }
    }
    let local = fs::read_to_string(project.join("local.properties")).ok()?;
    local.lines().find_map(|l| l.trim().strip_prefix("sdk.dir=")).map(PathBuf::from)
}

/// A build version's numbers, for picking the newest (`36.0.0` after `35.0.1`; previews last).
fn version_key(name: &str) -> Vec<u64> {
    name.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse().unwrap_or(0)).collect()
}

/// `tool` from the newest build tools that have it.
fn build_tool(sdk: &Path, tool: &str) -> Result<PathBuf, String> {
    let dir = sdk.join("build-tools");
    let mut versions: Vec<String> = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|v| dir.join(v).join(tool).is_file())
        .collect();
    versions.sort_by_key(|v| version_key(v));
    versions.last().map(|v| dir.join(v).join(tool)).ok_or_else(|| format!("no {tool} in {}", dir.display()))
}

/// The command line.
#[derive(Debug, Default, PartialEq)]
struct Args {
    version: Option<String>,
    abis: Vec<String>,
    out: Option<PathBuf>,
    any_key: bool,
    dry_run: bool,
}

pub const USAGE: &str = "usage: cargo xtask android-release [--version X.Y.Z] [--abi ABI]... [--out DIR] [--any-key] [--dry-run]";

fn args(list: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut it = list.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--version" => parsed.version = Some(value("--version")?),
            "--abi" => {
                let abi = value("--abi")?;
                if !ABIS.contains(&abi.as_str()) {
                    return Err(format!("unknown ABI {abi} (one of {})", ABIS.join(", ")));
                }
                if !parsed.abis.contains(&abi) {
                    parsed.abis.push(abi);
                }
            }
            "--out" => parsed.out = Some(PathBuf::from(value("--out")?)),
            "--any-key" => parsed.any_key = true,
            "--dry-run" => parsed.dry_run = true,
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(parsed)
}

/// The release version: the files', which `--version` must equal.
fn release_version(root: &Path, wanted: Option<&str>) -> Result<String, String> {
    let places = bump::places(root)?;
    let files = bump::agreed(&places)?.to_owned();
    match wanted {
        Some(wanted) if wanted != files => Err(format!(
            "the version files say {files}, the release is {wanted}: run `cargo xtask bump {wanted}` first \
             (the app compiles its version in and would offer its own release as an update)"
        )),
        _ => Ok(files),
    }
}

/// A warning, as a GitHub Actions annotation there.
fn warn(message: &str) {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        println!("::warning::{message}");
    } else {
        eprintln!("warning: {message}");
    }
}

/// Files written for one build (the keystore and `keystore.properties`), removed when dropped.
struct Materialised(Vec<PathBuf>);

impl Materialised {
    fn write(&mut self, path: PathBuf, contents: &[u8]) -> Result<(), String> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        // listed before it is written, so a half-written file goes too
        self.0.push(path.clone());
        let mut file = options.open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(contents).map_err(|e| format!("{}: {e}", path.display()))
    }
}

impl Drop for Materialised {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// Where the keystore goes: `$RUNNER_TEMP/release.jks` as today, else the system's temporary folder.
fn keystore_path() -> PathBuf {
    match std::env::var_os("RUNNER_TEMP").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("release.jks"),
        None => std::env::temp_dir().join(format!("rustradar-release-{}.jks", std::process::id())),
    }
}

fn run_command(mut command: Command, what: &str) -> Result<(), String> {
    let status = command.status().map_err(|e| format!("{what}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what} failed ({status})")) }
}

fn output(mut command: Command, what: &str) -> Result<String, String> {
    let out = command.output().map_err(|e| format!("{what}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{what} failed ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Appends `lines` to the file a GitHub Actions variable names, when it is set.
fn github_file(var: &str, lines: &str) -> Result<(), String> {
    let Some(path) = std::env::var_os(var).filter(|v| !v.is_empty()) else { return Ok(()) };
    let mut file = fs::OpenOptions::new().append(true).create(true).open(&path).map_err(|e| format!("{var}: {e}"))?;
    file.write_all(lines.as_bytes()).map_err(|e| format!("{var}: {e}"))
}

pub fn run(list: &[String]) -> Result<(), String> {
    let args = args(list)?;
    let root = bump::repo_root();
    let project = root.join(PROJECT);
    let version = release_version(&root, args.version.as_deref())?;
    let code = version_code(&version)?;
    let abis: Vec<&str> = if args.abis.is_empty() { ABIS.to_vec() } else { args.abis.iter().map(String::as_str).collect() };
    let signing = signing(|name| std::env::var(name).ok(), project.join("keystore.properties").is_file())?;
    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let out = args.out.clone().unwrap_or_else(|| target_dir.join("android"));
    let name = apk_name(&version, signing.signed());
    println!("Android release {version} (versionCode {code}), {}", abis.join(", "));
    match &signing {
        Signing::Secrets { .. } => println!("signing with the release keystore from {}", SECRETS.join(", ")),
        Signing::File => println!("signing with {PROJECT}/keystore.properties"),
        Signing::Unsigned { why, warning: true } => warn(&format!("{why}: the APK is unsigned")),
        Signing::Unsigned { why, warning: false } => println!("{why}: the APK is unsigned"),
    }
    println!("-> {}", out.join(&name).display());
    if args.dry_run {
        return Ok(());
    }

    let mut materialised = Materialised(Vec::new());
    if let Signing::Secrets { keystore, password, alias } = &signing {
        let jks = keystore_path();
        materialised.write(jks.clone(), keystore)?;
        materialised.write(project.join("keystore.properties"), keystore_properties(&jks, password, alias).as_bytes())?;
    }

    // a library of another build or ABI must not ride along
    let libs = project.join("app/src/main/jniLibs");
    if let Ok(dirs) = fs::read_dir(&libs) {
        for dir in dirs.flatten() {
            for file in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
                if file.path().extension().is_some_and(|e| e == "so") {
                    fs::remove_file(file.path()).map_err(|e| format!("{}: {e}", file.path().display()))?;
                }
            }
        }
    }
    let mut rust = Command::new("bash");
    rust.arg(project.join("build-rust.sh")).args(["--release", "--locked"]).args(&abis).current_dir(&root);
    run_command(rust, "build-rust.sh")?;
    for abi in &abis {
        let lib = libs.join(abi).join(LIBRARY);
        let size = fs::metadata(&lib).map_err(|e| format!("{}: {e}", lib.display()))?.len();
        println!("  {abi}/{LIBRARY}: {:.1} MB", size as f64 / 1e6);
    }

    let apks = project.join("app/build/outputs/apk/release");
    let (signed_apk, unsigned_apk) = (apks.join("app-release.apk"), apks.join("app-release-unsigned.apk"));
    for stale in [&signed_apk, &unsigned_apk] {
        if stale.exists() {
            fs::remove_file(stale).map_err(|e| format!("{}: {e}", stale.display()))?;
        }
    }
    let mut gradle = Command::new(project.join("gradlew"));
    gradle.args(["assembleRelease", "--no-daemon", &format!("-PversionName={version}")]).current_dir(&project);
    run_command(gradle, "gradlew assembleRelease")?;
    drop(materialised);

    let apk = if signing.signed() { signed_apk } else { unsigned_apk };
    if !apk.is_file() {
        return Err(format!("Gradle made no {}", apk.display()));
    }
    let sdk = sdk_root(&project).ok_or("no Android SDK: set ANDROID_HOME")?;
    let mut aapt2 = Command::new(build_tool(&sdk, "aapt2")?);
    aapt2.args(["dump", "badging"]).arg(&apk);
    let badging_text = output(aapt2, "aapt2 dump badging")?;
    for line in badging_text.lines() {
        let key = line.split(':').next().unwrap_or_default();
        if ["package", "minSdkVersion", "sdkVersion", "targetSdkVersion", "uses-permission", "launchable-activity", "native-code"]
            .contains(&key)
        {
            println!("  {line}");
        }
    }
    check_badging(&parse_badging(&badging_text)?, &version, code, &abis)?;
    if signing.signed() {
        let mut apksigner = Command::new(build_tool(&sdk, "apksigner")?);
        apksigner.args(["verify", "--print-certs"]).arg(&apk);
        let digest = signer_digest(&output(apksigner, "apksigner verify")?)?;
        if digest != RELEASE_CERT_SHA256 && !args.any_key {
            return Err(format!(
                "the APK is signed with the certificate {digest}, not today's {RELEASE_CERT_SHA256}: Android would not \
                 install it over the Tauri app (--any-key to allow another key)"
            ));
        }
        println!("  signed; certificate SHA-256 {digest}");
    }

    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let dest = out.join(&name);
    fs::copy(&apk, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mb = fs::metadata(&dest).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0);
    println!("{} ({mb:.1} MB)", dest.display());
    github_file("GITHUB_OUTPUT", &format!("apk={}\nname={name}\nsigned={}\n", dest.display(), signing.signed()))?;
    github_file(
        "GITHUB_STEP_SUMMARY",
        &format!("Android: `{name}`, versionCode {code}, {}, {mb:.1} MB\n", if signing.signed() { "signed" } else { "unsigned" }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn version_codes_are_the_tauri_clis() {
        // v0.3.16's released APK: versionCode 3016 (its manifest, read for P8)
        assert_eq!(version_code("0.3.16"), Ok(3016));
        assert_eq!(version_code("0.3.0"), Ok(3000));
        assert_eq!(version_code("1.2.3"), Ok(1_002_003));
        assert_eq!(version_code("0.0.1"), Ok(1));
        assert_eq!(version_code("2100.0.0"), Ok(2_100_000_000));
        // later versions, later codes: an update installs over the version before it
        assert!(version_code("0.3.17").unwrap() > version_code("0.3.16").unwrap());
        assert!(version_code("0.4.0").unwrap() > version_code("0.3.999").unwrap());
        assert!(version_code("1.0.0").unwrap() > version_code("0.999.999").unwrap());
    }

    #[test]
    fn versions_without_a_code_are_refused() {
        for bad in ["0.0.0", "2100.0.1", "99999999999999999999.0.0", "0.3", "0.3.0.1", "0.3.x", "0.3.0-rc.1", "v0.3.0", "", "0..1"] {
            assert!(version_code(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn apks_are_named_as_todays() {
        assert_eq!(apk_name("0.3.16", true), "RustRadar_0.3.16.apk");
        assert_eq!(apk_name("0.3.16", false), "RustRadar_0.3.16_unsigned.apk");
    }

    #[test]
    fn keystore_properties_are_todays_lines() {
        // rust.yml's android job: echo "storeFile=…"; echo "password=…"; echo "keyAlias=…"
        let text = keystore_properties(Path::new("/runner/temp/release.jks"), "pass word", "alias");
        assert_eq!(text, "storeFile=/runner/temp/release.jks\npassword=pass word\nkeyAlias=alias\n");
    }

    #[test]
    fn the_keystore_decodes_as_base64_decode_takes_it() {
        let bytes: Vec<u8> = (0..=255).collect();
        let plain = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(decode_keystore(&plain).unwrap(), bytes);
        // wrapped at 76 columns, as `base64` without -w0 writes it, with a trailing newline
        let wrapped: String = plain.as_bytes().chunks(76).map(|c| format!("{}\n", std::str::from_utf8(c).unwrap())).collect();
        assert_eq!(decode_keystore(&wrapped).unwrap(), bytes);
        assert_eq!(decode_keystore(plain.trim_end_matches('=')).unwrap(), bytes);
        assert!(decode_keystore("not base64!").is_err());
        assert!(decode_keystore("").is_err());
        // the message names the secret, not its value
        assert_eq!(decode_keystore("s3cr3t!").unwrap_err(), "ANDROID_KEYSTORE is not a base64 keystore");
    }

    fn env<'a>(set: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| set.iter().find(|(n, _)| *n == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn signing_takes_all_three_secrets_or_none() {
        let all = [("ANDROID_KEYSTORE", "AQID"), ("ANDROID_KEYSTORE_PASSWORD", "pw"), ("ANDROID_KEY_ALIAS", "key0")];
        assert_eq!(
            signing(env(&all), false).unwrap(),
            Signing::Secrets { keystore: vec![1, 2, 3], password: "pw".into(), alias: "key0".into() }
        );
        // a keystore.properties of someone's own is not overwritten
        let err = signing(env(&all), true).unwrap_err();
        assert!(err.contains("keystore.properties exists"), "{err}");
        assert!(!err.contains("pw") && !err.contains("key0"), "{err}");

        assert_eq!(signing(env(&[]), true).unwrap(), Signing::File);
        let none = signing(env(&[]), false).unwrap();
        assert!(matches!(&none, Signing::Unsigned { why, warning: false } if why.starts_with("no signing secrets")), "{none:?}");
        assert!(!none.signed());

        // some but not all (an empty secret counts as missing): unsigned, naming the missing ones
        let some = [("ANDROID_KEYSTORE", "AQID"), ("ANDROID_KEYSTORE_PASSWORD", ""), ("ANDROID_KEY_ALIAS", "key0")];
        let partial = signing(env(&some), false).unwrap();
        assert_eq!(
            partial,
            Signing::Unsigned { why: "not all the signing secrets are set (missing: ANDROID_KEYSTORE_PASSWORD)".into(), warning: true }
        );
        // even with a keystore.properties: half-set secrets are a mistake, not a choice of file
        assert!(!signing(env(&some), true).unwrap().signed());

        let bad = [("ANDROID_KEYSTORE", "%%%"), ("ANDROID_KEYSTORE_PASSWORD", "pw"), ("ANDROID_KEY_ALIAS", "key0")];
        assert!(signing(env(&bad), false).is_err());
    }

    #[test]
    fn materialised_files_are_private_and_removed() {
        let dir = std::env::temp_dir().join(format!("xtask-android-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (jks, props) = (dir.join("release.jks"), dir.join("keystore.properties"));
        {
            let mut m = Materialised(Vec::new());
            m.write(jks.clone(), &[1, 2, 3]).unwrap();
            m.write(props.clone(), b"storeFile=x\n").unwrap();
            assert_eq!(fs::read(&jks).unwrap(), [1, 2, 3]);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(fs::metadata(&props).unwrap().permissions().mode() & 0o777, 0o600);
            }
            // never over a file that is there
            assert!(m.write(props.clone(), b"again").is_err());
        }
        assert!(!jks.exists() && !props.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// `aapt2 dump badging` of a release build, in aapt2's format.
    const BADGING: &str = "package: name='com.felix.rustradar' versionCode='3016' versionName='0.3.16' platformBuildVersionName='17' platformBuildVersionCode='37' compileSdkVersion='37' compileSdkVersionCodename='17'
minSdkVersion:'24'
targetSdkVersion:'37'
uses-permission: name='android.permission.INTERNET'
uses-permission: name='com.felix.rustradar.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION'
application-label:'RustRadar'
application-icon-160:'res/mipmap-hdpi-v4/ic_launcher.png'
application: label='RustRadar' icon='res/mipmap-anydpi-v26/ic_launcher.xml'
launchable-activity: name='com.felix.rustradar.MainActivity'  label='RustRadar' icon=''
leanback-launchable-activity: name='com.felix.rustradar.MainActivity'  label='RustRadar' icon='' banner=''
uses-feature-not-required: name='android.software.leanback'
feature-group: label=''
  uses-feature: name='android.hardware.faketouch'
main
supports-screens: 'small' 'normal' 'large' 'xlarge'
supports-any-density: 'true'
densities: '160' '240' '320' '480' '640'
native-code: 'arm64-v8a' 'armeabi-v7a' 'x86' 'x86_64'
";

    #[test]
    fn badging_is_read() {
        let b = parse_badging(BADGING).unwrap();
        assert_eq!(b.package, "com.felix.rustradar");
        assert_eq!((b.version_code.as_str(), b.version_name.as_str()), ("3016", "0.3.16"));
        assert_eq!((b.min_sdk.as_str(), b.target_sdk.as_str()), ("24", "37"));
        assert_eq!(b.permissions, strings(&["android.permission.INTERNET", "com.felix.rustradar.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION"]));
        assert_eq!(b.launchable, ACTIVITY);
        assert_eq!(b.native_code, strings(&ABIS));
        // aapt (1) says sdkVersion
        assert_eq!(parse_badging(&BADGING.replace("minSdkVersion:", "sdkVersion:")).unwrap().min_sdk, "24");
        assert!(parse_badging("ERROR: dump failed").is_err());
    }

    #[test]
    fn badging_is_checked() {
        let b = parse_badging(BADGING).unwrap();
        assert_eq!(check_badging(&b, "0.3.16", 3016, &ABIS), Ok(()));
        // the order of the ABIs does not matter, their set does
        assert_eq!(check_badging(&b, "0.3.16", 3016, &["x86_64", "x86", "armeabi-v7a", "arm64-v8a"]), Ok(()));
        let err = check_badging(&b, "0.3.16", 3016, &["arm64-v8a"]).unwrap_err();
        assert!(err.contains("native-code"), "{err}");
        assert!(check_badging(&b, "0.3.17", 3017, &ABIS).unwrap_err().contains("versionCode is '3016', not '3017'"));
        for (from, to, what) in [
            ("name='com.felix.rustradar' ", "name='com.felix.rustradar.debug' ", "the package"),
            ("minSdkVersion:'24'", "minSdkVersion:'21'", "minSdkVersion"),
            ("launchable-activity: name='com.felix.rustradar.MainActivity'", "launchable-activity: name='a.B'", "launcher"),
            ("uses-permission: name='android.permission.INTERNET'\n", "", "INTERNET"),
        ] {
            let other = parse_badging(&BADGING.replace(from, to)).unwrap();
            let err = check_badging(&other, "0.3.16", 3016, &ABIS).unwrap_err();
            assert!(err.contains(what), "{what}: {err}");
        }
    }

    #[test]
    fn the_signers_digest_is_read() {
        let text = "Verifies
Verified using v1 scheme (JAR signing): false
Verified using v2 scheme (APK Signature Scheme v2): true
Verified using v3 scheme (APK Signature Scheme v3): false
Number of signers: 1
Signer #1 certificate DN: CN=Someone
Signer #1 certificate SHA-256 digest: A77966FB7F61D9465BF7C7D63918B13BF270CFB7D2D973B4B4F18B68B23C63A4
Signer #1 certificate SHA-1 digest: 0000000000000000000000000000000000000000
";
        assert_eq!(signer_digest(text).unwrap(), RELEASE_CERT_SHA256);
        assert!(signer_digest("Verifies\n").is_err());
        let two = format!("{text}Signer #2 certificate SHA-256 digest: {}\n", "ab".repeat(32));
        assert!(signer_digest(&two).is_err());
    }

    #[test]
    fn the_command_line_is_read() {
        let parsed = args(&strings(&["--version", "0.3.17", "--abi", "arm64-v8a", "--abi", "x86", "--abi", "x86", "--out", "dist", "--any-key", "--dry-run"])).unwrap();
        assert_eq!(
            parsed,
            Args { version: Some("0.3.17".into()), abis: strings(&["arm64-v8a", "x86"]), out: Some(PathBuf::from("dist")), any_key: true, dry_run: true }
        );
        assert_eq!(args(&[]).unwrap(), Args::default());
        assert!(args(&strings(&["--abi", "mips"])).is_err());
        assert!(args(&strings(&["--version"])).is_err());
        assert!(args(&strings(&["release"])).is_err());
    }

    #[test]
    fn the_release_version_is_the_files() {
        let root = bump::repo_root();
        let files = bump::current().unwrap();
        assert_eq!(release_version(&root, None).unwrap(), files);
        assert_eq!(release_version(&root, Some(&files)).unwrap(), files);
        let err = release_version(&root, Some("99.0.0")).unwrap_err();
        assert!(err.contains("cargo xtask bump 99.0.0"), "{err}");
        assert!(version_code(&files).is_ok());
    }

    #[test]
    fn build_tools_are_picked_newest_first() {
        let mut v = ["35.0.1", "9.0.0", "36.0.0", "36.0.0-rc1", "34.0.0"];
        v.sort_by_key(|n| version_key(n));
        assert_eq!(v.last(), Some(&"36.0.0-rc1"));
        assert!(version_key("36.0.0") > version_key("35.0.1"));
        assert!(version_key("10.0.0") > version_key("9.0.0"));
    }

    // The project against today's Tauri project (src-tauri/gen/android) while it exists: what must
    // not change for the native APK to install over today's and keep the user's data (doc 07 §13.6).

    fn read(path: &str) -> String {
        fs::read_to_string(bump::repo_root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    const TODAY: &str = "src-tauri/gen/android";

    fn today() -> Option<PathBuf> {
        let dir = bump::repo_root().join(TODAY);
        dir.is_dir().then_some(dir)
    }

    /// The value of `name = <value>` in a Gradle Kotlin file (quotes removed).
    fn gradle_value(text: &str, name: &str) -> String {
        let line = Regex::new(&format!(r#"(?m)^\s*{name}\s*=\s*"?([^"\s]+)"?\s*$"#)).unwrap();
        line.captures(text).map(|c| c[1].to_owned()).unwrap_or_else(|| panic!("no {name}"))
    }

    #[test]
    fn the_gradle_identity_is_todays() {
        let ours = read(&format!("{PROJECT}/app/build.gradle.kts"));
        assert_eq!(gradle_value(&ours, "applicationId"), APPLICATION_ID);
        assert_eq!(gradle_value(&ours, "namespace"), APPLICATION_ID);
        assert_eq!(gradle_value(&ours, "minSdk"), MIN_SDK.to_string());
        let Some(_) = today() else { return };
        let theirs = read(&format!("{TODAY}/app/build.gradle.kts"));
        for name in ["applicationId", "namespace", "minSdk", "targetSdk", "compileSdk"] {
            assert_eq!(gradle_value(&ours, name), gradle_value(&theirs, name), "{name}");
        }
        // signed the same way, from the same keystore.properties keys this writes
        for line in [
            r#"val keystoreFile = rootProject.file("keystore.properties")"#,
            r#"storeFile = file(keystore["storeFile"] as String)"#,
            r#"storePassword = keystore["password"] as String"#,
            r#"keyAlias = keystore["keyAlias"] as String"#,
            r#"keyPassword = keystore["password"] as String"#,
            r#"if (keystoreFile.exists()) signingConfig = signingConfigs.getByName("release")"#,
        ] {
            assert!(ours.contains(line) && theirs.contains(line), "{line}");
        }
        let keys: BTreeSet<String> =
            Regex::new(r#"keystore\["(\w+)"\]"#).unwrap().captures_iter(&ours).map(|c| c[1].to_owned()).collect();
        let written: BTreeSet<String> =
            keystore_properties(Path::new("x"), "p", "a").lines().map(|l| l.split('=').next().unwrap().to_owned()).collect();
        assert_eq!(keys, written);
    }

    #[test]
    fn gradle_reads_the_app_crates_version() {
        // build.gradle.kts defaults versionName to crates/app/Cargo.toml's [package] version
        let gradle = read(&format!("{PROJECT}/app/build.gradle.kts"));
        let path = Regex::new(r#"projectDirectory\.file\("([^"]+)"\)"#).unwrap().captures(&gradle).unwrap()[1].to_owned();
        let manifest = bump::repo_root().join(PROJECT).join(&path);
        let text = fs::read_to_string(&manifest).unwrap_or_else(|e| panic!("{}: {e}", manifest.display()));
        // the Kotlin pattern, unescaped, finds the version the files agree on
        let kotlin = Regex::new(r#"(?m)^version\s*=\s*"([^"]+)""#).unwrap();
        assert!(gradle.contains(r#"Regex("(?m)^version\\s*=\\s*\"([^\"]+)\"")"#));
        assert_eq!(kotlin.captures(&text).unwrap()[1], bump::current().unwrap());
        assert!(manifest.ends_with("crates/app/Cargo.toml"));
        // and the code from it with the same formula
        assert!(gradle.contains("major.toLong() * 1000000 + minor.toLong() * 1000 + patch.toLong()"));
        assert!(gradle.contains("it in 1L..2100000000L"));
    }

    /// The `android:name`s of the manifest's elements of one kind.
    fn names(manifest: &str, element: &str) -> BTreeSet<String> {
        let tag = Regex::new(&format!(r#"(?s)<{element}\b[^>]*?android:name="([^"]+)""#)).unwrap();
        tag.captures_iter(manifest).map(|c| c[1].to_owned()).collect()
    }

    fn attribute(manifest: &str, element: &str, name: &str) -> String {
        let tag = Regex::new(&format!(r#"(?s)<{element}\b[^>]*?android:{name}="([^"]+)""#)).unwrap();
        tag.captures(manifest).map(|c| c[1].to_owned()).unwrap_or_else(|| panic!("no {element} {name}"))
    }

    #[test]
    fn the_manifest_is_todays() {
        let ours = read(&format!("{PROJECT}/app/src/main/AndroidManifest.xml"));
        assert_eq!(names(&ours, "uses-permission"), PERMISSIONS.iter().map(|p| p.to_string()).collect());
        assert_eq!(attribute(&ours, "meta-data", "value"), LIBRARY.trim_start_matches("lib").trim_end_matches(".so"));
        let Some(_) = today() else { return };
        let theirs = read(&format!("{TODAY}/app/src/main/AndroidManifest.xml"));
        for element in ["uses-permission", "uses-feature", "activity", "action", "category"] {
            assert_eq!(names(&ours, element), names(&theirs, element), "{element}");
        }
        for name in ["launchMode", "configChanges", "exported", "label"] {
            assert_eq!(attribute(&ours, "activity", name), attribute(&theirs, "activity", name), "activity {name}");
        }
        for name in ["icon", "label", "theme", "usesCleartextTraffic"] {
            assert_eq!(attribute(&ours, "application", name), attribute(&theirs, "application", name), "application {name}");
        }
        assert_eq!(read(&format!("{PROJECT}/app/src/main/res/values/strings.xml")), read(&format!("{TODAY}/app/src/main/res/values/strings.xml")));
    }

    #[test]
    fn the_launcher_icons_are_todays() {
        let Some(today) = today() else { return };
        let res = today.join("app/src/main/res");
        let ours = bump::repo_root().join(PROJECT).join("app/src/main/res");
        let mut compared = 0;
        for dir in fs::read_dir(&res).unwrap().flatten() {
            for file in fs::read_dir(dir.path()).unwrap().flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                if !name.starts_with("ic_launcher") {
                    continue;
                }
                let mine = ours.join(dir.file_name()).join(&name);
                let same = fs::read(&mine).ok() == Some(fs::read(file.path()).unwrap());
                assert!(same, "{} differs from today's", mine.display());
                compared += 1;
            }
        }
        // five densities × three PNGs, the adaptive icon, its two drawables and its colour
        assert_eq!(compared, 19);
    }

    #[test]
    fn the_abis_are_todays() {
        let Some(today) = today() else { return };
        let plugin = fs::read_to_string(today.join("buildSrc/src/main/java/com/felix/rustradar/kotlin/RustPlugin.kt")).unwrap();
        let list = Regex::new(r#"defaultAbiList = listOf\(([^)]*)\)"#).unwrap().captures(&plugin).unwrap()[1].to_owned();
        let theirs: Vec<String> = Regex::new(r#""([^"]+)""#).unwrap().captures_iter(&list).map(|c| c[1].to_owned()).collect();
        assert_eq!(theirs, strings(&ABIS));
        // build-rust.sh takes each of them
        let script = read(&format!("{PROJECT}/build-rust.sh"));
        for abi in ABIS {
            assert!(script.contains(abi), "{abi}");
        }
        assert!(script.contains("-P 24"));
    }

    #[test]
    fn release_builds_keep_what_gameactivitys_c_finds_by_name() {
        let rules = read(&format!("{PROJECT}/app/proguard-rules.pro"));
        for rule in ["-keep class com.felix.rustradar.MainActivity { *; }", "-keep class com.google.androidgamesdk.** { *; }", "native <methods>;"] {
            assert!(rules.contains(rule), "{rule}");
        }
        // and the release build type reads every .pro file of the project
        assert!(read(&format!("{PROJECT}/app/build.gradle.kts")).contains(r#"include("**/*.pro")"#));
    }
}
