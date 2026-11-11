//! The UI's icons (plan 1.5; doc 06 §26): the Lucide outline icons the web build imports from
//! `@lucide/svelte`, drawn from the SVG files vendored in `assets/icons/lucide/`
//! ([`crate::assets::ICONS`]).
//!
//! Today each icon is an inline `<svg>` that Lucide's `Icon.svelte` writes: a 24×24 viewBox shown
//! `size` CSS px square, `fill="none"`, `stroke="currentColor"`, round caps and joins, and the
//! use's `strokeWidth` (1.75, 2 or 2.5) in viewBox units (`absoluteStrokeWidth` is never set, so
//! a 14 px icon's 1.75 stroke is 1.02 px wide). Here:
//!
//! - **Rasterising**: [`mask`] substitutes the file's one `stroke-width="2"` with the use's width
//!   and draws the SVG with resvg (tiny-skia, a Skia port, as Chrome's rasteriser) at
//!   `size × pixels_per_point` pixels, scaled from the viewBox as the browser does, into a coverage
//!   mask (`currentColor` comes out as alpha only). It draws [`SUPERSAMPLE`] times finer and
//!   averages, so each pixel is covered by the shape's area (Skia's analytic antialiasing) and not
//!   on tiny-skia's quarter-pixel edges.
//! - **Cache**: each mask becomes a white egui texture (premultiplied `(a, a, a, a)`), kept per
//!   egui context by (icon, size, stroke, scale) and the whole degrees of a baked rotation
//!   ([`Key`]). A context's textures go with it (B9's rebuild makes a new one). Entries not drawn
//!   for [`KEEP_PASSES`] passes are dropped once the cache holds more than [`EVICT_ABOVE`].
//! - **Tinting**: [`paint`] draws the texture with the colour as its vertex colour, which egui
//!   multiplies in gamma space, as the browser composites `currentColor` × coverage over what lies
//!   below. Opacity (a disabled button's 0.45) comes from the painter or the colour's alpha.
//! - **Placement**: the icon's `size` square is centred in the rect it is given (the flex
//!   centring of today's buttons) and its corner put on the device pixel grid, as Blink snaps an
//!   SVG root's paint offset, so the texture's texels land on pixels unfiltered.
//! - **Rotation** ([`Spec::rotated`]; CSS `rotate()`, clockwise, about the centre): the nearest
//!   whole degree is baked into the raster (the 45° plane of the progress marker, the compass
//!   needle at its bearing are drawn as crisply as Chrome draws a transformed SVG), and the rest
//!   of a degree turns the textured quad. A continuous spin (the 900 ms spinner) fills at most 360
//!   textures per spec and then only reuses them.
//!
//! Against today's pictures (`docs/port/reference`, `tests/icon_sheet.rs`): the shell's icons land
//! on the same pixels. Their ink is within 8 % of today's, mostly 2–5 % more: those pictures were
//! drawn by Chromium's GPU raster on SwiftShader, which covers stroke edges in quarters (4× MSAA),
//! and covers about a third of each of Lucide's dots (zero-length `h.01` segments with round
//! caps, a disc one stroke wide here, as Skia's CPU raster and WebKit draw it).
//!
//! Deviation: SVG content past the viewBox is not clipped when the raster is rotated (no Lucide
//! icon in use draws there, so nothing changes).

use std::collections::HashMap;
use std::sync::LazyLock;

use egui::epaint::{Mesh, TextureHandle};
use egui::emath::Rot2;
use egui::{Color32, ColorImage, Context, Id, Painter, Pos2, Rect, Response, Sense, TextureId, TextureOptions, Ui, Vec2};

/// The Lucide icons in use (doc 06 §26), named as the web build imports them. `PlaneLanding`
/// serves both the place kind and the board's arrivals tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Icon {
    Radar,
    Funnel,
    RefreshCw,
    TriangleAlert,
    Plane,
    TrainFront,
    Ship,
    Eye,
    Search,
    LoaderCircle,
    X,
    MapPin,
    House,
    Signpost,
    Landmark,
    PlaneLanding,
    Building,
    Mailbox,
    Map,
    Flag,
    ArrowLeft,
    ChevronLeft,
    ChevronRight,
    ExternalLink,
    LocateFixed,
    LockKeyhole,
    PlaneTakeoff,
    ArrowRight,
    Check,
    Layers,
    Settings,
    KeyRound,
    Keyboard,
    Plus,
    Minus,
    Rotate3d,
    Navigation2,
}

