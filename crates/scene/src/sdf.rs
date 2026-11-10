//! TinySDF (item E10; plan 1.4.10, doc 01 §5.9 and §6): `@mapbox/tiny-sdf` 2.3.0, the version
//! today's app runs under deck.gl 9.4's TextLayer (`node_modules/@mapbox/tiny-sdf/index.js`),
//! ported line by line.
//!
//! A glyph is drawn black on a cleared canvas, its anti-aliased coverage read back, and two
//! squared Euclidean distance transforms (Felzenszwalb and Huttenlocher) worked out over it: one
//! to the ink from outside, one to the outside from the ink. Partly covered pixels seed both
//! grids with a small distance from the gamma-corrected `alphaTable`. The signed distance
//! `sqrt(outer) − sqrt(inner)` (pixels, positive outside) is stored as a byte,
//! `255·(1 − cutoff) − 255/radius · d`, clamped and rounded as a `Uint8ClampedArray` does: the
//! glyph's edge reads `1 − cutoff` (0.75 with deck's 0.25), `radius·(1 − cutoff)` pixels inside it
//! reads 1 and `radius·cutoff` pixels outside it reads 0.
//!
//! The canvas is a trait, [`Canvas`]: its `measureText` and `fillText`. `scene::text` implements it
//! with the vendored Overpass fonts (skrifa outlines filled by tiny-skia).
//!
//! The 2.3.0 detail that matters downstream: the glyph is drawn with its pen
//! `glyphLeft = floor(−actualBoundingBoxLeft)` pixels left of the image's `buffer` column, so that
//! its ink starts there whatever its left side bearing. deck 9.4 does not read `glyphLeft` (its
//! `getSdfFontRenderer` hands back `left: buffer`, right for TinySDF 2.0), so in today's app every
//! glyph is drawn that many atlas pixels left of its pen; `scene::text` puts the pen where TinySDF
//! drew it instead (its module docs say why).

use std::sync::LazyLock;

use rustradar_core::num::{clamp_u8, js_round};

/// TinySDF's "infinite" squared distance.
pub const INF: f64 = 1e20;

/// The lookup table for gamma-corrected, signed squared alpha distances: for coverage `i`,
/// `d·|d|` with `d = 0.5 − (i/255)^(1/2.2)`; full coverage is `−INF`.
static ALPHA_TABLE: LazyLock<[f64; 256]> = LazyLock::new(|| {
    let mut table = [0.0; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let d = 0.5 - (i as f64 / 255.0).powf(1.0 / 2.2);
        *t = d * d.abs();
    }
    table[255] = -INF;
    table
});

/// The fields of the canvas's `TextMetrics` that TinySDF and deck read, in pixels at the
/// canvas's font size (y up for ascent, the alignment point at the origin).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextMetrics {
    /// The advance (`width`).
    pub width: f64,
    /// How far the ink reaches above the baseline.
    pub actual_bounding_box_ascent: f64,
    /// How far it reaches below the baseline.
    pub actual_bounding_box_descent: f64,
    /// How far it reaches left of the alignment point (positive to the left, so negative for a
    /// glyph whose ink starts to the right of the pen).
    pub actual_bounding_box_left: f64,
    /// How far it reaches right of the alignment point.
    pub actual_bounding_box_right: f64,
}

/// A rectangle of canvas pixels: left, top, width, height.
pub type Region = [i32; 4];

/// The 2D canvas TinySDF draws on, with its font set (`textBaseline: 'alphabetic'`,
/// `textAlign: 'left'`, black fill).
pub trait Canvas {
    /// `ctx.measureText(ch)`.
    fn measure_text(&self, ch: char) -> TextMetrics;

    /// `ctx.fillText(ch, x, y)` on a cleared canvas (the pen at `(x, y)`, y down), then
    /// `ctx.getImageData(..region)`: the alpha of each pixel of `region`, rows top first.
    fn fill_text(&self, ch: char, x: f64, y: f64, region: Region) -> Vec<u8>;
}

