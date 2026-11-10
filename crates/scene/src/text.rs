//! Text on the map (item E10; plan 1.4.10; docs 01 §5.9, §6, 03 §7.4): the vendored fonts, deck.gl
//! 9.4's font atlas made on demand, and deck's per-character layout. `map::pipelines::text` draws
//! the result.
//!
//! - **Fonts.** [`OVERPASS`] and [`OVERPASS_MONO`] are the TTFs of `assets/fonts/` (five subsets
//!   each, in their fallback order), embedded. A [`Face`] is a family at one `wght` (the axis is
//!   clamped to the font's range: Overpass 100–900, Mono 300–700). [`FontCanvas`] is the 2D canvas
//!   TinySDF and deck draw on, for one face at the atlas's font size: `measureText` from the skrifa
//!   outline's control box and advance, `fillText` as a tiny-skia anti-aliased fill (non-zero
//!   winding) of that outline. A character takes the first subset that maps it; one no subset has
//!   is skipped, with no advance (plan section 6, deviation 13: the browser fell back to a system
//!   font).
//! - **Atlas.** [`GlyphAtlas`] is deck's `FontAtlasManager` for one [`FontSettings`] (deck's
//!   `fontSettings`: font size 64, buffer, SDF or not, radius, cutoff, smoothing) and any number of
//!   faces, packed on demand into one `R8Unorm` picture ([`ATLAS_SIZE`]² texels) as deck's
//!   `characterSet: "auto"` does. A glyph's [`GlyphFrame`] is deck's mapping entry after its SDF
//!   renderer has drawn it: the whole SDF image as the frame, `anchorX = width/2 − left` with the
//!   pen `left` columns into the image, and `anchorY = ascent + buffer`; for the plain (non-SDF)
//!   atlas, the measured cell, its anchor at `(width/2, ascent)`. Cells are shelf-packed in rows as deck's `buildMapping` does, but by the
//!   size of the picture actually drawn, so a glyph whose ink reaches left of its pen no longer
//!   overlaps its neighbour (deck packs by `ceil(right − left)`); where a glyph sits in the atlas is
//!   not visible. When the atlas is full it is packed again with the most recently used glyphs
//!   first and the rest dropped (plan section 5, K5); [`GlyphAtlas::generation`] counts these
//!   rebuilds, after which earlier [`LaidText`] is stale.
//! - **Layout.** [`transform_paragraph`] (with deck's `autoWrapping`, `breakAll`, `breakWord`,
//!   `transformRow`), [`icon_offsets`] (TextLayer's `getIconOffsets`) and [`bounding_rect`]
//!   (`getBoundingRect`) are ported verbatim: per character, advance widths and no kerning, rows
//!   `lineHeight · fontSize` apart, anchors `start`/`middle`/`end`, baselines `top`/`center`/`bottom`.
//!   [`GlyphAtlas::layout`] runs them over a string and returns each inked glyph's atlas frame and
//!   its offset in atlas pixels from the label's anchor (deck's `instanceOffsets`, x right, y down),
//!   ready for the text pipeline, and the text's box for collision and picking.
//!
//! All sizes here are "texture size": pixels of the atlas's font size (64). A label `size` CSS
//! pixels tall scales them by `size · sizeScale / font_size` ([`LaidText::box_px`]).
//!
//! One difference from today's app, on purpose. TinySDF 2.3.0 (the version installed) draws each
//! glyph with its pen `glyphLeft = floor(−actualBoundingBoxLeft)` columns left of the image's
//! buffer column, so that its ink starts there whatever its left side bearing (`scene::sdf`).
//! deck 9.4's SDF renderer was written against TinySDF 2.0, which put the pen at the buffer column,
//! and still says `left: buffer`; so in today's app every glyph is drawn its whole-pixel left side
//! bearing (2 to 5 atlas pixels for Overpass at 64 px: 0.4 to 1.1 CSS px at 14 px) left of its pen
//! position, and letter spacing is uneven by up to half a pixel. Here the pen is where TinySDF put
//! it (`left = buffer − glyphLeft`), so labels are laid out as the font means them, which is what
//! the item's acceptance (the ink box of "Stockholm" within 1 px of epaint's layout) asks for;
//! with deck's `left` the box's right end would be 1.1 px short.
//!
//! The vertical metrics (`fontBoundingBoxAscent`, `fontBoundingBoxDescent`) are the font's line
//! metrics as FreeType and Skia read them (OS/2 typo metrics when `USE_TYPO_METRICS` is set, else
//! `hhea`, with the variation deltas) rounded to whole pixels as Blink does: 57 and 25 for Overpass
//! at 64 px, so `baselineOffset` is 16.

use std::collections::HashMap;
use std::fmt;

use skrifa::instance::{Location, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::sdf::{Canvas, Region, TextMetrics, TinySdf};

/// Overpass (the variable `wght` font, 100–900, default 400): latin, latin-ext, cyrillic,
/// cyrillic-ext, vietnamese.
pub const OVERPASS: [&[u8]; 5] = [
    include_bytes!("../../../assets/fonts/overpass-latin.ttf"),
    include_bytes!("../../../assets/fonts/overpass-latin-ext.ttf"),
    include_bytes!("../../../assets/fonts/overpass-cyrillic.ttf"),
    include_bytes!("../../../assets/fonts/overpass-cyrillic-ext.ttf"),
    include_bytes!("../../../assets/fonts/overpass-vietnamese.ttf"),
];

/// Overpass Mono (`wght` 300–700, default 300), the same subsets.
pub const OVERPASS_MONO: [&[u8]; 5] = [
    include_bytes!("../../../assets/fonts/overpass-mono-latin.ttf"),
    include_bytes!("../../../assets/fonts/overpass-mono-latin-ext.ttf"),
    include_bytes!("../../../assets/fonts/overpass-mono-cyrillic.ttf"),
    include_bytes!("../../../assets/fonts/overpass-mono-cyrillic-ext.ttf"),
    include_bytes!("../../../assets/fonts/overpass-mono-vietnamese.ttf"),
];

/// A font family of `assets/fonts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// `"Overpass Variable"`, the app's text and label font.
    Overpass,
    /// `"Overpass Mono Variable"`, its data font.
    OverpassMono,
}

impl Family {
    /// The subsets, in fallback order.
    pub fn files(self) -> &'static [&'static [u8]; 5] {
        match self {
            Family::Overpass => &OVERPASS,
            Family::OverpassMono => &OVERPASS_MONO,
        }
    }
}

/// A family at one weight (CSS `font-weight`, the `wght` axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Face {
    pub family: Family,
    pub weight: u16,
}

impl Face {
    /// The map labels' face: `"Overpass Variable"` at 600 (place names, airport codes on the
    /// labels layer, aircraft labels, the searched place).
    pub const LABEL: Face = Face { family: Family::Overpass, weight: 600 };

    pub const fn new(family: Family, weight: u16) -> Face {
        Face { family, weight }
    }
}

/// deck's `fontSettings` (`DEFAULT_FONT_SETTINGS` merged with the layer's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontSettings {
    /// The size glyphs are drawn at in the atlas, pixels.
    pub font_size: f64,
    /// Padding round each glyph, pixels (the SDF's reach outside the ink).
    pub buffer: f64,
    /// A signed distance field (`true`) or plain coverage.
    pub sdf: bool,
    /// TinySDF's `cutoff`: the SDF value at the edge is `1 − cutoff`.
    pub cutoff: f64,
    /// TinySDF's `radius`, pixels; also what the layer's `outlineWidth` is divided by.
    pub radius: f64,
    /// Half the width of the SDF edge's smoothstep, in SDF units (`gamma`).
    pub smoothing: f64,
}