/// Stroke width 1.75: most of today's icons (doc 06 §26).
pub const STROKE_THIN: f32 = 1.75;
/// Stroke width 2: Lucide's default (spinners, the back row, photo navigation, list arrows).
pub const STROKE_REGULAR: f32 = 2.0;
/// Stroke width 2.5: the chips' `Check`.
pub const STROKE_BOLD: f32 = 2.5;

/// The viewBox side of every Lucide icon.
pub const VIEW_BOX: f32 = 24.0;
/// Each pixel is drawn as `SUPERSAMPLE²` and averaged: tiny-skia's own antialiasing puts edges
/// on a quarter pixel (a 14 px `Check` came out as heavy at stroke 1.75 as at 2), where Chrome's
/// analytic antialiasing covers the exact area; four times finer is within 1/16 px.
pub const SUPERSAMPLE: usize = 4;
/// A cache holding more entries than this drops the ones not drawn lately.
pub const EVICT_ABOVE: usize = 128;
/// How many passes an entry outlives its last draw once the cache is past [`EVICT_ABOVE`].
pub const KEEP_PASSES: u64 = 600;

impl Icon {
    /// Every icon, in doc 06 §26's order.
    pub const ALL: [Icon; 37] = [
        Icon::Radar,
        Icon::Funnel,
        Icon::RefreshCw,
        Icon::TriangleAlert,
        Icon::Plane,
        Icon::TrainFront,
        Icon::Ship,
        Icon::Eye,
        Icon::Search,
        Icon::LoaderCircle,
        Icon::X,
        Icon::MapPin,
        Icon::House,
        Icon::Signpost,
        Icon::Landmark,
        Icon::PlaneLanding,
        Icon::Building,
        Icon::Mailbox,
        Icon::Map,
        Icon::Flag,
        Icon::ArrowLeft,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::ExternalLink,
        Icon::LocateFixed,
        Icon::LockKeyhole,
        Icon::PlaneTakeoff,
        Icon::ArrowRight,
        Icon::Check,
        Icon::Layers,
        Icon::Settings,
        Icon::KeyRound,
        Icon::Keyboard,
        Icon::Plus,
        Icon::Minus,
        Icon::Rotate3d,
        Icon::Navigation2,
    ];

    /// Lucide's kebab-case name: the vendored file's stem (`plane-takeoff`).
    pub const fn name(self) -> &'static str {
        match self {
            Icon::Radar => "radar",
            Icon::Funnel => "funnel",
            Icon::RefreshCw => "refresh-cw",
            Icon::TriangleAlert => "triangle-alert",
            Icon::Plane => "plane",
            Icon::TrainFront => "train-front",
            Icon::Ship => "ship",
            Icon::Eye => "eye",
            Icon::Search => "search",
            Icon::LoaderCircle => "loader-circle",
            Icon::X => "x",
            Icon::MapPin => "map-pin",
            Icon::House => "house",
            Icon::Signpost => "signpost",
            Icon::Landmark => "landmark",
            Icon::PlaneLanding => "plane-landing",
            Icon::Building => "building",
            Icon::Mailbox => "mailbox",
            Icon::Map => "map",
            Icon::Flag => "flag",
            Icon::ArrowLeft => "arrow-left",
            Icon::ChevronLeft => "chevron-left",
            Icon::ChevronRight => "chevron-right",
            Icon::ExternalLink => "external-link",
            Icon::LocateFixed => "locate-fixed",
            Icon::LockKeyhole => "lock-keyhole",
            Icon::PlaneTakeoff => "plane-takeoff",
            Icon::ArrowRight => "arrow-right",
            Icon::Check => "check",
            Icon::Layers => "layers",
            Icon::Settings => "settings",
            Icon::KeyRound => "key-round",
            Icon::Keyboard => "keyboard",
            Icon::Plus => "plus",
            Icon::Minus => "minus",
            Icon::Rotate3d => "rotate-3d",
            Icon::Navigation2 => "navigation-2",
        }
    }

    /// The vendored SVG file (Lucide's format: one root `stroke-width="2"`).
    pub fn svg(self) -> &'static str {
        let bytes = crate::assets::icon(self.name()).unwrap_or_else(|| panic!("{} is not vendored", self.name()));
        std::str::from_utf8(bytes).expect("a vendored SVG is UTF-8")
    }

    /// This icon drawn `size` CSS px (egui points) square with stroke width `stroke`.
    pub const fn at(self, size: f32, stroke: f32) -> Spec {
        Spec { icon: self, size, stroke, angle: 0.0 }
    }
}

