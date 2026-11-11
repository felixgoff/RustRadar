//! `.skeleton` (app.css:230-240; doc 06 §1.4, §27): the placeholder shown while something loads.
//! Radius 4; a horizontal gradient `--surface-raised` → `--surface-hover` (at 50 %) →
//! `--surface-raised`, `background-size: 200% 100%` (two widths of the element, repeating), its
//! `background-position` running from 0 to −200 % over 1.4 s, linearly, forever.
//!
//! At [`anim::skeleton_mix`]'s phase 0 the element shows the first half of the gradient: raised at
//! its left edge rising to hover at its right edge, as in the reference pictures (whose capture
//! stops the shimmer there: `dark-v1`'s sidebar skeletons run from 62 to 74 across). The position
//! moves the gradient right by two widths a cycle (U1's note: doc 06 §1.4 says right to left; the
//! CSS moves it rightwards). Colours mix in sRGB as CSS gradients do.
//!
//! Drawn as egui's rounded rectangle filled with a one-row texture of the gradient's period
//! (repeating, linearly filtered, a texel on each of the gradient's corners), its texture
//! coordinates shifted with the phase: the rounded corners and their antialiasing are egui's own.
//! While a skeleton is drawn egui repaints at once ([`anim::cycle`]); under reduced motion it
//! stands still at phase 0. Every skeleton runs on the one clock (today each starts its own cycle
//! when it appears).

use egui::epaint::{RectShape, TextureHandle};
use egui::{Color32, ColorImage, Context, CornerRadius, Id, Painter, Pos2, Rect, Response, Sense, TextureOptions, TextureWrapMode, Ui, Vec2};

use super::super::{anim, theme};

/// Texels across one period of the gradient (two element widths).
pub const TEXELS: usize = 64;

/// The skeleton's colour at `phase` (0–1 of the 1.4 s cycle) and `u` (0 at its left edge, 1 at
/// its right edge), in exact sRGB before 8-bit rounding.
pub fn color_at(phase: f32, u: f32) -> [f32; 3] {
    let mix = anim::skeleton_mix(phase, u);
    let lerp = |a: u8, b: u8| f32::from(a) + (f32::from(b) - f32::from(a)) * mix;
    [
        lerp(theme::SURFACE_RAISED.r(), theme::SURFACE_HOVER.r()),
        lerp(theme::SURFACE_RAISED.g(), theme::SURFACE_HOVER.g()),
        lerp(theme::SURFACE_RAISED.b(), theme::SURFACE_HOVER.b()),
    ]
}

/// The gradient's period as texels: texel `i` is the colour `i / TEXELS` of the way along it
/// (raised at 0, hover at the middle).
pub fn period_image() -> ColorImage {
    let pixels = (0..TEXELS)
        .map(|i| {
            // s along the two-width period, 0..2; the element's own u for phase 0
            let s = 2.0 * i as f32 / TEXELS as f32;
            let [r, g, b] = color_at(0.0, s).map(|v| v.round() as u8);
            Color32::from_rgb(r, g, b)
        })
        .collect();
    ColorImage::new([TEXELS, 1], pixels)
}

fn texture(ctx: &Context) -> TextureHandle {
    let id = Id::unique("rustradar::widgets::skeleton");
    if let Some(handle) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return handle;
    }
    let options = TextureOptions { wrap_mode: TextureWrapMode::Repeat, ..TextureOptions::LINEAR };
    let handle = ctx.load_texture("skeleton shimmer", period_image(), options);
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
}

/// The texture coordinates across an element at `phase`: x from its left edge to its right edge
/// (half a period, shifted back by the phase), on a texel centre at each of the gradient's corners.
pub fn uv(phase: f32) -> Rect {
    let half_texel = 0.5 / TEXELS as f32;
    Rect::from_min_max(Pos2::new(-phase + half_texel, 0.0), Pos2::new(0.5 - phase + half_texel, 1.0))
}

