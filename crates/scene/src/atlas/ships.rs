//! The ship icon atlas (item H2; doc 02 §4.2, `ships.ts:79-124` `shipAtlas`), drawn with tiny-skia
//! where today's app draws it on a Canvas 2D.
//!
//! [`WIDTH`] × [`HEIGHT`] RGBA, cells of [`CELL`] px: one row per type colour, in
//! [`SHIP_COLOR_KEYS`] order (`0, 2, 3, 4, 5, 6, 7, 8`). Column 0 is the pointed hull of a ship
//! under way (`hull-<key>`, pointing up, north), column 1 the dot of a ship without a heading
//! (`dot-<key>`), as [`ship_icon`] names them. Each glyph is the row's type colour with a border in
//! [`INK`], stroked first and then filled over, so only the outer half of the stroke shows (joins
//! round): the hull's stroke is 5 px wide, the dot's 8 px. The icons are full colour (deck's
//! `mask: false`): the instance colour's alpha fades them.
//!
//! The pixels are straight (not premultiplied) RGBA, as a canvas reads back and as WebGL takes a
//! canvas (`UNPACK_PREMULTIPLY_ALPHA_WEBGL` off).
//!
//! Deviation (plan section 6, item 11): tiny-skia's anti-aliasing, not Chrome's, so the edges are
//! not bit-identical; the canvas's `arc` is tiny-skia's circle (four cubics).
//!
//! [`ship_icon`]: rustradar_core::ships::ship_icon

use rustradar_core::geo::Rgb;
use rustradar_core::ships::{SHIP_COLOR_KEYS, ship_color};
use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

/// Cell size of the atlas, pixels; glyphs are drawn at twice their usual size for sharpness.
pub const CELL: u32 = 64;
/// The hull, then the dot.
pub const WIDTH: u32 = CELL * 2;
/// A row per type colour.
pub const ROWS: usize = SHIP_COLOR_KEYS.len();
pub const HEIGHT: u32 = CELL * ROWS as u32;
/// The app's navy ink, `rgba(8, 12, 20, 0.9)`: the glyphs' border.
pub const INK: [u8; 4] = [8, 12, 20, 230];

/// The hull's border, px (`lineWidth = 5`).
const HULL_STROKE: f32 = 5.0;
/// The dot's border, px (`lineWidth = 8`).
const DOT_STROKE: f32 = 8.0;
/// The dot: `arc(CELL + 32, y + 32, 20, 0, 2π)`.
const DOT_CENTRE: f32 = 32.0;
const DOT_RADIUS: f32 = 20.0;

/// One icon's cell in the atlas, texels (an entry of deck's `iconMapping`, anchored at its middle,
/// `mask: false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The drawn atlas: [`WIDTH`] × [`HEIGHT`] straight RGBA texels, row by row from the top.
#[derive(Clone, PartialEq)]
pub struct ShipAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for ShipAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShipAtlas").field("width", &self.width).field("height", &self.height).finish_non_exhaustive()
    }
}

