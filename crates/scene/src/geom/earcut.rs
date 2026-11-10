//! earcut 2.2.4 (`node_modules/earcut/src/earcut.js`) behind math.gl's `earcut(positions,
//! holeIndices, dim)` signature (S1.24; ported by B1): the triangulation deck.gl 9.4's
//! SolidPolygonLayer runs (`solid-polygon-layer/polygon.js`, `getSurfaceIndices`) and the one
//! `roofs.ts` and `radar-mesh.ts` call (math.gl 4.1's `earcut` is earcut 2.2.4 with the signed
//! area's sign turned round, and picks the same triangles).
//!
//! Transliterated with its formulas and branch order: z-order hashing above 80 vertices,
//! `filterPoints`, `cureLocalIntersections`, `splitEarcut`, and Eberly's hole bridge with
//! `sectorContainsSector`. So the triangles are earcut's own, in earcut's order, and on the globe
//! deck's very triangles (`tess::vector` checks them index for index against deck's
//! PolygonTesselator on six fixture tiles).
//!
//! Why not the `earcutr` crate (this module's first version): `earcutr` 0.5 follows an older
//! earcut whose hole bridging goes wrong where a hole touches the outline. Every part of a
//! polygon cut on the grid has such holes (the halves of an island cut by a grid line), and on
//! the fixture tiles' water `earcutr` covered too much (triangles over the holes, up to three
//! times a part's area) in 34 of 11 814 parts cut on the 5° grid and 2 of 2 924 whole polygons,
//! where earcut 2.2.4 is right on all of them.
//!
//! Deviations, all for input earcut.js would turn into garbage or an exception: a position whose
//! x or y is not finite, a `dim` below 2, or hole indices that are not increasing and inside the
//! vertices (so an empty hole too, where earcut.js throws) give no triangles.

use std::borrow::Cow;

use rustradar_core::num::to_int32;

use super::{js_max, js_min};

/// The triangles of a polygon: `positions` holds `dim` numbers per vertex (only x and y are
/// read), the outline first, then each hole from the vertex index in `hole_indices` (earcut's
/// convention: vertices, not values). The result is three vertex indices per triangle.
pub fn earcut(positions: &[f64], hole_indices: &[usize], dim: usize) -> Vec<usize> {
    if dim < 2 {
        return Vec::new();
    }
    let count = positions.len() / dim;
    let xy: Cow<[f64]> = if dim == 2 {
        Cow::Borrowed(&positions[..count * 2])
    } else {
        Cow::Owned((0..count).flat_map(|v| [positions[v * dim], positions[v * dim + 1]]).collect())
    };
    if xy.iter().any(|v| !v.is_finite()) {
        return Vec::new();
    }
    if hole_indices.first() == Some(&0)
        || hole_indices.windows(2).any(|w| w[0] >= w[1])
        || hole_indices.last().is_some_and(|&h| h >= count)
    {
        return Vec::new();
    }
    earcut_js(&xy, hole_indices, 2)
}

const NIL: usize = usize::MAX;

/// earcut's `Node`.
struct Node {
    /// The vertex's index into the data (vertex × dim).
    i: usize,
    x: f64,
    y: f64,
    prev: usize,
    next: usize,
    z: i32,
    prev_z: usize,
    next_z: usize,
    steiner: bool,
}

/// JavaScript's truthiness of a number.
fn truthy(x: f64) -> bool {
    x != 0.0 && !x.is_nan()
}

struct Ear<'a> {
    data: &'a [f64],
    dim: usize,
    nodes: Vec<Node>,
    triangles: Vec<usize>,
}

