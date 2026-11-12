//! The files the app embeds (plan 1.6), named in one place: the UI's fonts and the Lucide icons
//! of `assets/` (vendored by `cargo xtask vendor-fonts` and `vendor-icons`, plan 1.5).
//!
//! - [`FONTS`]: Overpass and Overpass Mono as variable TTFs (`wght` 100–900, default 400; Mono
//!   300–700, default 300), one file per subset, in the fallback order the fonts' CSS gives them
//!   (`@fontsource-variable/overpass`'s `unicode-range`s: latin, latin-ext, cyrillic, cyrillic-ext,
//!   vietnamese). [`crate::ui::fonts`] makes the egui families of them. Overpass's latin subset
//!   also comes as Overpass with tabular figures (`overpass-tnum-latin.ttf`, item F1), for
//!   `font-variant-numeric: tabular-nums`.
//! - [`ICONS`]: the Lucide icons in use (doc 06 §26), as Lucide's SVG files, for `ui::icons`
//!   (U2) to rasterise. An icon added to `assets/icons/lucide/` gets a line here (a test checks
//!   the two agree).
//!
//! Licences: `assets/fonts/OFL.txt` (SIL Open Font License 1.1) and
//! `assets/icons/lucide/LICENSE` (ISC), both also embedded ([`FONTS_LICENSE`], [`ICONS_LICENSE`])
//! for an about box.

/// Which of the two font families a file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFile {
    /// Overpass (`--font-ui`).
    Overpass,
    /// Overpass Mono (`--font-data`).
    OverpassMono,
    /// Overpass with its `tnum` figures as the default ones (latin only: the other subsets have
    /// no figures).
    OverpassTabular,
}

/// One vendored font file: its family, its subset and its bytes.
#[derive(Debug, Clone, Copy)]
pub struct Font {
    pub file: FontFile,
    /// `latin`, `latin-ext`, `cyrillic`, `cyrillic-ext` or `vietnamese`.
    pub subset: &'static str,
    pub bytes: &'static [u8],
}

macro_rules! font {
    ($file:ident, $subset:literal, $path:literal) => {
        Font { file: FontFile::$file, subset: $subset, bytes: include_bytes!(concat!("../../../assets/fonts/", $path)) }
    };
}

/// Every font file, each family's subsets in their fallback order.
pub static FONTS: [Font; 11] = [
    font!(Overpass, "latin", "overpass-latin.ttf"),
    font!(Overpass, "latin-ext", "overpass-latin-ext.ttf"),
    font!(Overpass, "cyrillic", "overpass-cyrillic.ttf"),
    font!(Overpass, "cyrillic-ext", "overpass-cyrillic-ext.ttf"),
    font!(Overpass, "vietnamese", "overpass-vietnamese.ttf"),
    font!(OverpassMono, "latin", "overpass-mono-latin.ttf"),
    font!(OverpassMono, "latin-ext", "overpass-mono-latin-ext.ttf"),
    font!(OverpassMono, "cyrillic", "overpass-mono-cyrillic.ttf"),
    font!(OverpassMono, "cyrillic-ext", "overpass-mono-cyrillic-ext.ttf"),
    font!(OverpassMono, "vietnamese", "overpass-mono-vietnamese.ttf"),
    font!(OverpassTabular, "latin", "overpass-tnum-latin.ttf"),
];

/// The fonts' licence (SIL OFL 1.1).
pub static FONTS_LICENSE: &str = include_str!("../../../assets/fonts/OFL.txt");

/// The files of one font family, in fallback order.
pub fn fonts(file: FontFile) -> impl Iterator<Item = &'static Font> {
    FONTS.iter().filter(move |f| f.file == file)
}

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        /// Every vendored Lucide icon by its Lucide name (`plane-takeoff`), as its SVG file, sorted
        /// by name.
        pub static ICONS: &[(&str, &[u8])] = &[
            $(($name, include_bytes!(concat!("../../../assets/icons/lucide/", $name, ".svg")))),*
        ];
    };
}

icons![
    "arrow-left",
    "arrow-right",
    "building",
    "check",
    "chevron-left",
    "chevron-right",
    "external-link",
    "eye",
    "flag",
    "funnel",
    "house",
    "key-round",
    "keyboard",
    "landmark",
    "layers",
    "loader-circle",
    "locate-fixed",
    "lock-keyhole",
    "mailbox",
    "map-pin",
    "map",
    "minus",
    "navigation-2",
    "plane-landing",
    "plane-takeoff",
    "plane",
    "plus",
    "radar",
    "refresh-cw",
    "rotate-3d",
    "search",
    "settings",
    "ship",
    "signpost",
    "train-front",
    "triangle-alert",
    "x",
];

/// The icons' licence (ISC).
pub static ICONS_LICENSE: &str = include_str!("../../../assets/icons/lucide/LICENSE");

/// The SVG file of the Lucide icon `name`, if it is vendored.
pub fn icon(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, svg)| *svg)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn folder(rel: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
    }

    #[test]
    fn every_vendored_icon_is_embedded_once() {
        let mut on_disk: Vec<String> = std::fs::read_dir(folder("assets/icons/lucide"))
            .expect("assets/icons/lucide")
            .filter_map(|e| e.ok()?.file_name().into_string().ok()?.strip_suffix(".svg").map(str::to_string))
            .collect();
        on_disk.sort();
        let mut embedded: Vec<String> = ICONS.iter().map(|(n, _)| n.to_string()).collect();
        embedded.sort();
        embedded.dedup();
        assert_eq!(embedded.len(), ICONS.len(), "an icon is listed twice");
        assert_eq!(embedded, on_disk, "assets/icons/lucide and ICONS differ");
        for (name, svg) in ICONS {
            let text = std::str::from_utf8(svg).expect("UTF-8");
            assert!(text.contains("<svg") && text.contains("stroke-width"), "{name} is a Lucide SVG");
        }
        assert_eq!(icon("plane").map(<[u8]>::len), Some(ICONS.iter().find(|(n, _)| *n == "plane").unwrap().1.len()));
        assert_eq!(icon("no-such-icon"), None);
        assert!(ICONS_LICENSE.starts_with("ISC License"));
    }

    #[test]
    fn every_vendored_font_is_embedded_in_fallback_order() {
        let ttfs = std::fs::read_dir(folder("assets/fonts"))
            .expect("assets/fonts")
            .filter(|e| e.as_ref().is_ok_and(|e| e.path().extension().is_some_and(|x| x == "ttf")))
            .count();
        assert_eq!(ttfs, FONTS.len());
        let order = ["latin", "latin-ext", "cyrillic", "cyrillic-ext", "vietnamese"];
        for file in [FontFile::Overpass, FontFile::OverpassMono] {
            let subsets: Vec<&str> = fonts(file).map(|f| f.subset).collect();
            assert_eq!(subsets, order, "{file:?}");
        }
        assert_eq!(fonts(FontFile::OverpassTabular).map(|f| f.subset).collect::<Vec<_>>(), ["latin"]);
        for font in &FONTS {
            // a TrueType file: version 1.0 in its first four bytes
            assert_eq!(&font.bytes[..4], &[0, 1, 0, 0], "{:?} {}", font.file, font.subset);
        }
        assert!(FONTS_LICENSE.contains("SIL OPEN FONT LICENSE"));
    }
}
