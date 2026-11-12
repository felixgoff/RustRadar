//! The lifted RainViewer layers and their walls (item W3; doc 02 §3.5 "RainViewer lifted layers";
//! doc 01 §5.11): today's `radarLifted` (G:1301-1339), pushed by `buildLayers` right after the
//! RainViewer tile layer (`layers.push(radarLayer(radarUrl), ...radarLifted(ex, lift))`, G:2683).
//!
//! - **Only while tilted.** `lift = liftFor(pitch)` ([`lift_for`]: 0 up to 11.5°, by the
//!   rounding; 1 from 40° on, in tenths). With no lift, or no RainViewer frame (`radarUrl` null:
//!   the weather off or no frame), the arenas are freed (`freeArenas`) and nothing is drawn;
//!   looking straight down there is no stack at all.
//! - **The arenas** ([`super::arena::Arenas`], W2's `liftedArenas`): a slices and a walls arena per
//!   height of `RADAR_LEVELS_KM`, holding every RainViewer tile in view (`radarTiles`, the radar
//!   tiles' [`super::tiles::RadarTiles::in_view`]) that reaches the height, from that level's band
//!   up, and updated only when the tiles in view have a new version (`radarVersion`).
//! - **The layers** ([`layers`]), level by level, lowest first, for each height whose slices arena
//!   has buffers (`if (!slices.data) return`): its slices (`radar-lift-<km>`) at
//!   `z = elevation(km · 3280.84 ft, scale)` ([`level_height`]), then its walls (`radar-wall-<km>`,
//!   if the walls arena has buffers) from `bottom` up to `z`, where `bottom` is the height below's
//!   `z` (the ground for the lowest), so the stack has no gaps. Both at opacity
//!   `max(0.16, 0.34 − 0.03·i) · lift` ([`level_opacity`]: 0.34, 0.31, … 0.16 at full lift), with
//!   `RADAR_LIFTED` (no culling, no depth written, depth-tested). `scale` is the effective
//!   exaggeration at this frame's zoom in tenths ([`height_scale`]). Higher layers keep only the
//!   stronger echoes, so cells rise like towers. One layer per height across all tiles, so the
//!   stack blends in height order across tile borders; level 0's slices, level 0's walls, level
//!   1's slices, and so on.
//! - **Drawn** from the arenas with [`ArenaPipelines`] (`radar.wgsl`): a slice raised by its
//!   frame's `z_offset` (`modelMatrix` translating by `z`), a wall's unit height stretched by its
//!   style's `z_scale = z − bottom` from its foot at `bottom` (the matrix
//!   `[1,0,0,0, 0,1,0,0, 0,0,z−bottom,0, 0,0,bottom,1]`), so a change of tilt or height scale
//!   moves the layers without touching their buffers. The draws are worked out every frame from
//!   this frame's camera; today's layers are rebuilt only when `"{scale}/{lift}/{radarVersion}"`
//!   changes, which gives the same layers.
//! - **The height setting** comes with the measured volumes' input
//!   (`radar::Input::measured.exaggeration`, the app's `exaggeration`): today's `ex` serves both.
//!
//! Checked against a CPU twin of today's draws (the tests' `Twin`: the ground tiles, then each
//! height's slices and walls in order, every triangle at the map pass's four samples, blended in
//! 8 bits): a storm cell tilted 55° and 25° agrees within 2 levels in 97 % of the pixels it
//! changes and V2 pitched 45° with the committed frame in 89 %, nearly all the rest an edge a
//! fraction of a pixel off among the stack's many edges; at pitch 10 the radar layer draws the
//! ground layer's picture to the pixel.
//!
//! Deviations: none in what is drawn. The arenas take a new set of tiles in view in one frame,
//! as today's do, outside the frame's tile upload budget (they are edited in place: only the
//! tiles new to them are written). The arenas' own deviations are in [`super::arena`] (an `f32`
//! world frame; on the flat map one copy of the world).

use rustradar_core::num::js_string;
use rustradar_scene::radar::RADAR_LEVELS_KM;

use super::arena::{self, ArenaPipelines, ArenaStyle, Arenas};
pub use super::measured::{height_scale, level_height, lift_for};
use super::measured::StyleUniform;
use super::ground;
use crate::frame::FrameSlot;
use crate::layers::{DrawCx, Init, PrepareCx};
use crate::state::DrawState;

/// The lowest height's opacity at full lift (`0.34 - i * 0.03`).
pub const LOWEST_OPACITY: f64 = 0.34;

/// What each height up takes off the opacity.
pub const OPACITY_STEP: f64 = 0.03;

/// The least opacity of a height at full lift (`Math.max(0.16, …)`).
pub const LEAST_OPACITY: f64 = 0.16;

/// The opacity of height `i` (an index of `RADAR_LEVELS_KM`) at `lift`:
/// `Math.max(0.16, 0.34 - i * 0.03) * lift`.
pub fn level_opacity(i: usize, lift: f64) -> f64 {
    // (no NaN here: f64::max is JavaScript's Math.max)
    LEAST_OPACITY.max(LOWEST_OPACITY - i as f64 * OPACITY_STEP) * lift
}

