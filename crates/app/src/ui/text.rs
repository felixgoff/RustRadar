//! UI text (plan 1.5; doc 06 §1.2, §29): every piece of text is a [`LayoutJob`] built here, so
//! that the rules the web build gets from CSS hold everywhere.
//!
//! - **Style**: a [`Style`] is the CSS that reaches a text: the face (`--font-ui` or `--font-data`),
//!   size, weight, colour, line height (the root's 1.4 unless a component sets one) and letter
//!   spacing (`.data` and the wordmark: −0.01 em). egui centres each row's text in its line
//!   height, as CSS's half-leading does.
//! - **The middle dot**: Overpass's `·` has no advance width (its ink lands on the next glyph),
//!   so today's CSS draws U+00B7 from a system font ("Middle Dot", app.css:8-16) and the `" · "`
//!   separators keep their gap. Here every `·` is set in Overpass Mono 400 ([`DOT_FAMILY`]), as
//!   plan section 7 decides, whatever the text's own face and weight (the system's font has no
//!   weight synthesised either: `font-synthesis: none`). Mono's cell is 0.616 em, about twice the
//!   system dots' (0.27–0.33 em), which would widen every separator by 4 px at 13 px; so the dot
//!   is narrowed to [`DOT_ADVANCE_EM`] with its ink centred (the dot moved left and the text after
//!   it pulled in, by `leading_space`), as wide as the reference pictures' dot. Deviation from
//!   plan 1.5's "a fixed advance": the advance is today's, not Mono's. Only `--font-ui` text
//!   takes this dot: `--font-data` lists no "Middle Dot" face (app.css:44-45), so Chromium draws
//!   a `.data` text's `·` from Overpass Mono itself, in its weight and spacing, a full cell wide,
//!   and [`append`] leaves it so.
//! - **Truncation**: `white-space: nowrap; overflow: hidden; text-overflow: ellipsis` is
//!   [`truncated`]: one row, cut at any character, `…` at the end, within the width.
//! - **Baselines** (item F1): egui and Chromium centre a line's text in its line box differently.
//!   Chromium rounds the font's ascent and descent to whole pixels, floors the half-leading
//!   `(line-height − (ascent + descent)) / 2` (in `LayoutUnit`s, 1/64 px, `line-height` floored to
//!   them) to a whole pixel and paints the baseline on the pixel nearest to the line's top plus the
//!   half-leading and the ascent ([`css_baseline`]); each line box is `line-height` floored to
//!   1/64 px under the one before. egui centres unrounded metrics and puts each row on a whole pixel, which drew text 0,
//!   1 or 2 px lower than Chromium (1 px at 12, 14 and 17 px at the root's 1.4) and wrapped rows
//!   up to a pixel off. [`install`] adds an egui plugin that, after every pass, moves each row of
//!   every text shape whose format names a line height (all text built here) onto Chromium's
//!   baseline for a line box whose top is the shape's position: the layout (galley sizes, rows,
//!   hit boxes, cursors) stays egui's, only the drawing moves. Code that draws next to text
//!   (underlines, swatches on the baseline, two texts on one baseline) reads the baseline from
//!   [`row_baseline`]. Measured in Playwright's Chromium on every size from 7 to 24 px at line
//!   heights 1 … 1.5, 20 px and 32 px, both faces, weights 400 and 700, at device scales 1 and 2,
//!   tops on fractions of a pixel and six-line paragraphs: the rule gives Chromium's baselines in
//!   all of them (at scale 2 Chromium also lays text out in CSS pixels and paints the baseline on a
//!   whole CSS pixel, which the hook does too).
//! - **Tabular figures**: `font-variant-numeric: tabular-nums` is [`tabular`]: the Overpass
//!   sections of a job in the `ui-tnum-*` families ([`super::fonts::tabular_family`]), whose latin
//!   file has Overpass's `tnum` lookup under a feature tag the shaper applies by default (`cargo
//!   xtask vendor-fonts`). Overpass Mono's figures are tabular already.
//!
//! [`job`], [`append`], [`wrapped`] and [`truncated`] build jobs; [`label`] lays one out and
//! paints it in the space it takes.

use std::sync::Arc;

use egui::epaint::text::ByteIndex;
use egui::epaint::{Galley, Shape, TextShape};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, FontFamily, FontId, Response, Sense, Ui, Vec2};

use super::fonts::{self, Face};
use super::theme;

/// The character the web build draws from a system font (U+00B7).
pub const MIDDLE_DOT: char = '·';
/// The ellipsis of `text-overflow: ellipsis`.
pub const ELLIPSIS: char = '…';
/// The face and weight every [`MIDDLE_DOT`] is set in.
pub const DOT_FACE: (Face, u16) = (Face::Mono, 400);

/// The family every [`MIDDLE_DOT`] is set in: Overpass Mono at 400.
pub fn dot_family() -> FontFamily {
    fonts::family(DOT_FACE.0, DOT_FACE.1)
}

