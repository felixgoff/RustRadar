//! `cargo xtask vendor-icons`: write one Lucide-format SVG per icon of doc 06 §26 plus the ISC
//! licence to `assets/icons/lucide` (S1.31).
//!
//! The source is the package the web build imports, `node_modules/@lucide/svelte`: each icon's
//! component `dist/icons/<file>.svelte` holds its geometry as JSON (`const iconData = { "name",
//! "size", "node": [[tag, {attributes}], …] }`). The SVG written for it is the one Lucide's
//! `Icon.svelte` renders with its default attributes (`dist/utils/defaultAttributes.js`): 24×24
//! viewBox, no fill, `stroke="currentColor"`, `stroke-width="2"`, round caps and joins, the
//! children in the order and with the attributes of `node`. This is also the layout of the SVG
//! files in the Lucide repository. Run `bun install --frozen-lockfile` first if `node_modules`
//! is missing.
//!
//! `ui::icons` (U2) substitutes the root `stroke-width="2"` (the only one in each file) with the
//! stroke width of the use (1.75, 2 or 2.5) and draws `currentColor` as its mask colour.
//!
//! Output, all generated (re-running gives identical bytes):
//! - `<file>.svg` for each icon of [`ICONS`], `<file>` being Lucide's kebab-case name;
//! - `LICENSE`, the package's licence (ISC, and MIT for the icons derived from Feather).
//!
//! Other `.svg` files in the folder are removed, so the folder holds exactly [`ICONS`].

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;

/// The icons of doc 06 §26, in its order: the component name the UI imports from
/// `@lucide/svelte` and the file stem of that component in `dist/icons` (also the vendored SVG's
/// stem). `PlaneLanding` appears twice in doc 06 §26 and once here.
pub const ICONS: [(&str, &str); 37] = [
    ("Radar", "radar"),
    ("Funnel", "funnel"),
    ("RefreshCw", "refresh-cw"),
    ("TriangleAlert", "triangle-alert"),
    ("Plane", "plane"),
    ("TrainFront", "train-front"),
    ("Ship", "ship"),
    ("Eye", "eye"),
    ("Search", "search"),
    ("LoaderCircle", "loader-circle"),
    ("X", "x"),
    ("MapPin", "map-pin"),
    ("House", "house"),
    ("Signpost", "signpost"),
    ("Landmark", "landmark"),
    ("PlaneLanding", "plane-landing"),
    ("Building", "building"),
    ("Mailbox", "mailbox"),
    ("Map", "map"),
    ("Flag", "flag"),
    ("ArrowLeft", "arrow-left"),
    ("ChevronLeft", "chevron-left"),
    ("ChevronRight", "chevron-right"),
    ("ExternalLink", "external-link"),
    ("LocateFixed", "locate-fixed"),
    ("LockKeyhole", "lock-keyhole"),
    ("PlaneTakeoff", "plane-takeoff"),
    ("ArrowRight", "arrow-right"),
    ("Check", "check"),
    ("Layers", "layers"),
    ("Settings", "settings"),
    ("KeyRound", "key-round"),
    ("Keyboard", "keyboard"),
    ("Plus", "plus"),
    ("Minus", "minus"),
    ("Rotate3d", "rotate-3d"),
    ("Navigation2", "navigation-2"),
];

/// The licence file written next to the icons.
pub const LICENSE: &str = "LICENSE";

/// The repository root: `crates/xtask` is two levels below it.
fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}

fn source_dir() -> PathBuf {
    repo_root().join("node_modules/@lucide/svelte")
}

fn out_dir() -> PathBuf {
    repo_root().join("assets/icons/lucide")
}

/// The vendored file name of an icon, e.g. `refresh-cw.svg`.
pub fn svg_name(file: &str) -> String {
    format!("{file}.svg")
}

pub fn run(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err(format!("vendor-icons takes no arguments, got {args:?}"));
    }
    let out = out_dir();
    let files = vendored_files(&source_dir())?;
    fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let entries = fs::read_dir(&out).map_err(|e| format!("cannot list {}: {e}", out.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("cannot list {}: {e}", out.display()))?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let stale = path.extension().is_some_and(|x| x == "svg") && !files.iter().any(|(n, _)| n == name);
        if stale {
            fs::remove_file(&path).map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
            println!("removed {}", path.display());
        }
    }
    for (name, text) in &files {
        let path = out.join(name);
        fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        println!("wrote {} ({} bytes)", path.display(), text.len());
    }
    Ok(())
}