/// One use of an icon: what today's `<Icon size={…} strokeWidth={…} />` (and a CSS rotation)
/// says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub icon: Icon,
    /// Side of the icon's square, points (CSS px).
    pub size: f32,
    /// Stroke width in viewBox units.
    pub stroke: f32,
    /// Clockwise rotation about the square's centre, radians (CSS `rotate()`).
    pub angle: f32,
}

impl Spec {
    /// This use turned clockwise by `angle` radians about its centre.
    pub const fn rotated(self, angle: f32) -> Spec {
        Spec { angle, ..self }
    }

    /// The icon's square, points.
    pub fn size_vec(self) -> Vec2 {
        Vec2::splat(self.size)
    }
}

/// An icon's coverage, ready to become a texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    /// Side of the square raster, pixels.
    pub side: usize,
    /// Where the unrotated icon square's corner sits in the raster, pixels from its top left
    /// (0 unless the raster is rotated, which needs room for the corners).
    pub offset: usize,
    /// Coverage, row by row, `side × side`.
    pub alpha: Vec<u8>,
}

impl Mask {
    /// Total coverage in pixels (each fully covered pixel counts 1).
    pub fn ink(&self) -> f64 {
        self.alpha.iter().map(|&a| f64::from(a)).sum::<f64>() / 255.0
    }

    /// The coverage at `(x, y)`.
    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.alpha[y * self.side + x]
    }

    /// As a white egui image (premultiplied `(a, a, a, a)`), to tint at draw time.
    pub fn image(&self) -> ColorImage {
        ColorImage::new([self.side, self.side], self.alpha.iter().map(|&a| Color32::from_white_alpha(a)).collect())
    }
}

/// The whole pixels a raster of `px` pixels needs (`px` itself when whole: 14 × 1.0 is 14, not
/// 15 for a rounding error).
fn whole(px: f32) -> usize {
    (px - 1e-3).ceil().max(1.0) as usize
}

/// The raster side for an icon of `px` pixels turned `turn` whole degrees, and the corner's
/// offset in it: the rotated square fits, with as many pixels added on each side (so the
/// unrotated square stays on the pixel grid).
pub fn raster_side(px: f32, turn: u16) -> (usize, usize) {
    let base = whole(px);
    if turn.is_multiple_of(90) {
        return (base, 0);
    }
    let t = f32::from(turn).to_radians();
    let extent = px * (t.cos().abs() + t.sin().abs());
    let offset = ((extent - base as f32) / 2.0).ceil().max(0.0) as usize;
    (base + 2 * offset, offset)
}

/// Rasterise `icon` at `px` pixels square with stroke width `stroke` (viewBox units), turned
/// clockwise `turn` whole degrees about its centre.
pub fn mask(icon: Icon, px: f32, stroke: f32, turn: u16) -> Mask {
    let svg = icon.svg();
    const DEFAULT: &str = "stroke-width=\"2\"";
    debug_assert_eq!(svg.matches(DEFAULT).count(), 1, "{}: one root stroke width", icon.name());
    let svg = svg.replacen(DEFAULT, &format!("stroke-width=\"{stroke}\""), 1);
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
        .unwrap_or_else(|e| panic!("{}.svg: {e}", icon.name()));
    let turn = turn % 360;
    let (side, offset) = raster_side(px, turn);
    let k = SUPERSAMPLE;
    let fine = side * k;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(fine as u32, fine as u32).expect("a raster of at least 1 px");
    let scale = px / VIEW_BOX;
    let o = offset as f32;
    let centre = o + px / 2.0;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale)
        .post_translate(o, o)
        .post_rotate_at(f32::from(turn), centre, centre)
        .post_scale(k as f32, k as f32);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let fine_alpha: Vec<u8> = pixmap.pixels().iter().map(|p| p.alpha()).collect();
    let mut alpha = vec![0u8; side * side];
    for (i, a) in alpha.iter_mut().enumerate() {
        let (x, y) = (i % side * k, i / side * k);
        let sum: usize = (y..y + k).map(|fy| fine_alpha[fy * fine + x..fy * fine + x + k].iter().map(|&v| usize::from(v)).sum::<usize>()).sum();
        *a = ((sum + k * k / 2) / (k * k)) as u8;
    }
    Mask { side, offset, alpha }
}

