//! Thin wrapper over `earcutr` with math.gl's `earcut(positions, holeIndices, dim)` signature
//! (S1.24): the triangulation `roofs.ts` and `radar-mesh.ts` use.
//!
//! `earcutr` is a port of Mapbox's earcut, the algorithm math.gl adapted, so winding rules and
//! hole handling carry over. The triangles are not always the same ones: `earcutr` switches to
//! z-order hashing from 40 vertices where earcut does above 80, which changes which ear is cut
//! first. Callers must not depend on triangle order (doc 05 §13.1, §15.7).
//!
//! Deviations, all for input math.gl would turn into garbage or an exception: a position that is
//! not finite (or is `±f64::MAX`, which `earcutr` cannot start a ring with), a `dim` below 2, or
//! hole indices that are not increasing and inside the vertices give no triangles, where
//! `earcutr` would return an error or panic.

/// The triangles of a polygon: `positions` holds `dim` numbers per vertex (only x and y are
/// read), the outline first, then each hole from the vertex index in `hole_indices` (earcut's
/// convention: vertices, not values). The result is three vertex indices per triangle.
pub fn earcut(positions: &[f64], hole_indices: &[usize], dim: usize) -> Vec<usize> {
    if dim < 2 {
        return Vec::new();
    }
    let count = positions.len() / dim;
    let xy: Vec<f64> = if dim == 2 {
        positions[..count * 2].to_vec()
    } else {
        (0..count).flat_map(|v| [positions[v * dim], positions[v * dim + 1]]).collect()
    };
    if xy.iter().any(|v| !v.is_finite() || v.abs() == f64::MAX) {
        return Vec::new();
    }
    if hole_indices.first() == Some(&0)
        || hole_indices.windows(2).any(|w| w[0] >= w[1])
        || hole_indices.last().is_some_and(|&h| h >= count)
    {
        return Vec::new();
    }
    earcutr::earcut(&xy, hole_indices, 2).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The triangles' area, each counted positive.
    fn area(positions: &[f64], dim: usize, triangles: &[usize]) -> f64 {
        triangles
            .chunks(3)
            .map(|t| {
                let p = |k: usize| [positions[t[k] * dim], positions[t[k] * dim + 1]];
                let (a, b, c) = (p(0), p(1), p(2));
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs()
            })
            .sum::<f64>()
            / 2.0
    }

    #[test]
    fn a_square_is_two_triangles() {
        let square = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        let t = earcut(&square, &[], 2);
        assert_eq!(t.len(), 6);
        assert_eq!(area(&square, 2, &t), 100.0);
    }

    #[test]
    fn holes_are_left_out_either_winding() {
        let mut ring = vec![0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        ring.extend([2.0, 2.0, 2.0, 4.0, 4.0, 4.0, 4.0, 2.0]);
        assert_eq!(area(&ring, 2, &earcut(&ring, &[4], 2)), 96.0);
        // the outline clockwise and the hole anticlockwise: the same area
        let mut reversed = vec![0.0, 10.0, 10.0, 10.0, 10.0, 0.0, 0.0, 0.0];
        reversed.extend([4.0, 2.0, 4.0, 4.0, 2.0, 4.0, 2.0, 2.0]);
        assert_eq!(area(&reversed, 2, &earcut(&reversed, &[4], 2)), 96.0);
    }

    #[test]
    fn three_numbers_a_vertex_reads_x_and_y() {
        let square = [0.0, 0.0, 5.0, 4.0, 0.0, 6.0, 4.0, 4.0, 7.0, 0.0, 4.0, 8.0];
        let t = earcut(&square, &[], 3);
        assert_eq!(t.len(), 6);
        assert!(t.iter().all(|&i| i < 4));
        assert_eq!(area(&square, 3, &t), 16.0);
    }

    #[test]
    fn the_same_triangles_as_math_gl_without_holes() {
        // math.gl 4.1's earcut on the same input, run with bun
        let quad = [0.0, 0.0, 4.0, 0.0, 4.0, 3.0, 1.0, 2.0];
        assert_eq!(earcut(&quad, &[], 2), [2, 3, 0, 0, 1, 2]);
        let l = [0.0, 0.0, 10.0, 0.0, 10.0, 4.0, 4.0, 4.0, 4.0, 10.0, 0.0, 10.0];
        assert_eq!(earcut(&l, &[], 2), [4, 5, 0, 0, 1, 2, 3, 4, 0, 0, 2, 3]);
    }

    #[test]
    fn with_a_hole_as_many_triangles_as_math_gl_over_the_same_area() {
        // math.gl bridges the hole from another vertex, [0, 4, 5, 7, 4, 0, 3, 0, 5, 7, 0, 1, 2, 3,
        // 5, 6, 7, 1, 2, 5, 6, 6, 1, 2]: the same count of triangles, the same area
        let holed = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0, 3.0, 3.0, 3.0, 6.0, 6.0, 6.0, 6.0, 3.0];
        let t = earcut(&holed, &[4], 2);
        assert_eq!(t.len(), 24);
        assert_eq!(area(&holed, 2, &t), 91.0);
    }

    #[test]
    fn nothing_for_what_cannot_be_triangulated() {
        assert!(earcut(&[], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 1.0], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 0.0, 2.0, 0.0], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 0.0, f64::NAN, 1.0], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 0.0, 1.0, f64::INFINITY], &[], 2).is_empty());
        assert!(earcut(&[f64::MAX, 0.0, 1.0, 0.0, 1.0, 1.0], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0], &[], 1).is_empty());
        let square = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        assert!(earcut(&square, &[0], 2).is_empty());
        assert!(earcut(&square, &[4], 2).is_empty());
        assert!(earcut(&square, &[3, 2], 2).is_empty());
    }
}
