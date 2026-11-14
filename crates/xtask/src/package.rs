//! `cargo xtask package <format>...`: the desktop packages (plan stage 11, rows P5–P7; doc 07 §13).
//!
//! ```text
//! cargo xtask package <deb|rpm|appimage|nsis|msi|app|dmg>... [--target TRIPLE] [--binary FILE] [--out DIR] [--sign]
//! ```
//!
//! Builds the app's release binary (`cargo build --release --locked` of the package `crates/app` is,
//! `rustradar-app` until item D2 names it `rustradar`; `--binary` packs one built before instead), then
//! for each format: copies it as `rustradar` (today's binary name), writes the format's installer kind
//! into the copy (below), packs the copy with the format's tool, and moves the package to `--out`
//! (default `<target dir>/package`) under the name today's release gives it. `--sign` then signs each
//! package as `cargo xtask sign` does, with `TAURI_SIGNING_PRIVATE_KEY` and its password: today's
//! release has a `.sig` beside each Linux package, and its `latest.json` points `linux-x86_64` and
//! `linux-x86_64-appimage` at the AppImage, `linux-x86_64-deb` and `linux-x86_64-rpm` at the others
//! (`cargo xtask latest-json` reads these names back).
//!
//! | format | tool and configuration | package (v0.3.16's asset) | installer kind |
//! |---|---|---|---|
//! | `deb` | cargo-packager 0.11.8, `Packager.toml` | `RustRadar_0.3.16_amd64.deb` | `__TAURI_BUNDLE_TYPE_VAR_DEB` |
//! | `rpm` | cargo-generate-rpm 0.21.1, `platform/linux/rpm.toml` | `RustRadar-0.3.16-1.x86_64.rpm` | `__TAURI_BUNDLE_TYPE_VAR_RPM` |
//! | `appimage` | cargo-packager 0.11.8 (linuxdeploy), `Packager.toml` | `RustRadar_0.3.16_amd64.AppImage` | `__TAURI_BUNDLE_TYPE_VAR_APP` |
//!
//! **The tools.** cargo-packager 0.11.8, the plan's (section 1.3), packs deb, AppImage, pacman, NSIS,
//! WiX, `.app` and dmg (its `PackageFormat`), not rpm; the rpm is cargo-generate-rpm's, which packs with
//! the `rpm` crate as Tauri's bundler does. Both must be installed at those versions
//! (`cargo install --locked cargo-packager@0.11.8 cargo-generate-rpm@0.21.1`; checked before anything is
//! built). The AppImage is linuxdeploy's: cargo-packager downloads it, its AppImage plugin and
//! AppImageKit's AppRun from GitHub into `~/.cache/.cargo-packager` on first use, and this sets
//! `APPIMAGE_EXTRACT_AND_RUN` so that they run without FUSE. The libraries the AppImage carries
//! (`[appimage] libs`) are copied from the build machine's `/usr/lib*`, which cargo-packager skips
//! without a word when one is missing, so they are looked for first ([`missing_libs`]).
//!
//! **The installer kind.** Tauri's bundler writes it into the binary of each package, and the updater
//! reads it back (`update::manifest::Installer::current` in the app; gap 6 of the plan's stages 8-10
//! status): the 27 bytes `__TAURI_BUNDLE_TYPE_VAR_UNK` are overwritten in place with the format's
//! marker ([`Format::marker`]), as `patch_binary` in tauri-bundler 2.10.1 (the bundler of today's Tauri
//! CLI, 2.12.1) does, and as `tauri-utils` 2.10.1's `bundle_type` and tauri-plugin-updater 2.13.2's
//! `installer_for_bundle_type` read it: `_DEB`, `_RPM`, `_APP` for the AppImage (and `_MSI`, `_NSS` for
//! Windows, item P5; the bundler leaves macOS's `.app` and dmg unpatched, an unknown marker meaning the
//! `.app` there). Deviation: the bundler patches the first match; here a binary with no marker, or with
//! more than one, is refused ([`patch_marker`]), so that a stray copy of the text cannot take the patch.
//!
//! **Identity** (doc 07 §13.3, §13.6), as v0.3.16's packages have it: package `rust-radar` (the product
//! name in kebab case) at the release version, release 1 for the rpm, Maintainer `you`, the Cargo
//! description, `/usr/bin/rustradar`, `/usr/share/applications/RustRadar.desktop` (today's file, kept in
//! `platform/linux`), the icons of `src-tauri/icons` at `hicolor/32x32`, `128x128` and `256x256@2`.
//! What changes: the dependencies (WebKitGTK and GTK give way to what winit and wgpu open at run time,
//! listed in `Packager.toml` and `rpm.toml`, and the deb's `libc6` at the newest glibc symbol version
//! the binary asks for, [`glibc_needed`]; the rpm finds what the binary links itself), the AppImage
//! carries xkbcommon and its X11 part instead of GTK, and its desktop file inside it is
//! `rustradar.desktop` (`Packager.toml` says why).
//!
//! **Windows** (item P5; doc 07 §7.2, §12.1, §13.1, §13.6): `nsis` and `msi`, both cargo-packager's,
//! from tauri-bundler 2.10.1's own templates kept in `platform/windows` (`installer.nsi`, `main.wxs`; the
//! heads of those files say what changed), so that each upgrades today's install in place and the old
//! Tauri updater can run it (`/S /UPDATE /R /ARGS`, `AUTOLAUNCHAPP`, `LAUNCHAPPARGS`):
//!
//! | format | packer | package (v0.3.16's asset) | installer kind |
//! |---|---|---|---|
//! | `nsis` | cargo-packager's NSIS, `platform/windows/installer.nsi` | `RustRadar_0.3.16_x64-setup.exe` | `__TAURI_BUNDLE_TYPE_VAR_NSS` |
//! | `msi` | cargo-packager's WiX 3, `platform/windows/main.wxs` | `RustRadar_0.3.16_x64_en-US.msi` | `__TAURI_BUNDLE_TYPE_VAR_MSI` |
//!
//! The binary is staged as `rustradar.exe` (today's `$INSTDIR\rustradar.exe`), and must carry the
//! resources `crates/app/build.rs` embeds ([`check_windows_resources`]). Before cargo-packager renders a
//! template, [`nsis_template`] and [`wix_template`] fill in what its data lacks or derives otherwise:
//! tauri-bundler's `utils.nsh`, the folder of nsis_tauri_utils 0.5.3 (today's installer's plugin, with
//! `RunAsUser` and the `*CurrentUser` functions the template calls; cargo-packager brings 0.2.1, which
//! has neither: [`nsis_plugin`] fetches tauri-bundler's download once and checks its SHA-1), the
//! estimated size as tauri-bundler computes it, and the MSI's UpgradeCode as tauri-bundler derives it
//! ([`upgrade_code`]: today's `{C73F46FF-44DC-5F3E-AB93-388B09B92DEC}`). The MSI is made on Windows only
//! (WiX 3); the NSIS installer also elsewhere, with `makensis` on the path.
//!
//! **macOS** (item P6; doc 07 §7.2, §12.1, §13.2, §13.6): `app` and `dmg`, one per architecture
//! (`--target aarch64-apple-darwin` and `x86_64-apple-darwin`, as today's release ships them, not
//! universal), the release built for the deployment target `Packager.toml`'s
//! `[macos] minimum-system-version` names (`MACOSX_DEPLOYMENT_TARGET`, as the Tauri CLI set it):
//!
//! | format | made by | package (v0.3.16's asset) | installer kind |
//! |---|---|---|---|
//! | `app` | cargo-packager's `.app`, sealed ad hoc, archived as tauri-bundler archives it | `RustRadar_0.3.16_aarch64.app.tar.gz`, `…_x64.app.tar.gz` | left `…_UNK` |
//! | `dmg` | the same `.app` in tauri-bundler 2.10.1's disk image (`platform/macos/bundle_dmg.sh`) | `RustRadar_0.3.16_aarch64.dmg`, `…_x64.dmg` | left `…_UNK` |
//!
//! The bundle is today's: `RustRadar.app/Contents/{Info.plist, MacOS/rustradar, Resources/icon.icns}`,
//! its `Info.plist` `platform/macos/Info.plist` at the release version ([`info_plist`]; the file's head
//! says which keys are today's and which are new), its icon `src-tauri/icons/icon.icns`; the binary
//! must be a Mach-O for the architecture named ([`check_macos_binary`]). The installer kind is left as
//! it is: tauri-bundler's `patch_binary` skips the `.app` and the dmg, and an unknown kind is the `.app`
//! on macOS (tauri-utils' `bundle_type`, the app's `Installer::from_marker`), where `_APP` would say
//! AppImage. On macOS the bundle is then sealed ad hoc ([`sign_app`]: `codesign --force --deep --sign -`,
//! verified with `--strict`, so that the bundle's seal covers its `Info.plist` and icon, doc 07 §13.2;
//! deviation: today's bundle carries only the linker's ad hoc signature on its executable and no
//! `_CodeSignature`); elsewhere it is left as cargo-packager made it, with a warning.
//! [`check_app_bundle`] checks the layout and the `Info.plist` before and after.
//!
//! The updater's archive is `RustRadar.app` in a `.tar.gz` as tauri-bundler 2.10.1's `create_tar` makes
//! it ([`app_archive`]: the tar crate's `append_dir_all` under the bundle's own name, symlinks kept,
//! flate2's default gzip), which the old Tauri updater and the app's (P3) unpack with the top folder
//! dropped. With `--sign` it is signed as `RustRadar.app.tar.gz`, the name tauri-bundler gave it and
//! v0.3.16's `.sig` files name in their trusted comment, then moved to its asset name with its `.sig`.
//! The disk image is made on macOS only (`hdiutil`, and the Finder through `osascript` for its layout):
//! tauri-bundler's script and AppleScript, byte for byte, with tauri-bundler's arguments for today's
//! configuration ([`dmg_args`]: `Packager.toml`'s `[dmg]` layout, the volume named after the product
//! with the app's icon), without `--skip-jenkins`, which tauri-bundler adds when `CI=true` unless
//! `TAURI_BUNDLER_DMG_IGNORE_CI=true`, which tauri-action sets: today's CI ran the Finder's layout on
//! GitHub's macOS runner, and v0.3.16's dmg has it. It gets no `.sig`, as today. cargo-packager's own
//! dmg is not used: it makes its own unsealed `.app` and passes `--skip-jenkins` on CI.

use crate::bump;
use crate::sign;
use regex::Regex;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The installer kind of a binary no package has claimed (`tauri-utils`' initial value).
pub const MARKER: &[u8; 27] = b"__TAURI_BUNDLE_TYPE_VAR_UNK";

/// The binary's name in every package (today's: Tauri's bundler named it after the Cargo package).
pub const BINARY: &str = "rustradar";

/// The packers, at the versions this is written against.
pub const PACKAGER: (&str, &str) = ("cargo-packager", "0.11.8");
pub const GENERATE_RPM: (&str, &str) = ("cargo-generate-rpm", "0.21.1");

/// The configuration files, relative to the repository root.
pub const PACKAGER_TOML: &str = "Packager.toml";
pub const RPM_TOML: &str = "platform/linux/rpm.toml";
pub const NSIS_TEMPLATE: &str = "platform/windows/installer.nsi";
pub const NSIS_UTILS: &str = "platform/windows/utils.nsh";
pub const WIX_TEMPLATE: &str = "platform/windows/main.wxs";
pub const INFO_PLIST: &str = "platform/macos/Info.plist";
pub const DMG_SCRIPT: &str = "platform/macos/bundle_dmg.sh";
pub const DMG_APPLESCRIPT: &str = "platform/macos/template.applescript";

/// The Windows executable's resources, which `crates/app/build.rs` writes (shared with it).
#[allow(dead_code)]
#[path = "../../../platform/windows/resources.rs"]
mod windows_resources;

/// A package this makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Deb,
    Rpm,
    AppImage,
    Nsis,
    Msi,
    App,
    Dmg,
}

impl Format {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const LINUX: [Format; 3] = [Format::Deb, Format::Rpm, Format::AppImage];
    pub const WINDOWS: [Format; 2] = [Format::Nsis, Format::Msi];
    pub const MACOS: [Format; 2] = [Format::App, Format::Dmg];
    pub const ALL: [Format; 7] = [Format::Deb, Format::Rpm, Format::AppImage, Format::Nsis, Format::Msi, Format::App, Format::Dmg];

    /// Its name on the command line (cargo-packager's, for those it packs, but `msi` for its `wix`).
    pub fn name(self) -> &'static str {
        match self {
            Format::Deb => "deb",
            Format::Rpm => "rpm",
            Format::AppImage => "appimage",
            Format::Nsis => "nsis",
            Format::Msi => "msi",
            Format::App => "app",
            Format::Dmg => "dmg",
        }
    }

    /// Its name in cargo-packager's `formats`: `app` for the dmg too, which is made from
    /// cargo-packager's `.app` with tauri-bundler's script (the module's docs say why).
    pub fn packager_name(self) -> &'static str {
        match self {
            Format::Msi => "wix",
            Format::Dmg => "app",
            other => other.name(),
        }
    }

    pub fn parse(name: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.name() == name)
    }

    /// Whether this is a Windows installer (the binary staged as `rustradar.exe`, with resources).
    pub fn windows(self) -> bool {
        Format::WINDOWS.contains(&self)
    }

    /// Whether this is made from the macOS `.app`.
    pub fn macos(self) -> bool {
        Format::MACOS.contains(&self)
    }

    /// What tauri-bundler's `patch_binary` writes over [`MARKER`] for this package: [`MARKER`] itself
    /// for the macOS ones, which it leaves as they are (the unknown kind is the `.app` on macOS; `_APP`
    /// is the AppImage).
    pub fn marker(self) -> &'static [u8; 27] {
        match self {
            Format::Deb => b"__TAURI_BUNDLE_TYPE_VAR_DEB",
            Format::Rpm => b"__TAURI_BUNDLE_TYPE_VAR_RPM",
            Format::AppImage => b"__TAURI_BUNDLE_TYPE_VAR_APP",
            Format::Nsis => b"__TAURI_BUNDLE_TYPE_VAR_NSS",
            Format::Msi => b"__TAURI_BUNDLE_TYPE_VAR_MSI",
            Format::App | Format::Dmg => MARKER,
        }
    }

    /// The ending of the file the packer writes.
    fn extension(self) -> &'static str {
        match self {
            Format::Deb => ".deb",
            Format::Rpm => ".rpm",
            Format::AppImage => ".AppImage",
            Format::Nsis => "-setup.exe",
            Format::Msi => ".msi",
            Format::App => ".app.tar.gz",
            Format::Dmg => ".dmg",
        }
    }

    /// The binary's name in the package: today's `rustradar`, `rustradar.exe` on Windows.
    pub fn binary_name(self) -> String {
        if self.windows() { format!("{BINARY}.exe") } else { BINARY.to_owned() }
    }

    /// The package's name as today's release names it (tauri-bundler 2.10.1: the deb and the AppImage
    /// `{product}_{version}_{arch}` with Debian's architecture names, the rpm
    /// `{product}-{version}-{release}.{arch}` with RPM's, the NSIS installer
    /// `{product}_{version}_{arch}-setup.exe` and the MSI `{product}_{version}_{arch}_{language}.msi` with
    /// Windows' names; the `.app.tar.gz` and the dmg `{product}_{version}_{arch}` with `x64` for x86_64,
    /// the dmg's name and the one tauri-action gave the archive), for `arch` as Rust names it.
    pub fn asset_name(self, version: &str, arch: &str) -> Result<String, String> {
        if self.macos() {
            let arch = match arch {
                "x86_64" => "x64",
                "aarch64" => "aarch64",
                "universal" => "universal",
                other => return Err(format!("no macOS package is made for {other} (aarch64 and x86_64 are shipped, universal named as Tauri names it)")),
            };
            return Ok(match self {
                Format::App => format!("RustRadar_{version}_{arch}.app.tar.gz"),
                _ => format!("RustRadar_{version}_{arch}.dmg"),
            });
        }
        if self.windows() {
            let arch = match arch {
                "x86_64" => "x64",
                "aarch64" => "arm64",
                "x86" | "i686" => "x86",
                other => return Err(format!("no Windows installer is made for {other} (x86_64 is shipped, the others named as Tauri names them)")),
            };
            return Ok(match self {
                Format::Nsis => format!("RustRadar_{version}_{arch}-setup.exe"),
                _ => format!("RustRadar_{version}_{arch}_en-US.msi"),
            });
        }
        let (deb, rpm, appimage) = match arch {
            "x86_64" => ("amd64", "x86_64", "amd64"),
            "aarch64" => ("arm64", "aarch64", "aarch64"),
            other => return Err(format!("no Linux package is made for {other} (x86_64 is shipped, aarch64 named as Tauri names it)")),
        };
        Ok(match self {
            Format::Deb => format!("RustRadar_{version}_{deb}.deb"),
            Format::Rpm => format!("RustRadar-{version}-1.{rpm}.rpm"),
            _ => format!("RustRadar_{version}_{appimage}.AppImage"),
        })
    }
}

