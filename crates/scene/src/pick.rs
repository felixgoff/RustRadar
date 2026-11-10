//! CPU picking (doc 03 §10.1, doc 00 §1.4.11): what is under the pointer, with no GPU read-back.
//!
//! deck picks by colour. Every pickable layer is drawn again into an off-screen buffer, each object
//! in its own colour, under its layer's depth test. `getClosestObject`
//! (`@deck.gl/core/dist/lib/picking/query-object.js`) then takes, of the pixels within
//! `pickingRadius` of the pointer that hold an object, the closest one, and returns whatever was
//! drawn on top at that pixel. The app sets `pickingRadius: 6` (G:3695, [`PICK_RADIUS_PX`]).
//!
//! Here each scene refresh records the pickable things **in draw order** in a [`PickList`], and
//! [`PickList::pick`] runs deck's search over their shapes instead of over a buffer:
//! - the same device pixels: luma.gl's `cssToDevicePixels` for the pointer, deck's
//!   `_getPickingRect` clipped to the canvas, and `round(6 · pixel ratio)` device pixels of radius;
//! - the same scan and the same tie rule: the closest pixel wins; of pixels equally close, the
//!   last one deck scans wins, which is the upper one on the screen, then the right one;
//! - at that pixel, the item on top is the one a depth buffer leaves there: items are drawn in
//!   list order, each under its layer's [`DepthTest`].
//!
//! So "topmost wins" holds at a pixel: with the pointer over two overlapping items, the later
//! drawn wins (or the nearer, where depth is tested). It does not hold across pixels: a pointer
//! over one item picks that one, even when a later-drawn item is within 6 px of it.
//!
//! Each item has a [`Shape`], its footprint as drawn. The 6 px of reach come on top of it, as in
//! deck:
//! - [`Shape::Point`]: a disc on the screen around a projected anchor. Used for icons, signals and
//!   far meshes. Its radius is the drawn footprint (for an icon, about half its drawn size), so an
//!   icon reaches 6 px past its edge. Doc 03 §10.1 sketches the test as the anchor within
//!   `max(screenPx/2, 6)` px; this keeps deck's reach instead. A radius of 0 covers a pixel only
//!   when the point sits exactly on its centre.
//! - [`Shape::Rect`]: a screen rectangle, for labels (the rectangle [`crate::declutter::claim`]
//!   gives, or the text's own box).
//! - [`Shape::Box`]: an [`OrientedBox`] in common space, for close meshes (aircraft at zoom ≥ 11,
//!   ship models, train cars). Each pixel's ray is tested against it, so it picks where the box
//!   shows on the screen, sides and top included, at any pitch.
//!
//! Depth is deck's: the third component of `Viewport::project` (NDC z). For a box it is taken
//! where the pixel's ray first enters the box.
//!
//! Where this differs from deck:
//! - shapes stand in for silhouettes: deck picks an icon only where its texture's alpha passes the
//!   cutoff, and a mesh only where its triangles are;
//! - depth is compared in f64, where deck's picking buffer has 16 bits (`depth16unorm`);
//! - points and rectangles are not clipped by the near and far planes, so the builders leave out
//!   what is not drawn (a box is clipped, as its ray runs from the near plane to the far plane);
//! - deck's multi-depth picking (`depth > 1`, `pickMultipleObjects`) and its `pickable: "3d"`
//!   pass are not ported: the app uses neither.

use glam::{DMat4, DVec3, DVec4};
use rustradar_core::num::js_round;

use crate::camera::ViewportBase;
use crate::declutter::Rect;

/// deck's `pickingRadius` as the app sets it (G:3695): an object within 6 CSS pixels of the pointer
/// is picked.
pub const PICK_RADIUS_PX: f64 = 6.0;

/// How a pickable layer meets the depth buffer (its deck `parameters`). This decides which item is
/// on top at a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthTest {
    /// `depthCompare: "always"` with no depth write (`LABEL_PARAMETERS`, `THROUGH_GROUND`, the
    /// signals' `top`). The item is drawn over everything before it.
    Always,
    /// `less-equal` with no depth write (`SHIP_PARAMETERS`, `TRAIN_PARAMETERS`). The item is
    /// hidden by nearer items drawn before it that wrote depth, and hides nothing drawn after it.
    LessEqual,
    /// deck's default, `less-equal` with depth write (aircraft icons and meshes, ship models, train
    /// cars). The nearer of two such items is on top; at equal depth, the later one.
    LessEqualWrite,
}

/// A pickable item's footprint as drawn. Screen positions are CSS pixels from the top left, y down,
/// as `Viewport::project` gives them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A disc on the screen: `at` is `[x, y, depth]` as `Viewport::project` gives it, and `radius`
    /// is in CSS pixels.
    Point { at: [f64; 3], radius: f64 },
    /// A rectangle on the screen, drawn at one depth.
    Rect { rect: Rect, depth: f64 },
    /// A box in common space, tested against each pixel's ray.
    Box(OrientedBox),
}

/// A box in the viewport's common space (deck's world space: the globe's sphere of 256 units, or
/// the flat map's 512 units round the world). It need not be lined up with the axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrientedBox {
    /// The centre, in common space.
    pub center: DVec3,
    /// Three orthogonal unit vectors along the box's edges.
    pub axes: [DVec3; 3],
    /// Half the box's extent along each axis, in common units, each ≥ 0. A flat box, with one
    /// half extent of 0, is a rectangle in space (a ground-plane icon).
    pub half: [f64; 3],
}