impl FontSettings {
    /// deck's `DEFAULT_FONT_SETTINGS`: 64 px, buffer 4, no SDF, cutoff 0.25, radius 12, smoothing
    /// 0.1 (the signal names, block labels and speed labels).
    pub const DEFAULT: FontSettings = FontSettings { font_size: 64.0, buffer: 4.0, sdf: false, cutoff: 0.25, radius: 12.0, smoothing: 0.1 };

    /// `LABEL_FONT_SETTINGS = { sdf: true, fontSize: 64, buffer: 6 }` (G:726): the labels with a
    /// halo.
    pub const LABELS: FontSettings = FontSettings { sdf: true, buffer: 6.0, ..Self::DEFAULT };
}

impl Default for FontSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// deck's `DEFAULT_ASCENT` and `DEFAULT_DESCENT`, fractions of the font size, for when the canvas
/// reports no ascent.
pub const DEFAULT_ASCENT: f64 = 0.9;
pub const DEFAULT_DESCENT: f64 = 0.3;

/// The advance deck gives a character that is not in the atlas's mapping (`MISSING_CHAR_WIDTH`).
/// With glyphs added on demand it only applies to [`transform_paragraph`] called with a partial
/// mapping.
pub const MISSING_CHAR_WIDTH: f64 = 32.0;

/// The atlas's side, texels: the WebGL2-class limit of plan 1.4.13.
pub const ATLAS_SIZE: u32 = 2048;

/// The canvas for one face at one font size: what TinySDF and deck's atlas measure and draw with.
#[derive(Clone)]
pub struct FontCanvas {
    fonts: Vec<(FontRef<'static>, Location)>,
    size: f32,
}

impl fmt::Debug for FontCanvas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontCanvas").field("subsets", &self.fonts.len()).field("size", &self.size).finish()
    }
}

/// The font's line metrics the canvas reports (`fontBoundingBoxAscent`, `fontBoundingBoxDescent`),
/// pixels, both positive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontBox {
    pub ascent: f64,
    pub descent: f64,
}

impl FontCanvas {
    /// `face` at `font_size` pixels.
    ///
    /// # Panics
    /// If an embedded font does not parse (a broken `assets/fonts`).
    pub fn new(face: Face, font_size: f64) -> FontCanvas {
        let fonts = face
            .family
            .files()
            .iter()
            .map(|bytes| {
                let font = FontRef::new(bytes).expect("an embedded font parses");
                let location = font.axes().location([("wght", f32::from(face.weight))]);
                (font, location)
            })
            .collect();
        FontCanvas { fonts, size: font_size as f32 }
    }

    /// The first subset that has a glyph for `ch`, and the glyph.
    fn glyph(&self, ch: char) -> Option<(&FontRef<'static>, &Location, GlyphId)> {
        self.fonts.iter().find_map(|(font, location)| {
            let id = font.charmap().map(ch)?;
            (id != GlyphId::NOTDEF).then_some((font, location, id))
        })
    }

    /// Whether some subset has a glyph for `ch`.
    pub fn has(&self, ch: char) -> bool {
        self.glyph(ch).is_some()
    }

    /// `measureText('A')`'s `fontBoundingBoxAscent` and `fontBoundingBoxDescent`: the line metrics
    /// of the subset that has `A`, rounded to whole pixels as Blink rounds them.
    pub fn font_box(&self) -> Option<FontBox> {
        let (font, location, _) = self.glyph('A')?;
        let metrics = font.metrics(Size::new(self.size), location);
        Some(FontBox { ascent: f64::from(metrics.ascent).round(), descent: f64::from(-metrics.descent).round() })
    }

    /// The outline of `ch` drawn into `pen`, in pixels with y up from the pen; its advance.
    fn draw(&self, ch: char, pen: &mut impl OutlinePen) -> Option<f64> {
        let (font, location, id) = self.glyph(ch)?;
        let size = Size::new(self.size);
        if let Some(outline) = font.outline_glyphs().get(id) {
            // a glyph whose outline does not draw is drawn as nothing, with its advance
            let _ = outline.draw(DrawSettings::unhinted(size, location), pen);
        }
        Some(f64::from(font.glyph_metrics(size, location).advance_width(id).unwrap_or(0.0)))
    }
}

/// The control box of an outline (every point, on and off the curve: what FreeType's
/// `FT_Outline_Get_CBox` gives Skia's glyph bounds).
#[derive(Default)]
struct BoundsPen {
    min: [f32; 2],
    max: [f32; 2],
    any: bool,
}

impl BoundsPen {
    fn add(&mut self, x: f32, y: f32) {
        if self.any {
            self.min = [self.min[0].min(x), self.min[1].min(y)];
            self.max = [self.max[0].max(x), self.max[1].max(y)];
        } else {
            (self.min, self.max, self.any) = ([x, y], [x, y], true);
        }
    }
}

impl OutlinePen for BoundsPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.add(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.add(x, y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.add(cx0, cy0);
        self.add(x, y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.add(cx0, cy0);
        self.add(cx1, cy1);
        self.add(x, y);
    }
    fn close(&mut self) {}
}

/// An outline into a tiny-skia path, moved to `origin` with y flipped (canvas pixels, y down).
struct PathPen {
    builder: PathBuilder,
    origin: [f32; 2],
}

impl PathPen {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (self.origin[0] + x, self.origin[1] - y)
    }
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.builder.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.builder.line_to(x, y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (cx0, cy0) = self.at(cx0, cy0);
        let (x, y) = self.at(x, y);
        self.builder.quad_to(cx0, cy0, x, y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (cx0, cy0) = self.at(cx0, cy0);
        let (cx1, cy1) = self.at(cx1, cy1);
        let (x, y) = self.at(x, y);
        self.builder.cubic_to(cx0, cy0, cx1, cy1, x, y);
    }
    fn close(&mut self) {
        self.builder.close();
    }
}

impl Canvas for FontCanvas {
    fn measure_text(&self, ch: char) -> TextMetrics {
        let mut bounds = BoundsPen::default();
        let Some(advance) = self.draw(ch, &mut bounds) else {
            return TextMetrics::default();
        };
        if !bounds.any {
            return TextMetrics { width: advance, ..TextMetrics::default() };
        }
        TextMetrics {
            width: advance,
            actual_bounding_box_ascent: f64::from(bounds.max[1]),
            actual_bounding_box_descent: f64::from(-bounds.min[1]),
            actual_bounding_box_left: f64::from(-bounds.min[0]),
            actual_bounding_box_right: f64::from(bounds.max[0]),
        }
    }

    fn fill_text(&self, ch: char, x: f64, y: f64, [left, top, width, height]: Region) -> Vec<u8> {
        let len = (width.max(0) * height.max(0)) as usize;
        let Some(mut pixmap) = Pixmap::new(width.max(0) as u32, height.max(0) as u32) else {
            return vec![0; len];
        };
        let mut pen = PathPen { builder: PathBuilder::new(), origin: [(x - f64::from(left)) as f32, (y - f64::from(top)) as f32] };
        if self.draw(ch, &mut pen).is_some()
            && let Some(path) = pen.builder.finish()
        {
            let mut paint = Paint::default();
            paint.set_color_rgba8(0, 0, 0, 255);
            paint.anti_alias = true;
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
        pixmap.data().chunks_exact(4).map(|px| px[3]).collect()
    }
}

/// What deck's `measureText(ctx, fontSize, char)` (`font-atlas-manager.js`) makes of the canvas's
/// metrics: the advance, the ink width `ceil(right − left)` and the ascent and descent rounded up,
/// or, when the canvas reports no ascent (a space, or a character with no glyph), the advance as
/// the width and the default ascent and descent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measure {
    pub advance: f64,
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
}

impl Measure {
    pub fn of(metrics: &TextMetrics, font_size: f64) -> Measure {
        // `!metrics.actualBoundingBoxAscent`: 0, −0 and NaN
        let ascent = metrics.actual_bounding_box_ascent;
        if ascent == 0.0 || ascent.is_nan() {
            return Measure { advance: metrics.width, width: metrics.width, ascent: font_size * DEFAULT_ASCENT, descent: font_size * DEFAULT_DESCENT };
        }
        Measure {
            advance: metrics.width,
            width: (metrics.actual_bounding_box_right - metrics.actual_bounding_box_left).ceil(),
            ascent: ascent.ceil(),
            descent: metrics.actual_bounding_box_descent.ceil(),
        }
    }
}

/// One character's entry in deck's atlas mapping, atlas texels: the frame drawn for it (x, y,
/// width, height), its advance, and its anchor from the frame's left and top (the pen position
/// along x, the baseline along y).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GlyphFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub advance: f64,
    pub anchor_x: f64,
    pub anchor_y: f64,
}