/// One glyph drawn by [`TinySdf::draw`] (TinySDF's return value).
#[derive(Debug, Clone, PartialEq)]
pub struct SdfGlyph {
    /// `width × height` bytes, rows top first: the signed distance as described above.
    pub data: Vec<u8>,
    /// `glyph_width + 2·buffer`.
    pub width: u32,
    /// `glyph_height + 2·buffer`.
    pub height: u32,
    /// The ink's width and height in whole pixels (0 for a glyph with no ink).
    pub glyph_width: u32,
    pub glyph_height: u32,
    /// `ceil(actualBoundingBoxAscent)`: the baseline is `buffer + glyph_top` rows down the image.
    pub glyph_top: i32,
    /// `floor(−actualBoundingBoxLeft)`: the pen is `buffer − glyph_left` columns into the image.
    pub glyph_left: i32,
    /// The advance (`measureText(ch).width`).
    pub glyph_advance: f64,
}

/// TinySDF: one canvas size and settings, and its scratch grids.
#[derive(Debug, Clone)]
pub struct TinySdf {
    /// Padding round a glyph's bounding box, whole pixels (`Math.round(buffer)`).
    pub buffer: i32,
    /// How many pixels round the edge are encoded.
    pub radius: f64,
    /// How much of the byte range is outside the edge.
    pub cutoff: f64,
    /// The canvas's side: `ceil(font_size) + 4·buffer`.
    pub size: i32,
    /// The largest ink a glyph may have each way: `size − buffer`; more is clipped bottom and right.
    pub max_glyph_dim: i32,
    grid_outer: Vec<f64>,
    grid_inner: Vec<f64>,
    f: Vec<f64>,
    z: Vec<f64>,
    v: Vec<u16>,
}

impl TinySdf {
    /// TinySDF's constructor (its defaults are 24, 3, 8, 0.25; deck's labels use 64, 6, 12, 0.25).
    pub fn new(font_size: f64, buffer: f64, radius: f64, cutoff: f64) -> TinySdf {
        let buffer = js_round(buffer) as i32;
        let size = font_size.ceil() as i32 + buffer * 4;
        let max_glyph_dim = size - buffer;
        let grid_dim = (max_glyph_dim + 2 * buffer).max(0) as usize;
        TinySdf {
            buffer,
            radius,
            cutoff,
            size,
            max_glyph_dim,
            grid_outer: vec![0.0; grid_dim * grid_dim],
            grid_inner: vec![0.0; grid_dim * grid_dim],
            f: vec![0.0; grid_dim],
            z: vec![0.0; grid_dim + 1],
            v: vec![0; grid_dim],
        }
    }

    /// `draw(char)`: measures `ch`, fills it on `canvas` and encodes its signed distance.
    pub fn draw(&mut self, canvas: &impl Canvas, ch: char) -> SdfGlyph {
        let TextMetrics {
            width: glyph_advance,
            actual_bounding_box_ascent,
            actual_bounding_box_descent,
            actual_bounding_box_left,
            actual_bounding_box_right,
        } = canvas.measure_text(ch);

        // the integer part of the alignment is in glyph_top and glyph_left, the rest in the
        // rasterisation
        let glyph_top = actual_bounding_box_ascent.ceil() as i32;
        let glyph_left = (-actual_bounding_box_left).floor() as i32;

        // a glyph that overflows the canvas is clipped at the bottom and right
        let glyph_width = (self.max_glyph_dim.min(actual_bounding_box_right.ceil() as i32 - glyph_left)).max(0);
        let glyph_height = (self.max_glyph_dim.min(glyph_top + actual_bounding_box_descent.ceil() as i32)).max(0);

        let buffer = self.buffer;
        let width = glyph_width + 2 * buffer;
        let height = glyph_height + 2 * buffer;
        let len = (width * height) as usize;
        let mut glyph = SdfGlyph {
            data: vec![0; len],
            width: width as u32,
            height: height as u32,
            glyph_width: glyph_width as u32,
            glyph_height: glyph_height as u32,
            glyph_top,
            glyph_left,
            glyph_advance,
        };
        if glyph_width == 0 || glyph_height == 0 {
            return glyph;
        }

        let alpha = canvas.fill_text(ch, f64::from(buffer - glyph_left), f64::from(buffer + glyph_top), [buffer, buffer, glyph_width, glyph_height]);
        self.encode(&alpha, glyph_width, glyph_height, &mut glyph.data);
        glyph
    }

