//! `cargo xtask ios-release`: the native app's iOS release (plan stage 11, row P9; doc 07 §8.2, §9.1,
//! §9.4, §13.5, §13.6), built as today's `ios` job (`rust.yml`) and `testflight-refresh.yml` build it
//! through the Tauri CLI 2.12.1 (`tauri ios build`, which drives cargo-mobile2 0.22.5's `xcodebuild`
//! calls), on a Mac with Xcode and xcodegen.
//!
//! ```text
//! cargo xtask ios-release [--unsigned | --app-store] [--upload] [--version X.Y.Z] [--build-number N]
//!                         [--out DIR] [--dry-run]
//! ```
//!
//! 1. **Which build** ([`choose`]). Without `--unsigned` or `--app-store` it is today's `ios` job's
//!    choice: on a release (a push to `main` or of a `v*` tag, today's `RELEASE`) with the four App Store
//!    Connect secrets ([`SECRETS`], by today's names) the App Store build, with only some of them a
//!    warning naming the missing ones and an unsigned IPA (today's words), otherwise (pull requests,
//!    `rust-port`, manual runs) the unsigned IPA. `--app-store` (testflight-refresh) insists on the App
//!    Store build and `--upload` uploads it to TestFlight; both are refused unless `GITHUB_REF` is
//!    `refs/heads/main` or a `refs/tags/v*` tag ([`may_publish`]), so nothing reaches App Store Connect
//!    from `rust-port`, a pull request or a run by hand.
//! 2. **Version** ([`bundle_versions`]): the one the version files agree on (`cargo xtask version`, the
//!    app crate's `CARGO_PKG_VERSION`), which `--version` must equal (run `cargo xtask bump <version>`
//!    first, as for Android). CFBundleShortVersionString is its `x.y.z` and CFBundleVersion, the build
//!    number, is the version too unless `--build-number` gives one (the refresh's `YYYYMMDDHH`), as the
//!    Tauri CLI sets them from `tauri.conf.json` and `bundle.iOS.bundleVersion`.
//! 3. **Project**: `xcodegen generate` in `platform/ios` (the checked-in `project.yml`: today's bundle
//!    id, product name, iOS 15.0, icons, launch screen, `Info.plist`; see the tests).
//! 4. **Archive** ([`archive`]): `xcodebuild archive` of the Release configuration for `iphoneos`
//!    without code signing (cargo-mobile2's `skip_codesign` flags: the Tauri CLI archives unsigned for
//!    both of today's builds), the version on the command line. Its first build phase runs
//!    `platform/ios/build-rust.sh`, which builds `librustradar_app.a` for `aarch64-apple-ios` with
//!    `cargo build --release --locked`. The archived app is then checked ([`check_app`]): bundle id,
//!    versions, iOS 15.0, executable, arm64 only, the compiled icons and launch screen.
//! 5. **Unsigned IPA** ([`write_ipa`]): `Payload/RustRadar.app` zipped as the Tauri CLI's `create_ipa`
//!    zips it (deflated, every entry 0755), named `RustRadar_{version}_unsigned.ipa`, today's release
//!    asset for AltStore and Sideloadly, which sign it with the user's own Apple ID.
//! 6. **App Store build**: the archived app signed ad hoc with today's empty entitlements (the Tauri CLI
//!    signs it with a throwaway self-signed certificate "just to preserve the entitlements"; the export
//!    replaces either signature), then `xcodebuild -exportArchive` with `-allowProvisioningUpdates` and
//!    the App Store Connect API key ([`export`]) and the export options the Tauri CLI writes
//!    ([`export_options`]: `app-store-connect`, or `app-store` before Xcode 15.4; automatic signing;
//!    the team): Xcode's cloud signing makes the distribution signature and profile. The IPA is
//!    `RustRadar_{version}.ipa`, today's name for the signed one.
//! 7. **Upload** (`--upload`): `xcrun altool --upload-app --type ios --file <ipa> --apiKey <key id>
//!    --apiIssuer <issuer>` ([`altool`]), today's command.
//!
//! The secrets: `APPLE_API_ISSUER`, `APPLE_API_KEY_ID`, `APPLE_API_KEY` (the `.p8` file's contents) and
//! `APPLE_DEVELOPMENT_TEAM`, read from the environment; a key already written to a file may be named
//! by `APPLE_API_KEY_PATH` instead of `APPLE_API_KEY` (as today's workflow steps leave it). The key file
//! is written, if it has to be, to `~/.appstoreconnect/private_keys/AuthKey_<key id>.p8` (where today's
//! step writes it and altool looks) only after the archive, so no build script of the Rust build runs
//! while it is there, and removed at the end. No child process gets the secrets in its environment
//! ([`without_secrets`]); the export and altool get the key id and issuer as arguments, as today, and
//! they are shown as `***` in what this prints. Nothing here prints a secret's value.
//!
//! Output: `--out` (default `<target dir>/ios`); under GitHub Actions `$GITHUB_OUTPUT` gets `ipa`,
//! `name`, `signed` and `uploaded`, the step summary a line. `--dry-run` prints the choice, the version
//! and the commands, and runs nothing (it works on any OS). Needs on the Mac: Xcode (15.4 or later for
//! today's export method name), xcodegen (`brew install xcodegen`), the Rust target `aarch64-apple-ios`.
//!
//! Deviations from today's job, by design: the archive gets no App Store Connect key (it is not signed,
//! so it had no use for it; the key is written after it); the ad hoc signature in place of the Tauri
//! CLI's self-signed one; the App Store build and the upload refuse any ref but `main` and `v*` tags
//! (today a manual run of testflight-refresh on another branch would upload); the Xcode target is
//! `RustRadar`, where the Tauri CLI's was `rustradar_iOS` (the app, its bundle id and its executable
//! are the same).

use crate::bump;
use flate2::Compression;
use flate2::write::DeflateEncoder;
use regex::Regex;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The bundle id of today's iOS app (doc 07 §13.6).
pub const BUNDLE_ID: &str = "com.felix.rustradar";
/// The product: `RustRadar.app`, its executable, the IPA's name.
pub const PRODUCT: &str = "RustRadar";
/// The deployment target: the Tauri CLI's default `minimumSystemVersion`, today's (plan section 9, question 5).
pub const MINIMUM_IOS: &str = "15.0";
/// The xcodegen project, relative to the repository root.
pub const PROJECT: &str = "platform/ios";
/// The Rust target of iPhones and iPads.
pub const RUST_TARGET: &str = "aarch64-apple-ios";
/// The App Store Connect secrets, by today's names, in the order today's warning lists them.
pub const SECRETS: [&str; 4] = ["APPLE_API_ISSUER", "APPLE_API_KEY_ID", "APPLE_API_KEY", "APPLE_DEVELOPMENT_TEAM"];
/// A key already written to a file (today's workflow steps put its path here).
pub const KEY_PATH_VAR: &str = "APPLE_API_KEY_PATH";
/// Taken out of every child's environment: the iOS secrets and the other release secrets a job may have.
const SCRUBBED: [&str; 10] = [
    "APPLE_API_ISSUER",
    "APPLE_API_KEY_ID",
    "APPLE_API_KEY",
    "APPLE_DEVELOPMENT_TEAM",
    KEY_PATH_VAR,
    "TAURI_SIGNING_PRIVATE_KEY",
    "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
    "ANDROID_KEYSTORE",
    "ANDROID_KEYSTORE_PASSWORD",
    "ANDROID_KEY_ALIAS",
];

/// Today's app icons: the Tauri CLI 2.12.1 template's `AppIcon.appiconset/Contents.json`, in its order
/// (size in points, idiom, file, scale), filled with `src-tauri/icons/ios`.
#[cfg(test)]
pub const ICONS: [(&str, &str, &str, &str); 18] = [
    ("20x20", "iphone", "AppIcon-20x20@2x.png", "2x"),
    ("20x20", "iphone", "AppIcon-20x20@3x.png", "3x"),
    ("29x29", "iphone", "AppIcon-29x29@2x-1.png", "2x"),
    ("29x29", "iphone", "AppIcon-29x29@3x.png", "3x"),
    ("40x40", "iphone", "AppIcon-40x40@2x.png", "2x"),
    ("40x40", "iphone", "AppIcon-40x40@3x.png", "3x"),
    ("60x60", "iphone", "AppIcon-60x60@2x.png", "2x"),
    ("60x60", "iphone", "AppIcon-60x60@3x.png", "3x"),
    ("20x20", "ipad", "AppIcon-20x20@1x.png", "1x"),
    ("20x20", "ipad", "AppIcon-20x20@2x-1.png", "2x"),
    ("29x29", "ipad", "AppIcon-29x29@1x.png", "1x"),
    ("29x29", "ipad", "AppIcon-29x29@2x.png", "2x"),
    ("40x40", "ipad", "AppIcon-40x40@1x.png", "1x"),
    ("40x40", "ipad", "AppIcon-40x40@2x-1.png", "2x"),
    ("76x76", "ipad", "AppIcon-76x76@1x.png", "1x"),
    ("76x76", "ipad", "AppIcon-76x76@2x.png", "2x"),
    ("83.5x83.5", "ipad", "AppIcon-83.5x83.5@2x.png", "2x"),
    ("1024x1024", "ios-marketing", "AppIcon-512@2x.png", "1x"),
];

