//! The train icon atlas (item T2; doc 02 §5.2, `trains.ts:195-258` `trainAtlas`), drawn with
//! tiny-skia where today's app draws it on a Canvas 2D.
//!
//! [`WIDTH`] × [`HEIGHT`] RGBA, cells of [`CELL`] px: one row per kind ([`TRAIN_KINDS`], keyed by
//! [`train_kind_key`]) then one per operator with a colour of its own ([`OPERATORS`], keyed
//! `op{i}`), 21 rows. Column 0 is the carriage (`car-<key>`, pointing up, north), column 1 the
//! rounded square of a train without a heading (`box-<key>`), as [`train_icon`] names them. Every
//! glyph is the row's colour with a border in [`INK`] (stroked first, then filled over, so only the
//! outer half of the stroke shows; joins round); the carriage has two joints across it in [`INK`].
//! The icons are full colour (deck's `mask: false`): the instance colour's alpha fades them.
//!
//! The pixels are straight (not premultiplied) RGBA, as a canvas reads back and as WebGL takes a
//! canvas (`UNPACK_PREMULTIPLY_ALPHA_WEBGL` off).
//!
//! Deviation (plan section 6, item 11): tiny-skia's anti-aliasing, not Chrome's, so the edges are
//! not bit-identical; the canvas's `roundRect` corners are drawn as cubic quarter circles.
//!
//! [`train_icon`]: rustradar_core::trains::train_icon

use rustradar_core::geo::Rgb;
use rustradar_core::trains::{OPERATORS, TRAIN_KINDS, train_kind_key};
use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

/// Cell size of the atlas, pixels; glyphs are drawn at about three times their usual size for
/// sharpness.
pub const CELL: u32 = 64;
/// The carriage, then the square.
pub const WIDTH: u32 = CELL * 2;
/// A row per kind's colour, then one per operator's.
pub const ROWS: usize = TRAIN_KINDS.len() + OPERATORS.len();
pub const HEIGHT: u32 = CELL * ROWS as u32;
/// The app's navy ink, `rgba(8, 12, 20, 0.9)`: the glyphs' border and the carriage joints.
pub const INK: [u8; 4] = [8, 12, 20, 230];

/// The carriage's border, px (`lineWidth = 5`).
const CAR_STROKE: f32 = 5.0;
/// The joints' width, px.
const JOINT_STROKE: f32 = 2.5;
/// The square's border, px.
const BOX_STROKE: f32 = 6.0;
/// The square: `roundRect(CELL + 14, y + 14, 36, 36, 9)`.
const BOX_INSET: f32 = 14.0;
const BOX_SIDE: f32 = 36.0;
const BOX_RADIUS: f32 = 9.0;

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
pub struct TrainAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for TrainAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrainAtlas").field("width", &self.width).field("height", &self.height).finish_non_exhaustive()
    }
}

impl TrainAtlas {
    /// The texel at `(x, y)`.
    pub fn texel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
}

/// The rows' keys and colours, top to bottom: the kinds, then `op0` to `op15`.
pub fn rows() -> Vec<(String, Rgb)> {
    TRAIN_KINDS
        .iter()
        .map(|&kind| (train_kind_key(kind).to_owned(), rustradar_core::trains::train_color(kind)))
        .chain(OPERATORS.iter().enumerate().map(|(i, (_, color))| (format!("op{i}"), *color)))
        .collect()
}

/// The row of the key `key` (a kind's name or `op{i}`).
fn row_of(key: &str) -> Option<usize> {
    if let Some(i) = TRAIN_KINDS.iter().position(|&kind| train_kind_key(kind) == key) {
        return Some(i);
    }
    let i: usize = key.strip_prefix("op")?.parse().ok()?;
    // `op{i}` as the atlas writes it: no sign, no leading zero
    (i < OPERATORS.len() && key == format!("op{i}")).then_some(TRAIN_KINDS.len() + i)
}

/// The cell of the icon `name` (`car-<key>` or `box-<key>`, [`rustradar_core::trains::train_icon`]);
/// none for a name the atlas has no cell for (deck draws nothing for it).
pub fn cell(name: &str) -> Option<Cell> {
    let (column, key) = if let Some(key) = name.strip_prefix("car-") {
        (0, key)
    } else {
        (1, name.strip_prefix("box-")?)
    };
    let row = row_of(key)? as u32;
    Some(Cell { x: column * CELL, y: row * CELL, width: CELL, height: CELL })
}

/// Every icon's name and cell, as deck's `mapping` (row by row, the carriage before the square).
pub fn mapping() -> Vec<(String, Cell)> {
    rows()
        .into_iter()
        .enumerate()
        .flat_map(|(row, (key, _))| {
            let y = row as u32 * CELL;
            [
                (format!("car-{key}"), Cell { x: 0, y, width: CELL, height: CELL }),
                (format!("box-{key}"), Cell { x: CELL, y, width: CELL, height: CELL }),
            ]
        })
        .collect()
}