impl OrientedBox {
    /// The box a mesh instance fills. `model` takes model space to common space (the f64 instance
    /// matrix the builder makes, with rotation and scale only, so its columns are orthogonal), and
    /// `min`/`max` are the mesh's bounds in model space.
    pub fn from_matrix(model: &DMat4, min: DVec3, max: DVec3) -> Self {
        let center = model.transform_point3((min + max) * 0.5);
        let size = (max - min) * 0.5;
        let columns = [model.x_axis.truncate(), model.y_axis.truncate(), model.z_axis.truncate()];
        let mut axes = [DVec3::ZERO; 3];
        let mut half = [0.0; 3];
        for i in 0..3 {
            axes[i] = columns[i].normalize_or_zero();
            half[i] = columns[i].length() * size[i].abs();
        }
        // a model squashed flat along one axis still faces that way
        for i in 0..3 {
            if axes[i] == DVec3::ZERO {
                axes[i] = axes[(i + 1) % 3].cross(axes[(i + 2) % 3]).normalize_or_zero();
            }
        }
        OrientedBox { center, axes, half }
    }

    /// The eight corners. Corner `k` goes to `+half` along axis `i` where bit `i` of `k` is set,
    /// else to `-half`.
    pub fn corners(&self) -> [DVec3; 8] {
        let mut out = [DVec3::ZERO; 8];
        for (k, corner) in out.iter_mut().enumerate() {
            *corner = self.center;
            for i in 0..3 {
                let sign = if (k >> i) & 1 == 1 { 1.0 } else { -1.0 };
                *corner += self.axes[i] * (sign * self.half[i]);
            }
        }
        out
    }

    /// Where the segment `from + t · along`, `t` in `[0, 1]`, first meets the box (`t`, 0 when
    /// `from` is inside it), or `None` when it misses (the slab test).
    pub fn segment_hit(&self, from: DVec3, along: DVec3) -> Option<f64> {
        let finite = self.half.iter().all(|h| h.is_finite());
        if !finite || !self.center.is_finite() || !from.is_finite() || !along.is_finite() {
            return None;
        }
        // a box without a face direction covers nothing
        if self.axes.contains(&DVec3::ZERO) {
            return None;
        }
        let to_center = self.center - from;
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        for i in 0..3 {
            let e = self.axes[i].dot(to_center);
            let f = self.axes[i].dot(along);
            let h = self.half[i];
            if f != 0.0 {
                let (a, b) = ((e - h) / f, (e + h) / f);
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
            } else if e.abs() > h {
                // parallel to this pair of faces and outside them
                return None;
            }
        }
        (t0 <= t1).then_some(t0)
    }

    /// The box's bounds on the screen, `[x0, y0, x1, y1]` CSS pixels, or `None` when some corner is
    /// not in front of the camera (the box may then cover any pixel).
    fn screen_bounds(&self, viewport: &ViewportBase) -> Option<[f64; 4]> {
        let mut bounds = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for corner in self.corners() {
            let p = viewport.pixel_projection_matrix * DVec4::new(corner.x, corner.y, corner.z, 1.0);
            // in front of the eye (NaN is not)
            if p.w.is_nan() || p.w <= 0.0 {
                return None;
            }
            let (x, y) = (p.x / p.w, p.y / p.w);
            bounds = [bounds[0].min(x), bounds[1].min(y), bounds[2].max(x), bounds[3].max(y)];
        }
        Some(bounds)
    }
}

/// One pickable thing.
#[derive(Clone, Debug, PartialEq)]
pub struct Item<K> {
    /// What the app gets back when this is picked (a flight id, a ship's index, a label feature).
    pub key: K,
    pub shape: Shape,
    pub depth_test: DepthTest,
}

impl<K> Item<K> {
    /// The same item under another key: for gathering the builders' lists into the app's one.
    pub fn map_key<J>(self, f: impl FnOnce(K) -> J) -> Item<J> {
        Item { key: f(self.key), shape: self.shape, depth_test: self.depth_test }
    }
}

/// What [`PickList::pick`] found.
#[derive(Debug, PartialEq)]
pub struct Hit<'a, K> {
    /// The item's place in the list.
    pub index: usize,
    pub key: &'a K,
    /// From the pointer's pixel to the picked pixel, CSS pixels: 0 when the pointer is over the item.
    pub distance_px: f64,
}

impl<K> Clone for Hit<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Hit<'_, K> {}

/// The pickable things of one scene refresh, in draw order (the layer order of doc 03 §3, and
/// within a layer the order of its instances).
#[derive(Clone, Debug, PartialEq)]
pub struct PickList<K> {
    items: Vec<Item<K>>,
}

impl<K> Default for PickList<K> {
    fn default() -> Self {
        PickList { items: Vec::new() }
    }
}

impl<K> Extend<Item<K>> for PickList<K> {
    fn extend<I: IntoIterator<Item = Item<K>>>(&mut self, iter: I) {
        self.items.extend(iter);
    }
}

impl<K> FromIterator<Item<K>> for PickList<K> {
    fn from_iter<I: IntoIterator<Item = Item<K>>>(iter: I) -> Self {
        PickList { items: iter.into_iter().collect() }
    }
}

impl<K> IntoIterator for PickList<K> {
    type Item = Item<K>;
    type IntoIter = std::vec::IntoIter<Item<K>>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<K> PickList<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[Item<K>] {
        &self.items
    }

    /// Adds an item on top of those already in the list.
    pub fn push(&mut self, key: K, shape: Shape, depth_test: DepthTest) {
        self.items.push(Item { key, shape, depth_test });
    }

    /// Adds a disc: `at` is `[x, y, depth]` from `Viewport::project`, `radius` its footprint in CSS
    /// pixels.
    pub fn point(&mut self, key: K, at: [f64; 3], radius: f64, depth_test: DepthTest) {
        self.push(key, Shape::Point { at, radius }, depth_test);
    }