/// Where the shimmer stands now (0–1 of [`anim::SKELETON_PERIOD`]); 0 under reduced motion.
/// Keeps egui repainting.
pub fn phase(ctx: &Context) -> f32 {
    anim::cycle(ctx, anim::SKELETON_PERIOD).unwrap_or(0.0)
}

/// Draws a skeleton filling `rect` with corner `radius` (4 everywhere but where a component
/// rounds its own placeholder).
pub fn paint(painter: &Painter, rect: Rect, radius: u8) {
    let ctx = painter.ctx();
    let phase = phase(ctx);
    let shape = RectShape::filled(rect, CornerRadius::same(radius), Color32::WHITE).with_texture(texture(ctx).id(), uv(phase));
    painter.add(shape);
}

/// A `.skeleton` of a fixed size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Skeleton {
    pub size: Vec2,
    pub radius: u8,
}

impl Skeleton {
    /// A skeleton `width` × `height` px with radius 4.
    pub fn new(width: f32, height: f32) -> Skeleton {
        Skeleton { size: Vec2::new(width, height), radius: theme::RADIUS }
    }

    /// Its corner radius.
    pub fn radius(self, radius: u8) -> Skeleton {
        Skeleton { radius, ..self }
    }

    /// Takes its space from `ui`'s layout and draws it.
    pub fn show(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::hover());
        if ui.is_rect_visible(rect) {
            paint(ui.painter(), rect, self.radius);
        }
        response
    }
}

impl egui::Widget for Skeleton {
    fn ui(self, ui: &mut Ui) -> Response {
        self.show(ui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texture as the GPU samples it: linear between texel centres, repeating.
    fn sample(image: &ColorImage, u: f32) -> f32 {
        let x = u * TEXELS as f32 - 0.5;
        let i = x.floor();
        let f = x - i;
        let at = |k: f32| f32::from(image.pixels[(k as i64).rem_euclid(TEXELS as i64) as usize].r());
        at(i) * (1.0 - f) + at(i + 1.0) * f
    }

    #[test]
    fn the_texture_draws_the_css_gradient_at_every_phase() {
        let image = period_image();
        assert_eq!(image.pixels[0], theme::SURFACE_RAISED);
        assert_eq!(image.pixels[TEXELS / 2], theme::SURFACE_HOVER);
        for phase in [0.0, 0.1, 0.25, 0.5, 0.73, 0.99] {
            let uv = uv(phase);
            for k in 0..=20 {
                let u = k as f32 / 20.0;
                let tex_u = uv.min.x + (uv.max.x - uv.min.x) * u;
                let want = color_at(phase, u)[0];
                let got = sample(&image, tex_u);
                assert!((got - want).abs() <= 0.5 + 1e-3, "phase {phase} at {u}: {got} for {want}");
            }
        }
    }

    #[test]
    fn at_rest_it_rises_from_raised_on_the_left_to_hover_on_the_right() {
        assert_eq!(color_at(0.0, 0.0), [62.0; 3]);
        assert_eq!(color_at(0.0, 1.0), [74.0; 3]);
        assert_eq!(color_at(0.0, 0.5), [68.0; 3]);
        // half a cycle on, the highlight has left on the right and comes back in from the left
        assert_eq!(color_at(0.5, 0.0), [74.0; 3]);
        assert_eq!(color_at(0.5, 1.0), [62.0; 3]);
        // moving right: a quarter on the dark end is in the middle, three quarters on the peak
        assert_eq!(color_at(0.25, 0.5), [62.0; 3]);
        assert_eq!(color_at(0.75, 0.5), [74.0; 3]);
    }

    #[test]
    fn one_texture_per_context() {
        let ctx = Context::default();
        let mut ids = Vec::new();
        for time in [0.0, 0.7] {
            let mut out = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
                Skeleton::new(90.0, 12.0).show(ui);
                ids.push(texture(ui.ctx()).id());
            });
            out.textures_delta.clear();
        }
        assert_eq!(ids[0], ids[1]);
    }
}