/// Overwrites the one [`MARKER`] in `binary` with `kind`, in place: the binary keeps its length.
pub fn patch_marker(binary: &mut [u8], kind: &[u8; 27]) -> Result<(), String> {
    let found: Vec<usize> = find_all(binary, MARKER).collect();
    match found[..] {
        [at] => {
            binary[at..at + MARKER.len()].copy_from_slice(kind);
            Ok(())
        }
        [] => Err("the binary holds no __TAURI_BUNDLE_TYPE_VAR_UNK: it is not the app (update::manifest's marker), or it was patched already".to_owned()),
        _ => Err(format!("the binary holds __TAURI_BUNDLE_TYPE_VAR_UNK {} times: which one the updater reads is unknown", found.len())),
    }
}

/// Where `needle` starts in `haystack`, every time.
fn find_all<'a>(haystack: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    let first = needle[0];
    haystack.iter().enumerate().filter(move |&(i, &b)| b == first && haystack[i..].starts_with(needle)).map(|(i, _)| i)
}

/// The newest glibc symbol version `binary` asks for (`GLIBC_2.34` → `[2, 34]`), as dpkg-shlibdeps
/// would turn it into `libc6 (>= 2.34)`: read off the version names in its dynamic string table.
pub fn glibc_needed(binary: &[u8]) -> Option<Vec<u32>> {
    const PREFIX: &[u8] = b"GLIBC_";
    let mut newest: Option<Vec<u32>> = None;
    for at in find_all(binary, PREFIX) {
        let rest = &binary[at + PREFIX.len()..];
        let len = rest.iter().take_while(|b| b.is_ascii_digit() || **b == b'.').count();
        let text = std::str::from_utf8(&rest[..len]).unwrap_or_default();
        let parts: Option<Vec<u32>> = text.split('.').map(|p| p.parse().ok()).collect();
        // a version name ends there (a NUL in .dynstr); `GLIBC_TUNABLES` and the like are no version
        let ends = rest.get(len).is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');
        if let Some(parts) = parts.filter(|p| p.len() >= 2 && ends)
            && newest.as_ref().is_none_or(|n| parts > *n)
        {
            newest = Some(parts);
        }
    }
    newest
}

/// cargo-packager's JSON configuration for one package: `Packager.toml` (its text) with this build's
/// version, format, binary folder and output folder, and `depends` put in front of the deb's.
pub fn packager_config(text: &str, format: Format, version: &str, binaries_dir: &Path, out_dir: &Path, depends: &[String]) -> Result<Value, String> {
    let mut config: Value = toml::from_str(text).map_err(|e| format!("{PACKAGER_TOML}: {e}"))?;
    let object = config.as_object_mut().ok_or_else(|| format!("{PACKAGER_TOML} is not a table"))?;
    for key in ["version", "formats", "out-dir", "outDir", "out_dir", "binaries-dir", "binariesDir", "binaries_dir"] {
        if object.contains_key(key) {
            return Err(format!("{PACKAGER_TOML} sets `{key}`, which `cargo xtask package` decides"));
        }
    }
    object.insert("version".into(), json!(version));
    object.insert("formats".into(), json!([format.packager_name()]));
    object.insert("binariesDir".into(), json!(binaries_dir));
    object.insert("outDir".into(), json!(out_dir));
    if format == Format::Deb && !depends.is_empty() {
        let deb = object.entry("deb").or_insert_with(|| json!({}));
        let listed = deb.get("depends").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut all: Vec<Value> = depends.iter().map(|d| json!(d)).collect();
        all.extend(listed);
        deb["depends"] = Value::Array(all);
    }
    Ok(config)
}

/// The `Cargo.toml` cargo-generate-rpm packs the rpm from (it reads its options from a manifest's
/// `[package.metadata.generate-rpm]` only, and the app's manifest is not this item's): `rpm.toml` (its
/// text) as those options, with this build's version, the binary at `/usr/bin/rustradar` in front of
/// the assets, and every asset's source made absolute under `root`.
pub fn rpm_manifest(text: &str, version: &str, binary: &Path, root: &Path) -> Result<String, String> {
    use toml::Value as V;
    let mut options: toml::Table = toml::from_str(text).map_err(|e| format!("{RPM_TOML}: {e}"))?;
    if options.contains_key("version") {
        return Err(format!("{RPM_TOML} sets `version`, which `cargo xtask package` decides"));
    }
    options.insert("version".into(), V::String(version.to_owned()));
    let path = |p: &Path| p.to_str().map(str::to_owned).ok_or_else(|| format!("{}: not UTF-8", p.display()));
    let mut entry = toml::Table::new();
    entry.insert("source".into(), V::String(path(binary)?));
    entry.insert("dest".into(), V::String(format!("/usr/bin/{BINARY}")));
    entry.insert("mode".into(), V::String("755".to_owned()));
    let assets = options.entry("assets").or_insert_with(|| V::Array(Vec::new()));
    let V::Array(assets) = assets else { return Err(format!("{RPM_TOML}: `assets` is not a list")) };
    for asset in assets.iter_mut() {
        if let Some(V::String(source)) = asset.get_mut("source")
            && Path::new(source.as_str()).is_relative()
        {
            *source = path(&root.join(source.as_str()))?;
        }
    }
    assets.insert(0, V::Table(entry));
    let name = options.get("name").cloned().ok_or_else(|| format!("{RPM_TOML} names no package (`name`)"))?;
    let mut metadata = toml::Table::new();
    metadata.insert("generate-rpm".into(), V::Table(options));
    let mut package = toml::Table::new();
    package.insert("name".into(), name);
    package.insert("version".into(), V::String(version.to_owned()));
    package.insert("metadata".into(), V::Table(metadata));
    let mut manifest = toml::Table::new();
    manifest.insert("package".into(), V::Table(package));
    toml::to_string(&manifest).map_err(|e| format!("{RPM_TOML}: {e}"))
}

/// The libraries of `[appimage] libs` that no `/usr/lib*` folder of this machine has, for `arch`.
pub fn missing_libs(config: &Value, arch: &str) -> Vec<String> {
    let libs = config.pointer("/appimage/libs").and_then(Value::as_array).cloned().unwrap_or_default();
    let dirs = [format!("/usr/lib/{arch}-linux-gnu"), "/usr/lib64".to_owned(), "/usr/lib".to_owned()];
    libs.iter()
        .filter_map(Value::as_str)
        .filter(|lib| !dirs.iter().any(|dir| Path::new(dir).join(lib).exists()))
        .map(str::to_owned)
        .collect()
}

/// The nsis_tauri_utils plugin today's NSIS installer carries, as tauri-bundler 2.10.1 downloads it.
pub const NSIS_TAURI_UTILS: (&str, &str) = (
    "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll",
    "75197fee3c6a814fe035788d1c34ead39349b860",
);

/// The SHA-1 of `bytes` in lower-case hex (tauri-bundler's checksum for its NSIS downloads).
pub fn sha1_hex(bytes: &[u8]) -> String {
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY, bytes);
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The MSI's UpgradeCode as tauri-bundler 2.10.1 derives it when `bundle.windows.wix.upgradeCode` is
/// not set (today's): `Uuid::new_v5(&Uuid::NAMESPACE_DNS, "{productName}.exe.app.x64")`, written as
/// uuid writes it (`x64` whatever the architecture). cargo-packager derives another one, from the
/// binary's name, which would install beside today's MSI instead of upgrading it.
pub fn upgrade_code(product_name: &str) -> String {
    // RFC 4122's DNS namespace, 6ba7b810-9dad-11d1-80b4-00c04fd430c8
    const DNS: [u8; 16] = [0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8];
    let name = format!("{product_name}.exe.app.x64");
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY, &[&DNS[..], name.as_bytes()].concat());
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest.as_ref()[..16]);
    id[6] = (id[6] & 0x0f) | 0x50; // version 5
    id[8] = (id[8] & 0x3f) | 0x80; // RFC 4122 variant
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// A path as the templates name it: refused if NSIS or WiX would read part of it as markup.
fn template_path(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or_else(|| format!("{}: not UTF-8", path.display()))?;
    if text.contains(['$', '"', '{', '}', '<', '>', '&']) {
        return Err(format!("{text}: a path the installer templates cannot name (no $ \" {{ }} < > &)"));
    }
    Ok(text.to_owned())
}

/// Fills in what cargo-packager's data lacks or derives otherwise in `platform/windows/installer.nsi`
/// (its text): tauri-bundler's `utils.nsh` (`utils`), the folder holding nsis_tauri_utils 0.5.3
/// (`plugins`), and the estimated size as tauri-bundler computes it (the main binary's bytes / 1024).
pub fn nsis_template(text: &str, utils: &Path, plugins: &Path, binary_len: u64) -> Result<String, String> {
    let filled = text
        .replace("@@UTILS_NSH@@", &template_path(utils)?)
        .replace("@@PLUGINS_DIR@@", &template_path(plugins)?)
        .replace("@@ESTIMATED_SIZE@@", &(binary_len / 1024).to_string());
    if filled.contains("@@") {
        return Err(format!("{NSIS_TEMPLATE} names a value `cargo xtask package` does not fill in"));
    }
    Ok(filled)
}

/// Fills in `platform/windows/main.wxs`'s UpgradeCode, tauri-bundler's for `product_name`.
pub fn wix_template(text: &str, product_name: &str) -> Result<String, String> {
    let filled = text.replace("@@UPGRADE_CODE@@", &upgrade_code(product_name));
    if filled.contains("@@") {
        return Err(format!("{WIX_TEMPLATE} names a value `cargo xtask package` does not fill in"));
    }
    Ok(filled)
}

/// The DLLs a Windows executable (PE32+) imports, as its import directory names them.
pub fn windows_imports(binary: &[u8]) -> Result<Vec<String>, String> {
    let bad = || "not a 64-bit Windows executable (PE32+)".to_owned();
    let u16_at = |at: usize| binary.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(bad);
    let u32_at = |at: usize| binary.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(bad);
    if !binary.starts_with(b"MZ") {
        return Err(bad());
    }
    let pe = u32_at(0x3c)? as usize;
    if binary.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(bad());
    }
    let sections = u16_at(pe + 6)? as usize;
    let optional = pe + 24;
    let optional_len = u16_at(pe + 20)? as usize;
    if u16_at(optional)? != 0x20b {
        return Err(bad());
    }
    // data directory 1 (imports) of PE32+'s optional header
    let imports = u32_at(optional + 112 + 8)? as usize;
    // a virtual address to its place in the file, through the section table
    let table = optional + optional_len;
    let file_at = |rva: usize| -> Result<usize, String> {
        for i in 0..sections {
            let header = table + i * 40;
            let (size, address, raw) = (u32_at(header + 8)? as usize, u32_at(header + 12)? as usize, u32_at(header + 20)? as usize);
            if (address..address + size.max(1)).contains(&rva) {
                return Ok(raw + rva - address);
            }
        }
        Err(format!("address {rva:#x} is in no section"))
    };
    let mut names = Vec::new();
    if imports == 0 {
        return Ok(names);
    }
    let mut descriptor = file_at(imports)?;
    loop {
        let name = u32_at(descriptor + 12)? as usize;
        if name == 0 {
            break;
        }
        let at = file_at(name)?;
        let len = binary[at..].iter().position(|b| *b == 0).ok_or_else(bad)?;
        names.push(String::from_utf8_lossy(&binary[at..at + len]).into_owned());
        descriptor += 20;
    }
    Ok(names)
}

/// Refuses a Windows binary that needs the Visual C++ Redistributable: today's `rustradar.exe` imports
/// neither `vcruntime140.dll` nor `msvcp140.dll` (tauri-build linked the Visual C++ runtime statically),
/// only Windows' own DLLs and the Universal CRT, so it starts on a Windows without the redistributable.
pub fn check_windows_imports(binary: &[u8]) -> Result<(), String> {
    let redistributable: Vec<String> = windows_imports(binary)?
        .into_iter()
        .filter(|dll| ["vcruntime", "msvcp", "concrt", "vccorlib"].iter().any(|p| dll.to_ascii_lowercase().starts_with(p)))
        .collect();
    if redistributable.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the executable imports {}, which come with the Visual C++ Redistributable and not with Windows (today's imports none): \
             build it with RUSTFLAGS=\"-C target-feature=+crt-static\" so that the C and C++ dependencies link their runtime statically too \
             (crates/app/build.rs links the Visual C++ runtime statically, as tauri-build did)",
            redistributable.join(", ")
        ))
    }
}

/// Refuses a Windows binary without the resources `crates/app/build.rs` embeds (a cross build without
/// a resource compiler leaves them out): the manifest as written, and the version information at
/// `version`.
pub fn check_windows_resources(binary: &[u8], version: &str) -> Result<(), String> {
    let utf16 = |text: &str| text.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
    let has = |needle: &[u8]| find_all(binary, needle).next().is_some();
    if !binary.starts_with(b"MZ") {
        return Err("the binary is not a Windows executable".to_owned());
    }
    let missing: Vec<&str> = [
        ("its manifest", has(windows_resources::MANIFEST.as_bytes())),
        ("its version information", has(&utf16("VS_VERSION_INFO")) && has(&utf16(version))),
    ]
    .into_iter()
    .filter_map(|(what, there)| (!there).then_some(what))
    .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the executable lacks {} at {version} (crates/app/build.rs embeds the icon, version information and manifest when it is built on Windows)",
            missing.join(" and ")
        ))
    }
}