    /// Adds a screen rectangle drawn at `depth`.
    pub fn rect(&mut self, key: K, rect: Rect, depth: f64, depth_test: DepthTest) {
        self.push(key, Shape::Rect { rect, depth }, depth_test);
    }

    /// Adds a box in common space.
    pub fn oriented_box(&mut self, key: K, b: OrientedBox, depth_test: DepthTest) {
        self.push(key, Shape::Box(b), depth_test);
    }

    /// What deck would pick at `pointer` (CSS pixels from the canvas's top left) with the app's
    /// radius of [`PICK_RADIUS_PX`]. `pixel_ratio` is the map's resolution scale (`map_scale`,
    /// deck's `useDevicePixels`).
    pub fn pick(&self, viewport: &ViewportBase, pointer: [f64; 2], pixel_ratio: f64) -> Option<Hit<'_, K>> {
        self.pick_within(viewport, pointer, pixel_ratio, PICK_RADIUS_PX)
    }

    /// [`PickList::pick`] with another radius, CSS pixels (deck's `pickObject({ radius })`).
    pub fn pick_within(
        &self,
        viewport: &ViewportBase,
        pointer: [f64; 2],
        pixel_ratio: f64,
        radius: f64,
    ) -> Option<Hit<'_, K>> {
        let grid = Grid::new(viewport, pointer, pixel_ratio, radius)?;
        let reach = grid.reach();
        let candidates: Vec<usize> = (0..self.items.len())
            .filter(|&i| match bounds(&self.items[i].shape, viewport) {
                Some([x0, y0, x1, y1]) => x1 >= reach[0] && x0 <= reach[2] && y1 >= reach[1] && y0 <= reach[3],
                None => true,
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let has_box = candidates.iter().any(|&i| matches!(self.items[i].shape, Shape::Box(_)));

        // getClosestObject: rows from the bottom of the canvas up (WebGL's rows), each from the left;
        // a pixel at least as close as the best so far takes its place
        let mut min = grid.radius * grid.radius;
        let mut best = None;
        for row in grid.y..grid.y + grid.height {
            let dy = row - grid.device_y;
            let dy2 = dy * dy;
            if dy2 > min {
                continue;
            }
            for col in grid.x..grid.x + grid.width {
                let dx = col - grid.device_x;
                let d2 = dx * dx + dy2;
                if d2 > min {
                    continue;
                }
                let at = grid.pixel_center(col, row);
                let ray = has_box.then(|| {
                    let near = viewport.pixels_to_world_with_depth([at[0], at[1], -1.0]);
                    let far = viewport.pixels_to_world_with_depth([at[0], at[1], 1.0]);
                    (near, far - near)
                });
                if let Some(top) = self.top_at(&candidates, viewport, at, ray) {
                    min = d2;
                    best = Some((top, d2));
                }
            }
        }
        best.map(|(index, d2)| Hit { index, key: &self.items[index].key, distance_px: (d2 as f64).sqrt() / grid.ratio })
    }

    /// The item a depth buffer leaves on top at the pixel centred on `at` (CSS pixels), drawing the
    /// candidates in list order; `None` when none covers it. `ray` is the pixel's segment from the
    /// near plane to the far plane, there when a candidate is a box.
    fn top_at(&self, candidates: &[usize], viewport: &ViewportBase, at: [f64; 2], ray: Option<(DVec3, DVec3)>) -> Option<usize> {
        let mut z = f64::INFINITY;
        let mut top = None;
        for &i in candidates {
            let item = &self.items[i];
            let Some(depth) = depth_at(&item.shape, viewport, at, ray) else { continue };
            let drawn = match item.depth_test {
                DepthTest::Always => true,
                DepthTest::LessEqual | DepthTest::LessEqualWrite => depth <= z,
            };
            if drawn {
                top = Some(i);
                if item.depth_test == DepthTest::LessEqualWrite {
                    z = depth;
                }
            }
        }
        top
    }
}

/// The depth of `shape` at the pixel centred on `pixel` (CSS pixels), or `None` when it does not
/// cover that pixel.
fn depth_at(shape: &Shape, viewport: &ViewportBase, pixel: [f64; 2], ray: Option<(DVec3, DVec3)>) -> Option<f64> {
    match shape {
        Shape::Point { at: [x, y, depth], radius } => {
            let (dx, dy) = (pixel[0] - x, pixel[1] - y);
            (dx * dx + dy * dy <= radius * radius).then_some(*depth)
        }
        Shape::Rect { rect, depth } => {
            let inside = (rect.x0..=rect.x1).contains(&pixel[0]) && (rect.y0..=rect.y1).contains(&pixel[1]);
            inside.then_some(*depth)
        }
        Shape::Box(b) => {
            let (near, along) = ray?;
            let t = b.segment_hit(near, along)?;
            Some(viewport.world_to_pixels(near + along * t)[2])
        }
    }
}

/// A shape's bounds on the screen, `[x0, y0, x1, y1]` CSS pixels; `None` when unbounded.
fn bounds(shape: &Shape, viewport: &ViewportBase) -> Option<[f64; 4]> {
    match shape {
        Shape::Point { at: [x, y, _], radius } => Some([x - radius, y - radius, x + radius, y + radius]),
        Shape::Rect { rect, .. } => Some([rect.x0, rect.y0, rect.x1, rect.y1]),
        Shape::Box(b) => b.screen_bounds(viewport),
    }
}