/// [`dot_family`]'s name, for tests and docs.
pub const DOT_FAMILY: &str = "mono-400";
/// The advance the web build's dot takes, em: DejaVu Sans's U+00B7 (651 of 2048 units), which
/// the "Middle Dot" face finds on the Linux the reference pictures were taken on (Arial's is
/// 0.333 em).
pub const DOT_ADVANCE_EM: f32 = 651.0 / 2048.0;
/// Overpass Mono's advance (every glyph), em.
pub const MONO_ADVANCE_EM: f32 = 0.616;

/// The CSS that reaches a text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// `--font-ui` or `--font-data` (`.data`).
    pub face: Face,
    /// `font-size`, px.
    pub size: f32,
    /// `font-weight`.
    pub weight: u16,
    pub color: Color32,
    /// `line-height` as a multiple of the size.
    pub line_height: f32,
    /// `letter-spacing` in em.
    pub letter_spacing: f32,
}

impl Style {
    /// Overpass at `size` and `weight` in `color`, at the root's line height 1.4.
    pub const fn ui(size: f32, weight: u16, color: Color32) -> Style {
        Style { face: Face::Ui, size, weight, color, line_height: theme::LINE_HEIGHT, letter_spacing: 0.0 }
    }

    /// `.data`: Overpass Mono at `size` and `weight` in `color`, `letter-spacing: -0.01em` (its
    /// `tabular-nums` is the font's own), at line height 1.4.
    pub const fn data(size: f32, weight: u16, color: Color32) -> Style {
        Style { face: Face::Mono, size, weight, color, line_height: theme::LINE_HEIGHT, letter_spacing: theme::LETTER_SPACING_TIGHT }
    }

    /// The body's text: 14 px Overpass 400 in `--text-1`.
    pub const fn body() -> Style {
        Style::ui(theme::TEXT_BASE, theme::WEIGHT_REGULAR, theme::TEXT_1)
    }

    pub const fn color(self, color: Color32) -> Style {
        Style { color, ..self }
    }

    pub const fn size(self, size: f32) -> Style {
        Style { size, ..self }
    }

    pub const fn weight(self, weight: u16) -> Style {
        Style { weight, ..self }
    }

    pub const fn line_height(self, line_height: f32) -> Style {
        Style { line_height, ..self }
    }

    pub const fn letter_spacing(self, em: f32) -> Style {
        Style { letter_spacing: em, ..self }
    }

    /// The egui font of this style.
    pub fn font_id(&self) -> FontId {
        FontId::new(self.size, fonts::family(self.face, self.weight))
    }

    /// A line box's height, px.
    pub fn line_px(&self) -> f32 {
        self.size * self.line_height
    }

    /// The egui format of this style's text.
    pub fn format(&self) -> TextFormat {
        TextFormat {
            font_id: self.font_id(),
            extra_letter_spacing: self.letter_spacing * self.size,
            line_height: Some(self.line_px()),
            color: self.color,
            ..Default::default()
        }
    }

    /// The format of a [`MIDDLE_DOT`] in this style's text: the same size, colour and line box
    /// in Overpass Mono 400, with no letter spacing.
    pub fn dot_format(&self) -> TextFormat {
        TextFormat { font_id: FontId::new(self.size, dot_family()), extra_letter_spacing: 0.0, ..self.format() }
    }

    /// Where Chromium puts this style's baseline under the top of its line box, px
    /// ([`css_baseline`]).
    pub fn css_baseline(&self) -> f32 {
        css_baseline(self.size, self.line_px())
    }
}

/// Overpass's and Overpass Mono's units per em.
pub const UNITS_PER_EM: f32 = 2000.0;
/// Their ascent and descent, font units: `hhea`'s, which `OS/2` repeats as its typo metrics
/// (`USE_TYPO_METRICS` is set); the same in both faces, every subset and every weight (no `MVAR`).
pub const ASCENT: f32 = 1766.0;
pub const DESCENT: f32 = 766.0;

/// `px` in Blink's `LayoutUnit`s (1/64 px), floored, as a `line-height` is.
fn layout_units(px: f32) -> i32 {
    // the small margin keeps a product like 12 × 1.25 that f32 puts a hair under 15 on 15
    (px * 64.0 + 1e-3).floor() as i32
}

/// Where Chromium puts the baseline of Overpass text `size` px large in a line box `line_height`
/// px high, px under the box's top, a whole number: the ascent and descent rounded
/// (`SkScalarRoundToScalar`), the half-leading `(line_height − (ascent + descent)) / 2` taken in
/// `LayoutUnit`s and floored to a whole pixel (`NGInlineBoxState::CalculateLeadingSpace`), plus
/// the ascent. 13 px at 1.4 (18.1875 px): 11 + 5, ⌊1.09⌋ + 11 = 12; 12 px at 1.4: ⌊0.4⌋ + 11 = 11;
/// 13 px at 1.2: ⌊−0.7⌋ + 11 = 10.
pub fn css_baseline(size: f32, line_height: f32) -> f32 {
    let ascent = (ASCENT / UNITS_PER_EM * size + 0.5).floor();
    let descent = (DESCENT / UNITS_PER_EM * size + 0.5).floor();
    let leading = layout_units(line_height) - 64 * (ascent + descent) as i32;
    // `LayoutUnit / 2` truncates toward zero, `Floor()` then goes down to a whole pixel
    (leading / 2).div_euclid(64) as f32 + ascent
}