/// nsis_tauri_utils 0.5.3 in `cache` (downloaded once, from where tauri-bundler downloads it, and
/// checked against its SHA-1), copied into `dir` as `nsis_tauri_utils.dll` for the template's
/// `!addplugindir`.
fn nsis_plugin(cache: &Path, dir: &Path) -> Result<(), String> {
    let (url, sha1) = NSIS_TAURI_UTILS;
    let cached = cache.join("nsis_tauri_utils-v0.5.3.dll");
    let bytes = match fs::read(&cached) {
        Ok(bytes) if sha1_hex(&bytes) == sha1 => bytes,
        _ => {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| format!("a runtime: {e}"))?;
            let bytes = runtime.block_on(async {
                let http = reqwest::Client::builder().build().map_err(|e| format!("an HTTP client: {e}"))?;
                let response = http.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("{url}: {e}"))?;
                response.bytes().await.map(|b| b.to_vec()).map_err(|e| format!("{url}: {e}"))
            })?;
            if sha1_hex(&bytes) != sha1 {
                return Err(format!("{url}: not the plugin tauri-bundler 2.10.1 checks for (SHA-1 {sha1})"));
            }
            fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
            fs::write(&cached, &bytes).map_err(|e| format!("{}: {e}", cached.display()))?;
            bytes
        }
    };
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let to = dir.join("nsis_tauri_utils.dll");
    fs::write(&to, bytes).map_err(|e| format!("{}: {e}", to.display()))
}

/// `platform/macos/Info.plist` (its text) at `version`: the bundle's two version strings
/// (`CFBundleShortVersionString` and `CFBundleVersion`, both the release version as in today's bundle).
pub fn info_plist(text: &str, version: &str) -> Result<String, String> {
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) {
        return Err(format!("`{version}` is no version an Info.plist can carry as it is"));
    }
    let filled = text.replace("@@VERSION@@", version);
    if filled.contains("@@") {
        return Err(format!("{INFO_PLIST} names a value `cargo xtask package` does not fill in"));
    }
    Ok(filled)
}

/// The keys of a flat XML property list (one `dict` of strings and booleans, as an app's `Info.plist`)
/// with their values, in their order; a boolean's value is `true` or `false`. Anything else in the
/// `dict` is refused.
pub fn plist_entries(text: &str) -> Result<Vec<(String, String)>, String> {
    let text = Regex::new(r"(?s)<!--.*?-->").expect("a valid pattern").replace_all(text, "");
    let body = text
        .split_once("<dict>")
        .and_then(|(_, rest)| rest.rsplit_once("</dict>"))
        .map(|(body, _)| body)
        .ok_or_else(|| "not a property list with a <dict>".to_owned())?;
    let entry = Regex::new(r"^\s*<key>([^<]*)</key>\s*(?:<string>([^<]*)</string>|<(true|false)/>)").expect("a valid pattern");
    let mut entries = Vec::new();
    let mut rest = body;
    while !rest.trim().is_empty() {
        let found = entry.captures(rest).ok_or_else(|| format!("not a flat dictionary of strings and booleans at `{}`", rest.trim().lines().next().unwrap_or_default()))?;
        let value = found.get(2).or_else(|| found.get(3)).map_or("", |m| m.as_str());
        entries.push((found[1].to_owned(), value.to_owned()));
        rest = &rest[found[0].len()..];
    }
    Ok(entries)
}

/// The architecture a Mach-O executable is built for, as Rust names it (`universal` for a fat one).
pub fn macho_arch(binary: &[u8]) -> Option<&'static str> {
    let word = |at: usize| binary.get(at..at + 4).map(|b| [b[0], b[1], b[2], b[3]]);
    match word(0)? {
        // MH_MAGIC_64, little-endian, then the CPU type
        [0xcf, 0xfa, 0xed, 0xfe] => match u32::from_le_bytes(word(4)?) {
            0x0100_0007 => Some("x86_64"),
            0x0100_000c => Some("aarch64"),
            _ => None,
        },
        [0xca, 0xfe, 0xba, 0xbe] => Some("universal"),
        _ => None,
    }
}

/// Refuses a binary that is not a macOS executable for `arch` (each architecture's bundle is its own
/// release, as today's two).
pub fn check_macos_binary(binary: &[u8], arch: &str) -> Result<(), String> {
    match macho_arch(binary) {
        Some(found) if found == arch => Ok(()),
        Some(found) => Err(format!("the binary is a Mach-O for {found}, and this package is for {arch}: build it with --target {arch}-apple-darwin")),
        None => Err("the binary is not a 64-bit macOS executable (Mach-O)".to_owned()),
    }
}

/// The files under `dir`, as `/`-separated paths relative to it.
fn files_under(dir: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let mut folders = vec![dir.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).map_err(|e| format!("{}: {e}", folder.display()))? {
            let path = entry.map_err(|e| format!("{}: {e}", folder.display()))?.path();
            if path.is_dir() && !path.is_symlink() {
                folders.push(path);
            } else {
                let relative = path.strip_prefix(dir).map_err(|e| e.to_string())?;
                files.push(relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"));
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Refuses a `.app` that is not today's layout (`Contents/Info.plist`, `Contents/MacOS/<executable>`
/// and `Contents/Resources/<icon>`, and `Contents/_CodeSignature/CodeResources` once `sealed`), whose
/// executable cannot be run, or whose `Info.plist` lacks a key of `plist` (the filled-in
/// `platform/macos/Info.plist`) or gives it another value.
pub fn check_app_bundle(app: &Path, plist: &str, sealed: bool) -> Result<(), String> {
    let ours = plist_entries(plist).map_err(|e| format!("{INFO_PLIST}: {e}"))?;
    let path = app.join("Contents/Info.plist");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let theirs = plist_entries(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    for (key, value) in &ours {
        match theirs.iter().find(|(k, _)| k == key) {
            Some((_, v)) if v == value => {}
            Some((_, v)) => return Err(format!("the bundle's Info.plist has {key} `{v}`, {INFO_PLIST} `{value}`")),
            None => return Err(format!("the bundle's Info.plist lacks {key}")),
        }
    }
    let get = |key: &str| ours.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).ok_or_else(|| format!("{INFO_PLIST} names no {key}"));
    let executable = format!("Contents/MacOS/{}", get("CFBundleExecutable")?);
    let mut layout = vec!["Contents/Info.plist".to_owned(), executable.clone(), format!("Contents/Resources/{}", get("CFBundleIconFile")?)];
    if sealed {
        layout.push("Contents/_CodeSignature/CodeResources".to_owned());
    }
    layout.sort();
    let found = files_under(app)?;
    if found != layout {
        return Err(format!("{} holds {found:?}, today's bundle {layout:?}", app.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(app.join(&executable)).map_err(|e| e.to_string())?.permissions().mode();
        if mode & 0o111 == 0 {
            return Err(format!("{executable} is not executable"));
        }
    }
    Ok(())
}

/// Seals `app` ad hoc as doc 07 §13.2 says, on macOS: its extended attributes cleared first (as both
/// bundlers do before `codesign`), `codesign --force --deep --sign -`, then checked with
/// `codesign --verify --deep --strict`. Elsewhere `codesign` is not to be had: the bundle is left as it
/// is, with a warning. Whether it was sealed.
pub fn sign_app(app: &Path) -> Result<bool, String> {
    if !cfg!(target_os = "macos") {
        eprintln!("warning: {} is not sealed: codesign runs on macOS only (its executable keeps the linker's signature, if any)", app.display());
        return Ok(false);
    }
    let mut clear = Command::new("xattr");
    clear.arg("-cr").arg(app);
    run_command(clear, "xattr -cr")?;
    let mut sign = Command::new("codesign");
    sign.args(["--force", "--deep", "--sign", "-"]).arg(app);
    run_command(sign, "codesign --sign -")?;
    let mut verify = Command::new("codesign");
    verify.args(["--verify", "--deep", "--strict", "--verbose=2"]).arg(app);
    run_command(verify, "codesign --verify")?;
    Ok(true)
}

/// The updater's archive of `app` at `to`, as tauri-bundler 2.10.1's `create_tar` writes it on macOS:
/// the tar crate's `append_dir_all` of the bundle under its own name (`RustRadar.app/…`, symlinks kept
/// as links) through flate2's gzip at its default level.
pub fn app_archive(app: &Path, to: &Path) -> Result<(), String> {
    use std::io::Write;
    let name = app.file_name().ok_or_else(|| format!("{}: no bundle name", app.display()))?;
    let file = fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::default()));
    tar.follow_symlinks(false);
    tar.append_dir_all(name, app).map_err(|e| format!("{}: {e}", app.display()))?;
    let gzip = tar.into_inner().map_err(|e| format!("{}: {e}", to.display()))?;
    let mut file = gzip.finish().map_err(|e| format!("{}: {e}", to.display()))?;
    file.flush().map_err(|e| format!("{}: {e}", to.display()))
}

/// tauri-bundler 2.10.1's arguments to its dmg script (`bundle_dmg.sh`, run in the folder holding the
/// `.app`) for `config` (cargo-packager's configuration: the product name and `[dmg]`'s layout, by
/// cargo-packager's names, tauri-bundler's defaults for any it lacks), the volume's icon `icns` and the
/// disk image `dmg_name`; never `--skip-jenkins` (module docs).
pub fn dmg_args(config: &Value, icns: &Path, dmg_name: &str) -> Result<Vec<String>, String> {
    let product = config["product-name"].as_str().ok_or_else(|| format!("{PACKAGER_TOML} names no product-name"))?;
    let app = format!("{product}.app");
    let dmg = config.get("dmg").cloned().unwrap_or_else(|| json!({}));
    let known = ["window-size", "windowSize", "window_size", "app-position", "appPosition", "app_position", "application-folder-position", "appFolderPosition", "application_folder_position", "windowPosition"];
    if let Some(other) = dmg.as_object().into_iter().flat_map(|o| o.keys()).find(|k| !known.contains(&k.as_str())) {
        return Err(format!("{PACKAGER_TOML}: [dmg] {other} is not passed to the dmg script"));
    }
    // a [dmg] pair by any of cargo-packager's names for it, if set
    let pair = |names: &[&str], (a, b): (&str, &str)| -> Result<Option<[String; 2]>, String> {
        match names.iter().find_map(|n| dmg.get(*n)) {
            Some(value) => match (value[a].as_u64(), value[b].as_u64()) {
                (Some(x), Some(y)) => Ok(Some([x.to_string(), y.to_string()])),
                _ => Err(format!("{PACKAGER_TOML}: [dmg] {} needs {a} and {b}", names[0])),
            },
            None => Ok(None),
        }
    };
    let or = |set: Option<[String; 2]>, (x, y): (u32, u32)| set.unwrap_or_else(|| [x.to_string(), y.to_string()]);
    let [app_x, app_y] = or(pair(&["app-position", "appPosition", "app_position"], ("x", "y"))?, (180, 170));
    let [folder_x, folder_y] = or(pair(&["application-folder-position", "appFolderPosition", "application_folder_position"], ("x", "y"))?, (480, 170));
    let [width, height] = or(pair(&["window-size", "windowSize", "window_size"], ("width", "height"))?, (660, 400));
    let mut args: Vec<String> = ["--volname", product, "--icon", &app, &app_x, &app_y, "--app-drop-link", &folder_x, &folder_y, "--window-size", &width, &height, "--hide-extension", &app]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some([x, y]) = pair(&["windowPosition"], ("x", "y"))? {
        args.extend(["--window-pos".to_owned(), x, y]);
    }
    let icns = icns.to_str().ok_or_else(|| format!("{}: not UTF-8", icns.display()))?;
    args.extend(["--volicon", icns, dmg_name, &app].map(str::to_owned));
    Ok(args)
}

/// tauri-bundler's dmg script and its AppleScript laid out in `tools` as tauri-bundler lays them out
/// (the script finds its support files in `../share/create-dmg/support`); the script's path.
fn dmg_tools(root: &Path, tools: &Path) -> Result<PathBuf, String> {
    let script = tools.join("bin/bundle_dmg.sh");
    let support = tools.join("share/create-dmg/support");
    for folder in [tools.join("bin"), support.clone()] {
        fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    }
    fs::copy(root.join(DMG_SCRIPT), &script).map_err(|e| format!("{DMG_SCRIPT}: {e}"))?;
    set_executable(&script)?;
    fs::copy(root.join(DMG_APPLESCRIPT), support.join("template.applescript")).map_err(|e| format!("{DMG_APPLESCRIPT}: {e}"))?;
    Ok(script)
}

/// Runs tauri-bundler's dmg script with `args` in `dir`, the folder holding the sealed `.app`.
fn make_dmg(root: &Path, tools: &Path, dir: &Path, args: &[String]) -> Result<(), String> {
    let mut command = Command::new(dmg_tools(root, tools)?);
    command.current_dir(dir).args(args);
    run_command(command, "bundle_dmg.sh")
}

/// `Packager.toml`'s `[macos] minimum-system-version`: the deployment target the macOS release is built
/// for and its Info.plist's LSMinimumSystemVersion.
pub fn minimum_system_version(text: &str) -> Result<String, String> {
    let config: Value = toml::from_str(text).map_err(|e| format!("{PACKAGER_TOML}: {e}"))?;
    config
        .pointer("/macos/minimum-system-version")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{PACKAGER_TOML} names no [macos] minimum-system-version"))
}

/// The command line.
#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub formats: Vec<Format>,
    pub target: Option<String>,
    pub binary: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub sign: bool,
}

pub const USAGE: &str = "usage: cargo xtask package <deb|rpm|appimage|nsis|msi|app|dmg>... [--target TRIPLE] [--binary FILE] [--out DIR] [--sign]";

pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().cloned().ok_or_else(|| format!("{flag} needs a value\n{USAGE}"));
        match arg.as_str() {
            "--target" => parsed.target = Some(value("--target")?),
            "--binary" => parsed.binary = Some(PathBuf::from(value("--binary")?)),
            "--out" => parsed.out = Some(PathBuf::from(value("--out")?)),
            "--sign" => parsed.sign = true,
            name => {
                let format = Format::parse(name).ok_or_else(|| format!("no package format `{name}`\n{USAGE}"))?;
                if !parsed.formats.contains(&format) {
                    parsed.formats.push(format);
                }
            }
        }
    }
    if parsed.formats.is_empty() {
        return Err(USAGE.to_owned());
    }
    Ok(parsed)
}

/// `cargo`, as the one running this (`CARGO`), else from the path.
fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn run_command(mut command: Command, what: &str) -> Result<(), String> {
    let status = command.status().map_err(|e| format!("{what}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what} failed ({status})")) }
}