/// Every file of the vendored folder, `(file name, contents)`: the SVGs in the order of
/// [`ICONS`], then the licence. Each SVG is checked to parse with resvg before it is returned.
fn vendored_files(source: &Path) -> Result<Vec<(String, String)>, String> {
    let read = |path: PathBuf| {
        fs::read_to_string(&path).map_err(|e| {
            format!("cannot read {}: {e} (run `bun install --frozen-lockfile` first)", path.display())
        })
    };
    let package: serde_json::Value = serde_json::from_str(&read(source.join("package.json"))?)
        .map_err(|e| format!("package.json of @lucide/svelte: {e}"))?;
    let version = package["version"].as_str().ok_or("package.json of @lucide/svelte has no version")?;
    let index = read(source.join("dist/icons/index.js"))?;
    let mut files = Vec::with_capacity(ICONS.len() + 1);
    for (component, file) in ICONS {
        let export = format!("export {{ default as {component} }} from './{file}.svelte';");
        if !index.contains(&export) {
            return Err(format!("dist/icons/index.js of @lucide/svelte {version} does not have `{export}`"));
        }
        let svelte = read(source.join(format!("dist/icons/{file}.svelte")))?;
        let data = icon_data(&svelte).map_err(|e| format!("{file}.svelte: {e}"))?;
        if data.name != file {
            return Err(format!("{file}.svelte: iconData.name is `{}`", data.name));
        }
        let svg = to_svg(&data, version);
        parse(&svg).map_err(|e| format!("{file}.svg does not parse with resvg: {e}"))?;
        files.push((svg_name(file), svg));
    }
    files.push((LICENSE.to_string(), read(source.join("LICENSE"))?));
    Ok(files)
}

/// The `iconData` constant of a Lucide Svelte component.
#[derive(Debug, Deserialize)]
struct IconData {
    name: String,
    /// The viewBox's width and height.
    size: u32,
    /// The children of `<svg>`: tag and attributes in the order Lucide wrote them.
    node: Vec<(String, IndexMap<String, serde_json::Value>)>,
}

/// Reads the JSON object after `const iconData = ` in a component's source.
fn icon_data(svelte: &str) -> Result<IconData, String> {
    const MARK: &str = "const iconData = ";
    let start = svelte.find(MARK).ok_or("no `const iconData = `")? + MARK.len();
    let mut values = serde_json::Deserializer::from_str(&svelte[start..]).into_iter::<IconData>();
    match values.next() {
        Some(Ok(data)) => Ok(data),
        Some(Err(e)) => Err(format!("iconData: {e}")),
        None => Err("iconData is empty".into()),
    }
}

/// An attribute value as Lucide renders it: strings as they are, numbers in JavaScript's
/// `String(n)` form (serde_json prints integers without `.0`, as JavaScript does).
fn attribute_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The Lucide-format SVG of an icon: Lucide's default root attributes, one child per line.
fn to_svg(data: &IconData, version: &str) -> String {
    let size = data.size;
    let mut svg = format!(
        "<!-- @license @lucide/svelte v{version} - ISC -->\n\
         <svg\n  \
         xmlns=\"http://www.w3.org/2000/svg\"\n  \
         width=\"{size}\"\n  \
         height=\"{size}\"\n  \
         viewBox=\"0 0 {size} {size}\"\n  \
         fill=\"none\"\n  \
         stroke=\"currentColor\"\n  \
         stroke-width=\"2\"\n  \
         stroke-linecap=\"round\"\n  \
         stroke-linejoin=\"round\"\n\
         >\n"
    );
    for (tag, attributes) in &data.node {
        svg.push_str("  <");
        svg.push_str(tag);
        for (name, value) in attributes {
            svg.push_str(&format!(" {name}=\"{}\"", escape(&attribute_value(value))));
        }
        svg.push_str(" />\n");
    }
    svg.push_str("</svg>\n");
    svg
}