    /// The part of `draw` after `getImageData`: `alpha` is the ink's `glyph_width × glyph_height`
    /// coverage; `data` gets the `(glyph_width + 2·buffer) × (glyph_height + 2·buffer)` distances.
    pub fn encode(&mut self, alpha: &[u8], glyph_width: i32, glyph_height: i32, data: &mut [u8]) {
        let buffer = self.buffer;
        let width = glyph_width + 2 * buffer;
        let height = glyph_height + 2 * buffer;
        let len = (width * height) as usize;
        assert_eq!(alpha.len(), (glyph_width * glyph_height) as usize, "coverage of {glyph_width} × {glyph_height}");
        assert_eq!(data.len(), len, "distances of {width} × {height}");
        let table = &*ALPHA_TABLE;
        let (outer, inner) = (&mut self.grid_outer, &mut self.grid_inner);

        // by default outside the glyph (INF to the ink) and not inside it (0 to the outside)
        outer[..len].fill(INF);
        inner[..len].fill(0.0);

        // a partly covered pixel seeds both grids with a small distance from how far its coverage
        // is from a half
        for (y, row) in alpha.chunks_exact(glyph_width as usize).enumerate() {
            let start = ((y as i32 + buffer) * width + buffer) as usize;
            for (j, &a) in (start..).zip(row) {
                if a != 0 {
                    let t = table[a as usize];
                    outer[j] = t.max(0.0);
                    inner[j] = (-t).max(0.0);
                }
            }
        }

        // the columns left and right of the glyph have no seeds and stay INF in the column pass
        edt(outer, 0, 0, width, height, width, &mut self.f, &mut self.v, &mut self.z, buffer);
        // the inner region padded by 1 so ink touching the box's edge sees the outside seeds in
        // the buffer (and no further than the buffer when it is 0)
        let pad = buffer.min(1);
        edt(
            inner,
            buffer - pad,
            buffer - pad,
            glyph_width + 2 * pad,
            glyph_height + 2 * pad,
            width,
            &mut self.f,
            &mut self.v,
            &mut self.z,
            0,
        );

        // inside the glyph high, outside low, the edge's gradient spanning
        // [−radius·cutoff, radius·(1 − cutoff)] pixels round it
        let scale = 255.0 / self.radius;
        let base = 255.0 * (1.0 - self.cutoff);
        for i in 0..len {
            let d = outer[i].sqrt() - inner[i].sqrt();
            data[i] = clamp_u8(base - scale * d);
        }
    }
}

/// The 2D squared Euclidean distance transform of Felzenszwalb and Huttenlocher
/// (<https://cs.brown.edu/~pff/papers/dt-final.pdf>) over the `width × height` window at
/// `(x0, y0)` of a grid `grid_size` wide: the columns (all but `skip_cols` at each side), then the
/// rows.
#[allow(clippy::too_many_arguments)]
fn edt(data: &mut [f64], x0: i32, y0: i32, width: i32, height: i32, grid_size: i32, f: &mut [f64], v: &mut [u16], z: &mut [f64], skip_cols: i32) {
    for x in x0 + skip_cols..x0 + width - skip_cols {
        edt1d(data, (y0 * grid_size + x) as usize, grid_size as usize, height as usize, f, v, z);
    }
    for y in y0..y0 + height {
        edt1d(data, (y * grid_size + x0) as usize, 1, width as usize, f, v, z);
    }
}