/// The format of the first glyph of each row of `galley` whose face has the family's own metrics
/// (Overpass or Overpass Mono, not a fallback font), with the glyph.
fn primary_glyphs(galley: &Galley) -> Vec<Option<(&egui::epaint::text::Glyph, &TextFormat)>> {
    let job = &galley.job;
    let mut row_start = 0;
    galley
        .rows
        .iter()
        .map(|row| {
            let start = row_start;
            row_start += row.char_count_including_newline().0;
            let at = row.glyphs.iter().position(|g| g.font_face_ascent == g.font_ascent && g.font_face_height == g.font_height)?;
            if job.sections.is_empty() {
                return None;
            }
            // a row has a glyph per character (the elision's `…` comes last)
            let byte = job.text.char_indices().nth(start + at).map_or(job.text.len(), |(b, _)| b);
            Some((&row.glyphs[at], job.format_at_byte(ByteIndex(byte))))
        })
        .collect()
}

/// One row as the baseline hook reads it: egui's baseline under the galley's top, and the line
/// box's height and Chromium's baseline in it when the hook moves the row.
struct RowLine {
    egui: f32,
    css: Option<(f32, f32)>,
}

fn row_lines(galley: &Galley) -> Vec<RowLine> {
    let ppp = galley.pixels_per_point;
    galley
        .rows
        .iter()
        .zip(primary_glyphs(galley))
        .map(|(row, primary)| {
            let Some((glyph, format)) = primary else {
                return RowLine { egui: row.pos.y + row.size.y, css: None };
            };
            let egui = row.pos.y + glyph.pos.y;
            // a row egui made taller than its line box (a minimum height, mixed sizes) keeps its place
            let css = format
                .line_height
                .filter(|&line| (row.size.y - (line * ppp).round() / ppp).abs() < 0.01)
                .map(|line| (line, css_baseline(format.font_id.size, line)));
            RowLine { egui, css }
        })
        .collect()
}

/// Where row `row` of `galley` has its baseline under the galley's top once [`install`]'s hook has
/// moved it, before the hook rounds it to a pixel: Chromium's line top (the line boxes before it,
/// each its `line-height` floored to 1/64 px) plus [`css_baseline`]; egui's own for a row the hook
/// leaves where it is. For underlines, swatches and texts aligned on another text's baseline.
pub fn row_baseline(galley: &Galley, row: usize) -> f32 {
    let lines = row_lines(galley);
    let tops = line_tops(galley, &lines);
    match (lines.get(row), tops.get(row)) {
        (Some(RowLine { css: Some((_, baseline)), .. }), Some(top)) => top + baseline,
        (Some(line), _) => line.egui,
        _ => lines.first().map_or(0.0, |l| l.egui),
    }
}

/// Each row's line top under the galley's top as Chromium stacks the line boxes: the one before
/// plus its `line-height` floored to 1/64 px; after a row the hook leaves alone, egui's next row.
fn line_tops(galley: &Galley, lines: &[RowLine]) -> Vec<f32> {
    let mut top = 0.0;
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let this = top;
            top = match line.css {
                Some((height, _)) => this + layout_units(height) as f32 / 64.0,
                None => galley.rows.get(i + 1).map_or(this, |next| next.pos.y),
            };
            this
        })
        .collect()
}

/// How far each row of `galley` drawn at `top` moves (points) to stand on Chromium's baseline,
/// with the tessellator putting the galley on a whole pixel when `round` (its
/// `round_text_to_pixels`); `None` when none moves.
fn row_shifts(galley: &Galley, top: f32, pixels_per_point: f32, round: bool) -> Option<Vec<f32>> {
    let ppp = pixels_per_point;
    let drawn_top = if round { (top * ppp).round() / ppp } else { top };
    let lines = row_lines(galley);
    let tops = line_tops(galley, &lines);
    let shifts: Vec<f32> = lines
        .iter()
        .zip(tops)
        .map(|(line, line_top)| match line.css {
            // Chromium paints the baseline on the nearest whole CSS pixel (half up)
            Some((_, baseline)) => ((top + line_top + baseline + 0.5).floor() * ppp).round() / ppp - (drawn_top + line.egui),
            None => 0.0,
        })
        .collect();
    shifts.iter().any(|&s| s != 0.0).then_some(shifts)
}

