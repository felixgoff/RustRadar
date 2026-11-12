//! `cargo xtask vendor-fonts`: decode the `.woff2` subsets with `wuff` and write TTFs plus the
//! OFL licence to `assets/fonts` (S1.30).
//!
//! The sources are the files the web build loads (`import "@fontsource-variable/overpass"` and
//! `"…/overpass-mono"` in `src/routes/+layout.svelte`): the upright `wght` variable fonts of
//! `node_modules/@fontsource-variable/{overpass,overpass-mono}/files`, one per subset. The
//! italic files are not vendored, the UI never sets italic. Run `bun install --frozen-lockfile`
//! first if `node_modules` is missing. woff2 is a lossless container, so the TTFs keep the
//! `fvar`/`gvar` tables and stay variable: the app sets the `wght` coordinate per egui family
//! (doc 00 §1.5).
//!
//! Output, all generated (re-running gives identical bytes):
//! - `<family>-<subset>.ttf` for the families of [`FAMILIES`] and the subsets of [`SUBSETS`];
//! - [`TABULAR`] (`overpass-tnum-latin.ttf`, item F1): Overpass's latin subset with its `tnum`
//!   feature renamed `rclt` ([`tabular`]). Shapers (egui's harfrust, as HarfBuzz) apply `rclt`
//!   by default and nothing can switch features on in egui, so this file draws the tabular figures
//!   that `font-variant-numeric: tabular-nums` asks for, everything else as Overpass. Renaming the
//!   feature rather than pointing the `cmap` at the `.tf` glyphs (as `pyftfeatfreeze` does) keeps
//!   the lookup order Chromium shapes with when it turns `tnum` on. Only latin has figures, and
//!   Overpass Mono's default figures are its tabular ones;
//! - `OFL.txt`, the SIL Open Font License 1.1 of both packages;
//! - `README.md`, where the files come from.

use std::fs;
use std::path::{Path, PathBuf};

/// The font packages under `node_modules/@fontsource-variable`; each file stem starts with the
/// package name.
pub const FAMILIES: [&str; 2] = ["overpass", "overpass-mono"];

/// The subsets, in the fallback order the app registers them in (doc 00 §1.5).
pub const SUBSETS: [&str; 5] = ["latin", "latin-ext", "cyrillic", "cyrillic-ext", "vietnamese"];

/// The vendored file with Overpass's tabular figures as its default ones ([`tabular`]).
pub const TABULAR: &str = "overpass-tnum-latin.ttf";

/// The repository root: `crates/xtask` is two levels below it.
fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf()
}

fn source_dir() -> PathBuf {
    repo_root().join("node_modules/@fontsource-variable")
}

fn out_dir() -> PathBuf {
    repo_root().join("assets/fonts")
}

/// The vendored file name of one subset of one family, e.g. `overpass-mono-latin-ext.ttf`.
pub fn ttf_name(family: &str, subset: &str) -> String {
    format!("{family}-{subset}.ttf")
}

pub fn run(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err(format!("vendor-fonts takes no arguments, got {args:?}"));
    }
    let out = out_dir();
    let files = vendored_files(&source_dir())?;
    fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    for (name, bytes) in &files {
        let path = out.join(name);
        fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        println!("wrote {} ({} bytes)", path.display(), bytes.len());
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// The package's `version` from its `package.json`.
fn package_version(package: &Path) -> Result<String, String> {
    let path = package.join("package.json");
    let json: serde_json::Value = serde_json::from_str(&read_text(&path)?)
        .map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
    json["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} has no version", path.display()))
}

/// Decodes one subset's `.woff2` file to TTF bytes.
fn decode(source: &Path, family: &str, subset: &str) -> Result<Vec<u8>, String> {
    let file = format!("{family}-{subset}-wght-normal.woff2");
    let path = source.join(family).join("files").join(file);
    let woff2 = read(&path)?;
    wuff::decompress_woff2(&woff2).map_err(|e| format!("cannot decode {}: {e}", path.display()))
}

/// The licence both packages ship. They differ only in the copyright line: Overpass's also names
/// its italic file, which is not vendored, so Overpass Mono's text (the same copyright holders,
/// without the italic file) covers all ten TTFs. Fails if anything else differs.
fn licence(source: &Path) -> Result<String, String> {
    let overpass = read_text(&source.join("overpass/LICENSE"))?;
    let mono = read_text(&source.join("overpass-mono/LICENSE"))?;
    let body = |text: &str| text.split_once('\n').map(|(_, rest)| rest.to_owned());
    if body(&overpass).is_none() || body(&overpass) != body(&mono) {
        return Err("the font packages' LICENSE files differ beyond the copyright line".into());
    }
    if !mono.contains("SIL Open Font License, Version 1.1") {
        return Err("the font licence is no longer the SIL Open Font License 1.1".into());
    }
    Ok(mono)
}