/// Which arena of a height a draw takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    /// The tiles' slices from the height's band up, flat at the height.
    Slices,
    /// Their walls, from the height below (or the ground) up to it.
    Walls,
}

/// One lifted layer this frame (a `radarPolygons` call of `radarLifted`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerSpec {
    /// The height's index in `RADAR_LEVELS_KM` (and in [`Arenas::levels`]).
    pub level: usize,
    pub what: What,
    /// The height, km.
    pub km: f64,
    /// Multiplies the vertices' alpha.
    pub opacity: f64,
    /// Metres: the slices' height, the walls' top.
    pub z: f64,
    /// Metres: the walls' foot (0 for slices).
    pub bottom: f64,
}

impl LayerSpec {
    /// Today's layer id (`radar-lift-<km>`, `radar-wall-<km>`).
    pub fn id(&self) -> String {
        match self.what {
            What::Slices => format!("radar-lift-{}", js_string(self.km)),
            What::Walls => format!("radar-wall-{}", js_string(self.km)),
        }
    }

    /// The frame's height and the style's stretch it is drawn with: a slice raised to `z`, a
    /// wall's unit height stretched to `z − bottom` from `bottom`.
    pub fn placement(&self) -> (f64, f64) {
        match self.what {
            What::Slices => (self.z, 0.0),
            What::Walls => (self.bottom, self.z - self.bottom),
        }
    }
}

/// The lifted layers in draw order (`radarLifted(ex, lift)` once the arenas are filled): nothing
/// while `lift_for(pitch)` is 0; else for each height `i` whose slices arena has buffers
/// (`present[i].0`), lowest first, its slices at `z`, then, if its walls arena has buffers
/// (`present[i].1`), its walls from the height below's `z` (0 for the lowest) up to `z`, both at
/// [`level_opacity`]. `present` holds one entry per height of `RADAR_LEVELS_KM`.
pub fn layers(present: &[(bool, bool)], exaggeration: f64, zoom: f64, pitch: f64) -> Vec<LayerSpec> {
    let lift = lift_for(pitch);
    if lift == 0.0 {
        return Vec::new();
    }
    let scale = height_scale(exaggeration, zoom);
    let mut out = Vec::new();
    // higher layers keep only the stronger echoes, so cells rise like towers
    for (i, &(slices, walls)) in present.iter().enumerate().take(RADAR_LEVELS_KM.len()) {
        if !slices {
            continue;
        }
        let km = RADAR_LEVELS_KM[i];
        let opacity = level_opacity(i, lift);
        let z = level_height(km, scale);
        out.push(LayerSpec { level: i, what: What::Slices, km, opacity, z, bottom: 0.0 });
        // walled down to the layer below, the lowest to the ground, so the stack has no gaps
        let bottom = if i > 0 { level_height(RADAR_LEVELS_KM[i - 1], scale) } else { 0.0 };
        if walls {
            out.push(LayerSpec { level: i, what: What::Walls, km, opacity, z, bottom });
        }
    }
    out
}

/// One draw of this frame.
struct Draw {
    spec: LayerSpec,
    frame: FrameSlot,
    style: usize,
}

/// The lifted RainViewer layers (row 5a after the ground layer): the arenas, their pipelines with
/// `RADAR_LIFTED`, and this frame's draws.
pub struct Lifted {
    arenas: Arenas,
    pipelines: ArenaPipelines,
    /// One per draw, reused from frame to frame.
    styles: Vec<ArenaStyle>,
    draws: Vec<Draw>,
}

impl Lifted {
    pub fn new(init: &Init<'_>) -> Self {
        Lifted { arenas: Arenas::new(), pipelines: ArenaPipelines::new(init, "radar lifted", DrawState::RADAR_LIFTED), styles: Vec::new(), draws: Vec::new() }
    }

    /// One frame's work, after the radar tiles' update: with RainViewer drawn and the view
    /// tilted, the arenas brought to the tiles in view and this frame's draws placed for its
    /// camera; else the arenas freed. `exaggeration` is the height setting.
    pub fn prepare(&mut self, cx: &mut PrepareCx<'_>, tiles: &ground::Tiles, exaggeration: f64) {
        self.draws.clear();
        let view = cx.view.view;
        if lift_for(view.pitch) == 0.0 || tiles.url().is_none() {
            self.arenas.free();
            return;
        }
        self.arenas.update(cx.gpu, tiles.in_view(), tiles.version());
        let present: Vec<(bool, bool)> = self.arenas.levels().iter().map(|l| (l.slices.has_data(), l.walls.has_data())).collect();
        for spec in layers(&present, exaggeration, view.zoom, view.pitch) {
            let (z_offset, z_scale) = spec.placement();
            let value = StyleUniform::new(spec.opacity, z_scale);
            let style = self.draws.len();
            if style == self.styles.len() {
                self.styles.push(self.pipelines.style(&cx.gpu.device, value));
            } else {
                self.styles[style].set(&cx.gpu.queue, value);
            }
            let frame = cx.frames.push(&arena::frame(cx.view, z_offset));
            self.draws.push(Draw { spec, frame, style });
        }
    }