/// Moves `text`'s rows onto Chromium's baselines (see the module documentation).
fn to_css_baselines(text: &mut TextShape, pixels_per_point: f32, round: bool) {
    if text.angle != 0.0 {
        return;
    }
    let Some(shifts) = row_shifts(&text.galley, text.pos.y, pixels_per_point, round) else { return };
    let galley = Arc::make_mut(&mut text.galley);
    for (row, dy) in galley.rows.iter_mut().zip(shifts) {
        row.pos.y += dy;
    }
}

fn shapes_to_css_baselines(shape: &mut Shape, pixels_per_point: f32, round: bool) {
    match shape {
        Shape::Vec(shapes) => shapes.iter_mut().for_each(|s| shapes_to_css_baselines(s, pixels_per_point, round)),
        Shape::Text(text) => to_css_baselines(text, pixels_per_point, round),
        _ => {}
    }
}

/// The egui plugin of [`install`].
struct CssBaselines;

impl egui::Plugin for CssBaselines {
    fn debug_name(&self) -> &'static str {
        "rustradar::ui::text::CssBaselines"
    }

    fn output_hook(&mut self, ctx: &egui::Context, output: &mut egui::FullOutput) {
        let round = ctx.tessellation_options(|o| o.round_text_to_pixels);
        let ppp = output.pixels_per_point;
        for clipped in &mut output.shapes {
            shapes_to_css_baselines(&mut clipped.shape, ppp, round);
        }
    }
}

/// Has every text `ctx` draws from its next pass on stand on Chromium's baselines (see the module
/// documentation); once per context ([`super::fonts::install`] calls it).
pub fn install(ctx: &egui::Context) {
    ctx.add_plugin(CssBaselines);
}

/// `font-variant-numeric: tabular-nums` on `job`: its Overpass sections in Overpass's tabular
/// figures (the `ui-tnum-*` family of their weight); Overpass Mono's and the middle dots' sections
/// stay as they are (Mono's figures are tabular).
pub fn tabular(mut job: LayoutJob) -> LayoutJob {
    for section in &mut job.sections {
        if let Some(family) = fonts::tabular_of(&section.format.font_id.family) {
            section.format.font_id.family = family;
        }
    }
    job
}

/// Appends `text` in `style` to `job`, each [`MIDDLE_DOT`] of a `--font-ui` text as its own
/// section in [`dot_family`], narrowed to [`DOT_ADVANCE_EM`] round its ink (a `--font-data`
/// text keeps Overpass Mono's own dot, as today).
pub fn append(job: &mut LayoutJob, text: &str, style: &Style) {
    let format = style.format();
    if style.face == Face::Mono {
        push(job, text, format);
        return;
    }
    let mut rest = text;
    while let Some(at) = rest.find(MIDDLE_DOT) {
        if at > 0 {
            push(job, &rest[..at], format.clone());
        }
        push(job, &rest[at..at + MIDDLE_DOT.len_utf8()], style.dot_format());
        rest = &rest[at + MIDDLE_DOT.len_utf8()..];
    }
    if !rest.is_empty() || job.sections.is_empty() {
        push(job, rest, format);
    }
}

/// How far a dot's ink moves left of where Overpass Mono puts it, and how much less the text
/// after it moves on, at `size` px: half the difference between the two advances.
fn dot_squeeze(size: f32) -> f32 {
    0.5 * (MONO_ADVANCE_EM - DOT_ADVANCE_EM) * size
}

/// Whether `section` of `job` is a middle dot set by [`append`].
fn is_dot(job: &LayoutJob, section: &egui::text::LayoutSection) -> bool {
    section.format.font_id.family == dot_family() && &job.text[section.byte_range.start.0..section.byte_range.end.0] == "·"
}

/// Appends one piece: a dot moved left by [`dot_squeeze`] after a glyph on its row, and the text
/// after a dot moved left by the same, so the dot takes [`DOT_ADVANCE_EM`] with its ink centred.
fn push(job: &mut LayoutJob, text: &str, format: TextFormat) {
    let mut leading = 0.0;
    if let Some(last) = job.sections.last()
        && is_dot(job, last)
    {
        leading -= dot_squeeze(last.format.font_id.size);
    }
    if text == "·" && format.font_id.family == dot_family() && !job.text.is_empty() && !job.text.ends_with('\n') {
        leading -= dot_squeeze(format.font_id.size);
    }
    job.append(text, leading, format);
}

/// `text` in `style` on one row (`white-space: nowrap`), however wide.
pub fn job(text: &str, style: &Style) -> LayoutJob {
    let mut job = LayoutJob { wrap: TextWrapping::no_max_width(), break_on_newline: true, ..Default::default() };
    append(&mut job, text, style);
    job
}

/// `text` in `style` wrapped at word boundaries within `width` (egui breaks a word wider than the
/// width where it must).
pub fn wrapped(text: &str, style: &Style, width: f32) -> LayoutJob {
    let mut job = job(text, style);
    job.wrap = TextWrapping::wrap_at_width(width);
    job
}