/// CFBundleShortVersionString and CFBundleVersion for `version`, as the Tauri CLI derives them: the
/// short version is `x.y.z` (a pre-release tag dropped), the bundle version the same with any build
/// metadata as a fourth part, unless `build_number` (`bundle.iOS.bundleVersion` today, the refresh's
/// `YYYYMMDDHH`) replaces it. App Store Connect takes one to three dot-separated integers.
pub fn bundle_versions(version: &str, build_number: Option<&str>) -> Result<(String, String), String> {
    let semver = Regex::new(r"^([0-9]+)\.([0-9]+)\.([0-9]+)(?:-[0-9A-Za-z.-]+)?(?:\+([0-9A-Za-z.-]+))?$").expect("pattern");
    let caps = semver.captures(version).ok_or_else(|| format!("{version} is not an x.y.z version"))?;
    let short = format!("{}.{}.{}", &caps[1], &caps[2], &caps[3]);
    let bundle = match (build_number, caps.get(4)) {
        (Some(number), _) => number.to_owned(),
        (None, Some(build)) => format!("{short}.{}", build.as_str()),
        (None, None) => short.clone(),
    };
    let integers = Regex::new(r"^[0-9]+(\.[0-9]+){0,2}$").expect("pattern");
    if !integers.is_match(&bundle) {
        return Err(format!("the build number {bundle} is not one to three dot-separated integers (CFBundleVersion)"));
    }
    Ok((short, bundle))
}

/// The release asset's name: `RustRadar_{version}.ipa` signed, `RustRadar_{version}_unsigned.ipa` not.
pub fn ipa_name(version: &str, signed: bool) -> String {
    format!("{PRODUCT}_{version}{}.ipa", if signed { "" } else { "_unsigned" })
}

/// Whether `git_ref` may reach App Store Connect: `main` and `v*` tags, as today's releases.
pub fn may_publish(git_ref: Option<&str>) -> bool {
    matches!(git_ref, Some(r) if r == "refs/heads/main" || r.starts_with("refs/tags/v"))
}

/// What the release reads from its environment: a variable's value, `None` when unset or empty (the
/// process's own in [`run`], a made-up one in the tests).
type Env = dyn Fn(&str) -> Option<String>;

/// Today's `RELEASE`: a push to `main` or of a `v*` tag.
pub fn is_release(env: &Env) -> bool {
    env("GITHUB_EVENT_NAME").as_deref() == Some("push") && may_publish(env("GITHUB_REF").as_deref())
}

/// The App Store Connect API key.
#[derive(Debug, PartialEq)]
pub enum Key {
    /// a `.p8` file already written (`APPLE_API_KEY_PATH`)
    File(PathBuf),
    /// the `.p8` file's contents (`APPLE_API_KEY`)
    Contents(String),
}

/// The App Store Connect secrets.
#[derive(Debug, PartialEq)]
pub struct Credentials {
    pub issuer: String,
    pub key_id: String,
    pub key: Key,
    pub team: String,
}

impl Credentials {
    /// The values that are shown as `***`.
    fn hidden(&self) -> Vec<&str> {
        vec![&self.issuer, &self.key_id, &self.team]
    }
}

/// The secrets from `env`, or the names of the missing ones in [`SECRETS`]' order.
pub fn credentials(env: &Env) -> Result<Credentials, Vec<&'static str>> {
    let key = match (env(KEY_PATH_VAR), env("APPLE_API_KEY")) {
        (Some(path), _) => Some(Key::File(PathBuf::from(path))),
        (None, Some(contents)) => Some(Key::Contents(contents)),
        (None, None) => None,
    };
    let (issuer, key_id, team) = (env(SECRETS[0]), env(SECRETS[1]), env(SECRETS[3]));
    let mut missing = Vec::new();
    for (name, present) in SECRETS.iter().zip([issuer.is_some(), key_id.is_some(), key.is_some(), team.is_some()]) {
        if !present {
            missing.push(*name);
        }
    }
    match (issuer, key_id, key, team) {
        (Some(issuer), Some(key_id), Some(key), Some(team)) => Ok(Credentials { issuer, key_id, key, team }),
        _ => Err(missing),
    }
}

/// Checks the secrets' shapes (they go into a file name, a plist and command lines), naming the
/// variable, never the value.
fn check_shapes(c: &Credentials) -> Result<(), String> {
    let id = Regex::new(r"^[A-Za-z0-9]+$").expect("pattern");
    let issuer = Regex::new(r"^[A-Za-z0-9-]+$").expect("pattern");
    for (name, value, pattern) in
        [("APPLE_API_ISSUER", &c.issuer, &issuer), ("APPLE_API_KEY_ID", &c.key_id, &id), ("APPLE_DEVELOPMENT_TEAM", &c.team, &id)]
    {
        if !pattern.is_match(value) {
            return Err(format!("{name} is not the shape App Store Connect gives it"));
        }
    }
    Ok(())
}

/// The build chosen.
#[derive(Debug, PartialEq)]
pub enum Build {
    /// an unsigned IPA; `missing` names the secrets a release lacked (today's warning), if any
    Unsigned { missing: Vec<&'static str> },
    /// the App Store build, uploaded to TestFlight when `upload`
    AppStore { credentials: Credentials, upload: bool },
}

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub enum Wanted {
    /// today's `ios` job's choice
    #[default]
    Auto,
    Unsigned,
    AppStore,
}

/// The build for `wanted` and `upload` (see the module's step 1).
pub fn choose(wanted: Wanted, upload: bool, env: &Env) -> Result<Build, String> {
    let git_ref = env("GITHUB_REF");
    let refused = |what: &str| {
        format!(
            "{what} only from main or a v* tag (GITHUB_REF is {}): nothing reaches App Store Connect from another branch, \
             a pull request or a run by hand",
            git_ref.as_deref().unwrap_or("unset")
        )
    };
    let app_store = |credentials: Credentials| {
        check_shapes(&credentials)?;
        if upload && !may_publish(git_ref.as_deref()) {
            return Err(refused("TestFlight uploads go out"));
        }
        Ok(Build::AppStore { credentials, upload })
    };
    match wanted {
        Wanted::Unsigned if upload => Err("--upload needs the App Store build, not --unsigned".to_owned()),
        Wanted::Unsigned => Ok(Build::Unsigned { missing: Vec::new() }),
        Wanted::AppStore => {
            if !may_publish(git_ref.as_deref()) {
                return Err(refused("the App Store build is made"));
            }
            app_store(credentials(env).map_err(|missing| format!("the App Store build needs {}", missing.join(", ")))?)
        }
        Wanted::Auto if !is_release(env) => Ok(Build::Unsigned { missing: Vec::new() }),
        Wanted::Auto => match credentials(env) {
            Ok(credentials) => app_store(credentials),
            Err(missing) => Ok(Build::Unsigned { missing }),
        },
    }
}

/// Today's warning for a release without all the secrets.
pub fn missing_warning(missing: &[&str]) -> String {
    format!("Not uploaded to TestFlight: missing secrets: {}. An unsigned IPA instead.", missing.join(" "))
}

/// Today's step summary line for it.
pub fn missing_summary(missing: &[&str]) -> String {
    format!("**iOS:** not uploaded to TestFlight, missing secrets: {}\n", missing.join(" "))
}

/// The export method's name for an `xcodebuild -version` output: Xcode 15.4 renamed `app-store` to
/// `app-store-connect`, and the Tauri CLI picks by the version as here.
pub fn export_method(xcodebuild_version: &str) -> Result<&'static str, String> {
    let caps = Regex::new(r"Xcode ([0-9]+)\.([0-9]+)")
        .expect("pattern")
        .captures(xcodebuild_version)
        .ok_or_else(|| format!("no Xcode version in {:?}", xcodebuild_version.lines().next().unwrap_or_default()))?;
    let major: u64 = caps[1].parse().map_err(|e| format!("Xcode version: {e}"))?;
    let minor: u64 = caps[2].parse().map_err(|e| format!("Xcode version: {e}"))?;
    Ok(if (major, minor) < (15, 4) { "app-store" } else { "app-store-connect" })
}

