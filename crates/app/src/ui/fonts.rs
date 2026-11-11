//! The UI's fonts (plan 1.5; doc 06 §1.2): Overpass (`--font-ui`) and Overpass Mono
//! (`--font-data`), the variable TTFs of [`crate::assets::FONTS`], as one egui font family per
//! weight in use.
//!
//! The web build loads `@fontsource-variable/overpass` and `overpass-mono`, so every
//! `font-weight` is drawn exactly from the `wght` axis. egui has no weight in a `FontId`, so each
//! weight is its own family backed by the same bytes with the axis set (`FontTweak::coords`):
//! `ui-400`, `ui-500`, `ui-550`, `ui-600`, `ui-650`, `ui-700`, `ui-750` and `mono-400` … `mono-700`
//! ([`family`]). The weights are exact, as today, not rounded to static instances.
//!
//! Each family is a fallback chain, as the fonts' CSS `unicode-range`s make one: the subsets
//! latin, latin-ext, cyrillic, cyrillic-ext, vietnamese; then the other face at the same weight;
//! then egui's own fonts (Ubuntu Light and egui's icons, from its `default_fonts` feature), where
//! today's CSS falls back to the system's fonts, which the app does not read. Overpass lacks Greek
//! and `≈ ≤ ≥`, which Ubuntu Light has; neither has `→ ← ⌘ ⇧ ⌥ ✓ ✕` (only the dev build's
//! shortcut rows show `⌘⇧`). Plan 1.5 turns `default_fonts` off: kept on, as this last fallback
//! (a deviation; a fallback font for other scripts is on plan section 6's post-parity list).
//! Overpass Mono's axis runs 300–700 with its default at 300, so its coordinate is always set; a
//! weight above 700 is drawn at 700, as the browser clamps it.
//!
//! egui's built-in families are bound too: `FontFamily::Proportional` is `ui-400` and
//! `FontFamily::Monospace` is `mono-400`, so text that names no family is Overpass.

use std::sync::{Arc, LazyLock};

use egui::epaint::text::{FontData, FontDefinitions, FontTweak, VariationCoords};
use egui::{FontFamily, FontId};

use crate::assets::{self, FontFile};

/// Which of the two faces: `--font-ui` or `--font-data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Overpass: `--font-ui`, all UI text.
    Ui,
    /// Overpass Mono: `--font-data` (`.data`), measured values: altitudes, speeds, codes, times.
    Mono,
}

impl Face {
    /// The family names' prefix.
    pub const fn prefix(self) -> &'static str {
        match self {
            Face::Ui => "ui",
            Face::Mono => "mono",
        }
    }

    /// The registered weights, lightest first.
    pub const fn weights(self) -> &'static [u16] {
        match self {
            Face::Ui => &UI_WEIGHTS,
            Face::Mono => &MONO_WEIGHTS,
        }
    }

    fn file(self) -> FontFile {
        match self {
            Face::Ui => FontFile::Overpass,
            Face::Mono => FontFile::OverpassMono,
        }
    }

    fn other(self) -> Face {
        match self {
            Face::Ui => Face::Mono,
            Face::Mono => Face::Ui,
        }
    }
}

/// Every `font-weight` the components use (doc 06 §1.2): 400 is the default.
pub const UI_WEIGHTS: [u16; 7] = [400, 500, 550, 600, 650, 700, 750];
/// The weights of the data font: `.data` inherits its parent's weight, and the components set
/// `font: 600 … var(--font-data)` (G:3842, the dev readout) and put data inside 650 and 700 text;
/// 750 is drawn at 700, the axis's end.
pub const MONO_WEIGHTS: [u16; 6] = [400, 500, 550, 600, 650, 700];
/// The end of Overpass Mono's `wght` axis.
pub const MONO_MAX_WEIGHT: u16 = 700;

/// The registered weight that draws `weight` of `face`: the weight itself when it is in use, a
/// weight above Overpass Mono's 700 at 700 (as the browser clamps the axis), anything else at the
/// nearest registered weight (the heavier of two equally near), which is never the case for the
/// weights of doc 06.
pub fn weight(face: Face, weight: u16) -> u16 {
    let weights = face.weights();
    if weights.contains(&weight) {
        return weight;
    }
    *weights.iter().min_by_key(|&&w| (w.abs_diff(weight), std::cmp::Reverse(w))).expect("weights")
}

/// The family name of `face` at `weight` (after [`weight`]), e.g. `ui-550`.
pub fn family_name(face: Face, w: u16) -> String {
    format!("{}-{}", face.prefix(), weight(face, w))
}

static FAMILIES: LazyLock<Vec<(Face, u16, FontFamily)>> = LazyLock::new(|| {
    [Face::Ui, Face::Mono]
        .into_iter()
        .flat_map(|face| face.weights().iter().map(move |&w| (face, w, FontFamily::Name(Arc::from(family_name(face, w))))))
        .collect()
});

/// The egui family of `face` at `weight` (after [`weight`]).
pub fn family(face: Face, w: u16) -> FontFamily {
    let w = weight(face, w);
    FAMILIES.iter().find(|(f, fw, _)| *f == face && *fw == w).map(|(_, _, family)| family.clone()).expect("a registered weight")
}

/// Overpass at `size` px and `weight`.
pub fn ui(size: f32, weight: u16) -> FontId {
    FontId::new(size, family(Face::Ui, weight))
}

/// Overpass Mono (`.data`) at `size` px and `weight`.
pub fn mono(size: f32, weight: u16) -> FontId {
    FontId::new(size, family(Face::Mono, weight))
}