impl ShipAtlas {
    /// The texel at `(x, y)`.
    pub fn texel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// The rows' colour keys and colours, top to bottom.
pub fn rows() -> Vec<(u8, Rgb)> {
    SHIP_COLOR_KEYS.iter().map(|&key| (key, ship_color(key))).collect()
}

/// The row of the colour key written `key` (`0`, `2`, … as the atlas writes it: no sign, no
/// leading zero).
fn row_of(key: &str) -> Option<usize> {
    SHIP_COLOR_KEYS.iter().position(|k| k.to_string() == key)
}

/// The cell of the icon `name` (`hull-<key>` or `dot-<key>`, [`rustradar_core::ships::ship_icon`]);
/// none for a name the atlas has no cell for (deck draws nothing for it).
pub fn cell(name: &str) -> Option<Cell> {
    let (column, key) = if let Some(key) = name.strip_prefix("hull-") {
        (0, key)
    } else {
        (1, name.strip_prefix("dot-")?)
    };
    let row = row_of(key)? as u32;
    Some(Cell { x: column * CELL, y: row * CELL, width: CELL, height: CELL })
}

/// Every icon's name and cell, as deck's `mapping` (row by row, the hull before the dot).
pub fn mapping() -> Vec<(String, Cell)> {
    rows()
        .into_iter()
        .enumerate()
        .flat_map(|(row, (key, _))| {
            let y = row as u32 * CELL;
            [
                (format!("hull-{key}"), Cell { x: 0, y, width: CELL, height: CELL }),
                (format!("dot-{key}"), Cell { x: CELL, y, width: CELL, height: CELL }),
            ]
        })
        .collect()
}

/// The hull of the row at `y`, pointing up (north): a pointed bow, a square stern.
fn hull(y: f32) -> Path {
    let mut p = PathBuilder::new();
    p.move_to(32.0, y + 4.0);
    p.line_to(47.0, y + 24.0);
    p.line_to(46.0, y + 59.0);
    p.line_to(18.0, y + 59.0);
    p.line_to(17.0, y + 24.0);
    p.close();
    p.finish().expect("the hull path")
}

/// The dot of the row at `y`.
fn dot(y: f32) -> Path {
    PathBuilder::from_circle(CELL as f32 + DOT_CENTRE, y + DOT_CENTRE, DOT_RADIUS).expect("the dot")
}

fn paint(rgba: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color(Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
    paint
}

/// `ctx.lineJoin = "round"` with the canvas's default butt caps.
fn stroke(width: f32) -> Stroke {
    Stroke { width, line_join: LineJoin::Round, line_cap: LineCap::Butt, ..Stroke::default() }
}

/// Draws the atlas (`shipAtlas()`): for each row, the hull stroked in [`INK`] then filled with the
/// row's colour, then the dot stroked and filled.
pub fn draw() -> ShipAtlas {
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT).expect("the atlas's size");
    let ink = paint(INK);
    let none = Transform::identity();
    for (row, (_, [r, g, b])) in rows().into_iter().enumerate() {
        let y = (row as u32 * CELL) as f32;
        let fill = paint([r, g, b, 255]);
        let hull = hull(y);
        pixmap.stroke_path(&hull, &ink, &stroke(HULL_STROKE), none, None);
        pixmap.fill_path(&hull, &fill, FillRule::Winding, none, None);
        let dot = dot(y);
        pixmap.stroke_path(&dot, &ink, &stroke(DOT_STROKE), none, None);
        pixmap.fill_path(&dot, &fill, FillRule::Winding, none, None);
    }
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    ShipAtlas { width: WIDTH, height: HEIGHT, rgba }
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::Ship;
    use rustradar_core::ships::ship_icon;

    use super::*;

    /// Whether `t` is `want` within 2 levels on every channel (tiny-skia keeps its pixels
    /// premultiplied, so a straight colour comes back within a level or so).
    fn near(t: [u8; 4], want: [u8; 4]) -> bool {
        t.iter().zip(want).all(|(&a, b)| a.abs_diff(b) <= 2)
    }

    /// The texels of `cell` that are drawn (alpha above 0).
    fn covered(atlas: &ShipAtlas, cell: Cell) -> Vec<(u32, u32, [u8; 4])> {
        let mut out = Vec::new();
        for y in cell.y..cell.y + cell.height {
            for x in cell.x..cell.x + cell.width {
                let t = atlas.texel(x, y);
                if t[3] > 0 {
                    out.push((x, y, t));
                }
            }
        }
        out
    }

    #[test]
    fn eight_rows_of_two_cells_in_the_colour_keys_order() {
        assert_eq!((WIDTH, HEIGHT), (128, 64 * 8));
        let rows = rows();
        assert_eq!(rows.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [0, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(rows[0].1, [140, 152, 168], "the unknown's grey");
        assert_eq!(rows[5].1, [218, 92, 92], "the tankers' red");
        let mapping = mapping();
        assert_eq!(mapping.len(), 16);
        assert_eq!(mapping[0], ("hull-0".to_owned(), Cell { x: 0, y: 0, width: 64, height: 64 }));
        assert_eq!(mapping[1], ("dot-0".to_owned(), Cell { x: 64, y: 0, width: 64, height: 64 }));
        assert_eq!(mapping[2], ("hull-2".to_owned(), Cell { x: 0, y: 64, width: 64, height: 64 }));
        assert_eq!(mapping[15], ("dot-8".to_owned(), Cell { x: 64, y: 64 * 7, width: 64, height: 64 }));
        for (name, c) in &mapping {
            assert_eq!(cell(name), Some(*c), "{name}");
        }
        for bad in ["hull-", "hull-1", "dot-9", "dot-02", "hull-+2", "car-3", "Hull-3", "hull-3 ", ""] {
            assert_eq!(cell(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn every_icon_the_ships_ask_for_has_a_cell() {
        for kind in 0..=u8::MAX {
            for heading in [None, Some(90.0)] {
                let name = ship_icon(&Ship { kind, heading, ..Ship::default() });
                assert!(cell(&name).is_some(), "{name}");
            }
        }
        // listed types have their own row; any other shares the unknown's
        assert_eq!(cell(&ship_icon(&Ship { kind: 6, heading: Some(1.0), ..Ship::default() })), Some(Cell { x: 0, y: 5 * 64, width: 64, height: 64 }));
        assert_eq!(cell(&ship_icon(&Ship { kind: 9, heading: None, ..Ship::default() })), Some(Cell { x: 64, y: 0, width: 64, height: 64 }));
    }

    #[test]
    fn every_key_maps_to_a_non_empty_cell_of_the_stated_size() {
        let atlas = draw();
        assert_eq!((atlas.width, atlas.height, atlas.rgba.len()), (WIDTH, HEIGHT, (WIDTH * HEIGHT * 4) as usize));
        let rows = rows();
        for (name, c) in mapping() {
            assert_eq!((c.width, c.height), (CELL, CELL), "{name}");
            let key: u8 = name.split('-').nth(1).unwrap().parse().unwrap();
            let (_, [r, g, b]) = rows.iter().find(|(k, _)| *k == key).expect("its row");
            let drawn = covered(&atlas, c);
            assert!(!drawn.is_empty(), "{name} is empty");
            // the row's colour inside, opaque
            let fill = drawn.iter().filter(|(_, _, t)| *t == [*r, *g, *b, 255]).count();
            // the border: ink at the stroke's alpha
            let ink = drawn.iter().filter(|(_, _, t)| near(*t, INK)).count();
            assert!(fill > 800, "{name}: {fill} texels of its colour");
            assert!(ink > 100, "{name}: {ink} texels of ink");
            // nothing spills into the next cell: a texel's margin all round is clear
            for &(x, y, _) in &drawn {
                assert!(x > c.x && x < c.x + c.width - 1 && y > c.y && y < c.y + c.height - 1, "{name} at {x},{y}");
            }
        }
    }

    #[test]
    fn the_hull_points_up_and_the_dot_is_round() {
        let atlas = draw();
        // the passengers' blue, row 2
        let [r, g, b] = [98, 152, 240];
        let y0 = 2 * CELL;
        // the hull: between x 17/18 and 46/47 from y 24 down to its square stern at 59, its bow
        // at (32, 4)
        let mid = y0 + 40;
        assert_eq!(atlas.texel(32, mid), [r, g, b, 255]);
        assert_eq!(atlas.texel(19, mid), [r, g, b, 255], "the fill reaches the outline");
        assert_eq!(atlas.texel(45, mid), [r, g, b, 255]);
        // half the 5 px stroke outside the outline, in ink
        assert!(near(atlas.texel(16, mid), INK), "{:?}", atlas.texel(16, mid));
        assert!(near(atlas.texel(48, mid), INK), "{:?}", atlas.texel(48, mid));
        assert_eq!(atlas.texel(13, mid)[3], 0, "clear beyond the border");
        assert_eq!(atlas.texel(51, mid)[3], 0);
        // the stern is square: its border runs straight across below y 59
        assert!(near(atlas.texel(32, y0 + 60), INK), "{:?}", atlas.texel(32, y0 + 60));
        assert_eq!(atlas.texel(32, y0 + 58), [r, g, b, 255]);
        // the bow: narrow at the top, as wide as the hull from y 24
        let width_at = |y: u32| (0..CELL).filter(|&x| atlas.texel(x, y) == [r, g, b, 255]).count();
        assert!(width_at(y0 + 10) < 12, "{}", width_at(y0 + 10));
        assert!(width_at(y0 + 30) >= 27, "{}", width_at(y0 + 30));
        assert!(near(atlas.texel(32, y0 + 2), INK), "the bow's border {:?}", atlas.texel(32, y0 + 2));
        // the dot: radius 20 round (96, y + 32), its border 4 px outside, as wide as tall
        let cy = y0 + 32;
        assert_eq!(atlas.texel(96, cy), [r, g, b, 255]);
        assert_eq!(atlas.texel(96 - 19, cy), [r, g, b, 255]);
        assert_eq!(atlas.texel(96, cy + 19), [r, g, b, 255]);
        assert!(near(atlas.texel(96 - 22, cy), INK), "{:?}", atlas.texel(96 - 22, cy));
        assert!(near(atlas.texel(96, cy - 22), INK), "{:?}", atlas.texel(96, cy - 22));
        assert_eq!(atlas.texel(96 - 25, cy)[3], 0);
        // round: the cell's corner square stays clear where a box would reach
        assert_eq!(atlas.texel(96 - 18, cy - 18)[3], 0, "outside the circle");
        let dot = covered(&atlas, Cell { x: CELL, y: y0, width: CELL, height: CELL });
        let (x0, x1) = (dot.iter().map(|d| d.0).min().unwrap(), dot.iter().map(|d| d.0).max().unwrap());
        let (t0, t1) = (dot.iter().map(|d| d.1).min().unwrap(), dot.iter().map(|d| d.1).max().unwrap());
        assert_eq!(x1 - x0, t1 - t0, "as wide as tall");
        assert!((x1 - x0 + 1).abs_diff(48) <= 1, "radius 20 plus half the 8 px border: {}", x1 - x0 + 1);
    }
}