// ---------------------------------------------------------------------------------------------
// Layout: `text-layer/utils.js`

/// CSS `word-break` for deck's auto wrapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordBreak {
    /// Break between words; a word longer than the line breaks anywhere (deck's default).
    BreakWord,
    /// Break anywhere.
    BreakAll,
    /// Any other value: no wrapping.
    Normal,
}

/// deck's `getTextAnchor`: where the text sits along x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAnchor {
    /// The text starts at the anchor.
    Start,
    /// The text is centred on it (deck's default).
    Middle,
    /// The text ends at it.
    End,
}

impl TextAnchor {
    /// deck's `TEXT_ANCHOR`: start 1, middle 0, end −1.
    pub fn value(self) -> f64 {
        match self {
            TextAnchor::Start => 1.0,
            TextAnchor::Middle => 0.0,
            TextAnchor::End => -1.0,
        }
    }
}

/// deck's `getAlignmentBaseline`: where the text sits along y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignmentBaseline {
    /// The text's top at the anchor (it hangs below).
    Top,
    /// Centred on it (deck's default).
    Center,
    /// Its bottom at the anchor.
    Bottom,
}

impl AlignmentBaseline {
    /// deck's `ALIGNMENT_BASELINE`: top 1, center 0, bottom −1.
    pub fn value(self) -> f64 {
        match self {
            AlignmentBaseline::Top => 1.0,
            AlignmentBaseline::Center => 0.0,
            AlignmentBaseline::Bottom => -1.0,
        }
    }
}

/// A TextLayer's layout props.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextLayout {
    pub anchor: TextAnchor,
    pub baseline: AlignmentBaseline,
    /// `lineHeight`, in multiples of the font size (deck's default 1).
    pub line_height: f64,
    pub word_break: WordBreak,
    /// `maxWidth`, in multiples of the font size; −1 (deck's default) for no wrapping.
    pub max_width: f64,
}

impl TextLayout {
    /// deck's defaults: middle, center, line height 1, `break-word`, no maximum width.
    pub const DEFAULT: TextLayout =
        TextLayout { anchor: TextAnchor::Middle, baseline: AlignmentBaseline::Center, line_height: 1.0, word_break: WordBreak::BreakWord, max_width: -1.0 };

    /// The defaults with the text starting at the anchor, vertically centred (the aircraft labels,
    /// signal names).
    pub const START: TextLayout = TextLayout { anchor: TextAnchor::Start, ..Self::DEFAULT };
}

impl Default for TextLayout {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `transformParagraph`'s result, texture pixels: per character its x (the cursor plus its
/// frame's `anchorX`), its row's y (the row's baseline offset) and its row's width; and the
/// paragraph's size (the widest row, rows × line height).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paragraph {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub row_width: Vec<f64>,
    pub size: [f64; 2],
}

/// `getTextWidth`: the advances of `text[start..end]` (an unmapped character counts 0).
fn text_width(text: &[char], start: usize, end: usize, mapping: &impl Fn(char) -> Option<GlyphFrame>) -> f64 {
    let mut width = 0.0;
    for &ch in &text[start..end] {
        // `mapping[character]?.advance || 0`
        width += mapping(ch).map(|f| f.advance).filter(|a| !a.is_nan()).unwrap_or(0.0);
    }
    width
}

/// `breakAll`: the row breaks of `text[start..end]` anywhere a character would pass `max_width`,
/// pushed onto `target`; the last row's width.
fn break_all(text: &[char], start: usize, end: usize, max_width: f64, mapping: &impl Fn(char) -> Option<GlyphFrame>, target: &mut Vec<usize>) -> f64 {
    let mut row_start = start;
    let mut row_offset_left = 0.0;
    for i in start..end {
        // 2. figure out where to break lines
        let width = text_width(text, i, i + 1, mapping);
        if row_offset_left + width > max_width {
            if row_start < i {
                target.push(i);
            }
            row_start = i;
            row_offset_left = 0.0;
        }
        row_offset_left += width;
    }
    row_offset_left
}

/// `breakWord`: the row breaks of `text[start..end]` between words (a space ends a group, and so
/// does the character before a space and the last one); a group wider than `max_width` breaks
/// anywhere.
fn break_word(text: &[char], start: usize, end: usize, max_width: f64, mapping: &impl Fn(char) -> Option<GlyphFrame>, target: &mut Vec<usize>) -> f64 {
    let mut row_start = start;
    let mut group_start = start;
    let mut group_end = start;
    let mut row_offset_left = 0.0;
    for i in start..end {
        // 1. break text into word groups: at a space, before a space, at the last character
        if text[i] == ' ' || text.get(i + 1) == Some(&' ') || i + 1 == end {
            group_end = i + 1;
        }
        if group_end > group_start {
            // 2. break text into next row at maxWidth
            let mut group_width = text_width(text, group_start, group_end, mapping);
            if row_offset_left + group_width > max_width {
                if row_start < group_start {
                    target.push(group_start);
                    row_start = group_start;
                    row_offset_left = 0.0;
                }
                // a group wider than the line breaks anywhere
                if group_width > max_width {
                    group_width = break_all(text, group_start, group_end, max_width, mapping, target);
                    // the reference moves to the last row
                    row_start = target.last().copied().unwrap_or(row_start);
                }
            }
            group_start = group_end;
            row_offset_left += group_width;
        }
    }
    row_offset_left
}

