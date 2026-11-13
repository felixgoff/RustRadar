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
//! **For P5 and P6** (Windows, macOS): their formats become variants of [`Format`] with their marker
//! and today's asset names (`RustRadar_0.3.16_x64-setup.exe` and `RustRadar_0.3.16_x64_en-US.msi`, the
//! NSIS and WiX packers of `Packager.toml`; `RustRadar_0.3.16_{x64,aarch64}.app.tar.gz` and `.dmg`), and
//! their tables go into `Packager.toml`; [`packager_config`], the build, the patch and the move to
//! `--out` are the same.

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

/// A package this makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Deb,
    Rpm,
    AppImage,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Deb, Format::Rpm, Format::AppImage];

    /// Its name on the command line (cargo-packager's, for those it packs).
    pub fn name(self) -> &'static str {
        match self {
            Format::Deb => "deb",
            Format::Rpm => "rpm",
            Format::AppImage => "appimage",
        }
    }

    pub fn parse(name: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.name() == name)
    }

    /// What tauri-bundler's `patch_binary` writes over [`MARKER`] for this package.
    pub fn marker(self) -> &'static [u8; 27] {
        match self {
            Format::Deb => b"__TAURI_BUNDLE_TYPE_VAR_DEB",
            Format::Rpm => b"__TAURI_BUNDLE_TYPE_VAR_RPM",
            Format::AppImage => b"__TAURI_BUNDLE_TYPE_VAR_APP",
        }
    }

    /// The ending of the file the packer writes.
    fn extension(self) -> &'static str {
        match self {
            Format::Deb => ".deb",
            Format::Rpm => ".rpm",
            Format::AppImage => ".AppImage",
        }
    }

    /// The package's name as today's release names it (tauri-bundler 2.10.1: the deb and the AppImage
    /// `{product}_{version}_{arch}` with Debian's architecture names, the rpm
    /// `{product}-{version}-{release}.{arch}` with RPM's), for `arch` as Rust names it.
    pub fn asset_name(self, version: &str, arch: &str) -> Result<String, String> {
        let (deb, rpm, appimage) = match arch {
            "x86_64" => ("amd64", "x86_64", "amd64"),
            "aarch64" => ("arm64", "aarch64", "aarch64"),
            other => return Err(format!("no Linux package is made for {other} (x86_64 is shipped, aarch64 named as Tauri names it)")),
        };
        Ok(match self {
            Format::Deb => format!("RustRadar_{version}_{deb}.deb"),
            Format::Rpm => format!("RustRadar-{version}-1.{rpm}.rpm"),
            Format::AppImage => format!("RustRadar_{version}_{appimage}.AppImage"),
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
    object.insert("formats".into(), json!([format.name()]));
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

/// The command line.
#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub formats: Vec<Format>,
    pub target: Option<String>,
    pub binary: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub sign: bool,
}

pub const USAGE: &str = "usage: cargo xtask package <deb|rpm|appimage>... [--target TRIPLE] [--binary FILE] [--out DIR] [--sign]";

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
    Ok(dir.join("release").join(&package))
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
    let mut patched = binary.to_vec();
    patch_marker(&mut patched, format.marker())?;
    let staged = bin_dir.join(BINARY);
    fs::write(&staged, &patched).map_err(|e| format!("{}: {e}", staged.display()))?;
    set_executable(&staged)?;

    match format {
        Format::Deb | Format::AppImage => {
            let text = fs::read_to_string(root.join(PACKAGER_TOML)).map_err(|e| format!("{PACKAGER_TOML}: {e}"))?;
            let depends: Vec<String> = match (format, glibc_needed(binary)) {
                (Format::Deb, Some(v)) => vec![format!("libc6 (>= {})", v.iter().map(u32::to_string).collect::<Vec<_>>().join("."))],
                _ => Vec::new(),
            };
            let config = packager_config(&text, format, version, &bin_dir, &out_dir, &depends)?;
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
    check_tool("packager", PACKAGER)?;
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
        for (format, installer) in [(Format::Deb, "Deb"), (Format::Rpm, "Rpm"), (Format::AppImage, "AppImage")] {
            let marker = std::str::from_utf8(format.marker()).unwrap();
            assert_eq!(marker.len(), MARKER.len());
            assert!(reader.contains(&format!("\"{marker}\" => Some(Installer::{installer}),")), "{marker}");
        }
    }

    /// The names of v0.3.16's Linux assets, which `latest-json` reads back as its keys.
    #[test]
    fn the_packages_have_todays_names() {
        let names: Vec<String> = Format::ALL.iter().map(|f| f.asset_name("0.3.16", "x86_64").unwrap()).collect();
        assert_eq!(names, ["RustRadar_0.3.16_amd64.deb", "RustRadar-0.3.16-1.x86_64.rpm", "RustRadar_0.3.16_amd64.AppImage"]);
        for (name, bundle) in names.iter().zip(["deb", "rpm", "appimage"]) {
            assert_eq!(crate::manifest::bundle(name), Some(("linux", bundle)), "{name}");
            assert_eq!(crate::manifest::arch(name), Some("x86_64"), "{name}");
        }
        // aarch64 as Tauri's bundler names it (not shipped today)
        let arm: Vec<String> = Format::ALL.iter().map(|f| f.asset_name("1.2.3", "aarch64").unwrap()).collect();
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
        assert_eq!(parsed.formats, Format::ALL);
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
}