/// The export options the Tauri CLI merges into its template's `ExportOptions.plist` for
/// `--export-method app-store-connect`: the method, automatic signing (the target's CODE_SIGN_STYLE)
/// and the team. `team` must have passed [`check_shapes`].
pub fn export_options(method: &str, team: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>method</key>\n\
         \t<string>{method}</string>\n\
         \t<key>signingStyle</key>\n\
         \t<string>automatic</string>\n\
         \t<key>teamID</key>\n\
         \t<string>{team}</string>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// The folders of one build under `platform/ios/build` (git-ignored).
pub struct Paths {
    pub project: PathBuf,
    pub build: PathBuf,
}

impl Paths {
    pub fn new(root: &Path) -> Paths {
        let project = root.join(PROJECT);
        let build = project.join("build");
        Paths { project, build }
    }
    pub fn archive(&self) -> PathBuf {
        self.build.join(format!("{PRODUCT}.xcarchive"))
    }
    /// The archived app.
    pub fn app(&self) -> PathBuf {
        self.archive().join("Products/Applications").join(format!("{PRODUCT}.app"))
    }
    pub fn export(&self) -> PathBuf {
        self.build.join("export")
    }
}

/// `xcodegen generate` of the checked-in spec.
pub fn xcodegen(paths: &Paths) -> Command {
    let mut c = Command::new("xcodegen");
    c.args(["generate", "--spec", "project.yml"]).current_dir(&paths.project);
    c
}

/// The Release archive for devices, unsigned (cargo-mobile2's `skip_codesign` flags), at the
/// release's versions.
pub fn archive(paths: &Paths, short: &str, bundle: &str) -> Command {
    let mut c = Command::new("xcodebuild");
    c.arg("-project")
        .arg(paths.project.join(format!("{PRODUCT}.xcodeproj")))
        .args(["-scheme", PRODUCT, "-configuration", "Release", "-sdk", "iphoneos", "-destination", "generic/platform=iOS"])
        .arg("-derivedDataPath")
        .arg(paths.build.join("DerivedData"))
        .arg("-archivePath")
        .arg(paths.archive())
        .arg(format!("MARKETING_VERSION={short}"))
        .arg(format!("CURRENT_PROJECT_VERSION={bundle}"))
        .args(["CODE_SIGNING_REQUIRED=NO", "CODE_SIGNING_ALLOWED=NO", "CODE_SIGN_IDENTITY=", "CODE_SIGN_ENTITLEMENTS="])
        .arg("archive")
        .current_dir(&paths.project);
    c
}

/// The archived app's executable signed ad hoc with today's (empty) entitlements, so the export finds
/// them (the Tauri CLI signs the same file with a throwaway certificate for the same reason).
pub fn adhoc_sign(paths: &Paths) -> Command {
    let mut c = Command::new("codesign");
    c.args(["--force", "-s", "-", "--entitlements"])
        .arg(paths.project.join(format!("{PRODUCT}.entitlements")))
        .arg(paths.app().join(PRODUCT));
    c
}

/// The App Store export: cloud signing through the API key (`-allowProvisioningUpdates` and the
/// `-authentication…` arguments cargo-mobile2 passes).
pub fn export(paths: &Paths, options: &Path, credentials: &Credentials, key_file: &Path) -> Command {
    let mut c = Command::new("xcodebuild");
    c.arg("-exportArchive")
        .arg("-archivePath")
        .arg(paths.archive())
        .arg("-exportOptionsPlist")
        .arg(options)
        .arg("-exportPath")
        .arg(paths.export())
        .arg("-allowProvisioningUpdates")
        .args(["-authenticationKeyID", &credentials.key_id])
        .arg("-authenticationKeyPath")
        .arg(key_file)
        .args(["-authenticationKeyIssuerID", &credentials.issuer]);
    c
}

/// Today's TestFlight upload. altool finds `AuthKey_<key id>.p8` in `~/.appstoreconnect/private_keys`
/// (or in `API_PRIVATE_KEYS_DIR`, set when the key file is elsewhere).
pub fn altool(ipa: &Path, credentials: &Credentials, keys_dir: Option<&Path>) -> Command {
    let mut c = Command::new("xcrun");
    c.args(["altool", "--upload-app", "--type", "ios", "--file"])
        .arg(ipa)
        .args(["--apiKey", &credentials.key_id, "--apiIssuer", &credentials.issuer]);
    if let Some(dir) = keys_dir {
        c.env("API_PRIVATE_KEYS_DIR", dir);
    }
    c
}

/// `command` with the secrets taken out of its environment: the builds run third-party code (cargo's
/// build scripts under Xcode's build phase), which has no business with them.
pub fn without_secrets(mut command: Command) -> Command {
    for name in SCRUBBED {
        command.env_remove(name);
    }
    command
}

/// `command` as one line, each argument holding a value of `hidden` shown as `***`.
pub fn shown(command: &Command, hidden: &[&str]) -> String {
    let mut words = vec![command.get_program().to_string_lossy().into_owned()];
    for arg in command.get_args() {
        let arg = arg.to_string_lossy();
        words.push(if hidden.iter().any(|h| !h.is_empty() && arg.contains(h)) { "***".to_owned() } else { arg.into_owned() });
    }
    words.join(" ")
}

/// One entry of the archived app's `Info.plist` (as `plutil -extract <key> raw` prints it).
pub type PlistValue<'a> = (&'a str, &'a str);

/// Checks the archived app: its `Info.plist` values, its executable's architectures (`lipo -archs`)
/// and the compiled asset catalog and launch screen beside it.
pub fn check_app(values: &[PlistValue], archs: &str, app: &Path, short: &str, bundle: &str) -> Result<(), String> {
    let expected = [
        ("CFBundleIdentifier", BUNDLE_ID),
        ("CFBundleExecutable", PRODUCT),
        ("CFBundleName", PRODUCT),
        ("CFBundleShortVersionString", short),
        ("CFBundleVersion", bundle),
        ("MinimumOSVersion", MINIMUM_IOS),
        ("ITSAppUsesNonExemptEncryption", "false"),
        ("UILaunchStoryboardName", "LaunchScreen"),
    ];
    for (key, want) in expected {
        let got = values.iter().find(|(k, _)| *k == key).map(|(_, v)| v.trim());
        if got != Some(want) {
            return Err(format!("the app's Info.plist has {key} = {}, not {want}", got.unwrap_or("nothing")));
        }
    }
    if archs.split_whitespace().collect::<Vec<_>>() != ["arm64"] {
        return Err(format!("the app's executable is built for {}, not arm64 alone", archs.trim()));
    }
    for part in [PRODUCT, "Assets.car", "LaunchScreen.storyboardc"] {
        if !app.join(part).exists() {
            return Err(format!("the app has no {part}"));
        }
    }
    Ok(())
}

/// The keys [`check_app`] reads.
const CHECKED_KEYS: [&str; 8] = [
    "CFBundleIdentifier",
    "CFBundleExecutable",
    "CFBundleName",
    "CFBundleShortVersionString",
    "CFBundleVersion",
    "MinimumOSVersion",
    "ITSAppUsesNonExemptEncryption",
    "UILaunchStoryboardName",
];

/// Writes `app` (a `.app` folder) into `ipa` as `Payload/<name>.app`, as the Tauri CLI's `create_ipa`
/// does: a `Payload/` entry, then the app's folders and files, deflated, each with Unix mode 0755.
/// Files are read through links; no entry may pass the zip format's 4 GB limits (no zip64).
pub fn write_ipa(app: &Path, ipa: &Path) -> Result<usize, String> {
    let name = app.file_name().ok_or_else(|| format!("{} has no name", app.display()))?.to_string_lossy().into_owned();
    let mut entries = vec![(format!("Payload/{name}/"), None)];
    collect(app, &format!("Payload/{name}"), &mut entries)?;
    let file = fs::File::create(ipa).map_err(|e| format!("{}: {e}", ipa.display()))?;
    let mut out = Zip { out: BufWriter::new(file), offset: 0, central: Vec::new(), count: 0 };
    out.add(&"Payload/".to_owned(), None)?;
    for (entry, path) in &entries {
        let data = match path {
            Some(path) => Some(fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?),
            None => None,
        };
        out.add(entry, data.as_deref())?;
    }
    out.finish().map_err(|e| format!("{}: {e}", ipa.display()))
}

/// The folders (ending in `/`, no path) and files under `dir`, named under `prefix`, sorted.
fn collect(dir: &Path, prefix: &str, entries: &mut Vec<(String, Option<PathBuf>)>) -> Result<(), String> {
    let mut children: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| format!("{}: {e}", dir.display())))
        .collect::<Result<_, _>>()?;
    children.sort();
    for path in children {
        let name = format!("{prefix}/{}", path.file_name().unwrap_or_default().to_string_lossy());
        if path.is_dir() {
            entries.push((format!("{name}/"), None));
            collect(&path, &name, entries)?;
        } else {
            entries.push((name, Some(path)));
        }
    }
    Ok(())
}

/// A zip archive being written: each entry's local header and data, then the central directory.
struct Zip<W: Write> {
    out: W,
    offset: u64,
    central: Vec<u8>,
    count: usize,
}

/// 1980-01-01 00:00 in MS-DOS form (date, time): the zip format's earliest time.
const DOS_DATE: u16 = (1 << 5) | 1;

impl<W: Write> Zip<W> {
    /// A folder (`data` `None`, stored) or a file (deflated).
    fn add(&mut self, name: &str, data: Option<&[u8]>) -> Result<(), String> {
        let (method, crc, size, body) = match data {
            None => (0u16, 0u32, 0u64, Vec::new()),
            Some(data) => {
                let mut crc = flate2::Crc::new();
                crc.update(data);
                let mut deflate = DeflateEncoder::new(Vec::new(), Compression::default());
                deflate.write_all(data).map_err(|e| format!("{name}: {e}"))?;
                (8, crc.sum(), data.len() as u64, deflate.finish().map_err(|e| format!("{name}: {e}"))?)
            }
        };
        let limit = u64::from(u32::MAX);
        if size >= limit || body.len() as u64 >= limit || self.offset >= limit || self.count >= usize::from(u16::MAX) {
            return Err(format!("{name}: the IPA would need zip64"));
        }
        let (size, packed, offset) = (size as u32, body.len() as u32, self.offset as u32);
        let mode: u32 = if data.is_some() { 0o100_755 } else { 0o040_755 };
        let dos: u32 = if data.is_some() { 0 } else { 0x10 };
        let name_len = u16::try_from(name.len()).map_err(|_| format!("{name}: name too long"))?;

        let mut local = Vec::with_capacity(30 + name.len());
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        for v in [20u16, 0, method, 0, DOS_DATE] {
            local.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, packed, size] {
            local.extend_from_slice(&v.to_le_bytes());
        }
        for v in [name_len, 0] {
            local.extend_from_slice(&v.to_le_bytes());
        }
        local.extend_from_slice(name.as_bytes());

        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        // made by Unix (3), zip 2.0; needs 2.0
        for v in [(3u16 << 8) | 20, 20, 0, method, 0, DOS_DATE] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, packed, size] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        for v in [name_len, 0, 0, 0, 0] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        for v in [(mode << 16) | dos, offset] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        c.extend_from_slice(name.as_bytes());

