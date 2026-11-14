//! The app's build script. On Windows targets it embeds what tauri-build embedded in today's
//! `rustradar.exe` (plan stage 11, row P5; doc 07 §11.7, §13.1): the icon (`src-tauri/icons/icon.ico`
//! as icon group 32512), the version information (from this crate's version, which `cargo xtask bump`
//! writes: item F5) and the application manifest (Common Controls 6, per-monitor-v2 DPI awareness, the
//! invoker's execution level), linked into the binaries only. The resources themselves are
//! `platform/windows/resources.rs`'s. For MSVC it also links the Visual C++ runtime statically, as
//! tauri-build does by default (`build.windows.staticVCRuntime`, its `static_vcruntime.rs`): today's
//! executable imports no `vcruntime140.dll`, only the Universal CRT that every Windows 10 has, so it
//! starts without the Visual C++ Redistributable installed (doc 07 §13.1 asked to decide this; the
//! answer is today's). The C++ dependencies (basisu) compiled `/MD` still import `msvcp140.dll` from
//! the redistributable, so the release build is made with `-C target-feature=+crt-static`, which links
//! every part's runtime statically; the hybrid is then left out (`cargo xtask package nsis msi`
//! refuses an executable that imports the redistributable's DLLs).
//!
//! On every other target it does nothing, so Linux, Android and iOS builds compile no resource tool:
//! `embed-resource` (the crate tauri-winres compiles with) is a build dependency on Windows hosts only.
//! A Windows target built from another host (a cross check) compiles the script with `llvm-rc` when
//! there is one and the target is MSVC's, else leaves the resources out with a warning; the packages
//! are made on Windows, and `cargo xtask package nsis msi` refuses an executable without them.

#[path = "../../platform/windows/resources.rs"]
mod resources;

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../platform/windows/resources.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
        // with `+crt-static` rustc links the whole C runtime statically already (and the C and C++
        // dependencies follow it, which the hybrid below cannot make them do)
        let crt_static = env::var("CARGO_CFG_TARGET_FEATURE").is_ok_and(|f| f.split(',').any(|f| f == "crt-static"));
        if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") && !crt_static {
            static_vc_runtime();
        }
    }
}

/// tauri-build 2.7.1's `static_vcruntime::build` (from ChrisDenton's static_vcruntime): the static
/// vcruntime and C runtime start-up (libvcruntime, libcmt) with the dynamic Universal CRT (ucrt), and an
/// empty `msvcrt.lib` first on the library path in place of the one rustc names.
fn static_vc_runtime() {
    let machine: &[u8] = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => &[0x64, 0x86],
        Ok("x86") => &[0x4C, 0x01],
        _ => return,
    };
    let bytes: &[u8] = &[
        1, 0, 94, 3, 96, 98, 60, 0, 0, 0, 1, 0, 0, 0, 0, 0, 132, 1, 46, 100, 114, 101, 99, 116, 118, 101, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 16, 0, 46, 100, 114, 101, 99, 116, 118, 101, 0,
        0, 0, 0, 1, 0, 0, 0, 3, 0, 4, 0, 0, 0,
    ];
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("msvcrt.lib"), [machine, bytes].concat()).unwrap_or_else(|e| panic!("msvcrt.lib: {e}"));
    println!("cargo:rustc-link-search=native={}", out.display());
    for lib in ["libvcruntimed.lib", "vcruntime.lib", "vcruntimed.lib", "libcmtd.lib", "msvcrt.lib", "msvcrtd.lib", "libucrt.lib", "libucrtd.lib"] {
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:{lib}");
    }
    for lib in ["libcmt.lib", "libvcruntime.lib", "ucrt.lib"] {
        println!("cargo:rustc-link-arg=/DEFAULTLIB:{lib}");
    }
}

/// Writes the manifest and the resource script into `OUT_DIR` and has them compiled and linked.
fn windows_resources() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    // tauri.conf.json's bundle.icon `.ico` (item D2 moves the icons: this path moves with them)
    let icon = crate_dir.join("../../src-tauri/icons/icon.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let icon = icon.canonicalize().unwrap_or_else(|e| panic!("{}: {e}", icon.display()));
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");

    let manifest = out.join("rustradar.manifest");
    write(&manifest, resources::MANIFEST);
    let script = resources::resource_script(&version, &text(&icon), &text(&manifest)).unwrap_or_else(|e| panic!("{e}"));
    let rc = out.join("rustradar.rc");
    write(&rc, &script);
    compile(&rc, &out);
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// A path as the resource script names it (without the `\\?\` that `canonicalize` puts on Windows).
fn text(path: &Path) -> String {
    let path = path.to_str().unwrap_or_else(|| panic!("{}: not UTF-8", path.display()));
    path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
}

/// On a Windows host: `embed-resource`, which finds the Windows SDK's `rc.exe` (or MinGW's windres);
/// a resource that does not compile fails the build.
#[cfg(windows)]
fn compile(rc: &Path, _out: &Path) {
    embed_resource::compile(rc, embed_resource::NONE)
        .manifest_required()
        .unwrap_or_else(|e| panic!("the Windows resources (icon, version information, manifest): {e}"));
}

/// On another host: `llvm-rc` for an MSVC target, whose linker takes the `.res` as it is (the script
/// includes nothing, so it is not preprocessed).
#[cfg(not(windows))]
fn compile(rc: &Path, out: &Path) {
    let res = out.join("rustradar.res");
    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let compiled = msvc
        && std::process::Command::new("llvm-rc")
            .args(["--no-preprocess", "/FO"])
            .arg(&res)
            .arg(rc)
            .status()
            .is_ok_and(|status| status.success());
    if compiled {
        println!("cargo:rustc-link-arg-bins={}", res.display());
    } else {
        println!(
            "cargo:warning=the Windows resources (icon, version information, manifest) are left out of this cross build: \
             compile it on Windows, or for an MSVC target with llvm-rc on the PATH"
        );
    }
}