fn be16(bytes: &[u8], at: usize) -> Result<usize, String> {
    bytes.get(at..at + 2).map(|b| usize::from(u16::from_be_bytes([b[0], b[1]]))).ok_or_else(|| format!("the font ends before byte {at}"))
}

fn be32(bytes: &[u8], at: usize) -> Result<u32, String> {
    bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| format!("the font ends before byte {at}"))
}

/// The OpenType checksum of `table`: the sum of its big-endian 32-bit words, zero-padded.
fn checksum(table: &[u8]) -> u32 {
    table.chunks(4).fold(0u32, |sum, word| {
        let mut w = [0u8; 4];
        w[..word.len()].copy_from_slice(word);
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

/// Where table `tag`'s directory record is, and the table's offset and length.
fn table(ttf: &[u8], tag: &[u8; 4]) -> Result<(usize, usize, usize), String> {
    let count = be16(ttf, 4)?;
    (0..count)
        .map(|i| 12 + 16 * i)
        .find(|&record| ttf.get(record..record + 4) == Some(tag))
        .map(|record| Ok((record, be32(ttf, record + 8)? as usize, be32(ttf, record + 12)? as usize)))
        .unwrap_or_else(|| Err(format!("no {} table", String::from_utf8_lossy(tag))))
}

/// `ttf` with its `GSUB` feature `tnum` renamed `rclt` (see the module documentation), the
/// table's checksum and `head.checkSumAdjustment` brought up to date. Fails if the font has no
/// `tnum` feature or already an `rclt` one.
pub fn tabular(ttf: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = ttf.to_vec();
    let (gsub_record, gsub, gsub_len) = table(ttf, b"GSUB")?;
    let features = gsub + be16(ttf, gsub + 6)?;
    let count = be16(ttf, features)?;
    let tags: Vec<usize> = (0..count).map(|i| features + 2 + 6 * i).collect();
    if tags.iter().any(|&at| ttf.get(at..at + 4) == Some(b"rclt")) {
        return Err("the font has an rclt feature already".into());
    }
    let tnum: Vec<usize> = tags.into_iter().filter(|&at| ttf.get(at..at + 4) == Some(b"tnum")).collect();
    if tnum.is_empty() {
        return Err("the font has no tnum feature".into());
    }
    for at in tnum {
        out[at..at + 4].copy_from_slice(b"rclt");
    }
    let sum = checksum(&out[gsub..gsub + gsub_len]);
    out[gsub_record + 4..gsub_record + 8].copy_from_slice(&sum.to_be_bytes());
    // the whole font sums to 0xB1B0AFBA with the adjustment in `head`
    let (_, head, _) = table(ttf, b"head")?;
    out[head + 8..head + 12].copy_from_slice(&[0; 4]);
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    out[head + 8..head + 12].copy_from_slice(&adjustment.to_be_bytes());
    Ok(out)
}

fn readme(source: &Path) -> Result<String, String> {
    let mut packages = Vec::new();
    for family in FAMILIES {
        let version = package_version(&source.join(family))?;
        packages.push(format!("`@fontsource-variable/{family}` {version}"));
    }
    Ok(format!(
        "# Fonts\n\
         \n\
         Generated by `cargo xtask vendor-fonts` (`crates/xtask/src/fonts.rs`): do not edit.\n\
         \n\
         The TTFs are the `files/*-wght-normal.woff2` files of {}, decoded with `wuff`.\n\
         They are variable fonts with a `wght` axis: Overpass 100 to 900 (default 400),\n\
         Overpass Mono 300 to 700 (default 300).\n\
         Subsets, in fallback order: {}.\n\
         \n\
         `{TABULAR}` is `overpass-latin.ttf` with its `tnum` feature renamed `rclt`, which shapers\n\
         apply by default: Overpass with tabular figures, for `font-variant-numeric: tabular-nums`.\n\
         \n\
         Licensed under the SIL Open Font License 1.1, see `OFL.txt`.\n",
        packages.join(" and "),
        SUBSETS.join(", "),
    ))
}

/// Every file of `assets/fonts` as (name, bytes), built from the packages under `source`.
fn vendored_files(source: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    if !source.is_dir() {
        return Err(format!(
            "{} is missing: run `bun install --frozen-lockfile` first",
            source.display()
        ));
    }
    let mut files = Vec::new();
    for family in FAMILIES {
        for subset in SUBSETS {
            files.push((ttf_name(family, subset), decode(source, family, subset)?));
        }
    }
    let latin = &files.iter().find(|(name, _)| *name == ttf_name("overpass", "latin")).expect("Overpass's latin subset").1;
    files.push((TABULAR.to_owned(), tabular(latin)?));
    files.push(("OFL.txt".to_owned(), licence(source)?.into_bytes()));
    files.push(("README.md".to_owned(), readme(source)?.into_bytes()));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use epaint::text::{FontData, Tag};
    use skrifa::MetadataProvider as _;

    fn vendored(name: &str) -> Vec<u8> {
        let path = out_dir().join(name);
        fs::read(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (run `cargo xtask vendor-fonts`)", path.display()))
    }

    #[test]
    fn each_ttf_has_the_wght_axis() {
        for family in FAMILIES {
            // (min, default, max). Mono's default is its minimum, 300, not the 400 the UI uses,
            // so the app always sets the coordinate (doc 00 §1.5).
            let expected =
                if family == "overpass" { (100.0, 400.0, 900.0) } else { (300.0, 300.0, 700.0) };
            for subset in SUBSETS {
                let name = ttf_name(family, subset);
                let axes = FontData::from_owned(vendored(&name)).variation_axes();
                let wght = axes
                    .iter()
                    .find(|axis| axis.tag == Tag::new(b"wght"))
                    .unwrap_or_else(|| panic!("{name} has no wght axis: {axes:?}"));
                assert_eq!((wght.range.min, wght.default, wght.range.max), expected, "{name}");
            }
        }
    }

    #[test]
    fn each_subset_maps_its_script() {
        // One character per subset, from the subset's CSS `unicode-range`.
        let samples = [
            ("latin", ['A', '0', '\u{b7}']),
            ("latin-ext", ['\u{141}', '\u{17e}', '\u{1e9e}']),
            ("cyrillic", ['\u{416}', '\u{44f}', '\u{2116}']),
            ("cyrillic-ext", ['\u{462}', '\u{4e8}', '\u{20b4}']),
            ("vietnamese", ['\u{1ea1}', '\u{1ef9}', '\u{20ab}']),
        ];
        assert_eq!(samples.map(|(subset, _)| subset), SUBSETS);
        for family in FAMILIES {
            for (subset, chars) in samples {
                let name = ttf_name(family, subset);
                let bytes = vendored(&name);
                let font = skrifa::FontRef::new(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
                let charmap = font.charmap();
                for c in chars {
                    assert!(charmap.map(c).is_some(), "{name} does not map U+{:04X}", u32::from(c));
                }
            }
        }
    }

    /// Why `ui::text::job()` sets every `·` in `mono-400` (doc 00 §1.5): Overpass classes its
    /// middle dot as a mark glyph in `GDEF`, so a shaper zeroes its advance (its `hmtx` advance
    /// is 450 of 2000 units); Overpass Mono's is a plain glyph.
    #[test]
    fn overpass_middle_dot_is_a_mark_and_mono_is_not() {
        use skrifa::raw::TableProvider as _;
        for (family, class) in [("overpass", 3), ("overpass-mono", 0)] {
            let name = ttf_name(family, "latin");
            let bytes = vendored(&name);
            let font = skrifa::FontRef::new(&bytes).unwrap();
            let glyph = font.charmap().map('\u{b7}').unwrap();
            let classes = font.gdef().unwrap().glyph_class_def().unwrap().unwrap();
            assert_eq!(classes.get(glyph), class, "{name}");
        }
    }

    /// The vertical metrics `ui::text`'s baselines are computed from (`ASCENT`, `DESCENT`,
    /// `UNITS_PER_EM` there): the same in both faces and every subset, `USE_TYPO_METRICS` set so
    /// that the typo metrics are the ones used, and no `MVAR` to change them with the weight.
    #[test]
    fn both_faces_have_the_vertical_metrics_the_baselines_use() {
        use skrifa::raw::TableProvider as _;
        for family in FAMILIES {
            for subset in SUBSETS {
                let name = ttf_name(family, subset);
                let bytes = vendored(&name);
                let font = skrifa::FontRef::new(&bytes).unwrap();
                let (head, hhea, os2) = (font.head().unwrap(), font.hhea().unwrap(), font.os2().unwrap());
                assert_eq!(head.units_per_em(), 2000, "{name}");
                assert_eq!((hhea.ascender().to_i16(), hhea.descender().to_i16(), hhea.line_gap().to_i16()), (1766, -766, 0), "{name}");
                assert_eq!((os2.s_typo_ascender(), os2.s_typo_descender(), os2.s_typo_line_gap()), (1766, -766, 0), "{name}");
                assert_ne!(os2.fs_selection().bits() & (1 << 7), 0, "{name}: USE_TYPO_METRICS");
                assert!(font.table_data(skrifa::raw::types::Tag::new(b"MVAR")).is_none(), "{name} has an MVAR");
            }
        }
    }

    /// The tabular file is Overpass's latin file with one feature tag changed, its checksums
    /// right, and that feature maps each figure to a glyph 0.616 em wide.
    #[test]
    fn the_tabular_file_renames_tnum_to_rclt() {
        use skrifa::raw::TableProvider as _;
        let latin = vendored(&ttf_name("overpass", "latin"));
        let tnum = vendored(TABULAR);
        assert_eq!(tnum, tabular(&latin).unwrap());
        assert_eq!(latin.len(), tnum.len());
        let changed: Vec<usize> = (0..latin.len()).filter(|&i| latin[i] != tnum[i]).collect();
        // the four tag bytes (fewer where the letters agree), the GSUB record's checksum, head's adjustment
        assert!(changed.len() <= 12, "{} bytes changed", changed.len());
        // every table's checksum, and the whole font's
        let count = be16(&tnum, 4).unwrap();
        for i in 0..count {
            let record = 12 + 16 * i;
            let (offset, length) = (be32(&tnum, record + 8).unwrap() as usize, be32(&tnum, record + 12).unwrap() as usize);
            let mut data = tnum[offset..offset + length].to_vec();
            if &tnum[record..record + 4] == b"head" {
                data[8..12].copy_from_slice(&[0; 4]);
            }
            assert_eq!(checksum(&data), be32(&tnum, record + 4).unwrap(), "{}", String::from_utf8_lossy(&tnum[record..record + 4]));
        }
        assert_eq!(checksum(&tnum), 0xB1B0_AFBA);
        // the features: rclt where tnum was, and its single substitutions give 1232-unit figures
        let font = skrifa::FontRef::new(&tnum).unwrap();
        let gsub = font.gsub().unwrap();
        let features = gsub.feature_list().unwrap();
        let tags: Vec<String> = features.feature_records().iter().map(|r| r.feature_tag().to_string()).collect();
        assert!(tags.contains(&"rclt".to_string()) && !tags.contains(&"tnum".to_string()), "{tags:?}");
        let record = features.feature_records().iter().find(|r| r.feature_tag() == skrifa::raw::types::Tag::new(b"rclt")).unwrap();
        let feature = record.feature(features.offset_data()).unwrap();
        let lookups = gsub.lookup_list().unwrap();
        let hmtx = font.hmtx().unwrap();
        let charmap = font.charmap();
        for c in '0'..='9' {
            let glyph = charmap.map(c).unwrap();
            let mut to = None;
            for index in feature.lookup_list_indices() {
                let lookup = lookups.lookups().get(usize::from(index.get())).unwrap();
                if let skrifa::raw::tables::gsub::SubstitutionLookup::Single(single) = lookup {
                    for sub in single.subtables().iter().flatten() {
                        if let Some(g) = single_substitute(&sub, skrifa::raw::types::GlyphId16::new(glyph.to_u32() as u16)) {
                            to = Some(g);
                        }
                    }
                }
            }
            let to = to.unwrap_or_else(|| panic!("{c} has no tabular glyph"));
            assert_eq!(hmtx.h_metrics()[usize::from(to)].advance(), 1232, "{c}");
        }
    }

    /// The glyph a single substitution puts for `glyph`.
    fn single_substitute(sub: &skrifa::raw::tables::gsub::SingleSubst<'_>, glyph: skrifa::raw::types::GlyphId16) -> Option<u16> {
        use skrifa::raw::tables::gsub::SingleSubst;
        match sub {
            SingleSubst::Format1(f) => {
                f.coverage().ok()?.get(glyph)?;
                Some((i32::from(glyph.to_u16()) + i32::from(f.delta_glyph_id())) as u16)
            }
            SingleSubst::Format2(f) => {
                let at = f.coverage().ok()?.get(glyph)?;
                f.substitute_glyph_ids().get(usize::from(at)).map(|g| g.get().to_u16())
            }
        }
    }

    #[test]
    fn the_licence_is_the_ofl() {
        let text = String::from_utf8(vendored("OFL.txt")).unwrap();
        assert!(text.starts_with("Copyright 2021 The Overpass Project Authors"));
        assert!(text.contains("SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007"));
    }

    #[test]
    fn vendored_files_match_a_fresh_decode() {
        let source = source_dir();
        if !source.is_dir() {
            eprintln!("skipped: {} is missing (bun install --frozen-lockfile)", source.display());
            return;
        }
        let files = vendored_files(&source).unwrap();
        assert_eq!(files.len(), FAMILIES.len() * SUBSETS.len() + 3);
        for (name, bytes) in files {
            let stale = "is stale: run `cargo xtask vendor-fonts`";
            assert!(vendored(&name) == bytes, "assets/fonts/{name} {stale}");
        }
    }
}
