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
//!
//! [`job`], [`append`], [`wrapped`] and [`truncated`] build jobs; [`label`] lays one out and
//! paints it in the space it takes.

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
}
