//! `cargo xtask package <format>...`: the desktop packages (plan stage 11, rows P5–P7; doc 07 §13).
//!
//! ```text
//! cargo xtask package deb rpm appimage [--target TRIPLE] [--binary FILE] [--out DIR] [--sign]
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
//! **For P6** (macOS): its formats become variants of [`Format`] with their marker and today's asset
//! names (`RustRadar_0.3.16_{x64,aarch64}.app.tar.gz` and `.dmg`), and their tables go into
//! `Packager.toml`; [`packager_config`], the build, the patch and the move to `--out` are the same.

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
}

impl Format {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const LINUX: [Format; 3] = [Format::Deb, Format::Rpm, Format::AppImage];
    pub const WINDOWS: [Format; 2] = [Format::Nsis, Format::Msi];
    pub const ALL: [Format; 5] = [Format::Deb, Format::Rpm, Format::AppImage, Format::Nsis, Format::Msi];

    /// Its name on the command line (cargo-packager's, for those it packs, but `msi` for its `wix`).
    pub fn name(self) -> &'static str {
        match self {
            Format::Deb => "deb",
            Format::Rpm => "rpm",
            Format::AppImage => "appimage",
            Format::Nsis => "nsis",
            Format::Msi => "msi",
        }
    }

    /// Its name in cargo-packager's `formats`.
    pub fn packager_name(self) -> &'static str {
        match self {
            Format::Msi => "wix",
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

    /// What tauri-bundler's `patch_binary` writes over [`MARKER`] for this package.
    pub fn marker(self) -> &'static [u8; 27] {
        match self {
            Format::Deb => b"__TAURI_BUNDLE_TYPE_VAR_DEB",
            Format::Rpm => b"__TAURI_BUNDLE_TYPE_VAR_RPM",
            Format::AppImage => b"__TAURI_BUNDLE_TYPE_VAR_APP",
            Format::Nsis => b"__TAURI_BUNDLE_TYPE_VAR_NSS",
            Format::Msi => b"__TAURI_BUNDLE_TYPE_VAR_MSI",
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
    /// Windows' names), for `arch` as Rust names it.
    pub fn asset_name(self, version: &str, arch: &str) -> Result<String, String> {
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

/// The command line.
#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub formats: Vec<Format>,
    pub target: Option<String>,
    pub binary: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub sign: bool,
}

pub const USAGE: &str = "usage: cargo xtask package <deb|rpm|appimage|nsis|msi>... [--target TRIPLE] [--binary FILE] [--out DIR] [--sign]";

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
    let mut patched = binary.to_vec();
    patch_marker(&mut patched, format.marker())?;
    let staged = bin_dir.join(format.binary_name());
    fs::write(&staged, &patched).map_err(|e| format!("{}: {e}", staged.display()))?;
    set_executable(&staged)?;

    match format {
        Format::Deb | Format::AppImage | Format::Nsis | Format::Msi => {
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
    if args.formats.iter().any(|f| matches!(f, Format::Deb | Format::AppImage | Format::Nsis | Format::Msi)) {
        check_tool("packager", PACKAGER)?;
    }
    if args.formats.contains(&Format::Msi) && !cfg!(windows) {
        return Err("the MSI is made on Windows (cargo-packager runs WiX 3 there only)".to_owned());
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
        made.push(package.display().to_string());
    }
    let _ = fs::remove_dir(target_dir.join("package-work"));
    if args.sign {
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
        for format in Format::ALL {
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
}