/// The device pixels deck reads round the pointer: the pointer's pixel and the picking rectangle,
/// in WebGL's coordinates (rows counted from the bottom of the canvas).
struct Grid {
    /// The drawing buffer's height, device pixels.
    height_px: f64,
    /// CSS pixels per device pixel along each axis.
    css_per_px: [f64; 2],
    /// luma.gl's `cssToDeviceRatio`.
    ratio: f64,
    device_x: i64,
    device_y: i64,
    radius: i64,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl Grid {
    /// deck's `_pickClosestObject` up to the picking pass: the pointer's device pixel through
    /// luma.gl's `cssToDevicePixels(pointer, yInvert = true)`, then `_getPickingRect`. `None` when
    /// the rectangle falls off the canvas (the pointer is left of it or above it).
    fn new(viewport: &ViewportBase, pointer: [f64; 2], pixel_ratio: f64, radius: f64) -> Option<Grid> {
        let (css_width, css_height) = (viewport.width, viewport.height);
        if !(pointer[0].is_finite() && pointer[1].is_finite() && pixel_ratio.is_finite() && radius.is_finite()) {
            return None;
        }
        // luma.gl's drawing buffer is `floor(css size · useDevicePixels)`, and its
        // `cssToDeviceRatio` is the ratio of the widths
        let width = (css_width * pixel_ratio).floor();
        let height = (css_height * pixel_ratio).floor();
        if !(width >= 1.0 && height >= 1.0) {
            return None;
        }
        let ratio = width / css_width;

        // scalePixels(pointer, ratio, width, height, yInvert = true)
        let scale_x = |x: f64| js_round(x * ratio).min(width - 1.0);
        let scale_y = |y: f64| (height - 1.0 - js_round(y * ratio)).max(0.0);
        let x = scale_x(pointer[0]);
        let mut y = scale_y(pointer[1]);
        let t = scale_x(pointer[0] + 1.0);
        let x_high = if t == width - 1.0 { t } else { t - 1.0 };
        let mut t = scale_y(pointer[1] + 1.0);
        t = if t == 0.0 { t } else { t + 1.0 };
        let y_high = y;
        y = t;
        let range_width = (x_high - x + 1.0).max(1.0);
        let range_height = (y_high - y + 1.0).max(1.0);
        let device_x = x + (range_width / 2.0).floor();
        let device_y = y + (range_height / 2.0).floor();
        let device_radius = js_round(radius * ratio);

        // _getPickingRect
        let rect_x = (device_x - device_radius).max(0.0);
        let rect_y = (device_y - device_radius).max(0.0);
        let rect_width = width.min(device_x + device_radius + 1.0) - rect_x;
        let rect_height = height.min(device_y + device_radius + 1.0) - rect_y;
        if rect_width <= 0.0 || rect_height <= 0.0 {
            return None;
        }
        Some(Grid {
            height_px: height,
            css_per_px: [css_width / width, css_height / height],
            ratio,
            device_x: device_x as i64,
            device_y: device_y as i64,
            radius: device_radius as i64,
            x: rect_x as i64,
            y: rect_y as i64,
            width: rect_width as i64,
            height: rect_height as i64,
        })
    }

    /// The centre of a device pixel (WebGL column and row), CSS pixels from the top left.
    fn pixel_center(&self, col: i64, row: i64) -> [f64; 2] {
        let x = col as f64 + 0.5;
        let y = self.height_px - row as f64 - 0.5;
        [x * self.css_per_px[0], y * self.css_per_px[1]]
    }

    /// The first and last pixel centres of the picking rectangle, `[x0, y0, x1, y1]` CSS pixels.
    fn reach(&self) -> [f64; 4] {
        let [x0, y1] = self.pixel_center(self.x, self.y);
        let [x1, y0] = self.pixel_center(self.x + self.width - 1, self.y + self.height - 1);
        [x0, y0, x1, y1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{GLOBE_EARTH_RADIUS_M, ViewState, Viewport, mercator_units_per_meter, viewport_for};

    const SIZE: [f64; 2] = [800.0, 600.0];

    fn globe() -> Viewport {
        viewport_for(&ViewState::INITIAL, SIZE)
    }

    fn flat(pitch: f64) -> Viewport {
        let view = ViewState { longitude: 17.9186, latitude: 59.6519, zoom: 16.0, bearing: 30.0, pitch, position: None };
        viewport_for(&view, SIZE)
    }

    /// At pixel ratio 1 a pointer at integer `(x, y)` is on the pixel whose centre is
    /// `(x + 0.5, y + 0.5)` (luma.gl rounds the pointer to a pixel index). Items here are centred on
    /// pixel centres so that distances come out whole.
    fn disc(x: f64, y: f64, radius: f64) -> Shape {
        Shape::Point { at: [x + 0.5, y + 0.5, 0.0], radius }
    }

    fn at(x: f64, y: f64, depth: f64, radius: f64) -> Shape {
        Shape::Point { at: [x + 0.5, y + 0.5, depth], radius }
    }

    fn list(items: &[(&'static str, Shape, DepthTest)]) -> PickList<&'static str> {
        let mut list = PickList::new();
        for &(key, shape, depth_test) in items {
            list.push(key, shape, depth_test);
        }
        list
    }

    fn picked(list: &PickList<&'static str>, viewport: &Viewport, pointer: [f64; 2]) -> Option<&'static str> {
        list.pick(viewport, pointer, 1.0).map(|hit| *hit.key)
    }

    /// The box a mesh of `size_m` (`[x, y, z]` metres, standing on its base) fills at `lon, lat`,
    /// `up_m` above the ground, turned `heading` degrees clockwise from north, in the flat map's
    /// common space.
    fn flat_box(lon: f64, lat: f64, up_m: f64, size_m: [f64; 3], heading: f64, viewport: &Viewport) -> OrientedBox {
        let origin = viewport.project_position([lon, lat, up_m]);
        let units = mercator_units_per_meter(lat);
        let model = DMat4::from_translation(origin)
            * DMat4::from_rotation_z(-heading.to_radians())
            * DMat4::from_scale(DVec3::splat(units));
        let [x, y, z] = size_m;
        OrientedBox::from_matrix(&model, DVec3::new(-x / 2.0, -y / 2.0, 0.0), DVec3::new(x / 2.0, y / 2.0, z))
    }

    /// The same on the globe, from the east/north/up frame at the place.
    fn globe_box(lon: f64, lat: f64, up_m: f64, size_m: [f64; 3], viewport: &Viewport) -> OrientedBox {
        let origin = viewport.project_position([lon, lat, up_m]);
        let up = origin.normalize();
        let east = DVec3::new(lon.to_radians().cos(), lon.to_radians().sin(), 0.0);
        let north = up.cross(east);
        let units = 256.0 / GLOBE_EARTH_RADIUS_M;
        let model = DMat4::from_cols(
            (east * units).extend(0.0),
            (north * units).extend(0.0),
            (up * units).extend(0.0),
            origin.extend(1.0),
        );
        let [x, y, z] = size_m;
        OrientedBox::from_matrix(&model, DVec3::new(-x / 2.0, -y / 2.0, 0.0), DVec3::new(x / 2.0, y / 2.0, z))
    }

    #[test]
    fn a_point_is_picked_within_six_pixels_of_its_footprint() {
        let vp = globe();
        let list = list(&[("icon", disc(400.0, 300.0, 10.0), DepthTest::LessEqualWrite)]);
        // over it
        let hit = list.pick(&vp, [400.0, 300.0], 1.0).unwrap();
        assert_eq!((hit.index, *hit.key, hit.distance_px), (0, "icon", 0.0));
        assert_eq!(list.pick(&vp, [409.0, 300.0], 1.0).unwrap().distance_px, 0.0);
        // a click 5 px from the icon selects it, 8 px does not (A9); 6 is the last that does
        assert_eq!(list.pick(&vp, [415.0, 300.0], 1.0).unwrap().distance_px, 5.0);
        assert_eq!(list.pick(&vp, [416.0, 300.0], 1.0).unwrap().distance_px, 6.0);
        assert_eq!(picked(&list, &vp, [417.0, 300.0]), None);
        assert_eq!(picked(&list, &vp, [418.0, 300.0]), None);
        // the same to the left, above and below
        assert_eq!(picked(&list, &vp, [384.0, 300.0]), Some("icon"));
        assert_eq!(picked(&list, &vp, [383.0, 300.0]), None);
        assert_eq!(picked(&list, &vp, [400.0, 284.0]), Some("icon"));
        assert_eq!(picked(&list, &vp, [400.0, 283.0]), None);
        assert_eq!(picked(&list, &vp, [400.0, 316.0]), Some("icon"));
        assert_eq!(picked(&list, &vp, [400.0, 317.0]), None);
    }

    #[test]
    fn a_bare_point_is_picked_only_on_its_own_pixel_centre() {
        // a radius of 0 covers a pixel only when the point sits on its centre
        let vp = globe();
        let on_centre = list(&[("dot", disc(400.0, 300.0, 0.0), DepthTest::Always)]);
        assert_eq!(on_centre.pick(&vp, [404.0, 300.0], 1.0).unwrap().distance_px, 4.0);
        let between = list(&[("dot", Shape::Point { at: [400.0, 300.0, 0.0], radius: 0.0 }, DepthTest::Always)]);
        assert_eq!(picked(&between, &vp, [400.0, 300.0]), None);
    }

    #[test]
    fn the_radius_is_a_circle_not_a_square() {
        // a label covering the pixels 300..=359 × 200..=219
        let vp = globe();
        let rect = Rect::new(300.5, 200.5, 359.5, 219.5);
        let list = list(&[("label", Shape::Rect { rect, depth: 0.0 }, DepthTest::Always)]);
        assert_eq!(list.pick(&vp, [330.0, 210.0], 1.0).unwrap().distance_px, 0.0);
        assert_eq!(list.pick(&vp, [300.0, 200.0], 1.0).unwrap().distance_px, 0.0);
        assert_eq!(list.pick(&vp, [359.0, 219.0], 1.0).unwrap().distance_px, 0.0);
        assert_eq!(list.pick(&vp, [365.0, 210.0], 1.0).unwrap().distance_px, 6.0);
        assert_eq!(picked(&list, &vp, [366.0, 210.0]), None);
        assert_eq!(list.pick(&vp, [330.0, 194.0], 1.0).unwrap().distance_px, 6.0);
        assert_eq!(picked(&list, &vp, [330.0, 193.0]), None);
        // off the corner: 4 px across and 4 down is 5.66 px away, 5 and 5 is 7.07
        let hit = list.pick(&vp, [363.0, 223.0], 1.0).unwrap();
        assert!((hit.distance_px - 32f64.sqrt()).abs() < 1e-12);
        assert_eq!(picked(&list, &vp, [364.0, 224.0]), None);
        assert_eq!(picked(&list, &vp, [296.0, 196.0]), Some("label"));
        assert_eq!(picked(&list, &vp, [295.0, 195.0]), None);
    }

    #[test]
    fn a_rectangle_from_the_declutter_claim_is_picked() {
        // an airport code laid out 24 × 12 px, anchored at its middle
        let vp = globe();
        let text = Rect::new(-12.0, -6.0, 12.0, 6.0);
        let claim = crate::declutter::claim([500.0, 200.0], [0.0, 0.0], text, 1.0, crate::declutter::NO_PADDING);
        let mut list = PickList::new();
        list.rect("ARN", claim, 0.0, DepthTest::Always);
        assert_eq!(list.pick(&vp, [511.0, 205.0], 1.0).map(|h| *h.key), Some("ARN"));
        assert_eq!(list.pick(&vp, [517.0, 200.0], 1.0).unwrap().distance_px, 6.0);
        assert_eq!(list.pick(&vp, [518.0, 200.0], 1.0), None);
    }

    #[test]
    fn the_later_drawn_wins_where_two_overlap() {
        let vp = globe();
        let a = disc(400.0, 300.0, 10.0);
        let b = disc(408.0, 300.0, 10.0);
        let first = list(&[("a", a, DepthTest::Always), ("b", b, DepthTest::Always)]);
        assert_eq!(picked(&first, &vp, [404.0, 300.0]), Some("b"));
        let reversed = list(&[("b", b, DepthTest::Always), ("a", a, DepthTest::Always)]);
        assert_eq!(picked(&reversed, &vp, [404.0, 300.0]), Some("a"));
        // outside the overlap each is picked where it alone is
        assert_eq!(picked(&first, &vp, [392.0, 300.0]), Some("a"));
        assert_eq!(picked(&first, &vp, [416.0, 300.0]), Some("b"));
        // icons and labels without depth writes, ships and trains: the same rule
        let tested = list(&[("a", a, DepthTest::LessEqual), ("b", b, DepthTest::LessEqual)]);
        assert_eq!(picked(&tested, &vp, [404.0, 300.0]), Some("b"));
    }

    #[test]
    fn the_nearest_pixel_wins_before_the_topmost() {
        // the pointer is over "under", and 3 px from "over", which is drawn later and covers part of
        // it: deck reads the pointer's own pixel first
        let vp = globe();
        let list = list(&[
            ("under", disc(400.0, 300.0, 10.0), DepthTest::Always),
            ("over", disc(405.0, 300.0, 2.0), DepthTest::Always),
        ]);
        assert_eq!(picked(&list, &vp, [400.0, 300.0]), Some("under"));
        assert_eq!(picked(&list, &vp, [405.0, 300.0]), Some("over"));
        // on "under"'s rim
        assert_eq!(picked(&list, &vp, [400.0, 290.0]), Some("under"));
        // off both: "under"'s pixel 410 is 2 px away, "over"'s 407 is 5
        assert_eq!(picked(&list, &vp, [412.0, 300.0]), Some("under"));
        assert_eq!(picked(&list, &vp, [400.0, 311.0]), Some("under"));
    }

    #[test]
    fn of_two_pixels_equally_near_the_upper_one_then_the_right_one_wins() {
        // deck scans WebGL's rows from the bottom up and each row from the left, and a pixel as
        // close as the best so far replaces it
        let vp = globe();
        let above = disc(400.0, 296.0, 0.0);
        let below = disc(400.0, 304.0, 0.0);
        for items in [[("above", above), ("below", below)], [("below", below), ("above", above)]] {
            let list = list(&items.map(|(key, shape)| (key, shape, DepthTest::Always)));
            let hit = list.pick(&vp, [400.0, 300.0], 1.0).unwrap();
            assert_eq!((*hit.key, hit.distance_px), ("above", 4.0));
        }
        let left = disc(395.0, 300.0, 0.0);
        let right = disc(405.0, 300.0, 0.0);
        for items in [[("left", left), ("right", right)], [("right", right), ("left", left)]] {
            let list = list(&items.map(|(key, shape)| (key, shape, DepthTest::Always)));
            assert_eq!(picked(&list, &vp, [400.0, 300.0]), Some("right"));
        }
    }

    #[test]
    fn depth_writes_keep_the_nearer_on_top() {
        let vp = globe();
        let p = [400.0, 300.0];
        use DepthTest::{Always, LessEqual, LessEqualWrite};
        let case = |a: (f64, DepthTest), b: (f64, DepthTest)| {
            let list = list(&[("a", at(400.0, 300.0, a.0, 5.0), a.1), ("b", at(400.0, 300.0, b.0, 5.0), b.1)]);
            picked(&list, &vp, p).unwrap()
        };
        // two aircraft icons: the nearer, whichever is drawn first
        assert_eq!(case((0.2, LessEqualWrite), (0.5, LessEqualWrite)), "a");
        assert_eq!(case((0.5, LessEqualWrite), (0.2, LessEqualWrite)), "b");
        // at equal depth the later (less-equal)
        assert_eq!(case((0.3, LessEqualWrite), (0.3, LessEqualWrite)), "b");
        // a label drawn later is over whatever is nearer
        assert_eq!(case((0.2, LessEqualWrite), (0.5, Always)), "b");
        // a ship icon (no depth write) behind an earlier mesh is hidden, in front of it shows
        assert_eq!(case((0.2, LessEqualWrite), (0.5, LessEqual)), "a");
        assert_eq!(case((0.2, LessEqualWrite), (0.1, LessEqual)), "b");
        // what wrote no depth hides nothing drawn after it
        assert_eq!(case((0.2, LessEqual), (0.5, LessEqualWrite)), "b");
        assert_eq!(case((0.2, Always), (0.5, LessEqual)), "b");
        // and is hidden by nothing drawn after it either: the later always draws over it
        assert_eq!(case((0.5, LessEqual), (0.2, LessEqual)), "b");
        // a non-finite depth fails the depth test, so that item is not drawn
        assert_eq!(case((0.5, LessEqualWrite), (f64::NAN, LessEqualWrite)), "a");
    }

    #[test]
    fn nothing_in_reach_picks_nothing() {
        let vp = globe();
        assert_eq!(picked(&PickList::new(), &vp, [400.0, 300.0]), None);
        let list = list(&[("far", disc(100.0, 100.0, 10.0), DepthTest::Always)]);
        assert_eq!(picked(&list, &vp, [400.0, 300.0]), None);
        assert_eq!(picked(&list, &vp, [f64::NAN, 300.0]), None);
        // a radius of 0 reads the pointer's pixel alone
        assert_eq!(list.pick_within(&vp, [111.0, 100.0], 1.0, 0.0), None);
        assert_eq!(list.pick_within(&vp, [110.0, 100.0], 1.0, 0.0).unwrap().distance_px, 0.0);
    }

    #[test]
    fn the_radius_and_the_pointer_scale_with_the_pixel_ratio() {
        // round(6 · ratio) device pixels: 12 at 2, 9 at 1.5; the verdicts of a click 5 and 8 px off
        // an icon hold at every ratio
        let vp = globe();
        let list = list(&[("icon", disc(400.0, 300.0, 10.0), DepthTest::LessEqualWrite)]);
        for ratio in [1.0, 1.25, 1.5, 2.0, 3.0] {
            assert!(list.pick(&vp, [415.0, 300.0], ratio).is_some(), "{ratio}");
            assert!(list.pick(&vp, [418.0, 300.0], ratio).is_none(), "{ratio}");
            assert_eq!(list.pick(&vp, [400.0, 300.0], ratio).unwrap().distance_px, 0.0, "{ratio}");
        }
        // at 2 the pointer at 415 is device pixel 831 (centre 415.75 CSS), the icon's last covered
        // pixel is 820 (410.25 CSS): 11 device pixels, 5.5 CSS
        assert_eq!(list.pick(&vp, [415.0, 300.0], 2.0).unwrap().distance_px, 5.5);
        // at 1.25, round(7.5) = 8 device pixels: 6.4 CSS
        let edge = list.pick(&vp, [416.0, 300.0], 1.25).unwrap();
        assert!(edge.distance_px <= 6.4, "{}", edge.distance_px);
    }

    #[test]
    fn the_canvas_edges_clip_as_luma_gl_does() {
        let vp = globe();
        let list = list(&[
            ("left", disc(2.0, 300.0, 1.0), DepthTest::Always),
            ("right", disc(797.0, 300.0, 1.0), DepthTest::Always),
            ("bottom", disc(400.0, 597.0, 1.0), DepthTest::Always),
        ]);
        assert_eq!(picked(&list, &vp, [0.0, 300.0]), Some("left"));
        assert_eq!(picked(&list, &vp, [799.0, 300.0]), Some("right"));
        // left of or above the canvas, nothing: the picking rectangle falls off it
        assert_eq!(picked(&list, &vp, [-20.0, 300.0]), None);
        assert_eq!(picked(&list, &vp, [400.0, -20.0]), None);
        // right of or below it, the pointer is clamped onto the last pixel
        assert_eq!(list.pick(&vp, [850.0, 300.0], 1.0).map(|h| (*h.key, h.distance_px)), Some(("right", 1.0)));
        assert_eq!(list.pick(&vp, [400.0, 650.0], 1.0).map(|h| (*h.key, h.distance_px)), Some(("bottom", 1.0)));
        // no canvas, nothing
        let empty = viewport_for(&ViewState::INITIAL, [0.0, 0.0]);
        assert_eq!(picked(&list, &empty, [0.0, 0.0]), None);
    }

    #[test]
    fn a_box_from_an_instance_matrix_has_the_matrix_corners() {
        let model = DMat4::from_translation(DVec3::new(10.0, -4.0, 2.0))
            * DMat4::from_rotation_z(0.5)
            * DMat4::from_rotation_x(-0.2)
            * DMat4::from_scale(DVec3::new(2.0, 3.0, 0.5));
        let (min, max) = (DVec3::new(-1.0, -2.0, 0.0), DVec3::new(3.0, 2.0, 4.0));
        let b = OrientedBox::from_matrix(&model, min, max);
        for (k, corner) in b.corners().into_iter().enumerate() {
            let pick = |i: usize| if (k >> i) & 1 == 1 { max[i] } else { min[i] };
            let expected = model.transform_point3(DVec3::new(pick(0), pick(1), pick(2)));
            assert!((corner - expected).length() < 1e-12, "{k}: {corner} {expected}");
        }
        for i in 0..3 {
            assert!((b.axes[i].length() - 1.0).abs() < 1e-12);
            for j in 0..i {
                assert!(b.axes[i].dot(b.axes[j]).abs() < 1e-12);
            }
        }
        // a model squashed flat keeps its face direction
        let flat = OrientedBox::from_matrix(&DMat4::from_scale(DVec3::new(1.0, 1.0, 0.0)), min, max);
        assert_eq!((flat.axes[2], flat.half[2]), (DVec3::Z, 0.0));
        // the slab test from inside, through, past and short of it
        let unit = OrientedBox { center: DVec3::ZERO, axes: [DVec3::X, DVec3::Y, DVec3::Z], half: [1.0; 3] };
        assert_eq!(unit.segment_hit(DVec3::ZERO, DVec3::X), Some(0.0));
        assert_eq!(unit.segment_hit(DVec3::new(-3.0, 0.5, 0.5), DVec3::new(4.0, 0.0, 0.0)), Some(0.5));
        assert_eq!(unit.segment_hit(DVec3::new(-3.0, 1.5, 0.5), DVec3::new(4.0, 0.0, 0.0)), None);
        assert_eq!(unit.segment_hit(DVec3::new(-3.0, 0.0, 0.0), DVec3::new(1.0, 0.0, 0.0)), None);
        assert_eq!(unit.segment_hit(DVec3::new(3.0, 0.0, 0.0), DVec3::new(4.0, 0.0, 0.0)), None);
        assert_eq!(flat.segment_hit(DVec3::new(0.0, 0.0, 5.0), DVec3::new(0.0, 0.0, -10.0)), Some(0.5));
    }

    #[test]
    fn a_box_is_picked_where_it_shows_from_above() {
        // a 40 × 20 m hull 10 m tall at Arlanda, at zoom 16 (about 0.6 m a pixel), heading 30°
        let vp = flat(0.0);
        let (lon, lat) = (17.9186, 59.6519);
        let hull = flat_box(lon, lat, 0.0, [20.0, 40.0, 10.0], 30.0, &vp);
        let mut list = PickList::new();
        list.oriented_box("ship", hull, DepthTest::LessEqualWrite);
        let [cx, cy, _] = vp.project([lon, lat, 10.0]);
        assert_eq!(list.pick(&vp, [cx, cy], 1.0).map(|h| h.distance_px), Some(0.0));
        // the view is turned with the hull (bearing 30, heading 30), so it lies along the screen's y
        let [_, bow_y, _] = vp.project(vp.unproject_position(hull.center + hull.axes[1] * hull.half[1]));
        let [side_x, _, _] = vp.project(vp.unproject_position(hull.center + hull.axes[0] * hull.half[0]));
        assert!((cy - bow_y - 40.0 / 2.0 / 0.6026).abs() < 3.0, "{cy} {bow_y}");
        let side = side_x.max(2.0 * cx - side_x);
        assert!(list.pick(&vp, [side + 3.0, cy], 1.0).is_some());
        assert!(list.pick(&vp, [side + 9.0, cy], 1.0).is_none());
        let bow = bow_y.min(2.0 * cy - bow_y);
        assert!(list.pick(&vp, [cx, bow - 3.0], 1.0).is_some());
        assert!(list.pick(&vp, [cx, bow - 9.0], 1.0).is_none());
    }

    #[test]
    fn a_tall_box_is_picked_by_its_side_and_top_when_pitched() {
        // a 10 m wide, 120 m tall mast seen at pitch 60: its top shows well above its foot
        let vp = flat(60.0);
        let (lon, lat) = (17.9186, 59.6519);
        let mut list = PickList::new();
        list.oriented_box("mast", flat_box(lon, lat, 0.0, [10.0, 10.0, 120.0], 0.0, &vp), DepthTest::LessEqualWrite);
        let foot = vp.project([lon, lat, 0.0]);
        let top = vp.project([lon, lat, 120.0]);
        assert!(foot[1] - top[1] > 60.0, "{foot:?} {top:?}");
        let middle = vp.project([lon, lat, 60.0]);
        for p in [foot, middle, top] {
            assert!(list.pick(&vp, [p[0], p[1]], 1.0).is_some(), "{p:?}");
        }
        // well above the top and beside the middle, nothing
        assert!(list.pick(&vp, [top[0], top[1] - 30.0], 1.0).is_none());
        assert!(list.pick(&vp, [middle[0] + 40.0, middle[1]], 1.0).is_none());
    }

    #[test]
    fn a_box_on_the_globe_hides_an_icon_beneath_and_not_one_above() {
        // a 3 km block 500 m tall under the view centre at zoom 11, and two icons at its centre
        // drawn after it: one on the ground (hidden inside the box), one at 2 km (above its top)
        let view = ViewState { longitude: 18.07, latitude: 59.33, zoom: 11.0, bearing: 0.0, pitch: 0.0, position: None };
        let vp = viewport_for(&view, SIZE);
        assert!(vp.is_globe());
        let block = globe_box(18.07, 59.33, 0.0, [3000.0, 3000.0, 500.0], &vp);
        let ground = vp.project([18.07, 59.33, 0.0]);
        let high = vp.project([18.07, 59.33, 2000.0]);
        let mut list = PickList::new();
        list.oriented_box("block", block, DepthTest::LessEqualWrite);
        list.point("ground", ground, 4.0, DepthTest::LessEqualWrite);
        assert_eq!(list.pick(&vp, [ground[0], ground[1]], 1.0).map(|h| *h.key), Some("block"));
        list.point("high", high, 4.0, DepthTest::LessEqualWrite);
        assert_eq!(list.pick(&vp, [high[0], high[1]], 1.0).map(|h| *h.key), Some("high"));
        // a label drawn after them is over both
        list.point("label", [ground[0], ground[1], 0.9], 4.0, DepthTest::Always);
        assert_eq!(list.pick(&vp, [ground[0], ground[1]], 1.0).map(|h| *h.key), Some("label"));
    }

    #[test]
    fn a_box_behind_the_camera_is_not_picked() {
        let vp = flat(0.0);
        let eye = vp.camera_position;
        let behind = OrientedBox {
            center: eye + DVec3::Z * 0.001,
            axes: [DVec3::X, DVec3::Y, DVec3::Z],
            half: [0.0005; 3],
        };
        let mut list = PickList::new();
        list.oriented_box("behind", behind, DepthTest::LessEqualWrite);
        assert!(behind.screen_bounds(&vp).is_none());
        assert_eq!(list.pick(&vp, [400.0, 300.0], 1.0), None);
    }

    #[test]
    fn builders_lists_gather_into_one_in_draw_order() {
        let vp = globe();
        let mut ships = PickList::new();
        ships.point(7usize, [400.5, 300.5, 0.0], 5.0, DepthTest::LessEqual);
        let mut trains = PickList::new();
        trains.point("ice", [400.5, 300.5, 0.0], 5.0, DepthTest::LessEqual);
        #[derive(Debug, PartialEq)]
        enum Picked {
            Ship(usize),
            Train(&'static str),
        }
        let mut all: PickList<Picked> = ships.into_iter().map(|item| item.map_key(Picked::Ship)).collect();
        all.extend(trains.into_iter().map(|item| item.map_key(Picked::Train)));
        assert_eq!(all.len(), 2);
        let hit = all.pick(&vp, [400.0, 300.0], 1.0).unwrap();
        assert_eq!((hit.index, hit.key), (1, &Picked::Train("ice")));
        all.clear();
        assert!(all.is_empty());
    }
}