/// `autoWrapping`: where rows of `text[start..end]` break so none passes `max_width`.
pub fn auto_wrapping(text: &[char], word_break: WordBreak, max_width: f64, mapping: &impl Fn(char) -> Option<GlyphFrame>, start: usize, end: usize) -> Vec<usize> {
    let mut result = Vec::new();
    if word_break == WordBreak::BreakAll {
        break_all(text, start, end, max_width, mapping, &mut result);
    } else {
        break_word(text, start, end, max_width, mapping, &mut result);
    }
    result
}

/// `transformRow`: each character's x in `line[start..end]` into `left_offsets` (cursor plus
/// `anchorX`; an unmapped character takes [`MISSING_CHAR_WIDTH`]); the row's width and height.
fn transform_row(line: &[char], start: usize, end: usize, mapping: &impl Fn(char) -> Option<GlyphFrame>, left_offsets: &mut [f64]) -> [f64; 2] {
    let mut x = 0.0;
    let mut row_height: f64 = 0.0;
    for &ch in &line[start..end] {
        if let Some(frame) = mapping(ch) {
            row_height = row_height.max(frame.height);
        }
    }
    for i in start..end {
        match mapping(line[i]) {
            Some(frame) => {
                left_offsets[i] = x + frame.anchor_x;
                x += frame.advance;
            }
            None => {
                left_offsets[i] = x;
                x += MISSING_CHAR_WIDTH;
            }
        }
    }
    [x, row_height]
}

/// `transformParagraph(paragraph, baselineOffset, lineHeight, wordBreak, maxWidth, iconMapping)`:
/// the characters of `paragraph` laid out in rows, `\n` starting a new line (an empty line takes no
/// row), rows wrapped when `word_break` is `BreakWord` or `BreakAll` and `max_width` is finite and
/// positive. Texture pixels; the first row's middle is at `line_height / 2`, its baseline offset
/// `baseline_offset` below that.
pub fn transform_paragraph(
    paragraph: &[char],
    baseline_offset: f64,
    line_height: f64,
    word_break: WordBreak,
    max_width: f64,
    mapping: &impl Fn(char) -> Option<GlyphFrame>,
) -> Paragraph {
    let characters = paragraph;
    let n = characters.len();
    let mut x = vec![0.0; n];
    let mut y = vec![0.0; n];
    let mut row_width = vec![0.0; n];
    let wrapping = matches!(word_break, WordBreak::BreakWord | WordBreak::BreakAll) && max_width.is_finite() && max_width > 0.0;

    let mut size = [0.0f64, 0.0];
    let mut row_count = 0usize;
    // this places the top of the first row at 0
    let mut row_offset_top = baseline_offset + line_height / 2.0;
    let mut line_start = 0;
    let mut line_end = 0;

    for i in 0..=n {
        let ch = characters.get(i).copied();
        if ch == Some('\n') || i == n {
            line_end = i;
        }
        if line_end > line_start {
            let rows = if wrapping { auto_wrapping(characters, word_break, max_width, mapping, line_start, line_end) } else { Vec::new() };
            for row_index in 0..=rows.len() {
                let row_start = if row_index == 0 { line_start } else { rows[row_index - 1] };
                let row_end = if row_index < rows.len() { rows[row_index] } else { line_end };
                let row_size = transform_row(characters, row_start, row_end, mapping, &mut x);
                for j in row_start..row_end {
                    y[j] = row_offset_top;
                    row_width[j] = row_size[0];
                }
                row_count += 1;
                row_offset_top += line_height;
                size[0] = size[0].max(row_size[0]);
            }
            line_start = line_end;
        }
        if ch == Some('\n') {
            // so that the result has one entry per character
            x[line_start] = 0.0;
            y[line_start] = 0.0;
            row_width[line_start] = 0.0;
            line_start += 1;
        }
    }
    // last row
    size[1] = row_count as f64 * line_height;
    Paragraph { x, y, row_width, size }
}

/// TextLayer's `getIconOffsets`: each character's offset from the label's anchor, texture pixels
/// (x right, y down): `((anchorX − 1)·rowWidth)/2 + x`, `((anchorY − 1)·height)/2 + y`.
pub fn icon_offsets(p: &Paragraph, anchor: TextAnchor, baseline: AlignmentBaseline) -> Vec<[f64; 2]> {
    let (ax, ay) = (anchor.value(), baseline.value());
    let height = p.size[1];
    (0..p.x.len()).map(|i| [((ax - 1.0) * p.row_width[i]) / 2.0 + p.x[i], ((ay - 1.0) * height) / 2.0 + p.y[i]]).collect()
}

/// TextLayer's `getBoundingRect`: the text's box from its anchor, texture pixels: x, y (y down),
/// width, height.
pub fn bounding_rect(p: &Paragraph, anchor: TextAnchor, baseline: AlignmentBaseline) -> [f64; 4] {
    let [width, height] = p.size;
    [((anchor.value() - 1.0) * width) / 2.0, ((baseline.value() - 1.0) * height) / 2.0, width, height]
}

// ---------------------------------------------------------------------------------------------
// The atlas

/// One glyph of a [`LaidText`]: its atlas frame (x, y, width, height texels) and the offset of
/// the frame's middle from the label's anchor, texture pixels, x right and y down (MultiIconLayer's
/// `instanceOffsets`: `[offsetX, height/2 − anchorY + offsetY]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaidGlyph {
    pub ch: char,
    pub frame: [f64; 4],
    pub offset: [f64; 2],
}

/// A string laid out in an atlas.
#[derive(Debug, Clone, PartialEq)]
pub struct LaidText {
    /// The glyphs with ink, in string order (spaces, line breaks and skipped characters draw
    /// nothing and are left out).
    pub glyphs: Vec<LaidGlyph>,
    /// [`bounding_rect`]: the text's box from its anchor, texture pixels.
    pub rect: [f64; 4],
    /// The atlas's font size: texture pixels per `size` pixel of text.
    pub font_size: f64,
    /// The atlas's [`GlyphAtlas::generation`] when it was laid out.
    pub generation: u64,
}

impl LaidText {
    /// The text's box from its anchor in CSS pixels (x right, y down) for a label `size_px`
    /// pixels tall (deck's `getSize · sizeScale`), before its pixel offset.
    pub fn box_px(&self, size_px: f64) -> [f64; 4] {
        let k = size_px / self.font_size;
        self.rect.map(|v| v * k)
    }
}

/// One character in the atlas.
#[derive(Debug, Clone, Copy)]
struct Slot {
    frame: GlyphFrame,
    /// Its picture in the atlas (left, top, width, height), `None` for a glyph without ink or
    /// one that found no room.
    cell: Option<[u32; 4]>,
    /// Whether it has ink to draw.
    inked: bool,
    /// The use clock when last laid out, and when added.
    used: u64,
    added: u64,
}

/// One face in the atlas: its canvas and `baselineOffset`.
#[derive(Debug, Clone)]
struct FaceState {
    face: Face,
    canvas: FontCanvas,
    baseline_offset: f64,
}

/// Rows of cells, left to right, a new row below the tallest cell of the last.
#[derive(Debug, Clone, Copy, Default)]
struct Shelf {
    x: u32,
    y: u32,
    row_height: u32,
}

impl Shelf {
    fn place(&mut self, [w, h]: [u32; 2], [width, height]: [u32; 2]) -> Option<[u32; 2]> {
        if w > width {
            return None;
        }
        if self.x + w > width {
            self.x = 0;
            self.y += self.row_height;
            self.row_height = 0;
        }
        if self.y + h > height {
            return None;
        }
        let at = [self.x, self.y];
        self.x += w;
        self.row_height = self.row_height.max(h);
        Some(at)
    }