/// The 1D squared distance transform along `length` cells from `offset`, `stride` apart.
fn edt1d(grid: &mut [f64], offset: usize, stride: usize, length: usize, f: &mut [f64], v: &mut [u16], z: &mut [f64]) {
    if length == 0 {
        return;
    }
    v[0] = 0;
    z[0] = -INF;
    z[1] = INF;
    f[0] = grid[offset];

    let mut k: isize = 0;
    for q in 1..length {
        let fq = grid[offset + q * stride];
        f[q] = fq;
        let qf = q as f64;
        let q2 = qf * qf;
        let mut s;
        loop {
            let r = f64::from(v[k as usize]);
            s = (fq - f[v[k as usize] as usize] + q2 - r * r) / (qf - r) / 2.0;
            // `while (s <= z[k] && --k > -1)`
            if s <= z[k as usize] {
                k -= 1;
                if k > -1 {
                    continue;
                }
            }
            break;
        }
        k += 1;
        v[k as usize] = q as u16;
        z[k as usize] = s;
        z[k as usize + 1] = INF;
    }

    let mut k = 0;
    for q in 0..length {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let r = v[k] as usize;
        let qr = q as f64 - r as f64;
        grid[offset + q * stride] = f[r] + qr * qr;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canvas whose only glyph is a square `side` pixels wide sitting on the baseline, pixel
    /// aligned (each pixel fully covered or empty), its left edge `lsb` pixels right of the pen.
    struct Square {
        side: i32,
        lsb: i32,
    }

    impl Canvas for Square {
        fn measure_text(&self, _: char) -> TextMetrics {
            TextMetrics {
                width: f64::from(self.side + 2 * self.lsb),
                actual_bounding_box_ascent: f64::from(self.side),
                actual_bounding_box_descent: 0.0,
                actual_bounding_box_left: -f64::from(self.lsb),
                actual_bounding_box_right: f64::from(self.lsb + self.side),
            }
        }

        fn fill_text(&self, _: char, x: f64, y: f64, [left, top, w, h]: Region) -> Vec<u8> {
            let (x0, x1) = (x as i32 + self.lsb, x as i32 + self.lsb + self.side);
            let (y0, y1) = (y as i32 - self.side, y as i32);
            let mut alpha = Vec::with_capacity((w * h) as usize);
            for py in top..top + h {
                for px in left..left + w {
                    alpha.push(if (x0..x1).contains(&px) && (y0..y1).contains(&py) { 255 } else { 0 });
                }
            }
            alpha
        }
    }

    /// The SDF value at `(x, y)`, 0 to 1.
    fn at(g: &SdfGlyph, x: u32, y: u32) -> f64 {
        f64::from(g.data[(y * g.width + x) as usize]) / 255.0
    }

    #[test]
    fn the_sdf_of_a_filled_square_is_symmetric_and_three_quarters_at_its_edge() {
        let mut sdf = TinySdf::new(64.0, 6.0, 12.0, 0.25);
        let g = sdf.draw(&Square { side: 20, lsb: 0 }, 'x');
        assert_eq!((g.glyph_width, g.glyph_height, g.width, g.height), (20, 20, 32, 32));
        assert_eq!((g.glyph_top, g.glyph_left), (20, 0));
        let n = g.width;
        // symmetric about both middles and the diagonal
        for y in 0..n {
            for x in 0..n {
                let v = g.data[(y * n + x) as usize];
                assert_eq!(v, g.data[(y * n + (n - 1 - x)) as usize], "mirrored left-right at {x},{y}");
                assert_eq!(v, g.data[((n - 1 - y) * n + x) as usize], "mirrored top-bottom at {x},{y}");
                assert_eq!(v, g.data[(x * n + y) as usize], "transposed at {x},{y}");
            }
        }
        // the edge is between columns 5 (outside) and 6 (inside), and between rows 5 and 6: the
        // two pixels either side are a pixel from it each way, so it reads 0.75 between them
        for i in 8..24 {
            let edge = (at(&g, 5, i) + at(&g, 6, i)) / 2.0;
            assert!((edge - 0.75).abs() <= 0.5 / 255.0, "edge at row {i}: {edge}");
            let edge = (at(&g, i, 5) + at(&g, i, 6)) / 2.0;
            assert!((edge - 0.75).abs() <= 0.5 / 255.0, "edge at column {i}: {edge}");
        }
        // a pixel inside reads 255·0.75 + 21.25 = 212.5 → 212 (ties to even), outside 170
        assert_eq!(g.data[(16 * n + 6) as usize], 212);
        assert_eq!(g.data[(16 * n + 5) as usize], 170);
        // the middle is 10 px in: 191.25 + 212.5 = 403.75 → 255; a corner is outside by √(6² + 6²)
        assert_eq!(g.data[(16 * n + 16) as usize], 255);
        assert_eq!(g.data[0], clamp_u8(191.25 - 21.25 * 72f64.sqrt()));
    }

    #[test]
    fn distances_fall_by_255_over_radius_per_pixel() {
        let mut sdf = TinySdf::new(64.0, 6.0, 12.0, 0.25);
        let g = sdf.draw(&Square { side: 20, lsb: 0 }, 'x');
        // along the middle row, outside the square, each pixel is one further out
        for x in 0..6 {
            let d = f64::from(6 - x);
            assert_eq!(g.data[(16 * g.width + x) as usize], clamp_u8(191.25 - 21.25 * d), "column {x}");
        }
    }

    #[test]
    fn the_pen_is_glyph_left_columns_left_of_the_buffer() {
        // a square 3 px right of the pen: glyph_left 3, so its ink starts at the buffer column
        let mut sdf = TinySdf::new(64.0, 6.0, 12.0, 0.25);
        let g = sdf.draw(&Square { side: 10, lsb: 3 }, 'x');
        assert_eq!((g.glyph_left, g.glyph_width, g.width), (3, 10, 22));
        assert_eq!(g.glyph_advance, 16.0);
        let middle = 6 + 5;
        assert_eq!(g.data[(middle * g.width + 5) as usize], 170, "outside, next to the ink");
        assert_eq!(g.data[(middle * g.width + 6) as usize], 212, "the ink's first column");
    }

    #[test]
    fn an_empty_glyph_is_all_far_outside() {
        let mut sdf = TinySdf::new(64.0, 6.0, 12.0, 0.25);
        let g = sdf.draw(&Square { side: 0, lsb: 0 }, ' ');
        assert_eq!((g.width, g.height, g.glyph_width, g.glyph_height), (12, 12, 0, 0));
        assert!(g.data.iter().all(|&v| v == 0));
    }

    #[test]
    fn ink_beyond_the_canvas_is_clipped() {
        // a 64 px canvas with buffer 6 is 88 px; a glyph may be at most 82 px each way
        let mut sdf = TinySdf::new(64.0, 6.0, 12.0, 0.25);
        assert_eq!((sdf.size, sdf.max_glyph_dim), (88, 82));
        let g = sdf.draw(&Square { side: 100, lsb: 0 }, 'x');
        assert_eq!((g.glyph_width, g.glyph_height), (82, 82));
    }

    #[test]
    fn buffer_rounds_as_javascript_does() {
        assert_eq!(TinySdf::new(24.0, 2.5, 8.0, 0.25).buffer, 3);
        assert_eq!(TinySdf::new(24.0, -2.5, 8.0, 0.25).buffer, -2);
        let sdf = TinySdf::new(24.0, 3.0, 8.0, 0.25);
        assert_eq!((sdf.size, sdf.max_glyph_dim), (36, 33));
    }

    #[test]
    fn partial_coverage_seeds_both_grids() {
        // a half-covered pixel is nearly on the edge: 0.5 − 0.5^(1/2.2) = −0.2295, so it is
        // 0.2295 px inside, which reads 191.25 + 21.25·0.2295 = 196.1
        let table = &*ALPHA_TABLE;
        assert!((table[128] - (0.5 - (128f64 / 255.0).powf(1.0 / 2.2)) * (0.5 - (128f64 / 255.0).powf(1.0 / 2.2)).abs()).abs() < 1e-15);
        assert_eq!(table[0], 0.25);
        assert_eq!(table[255], -INF);
        let mut sdf = TinySdf::new(8.0, 2.0, 8.0, 0.25);
        let mut data = vec![0; 25];
        sdf.encode(&[128], 1, 1, &mut data);
        let inside = (-table[128]).sqrt();
        assert_eq!(data[12], clamp_u8(255.0 * 0.75 + 255.0 / 8.0 * inside));
        // its neighbours are a pixel from it and outside: one pixel out
        assert_eq!(data[11], clamp_u8(255.0 * 0.75 - 255.0 / 8.0 * 1.0));
    }
}