        self.out.write_all(&local).and_then(|()| self.out.write_all(&body)).map_err(|e| format!("{name}: {e}"))?;
        self.offset += (local.len() + body.len()) as u64;
        self.count += 1;
        Ok(())
    }

    /// The central directory and its end record; the number of entries.
    fn finish(mut self) -> std::io::Result<usize> {
        let (start, len, count) = (self.offset as u32, self.central.len() as u32, self.count as u16);
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        for v in [0u16, 0, count, count] {
            end.extend_from_slice(&v.to_le_bytes());
        }
        for v in [len, start] {
            end.extend_from_slice(&v.to_le_bytes());
        }
        end.extend_from_slice(&0u16.to_le_bytes());
        self.out.write_all(&self.central)?;
        self.out.write_all(&end)?;
        self.out.flush()?;
        Ok(self.count)
    }
}

/// The command line.
#[derive(Debug, Default, PartialEq)]
struct Args {
    wanted: Wanted,
    upload: bool,
    version: Option<String>,
    build_number: Option<String>,
    out: Option<PathBuf>,
    dry_run: bool,
}

pub const USAGE: &str = "usage: cargo xtask ios-release [--unsigned | --app-store] [--upload] [--version X.Y.Z] \
                         [--build-number N] [--out DIR] [--dry-run]";

fn args(list: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut it = list.iter();
    let set = |wanted: Wanted, parsed: &mut Args| {
        if parsed.wanted != Wanted::Auto && parsed.wanted != wanted {
            return Err(format!("--unsigned and --app-store exclude each other\n{USAGE}"));
        }
        parsed.wanted = wanted;
        Ok(())
    };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--unsigned" => set(Wanted::Unsigned, &mut parsed)?,
            "--app-store" => set(Wanted::AppStore, &mut parsed)?,
            "--upload" => parsed.upload = true,
            "--version" => parsed.version = Some(value("--version")?),
            "--build-number" => parsed.build_number = Some(value("--build-number")?),
            "--out" => parsed.out = Some(PathBuf::from(value("--out")?)),
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
             (the app compiles its version in, and App Store Connect would see the files' one)"
        )),
        _ => Ok(files),
    }
}

/// A warning, as a GitHub Actions annotation there.
fn warn(env: &Env, message: &str) {
    if env("GITHUB_ACTIONS").is_some() {
        println!("::warning::{message}");
    } else {
        eprintln!("warning: {message}");
    }
}

/// Files written for one build (the API key, the export options), removed when dropped.
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