    /// Takes back the cell just placed at `at` (its row keeps its height).
    fn unplace(&mut self, at: [u32; 2], [w, _]: [u32; 2]) {
        if self.x == at[0] + w && self.y == at[1] {
            self.x = at[0];
        }
    }
}

/// deck's font atlas for one [`FontSettings`], filled on demand (see the module docs).
#[derive(Clone)]
pub struct GlyphAtlas {
    settings: FontSettings,
    size: [u32; 2],
    pixels: Vec<u8>,
    sdf: TinySdf,
    faces: Vec<FaceState>,
    glyphs: HashMap<(Face, char), Slot>,
    shelf: Shelf,
    dirty: Option<[u32; 4]>,
    generation: u64,
    clock: u64,
    added: u64,
}

impl fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("settings", &self.settings)
            .field("size", &self.size)
            .field("glyphs", &self.glyphs.len())
            .field("generation", &self.generation)
            .finish()
    }
}

impl GlyphAtlas {
    /// An empty [`ATLAS_SIZE`]² atlas.
    pub fn new(settings: FontSettings) -> GlyphAtlas {
        Self::with_size(settings, [ATLAS_SIZE; 2])
    }

    /// An empty atlas of `size` texels.
    pub fn with_size(settings: FontSettings, size: [u32; 2]) -> GlyphAtlas {
        GlyphAtlas {
            settings,
            size,
            pixels: vec![0; (size[0] * size[1]) as usize],
            sdf: TinySdf::new(settings.font_size, settings.buffer, settings.radius, settings.cutoff),
            faces: Vec::new(),
            glyphs: HashMap::new(),
            shelf: Shelf::default(),
            dirty: None,
            generation: 0,
            clock: 0,
            added: 0,
        }
    }

    pub fn settings(&self) -> &FontSettings {
        &self.settings
    }

    /// Width and height, texels.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// The picture, one byte per texel, rows top first: SDF values or coverage.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How many times the atlas has been packed again since it was made (each time moves its
    /// glyphs, so text laid out before is stale).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// How many characters it holds (those without ink included).
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Moves the use clock on: call once per scene refresh, so that a rebuild keeps the glyphs
    /// laid out most recently.
    pub fn tick(&mut self) {
        self.clock += 1;
    }

    /// The part of the picture changed since the last call (left, top, width, height), to upload.
    pub fn take_dirty(&mut self) -> Option<[u32; 4]> {
        self.dirty.take()
    }

    fn mark_dirty(&mut self, [x, y, w, h]: [u32; 4]) {
        self.dirty = Some(match self.dirty {
            None => [x, y, w, h],
            Some([dx, dy, dw, dh]) => {
                let (x0, y0) = (dx.min(x), dy.min(y));
                let (x1, y1) = ((dx + dw).max(x + w), (dy + dh).max(y + h));
                [x0, y0, x1 - x0, y1 - y0]
            }
        });
    }

    fn face_index(&mut self, face: Face) -> usize {
        if let Some(i) = self.faces.iter().position(|f| f.face == face) {
            return i;
        }
        let canvas = FontCanvas::new(face, self.settings.font_size);
        // `measureText(ctx, fontSize)` with no character
        let font_size = self.settings.font_size;
        let (ascent, descent) = match canvas.font_box() {
            Some(b) if b.ascent != 0.0 => (b.ascent.ceil(), b.descent.ceil()),
            _ => (font_size * DEFAULT_ASCENT, font_size * DEFAULT_DESCENT),
        };
        self.faces.push(FaceState { face, canvas, baseline_offset: (ascent - descent) / 2.0 });
        self.faces.len() - 1
    }

    /// The font atlas's `baselineOffset` for `face`: half its ascent minus its descent.
    pub fn baseline_offset(&mut self, face: Face) -> f64 {
        let i = self.face_index(face);
        self.faces[i].baseline_offset
    }

    /// `ch`'s frame in `face`, drawn into the atlas first if it is new.
    pub fn frame(&mut self, face: Face, ch: char) -> GlyphFrame {
        self.ensure(face, ch).frame
    }

    fn ensure(&mut self, face: Face, ch: char) -> Slot {
        let clock = self.clock;
        if let Some(slot) = self.glyphs.get_mut(&(face, ch)) {
            if slot.cell.is_some() || !slot.inked {
                slot.used = clock;
                return *slot;
            }
            // it found no room last time: try again
            self.glyphs.remove(&(face, ch));
        }
        let fi = self.face_index(face);
        let canvas = self.faces[fi].canvas.clone();
        let s = self.settings;
        let buffer = self.sdf.buffer;
        let mut slot = Slot { frame: GlyphFrame::default(), cell: None, inked: false, used: clock, added: self.added };
        self.added += 1;
        if canvas.has(ch) {
            let measure = Measure::of(&canvas.measure_text(ch), s.font_size);
            if s.sdf {
                // `getSdfFontRenderer`: the whole SDF picture is the frame; the pen is
                // `buffer − glyph_left` columns in (deck 9.4 says `buffer`: see the module docs)
                let glyph = self.sdf.draw(&canvas, ch);
                let (w, h) = (glyph.width, glyph.height);
                let left = f64::from(buffer - glyph.glyph_left);
                slot.frame = GlyphFrame {
                    x: 0.0,
                    y: 0.0,
                    width: f64::from(w),
                    height: f64::from(h),
                    advance: measure.advance,
                    anchor_x: f64::from(w) / 2.0 - left,
                    anchor_y: measure.ascent + f64::from(buffer),
                };
                slot.inked = glyph.glyph_width > 0 && glyph.glyph_height > 0;
                if slot.inked
                    && let Some([x, y]) = self.allocate([w, h])
                {
                    self.blit([x, y, w, h], &glyph.data);
                    slot.frame.x = f64::from(x);
                    slot.frame.y = f64::from(y);
                    slot.cell = Some([x, y, w, h]);
                }
            } else {
                // the plain atlas: the measured cell with `buffer` round it, drawn with `fillText`
                // at the pen
                let b = buffer.max(0) as u32;
                let height = measure.ascent + measure.descent;
                let cell = [measure.width.max(0.0).ceil() as u32 + 2 * b, height.max(0.0).ceil() as u32 + 2 * b];
                slot.frame = GlyphFrame {
                    x: 0.0,
                    y: 0.0,
                    width: measure.width,
                    height,
                    advance: measure.advance,
                    anchor_x: measure.width / 2.0,
                    anchor_y: measure.ascent,
                };
                if measure.width > 0.0
                    && let Some([x, y]) = self.allocate(cell)
                {
                    let (fx, fy) = (f64::from(x + b), f64::from(y + b));
                    let alpha = canvas.fill_text(ch, fx, fy + measure.ascent, [x as i32, y as i32, cell[0] as i32, cell[1] as i32]);
                    slot.frame.x = fx;
                    slot.frame.y = fy;
                    slot.inked = alpha.iter().any(|&a| a != 0);
                    if slot.inked {
                        self.blit([x, y, cell[0], cell[1]], &alpha);
                        slot.cell = Some([x, y, cell[0], cell[1]]);
                    } else {
                        // nothing drawn: give the cell back if it was the last one placed
                        self.shelf.unplace([x, y], cell);
                    }
                } else if measure.width > 0.0 {
                    slot.inked = true;
                }
            }
        }
        self.glyphs.insert((face, ch), slot);
        slot
    }