/// The name of one subset's file at one weight in [`FontDefinitions::font_data`].
fn data_name(face: Face, subset: &str, weight: u16) -> String {
    format!("{}-{subset}@{weight}", face.prefix())
}

/// The fonts: every subset of both faces at every registered weight, the families of [`family`],
/// and egui's `Proportional` and `Monospace` bound to `ui-400` and `mono-400`.
pub fn definitions() -> FontDefinitions {
    // egui's own fonts (with `default_fonts`) stay as the last fallback of every family
    let builtin = FontDefinitions::default();
    let mut defs = FontDefinitions { font_data: builtin.font_data.clone(), families: Default::default() };
    for face in [Face::Ui, Face::Mono] {
        for &w in face.weights() {
            for font in assets::fonts(face.file()) {
                let tweak = FontTweak { coords: VariationCoords::new([(b"wght", f32::from(w))]), ..Default::default() };
                defs.font_data.insert(data_name(face, font.subset, w), Arc::new(FontData::from_static(font.bytes).tweak(tweak)));
            }
        }
    }
    let chain = |face: Face, w: u16| -> Vec<String> {
        let other = face.other();
        let mut names: Vec<String> = assets::fonts(face.file()).map(|f| data_name(face, f.subset, w)).collect();
        names.extend(assets::fonts(other.file()).map(|f| data_name(other, f.subset, weight(other, w))));
        names.extend(builtin.families.get(&FontFamily::Proportional).into_iter().flatten().cloned());
        names
    };
    for (face, w, family) in FAMILIES.iter() {
        defs.families.insert(family.clone(), chain(*face, *w));
    }
    defs.families.insert(FontFamily::Proportional, chain(Face::Ui, 400));
    defs.families.insert(FontFamily::Monospace, chain(Face::Mono, 400));
    defs
}

/// Gives `ctx` the fonts of [`definitions`] (from its next pass, as egui's `set_fonts` does:
/// call it before the first pass).
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_weight_in_use_has_a_family() {
        for w in UI_WEIGHTS {
            assert_eq!(weight(Face::Ui, w), w);
            assert_eq!(family(Face::Ui, w), FontFamily::Name(format!("ui-{w}").into()));
        }
        for w in MONO_WEIGHTS {
            assert_eq!(family(Face::Mono, w), FontFamily::Name(format!("mono-{w}").into()));
        }
        // Overpass Mono stops at 700
        assert_eq!(family(Face::Mono, 750), family(Face::Mono, 700));
        assert_eq!(weight(Face::Mono, 900), MONO_MAX_WEIGHT);
        // a weight nobody uses goes to the nearest, the heavier of two
        assert_eq!(weight(Face::Ui, 800), 750);
        assert_eq!(weight(Face::Ui, 300), 400);
        assert_eq!(weight(Face::Ui, 525), 550);
        assert_eq!(ui(13.0, 550), FontId::new(13.0, FontFamily::Name("ui-550".into())));
        assert_eq!(mono(12.0, 600), FontId::new(12.0, FontFamily::Name("mono-600".into())));
    }

    #[test]
    fn every_family_chains_the_subsets_with_the_weight_set() {
        let defs = definitions();
        let ui550 = &defs.families[&family(Face::Ui, 550)];
        assert_eq!(&ui550[..5], ["ui-latin@550", "ui-latin-ext@550", "ui-cyrillic@550", "ui-cyrillic-ext@550", "ui-vietnamese@550"]);
        assert_eq!(&ui550[5..10], ["mono-latin@550", "mono-latin-ext@550", "mono-cyrillic@550", "mono-cyrillic-ext@550", "mono-vietnamese@550"]);
        let mono750 = &defs.families[&family(Face::Mono, 750)];
        assert_eq!(mono750[0], "mono-latin@700");
        assert_eq!(mono750[5], "ui-latin@700");
        assert_eq!(defs.families[&FontFamily::Proportional][0], "ui-latin@400");
        assert_eq!(defs.families[&FontFamily::Monospace][0], "mono-latin@400");
        for names in defs.families.values() {
            for name in names {
                assert!(defs.font_data.contains_key(name), "{name} has no font data");
            }
        }
        // each file carries its weight on the wght axis, inside the axis's range
        for (name, data) in &defs.font_data {
            let Some((_, w)) = name.split_once('@') else { continue };
            let w: f32 = w.parse().expect("a weight");
            let coords: Vec<(String, f32)> = data.tweak.coords.as_ref().iter().map(|(tag, v)| (tag.to_string(), *v)).collect();
            assert_eq!(coords, [("wght".to_string(), w)], "{name}");
            let axis = data.variation_axes().into_iter().find(|a| a.tag == "wght").expect("a wght axis");
            assert!(axis.range.contains(w), "{name}: {w} in {:?}", axis.range);
        }
    }

    #[test]
    fn heavier_weights_are_wider() {
        // the axis reaches the layout: the same text grows with its weight (HVAR), Mono's does not
        let ctx = egui::Context::default();
        install(&ctx);
        let mut widths = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for w in UI_WEIGHTS {
                widths.push(ui.painter().layout_no_wrap("Flightradar24 overhead".into(), self::ui(14.0, w), egui::Color32::WHITE).size().x);
            }
            let m400 = ui.painter().layout_no_wrap("12,345 ft".into(), mono(13.0, 400), egui::Color32::WHITE).size().x;
            let m700 = ui.painter().layout_no_wrap("12,345 ft".into(), mono(13.0, 700), egui::Color32::WHITE).size().x;
            assert!((m400 - m700).abs() < 0.01, "monospaced at every weight: {m400} {m700}");
        });
        output.textures_delta.clear();
        assert!(widths.windows(2).all(|w| w[1] > w[0]), "{widths:?}");
    }
}