    /// The arenas (for tests and logs).
    pub fn arenas(&self) -> &Arenas {
        &self.arenas
    }

    /// This frame's layers, in draw order.
    pub fn drawn(&self) -> Vec<LayerSpec> {
        self.draws.iter().map(|d| d.spec).collect()
    }

    /// How many draws this frame makes.
    pub fn draw_count(&self) -> usize {
        self.draws.len()
    }

    /// Records this frame's draws, level by level.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        let levels = self.arenas.levels();
        for draw in &self.draws {
            let Some(level) = levels.get(draw.spec.level) else { continue };
            let arena = match draw.spec.what {
                What::Slices => &level.slices,
                What::Walls => &level.walls,
            };
            self.pipelines.draw(pass, cx, arena, draw.frame, &self.styles[draw.style]);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::Arc;

    use rustradar_core::assert_close;
    use rustradar_core::geo::{Basemap, effective_exaggeration};
    use rustradar_core::num::{clamp01, js_round};
    use rustradar_scene::camera::{ViewState, Viewport};
    use rustradar_scene::radar::{RadarMesh, tile_bounds, tile_latitude};

    use super::super::tiles::RadarTile;
    use super::super::{Input as RadarInput, Radar, measured, tiles};
    use super::*;
    use crate::frame::ViewFrame;
    use crate::gpu::Gpu;
    use crate::gpu::testing::gpu;
    use crate::headless::{FORMAT, Image, render_with};
    use crate::io::{self, DirTiles, FnFetcher, TileFetcher};
    use crate::layers::{Layer, TileIndex, earth, vector_map};
    use crate::renderer::{MapRenderer, SceneInput};

    const FRAME: &str = "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png";

    #[test]
    fn each_height_is_as_opaque_as_today_s() {
        // `Math.max(0.16, 0.34 - i * 0.03) * lift`
        let full: Vec<f64> = (0..RADAR_LEVELS_KM.len()).map(|i| level_opacity(i, 1.0)).collect();
        for (got, want) in full.iter().zip([0.34, 0.31, 0.28, 0.25, 0.22, 0.19, 0.16]) {
            assert_close!(*got, want, 12);
        }
        assert_eq!(full[1], 0.34 - 0.03, "JavaScript's doubles");
        // the floor
        assert_eq!(level_opacity(7, 1.0), 0.16);
        assert_eq!(level_opacity(20, 1.0), 0.16);
        // half lifted, half as opaque; not lifted, not drawn
        assert_close!(level_opacity(0, 0.5), 0.17, 12);
        assert_close!(level_opacity(6, 0.3), 0.048, 12);
        assert_eq!(level_opacity(3, 0.0), 0.0);
    }

    #[test]
    fn the_stack_goes_height_by_height_slices_then_walls() {
        let all = [(true, true); 7];
        let tilted = layers(&all, 10.0, 4.5, 45.0);
        let ids: Vec<String> = tilted.iter().map(LayerSpec::id).collect();
        assert_eq!(
            ids,
            [
                "radar-lift-1.5",
                "radar-wall-1.5",
                "radar-lift-3",
                "radar-wall-3",
                "radar-lift-4.5",
                "radar-wall-4.5",
                "radar-lift-6",
                "radar-wall-6",
                "radar-lift-8",
                "radar-wall-8",
                "radar-lift-10",
                "radar-wall-10",
                "radar-lift-12",
                "radar-wall-12"
            ]
        );
        // far out the whole exaggeration (10), in tenths
        let z = |km: f64| level_height(km, 10.0);
        assert_close!(z(1.5), 1.5 * 3280.84 * 0.3048 * 10.0, 12);
        for (pair, (i, &km)) in tilted.chunks(2).zip(RADAR_LEVELS_KM.iter().enumerate()) {
            let (slices, walls) = (pair[0], pair[1]);
            assert_eq!((slices.level, slices.what, slices.km, walls.level, walls.what, walls.km), (i, What::Slices, km, i, What::Walls, km));
            assert_eq!((slices.z, walls.z), (z(km), z(km)));
            assert_eq!((slices.opacity, walls.opacity), (level_opacity(i, 1.0), level_opacity(i, 1.0)));
        }
        // the walls run from the height below, the lowest from the ground: no gaps
        let bottoms: Vec<f64> = tilted.iter().filter(|l| l.what == What::Walls).map(|l| l.bottom).collect();
        assert_eq!(bottoms, [0.0, z(1.5), z(3.0), z(4.5), z(6.0), z(8.0), z(10.0)]);
        // a slice is raised to its height; a wall's unit height stretched up from its foot
        assert_eq!(tilted[2].placement(), (z(3.0), 0.0));
        assert_eq!(tilted[3].placement(), (z(1.5), z(3.0) - z(1.5)));

        // a height whose slices arena has no buffers is left out, walls and all; walls only where
        // their arena has buffers, still from the height below whether or not it was drawn
        let some = [(true, false), (true, true), (false, true), (true, true), (false, false), (false, false), (false, false)];
        let drawn = layers(&some, 10.0, 4.5, 45.0);
        let ids: Vec<String> = drawn.iter().map(LayerSpec::id).collect();
        assert_eq!(ids, ["radar-lift-1.5", "radar-lift-3", "radar-wall-3", "radar-lift-6", "radar-wall-6"]);
        assert_eq!(drawn[4].bottom, z(4.5));
        assert!(layers(&[], 10.0, 4.5, 45.0).is_empty());

        // looking down (to 11.5°, by the rounding) nothing; a tenth from there; half at 25°
        assert!(layers(&all, 10.0, 4.5, 0.0).is_empty());
        assert!(layers(&all, 10.0, 4.5, 10.0).is_empty());
        assert!(layers(&all, 10.0, 4.5, 11.4).is_empty());
        assert_close!(layers(&all, 10.0, 4.5, 11.5)[0].opacity, 0.034, 12);
        let half = layers(&all, 10.0, 4.5, 25.0);
        assert_eq!(half.len(), 14);
        assert_close!(half[0].opacity, 0.17, 12);
        assert_close!(half[13].opacity, 0.08, 12);
        // close in the true heights; between, the exaggeration in tenths
        assert_eq!(layers(&all, 10.0, 11.0, 45.0)[0].z, level_height(1.5, 1.0));
        assert_eq!(layers(&all, 10.0, 8.5, 45.0)[0].z, level_height(1.5, 5.5));
    }

    /// What the probe's radar layer drew after its last prepare.
    #[derive(Debug, Default, Clone)]
    struct Seen {
        /// The lifted layers, in draw order.
        drawn: Vec<LayerSpec>,
        /// Heights the arenas hold (none once freed).
        levels: usize,
        /// The tiles the ground draws, in order, and the tiles in view (the arenas' order).
        ground: Vec<Arc<RadarTile>>,
        in_view: Vec<Arc<RadarTile>>,
    }

    /// What the probe draws of its radar layer.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Show {
        Nothing,
        Ground,
        All,
    }