/// What the cache keys a texture by: the icon, its size, stroke and the pixels per point (as
/// bits: they come from the same constants each frame), and the baked whole degrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub icon: Icon,
    pub size: u32,
    pub stroke: u32,
    pub scale: u32,
    pub turn: u16,
}

impl Key {
    pub fn new(spec: Spec, pixels_per_point: f32, turn: u16) -> Key {
        Key { icon: spec.icon, size: spec.size.to_bits(), stroke: spec.stroke.to_bits(), scale: pixels_per_point.to_bits(), turn: turn % 360 }
    }
}

/// A cached icon texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Texture {
    pub id: TextureId,
    /// Side, pixels.
    pub side: usize,
    /// Where the unrotated square's corner sits, pixels from the texture's top left.
    pub offset: usize,
}

#[derive(Clone)]
struct Entry {
    texture: TextureHandle,
    side: usize,
    offset: usize,
    used: u64,
}

/// The textures of one egui context, in its memory.
#[derive(Clone, Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
}

fn cache_id() -> Id {
    static ID: LazyLock<Id> = LazyLock::new(|| Id::unique("rustradar::ui::icons"));
    *ID
}

/// How many icon textures `ctx` holds.
pub fn cached(ctx: &Context) -> usize {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(cache_id()).entries.len())
}

/// The texture of `spec` at `pixels_per_point`, turned `turn` whole degrees: from the cache, or
/// rasterised and uploaded now.
pub fn texture(ctx: &Context, spec: Spec, pixels_per_point: f32, turn: u16) -> Texture {
    let key = Key::new(spec, pixels_per_point, turn);
    let pass = ctx.cumulative_pass_nr();
    let hit = ctx.data_mut(|d| {
        let entry = d.get_temp_mut_or_default::<Cache>(cache_id()).entries.get_mut(&key)?;
        entry.used = pass;
        Some(Texture { id: entry.texture.id(), side: entry.side, offset: entry.offset })
    });
    if let Some(hit) = hit {
        return hit;
    }
    let mask = mask(spec.icon, spec.size * pixels_per_point, spec.stroke, key.turn);
    let name = format!("icon {} {}/{} ×{} {}°", spec.icon.name(), spec.size, spec.stroke, pixels_per_point, key.turn);
    let handle = ctx.load_texture(name, mask.image(), TextureOptions::LINEAR);
    let texture = Texture { id: handle.id(), side: mask.side, offset: mask.offset };
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<Cache>(cache_id());
        if cache.entries.len() >= EVICT_ABOVE {
            cache.entries.retain(|_, e| e.used + KEEP_PASSES >= pass);
        }
        cache.entries.insert(key, Entry { texture: handle, side: mask.side, offset: mask.offset, used: pass });
    });
    texture
}

/// Where [`paint`] puts `spec` in `rect`: the icon's square centred in `rect`, its corner on the
/// pixel grid.
pub fn square(rect: Rect, spec: Spec, pixels_per_point: f32) -> Rect {
    let corner = rect.center() - spec.size_vec() / 2.0;
    let snapped = Pos2::new((corner.x * pixels_per_point).round(), (corner.y * pixels_per_point).round()) / pixels_per_point;
    Rect::from_min_size(snapped, spec.size_vec())
}

/// Draw `spec` centred in `rect` in `color` (`currentColor`).
pub fn paint(painter: &Painter, rect: Rect, spec: Spec, color: Color32) {
    let ppp = painter.pixels_per_point();
    let degrees = spec.angle.to_degrees();
    let whole_degrees = degrees.round();
    let turn = whole_degrees.rem_euclid(360.0) as u16;
    let texture = texture(painter.ctx(), spec, ppp, turn);
    let square = square(rect, spec, ppp);
    let min = square.min - Vec2::splat(texture.offset as f32 / ppp);
    let quad = Rect::from_min_size(min, Vec2::splat(texture.side as f32 / ppp));
    let mut mesh = Mesh::with_texture(texture.id);
    mesh.add_rect_with_uv(quad, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), color);
    let rest = (degrees - whole_degrees).to_radians();
    if rest != 0.0 {
        mesh.rotate(Rot2::from_angle(rest), square.center());
    }
    painter.add(mesh);
}