/// The key file for the export and altool's folder for it: the given file, or `AuthKey_<id>.p8` in
/// `~/.appstoreconnect/private_keys` (today's place), written there if it is not.
fn key_file(credentials: &Credentials, env: &Env, written: &mut Materialised) -> Result<(PathBuf, Option<PathBuf>), String> {
    let file_name = format!("AuthKey_{}.p8", credentials.key_id);
    let home = env("HOME").map(PathBuf::from).ok_or("no HOME for the App Store Connect key")?;
    let standard = home.join(".appstoreconnect/private_keys");
    let contents = match &credentials.key {
        Key::File(path) if !path.is_file() => return Err(format!("{KEY_PATH_VAR} names no file")),
        // altool looks for the file by its name
        Key::File(path) if path.file_name().is_some_and(|n| n.to_string_lossy() == file_name) => {
            return Ok((path.clone(), path.parent().map(Path::to_path_buf)));
        }
        Key::File(path) => fs::read(path).map_err(|e| format!("{KEY_PATH_VAR}: {e}"))?,
        Key::Contents(text) => {
            let mut bytes = text.clone().into_bytes();
            if !text.ends_with('\n') {
                bytes.push(b'\n');
            }
            bytes
        }
    };
    let path = standard.join(&file_name);
    if !path.is_file() {
        fs::create_dir_all(&standard).map_err(|e| format!("{}: {e}", standard.display()))?;
        written.write(path.clone(), &contents)?;
    }
    Ok((path, None))
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
fn github_file(env: &Env, var: &str, lines: &str) -> Result<(), String> {
    let Some(path) = env(var) else { return Ok(()) };
    let mut file = fs::OpenOptions::new().append(true).create(true).open(&path).map_err(|e| format!("{var}: {e}"))?;
    file.write_all(lines.as_bytes()).map_err(|e| format!("{var}: {e}"))
}

pub fn run(list: &[String]) -> Result<(), String> {
    let args = args(list)?;
    release(&bump::repo_root(), &args, &|name| std::env::var(name).ok().filter(|v| !v.is_empty())).map(|_| ())
}

/// The release under `root`: the IPA's path, or `None` for a dry run.
fn release(root: &Path, args: &Args, env: &Env) -> Result<Option<PathBuf>, String> {
    let paths = Paths::new(root);
    let version = release_version(root, args.version.as_deref())?;
    let (short, bundle) = bundle_versions(&version, args.build_number.as_deref())?;
    if short != version && args.build_number.is_none() {
        warn(env, &format!("{version} is not x.y.z: CFBundleShortVersionString is {short}, CFBundleVersion {bundle}"));
    }
    let build = choose(args.wanted, args.upload, env)?;
    let signed = matches!(build, Build::AppStore { .. });
    let target_dir = env("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let out = args.out.clone().unwrap_or_else(|| target_dir.join("ios"));
    let name = ipa_name(&version, signed);
    let hidden = match &build {
        Build::AppStore { credentials, .. } => credentials.hidden(),
        Build::Unsigned { .. } => Vec::new(),
    };
    println!(
        "iOS release {version} (CFBundleShortVersionString {short}, CFBundleVersion {bundle}), {BUNDLE_ID}, iOS {MINIMUM_IOS}, \
         the Rust library for {RUST_TARGET}"
    );
    match &build {
        Build::AppStore { upload, .. } => println!(
            "the App Store build (cloud signing with the App Store Connect key){}",
            if *upload { ", uploaded to TestFlight" } else { "" }
        ),
        Build::Unsigned { missing } if !missing.is_empty() => {
            warn(env, &missing_warning(missing));
            github_file(env, "GITHUB_STEP_SUMMARY", &missing_summary(missing))?;
        }
        Build::Unsigned { .. } => println!("an unsigned IPA"),
    }
    println!("-> {}", out.join(&name).display());
    let mut steps = vec![xcodegen(&paths), archive(&paths, &short, &bundle)];
    if let Build::AppStore { credentials, upload } = &build {
        steps.push(adhoc_sign(&paths));
        let options = paths.build.join("ExportOptions.plist");
        let key = Path::new("~/.appstoreconnect/private_keys").join(format!("AuthKey_{}.p8", credentials.key_id));
        steps.push(export(&paths, &options, credentials, &key));
        if *upload {
            steps.push(altool(&out.join(&name), credentials, None));
        }
    }
    for step in &steps {
        println!("  {}", shown(step, &hidden));
    }
    if args.dry_run {
        return Ok(None);
    }
    if !cfg!(target_os = "macos") {
        return Err("the iOS build needs a Mac with Xcode and xcodegen (--dry-run shows the plan anywhere)".to_owned());
    }
    let mut probe = Command::new("xcodegen");
    probe.arg("--version");
    output(without_secrets(probe), "xcodegen (brew install xcodegen)")?;

    run_command(without_secrets(xcodegen(&paths)), "xcodegen generate")?;
    for stale in [paths.archive(), paths.export()] {
        if stale.exists() {
            fs::remove_dir_all(&stale).map_err(|e| format!("{}: {e}", stale.display()))?;
        }
    }
    run_command(without_secrets(archive(&paths, &short, &bundle)), "xcodebuild archive")?;
    let app = paths.app();
    let plist = app.join("Info.plist");
    let mut values = Vec::new();
    for key in CHECKED_KEYS {
        let mut plutil = Command::new("plutil");
        plutil.args(["-extract", key, "raw", "-o", "-"]).arg(&plist);
        values.push((key, output(without_secrets(plutil), &format!("plutil -extract {key}")).unwrap_or_default()));
    }
    let mut lipo = Command::new("xcrun");
    lipo.args(["lipo", "-archs"]).arg(app.join(PRODUCT));
    let archs = output(without_secrets(lipo), "lipo -archs")?;
    let pairs: Vec<PlistValue> = values.iter().map(|(k, v)| (*k, v.as_str())).collect();
    check_app(&pairs, &archs, &app, &short, &bundle)?;
    println!("  {}: {BUNDLE_ID} {short} ({bundle}), iOS {MINIMUM_IOS}, {}", app.display(), archs.trim());

    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let dest = out.join(&name);
    let mut uploaded = false;
    match &build {
        Build::Unsigned { .. } => {
            let entries = write_ipa(&app, &dest)?;
            println!("  {entries} entries zipped");
        }
        Build::AppStore { credentials, upload } => {
            run_command(without_secrets(adhoc_sign(&paths)), "codesign (ad hoc, for the entitlements)")?;
            let mut xcode = Command::new("xcodebuild");
            xcode.arg("-version");
            let method = export_method(&output(without_secrets(xcode), "xcodebuild -version")?)?;
            // only now, after the Rust build: no build script ran while the key was on disk
            let mut written = Materialised(Vec::new());
            let (key, keys_dir) = key_file(credentials, env, &mut written)?;
            let options = paths.build.join("ExportOptions.plist");
            if options.exists() {
                fs::remove_file(&options).map_err(|e| format!("{}: {e}", options.display()))?;
            }
            written.write(options.clone(), export_options(method, &credentials.team).as_bytes())?;
            run_command(without_secrets(export(&paths, &options, credentials, &key)), "xcodebuild -exportArchive")?;
            let exported = fs::read_dir(paths.export())
                .map_err(|e| format!("{}: {e}", paths.export().display()))?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|e| e == "ipa"))
                .ok_or("the export made no IPA")?;
            fs::copy(&exported, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            if *upload {
                if !may_publish(env("GITHUB_REF").as_deref()) {
                    return Err("TestFlight uploads go out only from main or a v* tag".to_owned());
                }
                run_command(without_secrets(altool(&dest, credentials, keys_dir.as_deref())), "xcrun altool --upload-app")?;
                uploaded = true;
            }
            drop(written);
        }
    }
    let mb = fs::metadata(&dest).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0);
    println!("{} ({mb:.1} MB){}", dest.display(), if uploaded { ", uploaded to TestFlight" } else { "" });
    github_file(env, "GITHUB_OUTPUT", &format!("ipa={}\nname={name}\nsigned={signed}\nuploaded={uploaded}\n", dest.display()))?;
    github_file(
        env,
        "GITHUB_STEP_SUMMARY",
        &format!(
            "iOS: `{name}`, {short} ({bundle}), {}, {mb:.1} MB{}\n",
            if signed { "App Store build" } else { "unsigned" },
            if uploaded { ", uploaded to TestFlight" } else { "" }
        ),
    )?;
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Read;

    /// Made-up secrets: nothing like real ones, only their shapes.
    const ISSUER: &str = "00000000-test-0000-issuer-000000000000";
    const KEY_ID: &str = "TESTKEYID0";
    const TEAM: &str = "TESTTEAM00";

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| map.get(name).cloned().filter(|v| !v.is_empty())
    }

    /// A push of `git_ref` with all four secrets.
    fn release_env(event: &str, git_ref: &str) -> impl Fn(&str) -> Option<String> + use<> {
        env_of(&[
            ("GITHUB_EVENT_NAME", event),
            ("GITHUB_REF", git_ref),
            ("APPLE_API_ISSUER", ISSUER),
            ("APPLE_API_KEY_ID", KEY_ID),
            ("APPLE_API_KEY", "test key contents"),
            ("APPLE_DEVELOPMENT_TEAM", TEAM),
        ])
    }

    fn creds() -> Credentials {
        Credentials {
            issuer: ISSUER.to_owned(),
            key_id: KEY_ID.to_owned(),
            key: Key::Contents("test key contents".to_owned()),
            team: TEAM.to_owned(),
        }
    }

    /// A folder of its own under the system's temp folder, removed when dropped.
    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Tree {
            let root = std::env::temp_dir().join(format!("xtask-ios-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Tree(root)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn args_of(command: &Command) -> Vec<String> {
        command.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn repo(path: &str) -> PathBuf {
        bump::repo_root().join(path)
    }

    #[test]
    fn the_versions_are_the_tauri_clis() {
        let v = |version: &str, build: Option<&str>| bundle_versions(version, build);
        assert_eq!(v("0.3.16", None), Ok(("0.3.16".to_owned(), "0.3.16".to_owned())));
        assert_eq!(v("0.3.0", None), Ok(("0.3.0".to_owned(), "0.3.0".to_owned())));
        // a pre-release tag is dropped from both, as the Tauri CLI drops it
        assert_eq!(v("1.2.3-beta.1", None), Ok(("1.2.3".to_owned(), "1.2.3".to_owned())));
        // the refresh's build number (bundle.iOS.bundleVersion today)
        assert_eq!(v("0.3.16", Some("2026111306")), Ok(("0.3.16".to_owned(), "2026111306".to_owned())));
        assert_eq!(v("0.3.16", Some("4.5")), Ok(("0.3.16".to_owned(), "4.5".to_owned())));
        // App Store Connect takes one to three integers: build metadata as a fourth part is refused
        assert!(v("1.2.3+7", None).unwrap_err().contains("1.2.3.7"));
        for bad in ["1.2.3.4", "12a", "", "1..2", "-1"] {
            assert!(v("0.3.16", Some(bad)).is_err(), "{bad:?}");
        }
        for bad in ["0.3", "v0.3.16", "0.3.16.1", "x.y.z"] {
            assert!(v(bad, None).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_ipa_names_are_todays() {
        assert_eq!(ipa_name("0.3.16", false), "RustRadar_0.3.16_unsigned.ipa");
        assert_eq!(ipa_name("0.3.16", true), "RustRadar_0.3.16.ipa");
    }

    #[test]
    fn only_main_and_v_tags_may_publish() {
        for yes in ["refs/heads/main", "refs/tags/v0.3.16", "refs/tags/v1.0.0"] {
            assert!(may_publish(Some(yes)), "{yes}");
        }
        for no in ["refs/heads/rust-port", "refs/pull/12/merge", "refs/heads/main-old", "refs/tags/0.3.16", "refs/heads/feature/v1", "main", ""] {
            assert!(!may_publish(Some(no)), "{no}");
        }
        assert!(!may_publish(None));
    }

    #[test]
    fn a_release_is_todays_release() {
        // RELEASE: github.event_name == 'push' && (main || startsWith(ref, 'refs/tags/v'))
        let release = |event: &str, git_ref: &str| is_release(&env_of(&[("GITHUB_EVENT_NAME", event), ("GITHUB_REF", git_ref)]));
        assert!(release("push", "refs/heads/main"));
        assert!(release("push", "refs/tags/v0.3.17"));
        assert!(!release("push", "refs/heads/rust-port"));
        assert!(!release("pull_request", "refs/pull/3/merge"));
        assert!(!release("workflow_dispatch", "refs/heads/main"));
        assert!(!release("schedule", "refs/heads/main"));
        assert!(!is_release(&env_of(&[])));
    }

    #[test]
    fn the_choice_is_todays_ios_jobs() {
        // a release with the four secrets: the App Store build, uploaded when asked
        let build = choose(Wanted::Auto, true, &release_env("push", "refs/heads/main")).unwrap();
        assert_eq!(build, Build::AppStore { credentials: creds(), upload: true });
        let build = choose(Wanted::Auto, true, &release_env("push", "refs/tags/v0.3.17")).unwrap();
        assert!(matches!(build, Build::AppStore { upload: true, .. }));
        // a key already written to a file counts for APPLE_API_KEY
        let env = env_of(&[
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_REF", "refs/heads/main"),
            ("APPLE_API_ISSUER", ISSUER),
            ("APPLE_API_KEY_ID", KEY_ID),
            ("APPLE_API_KEY_PATH", "/tmp/AuthKey_TESTKEYID0.p8"),
            ("APPLE_DEVELOPMENT_TEAM", TEAM),
        ]);
        let Build::AppStore { credentials, .. } = choose(Wanted::Auto, false, &env).unwrap() else { panic!("not the App Store build") };
        assert_eq!(credentials.key, Key::File(PathBuf::from("/tmp/AuthKey_TESTKEYID0.p8")));
        // a release without some of them: today's warning and summary, and an unsigned IPA
        let env = env_of(&[
            ("GITHUB_EVENT_NAME", "push"),
            ("GITHUB_REF", "refs/heads/main"),
            ("APPLE_API_KEY_ID", KEY_ID),
            ("APPLE_API_KEY", "test key contents"),
        ]);
        let build = choose(Wanted::Auto, true, &env).unwrap();
        assert_eq!(build, Build::Unsigned { missing: vec!["APPLE_API_ISSUER", "APPLE_DEVELOPMENT_TEAM"] });
        assert_eq!(
            missing_warning(&["APPLE_API_ISSUER", "APPLE_DEVELOPMENT_TEAM"]),
            "Not uploaded to TestFlight: missing secrets: APPLE_API_ISSUER APPLE_DEVELOPMENT_TEAM. An unsigned IPA instead."
        );
        assert_eq!(missing_summary(&SECRETS), "**iOS:** not uploaded to TestFlight, missing secrets: APPLE_API_ISSUER APPLE_API_KEY_ID APPLE_API_KEY APPLE_DEVELOPMENT_TEAM\n");
        assert_eq!(choose(Wanted::Auto, false, &env_of(&[("GITHUB_EVENT_NAME", "push"), ("GITHUB_REF", "refs/heads/main")])).unwrap(), Build::Unsigned { missing: SECRETS.to_vec() });
        // not a release: unsigned, quietly, secrets or not, upload asked or not
        for (event, git_ref) in [("push", "refs/heads/rust-port"), ("pull_request", "refs/pull/3/merge"), ("workflow_dispatch", "refs/heads/main")] {
            assert_eq!(choose(Wanted::Auto, true, &release_env(event, git_ref)).unwrap(), Build::Unsigned { missing: Vec::new() }, "{event} {git_ref}");
        }
        assert_eq!(choose(Wanted::Unsigned, false, &release_env("push", "refs/heads/main")).unwrap(), Build::Unsigned { missing: Vec::new() });
    }

    #[test]
    fn nothing_reaches_app_store_connect_from_rust_port() {
        for git_ref in ["refs/heads/rust-port", "refs/pull/3/merge", "refs/heads/other"] {
            for event in ["push", "workflow_dispatch", "schedule"] {
                let err = choose(Wanted::AppStore, true, &release_env(event, git_ref)).unwrap_err();
                assert!(err.contains("only from main or a v* tag") && err.contains(git_ref), "{err}");
                assert!(choose(Wanted::AppStore, false, &release_env(event, git_ref)).is_err());
            }
        }
        // a run by hand has no GITHUB_REF
        let mut by_hand = vec![("APPLE_API_ISSUER", ISSUER), ("APPLE_API_KEY_ID", KEY_ID), ("APPLE_DEVELOPMENT_TEAM", TEAM)];
        by_hand.push(("APPLE_API_KEY", "test key contents"));
        assert!(choose(Wanted::AppStore, false, &env_of(&by_hand)).unwrap_err().contains("unset"));
        // testflight-refresh: scheduled or run by hand on main
        for event in ["schedule", "workflow_dispatch"] {
            assert_eq!(choose(Wanted::AppStore, true, &release_env(event, "refs/heads/main")).unwrap(), Build::AppStore { credentials: creds(), upload: true });
        }
        // --app-store without the secrets names them
        let err = choose(Wanted::AppStore, true, &env_of(&[("GITHUB_REF", "refs/heads/main"), ("APPLE_API_KEY_ID", KEY_ID)])).unwrap_err();
        assert_eq!(err, "the App Store build needs APPLE_API_ISSUER, APPLE_API_KEY, APPLE_DEVELOPMENT_TEAM");
        assert!(choose(Wanted::Unsigned, true, &release_env("push", "refs/heads/main")).is_err());
    }

    #[test]
    fn bad_secret_shapes_are_named_not_shown() {
        for (name, value) in [("APPLE_API_KEY_ID", "../../evil"), ("APPLE_DEVELOPMENT_TEAM", "A</string>B"), ("APPLE_API_ISSUER", "x y")] {
            let mut pairs = vec![("GITHUB_EVENT_NAME", "push"), ("GITHUB_REF", "refs/heads/main"), ("APPLE_API_KEY", "test key contents")];
            for (k, v) in [("APPLE_API_ISSUER", ISSUER), ("APPLE_API_KEY_ID", KEY_ID), ("APPLE_DEVELOPMENT_TEAM", TEAM)] {
                pairs.push((k, if k == name { value } else { v }));
            }
            let err = choose(Wanted::Auto, false, &env_of(&pairs)).unwrap_err();
            assert!(err.contains(name) && !err.contains(value), "{err}");
        }
    }

    #[test]
    fn the_export_options_are_the_tauri_clis() {
        assert_eq!(export_method("Xcode 16.4\nBuild version 16F6\n"), Ok("app-store-connect"));
        assert_eq!(export_method("Xcode 26.0.1\nBuild version 17A400\n"), Ok("app-store-connect"));
        assert_eq!(export_method("Xcode 15.4\nBuild version 15F31d\n"), Ok("app-store-connect"));
        assert_eq!(export_method("Xcode 15.3\nBuild version 15E204a\n"), Ok("app-store"));
        assert_eq!(export_method("Xcode 14.3.1\n"), Ok("app-store"));
        assert!(export_method("command not found").is_err());
        assert_eq!(
            export_options("app-store-connect", TEAM),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \t<key>method</key>\n\t<string>app-store-connect</string>\n\
             \t<key>signingStyle</key>\n\t<string>automatic</string>\n\
             \t<key>teamID</key>\n\t<string>TESTTEAM00</string>\n\
             </dict>\n</plist>\n"
        );
    }

    #[test]
    fn the_commands_are_todays() {
        let paths = Paths::new(Path::new("/repo"));
        let xg = xcodegen(&paths);
        assert_eq!(args_of(&xg), ["generate", "--spec", "project.yml"]);
        assert_eq!(xg.get_current_dir(), Some(Path::new("/repo/platform/ios")));

        let a = archive(&paths, "0.3.16", "2026111306");
        assert_eq!(a.get_program(), "xcodebuild");
        assert_eq!(
            args_of(&a),
            [
                "-project",
                "/repo/platform/ios/RustRadar.xcodeproj",
                "-scheme",
                "RustRadar",
                "-configuration",
                "Release",
                "-sdk",
                "iphoneos",
                "-destination",
                "generic/platform=iOS",
                "-derivedDataPath",
                "/repo/platform/ios/build/DerivedData",
                "-archivePath",
                "/repo/platform/ios/build/RustRadar.xcarchive",
                "MARKETING_VERSION=0.3.16",
                "CURRENT_PROJECT_VERSION=2026111306",
                "CODE_SIGNING_REQUIRED=NO",
                "CODE_SIGNING_ALLOWED=NO",
                "CODE_SIGN_IDENTITY=",
                "CODE_SIGN_ENTITLEMENTS=",
                "archive",
            ]
        );
        assert_eq!(paths.app(), Path::new("/repo/platform/ios/build/RustRadar.xcarchive/Products/Applications/RustRadar.app"));

        let s = adhoc_sign(&paths);
        assert_eq!(s.get_program(), "codesign");
        assert_eq!(
            args_of(&s),
            [
                "--force",
                "-s",
                "-",
                "--entitlements",
                "/repo/platform/ios/RustRadar.entitlements",
                "/repo/platform/ios/build/RustRadar.xcarchive/Products/Applications/RustRadar.app/RustRadar",
            ]
        );

        let c = creds();
        let e = export(&paths, Path::new("/tmp/ExportOptions.plist"), &c, Path::new("/keys/AuthKey_TESTKEYID0.p8"));
        assert_eq!(
            args_of(&e),
            [
                "-exportArchive",
                "-archivePath",
                "/repo/platform/ios/build/RustRadar.xcarchive",
                "-exportOptionsPlist",
                "/tmp/ExportOptions.plist",
                "-exportPath",
                "/repo/platform/ios/build/export",
                "-allowProvisioningUpdates",
                "-authenticationKeyID",
                KEY_ID,
                "-authenticationKeyPath",
                "/keys/AuthKey_TESTKEYID0.p8",
                "-authenticationKeyIssuerID",
                ISSUER,
            ]
        );

        // today's: xcrun altool --upload-app --type ios --file "$ipa" --apiKey "$APPLE_API_KEY_ID" --apiIssuer "$APPLE_API_ISSUER"
        let u = altool(Path::new("/out/RustRadar_0.3.16.ipa"), &c, None);
        assert_eq!(u.get_program(), "xcrun");
        assert_eq!(
            args_of(&u),
            ["altool", "--upload-app", "--type", "ios", "--file", "/out/RustRadar_0.3.16.ipa", "--apiKey", KEY_ID, "--apiIssuer", ISSUER]
        );
        assert_eq!(u.get_envs().count(), 0);
        let u = altool(Path::new("/out/x.ipa"), &c, Some(Path::new("/runner/temp")));
        let envs: Vec<_> = u.get_envs().collect();
        assert_eq!(envs, [(std::ffi::OsStr::new("API_PRIVATE_KEYS_DIR"), Some(std::ffi::OsStr::new("/runner/temp")))]);
    }

    #[test]
    fn secrets_stay_out_of_children_and_of_what_is_printed() {
        let c = creds();
        let paths = Paths::new(Path::new("/repo"));
        let command = without_secrets(export(&paths, Path::new("/o.plist"), &c, Path::new("/k/AuthKey_TESTKEYID0.p8")));
        let removed: Vec<String> =
            command.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        for name in SECRETS.iter().chain([&KEY_PATH_VAR]).chain(&["TAURI_SIGNING_PRIVATE_KEY", "ANDROID_KEYSTORE_PASSWORD"]) {
            assert!(removed.iter().any(|r| r == name), "{name} reaches the child");
        }
        let line = shown(&command, &c.hidden());
        for secret in [ISSUER, KEY_ID, TEAM] {
            assert!(!line.contains(secret), "{line}");
        }
        assert!(line.starts_with("xcodebuild -exportArchive -archivePath /repo/platform/ios/build/RustRadar.xcarchive"), "{line}");
        assert_eq!(line.matches("***").count(), 3, "{line}");
        let upload = shown(&altool(Path::new("/o/RustRadar_0.3.16.ipa"), &c, None), &c.hidden());
        assert_eq!(upload, "xcrun altool --upload-app --type ios --file /o/RustRadar_0.3.16.ipa --apiKey *** --apiIssuer ***");
    }

    #[test]
    fn the_key_file_is_written_only_where_needed_and_removed() {
        let tree = Tree::new("key");
        let home = tree.0.join("home");
        fs::create_dir_all(&home).unwrap();
        let env = env_of(&[("HOME", home.to_str().unwrap())]);
        let standard = home.join(".appstoreconnect/private_keys/AuthKey_TESTKEYID0.p8");
        {
            let mut written = Materialised(Vec::new());
            let (path, dir) = key_file(&creds(), &env, &mut written).unwrap();
            assert_eq!((path.as_path(), dir), (standard.as_path(), None));
            assert_eq!(fs::read_to_string(&standard).unwrap(), "test key contents\n");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(fs::metadata(&standard).unwrap().permissions().mode() & 0o777, 0o600);
            }
        }
        assert!(!standard.exists(), "the key outlived the build");
        // one already there (today's workflow step) is used and left
        fs::create_dir_all(standard.parent().unwrap()).unwrap();
        fs::write(&standard, "theirs\n").unwrap();
        {
            let mut written = Materialised(Vec::new());
            assert_eq!(key_file(&creds(), &env, &mut written).unwrap().0, standard);
        }
        assert_eq!(fs::read_to_string(&standard).unwrap(), "theirs\n");
        fs::remove_file(&standard).unwrap();
        // a file named as altool wants it: used where it is, altool pointed at its folder
        let named = tree.0.join("runner/AuthKey_TESTKEYID0.p8");
        fs::create_dir_all(named.parent().unwrap()).unwrap();
        fs::write(&named, "k\n").unwrap();
        let file = |path: &Path| Credentials { key: Key::File(path.to_path_buf()), ..creds() };
        {
            let mut written = Materialised(Vec::new());
            let (path, dir) = key_file(&file(&named), &env, &mut written).unwrap();
            assert_eq!((path, dir), (named.clone(), Some(tree.0.join("runner"))));
            assert!(written.0.is_empty() && !standard.exists());
        }
        // any other name: copied to altool's place for this build
        let other = tree.0.join("runner/key.p8");
        fs::write(&other, "k2\n").unwrap();
        {
            let mut written = Materialised(Vec::new());
            assert_eq!(key_file(&file(&other), &env, &mut written).unwrap().0, standard);
            assert_eq!(fs::read_to_string(&standard).unwrap(), "k2\n");
        }
        assert!(!standard.exists() && other.exists());
        let err = key_file(&file(&tree.0.join("nowhere.p8")), &env, &mut Materialised(Vec::new())).unwrap_err();
        assert!(err.contains(KEY_PATH_VAR), "{err}");
    }

    #[test]
    fn the_archived_app_is_checked() {
        let tree = Tree::new("app");
        let app = tree.0.join("RustRadar.app");
        fs::create_dir_all(app.join("LaunchScreen.storyboardc")).unwrap();
        fs::write(app.join("RustRadar"), b"\xcf\xfa\xed\xfe").unwrap();
        fs::write(app.join("Assets.car"), b"car").unwrap();
        let good = [
            ("CFBundleIdentifier", "com.felix.rustradar\n"),
            ("CFBundleExecutable", "RustRadar"),
            ("CFBundleName", "RustRadar"),
            ("CFBundleShortVersionString", "0.3.16"),
            ("CFBundleVersion", "2026111306"),
            ("MinimumOSVersion", "15.0"),
            ("ITSAppUsesNonExemptEncryption", "false"),
            ("UILaunchStoryboardName", "LaunchScreen"),
        ];
        assert_eq!(check_app(&good, "arm64\n", &app, "0.3.16", "2026111306"), Ok(()));
        assert!(check_app(&good, "arm64", &app, "0.3.17", "2026111306").unwrap_err().contains("CFBundleShortVersionString"));
        assert!(check_app(&good, "arm64 x86_64", &app, "0.3.16", "2026111306").unwrap_err().contains("arm64 alone"));
        let mut other = good;
        other[0] = ("CFBundleIdentifier", "com.felix.other");
        assert!(check_app(&other, "arm64", &app, "0.3.16", "2026111306").unwrap_err().contains("com.felix.rustradar"));
        assert!(check_app(&good[1..], "arm64", &app, "0.3.16", "2026111306").unwrap_err().contains("nothing"));
        fs::remove_file(app.join("Assets.car")).unwrap();
        assert!(check_app(&good, "arm64", &app, "0.3.16", "2026111306").unwrap_err().contains("Assets.car"));
    }

    /// One entry of a zip archive, read back from its central directory and local header.
    struct Entry {
        name: String,
        made_by: u16,
        method: u16,
        mode: u32,
        data: Vec<u8>,
    }

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([b[at], b[at + 1]])
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    }

    fn read_zip(b: &[u8]) -> Vec<Entry> {
        let end = b.len() - 22;
        assert_eq!(u32_at(b, end), 0x0605_4b50, "no end record");
        let (count, cd_size, mut p) = (u16_at(b, end + 10) as usize, u32_at(b, end + 12) as usize, u32_at(b, end + 16) as usize);
        assert_eq!(p + cd_size, end);
        let mut entries = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(b, p), 0x0201_4b50);
            let (made_by, method, crc) = (u16_at(b, p + 4), u16_at(b, p + 10), u32_at(b, p + 16));
            let (packed, size) = (u32_at(b, p + 20) as usize, u32_at(b, p + 24) as usize);
            let (n, extra, comment) = (u16_at(b, p + 28) as usize, u16_at(b, p + 30) as usize, u16_at(b, p + 32) as usize);
            let (external, offset) = (u32_at(b, p + 38), u32_at(b, p + 42) as usize);
            let name = String::from_utf8(b[p + 46..p + 46 + n].to_vec()).unwrap();
            p += 46 + n + extra + comment;
            assert_eq!(u32_at(b, offset), 0x0403_4b50);
            assert_eq!((u16_at(b, offset + 8), u32_at(b, offset + 14)), (method, crc), "{name}: local header disagrees");
            let start = offset + 30 + u16_at(b, offset + 26) as usize + u16_at(b, offset + 28) as usize;
            let raw = &b[start..start + packed];
            let data = match method {
                0 => raw.to_vec(),
                8 => {
                    let mut out = Vec::new();
                    flate2::read::DeflateDecoder::new(raw).read_to_end(&mut out).unwrap();
                    out
                }
                other => panic!("{name}: method {other}"),
            };
            assert_eq!(data.len(), size, "{name}");
            let mut check = flate2::Crc::new();
            check.update(&data);
            assert_eq!(check.sum(), crc, "{name}: crc");
            entries.push(Entry { name, made_by, method, mode: external >> 16, data });
        }
        entries
    }

    #[test]
    fn the_ipa_is_payload_app_zipped_as_the_tauri_cli_does() {
        let tree = Tree::new("ipa");
        let app = tree.0.join("RustRadar.app");
        fs::create_dir_all(app.join("LaunchScreen.storyboardc")).unwrap();
        fs::create_dir_all(app.join("_CodeSignature")).unwrap();
        let binary: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(app.join("RustRadar"), &binary).unwrap();
        fs::write(app.join("Info.plist"), b"bplist00").unwrap();
        fs::write(app.join("LaunchScreen.storyboardc/Info.plist"), b"x").unwrap();
        fs::write(app.join("empty"), b"").unwrap();
        let ipa = tree.0.join("RustRadar_0.3.16_unsigned.ipa");
        assert_eq!(write_ipa(&app, &ipa).unwrap(), 8);
        let bytes = fs::read(&ipa).unwrap();
        assert!(bytes.len() < binary.len(), "not deflated");
        let entries = read_zip(&bytes);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Payload/",
                "Payload/RustRadar.app/",
                "Payload/RustRadar.app/Info.plist",
                "Payload/RustRadar.app/LaunchScreen.storyboardc/",
                "Payload/RustRadar.app/LaunchScreen.storyboardc/Info.plist",
                "Payload/RustRadar.app/RustRadar",
                "Payload/RustRadar.app/_CodeSignature/",
                "Payload/RustRadar.app/empty",
            ]
        );
        for e in &entries {
            let dir = e.name.ends_with('/');
            // 0755 for every entry, folders stored, files deflated, made on Unix
            assert_eq!(e.mode, if dir { 0o040_755 } else { 0o100_755 }, "{}", e.name);
            assert_eq!(e.method, if dir { 0 } else { 8 }, "{}", e.name);
            assert_eq!(e.made_by >> 8, 3, "{}", e.name);
        }
        assert_eq!(entries[5].data, binary);
        assert_eq!(entries[2].data, b"bplist00");
        assert!(entries[7].data.is_empty());
        // and as Info-ZIP reads it, where it is installed (the CI runners and Macs have it)
        if let Ok(out) = Command::new("unzip").arg("-tq").arg(&ipa).output() {
            assert!(out.status.success(), "unzip -t: {}", String::from_utf8_lossy(&out.stdout));
        }
    }

    #[test]
    fn a_dry_run_plans_and_runs_nothing() {
        let root = bump::repo_root();
        let files = bump::current().unwrap();
        let unsigned = Args { wanted: Wanted::Unsigned, dry_run: true, ..Args::default() };
        assert_eq!(release(&root, &unsigned, &env_of(&[])), Ok(None));
        let app_store = Args { wanted: Wanted::AppStore, upload: true, dry_run: true, ..Args::default() };
        assert_eq!(release(&root, &app_store, &release_env("schedule", "refs/heads/main")), Ok(None));
        assert!(release(&root, &app_store, &release_env("push", "refs/heads/rust-port")).unwrap_err().contains("rust-port"));
        // the release version must be the files'
        let other = Args { version: Some("9.9.9".to_owned()), ..unsigned };
        assert!(release(&root, &other, &env_of(&[])).unwrap_err().contains("cargo xtask bump 9.9.9"));
        let same = Args { version: Some(files), wanted: Wanted::Unsigned, dry_run: true, ..Args::default() };
        assert_eq!(release(&root, &same, &env_of(&[])), Ok(None));
        assert!(!root.join(PROJECT).join("build/ExportOptions.plist").exists());
    }

    #[test]
    fn the_command_line() {
        let a = |list: &[&str]| args(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&[]).unwrap(), Args::default());
        let parsed = a(&["--app-store", "--upload", "--build-number", "2026111306", "--version", "0.3.16", "--out", "dist"]).unwrap();
        assert_eq!(
            parsed,
            Args {
                wanted: Wanted::AppStore,
                upload: true,
                version: Some("0.3.16".to_owned()),
                build_number: Some("2026111306".to_owned()),
                out: Some(PathBuf::from("dist")),
                dry_run: false,
            }
        );
        assert_eq!(a(&["--unsigned", "--dry-run"]).unwrap().wanted, Wanted::Unsigned);
        assert!(a(&["--unsigned", "--app-store"]).is_err());
        assert!(a(&["--version"]).is_err());
        assert!(a(&["--sign"]).is_err());
    }

    /// The `<key>`/value pairs of a plist in XML, values as text (`true`/`false`, arrays joined by `,`).
    fn plist(path: &Path) -> HashMap<String, String> {
        let text = fs::read_to_string(path).unwrap();
        let pair = Regex::new(r"(?s)<key>([^<]+)</key>\s*(?:<string>([^<]*)</string>|<(true|false)/>|<array>(.*?)</array>)").unwrap();
        let item = Regex::new(r"<string>([^<]*)</string>").unwrap();
        pair.captures_iter(&text)
            .map(|c| {
                let value = match (c.get(2), c.get(3), c.get(4)) {
                    (Some(s), _, _) => s.as_str().to_owned(),
                    (_, Some(b), _) => b.as_str().to_owned(),
                    (_, _, Some(a)) => item.captures_iter(a.as_str()).map(|i| i[1].to_owned()).collect::<Vec<_>>().join(","),
                    _ => String::new(),
                };
                (c[1].to_owned(), value)
            })
            .collect()
    }

    #[test]
    fn the_project_has_todays_identity() {
        let spec = fs::read_to_string(repo("platform/ios/project.yml")).unwrap();
        let line = |l: &str| spec.lines().any(|s| s.trim() == l);
        for l in [
            "PRODUCT_BUNDLE_IDENTIFIER: com.felix.rustradar",
            "PRODUCT_NAME: RustRadar",
            "iOS: \"15.0\"",
            "deploymentTarget: \"15.0\"",
            "INFOPLIST_FILE: Info.plist",
            "ASSETCATALOG_COMPILER_APPICON_NAME: AppIcon",
            "ENABLE_BITCODE: NO",
            "CODE_SIGN_STYLE: Automatic",
            "- path: LaunchScreen.storyboard",
            "path: RustRadar.entitlements",
            "RustRadar:",
            "script: '\"$SRCROOT/build-rust.sh\"'",
        ] {
            assert!(line(l), "project.yml has no `{l}`");
        }
        assert!(spec.contains(&format!("iOS: \"{MINIMUM_IOS}\"")));
        // today's, read from the Tauri config while it exists (stage 12 deletes it)
        let conf = repo("src-tauri/tauri.conf.json");
        if conf.exists() {
            let conf: serde_json::Value = serde_json::from_str(&fs::read_to_string(conf).unwrap()).unwrap();
            assert_eq!(conf["identifier"], BUNDLE_ID);
            assert_eq!(conf["productName"], PRODUCT);
            // no bundle.iOS.minimumSystemVersion: the Tauri CLI's default, 15.0 (tauri-utils)
            assert!(conf["bundle"]["iOS"]["minimumSystemVersion"].is_null());
            // no bundle.iOS.bundleVersion: the build number is the version
            assert!(conf["bundle"]["iOS"]["bundleVersion"].is_null());
        }
        let ignore = fs::read_to_string(repo("platform/ios/.gitignore")).unwrap();
        // the build folder holds the archive, the export and, for a moment, the export options with the team
        assert!(ignore.lines().any(|l| l == "/build") && ignore.lines().any(|l| l == "/RustRadar.xcodeproj"));
    }

    #[test]
    fn the_info_plist_is_todays() {
        let info = plist(&repo("platform/ios/Info.plist"));
        let get = |k: &str| info.get(k).map(String::as_str).unwrap_or_else(|| panic!("Info.plist has no {k}"));
        assert_eq!(get("CFBundleIdentifier"), "$(PRODUCT_BUNDLE_IDENTIFIER)");
        assert_eq!(get("CFBundleExecutable"), "$(EXECUTABLE_NAME)");
        assert_eq!(get("CFBundleName"), "$(PRODUCT_NAME)");
        assert_eq!(get("CFBundleDisplayName"), PRODUCT);
        assert_eq!(get("CFBundlePackageType"), "APPL");
        assert_eq!(get("CFBundleShortVersionString"), "$(MARKETING_VERSION)");
        assert_eq!(get("CFBundleVersion"), "$(CURRENT_PROJECT_VERSION)");
        // the Tauri CLI template's properties
        assert_eq!(get("LSRequiresIPhoneOS"), "true");
        assert_eq!(get("UILaunchStoryboardName"), "LaunchScreen");
        assert_eq!(get("UIRequiredDeviceCapabilities"), "arm64,metal");
        assert_eq!(
            get("UISupportedInterfaceOrientations"),
            "UIInterfaceOrientationPortrait,UIInterfaceOrientationLandscapeLeft,UIInterfaceOrientationLandscapeRight"
        );
        assert_eq!(
            get("UISupportedInterfaceOrientations~ipad"),
            "UIInterfaceOrientationPortrait,UIInterfaceOrientationPortraitUpsideDown,UIInterfaceOrientationLandscapeLeft,\
             UIInterfaceOrientationLandscapeRight"
        );
        assert!(!info.contains_key("UILaunchScreen"));
        // src-tauri/Info.ios.plist, merged in today
        assert_eq!(get("ITSAppUsesNonExemptEncryption"), "false");
        let merged = repo("src-tauri/Info.ios.plist");
        if merged.exists() {
            for (key, value) in plist(&merged) {
                assert_eq!(info.get(&key), Some(&value), "{key} differs from src-tauri/Info.ios.plist");
            }
        }
    }

    /// A PNG's width, height, bit depth and colour type, from its IHDR.
    fn png_header(bytes: &[u8]) -> (u32, u32, u8, u8) {
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&bytes[12..16], b"IHDR");
        let be = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        (be(16), be(20), bytes[24], bytes[25])
    }

    #[test]
    fn the_icons_are_todays() {
        let dir = repo("platform/ios/Assets.xcassets/AppIcon.appiconset");
        let contents: serde_json::Value = serde_json::from_str(&fs::read_to_string(dir.join("Contents.json")).unwrap()).unwrap();
        let listed: Vec<(String, String, String, String)> = contents["images"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                let s = |k: &str| i[k].as_str().unwrap_or_default().to_owned();
                (s("size"), s("idiom"), s("filename"), s("scale"))
            })
            .collect();
        let today: Vec<(String, String, String, String)> =
            ICONS.iter().map(|(a, b, c, d)| (a.to_string(), b.to_string(), c.to_string(), d.to_string())).collect();
        assert_eq!(listed, today);
        let mut pngs: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".png"))
            .collect();
        pngs.sort();
        let mut wanted: Vec<String> = ICONS.iter().map(|i| i.2.to_owned()).collect();
        wanted.sort();
        wanted.dedup();
        assert_eq!(pngs, wanted, "the icon set's files");
        let tauri = repo("src-tauri/icons/ios");
        for (size, idiom, file, scale) in ICONS {
            let bytes = fs::read(dir.join(file)).unwrap();
            let points: f64 = size.split('x').next().unwrap().parse().unwrap();
            let factor: f64 = scale.trim_end_matches('x').parse().unwrap();
            let pixels = (points * factor) as u32;
            // RGB, 8 bits: App Store Connect refuses an icon with an alpha channel
            assert_eq!(png_header(&bytes), (pixels, pixels, 8, 2), "{file} ({idiom} {size} @{scale})");
            if tauri.exists() {
                assert_eq!(bytes, fs::read(tauri.join(file)).unwrap(), "{file} is not today's");
            }
        }
    }

    #[test]
    fn the_launch_screen_and_entitlements_are_todays() {
        let board = fs::read_to_string(repo("platform/ios/LaunchScreen.storyboard")).unwrap();
        // the Tauri CLI 2.12.1 template's storyboard: one view controller, the system background
        for part in [
            "type=\"com.apple.InterfaceBuilder3.CocoaTouch.Storyboard.XIB\"",
            "initialViewController=\"Y6W-OH-hqX\"",
            "<viewController id=\"Y6W-OH-hqX\" sceneMemberID=\"viewController\">",
            "<color key=\"backgroundColor\" systemColor=\"systemBackgroundColor\"/>",
        ] {
            assert!(board.contains(part), "LaunchScreen.storyboard has no {part}");
        }
        let entitlements = fs::read_to_string(repo("platform/ios/RustRadar.entitlements")).unwrap();
        assert!(entitlements.contains("<dict/>") && !entitlements.contains("<key>"), "today's entitlements are empty");
    }

    #[test]
    fn the_release_library_is_built_for_devices_from_the_lock() {
        let script = fs::read_to_string(repo("platform/ios/build-rust.sh")).unwrap();
        assert!(script.contains(&format!("iphoneos) target={RUST_TARGET} ;;")));
        assert!(script.contains("flags=(--release --locked)"));
        assert!(script.contains("IPHONEOS_DEPLOYMENT_TARGET=\"${IPHONEOS_DEPLOYMENT_TARGET:-15.0}\""));
        assert!(script.contains("cargo build -p rustradar-app --lib"));
    }
}