/// Fails unless `cargo <subcommand> --version` is `name version`.
fn check_tool(subcommand: &str, (name, version): (&str, &str)) -> Result<(), String> {
    let found = cargo().args([subcommand, "--version"]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    match found {
        Some(found) if found == format!("{name} {version}") => Ok(()),
        found => Err(format!(
            "{name} {version} is needed ({}): cargo install --locked {name}@{version}",
            found.map_or("not installed".to_owned(), |f| format!("found {f}"))
        )),
    }
}

/// The package of `crates/app` (also its binary's name).
fn app_package(root: &Path) -> Result<String, String> {
    let manifest = fs::read_to_string(root.join("crates/app/Cargo.toml")).map_err(|e| format!("crates/app/Cargo.toml: {e}"))?;
    let name = Regex::new(r#"(?m)^name\s*=\s*"([^"]+)""#).expect("a valid pattern");
    name.captures(&manifest).map(|c| c[1].to_owned()).ok_or_else(|| "crates/app/Cargo.toml names no package".to_owned())
}

/// The workspace's target folder, as cargo has it (`CARGO_TARGET_DIR` and all).
fn target_dir(root: &Path) -> Result<PathBuf, String> {
    let output = cargo().current_dir(root).args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    metadata["target_directory"].as_str().map(PathBuf::from).ok_or_else(|| "cargo metadata names no target directory".to_owned())
}

/// The architecture of `target` (a Rust triple) or of this machine.
fn arch(target: Option<&str>) -> String {
    target.and_then(|t| t.split('-').next()).unwrap_or(std::env::consts::ARCH).to_owned()
}

/// Builds the release binary and says where it is.
fn build(root: &Path, target_dir: &Path, target: Option<&str>) -> Result<PathBuf, String> {
    let package = app_package(root)?;
    let mut command = cargo();
    command.current_dir(root).args(["build", "--release", "--locked", "-p", &package, "--bin", &package]);
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    // a macOS release is built for the bundle's minimum system version, as the Tauri CLI built it for
    // bundle.macOS.minimumSystemVersion (the C and C++ dependencies too)
    let macos = target.map_or(cfg!(target_os = "macos"), |t| t.contains("apple-darwin"));
    if macos && std::env::var_os("MACOSX_DEPLOYMENT_TARGET").is_none() {
        let text = fs::read_to_string(root.join(PACKAGER_TOML)).map_err(|e| format!("{PACKAGER_TOML}: {e}"))?;
        command.env("MACOSX_DEPLOYMENT_TARGET", minimum_system_version(&text)?);
    }
    run_command(command, "cargo build --release")?;
    let mut dir = target_dir.to_owned();
    if let Some(target) = target {
        dir.push(target);
    }
    // a Windows target's binary (the target named, or this machine's) ends in .exe
    let windows = target.map_or(cfg!(windows), |t| t.contains("windows"));
    Ok(dir.join("release").join(if windows { format!("{package}.exe") } else { package }))
}

/// The one file in `dir` ending in `extension`.
fn the_output(dir: &Path, extension: &str) -> Result<PathBuf, String> {
    let found: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(extension)))
        .collect();
    match &found[..] {
        [one] => Ok(one.clone()),
        _ => Err(format!("{} holds {} {extension} files, one was expected", dir.display(), found.len())),
    }
}

/// Moves `from` to `to` (copying when they are on different file systems).
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))?;
    fs::remove_file(from).map_err(|e| format!("{}: {e}", from.display()))
}