/// The carriage of the row at `y`: 26 wide, 56 long, a pointed, rounded nose.
fn carriage(y: f32) -> Path {
    let (left, right, top, bottom) = (19.0, 45.0, y + 4.0, y + 60.0);
    let mut p = PathBuilder::new();
    p.move_to(32.0, top);
    p.cubic_to(40.0, top + 2.0, right, top + 9.0, right, top + 18.0);
    p.line_to(right, bottom - 3.0);
    p.quad_to(right, bottom, right - 3.0, bottom);
    p.line_to(left + 3.0, bottom);
    p.quad_to(left, bottom, left, bottom - 3.0);
    p.line_to(left, top + 18.0);
    p.cubic_to(left, top + 9.0, 24.0, top + 2.0, 32.0, top);
    p.close();
    p.finish().expect("the carriage path")
}

/// The joints between the cars: two lines across the carriage, `top + 25` and `top + 41`.
fn joints(y: f32) -> Path {
    let (left, right, top) = (19.0, 45.0, y + 4.0);
    let mut p = PathBuilder::new();
    for j in [top + 25.0, top + 41.0] {
        p.move_to(left + 1.0, j);
        p.line_to(right - 1.0, j);
    }
    p.finish().expect("the joints' path")
}

/// The canvas's `roundRect(x, y, w, h, r)`: from the top edge's start clockwise, each corner a
/// quarter circle (as cubics).
fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    // the cubic nearest a quarter circle
    const K: f32 = 0.552_284_8;
    let c = r * K;
    let mut p = PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.cubic_to(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.cubic_to(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
    p.line_to(x, y + r);
    p.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
    p.close();
    p.finish().expect("the rounded square")
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

/// Draws the atlas (`trainAtlas()`): for each row, the carriage stroked in [`INK`] then filled
/// with the row's colour, its joints, then the square stroked and filled.
pub fn draw() -> TrainAtlas {
    let mut pixmap = Pixmap::new(WIDTH, HEIGHT).expect("the atlas's size");
    let ink = paint(INK);
    let none = Transform::identity();
    for (row, (_, [r, g, b])) in rows().into_iter().enumerate() {
        let y = (row as u32 * CELL) as f32;
        let fill = paint([r, g, b, 255]);
        let car = carriage(y);
        pixmap.stroke_path(&car, &ink, &stroke(CAR_STROKE), none, None);
        pixmap.fill_path(&car, &fill, FillRule::Winding, none, None);
        pixmap.stroke_path(&joints(y), &ink, &stroke(JOINT_STROKE), none, None);
        let square = round_rect(CELL as f32 + BOX_INSET, y + BOX_INSET, BOX_SIDE, BOX_SIDE, BOX_RADIUS);
        pixmap.stroke_path(&square, &ink, &stroke(BOX_STROKE), none, None);
        pixmap.fill_path(&square, &fill, FillRule::Winding, none, None);
    }
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    TrainAtlas { width: WIDTH, height: HEIGHT, rgba }
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::TrainKind;
    use rustradar_core::trains::{train_color, train_icon};

    use super::*;

    /// Whether `t` is `want` within 2 levels on every channel (tiny-skia keeps its pixels
    /// premultiplied, so a straight colour comes back within a level or so).
    fn near(t: [u8; 4], want: [u8; 4]) -> bool {
        t.iter().zip(want).all(|(&a, b)| a.abs_diff(b) <= 2)
    }

    /// Whether `t` is the ink drawn over a colour: opaque and dark.
    fn inked(t: [u8; 4], fill: [u8; 3]) -> bool {
        t[3] == 255 && (0..3).all(|c| t[c] < fill[c] / 4 + 12)
    }

    /// The texels of `cell` that are drawn (alpha above 0).
    fn covered(atlas: &TrainAtlas, cell: Cell) -> Vec<(u32, u32, [u8; 4])> {
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
    fn twenty_one_rows_of_two_cells_the_kinds_then_the_operators() {
        assert_eq!((WIDTH, HEIGHT), (128, 64 * 21));
        let rows = rows();
        assert_eq!(rows.len(), 21);
        let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(&keys[..5], ["highSpeed", "intercity", "regional", "suburban", "other"]);
        assert_eq!(keys[5], "op0");
        assert_eq!(keys[20], "op15");
        assert_eq!(rows[2].1, train_color(TrainKind::Regional));
        assert_eq!(rows[5 + 6].1, [250, 212, 64], "op6 is Arlanda Express's yellow");
        let mapping = mapping();
        assert_eq!(mapping.len(), 42);
        assert_eq!(mapping[0], ("car-highSpeed".to_owned(), Cell { x: 0, y: 0, width: 64, height: 64 }));
        assert_eq!(mapping[1], ("box-highSpeed".to_owned(), Cell { x: 64, y: 0, width: 64, height: 64 }));
        assert_eq!(mapping[41], ("box-op15".to_owned(), Cell { x: 64, y: 64 * 20, width: 64, height: 64 }));
        for (name, c) in &mapping {
            assert_eq!(cell(name), Some(*c), "{name}");
        }
        for bad in ["car-", "box-op16", "car-op01", "car-op-1", "dot-regional", "car-Regional", "hull-0", ""] {
            assert_eq!(cell(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn every_icon_the_trains_ask_for_has_a_cell() {
        let operators = [None, Some("SJ"), Some("Mälartåg"), Some("Inlandsbanan"), Some("  VY Tåg "), Some("Värmlandstrafik")];
        for kind in TRAIN_KINDS {
            for heading in [None, Some(90.0)] {
                for operator in operators {
                    let name = train_icon(kind, heading, operator);
                    assert!(cell(&name).is_some(), "{name}");
                }
            }
        }
        assert_eq!(cell(&train_icon(TrainKind::Regional, Some(1.0), Some("SJ"))), Some(Cell { x: 0, y: 5 * 64, width: 64, height: 64 }));
        assert_eq!(cell(&train_icon(TrainKind::Suburban, None, None)), Some(Cell { x: 64, y: 3 * 64, width: 64, height: 64 }));
    }

    #[test]
    fn every_key_maps_to_a_non_empty_cell_of_the_stated_size() {
        let atlas = draw();
        assert_eq!((atlas.width, atlas.height, atlas.rgba.len()), (WIDTH, HEIGHT, (WIDTH * HEIGHT * 4) as usize));
        let rows = rows();
        for (name, c) in mapping() {
            assert_eq!((c.width, c.height), (CELL, CELL), "{name}");
            let key = &name[4..];
            let (_, [r, g, b]) = rows.iter().find(|(k, _)| k == key).expect("its row");
            let drawn = covered(&atlas, c);
            assert!(!drawn.is_empty(), "{name} is empty");
            // the row's colour inside, opaque
            let fill = drawn.iter().filter(|(_, _, t)| *t == [*r, *g, *b, 255]).count();
            // the border: ink at the stroke's alpha
            let ink = drawn.iter().filter(|(_, _, t)| near(*t, INK)).count();
            assert!(fill > 600, "{name}: {fill} texels of its colour");
            assert!(ink > 100, "{name}: {ink} texels of ink");
            // nothing spills into the next cell: a texel's margin all round is clear
            for &(x, y, _) in &drawn {
                assert!(x > c.x && x < c.x + c.width - 1 && y > c.y && y < c.y + c.height - 1, "{name} at {x},{y}");
            }
        }
    }

    #[test]
    fn the_carriage_points_up_with_two_joints_and_the_square_is_rounded() {
        let atlas = draw();
        let (r, g, b) = (226, 92, 196);
        let row = 0;
        let y0 = row * CELL;
        // the carriage: 26 wide inside its border between x 19 and 45, 56 long from y 4 to 60
        let mid = y0 + 40;
        assert_eq!(atlas.texel(32, mid), [r, g, b, 255]);
        assert_eq!(atlas.texel(20, mid), [r, g, b, 255], "the fill reaches the outline");
        assert_eq!(atlas.texel(44, mid), [r, g, b, 255]);
        // half the 5 px stroke outside the outline, in ink
        assert!(near(atlas.texel(18, mid), INK), "{:?}", atlas.texel(18, mid));
        assert!(near(atlas.texel(46, mid), INK), "{:?}", atlas.texel(46, mid));
        assert_eq!(atlas.texel(15, mid)[3], 0, "clear beyond the border");
        assert_eq!(atlas.texel(49, mid)[3], 0);
        // the nose is narrower than the tail: at 6 px from each end
        let width_at = |y: u32| (0..CELL).filter(|&x| atlas.texel(x, y)[3] > 0).count();
        assert!(width_at(y0 + 6) < width_at(y0 + 58), "{} vs {}", width_at(y0 + 6), width_at(y0 + 58));
        // the joints at top + 25 and top + 41: ink over the colour across the carriage, colour
        // between
        for j in [y0 + 29, y0 + 45] {
            assert!(inked(atlas.texel(32, j), [r, g, b]), "joint at {j}: {:?}", atlas.texel(32, j));
            assert!(inked(atlas.texel(25, j), [r, g, b]), "joint at {j}: {:?}", atlas.texel(25, j));
        }
        assert_eq!(atlas.texel(32, y0 + 37), [r, g, b, 255]);
        // the square: 36 px from 78 to 114, rounded by 9; its border 3 px outside
        let sy = y0 + 32;
        assert_eq!(atlas.texel(96, sy), [r, g, b, 255]);
        assert_eq!(atlas.texel(79, sy), [r, g, b, 255]);
        assert!(near(atlas.texel(76, sy), INK), "{:?}", atlas.texel(76, sy));
        assert_eq!(atlas.texel(73, sy)[3], 0);
        // a corner is cut: the square's corner texel is clear, its middle edge is not
        assert_eq!(atlas.texel(CELL + 12, y0 + 12)[3], 0, "the rounded corner");
        assert!(near(atlas.texel(96, y0 + 12), INK), "{:?}", atlas.texel(96, y0 + 12));
    }
}