    /// A free cell of `[w, h]`, packing the atlas again (most recently used glyphs first) if there
    /// is none; `None` if it does not fit even then.
    fn allocate(&mut self, cell: [u32; 2]) -> Option<[u32; 2]> {
        if let Some(at) = self.shelf.place(cell, self.size) {
            return Some(at);
        }
        self.rebuild();
        self.shelf.place(cell, self.size)
    }

    /// Packs the atlas again: the glyphs with ink, most recently used (then most recently added)
    /// first. Those used at the current clock are kept while they fit; older ones only while the
    /// rows reach no further than half the atlas, so that the glyph that did not fit finds room.
    /// The rest are dropped and drawn again when next needed.
    fn rebuild(&mut self) {
        let old = std::mem::replace(&mut self.pixels, vec![0; (self.size[0] * self.size[1]) as usize]);
        let mut inked: Vec<((Face, char), Slot)> = self.glyphs.iter().filter(|(_, s)| s.cell.is_some()).map(|(k, s)| (*k, *s)).collect();
        inked.sort_by_key(|(_, s)| std::cmp::Reverse((s.used, s.added)));
        self.shelf = Shelf::default();
        let mut full = false;
        let half = self.size[1] / 2;
        for (key, mut slot) in inked {
            let Some([cx, cy, w, h]) = slot.cell else { continue };
            let current = slot.used == self.clock;
            let mut at = if full { None } else { self.shelf.place([w, h], self.size) };
            if !current && at.is_some_and(|[_, y]| y + h > half) {
                at = None;
            }
            match at {
                Some([x, y]) => {
                    for row in 0..h {
                        let from = ((cy + row) * self.size[0] + cx) as usize;
                        let to = ((y + row) * self.size[0] + x) as usize;
                        self.pixels[to..to + w as usize].copy_from_slice(&old[from..from + w as usize]);
                    }
                    slot.frame.x += f64::from(x) - f64::from(cx);
                    slot.frame.y += f64::from(y) - f64::from(cy);
                    slot.cell = Some([x, y, w, h]);
                    self.glyphs.insert(key, slot);
                }
                None => {
                    full = true;
                    self.glyphs.remove(&key);
                }
            }
        }
        self.generation += 1;
        self.dirty = Some([0, 0, self.size[0], self.size[1]]);
    }

    /// Copies `data` (`w × h`, rows top first) into the atlas at `(x, y)`.
    fn blit(&mut self, [x, y, w, h]: [u32; 4], data: &[u8]) {
        for row in 0..h {
            let to = ((y + row) * self.size[0] + x) as usize;
            let from = (row * w) as usize;
            self.pixels[to..to + w as usize].copy_from_slice(&data[from..from + w as usize]);
        }
        self.mark_dirty([x, y, w, h]);
    }