/// Take `spec`'s square from `ui`'s layout and draw the icon in it.
pub fn show(ui: &mut Ui, spec: Spec, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(spec.size_vec(), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, spec, color);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes doc 06 §26 lists, and the acceptance's 14, 16, 18, 20.
    const SIZES: [f32; 7] = [11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 18.0];

    #[test]
    fn every_icon_is_vendored_and_named_as_lucide_names_it() {
        assert_eq!(Icon::ALL.len(), crate::assets::ICONS.len());
        let mut names: Vec<&str> = Icon::ALL.iter().map(|i| i.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Icon::ALL.len(), "two icons share a name");
        for icon in Icon::ALL {
            assert!(icon.svg().contains("viewBox=\"0 0 24 24\""), "{icon:?}");
            assert_eq!(icon.svg().matches("stroke-width=\"2\"").count(), 1, "{icon:?}");
        }
    }

    #[test]
    fn every_icon_has_ink_at_every_size_and_scale() {
        for icon in Icon::ALL {
            for size in SIZES.into_iter().chain([20.0]) {
                for scale in [1.0, 1.25, 2.0] {
                    let px = size * scale;
                    let m = mask(icon, px, STROKE_THIN, 0);
                    assert_eq!(m.side, whole(px), "{icon:?} {size} ×{scale}");
                    assert_eq!(m.alpha.len(), m.side * m.side);
                    // The thinnest icon (`minus`, one 14-unit line) covers 14 × 1.75 units².
                    let least = (14.0 * 1.75) * (px / 24.0) * (px / 24.0) * 0.8;
                    assert!(m.ink() > f64::from(least), "{icon:?} {size} ×{scale}: ink {}", m.ink());
                    // a 0.875 px line between two rows (`minus` at 12 px) covers each 44 %
                    assert!(m.alpha.iter().any(|&a| a >= 100), "{icon:?} {size} ×{scale}: no pixel 40 % covered");
                }
            }
        }
    }

    #[test]
    fn a_wider_stroke_covers_more() {
        for icon in Icon::ALL {
            for size in [14.0, 16.0, 18.0, 20.0] {
                let thin = mask(icon, size, STROKE_THIN, 0).ink();
                let regular = mask(icon, size, STROKE_REGULAR, 0).ink();
                let bold = mask(icon, size, STROKE_BOLD, 0).ink();
                assert!(thin < regular && regular < bold, "{icon:?} {size}: {thin} {regular} {bold}");
            }
        }
    }

    /// `minus` is `M5 12h14`: at 24 px and stroke 2, a bar from x 5 to 19 between y 11 and 13
    /// with round caps of radius 1, so 14 × 2 + π px² of ink.
    #[test]
    fn minus_at_24_px_is_the_bar_its_path_says() {
        let m = mask(Icon::Minus, 24.0, 2.0, 0);
        assert_eq!((m.side, m.offset), (24, 0));
        assert!((m.ink() - (28.0 + std::f64::consts::PI)).abs() < 0.1, "ink {}", m.ink());
        for x in 5..19 {
            assert_eq!((m.at(x, 10), m.at(x, 11), m.at(x, 12), m.at(x, 13)), (0, 255, 255, 0), "column {x}");
        }
        assert_eq!(m.at(3, 12), 0);
        assert_eq!(m.at(20, 12), 0);
        // half a stroke wider: rows 10 and 13 a quarter covered
        let bold = mask(Icon::Minus, 24.0, 2.5, 0);
        assert!((i32::from(bold.at(12, 10)) - 64).abs() <= 2, "{}", bold.at(12, 10));
        assert!((i32::from(bold.at(12, 13)) - 64).abs() <= 2, "{}", bold.at(12, 13));
    }

    /// The stroke width is in viewBox units, as Lucide's `Icon.svelte` writes it: a 12 px icon
    /// at stroke 2 draws a 1 px line.
    #[test]
    fn the_stroke_scales_with_the_icon() {
        let m = mask(Icon::Minus, 12.0, 2.0, 0);
        // the bar's centre line is y 6: rows 5.5 to 6.5, half of rows 5 and 6 each
        assert!((i32::from(m.at(6, 5)) - 128).abs() <= 2 && (i32::from(m.at(6, 6)) - 128).abs() <= 2, "{} {}", m.at(6, 5), m.at(6, 6));
        assert!((m.ink() - (7.0 + std::f64::consts::PI / 4.0)).abs() < 0.2, "ink {}", m.ink());
    }

    #[test]
    fn a_rotated_raster_has_room_for_the_corners_and_keeps_its_ink() {
        assert_eq!(raster_side(14.0, 0), (14, 0));
        assert_eq!(raster_side(14.0, 90), (14, 0));
        assert_eq!(raster_side(17.5, 180), (18, 0));
        // 14 √2 = 19.8: three pixels each side
        assert_eq!(raster_side(14.0, 45), (20, 3));
        let (side, offset) = raster_side(18.0, 30);
        assert!(side as f32 >= 18.0 * (30f32.to_radians().cos() + 0.5) && (side - 18) % 2 == 0 && offset == (side - 18) / 2);

        let flat = mask(Icon::Minus, 24.0, 2.0, 0);
        let upright = mask(Icon::Minus, 24.0, 2.0, 90);
        assert!((flat.ink() - upright.ink()).abs() < 0.05);
        for y in 5..19 {
            assert_eq!((upright.at(10, y), upright.at(11, y), upright.at(12, y), upright.at(13, y)), (0, 255, 255, 0), "row {y}");
        }
        let plane = mask(Icon::Plane, 14.0, 2.0, 0);
        let turned = mask(Icon::Plane, 14.0, 2.0, 45);
        assert_eq!((turned.side, turned.offset), (20, 3));
        assert!((plane.ink() - turned.ink()).abs() / plane.ink() < 0.03, "{} {}", plane.ink(), turned.ink());
        // 405° is 45°
        assert_eq!(mask(Icon::Plane, 14.0, 2.0, 405), turned);
    }

    /// One pass of a bare context, with `ppp` pixels per point.
    fn pass(ctx: &Context, ppp: f32, ui: impl FnMut(&mut Ui)) -> egui::FullOutput {
        let mut raw = egui::RawInput::default();
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        let mut output = ctx.run_ui(raw, ui);
        output.textures_delta.clear();
        output
    }

    #[test]
    fn textures_are_cached_by_icon_size_stroke_scale_and_turn() {
        let ctx = Context::default();
        let mut ids = Vec::new();
        let _ = pass(&ctx, 1.0, |ui| {
            let c = ui.ctx();
            let x = texture(c, Icon::X.at(16.0, STROKE_THIN), 1.0, 0);
            assert_eq!(texture(c, Icon::X.at(16.0, STROKE_THIN), 1.0, 0), x, "the same use again");
            assert_eq!(texture(c, Icon::X.at(16.0, STROKE_THIN), 1.0, 360), x, "a full turn");
            ids.extend([
                x.id,
                texture(c, Icon::X.at(18.0, STROKE_THIN), 1.0, 0).id,
                texture(c, Icon::X.at(16.0, STROKE_REGULAR), 1.0, 0).id,
                texture(c, Icon::X.at(16.0, STROKE_THIN), 2.0, 0).id,
                texture(c, Icon::X.at(16.0, STROKE_THIN), 1.0, 45).id,
                texture(c, Icon::Plus.at(16.0, STROKE_THIN), 1.0, 0).id,
            ]);
            let doubled = texture(c, Icon::X.at(16.0, STROKE_THIN), 2.0, 0);
            assert_eq!((doubled.side, doubled.offset), (32, 0));
        });
        let mut unique = ids.clone();
        unique.sort_unstable_by_key(|id| format!("{id:?}"));
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "{ids:?}");
        assert_eq!(cached(&ctx), 6);
        // A new context (the renderer rebuilt) starts empty.
        assert_eq!(cached(&Context::default()), 0);
    }

    #[test]
    fn textures_not_drawn_for_a_while_are_dropped_once_the_cache_is_full() {
        let ctx = Context::default();
        let _ = pass(&ctx, 1.0, |ui| {
            for turn in 0..EVICT_ABOVE as u16 {
                texture(ui.ctx(), Icon::LoaderCircle.at(14.0, STROKE_REGULAR), 1.0, turn);
            }
        });
        assert_eq!(cached(&ctx), EVICT_ABOVE);
        // still drawn lately: a new texture keeps them
        let _ = pass(&ctx, 1.0, |ui| {
            texture(ui.ctx(), Icon::X.at(14.0, STROKE_THIN), 1.0, 0);
        });
        assert_eq!(cached(&ctx), EVICT_ABOVE + 1);
        for _ in 0..KEEP_PASSES {
            let _ = pass(&ctx, 1.0, |ui| {
                texture(ui.ctx(), Icon::X.at(14.0, STROKE_THIN), 1.0, 0);
            });
        }
        let _ = pass(&ctx, 1.0, |ui| {
            texture(ui.ctx(), Icon::Plus.at(14.0, STROKE_THIN), 1.0, 0);
        });
        assert_eq!(cached(&ctx), 2, "the spinner's turns went, X (drawn every pass) and Plus stay");
    }

    /// The meshes a pass painted.
    fn meshes(output: &egui::FullOutput) -> Vec<Mesh> {
        output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Mesh(m) => Some((**m).clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn paint_centres_the_icon_on_the_pixel_grid_tinted() {
        let ctx = Context::default();
        let tint = Color32::from_rgb(196, 196, 196);
        for ppp in [1.0, 1.5, 2.0] {
            let output = pass(&ctx, ppp, |ui| {
                // a 32 px button with a 15 px icon: the square starts 8.5 px in, snapped
                paint(ui.painter(), Rect::from_min_size(Pos2::new(100.0, 40.0), Vec2::splat(32.0)), Icon::Plane.at(15.0, STROKE_THIN), tint);
            });
            let meshes = meshes(&output);
            assert_eq!(meshes.len(), 1);
            let mesh = &meshes[0];
            assert!(mesh.vertices.iter().all(|v| v.color == tint));
            let (min, max) = (mesh.vertices[0].pos, mesh.vertices[3].pos);
            for p in [min, max] {
                assert!(((p.x * ppp) - (p.x * ppp).round()).abs() < 1e-3 && ((p.y * ppp) - (p.y * ppp).round()).abs() < 1e-3, "×{ppp}: {p:?} off the grid");
            }
            assert!(((max.x - min.x) * ppp - whole(15.0 * ppp) as f32).abs() < 1e-3, "×{ppp}: one texel per pixel");
            assert!((min.x - 108.5).abs() <= 0.5 / ppp && (min.y - 48.5).abs() <= 0.5 / ppp, "×{ppp}: {min:?}");
            let uv: Vec<Pos2> = mesh.vertices.iter().map(|v| v.uv).collect();
            assert_eq!(uv, [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0), Pos2::new(1.0, 1.0)]);
        }
    }

    #[test]
    fn a_rotation_is_baked_to_the_degree_and_the_rest_turns_the_quad() {
        let ctx = Context::default();
        let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(14.0));
        let output = pass(&ctx, 1.0, |ui| {
            paint(ui.painter(), rect, Icon::Plane.at(14.0, STROKE_REGULAR).rotated(std::f32::consts::FRAC_PI_4), Color32::WHITE);
        });
        let quad = &meshes(&output)[0];
        // 45° baked: an upright 20 px quad round the 14 px square
        assert_eq!([quad.vertices[0].pos, quad.vertices[3].pos], [Pos2::new(7.0, 7.0), Pos2::new(27.0, 27.0)]);
        assert_eq!(cached(&ctx), 1);

        let output = pass(&ctx, 1.0, |ui| {
            paint(ui.painter(), rect, Icon::Plane.at(14.0, STROKE_REGULAR).rotated(45.3f32.to_radians()), Color32::WHITE);
        });
        assert_eq!(cached(&ctx), 1, "45.3° uses the 45° raster");
        let quad = &meshes(&output)[0];
        let centre = Pos2::new(17.0, 17.0);
        let corner = quad.vertices[0].pos - centre;
        let expected = Rot2::from_angle(0.3f32.to_radians()) * Vec2::new(-10.0, -10.0);
        assert!((corner - expected).length() < 1e-3, "{corner:?} {expected:?}");
    }

    #[test]
    fn show_takes_the_icons_square() {
        let ctx = Context::default();
        let mut rects = Vec::new();
        let _ = pass(&ctx, 1.0, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                rects.push(show(ui, Icon::Funnel.at(14.0, STROKE_THIN), Color32::WHITE).rect);
                rects.push(show(ui, Icon::Funnel.at(14.0, STROKE_THIN), Color32::WHITE).rect);
            });
        });
        assert_eq!(rects[0].size(), Vec2::splat(14.0));
        assert_eq!(rects[1].min.x - rects[0].max.x, 8.0);
    }
}
