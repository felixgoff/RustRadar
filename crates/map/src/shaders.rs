//! WGSL sources and the loader (plan 1.4.6): plain concatenation, no `naga_oil`.
//!
//! A shader names the libraries it needs on lines of their own, before its own code:
//!
//! ```wgsl
//! //!include lib/project.wgsl
//! ```
//!
//! [`expand`] replaces each such line with the library's source (paths relative to
//! `crates/map/src/shaders/`), depth first, each library once. The line is a WGSL comment, so a
//! shader file stays readable on its own. Libraries are compiled into the binary through
//! [`LIBRARIES`]; a shader embeds its own source with `include_str!` in the pipeline file that uses
//! it and hands it to [`module`]. A new library adds one line to [`LIBRARIES`] (the test
//! `all_shaders_validate` fails until it does).

use std::borrow::Cow;
use std::fmt;

/// The directive that pulls a library in.
pub const INCLUDE: &str = "//!include ";

/// `lib/project.wgsl`: the projection (plan 1.4.2), `globals` at group 0, `frame` at group 1.
pub const PROJECT: &str = include_str!("shaders/lib/project.wgsl");

/// `composite.wgsl`: the map texture onto the surface.
pub const COMPOSITE: &str = include_str!("shaders/composite.wgsl");

/// Every library a shader may include, by its path under `shaders/`.
pub const LIBRARIES: &[(&str, &str)] = &[("lib/project.wgsl", PROJECT)];

/// A shader that names a library nobody has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownLibrary(pub String);

impl fmt::Display for UnknownLibrary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no WGSL library {:?}", self.0)
    }
}

impl std::error::Error for UnknownLibrary {}

/// `source` with its `//!include` lines replaced by the libraries of [`LIBRARIES`].
pub fn expand(source: &str) -> Result<String, UnknownLibrary> {
    expand_with(source, |path| LIBRARIES.iter().find(|(p, _)| *p == path).map(|(_, s)| Cow::Borrowed(*s)))
}

/// `source` with its `//!include` lines replaced by what `library` gives for each path (the test
/// reads them from disk).
pub fn expand_with<'a>(
    source: &str,
    mut library: impl FnMut(&str) -> Option<Cow<'a, str>>,
) -> Result<String, UnknownLibrary> {
    let mut out = String::with_capacity(source.len());
    let mut included = Vec::new();
    expand_into(source, &mut library, &mut included, &mut out)?;
    Ok(out)
}

fn expand_into<'a>(
    source: &str,
    library: &mut impl FnMut(&str) -> Option<Cow<'a, str>>,
    included: &mut Vec<String>,
    out: &mut String,
) -> Result<(), UnknownLibrary> {
    for line in source.lines() {
        match line.trim_end().strip_prefix(INCLUDE) {
            Some(path) => {
                let path = path.trim();
                if included.iter().any(|p| p == path) {
                    continue;
                }
                included.push(path.to_string());
                let text = library(path).ok_or_else(|| UnknownLibrary(path.to_string()))?;
                out.push_str("// ---- ");
                out.push_str(path);
                out.push_str(" ----\n");
                expand_into(&text, library, included, out)?;
                out.push_str("// ---- end of ");
                out.push_str(path);
                out.push_str(" ----\n");
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    Ok(())
}

/// A shader module from `source` with its libraries.
///
/// # Panics
/// If `source` includes a library that is not in [`LIBRARIES`]: sources are compile-time constants
/// and `all_shaders_validate` checks every file, so this is a programming error.
pub fn module(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    let code = expand(source).unwrap_or_else(|e| panic!("shader {label}: {e}"));
    device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(code.into()) })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use wgpu::naga;

    fn shader_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shaders")
    }

    fn wgsl_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).expect("shader folder").map(|e| e.expect("entry").path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                wgsl_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "wgsl") {
                out.push(path);
            }
        }
    }

    /// Parses and validates `source` (already expanded), as wgpu would on a device with no
    /// optional feature.
    pub(crate) fn validate(name: &str, source: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{name}:\n{}", e.emit_to_string_with_path(source, name)));
        let mut validator = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty());
        if let Err(e) = validator.validate(&module) {
            panic!("{name}:\n{}", e.emit_to_string_with_path(source, name));
        }
        module
    }

    /// Every `.wgsl` file under `src/shaders`, libraries included, parses and validates with naga
    /// once its includes are expanded from the files on disk; every library on disk is also the one
    /// compiled in through [`LIBRARIES`].
    #[test]
    fn all_shaders_validate() {
        let dir = shader_dir();
        let mut files = Vec::new();
        wgsl_files(&dir, &mut files);
        assert!(files.len() >= 2, "found only {files:?}");
        for path in &files {
            let name = path.strip_prefix(&dir).expect("under the shader folder").to_string_lossy().replace('\\', "/");
            let source = std::fs::read_to_string(path).expect("readable");
            let expanded = expand_with(&source, |lib| std::fs::read_to_string(dir.join(lib)).ok().map(Cow::Owned))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            validate(&name, &expanded);
            if name.starts_with("lib/") {
                let compiled = LIBRARIES.iter().find(|(p, _)| *p == name).unwrap_or_else(|| panic!("{name} is not in LIBRARIES"));
                assert_eq!(compiled.1, source, "{name}: the compiled-in copy differs from the file");
            }
        }
        // and the runtime path agrees with the disk path for the shader compiled in here
        validate("composite.wgsl", &expand(COMPOSITE).expect("libraries known"));
    }

    #[test]
    fn includes_expand_once_and_unknown_ones_fail() {
        let main = "//!include lib/a.wgsl\n//!include lib/b.wgsl\nfn main_fn() {}\n";
        let lib = |path: &str| match path {
            "lib/a.wgsl" => Some(Cow::Borrowed("const A: f32 = 1.0;\n")),
            "lib/b.wgsl" => Some(Cow::Borrowed("//!include lib/a.wgsl\nconst B: f32 = A;\n")),
            _ => None,
        };
        let out = expand_with(main, lib).expect("known");
        assert_eq!(out.matches("const A").count(), 1);
        assert!(out.find("const A").expect("a") < out.find("const B").expect("b"));
        assert!(out.ends_with("fn main_fn() {}\n"));
        assert_eq!(expand("//!include lib/nope.wgsl\n"), Err(UnknownLibrary("lib/nope.wgsl".into())));
        // a shader without includes is unchanged
        assert_eq!(expand("fn f() {}\n").expect("plain"), "fn f() {}\n");
    }
}