/// earcut.js's `earcut(data, holeIndices, dim)`: three vertex indices per triangle.
fn earcut_js(data: &[f64], hole_indices: &[usize], dim: usize) -> Vec<usize> {
    let dim = if dim == 0 { 2 } else { dim };
    let has_holes = !hole_indices.is_empty();
    let outer_len = if has_holes { (hole_indices[0] * dim).min(data.len()) } else { data.len() };
    let mut e = Ear { data, dim, nodes: Vec::with_capacity(data.len() / dim * 2), triangles: Vec::new() };
    let mut outer_node = e.linked_list(0, outer_len, true);
    if outer_node == NIL || e.nodes[outer_node].next == e.nodes[outer_node].prev {
        return e.triangles;
    }
    let (mut min_x, mut min_y, mut inv_size) = (0.0, 0.0, 0.0);
    if has_holes {
        outer_node = e.eliminate_holes(hole_indices, outer_node);
    }
    // if the shape is not too simple, a z-order curve hash is used later: the polygon's bbox
    if data.len() > 80 * dim {
        min_x = data[0];
        let mut max_x = data[0];
        min_y = data[1];
        let mut max_y = data[1];
        let mut i = dim;
        while i < outer_len {
            let (x, y) = (data[i], data[i + 1]);
            if x < min_x {
                min_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if x > max_x {
                max_x = x;
            }
            if y > max_y {
                max_y = y;
            }
            i += dim;
        }
        // min_x, min_y and inv_size turn coordinates into integers for the z-order
        inv_size = js_max(max_x - min_x, max_y - min_y);
        inv_size = if inv_size != 0.0 { 32767.0 / inv_size } else { 0.0 };
    }
    e.earcut_linked(outer_node, min_x, min_y, inv_size, 0);
    e.triangles
}

/// `signedArea(data, start, end, dim)`.
fn signed_area(data: &[f64], start: usize, end: usize, dim: usize) -> f64 {
    let mut sum = 0.0;
    let mut j = end - dim;
    let mut i = start;
    while i < end {
        sum += (data[j] - data[i]) * (data[i + 1] + data[j + 1]);
        j = i;
        i += dim;
    }
    sum
}

/// `pointInTriangle`: whether the point lies within the convex triangle.
#[allow(clippy::too_many_arguments)]
fn point_in_triangle(ax: f64, ay: f64, bx: f64, by: f64, cx: f64, cy: f64, px: f64, py: f64) -> bool {
    (cx - px) * (ay - py) >= (ax - px) * (cy - py)
        && (ax - px) * (by - py) >= (bx - px) * (ay - py)
        && (bx - px) * (cy - py) >= (cx - px) * (by - py)
}

/// `sign(num)`.
fn sign(num: f64) -> i8 {
    if num > 0.0 {
        1
    } else if num < 0.0 {
        -1
    } else {
        0
    }
}

/// `zOrder`: the z-order of a point from its coordinates and the inverse of the longer side
/// of the data's bbox.
fn z_order(x: f64, y: f64, min_x: f64, min_y: f64, inv_size: f64) -> i32 {
    // coordinates are taken into the non-negative 15-bit range
    let mut x = to_int32((x - min_x) * inv_size);
    let mut y = to_int32((y - min_y) * inv_size);
    x = (x | x.wrapping_shl(8)) & 0x00FF_00FF;
    x = (x | x.wrapping_shl(4)) & 0x0F0F_0F0F;
    x = (x | x.wrapping_shl(2)) & 0x3333_3333;
    x = (x | x.wrapping_shl(1)) & 0x5555_5555;
    y = (y | y.wrapping_shl(8)) & 0x00FF_00FF;
    y = (y | y.wrapping_shl(4)) & 0x0F0F_0F0F;
    y = (y | y.wrapping_shl(2)) & 0x3333_3333;
    y = (y | y.wrapping_shl(1)) & 0x5555_5555;
    x | y.wrapping_shl(1)
}

impl Ear<'_> {
    fn x(&self, p: usize) -> f64 {
        self.nodes[p].x
    }

    fn y(&self, p: usize) -> f64 {
        self.nodes[p].y
    }

    fn next(&self, p: usize) -> usize {
        self.nodes[p].next
    }

    fn prev(&self, p: usize) -> usize {
        self.nodes[p].prev
    }

    /// `area(p, q, r)`: the signed area of a triangle.
    fn area(&self, p: usize, q: usize, r: usize) -> f64 {
        let (p, q, r) = (&self.nodes[p], &self.nodes[q], &self.nodes[r]);
        (q.y - p.y) * (r.x - q.x) - (q.x - p.x) * (r.y - q.y)
    }

    /// `equals(p1, p2)`.
    fn equals(&self, p1: usize, p2: usize) -> bool {
        self.nodes[p1].x == self.nodes[p2].x && self.nodes[p1].y == self.nodes[p2].y
    }

    /// `insertNode`: a node, linked after `last` (a circular doubly linked list).
    fn insert_node(&mut self, i: usize, x: f64, y: f64, last: usize) -> usize {
        let p = self.new_node(i, x, y);
        if last == NIL {
            self.nodes[p].prev = p;
            self.nodes[p].next = p;
        } else {
            let last_next = self.nodes[last].next;
            self.nodes[p].next = last_next;
            self.nodes[p].prev = last;
            self.nodes[last_next].prev = p;
            self.nodes[last].next = p;
        }
        p
    }

    /// `new Node(i, x, y)`.
    fn new_node(&mut self, i: usize, x: f64, y: f64) -> usize {
        self.nodes.push(Node { i, x, y, prev: NIL, next: NIL, z: 0, prev_z: NIL, next_z: NIL, steiner: false });
        self.nodes.len() - 1
    }

    /// `removeNode`.
    fn remove_node(&mut self, p: usize) {
        let Node { prev, next, prev_z, next_z, .. } = self.nodes[p];
        self.nodes[next].prev = prev;
        self.nodes[prev].next = next;
        if prev_z != NIL {
            self.nodes[prev_z].next_z = next_z;
        }
        if next_z != NIL {
            self.nodes[next_z].prev_z = prev_z;
        }
    }

    /// `linkedList`: a circular doubly linked list of the points from `start` to `end`, in
    /// the winding asked for.
    fn linked_list(&mut self, start: usize, end: usize, clockwise: bool) -> usize {
        let (data, dim) = (self.data, self.dim);
        let mut last = NIL;
        if end <= start {
            return NIL;
        }
        if clockwise == (signed_area(data, start, end, dim) > 0.0) {
            let mut i = start;
            while i < end {
                last = self.insert_node(i, data[i], data[i + 1], last);
                i += dim;
            }
        } else {
            let mut i = end - dim;
            loop {
                last = self.insert_node(i, data[i], data[i + 1], last);
                if i < start + dim {
                    break;
                }
                i -= dim;
            }
        }
        if last != NIL && self.equals(last, self.next(last)) {
            self.remove_node(last);
            last = self.next(last);
        }
        last
    }

    /// `filterPoints`: removes collinear or duplicate points.
    fn filter_points(&mut self, start: usize, end: usize) -> usize {
        if start == NIL {
            return start;
        }
        let mut end = if end == NIL { start } else { end };
        let mut p = start;
        loop {
            let mut again = false;
            if !self.nodes[p].steiner && (self.equals(p, self.next(p)) || self.area(self.prev(p), p, self.next(p)) == 0.0) {
                self.remove_node(p);
                p = self.prev(p);
                end = p;
                if p == self.next(p) {
                    break;
                }
                again = true;
            } else {
                p = self.next(p);
            }
            if !(again || p != end) {
                break;
            }
        }
        end
    }

    /// `earcutLinked`: the main ear slicing loop.
    fn earcut_linked(&mut self, ear: usize, min_x: f64, min_y: f64, inv_size: f64, pass: u8) {
        if ear == NIL {
            return;
        }
        // interlink polygon nodes in z-order
        if pass == 0 && truthy(inv_size) {
            self.index_curve(ear, min_x, min_y, inv_size);
        }
        let mut ear = ear;
        let mut stop = ear;
        // iterate through ears, slicing them one by one
        while self.prev(ear) != self.next(ear) {
            let prev = self.prev(ear);
            let next = self.next(ear);
            let is_ear = if truthy(inv_size) { self.is_ear_hashed(ear, min_x, min_y, inv_size) } else { self.is_ear(ear) };
            if is_ear {
                // cut off the triangle
                let dim = self.dim;
                self.triangles.push(self.nodes[prev].i / dim);
                self.triangles.push(self.nodes[ear].i / dim);
                self.triangles.push(self.nodes[next].i / dim);
                self.remove_node(ear);
                // skipping the next vertex leads to fewer sliver triangles
                ear = self.next(next);
                stop = self.next(next);
                continue;
            }
            ear = next;
            // the whole remaining polygon looped through and no more ears found
            if ear == stop {
                if pass == 0 {
                    // try filtering points and slicing again
                    let filtered = self.filter_points(ear, NIL);
                    self.earcut_linked(filtered, min_x, min_y, inv_size, 1);
                } else if pass == 1 {
                    // if this didn't work, try curing all small self-intersections locally
                    let filtered = self.filter_points(ear, NIL);
                    let ear = self.cure_local_intersections(filtered);
                    self.earcut_linked(ear, min_x, min_y, inv_size, 2);
                } else if pass == 2 {
                    // as a last resort, try splitting the remaining polygon into two
                    self.split_earcut(ear, min_x, min_y, inv_size);
                }
                break;
            }
        }
    }

    /// `isEar`: whether a node makes a valid ear with its neighbours.
    fn is_ear(&self, ear: usize) -> bool {
        let (a, b, c) = (self.prev(ear), ear, self.next(ear));
        if self.area(a, b, c) >= 0.0 {
            return false; // reflex, can't be an ear
        }
        // no other point may lie inside the ear
        let (ax, bx, cx, ay, by, cy) = (self.x(a), self.x(b), self.x(c), self.y(a), self.y(b), self.y(c));
        let (x0, y0, x1, y1) = bbox(ax, bx, cx, ay, by, cy);
        let mut p = self.next(c);
        while p != a {
            let (px, py) = (self.x(p), self.y(p));
            if px >= x0
                && px <= x1
                && py >= y0
                && py <= y1
                && point_in_triangle(ax, ay, bx, by, cx, cy, px, py)
                && self.area(self.prev(p), p, self.next(p)) >= 0.0
            {
                return false;
            }
            p = self.next(p);
        }
        true
    }

    /// Whether `p` (not `a` or `c`) lies in the ear's triangle and is not reflex (the test of
    /// `isEarHashed`'s loops).
    #[allow(clippy::too_many_arguments)]
    fn blocks(&self, p: usize, a: usize, c: usize, [x0, y0, x1, y1]: [f64; 4], [ax, ay, bx, by, cx, cy]: [f64; 6]) -> bool {
        let (px, py) = (self.x(p), self.y(p));
        px >= x0
            && px <= x1
            && py >= y0
            && py <= y1
            && p != a
            && p != c
            && point_in_triangle(ax, ay, bx, by, cx, cy, px, py)
            && self.area(self.prev(p), p, self.next(p)) >= 0.0
    }

    /// `isEarHashed`.
    fn is_ear_hashed(&self, ear: usize, min_x: f64, min_y: f64, inv_size: f64) -> bool {
        let (a, b, c) = (self.prev(ear), ear, self.next(ear));
        if self.area(a, b, c) >= 0.0 {
            return false; // reflex, can't be an ear
        }
        let (ax, bx, cx, ay, by, cy) = (self.x(a), self.x(b), self.x(c), self.y(a), self.y(b), self.y(c));
        let (x0, y0, x1, y1) = bbox(ax, bx, cx, ay, by, cy);
        let rect = [x0, y0, x1, y1];
        let tri = [ax, ay, bx, by, cx, cy];
        // z-order range for the current triangle bbox
        let min_z = z_order(x0, y0, min_x, min_y, inv_size);
        let max_z = z_order(x1, y1, min_x, min_y, inv_size);
        let mut p = self.nodes[ear].prev_z;
        let mut n = self.nodes[ear].next_z;
        // look for points inside the triangle in both directions
        while p != NIL && self.nodes[p].z >= min_z && n != NIL && self.nodes[n].z <= max_z {
            if self.blocks(p, a, c, rect, tri) {
                return false;
            }
            p = self.nodes[p].prev_z;
            if self.blocks(n, a, c, rect, tri) {
                return false;
            }
            n = self.nodes[n].next_z;
        }
        // look for remaining points in decreasing z-order
        while p != NIL && self.nodes[p].z >= min_z {
            if self.blocks(p, a, c, rect, tri) {
                return false;
            }
            p = self.nodes[p].prev_z;
        }
        // look for remaining points in increasing z-order
        while n != NIL && self.nodes[n].z <= max_z {
            if self.blocks(n, a, c, rect, tri) {
                return false;
            }
            n = self.nodes[n].next_z;
        }
        true
    }

    /// `cureLocalIntersections`: cures small local self-intersections.
    fn cure_local_intersections(&mut self, start: usize) -> usize {
        let mut start = start;
        let mut p = start;
        loop {
            let a = self.prev(p);
            let b = self.next(self.next(p));
            if !self.equals(a, b) && self.intersects(a, p, self.next(p), b) && self.locally_inside(a, b) && self.locally_inside(b, a) {
                let dim = self.dim;
                self.triangles.push(self.nodes[a].i / dim);
                self.triangles.push(self.nodes[p].i / dim);
                self.triangles.push(self.nodes[b].i / dim);
                // remove the two nodes involved
                self.remove_node(p);
                self.remove_node(self.next(p));
                p = b;
                start = b;
            }
            p = self.next(p);
            if p == start {
                break;
            }
        }
        self.filter_points(p, NIL)
    }

    /// `splitEarcut`: splits the polygon in two along a valid diagonal and triangulates
    /// both halves.
    fn split_earcut(&mut self, start: usize, min_x: f64, min_y: f64, inv_size: f64) {
        let mut a = start;
        loop {
            let mut b = self.next(self.next(a));
            while b != self.prev(a) {
                if self.nodes[a].i != self.nodes[b].i && self.is_valid_diagonal(a, b) {
                    // split the polygon in two by the diagonal
                    let mut c = self.split_polygon(a, b);
                    // filter collinear points around the cuts
                    a = self.filter_points(a, self.next(a));
                    c = self.filter_points(c, self.next(c));
                    // run earcut on each half
                    self.earcut_linked(a, min_x, min_y, inv_size, 0);
                    self.earcut_linked(c, min_x, min_y, inv_size, 0);
                    return;
                }
                b = self.next(b);
            }
            a = self.next(a);
            if a == start {
                break;
            }
        }
    }

    /// `eliminateHoles`: links every hole into the outer ring, leaving one ring.
    fn eliminate_holes(&mut self, hole_indices: &[usize], outer_node: usize) -> usize {
        let (data, dim) = (self.data, self.dim);
        let mut queue = Vec::with_capacity(hole_indices.len());
        for (k, &h) in hole_indices.iter().enumerate() {
            let start = (h * dim).min(data.len());
            let end = if k < hole_indices.len() - 1 { (hole_indices[k + 1] * dim).min(data.len()) } else { data.len() };
            let list = self.linked_list(start, end, false);
            if list == NIL {
                continue; // earcut.js throws; [`earcut`] lets no empty hole through
            }
            if list == self.next(list) {
                self.nodes[list].steiner = true;
            }
            queue.push(self.get_leftmost(list));
        }
        // `queue.sort(compareX)`: a stable sort by x
        queue.sort_by(|&a, &b| self.x(a).partial_cmp(&self.x(b)).unwrap_or(std::cmp::Ordering::Equal));
        // process holes from left to right
        let mut outer_node = outer_node;
        for hole in queue {
            outer_node = self.eliminate_hole(hole, outer_node);
        }
        outer_node
    }

    /// `eliminateHole`: bridges a hole to the outer ring and links it in.
    fn eliminate_hole(&mut self, hole: usize, outer_node: usize) -> usize {
        let bridge = self.find_hole_bridge(hole, outer_node);
        if bridge == NIL {
            return outer_node;
        }
        let bridge_reverse = self.split_polygon(bridge, hole);
        // filter collinear points around the cuts
        self.filter_points(bridge_reverse, self.next(bridge_reverse));
        self.filter_points(bridge, self.next(bridge))
    }

    /// `findHoleBridge`: David Eberly's algorithm for a bridge between a hole and the outer
    /// ring.
    fn find_hole_bridge(&self, hole: usize, outer_node: usize) -> usize {
        let mut p = outer_node;
        let (hx, hy) = (self.x(hole), self.y(hole));
        let mut qx = f64::NEG_INFINITY;
        let mut m = NIL;
        // a segment crossed by a ray from the hole's leftmost point to the left; the
        // segment's end with the lesser x is a possible connection
        loop {
            let next = self.next(p);
            if hy <= self.y(p) && hy >= self.y(next) && self.y(next) != self.y(p) {
                let x = self.x(p) + (hy - self.y(p)) * (self.x(next) - self.x(p)) / (self.y(next) - self.y(p));
                if x <= hx && x > qx {
                    qx = x;
                    m = if self.x(p) < self.x(next) { p } else { next };
                    if x == hx {
                        return m; // the hole touches the outer segment; pick its leftmost end
                    }
                }
            }
            p = next;
            if p == outer_node {
                break;
            }
        }
        if m == NIL {
            return NIL;
        }
        // look for points inside the triangle of the hole's point, the crossing and the
        // end; with none, the connection is valid; else take the point of the least angle
        // with the ray
        let stop = m;
        let (mx, my) = (self.x(m), self.y(m));
        let mut tan_min = f64::INFINITY;
        p = m;
        loop {
            let (px, py) = (self.x(p), self.y(p));
            if hx >= px
                && px >= mx
                && hx != px
                && point_in_triangle(if hy < my { hx } else { qx }, hy, mx, my, if hy < my { qx } else { hx }, hy, px, py)
            {
                let tan = (hy - py).abs() / (hx - px); // tangential
                if self.locally_inside(p, hole)
                    && (tan < tan_min
                        || (tan == tan_min && (px > self.x(m) || (px == self.x(m) && self.sector_contains_sector(m, p)))))
                {
                    m = p;
                    tan_min = tan;
                }
            }
            p = self.next(p);
            if p == stop {
                break;
            }
        }
        m
    }

    /// `sectorContainsSector`: whether the sector at `m` contains the sector at `p`.
    fn sector_contains_sector(&self, m: usize, p: usize) -> bool {
        self.area(self.prev(m), m, self.prev(p)) < 0.0 && self.area(self.next(p), m, self.next(m)) < 0.0
    }

    /// `indexCurve`: interlinks the nodes in z-order.
    fn index_curve(&mut self, start: usize, min_x: f64, min_y: f64, inv_size: f64) {
        let mut p = start;
        loop {
            if self.nodes[p].z == 0 {
                self.nodes[p].z = z_order(self.x(p), self.y(p), min_x, min_y, inv_size);
            }
            self.nodes[p].prev_z = self.prev(p);
            self.nodes[p].next_z = self.next(p);
            p = self.next(p);
            if p == start {
                break;
            }
        }
        let last = self.nodes[p].prev_z;
        self.nodes[last].next_z = NIL;
        self.nodes[p].prev_z = NIL;
        self.sort_linked(p);
    }

    /// `sortLinked`: Simon Tatham's linked-list merge sort, on z.
    fn sort_linked(&mut self, list: usize) -> usize {
        let mut list = list;
        let mut in_size = 1;
        loop {
            let mut p = list;
            list = NIL;
            let mut tail = NIL;
            let mut num_merges = 0;
            while p != NIL {
                num_merges += 1;
                let mut q = p;
                let mut p_size = 0;
                for _ in 0..in_size {
                    p_size += 1;
                    q = self.nodes[q].next_z;
                    if q == NIL {
                        break;
                    }
                }
                let mut q_size = in_size;
                while p_size > 0 || (q_size > 0 && q != NIL) {
                    let e;
                    if p_size != 0 && (q_size == 0 || q == NIL || self.nodes[p].z <= self.nodes[q].z) {
                        e = p;
                        p = self.nodes[p].next_z;
                        p_size -= 1;
                    } else {
                        e = q;
                        q = self.nodes[q].next_z;
                        q_size -= 1;
                    }
                    if tail != NIL {
                        self.nodes[tail].next_z = e;
                    } else {
                        list = e;
                    }
                    self.nodes[e].prev_z = tail;
                    tail = e;
                }
                p = q;
            }
            self.nodes[tail].next_z = NIL;
            in_size *= 2;
            if num_merges <= 1 {
                break;
            }
        }
        list
    }

    /// `getLeftmost`: a ring's leftmost node (the lowest of those).
    fn get_leftmost(&self, start: usize) -> usize {
        let mut p = start;
        let mut leftmost = start;
        loop {
            if self.x(p) < self.x(leftmost) || (self.x(p) == self.x(leftmost) && self.y(p) < self.y(leftmost)) {
                leftmost = p;
            }
            p = self.next(p);
            if p == start {
                break;
            }
        }
        leftmost
    }

    /// `isValidDiagonal`: whether the diagonal between two nodes lies inside the polygon.
    fn is_valid_diagonal(&self, a: usize, b: usize) -> bool {
        let bi = self.nodes[b].i;
        self.nodes[self.next(a)].i != bi
            && self.nodes[self.prev(a)].i != bi
            && !self.intersects_polygon(a, b) // crosses no other edge
            && ((self.locally_inside(a, b) && self.locally_inside(b, a) && self.middle_inside(a, b) // locally visible
                && (truthy(self.area(self.prev(a), a, self.prev(b))) || truthy(self.area(a, self.prev(b), b)))) // no opposite-facing sectors
                || (self.equals(a, b) && self.area(self.prev(a), a, self.next(a)) > 0.0 && self.area(self.prev(b), b, self.next(b)) > 0.0)) // the zero-length case
    }

    /// `intersects(p1, q1, p2, q2)`: whether two segments cross.
    fn intersects(&self, p1: usize, q1: usize, p2: usize, q2: usize) -> bool {
        let o1 = sign(self.area(p1, q1, p2));
        let o2 = sign(self.area(p1, q1, q2));
        let o3 = sign(self.area(p2, q2, p1));
        let o4 = sign(self.area(p2, q2, q1));
        if o1 != o2 && o3 != o4 {
            return true; // the general case
        }
        // collinear, and an end on the other segment
        (o1 == 0 && self.on_segment(p1, p2, q1))
            || (o2 == 0 && self.on_segment(p1, q2, q1))
            || (o3 == 0 && self.on_segment(p2, p1, q2))
            || (o4 == 0 && self.on_segment(p2, q1, q2))
    }

    /// `onSegment(p, q, r)`: for collinear points, whether `q` lies on the segment `pr`.
    fn on_segment(&self, p: usize, q: usize, r: usize) -> bool {
        let (p, q, r) = (&self.nodes[p], &self.nodes[q], &self.nodes[r]);
        q.x <= js_max(p.x, r.x) && q.x >= js_min(p.x, r.x) && q.y <= js_max(p.y, r.y) && q.y >= js_min(p.y, r.y)
    }

    /// `intersectsPolygon`: whether a diagonal crosses any of the polygon's edges.
    fn intersects_polygon(&self, a: usize, b: usize) -> bool {
        let (ai, bi) = (self.nodes[a].i, self.nodes[b].i);
        let mut p = a;
        loop {
            let next = self.next(p);
            let (pi, ni) = (self.nodes[p].i, self.nodes[next].i);
            if pi != ai && ni != ai && pi != bi && ni != bi && self.intersects(p, next, a, b) {
                return true;
            }
            p = next;
            if p == a {
                break;
            }
        }
        false
    }

    /// `locallyInside`: whether a diagonal is locally inside the polygon.
    fn locally_inside(&self, a: usize, b: usize) -> bool {
        if self.area(self.prev(a), a, self.next(a)) < 0.0 {
            self.area(a, b, self.next(a)) >= 0.0 && self.area(a, self.prev(a), b) >= 0.0
        } else {
            self.area(a, b, self.prev(a)) < 0.0 || self.area(a, self.next(a), b) < 0.0
        }
    }

    /// `middleInside`: whether the middle of a diagonal is inside the polygon.
    fn middle_inside(&self, a: usize, b: usize) -> bool {
        let mut p = a;
        let mut inside = false;
        let px = (self.x(a) + self.x(b)) / 2.0;
        let py = (self.y(a) + self.y(b)) / 2.0;
        loop {
            let next = self.next(p);
            let (x, y, nx, ny) = (self.x(p), self.y(p), self.x(next), self.y(next));
            if (y > py) != (ny > py) && ny != y && px < (nx - x) * (py - y) / (ny - y) + x {
                inside = !inside;
            }
            p = next;
            if p == a {
                break;
            }
        }
        inside
    }

    /// `splitPolygon`: links two vertices by a bridge; for two of one ring it splits the
    /// polygon in two, for an outer one and a hole's it merges them into one ring.
    fn split_polygon(&mut self, a: usize, b: usize) -> usize {
        let (ai, ax, ay) = (self.nodes[a].i, self.x(a), self.y(a));
        let (bi, bx, by) = (self.nodes[b].i, self.x(b), self.y(b));
        let a2 = self.new_node(ai, ax, ay);
        let b2 = self.new_node(bi, bx, by);
        let an = self.next(a);
        let bp = self.prev(b);
        self.nodes[a].next = b;
        self.nodes[b].prev = a;
        self.nodes[a2].next = an;
        self.nodes[an].prev = a2;
        self.nodes[b2].next = a2;
        self.nodes[a2].prev = b2;
        self.nodes[bp].next = b2;
        self.nodes[b2].prev = bp;
        b2
    }
}