/// Packs `format` from the release binary `binary` into `out`, under today's name.
fn pack(root: &Path, work: &Path, format: Format, binary: &[u8], version: &str, args: &Args, out: &Path) -> Result<PathBuf, String> {
    let arch = arch(args.target.as_deref());
    let name = format.asset_name(version, &arch)?;
    let _ = fs::remove_dir_all(work);
    let (bin_dir, out_dir) = (work.join("bin"), work.join("out"));
    for dir in [&bin_dir, &out_dir] {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }

    // the binary as this package installs it, its installer kind written in
    if format.windows() {
        check_windows_resources(binary, version)?;
        check_windows_imports(binary)?;
    }
    if format.macos() {
        check_macos_binary(binary, &arch)?;
    }
    let mut patched = binary.to_vec();
    patch_marker(&mut patched, format.marker())?;
    let staged = bin_dir.join(format.binary_name());
    fs::write(&staged, &patched).map_err(|e| format!("{}: {e}", staged.display()))?;
    set_executable(&staged)?;

    match format {
        Format::Deb | Format::AppImage | Format::Nsis | Format::Msi | Format::App | Format::Dmg => {
            let text = fs::read_to_string(root.join(PACKAGER_TOML)).map_err(|e| format!("{PACKAGER_TOML}: {e}"))?;
            let depends: Vec<String> = match (format, glibc_needed(binary)) {
                (Format::Deb, Some(v)) => vec![format!("libc6 (>= {})", v.iter().map(u32::to_string).collect::<Vec<_>>().join("."))],
                _ => Vec::new(),
            };
            let mut config = packager_config(&text, format, version, &bin_dir, &out_dir, &depends)?;
            // the Windows templates, filled in with what cargo-packager does not derive as
            // tauri-bundler did, then handed to cargo-packager as the template
            let read = |file: &str| fs::read_to_string(root.join(file)).map_err(|e| format!("{file}: {e}"));
            let template = match format {
                Format::Nsis => {
                    let plugins = work.join("nsis-plugins");
                    // kept beside the target folder's packages: <target>/package-tools
                    let tools = work.ancestors().nth(2).unwrap_or(work).join("package-tools");
                    nsis_plugin(&tools, &plugins)?;
                    Some(("nsis", nsis_template(&read(NSIS_TEMPLATE)?, &root.join(NSIS_UTILS), &plugins, binary.len() as u64)?))
                }
                Format::Msi => {
                    let product = config["product-name"].as_str().ok_or_else(|| format!("{PACKAGER_TOML} names no product-name"))?.to_owned();
                    Some(("wix", wix_template(&read(WIX_TEMPLATE)?, &product)?))
                }
                _ => None,
            };
            if let Some((table, filled)) = template {
                let path = work.join(format!("{table}-template"));
                fs::write(&path, filled).map_err(|e| format!("{}: {e}", path.display()))?;
                config[table]["template"] = json!(path);
            }
            // the bundle's Info.plist at this version, whose keys cargo-packager puts over its own
            let plist = if format.macos() {
                let plist = info_plist(&read(INFO_PLIST)?, version)?;
                let path = work.join("Info.plist");
                fs::write(&path, &plist).map_err(|e| format!("{}: {e}", path.display()))?;
                config["macos"]["info-plist-path"] = json!(path);
                Some(plist)
            } else {
                None
            };
            if format == Format::AppImage {
                let missing = missing_libs(&config, &arch);
                if !missing.is_empty() {
                    return Err(format!(
                        "the AppImage carries {} from this machine, which has none: install them (Ubuntu: libxkbcommon0 libxkbcommon-x11-0 libxcb-xkb1)",
                        missing.join(", ")
                    ));
                }
            }
            let mut command = cargo();
            command.current_dir(root).arg("packager").arg("--config").arg(config.to_string());
            if let Some(target) = &args.target {
                command.args(["--target", target]);
            }
            // linuxdeploy and its plugin are AppImages themselves: run them without FUSE
            command.env("APPIMAGE_EXTRACT_AND_RUN", "1");
            run_command(command, &format!("cargo packager ({})", format.name()))?;
            if let Some(plist) = plist {
                let product = config["product-name"].as_str().ok_or_else(|| format!("{PACKAGER_TOML} names no product-name"))?.to_owned();
                let app = out_dir.join(format!("{product}.app"));
                check_app_bundle(&app, &plist, false)?;
                let sealed = sign_app(&app)?;
                check_app_bundle(&app, &plist, sealed)?;
                if format == Format::App {
                    // named and signed as tauri-bundler named it (the .sig's trusted comment says
                    // RustRadar.app.tar.gz, as v0.3.16's), moved to the asset's name with its .sig below
                    let archive = out_dir.join(format!("{product}.app.tar.gz"));
                    app_archive(&app, &archive)?;
                    if args.sign {
                        sign::run(&[archive.display().to_string()])?;
                    }
                } else {
                    let icns = config["icons"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .find(|i| i.ends_with(".icns"))
                        .ok_or_else(|| format!("{PACKAGER_TOML}'s icons have no .icns for the volume"))?;
                    make_dmg(root, &work.join("dmg-tools"), &out_dir, &dmg_args(&config, &root.join(icns), &name)?)?;
                }
            }
        }
        Format::Rpm => {
            let text = fs::read_to_string(root.join(RPM_TOML)).map_err(|e| format!("{RPM_TOML}: {e}"))?;
            let manifest = work.join("Cargo.toml");
            fs::write(&manifest, rpm_manifest(&text, version, &staged, root)?).map_err(|e| format!("{}: {e}", manifest.display()))?;
            // gzip as Tauri's rpm (cargo-generate-rpm's default is zstd); the linked libraries found
            // by its own reading of the binary, the same on every build machine
            let mut command = cargo();
            command.current_dir(work).args(["generate-rpm", "--payload-compress", "gzip", "--auto-req", "builtin"]);
            command.arg("-o").arg(&out_dir);
            if let Some(target) = &args.target {
                command.args(["--target", target]);
            }
            run_command(command, "cargo generate-rpm")?;
        }
    }

    let made = the_output(&out_dir, format.extension())?;
    let to = out.join(&name);
    move_file(&made, &to)?;
    // the .app.tar.gz's signature, made before the move
    let sig = PathBuf::from(format!("{}.sig", made.display()));
    if sig.is_file() {
        move_file(&sig, &out.join(format!("{name}.sig")))?;
    }
    let _ = fs::remove_dir_all(work);
    Ok(to)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<(), String> {
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), String> {
    let args = parse(args)?;
    let root = bump::repo_root();
    if args.formats.iter().any(|f| matches!(f, Format::Deb | Format::AppImage | Format::Nsis | Format::Msi | Format::App | Format::Dmg)) {
        check_tool("packager", PACKAGER)?;
    }
    if args.formats.contains(&Format::Msi) && !cfg!(windows) {
        return Err("the MSI is made on Windows (cargo-packager runs WiX 3 there only)".to_owned());
    }
    if args.formats.contains(&Format::Dmg) && !cfg!(target_os = "macos") {
        return Err("the dmg is made on macOS (hdiutil, and the Finder for its layout)".to_owned());
    }
    if args.formats.contains(&Format::Rpm) {
        check_tool("generate-rpm", GENERATE_RPM)?;
    }
    let version = bump::current()?;
    let target_dir = target_dir(&root)?;
    let binary = match &args.binary {
        Some(binary) => binary.clone(),
        None => build(&root, &target_dir, args.target.as_deref())?,
    };
    let bytes = fs::read(&binary).map_err(|e| format!("{}: {e}", binary.display()))?;
    let out = args.out.clone().unwrap_or_else(|| target_dir.join("package"));
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;

    let mut made = Vec::new();
    for format in &args.formats {
        let work = target_dir.join("package-work").join(format.name());
        let package = pack(&root, &work, *format, &bytes, &version, &args, &out)?;
        println!("packed {}", package.display());
        // the .app.tar.gz was signed as it was packed, under tauri-bundler's name; the dmg has no .sig
        // (as today)
        if !format.macos() {
            made.push(package.display().to_string());
        }
    }
    let _ = fs::remove_dir(target_dir.join("package-work"));
    if args.sign && !made.is_empty() {
        sign::run(&made)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        bump::repo_root()
    }

    fn packager_toml() -> Value {
        toml::from_str(&fs::read_to_string(root().join(PACKAGER_TOML)).unwrap()).unwrap()
    }

    #[test]
    fn the_marker_is_patched_in_place() {
        // every format whose packages tauri-bundler patches (the macOS ones keep the marker:
        // macos_packages_keep_the_marker)
        for format in Format::ALL.into_iter().filter(|f| !f.macos()) {
            let mut binary = b"\x7fELF....".to_vec();
            binary.extend_from_slice(b"__TAURI_BUNDLE_TYPE_VAR_DEB__TAURI_BUNDLE_TYPE_VAR_RPM");
            binary.extend_from_slice(MARKER);
            binary.extend_from_slice(b"__TAURI_BUNDLE_TYPE_VAR_APP tail");
            let len = binary.len();
            patch_marker(&mut binary, format.marker()).unwrap();
            assert_eq!(binary.len(), len);
            assert_eq!(find_all(&binary, MARKER).count(), 0);
            let at = 8 + 2 * 27;
            assert_eq!(&binary[at..at + 27], format.marker());
            assert!(binary.ends_with(b"__TAURI_BUNDLE_TYPE_VAR_APP tail"), "only the marker changed");
        }
    }

    #[test]
    fn a_binary_with_no_marker_or_two_is_refused() {
        let mut none = b"no marker here, only __TAURI_BUNDLE_TYPE_VAR_DEB".to_vec();
        assert!(patch_marker(&mut none, Format::Deb.marker()).unwrap_err().contains("holds no"));
        let mut two = [&MARKER[..], b"..", &MARKER[..]].concat();
        let before = two.clone();
        assert!(patch_marker(&mut two, Format::Rpm.marker()).unwrap_err().contains("2 times"));
        assert_eq!(two, before, "nothing patched");
        // a patched binary cannot be patched again
        let mut once = MARKER.to_vec();
        patch_marker(&mut once, Format::AppImage.marker()).unwrap();
        assert!(patch_marker(&mut once, Format::Deb.marker()).is_err());
    }

    /// The markers are the ones the app reads (`update::manifest::Installer::from_marker`) and
    /// tauri-utils 2.10.1's `bundle_type` reads, and the app's static starts as [`MARKER`].
    #[test]
    fn the_markers_are_the_apps() {
        let reader = fs::read_to_string(root().join("crates/app/src/update/manifest.rs")).unwrap();
        let marker = std::str::from_utf8(MARKER).unwrap();
        assert!(reader.contains(&format!("static mut __TAURI_BUNDLE_TYPE: &str = \"{marker}\";")));
        for (format, installer) in [(Format::Deb, "Deb"), (Format::Rpm, "Rpm"), (Format::AppImage, "AppImage"), (Format::Nsis, "Nsis"), (Format::Msi, "Msi")] {
            let marker = std::str::from_utf8(format.marker()).unwrap();
            assert_eq!(marker.len(), MARKER.len());
            assert!(reader.contains(&format!("\"{marker}\" => Some(Installer::{installer}),")), "{marker}");
        }
    }

    /// The names of v0.3.16's Linux assets, which `latest-json` reads back as its keys.
    #[test]
    fn the_packages_have_todays_names() {
        let names: Vec<String> = Format::LINUX.iter().map(|f| f.asset_name("0.3.16", "x86_64").unwrap()).collect();
        assert_eq!(names, ["RustRadar_0.3.16_amd64.deb", "RustRadar-0.3.16-1.x86_64.rpm", "RustRadar_0.3.16_amd64.AppImage"]);
        for (name, bundle) in names.iter().zip(["deb", "rpm", "appimage"]) {
            assert_eq!(crate::manifest::bundle(name), Some(("linux", bundle)), "{name}");
            assert_eq!(crate::manifest::arch(name), Some("x86_64"), "{name}");
        }
        // aarch64 as Tauri's bundler names it (not shipped today)
        let arm: Vec<String> = Format::LINUX.iter().map(|f| f.asset_name("1.2.3", "aarch64").unwrap()).collect();
        assert_eq!(arm, ["RustRadar_1.2.3_arm64.deb", "RustRadar-1.2.3-1.aarch64.rpm", "RustRadar_1.2.3_aarch64.AppImage"]);
        for name in &arm {
            assert_eq!(crate::manifest::arch(name), Some("aarch64"), "{name}");
        }
        assert!(Format::Deb.asset_name("1.2.3", "riscv64").is_err());
    }

    #[test]
    fn the_command_line() {
        let args = |a: &[&str]| parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let parsed = args(&["deb", "rpm", "appimage", "deb", "--target", "x86_64-unknown-linux-gnu", "--out", "dist", "--sign"]).unwrap();
        assert_eq!(parsed.formats, Format::LINUX);
        assert_eq!(parsed.target.as_deref(), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(parsed.out, Some(PathBuf::from("dist")));
        assert!(parsed.sign && parsed.binary.is_none());
        assert_eq!(args(&["rpm", "--binary", "target/release/rustradar-app"]).unwrap().binary, Some(PathBuf::from("target/release/rustradar-app")));
        assert!(args(&[]).unwrap_err().starts_with("usage"));
        assert!(args(&["--sign"]).unwrap_err().starts_with("usage"));
        assert!(args(&["snap"]).unwrap_err().contains("no package format `snap`"));
        assert!(args(&["deb", "--target"]).unwrap_err().contains("--target needs a value"));
    }

    #[test]
    fn glibc_versions_are_read_off_the_binary() {
        let binary = b"\0libc.so.6\0GLIBC_2.2.5\0GLIBC_2.34\0GLIBC_2.3.4\0GLIBC_TUNABLES\0GLIBC_PRIVATE\0GLIBC_2.18\0";
        assert_eq!(glibc_needed(binary), Some(vec![2, 34]));
        assert_eq!(glibc_needed(b"\0GLIBC_2.2.5\0"), Some(vec![2, 2, 5]));
        assert_eq!(glibc_needed(b"GLIBC_2.17"), Some(vec![2, 17]), "at the very end");
        assert_eq!(glibc_needed(b"no glibc here; GLIBC_2x"), None);
        assert_eq!(glibc_needed(b"GLIBC_2.40abc"), None, "not a version name");
    }

    /// `Packager.toml` keeps today's identity (doc 07 §13.6; v0.3.16's deb: Package rust-radar,
    /// Maintainer you, /usr/bin/rustradar) and the libraries winit needs (plan stage 2 gap 7).
    #[test]
    fn the_packager_configuration_is_todays_identity() {
        let config = packager_toml();
        let conf: Value = serde_json::from_str(&fs::read_to_string(root().join("src-tauri/tauri.conf.json")).unwrap()).unwrap();
        assert_eq!(config["product-name"], conf["productName"]);
        assert_eq!(config["identifier"], conf["identifier"]);
        assert_eq!(config["identifier"], "com.felix.rustradar");
        assert_eq!(config["description"], "Live flight radar on a 3D globe, powered by a Rust port of fr24");
        assert_eq!(config["authors"], json!(["you"]));
        let icons: Vec<String> = config["icons"].as_array().unwrap().iter().map(|i| format!("src-tauri/{}", i.as_str().unwrap().trim_start_matches("src-tauri/"))).collect();
        let today: Vec<String> = conf["bundle"]["icon"].as_array().unwrap().iter().map(|i| format!("src-tauri/{}", i.as_str().unwrap())).collect();
        assert_eq!(icons, today, "tauri.conf.json's bundle.icon");
        for icon in &icons {
            assert!(root().join(icon).is_file(), "{icon}");
        }
        assert_eq!(config["binaries"], json!([{ "path": BINARY, "main": true }]));
        assert_eq!(config["deb"]["package-name"], "rust-radar");
        assert_eq!(config["linux"]["generate-desktop-entry"], false);
        assert_eq!(config["deb"]["files"], json!({ "platform/linux/RustRadar.desktop": "usr/share/applications/RustRadar.desktop" }));
        assert_eq!(config["appimage"]["files"], json!({ "platform/linux/RustRadar.desktop": "usr/share/applications/rustradar.desktop" }));
        let depends: Vec<&str> = config["deb"]["depends"].as_array().unwrap().iter().map(|d| d.as_str().unwrap()).collect();
        for needed in ["libxkbcommon0", "libxkbcommon-x11-0", "libxcb-xkb1", "libx11-6", "libwayland-client0", "libvulkan1", "libegl1", "ca-certificates"] {
            assert!(depends.contains(&needed), "{needed}");
        }
        assert!(!depends.iter().any(|d| d.contains("webkit") || d.contains("gtk")), "no webview");
        let libs = config["appimage"]["libs"].as_array().unwrap();
        for needed in ["libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libxcb-xkb.so.1"] {
            assert!(libs.contains(&json!(needed)), "{needed}");
        }
    }

    /// The desktop file is v0.3.16's, byte for byte (`usr/share/applications/RustRadar.desktop` in
    /// its deb, rpm and AppImage).
    #[test]
    fn the_desktop_file_is_todays() {
        let today = "[Desktop Entry]\nCategories=\nComment=Live flight radar on a 3D globe, powered by a Rust port of fr24\nExec=rustradar\nStartupWMClass=rustradar\nIcon=rustradar\nName=RustRadar\nTerminal=false\nType=Application\n";
        assert_eq!(fs::read_to_string(root().join("platform/linux/RustRadar.desktop")).unwrap(), today);
    }

    /// A PNG's size, from its IHDR chunk.
    fn png_size(path: &Path) -> (u32, u32) {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&bytes[12..16], b"IHDR");
        let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        (word(16), word(20))
    }

    /// The rpm installs the files the deb does: the desktop file, and each PNG icon where Tauri's
    /// bundler and cargo-packager put it (`{w}x{h}`, `@2` for an `@2x` file, `/apps/rustradar.png`).
    #[test]
    fn the_rpm_installs_what_the_deb_does() {
        let table: toml::Table = toml::from_str(&fs::read_to_string(root().join(RPM_TOML)).unwrap()).unwrap();
        assert_eq!(table["name"].as_str(), Some("rust-radar"));
        assert_eq!(table["release"].as_str(), Some("1"));
        assert_eq!(table["summary"].as_str(), packager_toml()["description"].as_str());
        let assets = table["assets"].as_array().unwrap();
        let mut dests = Vec::new();
        for asset in assets {
            let (source, dest) = (asset["source"].as_str().unwrap(), asset["dest"].as_str().unwrap());
            assert!(root().join(source).is_file(), "{source}");
            if source.ends_with(".png") {
                let (w, h) = png_size(&root().join(source));
                let high = if source.ends_with("@2x.png") { "@2" } else { "" };
                assert_eq!(dest, format!("/usr/share/icons/hicolor/{w}x{h}{high}/apps/{BINARY}.png"), "{source}");
            }
            dests.push(dest.to_owned());
        }
        let config = packager_toml();
        let pngs: Vec<&str> = config["icons"].as_array().unwrap().iter().filter_map(Value::as_str).filter(|i| i.ends_with(".png")).collect();
        assert_eq!(assets.iter().filter(|a| a["source"].as_str().unwrap().ends_with(".png")).count(), pngs.len(), "every PNG of Packager.toml");
        assert!(dests.contains(&"/usr/share/applications/RustRadar.desktop".to_owned()));
        for needed in ["libxkbcommon-x11.so.0()(64bit)", "libxcb-xkb.so.1()(64bit)", "libX11.so.6()(64bit)", "libwayland-client.so.0()(64bit)", "libvulkan.so.1()(64bit)", "libEGL.so.1()(64bit)", "ca-certificates"] {
            assert!(table["requires"].as_table().unwrap().contains_key(needed), "{needed}");
        }
    }

    #[test]
    fn each_package_gets_its_build_in_the_configuration() {
        let text = fs::read_to_string(root().join(PACKAGER_TOML)).unwrap();
        let (bin, out) = (Path::new("/w/bin"), Path::new("/w/out"));
        let deb = packager_config(&text, Format::Deb, "0.3.16", bin, out, &["libc6 (>= 2.34)".to_owned()]).unwrap();
        assert_eq!(deb["version"], "0.3.16");
        assert_eq!(deb["formats"], json!(["deb"]));
        assert_eq!(deb["binariesDir"], "/w/bin");
        assert_eq!(deb["outDir"], "/w/out");
        let depends = deb["deb"]["depends"].as_array().unwrap();
        assert_eq!(depends[0], "libc6 (>= 2.34)");
        assert_eq!(depends.len(), packager_toml()["deb"]["depends"].as_array().unwrap().len() + 1);
        let appimage = packager_config(&text, Format::AppImage, "0.3.16", bin, out, &[]).unwrap();
        assert_eq!(appimage["formats"], json!(["appimage"]));
        assert_eq!(appimage["deb"], packager_toml()["deb"], "the deb's list untouched");
        assert!(packager_config(&format!("version = \"1.0.0\"\n{text}"), Format::Deb, "0.3.16", bin, out, &[]).unwrap_err().contains("sets `version`"));
        assert!(packager_config(&format!("out-dir = \"x\"\n{text}"), Format::Deb, "0.3.16", bin, out, &[]).is_err());

        let manifest = rpm_manifest(&fs::read_to_string(root().join(RPM_TOML)).unwrap(), "0.3.16", Path::new("/w/bin/rustradar"), Path::new("/repo")).unwrap();
        let manifest: toml::Table = toml::from_str(&manifest).unwrap();
        let package = manifest["package"].as_table().unwrap();
        assert_eq!(package["name"].as_str(), Some("rust-radar"));
        assert_eq!(package["version"].as_str(), Some("0.3.16"));
        let rpm = package["metadata"]["generate-rpm"].as_table().unwrap();
        assert_eq!(rpm["version"].as_str(), Some("0.3.16"));
        assert_eq!(rpm["license"].as_str(), Some(""));
        let assets = rpm["assets"].as_array().unwrap();
        assert_eq!(assets.len(), 5);
        assert_eq!(assets[0]["source"].as_str(), Some("/w/bin/rustradar"));
        assert_eq!(assets[0]["dest"].as_str(), Some("/usr/bin/rustradar"));
        assert_eq!(assets[0]["mode"].as_str(), Some("755"));
        assert_eq!(assets[1]["source"].as_str(), Some("/repo/platform/linux/RustRadar.desktop"));
        assert_eq!(rpm["requires"], toml::from_str::<toml::Table>(&fs::read_to_string(root().join(RPM_TOML)).unwrap()).unwrap()["requires"]);
        assert!(rpm_manifest("version = \"1\"\nassets = []", "0.3.16", Path::new("/b"), Path::new("/r")).is_err());
        assert!(rpm_manifest("assets = []", "0.3.16", Path::new("/b"), Path::new("/r")).unwrap_err().contains("names no package"));
    }

    #[test]
    fn missing_libraries_are_named() {
        let config = json!({ "appimage": { "libs": ["libc.so.6", "libsurely-not-here-xtask.so.9"] } });
        let missing = missing_libs(&config, std::env::consts::ARCH);
        assert!(missing.contains(&"libsurely-not-here-xtask.so.9".to_owned()));
        if cfg!(target_os = "linux") && Path::new(&format!("/usr/lib/{}-linux-gnu/libc.so.6", std::env::consts::ARCH)).exists() {
            assert_eq!(missing, ["libsurely-not-here-xtask.so.9"]);
        }
        assert!(missing_libs(&json!({}), "x86_64").is_empty());
    }

    /// The Windows installers' names, as v0.3.16's release has them and `latest-json` reads them back.
    #[test]
    fn the_windows_installers_have_todays_names() {
        let names: Vec<String> = Format::WINDOWS.iter().map(|f| f.asset_name("0.3.16", "x86_64").unwrap()).collect();
        assert_eq!(names, ["RustRadar_0.3.16_x64-setup.exe", "RustRadar_0.3.16_x64_en-US.msi"]);
        for (name, bundle) in names.iter().zip(["nsis", "msi"]) {
            assert_eq!(crate::manifest::bundle(name), Some(("windows", bundle)), "{name}");
            assert_eq!(crate::manifest::arch(name), Some("x86_64"), "{name}");
        }
        assert_eq!(Format::Nsis.asset_name("1.2.3", "aarch64").unwrap(), "RustRadar_1.2.3_arm64-setup.exe");
        assert_eq!(Format::Msi.asset_name("1.2.3", "aarch64").unwrap(), "RustRadar_1.2.3_arm64_en-US.msi");
        assert!(Format::Msi.asset_name("1.2.3", "riscv64").is_err());
        assert_eq!([Format::Nsis.binary_name(), Format::Msi.binary_name(), Format::Deb.binary_name()], ["rustradar.exe", "rustradar.exe", "rustradar"]);
        assert_eq!([Format::Nsis.packager_name(), Format::Msi.packager_name()], ["nsis", "wix"]);
        let args = parse(&["nsis".to_owned(), "msi".to_owned()]).unwrap();
        assert_eq!(args.formats, Format::WINDOWS);
        assert!(USAGE.contains("nsis|msi"));
    }

    /// tauri-bundler's UpgradeCode for today's product name is the one in v0.3.16's MSI (its Property
    /// and Upgrade tables, read with msiinfo), and the UUID v5 is RFC 4122's.
    #[test]
    fn the_upgrade_code_is_todays() {
        assert_eq!(upgrade_code("RustRadar"), "c73f46ff-44dc-5f3e-ab93-388b09b92dec");
        // Python's uuid.uuid5(uuid.NAMESPACE_DNS, 'python.org') names "python.org"; the same digest
        // here names "python.org.exe.app.x64", so check the arithmetic through the product name
        assert_eq!(upgrade_code("python.org").len(), 36);
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let code = upgrade_code("Other");
        assert_ne!(code, upgrade_code("RustRadar"));
        assert_eq!(&code[14..15], "5", "version 5");
        assert!(matches!(&code[19..20], "8" | "9" | "a" | "b"), "RFC 4122 variant");
    }

    /// `{{name}}`s a template renders, without block keywords, loop variables and helpers.
    fn template_keys(text: &str, local: &[&str]) -> Vec<String> {
        let mut keys = Vec::new();
        for token in text.split("{{").skip(1).filter_map(|t| t.split("}}").next()) {
            let words: Vec<&str> = token.trim_matches(|c: char| c == '~' || c.is_whitespace()).split_whitespace().collect();
            let word = match words.first().copied() {
                Some("#if" | "#each" | "#unless") => words.get(1).copied(),
                Some(w) if w.starts_with('/') || w == "else" => None,
                Some("or" | "association-description" | "unescape_newlines" | "unescape_dollar_sign") => None,
                other => other,
            };
            if let Some(word) = word.filter(|w| !w.contains('.') && !w.starts_with('@') && *w != "this" && !local.contains(w)) {
                keys.push(word.to_owned());
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    /// The NSIS template, filled in: tauri-bundler's script with today's identity (per user, the
    /// uninstall key, the update and restart flags), what `cargo xtask package` fills in, no WebView2,
    /// and only names cargo-packager 0.11.8's data has (nsis/mod.rs), or tauri-bundler's that were
    /// empty for today's configuration too.
    #[test]
    fn the_nsis_template_is_tauri_bundlers() {
        let text = fs::read_to_string(root().join(NSIS_TEMPLATE)).unwrap();
        let filled = nsis_template(&text, Path::new("/repo/platform/windows/utils.nsh"), Path::new("/t/package-work/nsis/nsis-plugins"), 38_000).unwrap();
        for line in [
            "!include \"/repo/platform/windows/utils.nsh\"",
            "!define ADDITIONALPLUGINSPATH \"/t/package-work/nsis/nsis-plugins\"",
            "!addplugindir \"${ADDITIONALPLUGINSPATH}\"",
            "!define ESTIMATEDSIZE \"37\"",
            "!define UNINSTKEY \"Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${PRODUCTNAME}\"",
            "!define MANUPRODUCTKEY \"${MANUKEY}\\${PRODUCTNAME}\"",
            "      StrCpy $INSTDIR \"$LOCALAPPDATA\\${PRODUCTNAME}\"",
            "  RequestExecutionLevel user",
            "  ${GetOptions} $CMDLINE \"/UPDATE\" $UpdateMode",
            "    ${GetOptions} $CMDLINE \"/R\" $R0",
            "      ${GetOptions} $CMDLINE \"/ARGS\" $R0",
            "      nsis_tauri_utils::RunAsUser \"$INSTDIR\\${MAINBINARYNAME}.exe\" \"$R0\"",
            "  WriteRegStr SHCTX \"${UNINSTKEY}\" \"UninstallString\" \"$\\\"$INSTDIR\\uninstall.exe$\\\"\"",
            "    CreateShortcut \"$SMPROGRAMS\\${PRODUCTNAME}.lnk\" \"$INSTDIR\\${MAINBINARYNAME}.exe\"",
            "  CreateShortcut \"$DESKTOP\\${PRODUCTNAME}.lnk\" \"$INSTDIR\\${MAINBINARYNAME}.exe\"",
            "!define BUNDLEID \"{{identifier}}\"",
        ] {
            assert!(filled.lines().any(|l| l == line), "{line}");
        }
        assert!(!filled.contains("@@"));
        let code: String = filled.lines().filter(|l| !l.trim_start().starts_with(';')).collect();
        assert!(!code.to_lowercase().contains("webview"), "no WebView2 outside the comments");
        let provided = [
            "additional_plugins_path", "allow_downgrades", "arch", "binaries", "compression", "copyright", "deep_link_protocols",
            "display_language_selector", "estimated_size", "file_associations", "header_image", "identifier",
            "install_mode", "installer_icon", "language_files", "languages", "license", "main_binary_name",
            "main_binary_path", "manufacturer", "out_file", "product_name", "resources", "resources_dirs",
            "sidebar_image", "uninstaller_sign_cmd", "version", "version_with_build",
        ];
        // empty in tauri-bundler's rendering of today's tauri.conf.json too (none of them is set)
        let empty_today = ["homepage", "installer_hooks", "signed_plugins_path", "start_menu_folder", "uninstaller_header_image", "uninstaller_icon"];
        for key in template_keys(&filled, &["association", "ext", "protocol"]) {
            assert!(provided.contains(&key.as_str()) || empty_today.contains(&key.as_str()), "{{{{{key}}}}}");
        }
        assert!(nsis_template(&text, Path::new("C:\\$x\\utils.nsh"), Path::new("/p"), 1).is_err());
        assert!(nsis_template("@@OTHER@@", Path::new("/u"), Path::new("/p"), 1).is_err());
    }

    /// The template's messages are all in tauri-bundler's English strings, and the plugin commands it
    /// calls are all exports of nsis_tauri_utils 0.5.3 (the DLL in v0.3.16's installer, read with
    /// objdump -p; 0.2.1, cargo-packager's, exports only Download, FindProcess, KillProcess and
    /// SemverCompare).
    #[test]
    fn the_nsis_messages_and_plugin_calls_exist() {
        let script = [NSIS_TEMPLATE, NSIS_UTILS].map(|f| fs::read_to_string(root().join(f)).unwrap()).concat();
        let english = fs::read_to_string(root().join("platform/windows/English.nsh")).unwrap();
        let defined: Vec<&str> = english.lines().filter_map(|l| l.strip_prefix("LangString ")?.split_whitespace().next()).collect();
        let used = Regex::new(r"\$\(([A-Za-z0-9_]+)\)").unwrap();
        let mut count = 0;
        for name in used.captures_iter(&script).map(|c| c[1].to_owned()) {
            assert!(defined.contains(&name.as_str()), "$({name})");
            count += 1;
        }
        assert!(count > 20);
        let exports = ["FindProcess", "FindProcessCurrentUser", "KillProcess", "KillProcessCurrentUser", "RunAsUser", "SemverCompare", "StrReplace"];
        let calls = Regex::new(r"nsis_tauri_utils::([A-Za-z]+)").unwrap();
        let called: Vec<String> = calls.captures_iter(&script).map(|c| c[1].to_owned()).collect();
        assert!(called.iter().any(|c| c == "RunAsUser") && called.iter().any(|c| c == "StrReplace"));
        for call in &called {
            assert!(exports.contains(&call.as_str()), "nsis_tauri_utils::{call}");
        }
        assert_eq!(NSIS_TAURI_UTILS.1.len(), 40);
    }

    /// The WiX template, filled in: today's UpgradeCode, the per-machine install and the updaters'
    /// properties, cargo-packager's locale strings, no WebView2, and only names cargo-packager 0.11.8's
    /// data has (wix/mod.rs) or tauri-bundler's empty for today's configuration too.
    #[test]
    fn the_wix_template_is_tauri_bundlers() {
        let text = fs::read_to_string(root().join(WIX_TEMPLATE)).unwrap();
        let filled = wix_template(&text, "RustRadar").unwrap();
        for part in [
            "UpgradeCode=\"c73f46ff-44dc-5f3e-ab93-388b09b92dec\"",
            "Language=\"!(loc.CargoPackagerLanguage)\"",
            "SummaryCodepage=\"!(loc.CargoPackagerCodepage)\"",
            "InstallScope=\"perMachine\"",
            "<Property Id=\"AUTOLAUNCHAPP\" Secure=\"yes\" />",
            "<Property Id=\"LAUNCHAPPARGS\" Secure=\"yes\" />",
            "<CustomAction Id=\"LaunchApplication\" Impersonate=\"yes\" FileKey=\"Path\" ExeCommand=\"[LAUNCHAPPARGS]\" Return=\"asyncNoWait\" />",
            "<Custom Action=\"LaunchApplication\" After=\"InstallFinalize\">AUTOLAUNCHAPP AND NOT Installed</Custom>",
            "<File Id=\"Path\" Source=\"{{app_exe_source}}\" KeyPath=\"yes\" Checksum=\"yes\"/>",
            "<Directory Id=\"INSTALLDIR\" Name=\"{{product_name}}\"/>",
            "<ShortcutProperty Key=\"System.AppUserModel.ID\" Value=\"{{identifier}}\"/>",
            "Key=\"Software\\\\{{manufacturer}}\\\\{{product_name}}\" Name=\"InstallDir\"",
        ] {
            assert!(filled.contains(part), "{part}");
        }
        assert!(!filled.contains("@@") && !filled.contains("loc.Tauri"));
        let code = Regex::new(r"(?s)<!--.*?-->").unwrap().replace_all(&filled, "");
        assert!(!code.to_lowercase().contains("webview") && !code.contains("UpdateTask"), "no WebView2 or update task outside the comments");
        let provided = [
            "allow_downgrades", "app_exe_source", "banner_path", "binaries", "component_group_refs", "component_refs",
            "deep_link_protocols", "dialog_image_path", "feature_group_refs", "feature_refs", "file_associations",
            "icon_path", "identifier", "license", "manufacturer", "merge_modules", "merge_refs", "path_component_guid",
            "product_name", "resource_file_ids", "resources", "shortcut_guid", "version",
        ];
        for key in template_keys(&filled, &["association", "ext", "protocol", "bin", "msm", "id", "resource_file_id"]) {
            assert!(provided.contains(&key.as_str()) || key == "homepage", "{{{{{key}}}}}");
        }
        assert!(wix_template("@@OTHER@@", "RustRadar").is_err());
    }

    /// `Packager.toml`'s Windows part: tauri-bundler's templates and English strings, per user (the NSIS
    /// default today's tauri.conf.json keeps), downgrades allowed as tauri-bundler's default; each
    /// installer gets cargo-packager's format name.
    #[test]
    fn the_windows_configuration_is_todays() {
        let config = packager_toml();
        assert_eq!(config["windows"]["allow-downgrades"], true);
        assert_eq!(config["nsis"]["template"], NSIS_TEMPLATE);
        assert_eq!(config["nsis"]["installer-mode"], "currentUser");
        assert_eq!(config["nsis"]["compression"], "lzma");
        assert_eq!(config["nsis"]["languages"], json!(["English"]));
        assert_eq!(config["nsis"]["custom-language-file"], json!({ "English": "platform/windows/English.nsh" }));
        assert_eq!(config["wix"]["template"], WIX_TEMPLATE);
        assert_eq!(config["wix"]["languages"], json!(["en-US"]));
        for file in [NSIS_TEMPLATE, NSIS_UTILS, WIX_TEMPLATE, "platform/windows/English.nsh"] {
            assert!(root().join(file).is_file(), "{file}");
        }
        let conf: Value = serde_json::from_str(&fs::read_to_string(root().join("src-tauri/tauri.conf.json")).unwrap()).unwrap();
        assert!(conf["bundle"].get("windows").is_none(), "today's NSIS and WiX settings are tauri-bundler's defaults");
        assert!(config["icons"].as_array().unwrap().contains(&json!("src-tauri/icons/icon.ico")), "the MSI's ProductIcon");
        let text = fs::read_to_string(root().join(PACKAGER_TOML)).unwrap();
        let (bin, out) = (Path::new("/w/bin"), Path::new("/w/out"));
        assert_eq!(packager_config(&text, Format::Nsis, "0.3.16", bin, out, &[]).unwrap()["formats"], json!(["nsis"]));
        assert_eq!(packager_config(&text, Format::Msi, "0.3.16", bin, out, &[]).unwrap()["formats"], json!(["wix"]));
        // the publisher both bundlers and tauri-build derive: the identifier's second part
        assert_eq!(config["identifier"].as_str().unwrap().split('.').nth(1), Some(windows_resources::COMPANY_NAME));
        assert_eq!(config["product-name"], windows_resources::PRODUCT_NAME);
    }

    /// A Windows binary is packed only with the resources build.rs embeds.
    #[test]
    fn a_windows_binary_needs_its_resources() {
        let utf16 = |text: &str| text.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        let mut exe = b"MZ\x90\0".to_vec();
        exe.extend_from_slice(&utf16("VS_VERSION_INFO"));
        exe.extend_from_slice(&utf16("0.3.16"));
        exe.extend_from_slice(windows_resources::MANIFEST.as_bytes());
        check_windows_resources(&exe, "0.3.16").unwrap();
        assert!(check_windows_resources(&exe, "0.3.17").unwrap_err().contains("version information"));
        let bare = b"MZ\x90\0 no resources".to_vec();
        let error = check_windows_resources(&bare, "0.3.16").unwrap_err();
        assert!(error.contains("its manifest and its version information"), "{error}");
        assert!(check_windows_resources(b"\x7fELF", "0.3.16").unwrap_err().contains("not a Windows executable"));
    }

    /// A minimal PE32+ importing `dlls` (one section at 0x1000, its import descriptors first).
    fn pe_importing(dlls: &[&str]) -> Vec<u8> {
        let mut pe = vec![0u8; 0x600];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes());
        pe[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        pe[0x54..0x56].copy_from_slice(&240u16.to_le_bytes());
        let optional = 0x58;
        pe[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        pe[optional + 120..optional + 124].copy_from_slice(&0x1000u32.to_le_bytes());
        let section = optional + 240;
        pe[section..section + 5].copy_from_slice(b".idat");
        for (at, value) in [(8, 0x400u32), (12, 0x1000), (16, 0x400), (20, 0x200)] {
            pe[section + at..section + at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (i, dll) in dlls.iter().enumerate() {
            let name = 0x100 + i * 0x40;
            pe[0x200 + i * 20 + 12..0x200 + i * 20 + 16].copy_from_slice(&(0x1000 + name as u32).to_le_bytes());
            pe[0x200 + name..0x200 + name + dll.len()].copy_from_slice(dll.as_bytes());
        }
        pe
    }

    /// The executable's imports are read off its import directory; the Visual C++ Redistributable's
    /// DLLs are refused (v0.3.16's rustradar.exe imports Windows' DLLs and the Universal CRT only, read
    /// with objdump -p).
    #[test]
    fn a_windows_binary_needs_no_redistributable() {
        let today = ["KERNEL32.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "comctl32.dll", "user32.dll"];
        assert_eq!(windows_imports(&pe_importing(&today)).unwrap(), today);
        check_windows_imports(&pe_importing(&today)).unwrap();
        check_windows_imports(&pe_importing(&[])).unwrap();
        for dll in ["MSVCP140.dll", "VCRUNTIME140.dll", "vcruntime140_1.dll"] {
            let error = check_windows_imports(&pe_importing(&["KERNEL32.dll", dll])).unwrap_err();
            assert!(error.contains(dll) && error.contains("+crt-static"), "{error}");
        }
        assert!(windows_imports(b"MZ").is_err());
        assert!(windows_imports(b"\x7fELF").is_err());
    }

    /// The whole flow on a stand-in binary: each package built, named and laid out as today's. Run by
    /// hand with the packers installed (`cargo test -p xtask -- --ignored packs_`); the AppImage also
    /// needs the network once, for linuxdeploy.
    #[test]
    #[ignore = "needs cargo-packager 0.11.8, cargo-generate-rpm 0.21.1, dpkg-deb, rpm and unsquashfs; the AppImage downloads linuxdeploy"]
    fn packs_a_stand_in_binary() {
        let dir = std::env::temp_dir().join(format!("xtask-package-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // an ELF (linuxdeploy reads its dependencies) carrying the marker once
        let mut binary = fs::read("/bin/true").unwrap();
        binary.extend_from_slice(MARKER);
        let stand_in = dir.join("stand-in");
        fs::write(&stand_in, &binary).unwrap();
        let out = dir.join("out");
        let args: Vec<String> = ["deb", "rpm", "appimage", "--binary", stand_in.to_str().unwrap(), "--out", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
        run(&args).unwrap();
        let version = bump::current().unwrap();
        let shell = |command: &str| {
            let output = Command::new("sh").arg("-c").arg(command).output().unwrap();
            assert!(output.status.success(), "{command}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        let path = |format: Format| out.join(format.asset_name(&version, std::env::consts::ARCH).unwrap()).display().to_string();

        let deb = path(Format::Deb);
        let info = shell(&format!("dpkg-deb -f '{deb}' Package Version Maintainer Depends"));
        assert!(info.contains("Package: rust-radar\n") && info.contains(&format!("Version: {version}\n")) && info.contains("Maintainer: you\n"), "{info}");
        assert!(info.contains("Depends: libc6 (>= 2.") && info.contains("libxkbcommon-x11-0"), "{info}");
        let files = shell(&format!("dpkg-deb -c '{deb}' | awk '$1 ~ /^-/ {{print $6}}' | sort"));
        let today = "usr/bin/rustradar\nusr/share/applications/RustRadar.desktop\nusr/share/icons/hicolor/128x128/apps/rustradar.png\nusr/share/icons/hicolor/256x256@2/apps/rustradar.png\nusr/share/icons/hicolor/32x32/apps/rustradar.png\n";
        assert_eq!(files, today);
        let x = dir.join("deb");
        shell(&format!("dpkg-deb -x '{deb}' '{}'", x.display()));
        let installed = fs::read(x.join("usr/bin/rustradar")).unwrap();
        assert!(installed.ends_with(Format::Deb.marker()), "the deb's binary says deb");

        let rpm = path(Format::Rpm);
        let info = shell(&format!("rpm -qp --queryformat '%{{NAME}} %{{EPOCH}} %{{VERSION}} %{{RELEASE}} %{{ARCH}} %{{PAYLOADCOMPRESSOR}}' '{rpm}'"));
        assert_eq!(info, format!("rust-radar 0 {version} 1 x86_64 gzip"), "as v0.3.16's rpm");
        assert_eq!(shell(&format!("rpm -qlp '{rpm}' | sed 's|^/||' | sort")), today);
        let requires = shell(&format!("rpm -qp --requires '{rpm}'"));
        assert!(requires.contains("libxkbcommon-x11.so.0()(64bit)") && requires.contains("libc.so.6()(64bit)"), "{requires}");
        assert!(!requires.contains("/bin/sh"), "{requires}");

        let appimage = path(Format::AppImage);
        assert_eq!(&fs::read(&appimage).unwrap()[8..11], b"AI\x02", "a type 2 AppImage");
        let files = shell(&format!("unsquashfs -o $(python3 -c \"import struct,sys; f=open(sys.argv[1],'rb').read(64); o,=struct.unpack_from('<Q',f,0x28); s,n=struct.unpack_from('<HH',f,0x3A); print(o+s*n)\" '{appimage}') -l '{appimage}'"));
        for needed in ["usr/bin/rustradar", "usr/share/applications/rustradar.desktop", "rustradar.desktop", "usr/share/icons/hicolor/128x128/apps/rustradar.png", "libxkbcommon-x11.so.0", "libxcb-xkb.so.1"] {
            assert!(files.contains(needed), "{needed} in the AppImage:\n{files}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The NSIS installer of a stand-in `rustradar.exe` (a do-nothing program linked with the resources
    /// `crates/app/build.rs` writes), built on this machine and unpacked: today's layout, the binary
    /// with its installer kind, today's plugin and no WebView2 downloader. Run by hand
    /// (`cargo test -p xtask -- --ignored packs_an_nsis`); the MSI is CI's (WiX runs on Windows only).
    #[test]
    #[ignore = "needs cargo-packager 0.11.8 with its NSIS plugins, makensis, clang, lld-link, llvm-rc and 7z; fetches nsis_tauri_utils once"]
    fn packs_an_nsis_installer_from_a_stand_in() {
        let dir = std::env::temp_dir().join(format!("xtask-package-nsis-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let shell = |command: &str| {
            let output = Command::new("sh").arg("-c").arg(command).current_dir(&dir).output().unwrap();
            assert!(output.status.success(), "{command}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        let version = bump::current().unwrap();
        let icon = root().join("src-tauri/icons/icon.ico");
        fs::write(dir.join("rustradar.manifest"), windows_resources::MANIFEST).unwrap();
        let script = windows_resources::resource_script(&version, icon.to_str().unwrap(), dir.join("rustradar.manifest").to_str().unwrap()).unwrap();
        fs::write(dir.join("rustradar.rc"), script).unwrap();
        fs::write(dir.join("stub.c"), "int start(void) { return 0; }\n").unwrap();
        shell("llvm-rc --no-preprocess /FO rustradar.res rustradar.rc");
        shell("clang --target=x86_64-pc-windows-msvc -ffreestanding -O2 -c stub.c -o stub.obj");
        shell("lld-link /entry:start /subsystem:windows /nodefaultlib stub.obj rustradar.res /out:stand-in.exe");
        let mut exe = fs::read(dir.join("stand-in.exe")).unwrap();
        exe.extend_from_slice(MARKER);
        fs::write(dir.join("stand-in.exe"), &exe).unwrap();

        // a binary without its resources is refused
        let bare = dir.join("bare.exe");
        fs::write(&bare, [&b"MZ"[..], &MARKER[..]].concat()).unwrap();
        let out = dir.join("out");
        let args = |binary: &Path| -> Vec<String> { ["nsis", "--binary", binary.to_str().unwrap(), "--out", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect() };
        assert!(run(&args(&bare)).unwrap_err().contains("lacks its manifest"));

        run(&args(&dir.join("stand-in.exe"))).unwrap();
        let setup = out.join(Format::Nsis.asset_name(&version, "x86_64").unwrap());
        assert!(setup.is_file(), "{}", setup.display());
        shell(&format!("7z x -ounpacked '{}'", setup.display()));
        let listing = shell("cd unpacked && find . -type f | sort");
        let today = "./$PLUGINSDIR/StartMenu.dll\n./$PLUGINSDIR/System.dll\n./$PLUGINSDIR/modern-wizard.bmp\n./$PLUGINSDIR/nsDialogs.dll\n./$PLUGINSDIR/nsis_tauri_utils.dll\n./rustradar.exe\n./uninstall.exe\n";
        assert_eq!(listing, today, "v0.3.16's files without NSISdl (its WebView2 download), with the uninstaller");
        let installed = fs::read(dir.join("unpacked/rustradar.exe")).unwrap();
        assert!(installed.ends_with(Format::Nsis.marker()), "the installer's binary says NSIS");
        assert_eq!(installed.len(), exe.len());
        check_windows_resources(&installed, &version).unwrap();
        let plugin = fs::read(dir.join("unpacked/$PLUGINSDIR/nsis_tauri_utils.dll")).unwrap();
        assert_eq!(sha1_hex(&plugin), NSIS_TAURI_UTILS.1, "today's plugin, not cargo-packager's 0.2.1");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// `RustRadar.app/Contents/Info.plist` of v0.3.16 (its `RustRadar_0.3.16_aarch64.app.tar.gz`, the
    /// same in the x64 one and in both dmgs), byte for byte.
    const TODAYS_INFO_PLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>CFBundleDevelopmentRegion</key>\n\t<string>English</string>\n\t<key>CFBundleDisplayName</key>\n\t<string>RustRadar</string>\n\t<key>CFBundleExecutable</key>\n\t<string>rustradar</string>\n\t<key>CFBundleIdentifier</key>\n\t<string>com.felix.rustradar</string>\n\t<key>CFBundleInfoDictionaryVersion</key>\n\t<string>6.0</string>\n\t<key>CFBundleName</key>\n\t<string>RustRadar</string>\n\t<key>CFBundlePackageType</key>\n\t<string>APPL</string>\n\t<key>CFBundleShortVersionString</key>\n\t<string>0.3.16</string>\n\t<key>CFBundleVersion</key>\n\t<string>0.3.16</string>\n\t<key>CSResourcesFileMapped</key>\n\t<true/>\n\t<key>LSMinimumSystemVersion</key>\n\t<string>10.13</string>\n\t<key>CFBundleIconFile</key>\n\t<string>icon.icns</string>\n\t<key>LSRequiresCarbon</key>\n\t<true/>\n\t<key>NSHighResolutionCapable</key>\n\t<true/>\n</dict>\n</plist>";

    fn sha256_hex(bytes: &[u8]) -> String {
        aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A Mach-O header for `arch` with the marker after it (enough for the packing; no Mach-O runs here).
    fn mach_o(arch: &str) -> Vec<u8> {
        let cpu: u32 = match arch {
            "x86_64" => 0x0100_0007,
            "aarch64" => 0x0100_000c,
            other => panic!("{other}"),
        };
        let mut binary = vec![0xcf, 0xfa, 0xed, 0xfe];
        binary.extend_from_slice(&cpu.to_le_bytes());
        binary.extend_from_slice(&[0; 24]);
        binary.extend_from_slice(b"... __TAURI_BUNDLE_TYPE_VAR_DEB__TAURI_BUNDLE_TYPE_VAR_APP ...");
        binary.extend_from_slice(MARKER);
        binary
    }

    /// The macOS packages' names, as v0.3.16's release has them and `latest-json` reads the archive's
    /// back (the dmg is no updater artifact).
    #[test]
    fn the_macos_packages_have_todays_names() {
        let names: Vec<String> = ["aarch64", "x86_64"].iter().flat_map(|arch| Format::MACOS.map(|f| f.asset_name("0.3.16", arch).unwrap())).collect();
        assert_eq!(names, ["RustRadar_0.3.16_aarch64.app.tar.gz", "RustRadar_0.3.16_aarch64.dmg", "RustRadar_0.3.16_x64.app.tar.gz", "RustRadar_0.3.16_x64.dmg"]);
        for (name, arch) in [(&names[0], "aarch64"), (&names[2], "x86_64")] {
            assert_eq!(crate::manifest::bundle(name), Some(("darwin", "app")), "{name}");
            assert_eq!(crate::manifest::arch(name), Some(arch), "{name}");
        }
        assert_eq!(crate::manifest::bundle(&names[1]), None, "the dmg is no updater artifact");
        assert_eq!(Format::App.asset_name("1.2.3", "universal").unwrap(), "RustRadar_1.2.3_universal.app.tar.gz");
        assert!(Format::Dmg.asset_name("1.2.3", "riscv64").is_err());
        assert_eq!(Format::MACOS.map(Format::binary_name), ["rustradar", "rustradar"]);
        assert_eq!(Format::MACOS.map(Format::packager_name), ["app", "app"], "the dmg is made from cargo-packager's .app");
        let args = parse(&["app".to_owned(), "dmg".to_owned(), "--target".to_owned(), "aarch64-apple-darwin".to_owned()]).unwrap();
        assert_eq!(args.formats, Format::MACOS);
        assert!(USAGE.contains("app|dmg"));
        assert_eq!(Format::ALL.iter().filter(|f| f.macos()).count(), 2);
    }

    /// The macOS packages keep the binary's installer kind as it is, as tauri-bundler 2.10.1 does
    /// (v0.3.16's aarch64 `rustradar` holds `__TAURI_BUNDLE_TYPE_VAR_UNK` once, beside the five kinds'
    /// names), because the app takes the unknown kind for the `.app` on macOS and `_APP` for an
    /// AppImage (update::manifest's reader, as tauri-utils' `bundle_type`).
    #[test]
    fn macos_packages_keep_the_marker() {
        let reader = fs::read_to_string(root().join("crates/app/src/update/manifest.rs")).unwrap();
        assert!(reader.contains("_ if cfg!(target_os = \"macos\") => Some(Installer::App),"));
        assert!(reader.contains("\"__TAURI_BUNDLE_TYPE_VAR_APP\" => Some(Installer::AppImage),"));
        for format in Format::MACOS {
            assert_eq!(format.marker(), MARKER);
            let binary = mach_o("aarch64");
            let mut patched = binary.clone();
            patch_marker(&mut patched, format.marker()).unwrap();
            assert_eq!(patched, binary, "nothing written");
            // still the app's binary: one marker, not none or two
            let mut none = binary[..binary.len() - MARKER.len()].to_vec();
            assert!(patch_marker(&mut none, format.marker()).is_err());
            let mut two = [&binary[..], &MARKER[..]].concat();
            assert!(patch_marker(&mut two, format.marker()).is_err());
        }
    }

    /// The binary must be a Mach-O for the package's architecture.
    #[test]
    fn a_macos_binary_is_a_mach_o_for_its_architecture() {
        assert_eq!(macho_arch(&mach_o("aarch64")), Some("aarch64"));
        assert_eq!(macho_arch(&mach_o("x86_64")), Some("x86_64"));
        assert_eq!(macho_arch(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]), Some("universal"));
        check_macos_binary(&mach_o("x86_64"), "x86_64").unwrap();
        let wrong = check_macos_binary(&mach_o("aarch64"), "x86_64").unwrap_err();
        assert!(wrong.contains("for aarch64") && wrong.contains("--target x86_64-apple-darwin"), "{wrong}");
        for other in [&b"\x7fELF\x02\x01\x01\0"[..], b"MZ\x90\0", b"\xcf\xfa\xed\xfe\x12\0\0\x01", b"\xcf\xfa"] {
            assert!(check_macos_binary(other, "aarch64").unwrap_err().contains("not a 64-bit macOS executable"));
        }
    }

    /// `platform/macos/Info.plist` is v0.3.16's, key for key and in its order, at the release version and
    /// macOS 11, with doc 07 §13.2's two more keys; its identity is tauri.conf.json's, its executable and
    /// icon the package's, its minimum system the deployment target the release is built for.
    #[test]
    fn the_info_plist_is_todays_with_doc_07s_keys() {
        let text = fs::read_to_string(root().join(INFO_PLIST)).unwrap();
        let ours = plist_entries(&text).unwrap();
        let today = plist_entries(TODAYS_INFO_PLIST).unwrap();
        assert_eq!(today.len(), 14);
        for ((key, value), (today_key, today_value)) in ours.iter().zip(&today) {
            assert_eq!(key, today_key, "today's keys in today's order");
            let expected = match key.as_str() {
                "CFBundleShortVersionString" | "CFBundleVersion" => "@@VERSION@@",
                "LSMinimumSystemVersion" => "11.0",
                _ => today_value,
            };
            assert_eq!(value, expected, "{key}");
        }
        let added: Vec<(&str, &str)> = ours[today.len()..].iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(added, [("NSPrincipalClass", "NSApplication"), ("LSApplicationCategoryType", "public.app-category.travel")]);

        let value = |key: &str| ours.iter().find(|(k, _)| k == key).unwrap().1.clone();
        let conf: Value = serde_json::from_str(&fs::read_to_string(root().join("src-tauri/tauri.conf.json")).unwrap()).unwrap();
        assert_eq!(value("CFBundleIdentifier"), conf["identifier"].as_str().unwrap());
        assert_eq!(value("CFBundleName"), conf["productName"].as_str().unwrap());
        assert_eq!(value("CFBundleDisplayName"), conf["productName"].as_str().unwrap());
        assert_eq!(value("CFBundleExecutable"), Format::App.binary_name());
        let config = packager_toml();
        let icns = config["icons"].as_array().unwrap().iter().filter_map(Value::as_str).find(|i| i.ends_with(".icns")).unwrap();
        assert_eq!(value("CFBundleIconFile"), Path::new(icns).file_name().unwrap().to_str().unwrap(), "cargo-packager copies the .icns under its own name");
        let packager = fs::read_to_string(root().join(PACKAGER_TOML)).unwrap();
        assert_eq!(value("LSMinimumSystemVersion"), minimum_system_version(&packager).unwrap());

        // filled in: the version twice, nothing left over, the same keys
        let filled = info_plist(&text, "0.3.16").unwrap();
        assert!(!filled.contains("@@"));
        let entries = plist_entries(&filled).unwrap();
        assert_eq!(entries.len(), ours.len());
        assert_eq!(entries[..today.len()].iter().filter(|(k, v)| today.contains(&(k.clone(), v.clone()))).count(), 13, "all but the minimum system as today's");
        assert!(info_plist(&text, "1.0.0</string>").is_err());
        assert!(info_plist("<string>@@BUILD@@</string>", "1.0.0").is_err());
        assert!(plist_entries("<plist><dict><key>A</key><array/></dict></plist>").unwrap_err().contains("not a flat dictionary"));
        assert!(plist_entries("no plist").is_err());
    }

    /// `Packager.toml`'s macOS part and the dmg script's arguments: tauri-bundler 2.10.1's for today's
    /// configuration (the layout v0.3.16's dmg has in its .DS_Store: a 660 x 400 window, the app at
    /// (180, 170), Applications at (480, 170); the volume RustRadar with the app's icon), and no
    /// `--skip-jenkins`.
    #[test]
    fn the_macos_configuration_is_todays() {
        let config = packager_toml();
        assert_eq!(config["macos"]["minimum-system-version"], "11.0");
        assert_eq!(config["macos"]["info-plist-path"], INFO_PLIST);
        assert_eq!(config["dmg"], json!({ "window-size": { "width": 660, "height": 400 }, "app-position": { "x": 180, "y": 170 }, "application-folder-position": { "x": 480, "y": 170 } }));
        assert!(config["macos"].get("signing-identity").is_none(), "cargo-packager signs nothing: the ad hoc seal is sign_app's");
        let text = fs::read_to_string(root().join(PACKAGER_TOML)).unwrap();
        let (bin, out) = (Path::new("/w/bin"), Path::new("/w/out"));
        for format in Format::MACOS {
            let made = packager_config(&text, format, "0.3.16", bin, out, &[]).unwrap();
            assert_eq!(made["formats"], json!(["app"]));
            assert_eq!(made["binaries"], json!([{ "path": BINARY, "main": true }]));
        }
        let icns = Path::new("/repo/src-tauri/icons/icon.icns");
        let args = dmg_args(&config, icns, "RustRadar_0.3.16_aarch64.dmg").unwrap();
        let today = [
            "--volname", "RustRadar", "--icon", "RustRadar.app", "180", "170", "--app-drop-link", "480", "170", "--window-size", "660", "400",
            "--hide-extension", "RustRadar.app", "--volicon", "/repo/src-tauri/icons/icon.icns", "RustRadar_0.3.16_aarch64.dmg", "RustRadar.app",
        ];
        assert_eq!(args, today);
        // tauri-bundler's defaults without a [dmg] table; a window position as cargo-packager names it
        let mut bare = config.clone();
        bare.as_object_mut().unwrap().remove("dmg");
        assert_eq!(dmg_args(&bare, icns, "RustRadar_0.3.16_aarch64.dmg").unwrap(), today);
        let mut placed = config.clone();
        placed["dmg"]["windowPosition"] = json!({ "x": 10, "y": 60 });
        let args = dmg_args(&placed, icns, "x.dmg").unwrap();
        assert_eq!(&args[14..17], ["--window-pos", "10", "60"]);
        let mut background = config.clone();
        background["dmg"]["background"] = json!("bg.png");
        assert!(dmg_args(&background, icns, "x.dmg").unwrap_err().contains("[dmg] background"));
        let mut broken = config.clone();
        broken["dmg"]["app-position"] = json!({ "x": 1 });
        assert!(dmg_args(&broken, icns, "x.dmg").unwrap_err().contains("needs x and y"));
        // the volume's icon is the bundle's (v0.3.16's .VolumeIcon.icns is icon.icns)
        assert!(config["icons"].as_array().unwrap().contains(&json!("src-tauri/icons/icon.icns")));
        assert!(minimum_system_version("[macos]\n").is_err());
    }

    /// The dmg script and its AppleScript are tauri-bundler 2.10.1's (src/bundle/macos/dmg/bundle_dmg
    /// and template.applescript in its crate), byte for byte: the script that made v0.3.16's dmgs.
    #[test]
    fn the_dmg_script_is_tauri_bundlers() {
        let read = |file: &str| fs::read_to_string(root().join(file)).unwrap().replace("\r\n", "\n");
        let script = read(DMG_SCRIPT);
        assert_eq!(sha256_hex(script.as_bytes()), "dce473d510339f1a3f8009d84191ad2bf716d8cdbe12eb6e1f821771cf8f960f");
        assert_eq!(sha256_hex(read(DMG_APPLESCRIPT).as_bytes()), "13727c7930c50be639a1eb191e893f1ebc72c270268d4611391b41410a3e862a");
        for line in ["CDMG_VERSION='1.2.1'", "WINX=10", "WINY=60", "ICON_SIZE=128", "TEXT_SIZE=16", "FORMAT=\"UDZO\"", "FILESYSTEM=\"HFS+\"", "\tCDMG_SUPPORT_DIR=\"$prefix_dir/share/create-dmg/support\""] {
            assert!(script.lines().any(|l| l == line), "{line}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(fs::metadata(root().join(DMG_SCRIPT)).unwrap().permissions().mode() & 0o111, 0, "executable, as tauri-bundler makes it");
        }
    }

    /// tauri-bundler's script, laid out as `cargo xtask package dmg` lays it out, takes today's
    /// arguments: it finds its support files, knows every option, and asks `hdiutil` for today's image
    /// (HFS+, the bundle as the source, the volume RustRadar). `hdiutil` and `sw_vers` are stand-ins that
    /// stop it there; the rest of the script is macOS's (CI's dmg, a person's check).
    #[test]
    #[cfg(unix)]
    fn the_dmg_script_takes_todays_arguments() {
        let dir = std::env::temp_dir().join(format!("xtask-dmg-script-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let bin = dir.join("stand-ins");
        fs::create_dir_all(&bin).unwrap();
        let asked = dir.join("hdiutil-args");
        fs::write(bin.join("sw_vers"), "#!/bin/sh\nprintf 'ProductName:\\tmacOS\\nProductVersion:\\t14.5\\nBuildVersion:\\t23F79\\n'\n").unwrap();
        fs::write(bin.join("hdiutil"), format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 3\n", asked.display())).unwrap();
        for tool in ["sw_vers", "hdiutil"] {
            set_executable(&bin.join(tool)).unwrap();
        }
        let plist = info_plist(&fs::read_to_string(root().join(INFO_PLIST)).unwrap(), "0.3.16").unwrap();
        let app = a_bundle(&dir, &plist);
        let script = dmg_tools(&root(), &dir.join("dmg-tools")).unwrap();
        let args = dmg_args(&packager_toml(), &root().join("src-tauri/icons/icon.icns"), "RustRadar_0.3.16_aarch64.dmg").unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        let output = Command::new(&script).current_dir(&dir).args(&args).env("PATH", path).output().unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert_eq!(output.status.code(), Some(3), "stopped by the stand-in hdiutil: {said}");
        assert!(!said.contains("Unknown option") && !said.contains("Cannot find support"), "{said}");
        let asked = fs::read_to_string(&asked).unwrap();
        let asked: Vec<&str> = asked.lines().collect();
        let canonical = app.canonicalize().unwrap();
        assert_eq!(asked[..10], ["create", "-srcfolder", canonical.to_str().unwrap(), "-volname", "RustRadar", "-fs", "HFS+", "-fsargs", "-c c=64,a=16,e=16", "-format"]);
        assert_eq!(asked[10], "UDRW");
        assert!(asked[11].ends_with(".RustRadar_0.3.16_aarch64.dmg") && asked[11].contains("/rw."), "{}", asked[11]);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A bundle laid out as cargo-packager lays it out (`plist` its Info.plist), under `dir`.
    fn a_bundle(dir: &Path, plist: &str) -> PathBuf {
        let app = dir.join("RustRadar.app");
        for folder in ["Contents/MacOS", "Contents/Resources"] {
            fs::create_dir_all(app.join(folder)).unwrap();
        }
        fs::write(app.join("Contents/Info.plist"), plist).unwrap();
        fs::write(app.join("Contents/MacOS/rustradar"), mach_o("aarch64")).unwrap();
        set_executable(&app.join("Contents/MacOS/rustradar")).unwrap();
        fs::copy(root().join("src-tauri/icons/icon.icns"), app.join("Contents/Resources/icon.icns")).unwrap();
        app
    }

    /// The bundle is checked against today's layout and the filled-in Info.plist.
    #[test]
    fn an_app_bundle_is_checked() {
        let dir = std::env::temp_dir().join(format!("xtask-app-check-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let plist = info_plist(&fs::read_to_string(root().join(INFO_PLIST)).unwrap(), "0.3.16").unwrap();
        let app = a_bundle(&dir, &plist);
        check_app_bundle(&app, &plist, false).unwrap();
        // sealed: codesign's CodeResources is expected, and nothing else
        assert!(check_app_bundle(&app, &plist, true).unwrap_err().contains("today's bundle"));
        fs::create_dir_all(app.join("Contents/_CodeSignature")).unwrap();
        fs::write(app.join("Contents/_CodeSignature/CodeResources"), "<plist/>").unwrap();
        check_app_bundle(&app, &plist, true).unwrap();
        assert!(check_app_bundle(&app, &plist, false).is_err());
        fs::remove_dir_all(app.join("Contents/_CodeSignature")).unwrap();
        // a stray file, a missing icon, a value or key the bundle lacks, an executable that cannot run
        fs::write(app.join("Contents/Resources/extra.txt"), "x").unwrap();
        assert!(check_app_bundle(&app, &plist, false).unwrap_err().contains("extra.txt"));
        fs::remove_file(app.join("Contents/Resources/extra.txt")).unwrap();
        fs::rename(app.join("Contents/Resources/icon.icns"), dir.join("icon.icns")).unwrap();
        assert!(check_app_bundle(&app, &plist, false).is_err());
        fs::rename(dir.join("icon.icns"), app.join("Contents/Resources/icon.icns")).unwrap();
        fs::write(app.join("Contents/Info.plist"), plist.replace("<string>11.0</string>", "<string>10.13</string>")).unwrap();
        assert!(check_app_bundle(&app, &plist, false).unwrap_err().contains("LSMinimumSystemVersion `10.13`"));
        fs::write(app.join("Contents/Info.plist"), plist.replace("<string>0.3.16</string>", "<string>0.3.15</string>")).unwrap();
        assert!(check_app_bundle(&app, &plist, false).unwrap_err().contains("CFBundleShortVersionString `0.3.15`"));
        fs::write(app.join("Contents/Info.plist"), plist.replace("\t<key>NSPrincipalClass</key>\n\t<string>NSApplication</string>\n", "")).unwrap();
        assert!(check_app_bundle(&app, &plist, false).unwrap_err().contains("lacks NSPrincipalClass"));
        fs::write(app.join("Contents/Info.plist"), &plist).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(app.join("Contents/MacOS/rustradar"), fs::Permissions::from_mode(0o644)).unwrap();
            assert!(check_app_bundle(&app, &plist, false).unwrap_err().contains("not executable"));
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The updater's archive is tauri-bundler's: gzip as v0.3.16's (no name, no time, OS unknown), one
    /// top folder `RustRadar.app` holding today's files with their modes, which the Tauri updater
    /// (tauri-plugin-updater 2.13.2's macOS install) and the app's take with that folder dropped.
    #[test]
    fn the_updater_archive_is_tauri_bundlers() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("xtask-app-archive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let plist = info_plist(&fs::read_to_string(root().join(INFO_PLIST)).unwrap(), "0.3.16").unwrap();
        let app = a_bundle(&dir, &plist);
        let archive = dir.join("RustRadar.app.tar.gz");
        app_archive(&app, &archive).unwrap();
        let bytes = fs::read(&archive).unwrap();
        // v0.3.16's RustRadar_0.3.16_aarch64.app.tar.gz starts 1f8b 08 00 00000000 00 ff
        assert_eq!(&bytes[..10], [0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0xff]);
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
        let mut entries = Vec::new();
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let mode = entry.header().mode().unwrap();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            // what the updaters unpack it to: the path less its first part
            let unpacked: PathBuf = Path::new(&path).iter().skip(1).collect();
            entries.push((path, mode, unpacked, data));
        }
        assert_eq!(entries[0].0.trim_end_matches('/'), "RustRadar.app", "the bundle's folder first");
        assert!(entries.iter().all(|(p, ..)| p.starts_with("RustRadar.app")), "one top folder");
        let mut paths: Vec<String> = entries.iter().map(|(p, ..)| p.trim_end_matches('/').to_owned()).collect();
        paths.sort();
        // v0.3.16's archive lists these seven entries
        assert_eq!(paths, ["RustRadar.app", "RustRadar.app/Contents", "RustRadar.app/Contents/Info.plist", "RustRadar.app/Contents/MacOS", "RustRadar.app/Contents/MacOS/rustradar", "RustRadar.app/Contents/Resources", "RustRadar.app/Contents/Resources/icon.icns"]);
        let file = |name: &str| entries.iter().find(|(p, ..)| p == name).unwrap();
        let binary = file("RustRadar.app/Contents/MacOS/rustradar");
        assert_eq!(binary.1 & 0o777, 0o755);
        assert_eq!(binary.2, Path::new("Contents/MacOS/rustradar"));
        assert_eq!(binary.3, mach_o("aarch64"));
        assert_eq!(file("RustRadar.app/Contents/Info.plist").3, plist.as_bytes());
        assert_eq!(file("RustRadar.app/Contents/Resources/icon.icns").3, fs::read(root().join("src-tauri/icons/icon.icns")).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The `.app.tar.gz` of a stand-in Mach-O, built by cargo-packager on this machine: today's layout,
    /// the filled-in Info.plist, the marker as it was, the archive under today's name; the dmg is
    /// refused off macOS. Run by hand with cargo-packager installed (`cargo test -p xtask -- --ignored
    /// packs_an_app`); the sealed bundle and the dmg are CI's (macOS).
    #[test]
    #[ignore = "needs cargo-packager 0.11.8"]
    fn packs_an_app_from_a_stand_in() {
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("xtask-package-app-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let arch = std::env::consts::ARCH;
        let stand_in = dir.join("stand-in");
        fs::write(&stand_in, mach_o(arch)).unwrap();
        let out = dir.join("out");
        let args = |formats: &[&str], binary: &Path| -> Vec<String> {
            formats.iter().copied().chain(["--binary", binary.to_str().unwrap(), "--out", out.to_str().unwrap()]).map(str::to_owned).collect()
        };
        let elf = dir.join("elf");
        fs::write(&elf, [&b"\x7fELF"[..], &MARKER[..]].concat()).unwrap();
        assert!(run(&args(&["app"], &elf)).unwrap_err().contains("not a 64-bit macOS executable"));
        if !cfg!(target_os = "macos") {
            assert!(run(&args(&["dmg"], &stand_in)).unwrap_err().contains("made on macOS"));
        }

        run(&args(&["app"], &stand_in)).unwrap();
        let version = bump::current().unwrap();
        let archive = out.join(Format::App.asset_name(&version, arch).unwrap());
        assert!(archive.is_file(), "{}", archive.display());
        assert!(!out.join(format!("{}.sig", archive.file_name().unwrap().to_str().unwrap())).exists(), "no --sign, no .sig");
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(&archive).unwrap()));
        tar.unpack(dir.join("unpacked")).unwrap();
        let app = dir.join("unpacked/RustRadar.app");
        let plist = info_plist(&fs::read_to_string(root().join(INFO_PLIST)).unwrap(), &version).unwrap();
        check_app_bundle(&app, &plist, cfg!(target_os = "macos")).unwrap();
        let mut binary = Vec::new();
        fs::File::open(app.join("Contents/MacOS/rustradar")).unwrap().read_to_end(&mut binary).unwrap();
        if !cfg!(target_os = "macos") {
            assert_eq!(binary, mach_o(arch), "the binary as it was, its marker unpatched");
        }
        let entries = plist_entries(&fs::read_to_string(app.join("Contents/Info.plist")).unwrap()).unwrap();
        assert!(entries.contains(&("CFBundleVersion".to_owned(), version.clone())), "the release version, not cargo-packager's build time");
        fs::remove_dir_all(&dir).unwrap();
    }
}