/// `text` in `style` on one row within `width`: cut at the last character that leaves room for
/// `…` when it does not fit (`white-space: nowrap; overflow: hidden; text-overflow: ellipsis`).
pub fn truncated(text: &str, style: &Style, width: f32) -> LayoutJob {
    let mut job = job(text, style);
    truncate(&mut job, width);
    job
}

/// Makes `job` one row within `width` with `…` at the end when it does not fit.
pub fn truncate(job: &mut LayoutJob, width: f32) {
    job.wrap = TextWrapping { max_width: width.max(0.0), max_rows: 1, break_anywhere: true, overflow_character: Some(ELLIPSIS) };
}

/// Lays `job` out and paints it at the top left of the space it takes in `ui` (its rows' line
/// boxes: the galley's size), returning that space's response for `sense`.
pub fn label(ui: &mut Ui, job: LayoutJob, sense: Sense) -> Response {
    let galley = ui.painter().layout_job(job);
    let (rect, response) = ui.allocate_exact_size(galley.size(), sense);
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    }
    response
}

/// The size `job` takes when laid out in `ctx` (its rows' line boxes).
pub fn measure(ctx: &egui::Context, job: LayoutJob) -> Vec2 {
    ctx.fonts_mut(|f| f.layout_job(job)).size()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_fonts(f: impl FnOnce(&egui::Context)) {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut f = Some(f);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                f(ui.ctx());
            }
        });
        output.textures_delta.clear();
    }

    fn sections(job: &LayoutJob) -> Vec<(&str, String)> {
        job.sections
            .iter()
            .map(|s| {
                let family = match &s.format.font_id.family {
                    FontFamily::Name(n) => n.to_string(),
                    other => format!("{other:?}"),
                };
                (&job.text[s.byte_range.start.0..s.byte_range.end.0], family)
            })
            .collect()
    }

    #[test]
    fn every_middle_dot_is_set_in_its_own_face() {
        let style = Style::ui(13.0, 650, theme::TEXT_2);
        let j = job("KLM · AMS · 12,000 ft", &style);
        assert_eq!(
            sections(&j),
            [
                ("KLM ", "ui-650".to_string()),
                ("·", DOT_FAMILY.to_string()),
                (" AMS ", "ui-650".to_string()),
                ("·", DOT_FAMILY.to_string()),
                (" 12,000 ft", "ui-650".to_string()),
            ]
        );
        assert_eq!(dot_family(), FontFamily::Name(DOT_FAMILY.into()));
        // the dot keeps the text's size, colour and line box
        let dot = &j.sections[1].format;
        assert_eq!((dot.font_id.size, dot.color, dot.line_height), (13.0, theme::TEXT_2, Some(13.0 * 1.4)));
        // dots at either end, side by side, and none
        assert_eq!(sections(&job("··x·", &style)).iter().map(|(t, _)| *t).collect::<Vec<_>>(), ["·", "·", "x", "·"]);
        // the dot is pulled left after a glyph, and what follows it pulled in, by the same
        let squeeze = 0.5 * (MONO_ADVANCE_EM - DOT_ADVANCE_EM) * 13.0;
        let leading: Vec<f32> = j.sections.iter().map(|s| s.leading_space).collect();
        assert_eq!(leading, [0.0, -squeeze, -squeeze, -squeeze, -squeeze]);
        let first = job("· x", &style);
        assert_eq!(first.sections.iter().map(|s| s.leading_space).collect::<Vec<_>>(), [0.0, -squeeze], "nothing to pull back at the start");
        // a later append after a dot is pulled in too
        let mut two = job("A ·", &style);
        append(&mut two, " B", &Style::data(13.0, 400, theme::TEXT_1));
        assert_eq!(two.sections.last().map(|s| s.leading_space), Some(-squeeze));
        assert_eq!(sections(&job("plain", &style)), [("plain", "ui-650".to_string())]);
        assert_eq!(job("", &style).sections.len(), 1, "an empty text still has its format (its line box)");
        // not in data text: `--font-data` has no "Middle Dot" face, so its dot is Overpass Mono's
        // own, in the text's weight and letter spacing (one section, nothing pulled in)
        let data = job("12 · 34", &Style::data(12.0, 600, theme::TEXT_1));
        assert_eq!(sections(&data), [("12 · 34", "mono-600".to_string())]);
        assert!((data.sections[0].format.extra_letter_spacing + 0.12).abs() < 1e-6);
        assert_eq!(data.sections[0].leading_space, 0.0);
    }

    #[test]
    fn the_dot_has_a_gap_on_both_sides() {
        with_fonts(|ctx| {
            let style = Style::ui(14.0, 400, theme::TEXT_1);
            let galley = ctx.fonts_mut(|f| f.layout_job(job("A · B", &style)));
            let glyphs = &galley.rows[0].row.glyphs;
            let x = |c: char| glyphs.iter().find(|g| g.chr == c).map(|g| (g.pos.x, g.advance_width)).expect("a glyph");
            let (a, a_w) = x('A');
            let (dot, dot_w) = x(MIDDLE_DOT);
            let (b, _) = x('B');
            let (space, space_w) = x(' ');
            // A, a space, the dot's 0.318 em, a space, B: as the reference pictures' DejaVu dot
            assert!((b - (space + 2.0 * space_w + DOT_ADVANCE_EM * 14.0)).abs() <= 1.0, "A at {a}+{a_w}, space {space}+{space_w}, dot at {dot}, B at {b}");
            // its ink in the middle of its cell: the cell runs from the space's end to B's space
            let cell = (space + space_w, b - space_w);
            let ink = dot + 0.5 * MONO_ADVANCE_EM * 14.0;
            assert!((ink - 0.5 * (cell.0 + cell.1)).abs() <= 1.0, "ink at {ink} in {cell:?}");
            assert!(dot_w > 3.0 && ink - a - a_w > 4.0 && b - ink > 4.0, "a gap on both sides: {a}+{a_w} {ink} {b}");
            // Overpass's own dot would not advance
            let mut bare = LayoutJob::default();
            bare.append("·", 0.0, style.format());
            let bare = ctx.fonts_mut(|f| f.layout_job(bare));
            assert!(bare.rows[0].row.glyphs[0].advance_width < 0.5, "Overpass's · has no advance: {}", bare.rows[0].row.glyphs[0].advance_width);
        });
    }

    #[test]
    fn truncation_ends_in_an_ellipsis_within_the_width() {
        with_fonts(|ctx| {
            let style = Style::ui(13.0, 400, theme::TEXT_1);
            let text = "Scandinavian Airlines System · Stockholm Arlanda";
            let full = measure(ctx, job(text, &style));
            let galley = ctx.fonts_mut(|f| f.layout_job(truncated(text, &style, 120.0)));
            assert_eq!(galley.rows.len(), 1);
            assert!(galley.elided, "cut");
            assert!(galley.size().x <= 120.0 + 0.01 && galley.size().x > 100.0, "{} of {}", galley.size().x, full.x);
            let shown: String = galley.rows[0].row.glyphs.iter().map(|g| g.chr).collect();
            assert!(shown.ends_with(ELLIPSIS) && text.starts_with(shown.trim_end_matches(ELLIPSIS)), "{shown:?}");
            // what fits is left alone
            let galley = ctx.fonts_mut(|f| f.layout_job(truncated("SAS", &style, 120.0)));
            assert!(!galley.elided);
            assert_eq!(galley.rows[0].row.glyphs.iter().map(|g| g.chr).collect::<String>(), "SAS");
            // a line box of 13 × 1.4
            assert!((galley.size().y - 18.2).abs() <= 0.5, "{}", galley.size().y);
        });
    }

    #[test]
    fn wrapped_text_breaks_at_words() {
        with_fonts(|ctx| {
            let style = Style::body();
            let galley = ctx.fonts_mut(|f| f.layout_job(wrapped("RustRadar needs its Rust backend. Start it with bun tauri dev.", &style, 160.0)));
            assert!(galley.rows.len() >= 2);
            assert!(galley.size().x <= 160.0 + 0.01);
            let first: String = galley.rows[0].row.glyphs.iter().map(|g| g.chr).collect();
            assert!(first.trim_end().ends_with(|c: char| c.is_alphabetic() || c == '.'), "{first:?}");
        });
    }
    /// The line heights of [`MEASURED`]: multiples of the size, then pixels.
    const HEIGHTS: [&str; 9] = ["1", "1.2", "1.25", "1.3", "1.35", "1.4", "1.5", "20px", "32px"];

    /// Where Chromium put the baseline, px under the line box's top, for Overpass 400 at each size
    /// and each of [`HEIGHTS`] (Playwright's Chromium 1243 on Linux, the fonts as today's app loads
    /// them: `getBoundingClientRect` of an empty inline-block on the baseline, which the ink of an
    /// `H` agreed with on every row; Overpass Mono, weight 700 and device scale 2 gave the same).
    const MEASURED: [(f32, [f32; 9]); 14] = [
        (7.0, [5.0, 5.0, 5.0, 6.0, 6.0, 6.0, 6.0, 11.0, 17.0]),
        (8.0, [6.0, 6.0, 7.0, 7.0, 7.0, 7.0, 8.0, 12.0, 18.0]),
        (10.0, [7.0, 8.0, 8.0, 9.0, 9.0, 9.0, 10.0, 12.0, 18.0]),
        (11.0, [8.0, 9.0, 9.0, 10.0, 10.0, 10.0, 11.0, 13.0, 19.0]),
        (12.0, [9.0, 10.0, 10.0, 10.0, 11.0, 11.0, 12.0, 13.0, 19.0]),
        (13.0, [9.0, 10.0, 11.0, 11.0, 11.0, 12.0, 12.0, 13.0, 19.0]),
        (14.0, [10.0, 11.0, 12.0, 12.0, 12.0, 13.0, 14.0, 13.0, 19.0]),
        (15.0, [11.0, 12.0, 12.0, 13.0, 13.0, 14.0, 14.0, 13.0, 19.0]),
        (16.0, [12.0, 13.0, 14.0, 14.0, 14.0, 15.0, 16.0, 14.0, 20.0]),
        (17.0, [12.0, 14.0, 14.0, 15.0, 15.0, 15.0, 16.0, 14.0, 20.0]),
        (18.0, [13.0, 15.0, 15.0, 16.0, 16.0, 17.0, 18.0, 14.0, 20.0]),
        (20.0, [15.0, 17.0, 17.0, 18.0, 18.0, 19.0, 20.0, 15.0, 21.0]),
        (22.0, [16.0, 18.0, 19.0, 19.0, 20.0, 20.0, 22.0, 15.0, 21.0]),
        (24.0, [18.0, 20.0, 21.0, 21.0, 22.0, 22.0, 24.0, 16.0, 22.0]),
    ];

    /// `line-height` `h` for text `size` px large, px.
    fn line_px(h: &str, size: f32) -> f32 {
        h.strip_suffix("px").map_or_else(|| h.parse::<f32>().unwrap() * size, |px| px.parse().unwrap())
    }

    #[test]
    fn baselines_are_chromiums() {
        for (size, row) in MEASURED {
            for (h, want) in HEIGHTS.iter().zip(row) {
                assert_eq!(css_baseline(size, line_px(h, size)), want, "{size} px at {h}");
            }
        }
        assert_eq!(Style::ui(13.0, 400, theme::TEXT_1).css_baseline(), 12.0);
        assert_eq!(Style::data(12.0, 600, theme::TEXT_1).line_height(1.25).css_baseline(), 10.0);
    }

    /// The text shapes of one pass at `pixels_per_point` that paints each `(text, style, top)` at
    /// x 4, after the hook: each row's drawn baseline (whole galley pixel plus row and glyph).
    fn drawn_baselines(pixels_per_point: f32, texts: &[(&str, Style, f32)]) -> Vec<Vec<f32>> {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx.set_pixels_per_point(pixels_per_point);
        let mut baselines = Vec::new();
        for pass in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                for (text, style, top) in texts {
                    let galley = ui.painter().layout_job(job(text, style));
                    ui.painter().galley(egui::Pos2::new(4.0, *top), galley, Color32::WHITE);
                }
            });
            output.textures_delta.clear();
            if pass == 1 {
                let ppp = output.pixels_per_point;
                for clipped in &output.shapes {
                    let Shape::Text(t) = &clipped.shape else { continue };
                    let top = (t.pos.y * ppp).round() / ppp;
                    baselines.push(t.galley.rows.iter().map(|r| top + r.pos.y + r.glyphs[0].pos.y).collect());
                }
            }
        }
        baselines
    }

    /// Six-line paragraphs on fractions of a pixel: each line's baseline where Chromium painted
    /// the ink of its `H` (the line boxes 18.1875 and 16.796875 px apart, each baseline on the
    /// nearest pixel), at device scales 1 and 2 (Chromium lays out in CSS pixels at both).
    #[test]
    fn the_hook_puts_every_row_on_chromiums_baseline() {
        let six = "H\nH\nH\nH\nH\nH";
        let s13 = Style::ui(13.0, 400, theme::TEXT_1);
        let s12 = Style::ui(12.0, 400, theme::TEXT_1);
        let texts = [(six, s13, 4.0), (six, s13, 4.25), (six, s13, 4.5), (six, s13, 4.9), (six, s12, 4.0), (six, s12, 4.6)];
        let chromium: [[f32; 6]; 6] = [
            [16.0, 34.0, 52.0, 71.0, 89.0, 107.0],
            [16.0, 34.0, 53.0, 71.0, 89.0, 107.0],
            [17.0, 35.0, 53.0, 71.0, 89.0, 107.0],
            [17.0, 35.0, 53.0, 71.0, 90.0, 108.0],
            [15.0, 32.0, 49.0, 65.0, 82.0, 99.0],
            [16.0, 32.0, 49.0, 66.0, 83.0, 100.0],
        ];
        for ppp in [1.0, 2.0] {
            let drawn = drawn_baselines(ppp, &texts);
            assert_eq!(drawn.len(), texts.len());
            for (i, (got, want)) in drawn.iter().zip(chromium).enumerate() {
                assert_eq!(got.as_slice(), want.as_slice(), "paragraph {i} at scale {ppp}");
            }
        }
        // one line of each measured size at 1.4 and 1.2, on a whole pixel
        let mut texts = Vec::new();
        for (size, _) in MEASURED {
            for h in [1.4, 1.2] {
                texts.push(("Hamburg 0123", Style::ui(size, 400, theme::TEXT_1).line_height(h), 10.0));
            }
        }
        for ppp in [1.0, 2.0] {
            let drawn = drawn_baselines(ppp, &texts);
            for ((_, style, top), got) in texts.iter().zip(&drawn) {
                assert_eq!(got[0], top + style.css_baseline(), "{} px at {} (scale {ppp})", style.size, style.line_height);
            }
        }
    }

    /// egui alone draws 12 px text at 1.4 a pixel lower than Chromium; the layout (the galley's
    /// rows) is egui's, only the drawing moves; text without a line height is left alone.
    #[test]
    fn the_hook_moves_the_drawing_only() {
        with_fonts(|ctx| {
            let style = Style::ui(12.0, 400, theme::TEXT_1);
            let galley = ctx.fonts_mut(|f| f.layout_job(job("Hamburg", &style)));
            let egui_baseline = galley.rows[0].pos.y + galley.rows[0].glyphs[0].pos.y;
            assert_eq!(egui_baseline, 12.0, "egui's own");
            assert_eq!(row_baseline(&galley, 0), 11.0, "Chromium's");
            let mut shape = TextShape::new(egui::Pos2::new(0.0, 20.0), galley.clone(), Color32::WHITE);
            to_css_baselines(&mut shape, 1.0, true);
            assert_eq!(shape.galley.rows[0].pos.y, galley.rows[0].pos.y - 1.0);
            assert_eq!(shape.galley.size(), galley.size(), "the layout is egui's");
            // no line height (egui's own widgets' text): not moved
            let mut plain = LayoutJob::default();
            plain.append("Hamburg", 0.0, TextFormat { line_height: None, ..style.format() });
            let plain = ctx.fonts_mut(|f| f.layout_job(plain));
            assert!(row_shifts(&plain, 20.0, 1.0, true).is_none());
            // a row egui made taller than its line box keeps its place
            let mut tall = job("Hamburg", &style);
            tall.first_row_min_height = 30.0;
            let tall = ctx.fonts_mut(|f| f.layout_job(tall));
            assert!(row_shifts(&tall, 20.0, 1.0, true).is_none());
            // a paragraph's rows: Chromium's line boxes, 16.796875 px apart
            let three = ctx.fonts_mut(|f| f.layout_job(job("A\nB\nC", &style)));
            assert_eq!([0, 1, 2].map(|r| row_baseline(&three, r)), [11.0, 11.0 + 16.796_875, 11.0 + 2.0 * 16.796_875]);
        });
    }

    #[test]
    fn tabular_figures_are_all_as_wide() {
        with_fonts(|ctx| {
            let style = Style::ui(12.0, 400, theme::TEXT_2);
            let advances = |job: LayoutJob| -> Vec<f32> {
                let galley = ctx.fonts_mut(|f| f.layout_job(job));
                galley.rows[0].glyphs.iter().filter(|g| g.chr.is_ascii_digit()).map(|g| g.advance_width).collect()
            };
            let proportional = advances(job("0123456789", &style));
            let tabular_ = advances(tabular(job("0123456789", &style)));
            // Overpass's own figures differ (the 1 is 746 units, the 0 1284) …
            assert!((proportional[1] - 746.0 / 2000.0 * 12.0).abs() < 0.01, "{proportional:?}");
            assert!((proportional[0] - 1284.0 / 2000.0 * 12.0).abs() < 0.01, "{proportional:?}");
            // … its tabular ones are 1232 units each, as Overpass Mono's
            for a in &tabular_ {
                assert!((a - 1232.0 / 2000.0 * 12.0).abs() < 0.01, "{tabular_:?}");
            }
            // letters stay Overpass's
            assert_eq!(advances(tabular(job("1,234 of 5,678", &style))).len(), 8);
            // as wide as Chromium sets the flight panel's distances with and without `tabular-nums`
            // (12 px, the dots in DejaVu Sans: 222.55 and 213.47 px)
            let distances = "312 km flown · 211 km to go · 52 min left";
            let tab = measure(ctx, tabular(job(distances, &style))).x;
            let own = measure(ctx, job(distances, &style)).x;
            assert!((tab - 222.55).abs() < 0.1 && (own - 213.47).abs() < 0.1, "{tab} {own}");
            let words = |j: LayoutJob| ctx.fonts_mut(|f| f.layout_job(j)).rows[0].glyphs.iter().filter(|g| g.chr == 'o').map(|g| g.advance_width).sum::<f32>();
            assert_eq!(words(job("of", &style)), words(tabular(job("of", &style))));
        });
        // the families: Overpass's sections move, Mono's and the dots' stay
        let mut j = job("12 · 34", &Style::ui(13.0, 650, theme::TEXT_1));
        append(&mut j, " 56", &Style::data(13.0, 400, theme::TEXT_1));
        let j = tabular(j);
        let names: Vec<String> = sections(&j).into_iter().map(|(_, f)| f).collect();
        assert_eq!(names, ["ui-tnum-650", DOT_FAMILY, "ui-tnum-650", "mono-400"]);
    }
}