fn parse(svg: &str) -> Result<resvg::usvg::Tree, String> {
    resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The icon names of doc 06 §26: every backticked name in the first column of its table.
    fn doc_06_icons() -> BTreeSet<String> {
        let doc = fs::read_to_string(repo_root().join("docs/port/06-ui-spec.md")).unwrap();
        let start = doc.find("\n## 26. Icon inventory").expect("doc 06 has §26");
        let section = &doc[start + 1..];
        let section = &section[..section[1..].find("\n## ").map_or(section.len(), |end| end + 1)];
        let mut names = BTreeSet::new();
        for row in section.lines().filter(|l| l.starts_with("| `")) {
            let first = row.split('|').nth(1).unwrap();
            names.extend(first.split('`').skip(1).step_by(2).map(str::to_string));
        }
        names
    }

    fn vendored(name: &str) -> String {
        let path = out_dir().join(name);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn icons_are_the_icons_of_doc_06() {
        let doc: BTreeSet<String> = doc_06_icons();
        let ours: BTreeSet<String> = ICONS.iter().map(|(c, _)| c.to_string()).collect();
        assert_eq!(ours.len(), ICONS.len(), "an icon is listed twice");
        assert_eq!(doc.len(), 37);
        assert_eq!(ours, doc);
    }

    #[test]
    fn every_icon_of_doc_06_has_a_file() {
        for name in doc_06_icons() {
            let (_, file) = ICONS.iter().find(|(c, _)| *c == name).unwrap();
            let path = out_dir().join(svg_name(file));
            assert!(path.is_file(), "{name}: {} is missing", path.display());
        }
        assert!(vendored(LICENSE).starts_with("ISC License"));
    }

    #[test]
    fn every_vendored_icon_parses_with_resvg() {
        for (component, file) in ICONS {
            let svg = vendored(&svg_name(file));
            let tree = parse(&svg).unwrap_or_else(|e| panic!("{component}: {e}"));
            assert_eq!(tree.size(), resvg::usvg::Size::from_wh(24.0, 24.0).unwrap(), "{component}");
            assert!(tree.root().has_children(), "{component} is empty");
            // U2 swaps the stroke width with one replacement.
            assert_eq!(svg.matches("stroke-width=").count(), 1, "{component}");
            assert_eq!(svg.matches("stroke-width=\"2\"").count(), 1, "{component}");
            // And it draws: ink inside the 24×24 box at 1.75 px.
            let thin = svg.replace("stroke-width=\"2\"", "stroke-width=\"1.75\"");
            let tree = parse(&thin).unwrap();
            let mut pixmap = resvg::tiny_skia::Pixmap::new(24, 24).unwrap();
            resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
            let ink = pixmap.pixels().iter().filter(|p| p.alpha() > 0).count();
            assert!(ink > 10, "{component} draws {ink} pixels");
        }
    }

    #[test]
    fn the_folder_holds_only_the_icons() {
        let mut svgs: Vec<String> = fs::read_dir(out_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".svg"))
            .collect();
        svgs.sort();
        let mut expected: Vec<String> = ICONS.iter().map(|(_, f)| svg_name(f)).collect();
        expected.sort();
        assert_eq!(svgs, expected);
    }

    #[test]
    fn svg_has_lucide_layout() {
        let data = icon_data(
            "<script lang=\"ts\">const iconData = { \"name\": \"mailbox\", \"size\": 24, \"node\": \
             [[\"polyline\", { \"points\": \"15,9 18,9 18,11\" }], [\"line\", { \"x1\": \"6\", \"x2\": 7, \
             \"y1\": \"10\", \"y2\": \"10\" }]], \"aliases\": [\"m\"] };\n</script>",
        )
        .unwrap();
        assert_eq!(
            to_svg(&data, "1.54.0"),
            "<!-- @license @lucide/svelte v1.54.0 - ISC -->\n<svg\n  xmlns=\"http://www.w3.org/2000/svg\"\n  \
             width=\"24\"\n  height=\"24\"\n  viewBox=\"0 0 24 24\"\n  fill=\"none\"\n  stroke=\"currentColor\"\n  \
             stroke-width=\"2\"\n  stroke-linecap=\"round\"\n  stroke-linejoin=\"round\"\n>\n  \
             <polyline points=\"15,9 18,9 18,11\" />\n  <line x1=\"6\" x2=\"7\" y1=\"10\" y2=\"10\" />\n</svg>\n"
        );
    }

    /// The vendored files are what `cargo xtask vendor-icons` writes today. Skipped once
    /// `node_modules` is gone (stage 12).
    #[test]
    fn vendored_files_match_node_modules() {
        let source = source_dir();
        if !source.join("package.json").is_file() {
            eprintln!("skipped: {} is missing", source.display());
            return;
        }
        for (name, text) in vendored_files(&source).unwrap() {
            assert_eq!(vendored(&name), text, "{name} is out of date: run `cargo xtask vendor-icons`");
        }
    }
}