/// A triangle's bbox, `(x0, y0, x1, y1)`, computed as earcut does.
fn bbox(ax: f64, bx: f64, cx: f64, ay: f64, by: f64, cy: f64) -> (f64, f64, f64, f64) {
    let x0 = if ax < bx { if ax < cx { ax } else { cx } } else if bx < cx { bx } else { cx };
    let y0 = if ay < by { if ay < cy { ay } else { cy } } else if by < cy { by } else { cy };
    let x1 = if ax > bx { if ax > cx { ax } else { cx } } else if bx > cx { bx } else { cx };
    let y1 = if ay > by { if ay > cy { ay } else { cy } } else if by > cy { by } else { cy };
    (x0, y0, x1, y1)
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
        // earcut.js 2.2.4's triangles (and math.gl 4.1's, the same), run with bun
        let mut ring = vec![0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        ring.extend([2.0, 2.0, 2.0, 4.0, 4.0, 4.0, 4.0, 2.0]);
        let t = earcut(&ring, &[4], 2);
        assert_eq!(t, [0, 4, 5, 7, 4, 0, 1, 2, 3, 3, 0, 5, 7, 0, 1, 3, 5, 6, 6, 7, 1, 1, 3, 6]);
        assert_eq!(area(&ring, 2, &t), 96.0);
        // the outline clockwise and the hole anticlockwise: the same area
        let mut reversed = vec![0.0, 10.0, 10.0, 10.0, 10.0, 0.0, 0.0, 0.0];
        reversed.extend([4.0, 2.0, 4.0, 4.0, 2.0, 4.0, 2.0, 2.0]);
        let t = earcut(&reversed, &[4], 2);
        assert_eq!(t, [3, 7, 6, 4, 7, 3, 2, 1, 0, 0, 3, 6, 4, 3, 2, 0, 6, 5, 5, 4, 2, 2, 0, 5]);
        assert_eq!(area(&reversed, 2, &t), 96.0);
    }

    #[test]
    fn three_numbers_a_vertex_reads_x_and_y() {
        let square = [0.0, 0.0, 5.0, 4.0, 0.0, 6.0, 4.0, 4.0, 7.0, 0.0, 4.0, 8.0];
        let t = earcut(&square, &[], 3);
        // earcut.js with dim 3
        assert_eq!(t, [2, 3, 0, 0, 1, 2]);
        assert_eq!(area(&square, 3, &t), 16.0);
    }

    #[test]
    fn the_same_triangles_as_math_gl_without_holes() {
        // math.gl 4.1's earcut (and earcut.js 2.2.4, the same) on the same input, run with bun
        let square = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        assert_eq!(earcut(&square, &[], 2), [2, 3, 0, 0, 1, 2]);
        let quad = [0.0, 0.0, 4.0, 0.0, 4.0, 3.0, 1.0, 2.0];
        assert_eq!(earcut(&quad, &[], 2), [2, 3, 0, 0, 1, 2]);
        let l = [0.0, 0.0, 10.0, 0.0, 10.0, 4.0, 4.0, 4.0, 4.0, 10.0, 0.0, 10.0];
        assert_eq!(earcut(&l, &[], 2), [4, 5, 0, 0, 1, 2, 3, 4, 0, 0, 2, 3]);
    }

    #[test]
    fn with_a_hole_the_same_triangles_as_math_gl() {
        // math.gl 4.1's earcut (and earcut.js 2.2.4) on the same input, run with bun
        let want = [0, 4, 5, 7, 4, 0, 3, 0, 5, 7, 0, 1, 2, 3, 5, 6, 7, 1, 2, 5, 6, 6, 1, 2];
        let holed = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0, 3.0, 3.0, 3.0, 6.0, 6.0, 6.0, 6.0, 3.0];
        let t = earcut(&holed, &[4], 2);
        assert_eq!(t, want);
        assert_eq!(area(&holed, 2, &t), 91.0);
        let holed = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0, 2.0, 2.0, 2.0, 8.0, 8.0, 8.0, 8.0, 2.0];
        let t = earcut(&holed, &[4], 2);
        assert_eq!(t, want);
        assert_eq!(area(&holed, 2, &t), 64.0);
    }

    /// The sum `h = h·31 + index` (32 bits) of a triangle list, as `tess::vector`'s tests hash
    /// deck's.
    fn hash(triangles: &[usize]) -> u32 {
        triangles.iter().fold(0u32, |h, &i| h.wrapping_mul(31).wrapping_add(i as u32))
    }

    /// A comb: a bottom edge `2n` long under a zigzag of `n + 1` teeth; with holes, `n / 4`
    /// squares, a triangle standing on the bottom edge and one hanging from a tooth's tip.
    fn comb(n: usize, holes: bool) -> (Vec<f64>, Vec<usize>) {
        let mut p = vec![0.0, 0.0, 2.0 * n as f64, 0.0];
        for k in (0..=n).rev() {
            p.extend([2.0 * k as f64, 10.0 + ((k * 7) % 5) as f64]);
        }
        let mut h = Vec::new();
        if holes {
            for j in 0..n / 4 {
                let x = 4.0 * j as f64;
                h.push(p.len() / 2);
                p.extend([x + 1.0, 2.0, x + 1.0, 4.0, x + 3.0, 4.0, x + 3.0, 2.0]);
            }
            h.push(p.len() / 2);
            p.extend([10.0, 0.0, 11.0, 1.5, 12.0, 0.0]);
            h.push(p.len() / 2);
            p.extend([14.0, 10.0 + ((7 * 7) % 5) as f64, 13.5, 6.0, 14.5, 6.0]);
        }
        (p, h)
    }

    /// earcut hashes in z-order above 80 vertices, holes included (`earcutr` from 40, which
    /// changed the triangles): below and above that, as earcut.js does (length and hash of its
    /// triangles, run with bun; math.gl 4.1 gives the same), with two numbers a vertex or three.
    #[test]
    fn z_order_hashing_from_81_vertices_as_earcut_js_does() {
        for (n, holes, vertices, len, h) in
            [(30, true, 67, 183, 1371182959), (100, true, 209, 567, 1026730630), (100, false, 103, 303, 4090893373)]
        {
            let (p, holes) = comb(n, holes);
            assert_eq!(p.len() / 2, vertices);
            let t = earcut(&p, &holes, 2);
            assert_eq!((t.len(), hash(&t)), (len, h), "comb {n}");
            let p3: Vec<f64> = p.chunks(2).enumerate().flat_map(|(i, v)| [v[0], v[1], i as f64]).collect();
            assert_eq!(earcut(&p3, &holes, 3), t, "comb {n}, three numbers a vertex");
        }
    }

    #[test]
    fn nothing_for_what_cannot_be_triangulated() {
        // too few points, all in a line
        assert!(earcut(&[], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 1.0], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 0.0, 2.0, 0.0], &[], 2).is_empty());
        // what earcut.js makes garbage of or throws on
        assert!(earcut(&[0.0, 0.0, 1.0, 0.0, f64::NAN, 1.0], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 0.0, 1.0, f64::INFINITY], &[], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0], &[], 1).is_empty());
        let square = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        assert!(earcut(&square, &[0], 2).is_empty());
        assert!(earcut(&square, &[3, 2], 2).is_empty());
        assert!(earcut(&square, &[5], 2).is_empty());
        // an empty hole, at the end or between two (earcut.js throws)
        assert!(earcut(&square, &[4], 2).is_empty());
        assert!(earcut(&[0.0, 0.0, 10.0, 0.0, 10.0, 10.0], &[3], 2).is_empty());
        let mut ring = square.to_vec();
        ring.extend([2.0, 2.0, 2.0, 4.0, 4.0, 4.0, 4.0, 2.0]);
        assert!(earcut(&ring, &[4, 4], 2).is_empty());
        // a z that is not finite is not read
        assert_eq!(earcut(&[0.0, 0.0, f64::NAN, 10.0, 0.0, f64::NAN, 10.0, 10.0, f64::NAN], &[], 3).len(), 3);
    }

    /// Finite, if huge: as earcut.js does (`earcutr` could not start a ring at `±f64::MAX`).
    #[test]
    fn huge_positions_as_earcut_js_does() {
        assert_eq!(earcut(&[f64::MAX, 0.0, 1.0, 0.0, 1.0, 1.0], &[], 2), [1, 0, 2]);
        assert_eq!(earcut(&[-f64::MAX, 0.0, 1.0, 0.0, 1.0, 1.0], &[], 2), [1, 2, 0]);
    }

    /// Two parts of the 5° cut of the z0 tile's water whose holes (land) touch the part's
    /// outline along a grid line: earcut.js 2.2.4's triangles, exactly (`earcutr` 0.5 covered
    /// too much of both).
    #[test]
    fn holes_touching_the_outline_triangulate_as_earcut_js_does() {
        let cell = [
            -15.0, 75.0, -20.0, 75.0, -20.0, 80.0, -15.0, 80.0, -20.0, 80.0, -20.0, 75.21898243165758, -19.599609375,
            75.25305660483545, -19.86328125, 76.10079606754579, -20.0, 76.1391741748791, -20.0, 76.94146766973962,
            -18.45703125, 76.98014914976216, -19.6875, 77.63654163009747, -19.6875, 78.75065903068668, -17.940747155567326, 80.0,
        ];
        assert_eq!(
            earcut(&cell, &[4], 2),
            [0, 3, 13, 13, 12, 11, 10, 9, 8, 6, 5, 1, 13, 11, 10, 10, 8, 7, 6, 1, 0, 0, 13, 10, 10, 7, 6, 6, 0, 10]
        );
        let cell = [
            -25.0, 80.0, -30.0, 80.0, -30.0, 85.0, -25.0, 85.0, -27.0703125, 83.52016238353204, -30.0, 83.56692972324694, -30.0,
            80.0, -25.0, 80.0, -25.0, 81.8008335117073, -27.861328125, 82.13042660886293, -30.0, 82.06408871700589, -30.0,
            82.23156156448887, -25.0, 82.3083878077587, -25.0, 83.25870547036925, -27.0703125, 83.52016238353204,
        ];
        assert_eq!(earcut(&cell, &[4], 2), [3, 2, 5, 12, 11, 10, 3, 5, 14, 12, 10, 9, 3, 14, 13, 12, 9, 8]);
    }
}