    struct Control {
        input: RadarInput,
        show: Show,
    }

    /// The radar layer (`layers::radar::Radar`, with this item's wiring) on the probe's own radar
    /// input, reporting what it drew and drawing all of it, the ground alone or nothing.
    struct Probe {
        radar: Radar,
        scene: SceneInput,
        control: Rc<RefCell<Control>>,
        seen: Rc<RefCell<Seen>>,
    }

    impl Layer for Probe {
        fn name(&self) -> &'static str {
            "lifted probe"
        }

        fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
            self.scene.radar = self.control.borrow().input.clone();
            let mut inner = PrepareCx {
                gpu: cx.gpu,
                scene: &self.scene,
                view: cx.view,
                frames: &mut *cx.frames,
                local_now_ms: cx.local_now_ms,
                tiles: cx.tiles,
                budget: &mut *cx.budget,
                textures: &mut *cx.textures,
            };
            self.radar.prepare(&mut inner);
            let radar = &self.radar;
            *self.seen.borrow_mut() = Seen {
                drawn: radar.lifted.drawn(),
                levels: radar.lifted.arenas().levels().len(),
                ground: radar.tiles.visible().map(|(_, t, _)| Arc::clone(t)).collect(),
                in_view: radar.tiles.in_view().iter().map(|(_, t)| Arc::clone(t)).collect(),
            };
        }

        fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
            match self.control.borrow().show {
                Show::Nothing => {}
                Show::Ground => self.radar.ground.draw(pass, cx, &self.radar.tiles),
                Show::All => self.radar.draw(pass, cx),
            }
        }

        fn pending(&self) -> bool {
            self.radar.pending()
        }
    }

    /// A renderer on `tiles` with the probe as its last layer, RainViewer's `url` and the height
    /// setting `exaggeration`.
    fn probed(gpu: &Gpu, tiles: Arc<dyn TileFetcher>, url: &str, exaggeration: f64) -> (MapRenderer, Rc<RefCell<Control>>, Rc<RefCell<Seen>>) {
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        renderer.set_tiles(tiles, None);
        let input = RadarInput { tiles: tiles::Input { url: Some(url.into()), ..Default::default() }, measured: measured::Input { exaggeration, ..Default::default() } };
        let control = Rc::new(RefCell::new(Control { input, show: Show::All }));
        let seen = Rc::new(RefCell::new(Seen::default()));
        let scene = SceneInput::new(ViewState::INITIAL, [1.0, 1.0], 1.0, Basemap::Dark);
        let probe = Probe { radar: Radar::new(&renderer.init(gpu)), scene, control: Rc::clone(&control), seen: Rc::clone(&seen) };
        renderer.add_layer(Box::new(probe));
        (renderer, control, seen)
    }

    /// `view` on a `size` canvas, over the dark map when `map`, else over ink; the probe showing
    /// `show`.
    fn shot(gpu: &Gpu, renderer: &mut MapRenderer, control: &Rc<RefCell<Control>>, view: ViewState, size: [f64; 2], map: bool, show: Show) -> Image {
        control.borrow_mut().show = show;
        let mut scene = SceneInput::new(view, size, 1.0, Basemap::Dark);
        if map {
            scene.earth = earth::Input::SHOWN;
            scene.vector_map = vector_map::Input::SHOWN;
        }
        let image = render_with(gpu, renderer, &scene, 1.0).expect("rendered");
        assert!(!renderer.pending(), "still loading: {:?}", renderer.pending_layers());
        image
    }

    fn shots_dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/shots");
        std::fs::create_dir_all(&dir).expect("the shot folder");
        dir
    }

    /// The map pass's blend of `color` at `opacity` over `under` (gamma space), kept in 8 bits.
    fn blend(under: [u8; 3], color: &[u8], opacity: f64) -> [u8; 3] {
        let alpha = f64::from(color[3]) / 255.0 * opacity;
        [0, 1, 2].map(|i| (f64::from(under[i]) + (f64::from(color[i]) - f64::from(under[i])) * alpha).round() as u8)
    }

    /// The map pass's four sample positions in a pixel (Vulkan's standard 4× pattern).
    const SAMPLES: [[f64; 2]; 4] = [[0.375, 0.125], [0.875, 0.375], [0.125, 0.625], [0.625, 0.875]];

    /// A CPU twin of the radar's draws: each triangle of each draw in order, at the map pass's four
    /// samples a pixel, blended as the map pass blends (gamma space, each sample kept in 8 bits),
    /// over a picture the GPU drew without the radar; a pixel is its samples' mean.
    struct Twin {
        width: u32,
        height: u32,
        samples: Vec<[[u8; 3]; 4]>,
        /// How many lifted layers cover each pixel, and whether walls and the ground layer do.
        lifted: Vec<u8>,
        walled: Vec<bool>,
        grounded: Vec<bool>,
        /// The layer being drawn (counted once a pixel however many of its triangles cover it).
        layer: u32,
        last: Vec<u32>,
        kind: (bool, bool),
    }

    impl Twin {
        fn over(base: &Image) -> Self {
            let n = (base.width * base.height) as usize;
            Twin {
                width: base.width,
                height: base.height,
                samples: base.rgba.chunks_exact(4).map(|p| [[p[0], p[1], p[2]]; 4]).collect(),
                lifted: vec![0; n],
                walled: vec![false; n],
                grounded: vec![false; n],
                layer: 0,
                last: vec![0; n],
                kind: (false, false),
            }
        }

        /// A new layer: lifted or the ground, walls or slices.
        fn layer(&mut self, lifted: bool, walls: bool) {
            self.layer += 1;
            self.kind = (lifted, walls);
        }

        /// `mesh`'s triangles from `from_band` up, each vertex `height(position)` metres up, at
        /// `opacity`.
        fn draw(&mut self, viewport: &Viewport, mesh: &RadarMesh, from_band: usize, height: impl Fn(&[f32]) -> f64, opacity: f64) {
            let screen: Vec<[f64; 2]> = mesh
                .positions
                .chunks_exact(mesh.size)
                .map(|p| {
                    let [x, y, _] = viewport.project([f64::from(p[0]), f64::from(p[1]), height(p)]);
                    [x, y]
                })
                .collect();
            let (first, _) = mesh.band_range(from_band);
            let edge = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
            for t in mesh.indices[first..].chunks_exact(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| screen[i as usize]);
                let color = &mesh.colors[t[0] as usize * 4..t[0] as usize * 4 + 4];
                assert!(t.iter().all(|&i| mesh.colors[i as usize * 4..i as usize * 4 + 4] == *color), "a triangle of one colour");
                let area = edge(a, b, c);
                if area == 0.0 || !area.is_finite() {
                    continue;
                }
                let (x0, x1) = (a[0].min(b[0]).min(c[0]).floor().max(0.0), a[0].max(b[0]).max(c[0]).ceil().min(f64::from(self.width)));
                let (y0, y1) = (a[1].min(b[1]).min(c[1]).floor().max(0.0), a[1].max(b[1]).max(c[1]).ceil().min(f64::from(self.height)));
                for y in y0 as u32..y1.max(0.0) as u32 {
                    for x in x0 as u32..x1.max(0.0) as u32 {
                        let i = (y * self.width + x) as usize;
                        let mut hit = false;
                        for (k, [sx, sy]) in SAMPLES.iter().enumerate() {
                            let p = [f64::from(x) + sx, f64::from(y) + sy];
                            if [edge(b, c, p), edge(c, a, p), edge(a, b, p)].iter().all(|&w| w * area.signum() >= 0.0) {
                                self.samples[i][k] = blend(self.samples[i][k], color, opacity);
                                hit = true;
                            }
                        }
                        if hit && self.last[i] != self.layer {
                            self.last[i] = self.layer;
                            self.lifted[i] += u8::from(self.kind.0);
                            self.walled[i] |= self.kind.1;
                            self.grounded[i] |= !self.kind.0;
                        }
                    }
                }
            }
        }

        /// The resolved pixel: its samples' mean.
        fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
            let s = &self.samples[(y * self.width + x) as usize];
            [0, 1, 2].map(|c| ((s.iter().map(|p| f64::from(p[c])).sum::<f64>()) / 4.0).round() as u8)
        }

        fn save(&self, path: &Path) {
            let rgba: Vec<u8> = (0..self.height).flat_map(|y| (0..self.width).map(move |x| (x, y))).flat_map(|(x, y)| {
                let [r, g, b] = self.pixel(x, y);
                [r, g, b, 255]
            }).collect();
            Image { width: self.width, height: self.height, rgba }.save_png(path).expect("the twin written");
        }
    }

    /// Today's radar draws (doc 02 §3.5, restated here rather than taken from the layer) on
    /// `twin`: `radarLayer` (each ground tile's every band at 0.55), then `radarLifted`: while the
    /// view is tilted, each height some tile in view reaches, lowest first, the tiles' slices from
    /// that height's band up raised to `elevation(km · 3280.84, scale)`, then their walls from the
    /// height below (the ground for the lowest), at `max(0.16, 0.34 − 0.03 i) · lift`.
    fn today(twin: &mut Twin, view: &ViewState, size: [f64; 2], seen: &Seen, exaggeration: f64) {
        let viewport = &ViewFrame::new(view, size, 1.0, 0.0).viewport;
        for tile in &seen.ground {
            twin.layer(false, false);
            twin.draw(viewport, &tile.mesh.slices, 0, |_| 0.0, 0.55);
        }
        let lift = js_round(clamp01((view.pitch - 10.0) / 30.0) * 10.0) / 10.0;
        if lift == 0.0 {
            return;
        }
        let scale = js_round(effective_exaggeration(exaggeration, view.zoom) * 10.0) / 10.0;
        let z = |km: f64| km * 3280.84 * 0.3048 * scale;
        for (i, &km) in RADAR_LEVELS_KM.iter().enumerate() {
            let reaching: Vec<(&RadarTile, usize)> = seen.in_view.iter().filter_map(|t| t.mesh.levels.get(i).map(|l| (&**t, l.from_band))).collect();
            if reaching.is_empty() {
                continue;
            }
            let opacity = (0.34 - i as f64 * 0.03).max(0.16) * lift;
            let (top, bottom) = (z(km), if i > 0 { z(RADAR_LEVELS_KM[i - 1]) } else { 0.0 });
            twin.layer(true, false);
            for &(tile, from_band) in &reaching {
                twin.draw(viewport, &tile.mesh.slices, from_band, |_| top, opacity);
            }
            twin.layer(true, true);
            for &(tile, from_band) in &reaching {
                if let Some(walls) = &tile.mesh.walls {
                    twin.draw(viewport, walls, from_band, |p| bottom + f64::from(p[2]) * (top - bottom), opacity);
                }
            }
        }
    }

    /// How the GPU's picture agrees with the twin's over the same base.
    #[derive(Debug, Default)]
    struct Agreement {
        /// Pixels the radar changed in either picture.
        changed: usize,
        /// Of those, within 2 levels of the twin.
        agreed: usize,
        /// Of the others, within 2 levels of the range of the twin's samples in the 3 × 3 pixels
        /// round them: an edge a fraction of a pixel off (the GPU's rasteriser snaps to its own
        /// grid and projects in `f32`), its two sides mixed in other shares.
        near: usize,
        /// The worst difference from the twin.
        worst: i32,
        /// Agreed pixels under two or more lifted layers, under walls, and under lifted layers
        /// alone (beyond the ground radar).
        stacked: usize,
        walled: usize,
        lifted_only: usize,
    }

    fn agreement(image: &Image, base: &Image, twin: &Twin) -> Agreement {
        let mut a = Agreement::default();
        for y in 0..image.height {
            for x in 0..image.width {
                let b = base.pixel(x, y);
                let (want, got) = (twin.pixel(x, y), image.pixel(x, y));
                if want == [b[0], b[1], b[2]] && got == b {
                    continue;
                }
                a.changed += 1;
                let off = (0..3).map(|c| (i32::from(got[c]) - i32::from(want[c])).abs()).max().unwrap();
                a.worst = a.worst.max(off);
                if off > 2 {
                    let (xs, ys) = (x.saturating_sub(1)..=(x + 1).min(image.width - 1), y.saturating_sub(1)..=(y + 1).min(image.height - 1));
                    let round: Vec<[u8; 3]> = ys.flat_map(|j| xs.clone().map(move |i| (i, j))).flat_map(|(i, j)| twin.samples[(j * twin.width + i) as usize]).collect();
                    let near = (0..3).all(|c| {
                        let (lo, hi) = round.iter().fold((255u8, 0u8), |(lo, hi), p| (lo.min(p[c]), hi.max(p[c])));
                        (i32::from(lo) - 2..=i32::from(hi) + 2).contains(&i32::from(got[c]))
                    });
                    a.near += usize::from(near);
                    continue;
                }
                a.agreed += 1;
                let i = (y * twin.width + x) as usize;
                a.stacked += usize::from(twin.lifted[i] >= 2);
                a.walled += usize::from(twin.walled[i]);
                a.lifted_only += usize::from(twin.lifted[i] > 0 && !twin.grounded[i]);
            }
        }
        a
    }

    /// Whether `a` says the GPU drew what the twin did: `exact` per cent of the pixels either
    /// changed within 2 levels (fewer where the stack is dense with edges, as at V2), and all but
    /// one in two hundred within the twin's range round them.
    fn agrees(a: &Agreement, exact: usize) -> bool {
        a.agreed * 100 >= a.changed * exact && (a.agreed + a.near) * 200 >= a.changed * 199
    }

    /// Pixels where `a` and `b` differ, and of those where `b` is `c`.
    fn differ(a: &Image, b: &Image, c: &Image) -> (usize, usize) {
        let mut out = (0, 0);
        for (i, (pa, pb)) in a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).enumerate() {
            if pa != pb {
                out.0 += 1;
                out.1 += usize::from(*pb == c.rgba[i * 4..i * 4 + 4]);
            }
        }
        out
    }

    /// A storm cell as Universal Blue paints it, in the middle of a 256 px tile: 60 dBZ in the
    /// core (pink, its echo top past 12 km), then rings of 46, 26 and 16 dBZ out to 60 px.
    fn storm_png() -> Vec<u8> {
        let mut picture = image::RgbaImage::new(256, 256);
        for (x, y, p) in picture.enumerate_pixels_mut() {
            let r = ((f64::from(x) + 0.5 - 128.0).powi(2) + (f64::from(y) + 0.5 - 128.0).powi(2)).sqrt();
            *p = image::Rgba(if r < 12.0 {
                [255, 0, 255, 255]
            } else if r < 26.0 {
                [255, 130, 0, 255]
            } else if r < 44.0 {
                [30, 100, 200, 255]
            } else if r < 60.0 {
                [140, 160, 200, 255]
            } else {
                [0, 0, 0, 0]
            });
        }
        let mut bytes = Vec::new();
        picture.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).expect("a PNG");
        bytes
    }

    /// One storm cell in a RainViewer tile over ink, the radar layer drawing it: tilted 55° it
    /// stands up as today's stack (every height from 1.5 to 12 km, slices and walls level by
    /// level at their opacities, checked pixel by pixel against the CPU twin of today's draws),
    /// at 25° half as opaque, at 10° the ground alone with the arenas freed; without a frame
    /// nothing, and with it back the stack again.
    #[test]
    fn a_storm_cell_stands_up_as_today_s_stack() {
        let Some(gpu) = gpu() else { return };
        // the Rhine-Main area, z7
        let tile = TileIndex::new(67, 43, 7);
        let png = storm_png();
        let fetcher: Arc<dyn TileFetcher> = Arc::new(FnFetcher(move |source: &str, t: TileIndex| Ok((source == io::RAINVIEWER && t == tile).then(|| png.clone()))));
        let exaggeration = 4.0;
        let (mut renderer, control, seen) = probed(&gpu, fetcher, FRAME, exaggeration);
        let [west, _, east, _] = tile_bounds(rustradar_scene::radar::TileIndex { x: tile.x, y: tile.y, z: tile.z });
        let centre = ViewState { longitude: (west + east) / 2.0, latitude: tile_latitude(f64::from(tile.y) + 0.5, tile.z), zoom: 7.2, pitch: 55.0, bearing: 0.0, position: None };
        let size = [600.0, 500.0];
        let base = shot(&gpu, &mut renderer, &control, centre, size, false, Show::Nothing);
        for (pitch, name) in [(55.0, "w3-storm.png"), (25.0, "w3-storm-half.png"), (10.0, "w3-storm-pitch10.png")] {
            let view = ViewState { pitch, ..centre };
            let image = shot(&gpu, &mut renderer, &control, view, size, false, Show::All);
            image.save_png(&shots_dir().join(name)).expect("the shot written");
            let s = seen.borrow().clone();
            assert_eq!((s.ground.len(), s.in_view.len()), (1, 1), "the storm's tile alone has echo");
            assert_eq!(s.in_view[0].mesh.levels.len(), RADAR_LEVELS_KM.len(), "the core reaches 12 km");
            let mut twin = Twin::over(&base);
            today(&mut twin, &view, size, &s, exaggeration);
            twin.save(&shots_dir().join(name.replace(".png", "-twin.png")));
            let a = agreement(&image, &base, &twin);
            eprintln!("storm at {pitch}°: {a:?}; {} lifted layers", s.drawn.len());
            assert!(a.changed > 20_000, "{a:?}");
            assert!(agrees(&a, 95), "{a:?}");
            if pitch == 10.0 {
                // looking down: no stack and no arenas; the ground layer's picture to the pixel
                assert!(s.drawn.is_empty() && s.levels == 0, "{:?}", s.drawn);
                let ground = shot(&gpu, &mut renderer, &control, view, size, false, Show::Ground);
                assert_eq!(image.rgba, ground.rgba);
                assert_eq!(a.stacked + a.walled + a.lifted_only, 0, "{a:?}");
            } else {
                let lift = lift_for(pitch);
                let ids: Vec<String> = s.drawn.iter().map(LayerSpec::id).collect();
                assert_eq!(ids.len(), 14, "{ids:?}");
                assert_eq!((ids[0].as_str(), ids[1].as_str(), ids[13].as_str()), ("radar-lift-1.5", "radar-wall-1.5", "radar-wall-12"));
                assert!(s.drawn.iter().all(|l| l.opacity == level_opacity(l.level, lift)));
                assert_eq!(s.levels, RADAR_LEVELS_KM.len());
                // the stack: layers over layers, walls between them, and much of it off the
                // ground's footprint (it rises)
                assert!(a.stacked > 5_000 && a.walled > 2_000 && a.lifted_only > 1_000, "{a:?}");
            }
        }
        // no frame: the arenas are freed and nothing is drawn
        control.borrow_mut().input.tiles.url = None;
        let image = shot(&gpu, &mut renderer, &control, centre, size, false, Show::All);
        assert!(seen.borrow().drawn.is_empty() && seen.borrow().levels == 0);
        assert_eq!(image.rgba, base.rgba);
        // the frame back: the stack from the tiles in view again
        control.borrow_mut().input.tiles.url = Some(FRAME.into());
        shot(&gpu, &mut renderer, &control, centre, size, false, Show::All);
        assert_eq!((seen.borrow().drawn.len(), seen.borrow().levels), (14, RADAR_LEVELS_KM.len()));
    }

    /// The acceptance shot: V2 pitched 45° over the dark map with the committed RainViewer frame
    /// and today's height setting (10). The stack stands over the ground radar: every height,
    /// slices then walls, translucent, matching the CPU twin of today's draws (the ground at 0.55,
    /// then each height at `max(0.16, 0.34 − 0.03 i)`) pixel by pixel but where its many edges
    /// fall a fraction of a pixel apart; much of it off the ground's footprint. At pitch 10 none:
    /// the ground layer's picture, the arenas freed.
    #[test]
    fn v2_pitched_45_stacks_translucent_layers_with_walls_and_none_at_pitch_10() {
        let Some(gpu) = gpu() else { return };
        let exaggeration = 10.0;
        let (mut renderer, control, seen) = probed(&gpu, Arc::new(DirTiles::fixtures()), FRAME, exaggeration);
        let size = [1100.0, 720.0];
        let pitched = ViewState { latitude: 52.0, longitude: 10.0, zoom: 4.5, pitch: 45.0, bearing: 0.0, position: None };
        let image = shot(&gpu, &mut renderer, &control, pitched, size, true, Show::All);
        let s = seen.borrow().clone();
        let ground = shot(&gpu, &mut renderer, &control, pitched, size, true, Show::Ground);
        let base = shot(&gpu, &mut renderer, &control, pitched, size, true, Show::Nothing);
        image.save_png(&shots_dir().join("w3-v2-pitched.png")).expect("the shot written");
        ground.save_png(&shots_dir().join("w3-v2-pitched-ground.png")).expect("the shot written");
        let ids: Vec<String> = s.drawn.iter().map(LayerSpec::id).collect();
        let want: Vec<String> = RADAR_LEVELS_KM.iter().flat_map(|&km| [format!("radar-lift-{}", js_string(km)), format!("radar-wall-{}", js_string(km))]).collect();
        assert_eq!(ids, want);
        assert!(s.in_view.len() >= 10, "{} tiles in view", s.in_view.len());
        let mut twin = Twin::over(&base);
        today(&mut twin, &pitched, size, &s, exaggeration);
        twin.save(&shots_dir().join("w3-v2-pitched-twin.png"));
        let a = agreement(&image, &base, &twin);
        let (lifted, beyond) = differ(&image, &ground, &base);
        eprintln!("V2 pitched 45: {a:?}; the stack changes {lifted} px, {beyond} of them off the ground radar");
        assert!(agrees(&a, 85), "{a:?}");
        assert!(a.stacked > 50_000 && a.walled > 50_000 && a.lifted_only > 20_000, "{a:?}");
        assert!(lifted > 50_000 && beyond * 5 > lifted, "{lifted}, {beyond}");

        // pitch 10: nothing lifted, the arenas freed
        let flat = ViewState { pitch: 10.0, ..pitched };
        let image = shot(&gpu, &mut renderer, &control, flat, size, true, Show::All);
        let s = seen.borrow().clone();
        image.save_png(&shots_dir().join("w3-v2-pitch10.png")).expect("the shot written");
        assert!(s.drawn.is_empty() && s.levels == 0, "{:?}", s.drawn);
        let ground = shot(&gpu, &mut renderer, &control, flat, size, true, Show::Ground);
        assert_eq!(image.rgba, ground.rgba, "at pitch 10 the ground alone");
    }
}