    /// `text` in `face`, laid out as TextLayer does with `layout`: each glyph with ink, its frame
    /// and offset, and the text's box. New characters are drawn into the atlas first.
    pub fn layout(&mut self, face: Face, text: &str, layout: &TextLayout) -> LaidText {
        let chars: Vec<char> = text.chars().collect();
        for &ch in &chars {
            self.ensure(face, ch);
        }
        // a rebuild while drawing the last characters may have moved the first ones: read the
        // frames after all are in
        let slots: HashMap<char, Slot> = chars.iter().filter_map(|&ch| self.glyphs.get(&(face, ch)).map(|s| (ch, *s))).collect();
        let mapping = |ch: char| slots.get(&ch).map(|s| s.frame);
        let font_size = self.settings.font_size;
        let baseline_offset = self.baseline_offset(face);
        let paragraph = transform_paragraph(&chars, baseline_offset, layout.line_height * font_size, layout.word_break, layout.max_width * font_size, &mapping);
        let offsets = icon_offsets(&paragraph, layout.anchor, layout.baseline);
        let glyphs = chars
            .iter()
            .zip(&offsets)
            .filter_map(|(&ch, &[ox, oy])| {
                let slot = slots.get(&ch)?;
                slot.cell?;
                let f = slot.frame;
                Some(LaidGlyph { ch, frame: [f.x, f.y, f.width, f.height], offset: [ox, f.height / 2.0 - f.anchor_y + oy] })
            })
            .collect();
        LaidText { glyphs, rect: bounding_rect(&paragraph, layout.anchor, layout.baseline), font_size, generation: self.generation }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mapping where every character is 10 wide (advance), its frame 12 × 20 with the anchor at
    /// (6, 15); `#` is unmapped.
    fn fixed(ch: char) -> Option<GlyphFrame> {
        (ch != '#').then_some(GlyphFrame { x: 0.0, y: 0.0, width: 12.0, height: 20.0, advance: 10.0, anchor_x: 6.0, anchor_y: 15.0 })
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn one_row_is_laid_out_by_advances() {
        let p = transform_paragraph(&chars("abc"), 5.0, 64.0, WordBreak::BreakWord, -64.0, &fixed);
        assert_eq!(p.x, vec![6.0, 16.0, 26.0]);
        assert_eq!(p.y, vec![37.0; 3]);
        assert_eq!(p.row_width, vec![30.0; 3]);
        assert_eq!(p.size, [30.0, 64.0]);
    }

    #[test]
    fn line_breaks_start_rows_and_empty_lines_take_none() {
        let p = transform_paragraph(&chars("ab\ncde\n\nf"), 0.0, 64.0, WordBreak::BreakWord, -1.0, &fixed);
        // rows: "ab", "cde", "f"; the empty line between takes no row
        assert_eq!(p.size, [30.0, 192.0]);
        assert_eq!(p.x, vec![6.0, 16.0, 0.0, 6.0, 16.0, 26.0, 0.0, 0.0, 6.0]);
        assert_eq!(p.y, vec![32.0, 32.0, 0.0, 96.0, 96.0, 96.0, 0.0, 0.0, 160.0]);
        assert_eq!(p.row_width, vec![20.0, 20.0, 0.0, 30.0, 30.0, 30.0, 0.0, 0.0, 10.0]);
    }

    #[test]
    fn an_unmapped_character_takes_the_missing_width() {
        let p = transform_paragraph(&chars("a#b"), 0.0, 64.0, WordBreak::Normal, -1.0, &fixed);
        assert_eq!(p.x, vec![6.0, 10.0, 48.0]);
        assert_eq!(p.size[0], 52.0);
    }

    #[test]
    fn break_word_wraps_between_words() {
        // "aa bb cc" at 10 per character, at most 55 wide: "aa bb" is 50 and its space would make
        // 60, so deck breaks before the space and the second row is " cc"
        let text = chars("aa bb cc");
        assert_eq!(auto_wrapping(&text, WordBreak::BreakWord, 55.0, &fixed, 0, text.len()), vec![5]);
        let p = transform_paragraph(&text, 0.0, 64.0, WordBreak::BreakWord, 55.0, &fixed);
        assert_eq!(p.size, [50.0, 128.0]);
        assert_eq!(p.y, vec![32.0, 32.0, 32.0, 32.0, 32.0, 96.0, 96.0, 96.0]);
        assert_eq!(p.x, vec![6.0, 16.0, 26.0, 36.0, 46.0, 6.0, 16.0, 26.0]);
        assert_eq!(p.row_width, vec![50.0, 50.0, 50.0, 50.0, 50.0, 30.0, 30.0, 30.0]);
        // at most 25: each word on its own row, and each space too (deck's groups: a space is one)
        assert_eq!(auto_wrapping(&text, WordBreak::BreakWord, 25.0, &fixed, 0, text.len()), vec![2, 3, 5, 6]);
        // wide enough for all: no breaks; and Normal never wraps
        assert!(auto_wrapping(&text, WordBreak::BreakWord, 80.0, &fixed, 0, text.len()).is_empty());
        let p = transform_paragraph(&text, 0.0, 64.0, WordBreak::Normal, 55.0, &fixed);
        assert_eq!(p.size, [80.0, 64.0]);
    }

    #[test]
    fn a_word_longer_than_the_line_breaks_anywhere() {
        let text = chars("abcdefg hi");
        // break-word: the long word breaks at 30, then "hi" follows on the last row
        assert_eq!(auto_wrapping(&text, WordBreak::BreakWord, 30.0, &fixed, 0, text.len()), vec![3, 6, 8]);
        assert_eq!(auto_wrapping(&text, WordBreak::BreakAll, 30.0, &fixed, 0, text.len()), vec![3, 6, 9]);
    }

    #[test]
    fn offsets_and_box_follow_the_anchor_and_baseline() {
        let p = transform_paragraph(&chars("ab\nc"), 4.0, 64.0, WordBreak::BreakWord, -1.0, &fixed);
        // two rows, 20 and 10 wide; 128 tall
        let start = icon_offsets(&p, TextAnchor::Start, AlignmentBaseline::Top);
        assert_eq!(start, vec![[6.0, 36.0], [16.0, 36.0], [0.0, 0.0], [6.0, 100.0]]);
        let middle = icon_offsets(&p, TextAnchor::Middle, AlignmentBaseline::Center);
        assert_eq!(middle, vec![[-4.0, -28.0], [6.0, -28.0], [0.0, -64.0], [1.0, 36.0]]);
        let end = icon_offsets(&p, TextAnchor::End, AlignmentBaseline::Bottom);
        assert_eq!(end, vec![[-14.0, -92.0], [-4.0, -92.0], [0.0, -128.0], [-4.0, -28.0]]);
        assert_eq!(bounding_rect(&p, TextAnchor::Start, AlignmentBaseline::Top), [0.0, 0.0, 20.0, 128.0]);
        assert_eq!(bounding_rect(&p, TextAnchor::Middle, AlignmentBaseline::Center), [-10.0, -64.0, 20.0, 128.0]);
        assert_eq!(bounding_rect(&p, TextAnchor::End, AlignmentBaseline::Bottom), [-20.0, -128.0, 20.0, 128.0]);
    }

    #[test]
    fn the_fonts_have_their_weight_axes() {
        for (family, range) in [(Family::Overpass, (100.0, 900.0)), (Family::OverpassMono, (300.0, 700.0))] {
            for bytes in family.files() {
                let font = FontRef::new(bytes).expect("parses");
                let axis = font.axes().iter().find(|a| a.tag() == skrifa::Tag::new(b"wght")).expect("a wght axis");
                assert_eq!((axis.min_value(), axis.max_value()), range);
            }
        }
    }

    #[test]
    fn heavier_weights_have_more_ink() {
        let ink = |weight| {
            let canvas = FontCanvas::new(Face::new(Family::Overpass, weight), 64.0);
            canvas.fill_text('o', 10.0, 70.0, [0, 0, 80, 80]).iter().map(|&a| u32::from(a)).sum::<u32>()
        };
        let (light, label, bold) = (ink(300), ink(600), ink(800));
        assert!(light < label && label < bold, "{light} {label} {bold}");
    }

    #[test]
    fn the_canvas_measures_outlines() {
        let canvas = FontCanvas::new(Face::LABEL, 64.0);
        let s = canvas.measure_text('S');
        assert!(s.width > 25.0 && s.width < 50.0, "{s:?}");
        assert!(s.actual_bounding_box_left < 0.0, "S starts right of the pen: {s:?}");
        assert!(s.actual_bounding_box_right < s.width && s.actual_bounding_box_ascent > 40.0, "{s:?}");
        // a capital sits on the baseline (its round bottom just below it)
        assert!(s.actual_bounding_box_descent.abs() < 2.0, "{s:?}");
        let space = canvas.measure_text(' ');
        assert!(space.width > 5.0 && space.actual_bounding_box_ascent == 0.0, "{space:?}");
        let g = canvas.measure_text('g');
        assert!(g.actual_bounding_box_descent > 8.0, "{g:?}");
        // a character no subset has: nothing at all
        assert!(!canvas.has('中'));
        assert_eq!(canvas.measure_text('中'), TextMetrics::default());
        // Cyrillic and Vietnamese come from their subsets
        assert!(canvas.has('Ж') && canvas.has('ữ') && canvas.has('Ł'));
        let font_box = canvas.font_box().expect("metrics");
        assert!(font_box.ascent > 40.0 && font_box.descent > 5.0, "{font_box:?}");
        assert_eq!(font_box.ascent.fract(), 0.0);
    }

    #[test]
    fn deck_measures_a_space_with_the_default_ascent() {
        let m = Measure::of(&TextMetrics { width: 15.5, ..TextMetrics::default() }, 64.0);
        assert_eq!(m, Measure { advance: 15.5, width: 15.5, ascent: 64.0 * 0.9, descent: 64.0 * 0.3 });
        let m = Measure::of(
            &TextMetrics {
                width: 40.0,
                actual_bounding_box_ascent: 45.2,
                actual_bounding_box_descent: 0.4,
                actual_bounding_box_left: -2.5,
                actual_bounding_box_right: 37.25,
            },
            64.0,
        );
        // deck's width is `ceil(right − left)`, wider than the ink by the left bearing
        assert_eq!(m, Measure { advance: 40.0, width: 40.0, ascent: 46.0, descent: 1.0 });
    }

    #[test]
    fn sdf_glyphs_are_framed_as_deck_frames_them() {
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let canvas = FontCanvas::new(Face::LABEL, 64.0);
        let s = atlas.frame(Face::LABEL, 'S');
        let glyph = TinySdf::new(64.0, 6.0, 12.0, 0.25).draw(&canvas, 'S');
        assert_eq!((s.width, s.height), (f64::from(glyph.width), f64::from(glyph.height)));
        // the pen is `buffer − glyph_left` columns into the picture: 3 for Overpass's S
        assert_eq!(glyph.glyph_left, 3);
        assert_eq!(s.anchor_x, s.width / 2.0 - f64::from(6 - glyph.glyph_left));
        assert_eq!(s.anchor_y, f64::from(glyph.glyph_top) + 6.0);
        assert_eq!(s.advance, canvas.measure_text('S').width);
        // the picture is TinySDF's, where the frame says
        let [w, _] = atlas.size();
        for row in 0..glyph.height {
            let at = ((s.y as u32 + row) * w + s.x as u32) as usize;
            assert_eq!(&atlas.pixels()[at..at + glyph.width as usize], &glyph.data[(row * glyph.width) as usize..((row + 1) * glyph.width) as usize]);
        }
        assert_eq!(atlas.take_dirty(), Some([s.x as u32, s.y as u32, glyph.width, glyph.height]));
        assert_eq!(atlas.take_dirty(), None);
        // asked again: the same frame, nothing drawn
        assert_eq!(atlas.frame(Face::LABEL, 'S'), s);
        assert_eq!(atlas.take_dirty(), None);
    }

    #[test]
    fn stockholm_is_laid_out_centred_on_its_anchor() {
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let laid = atlas.layout(Face::LABEL, "Stockholm", &TextLayout::DEFAULT);
        assert_eq!(laid.glyphs.iter().map(|g| g.ch).collect::<String>(), "Stockholm");
        let advances: f64 = "Stockholm".chars().map(|c| atlas.frame(Face::LABEL, c).advance).sum();
        let baseline = atlas.baseline_offset(Face::LABEL);
        assert_eq!(laid.rect, [-advances / 2.0, -32.0, advances, 64.0]);
        // each glyph's frame middle: the pen plus the frame's anchor along x; along y the
        // baseline `baseline_offset` below the anchor
        assert_eq!(baseline, 16.0);
        let mut pen = -advances / 2.0;
        for g in &laid.glyphs {
            let f = atlas.frame(Face::LABEL, g.ch);
            assert_eq!(g.frame, [f.x, f.y, f.width, f.height]);
            assert!((g.offset[0] - (pen + f.anchor_x)).abs() < 1e-9, "{g:?}");
            let baseline_in_frame = g.offset[1] - f.height / 2.0 + f.anchor_y;
            assert!((baseline_in_frame - baseline).abs() < 1e-9, "{g:?}");
            pen += f.advance;
        }
        // 14 px text: the box scales by 14/64
        assert_eq!(laid.box_px(14.0), laid.rect.map(|v| v * 14.0 / 64.0));
        // the frames do not overlap
        for (i, a) in laid.glyphs.iter().enumerate() {
            for b in &laid.glyphs[i + 1..] {
                if a.ch == b.ch {
                    assert_eq!(a.frame, b.frame);
                    continue;
                }
                let apart = a.frame[0] + a.frame[2] <= b.frame[0] || b.frame[0] + b.frame[2] <= a.frame[0] || a.frame[1] + a.frame[3] <= b.frame[1] || b.frame[1] + b.frame[3] <= a.frame[1];
                assert!(apart, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn spaces_and_missing_characters_draw_nothing() {
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let laid = atlas.layout(Face::LABEL, "a 中b", &TextLayout::START);
        assert_eq!(laid.glyphs.iter().map(|g| g.ch).collect::<String>(), "ab");
        let missing = atlas.frame(Face::LABEL, '中');
        assert_eq!(missing.advance, 0.0);
        let (a, space, b) = (atlas.frame(Face::LABEL, 'a'), atlas.frame(Face::LABEL, ' '), atlas.frame(Face::LABEL, 'b'));
        // start anchor: the pen starts at 0; the space advances, the missing character does not
        assert!((laid.glyphs[0].offset[0] - a.anchor_x).abs() < 1e-9);
        assert!((laid.glyphs[1].offset[0] - (a.advance + space.advance + b.anchor_x)).abs() < 1e-9);
        assert!((laid.rect[2] - (a.advance + space.advance + b.advance)).abs() < 1e-9);
    }

    #[test]
    fn a_full_atlas_keeps_the_recently_used_glyphs() {
        // room for two rows of about ten label glyphs
        let mut atlas = GlyphAtlas::with_size(FontSettings::LABELS, [512, 160]);
        let first = atlas.layout(Face::LABEL, "abcdefgh", &TextLayout::DEFAULT);
        atlas.tick();
        let kept = atlas.layout(Face::LABEL, "Stock", &TextLayout::DEFAULT);
        assert_eq!(atlas.generation(), 0);
        let before: Vec<Vec<u8>> = kept.glyphs.iter().map(|g| cell_pixels(&atlas, g.frame)).collect();
        atlas.tick();
        let mut more = String::new();
        for ch in 'i'..='z' {
            more.push(ch);
            let laid = atlas.layout(Face::LABEL, &more, &TextLayout::DEFAULT);
            assert_eq!(laid.glyphs.len(), more.len(), "{more}: every glyph of the text being laid out is in");
            if atlas.generation() > 0 {
                break;
            }
        }
        assert!(atlas.generation() > 0, "the atlas filled");
        assert_eq!(atlas.take_dirty(), Some([0, 0, 512, 160]));
        // the oldest glyphs went to make room
        assert!(atlas.len() < 8 + 5 + more.len(), "{} glyphs", atlas.len());
        assert_ne!(first.generation, atlas.generation());
        // laid out again, the recently used glyphs have the same pictures where they moved to
        let again = atlas.layout(Face::LABEL, "Stock", &TextLayout::DEFAULT);
        for (g, pixels) in again.glyphs.iter().zip(&before) {
            assert_eq!(&cell_pixels(&atlas, g.frame), pixels, "{}", g.ch);
        }
        assert_eq!(again.rect, kept.rect);
    }

    fn cell_pixels(atlas: &GlyphAtlas, [x, y, w, h]: [f64; 4]) -> Vec<u8> {
        let width = atlas.size()[0] as usize;
        (y as usize..(y + h) as usize).flat_map(|row| atlas.pixels()[row * width + x as usize..row * width + (x + w) as usize].to_vec()).collect()
    }

    #[test]
    fn plain_glyphs_are_framed_by_their_measured_cell() {
        let face = Face::new(Family::OverpassMono, 400);
        let mut atlas = GlyphAtlas::new(FontSettings::DEFAULT);
        let canvas = FontCanvas::new(face, 64.0);
        let m = Measure::of(&canvas.measure_text('B'), 64.0);
        let b = atlas.frame(face, 'B');
        assert_eq!((b.width, b.height, b.advance), (m.width, m.ascent + m.descent, m.advance));
        assert_eq!((b.anchor_x, b.anchor_y), (m.width / 2.0, m.ascent));
        // the cell starts `buffer` before the frame; the glyph has ink inside the frame
        assert_eq!((b.x, b.y), (4.0, 4.0));
        let ink: u32 = (b.y as usize..(b.y + b.height) as usize)
            .flat_map(|row| atlas.pixels()[row * 2048 + b.x as usize..row * 2048 + (b.x + b.width) as usize].iter().map(|&a| u32::from(a)))
            .sum();
        assert!(ink > 255 * 200, "{ink}");
        // all of it: nothing in the cell's buffer ring
        let ring: u32 = (0..4).flat_map(|row| atlas.pixels()[row * 2048..row * 2048 + 60].iter().map(|&a| u32::from(a))).sum();
        assert_eq!(ring, 0);
        // a plain glyph's offset puts its pen at the cursor and its baseline on the row's
        let laid = atlas.layout(face, "B", &TextLayout::START);
        let baseline = atlas.baseline_offset(face);
        assert_eq!(laid.glyphs[0].offset, [b.width / 2.0, b.height / 2.0 - b.anchor_y + baseline]);
    }

    #[test]
    fn the_label_font_box_centres_the_line() {
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let font_box = FontCanvas::new(Face::LABEL, 64.0).font_box().expect("metrics");
        assert_eq!(atlas.baseline_offset(Face::LABEL), (font_box.ascent - font_box.descent) / 2.0);
    }
}
