//! Markers: the selected flight's route airports, the focused airport, the searched place (rows
//! 22–24 of doc 03 §3), and the selected aircraft's ring (row 30), as today's `airportMarkers`
//! (G:1480-1517), `placeMarker` (G:1520-1556) and the `selection-ring` (G:3037-3055) draw them
//! with deck.gl 9.4's ScatterplotLayer and TextLayer (item A8; doc 03 §8.3–8.4, doc 02 §10.2–10.3).
//!
//! | Layer | Style | Draw state |
//! |---|---|---|
//! | `{id}-dots` (`route-airports`, then `focus-airport`) | billboard dot, radius 5 px, fill [`MARKER_FILL`], 2 px line [`MARKER_LINE`] | [`circle::MARKER`] |
//! | `{id}-codes` | the IATA code (else the ICAO), 13 px, Overpass 700, `[241,238,231,255]`, 16 px above the dot, SDF halo 3 in `LABEL_OUTLINE`, not scaled by the label size | `LABEL` |
//! | `place-dot` | billboard dot, radius 4 px, fill `[241,238,231,255]`, 2 px line [`MARKER_LINE`] | [`circle::MARKER`] |
//! | `place-name` | the name, 12 px, Overpass 600, `[241,238,231,230]`, 14 px above the dot, the same halo | `LABEL` |
//! | `selection-ring` | billboard ring at the aircraft's drawn position, radius `screenPx(sizeOf(icon)) · 0.6 + 4` px, not filled, 1.5 px line [`RING_COLOR`] | [`circle::MARKER`] |
//!
//! Drawn in that order, each dot layer before its text layer; the ring after the aircraft (its own
//! [`SelectionRing`] layer). The dots and the ring draw through everything, the globe included
//! (`depthCompare: "always"`), and nothing culls them on the far side of the globe: today's
//! behaviour (the scene builder leaves the ring out on the far hemisphere, as `buildLayers` does).
//! deck's TextLayer defaults: centred on the anchor (`getTextAnchor: "middle"`,
//! `getAlignmentBaseline: "center"`), billboards, sizes in pixels.
//!
//! Every circle and glyph is placed at each frame for that frame's camera (a few of them).

use rustradar_scene::text::{Face, Family, FontSettings, GlyphAtlas, LaidText, TextLayout};

use super::{DrawCx, Init, Layer, Pass, PrepareCx};
use crate::pipelines::circle::{self, Circle, CircleStyle, MARKER};
use crate::pipelines::text::{AtlasTexture, BackgroundStyle, Glyph, Label, TextBatch, TextPipeline, TextStyle};
use crate::state::DrawState;

/// `MARKER_FILL` (G:1478): the airport dots.
pub const MARKER_FILL: [u8; 4] = [55, 142, 240, 255];
/// `MARKER_LINE` (G:1479): the airport and place dots' outline.
pub const MARKER_LINE: [u8; 4] = [6, 12, 19, 230];
/// The airport dots' radius and line width, px.
pub const AIRPORT_DOT_RADIUS: f64 = 5.0;
pub const DOT_LINE_WIDTH: f64 = 2.0;
/// The airport codes: size px, colour, pixel offset, weight.
pub const CODE_SIZE: f64 = 13.0;
pub const CODE_COLOR: [u8; 4] = [241, 238, 231, 255];
pub const CODE_OFFSET: [f64; 2] = [0.0, -16.0];
pub const CODE_FACE: Face = Face::new(Family::Overpass, 700);
/// The searched place's dot: radius px, fill.
pub const PLACE_DOT_RADIUS: f64 = 4.0;
pub const PLACE_FILL: [u8; 4] = [241, 238, 231, 255];
/// The searched place's name: size px, colour, pixel offset, weight.
pub const PLACE_NAME_SIZE: f64 = 12.0;
pub const PLACE_NAME_COLOR: [u8; 4] = [241, 238, 231, 230];
pub const PLACE_NAME_OFFSET: [f64; 2] = [0.0, -14.0];
pub const PLACE_FACE: Face = Face::LABEL;
/// The selection ring's line: colour (G:3047) and width, px.
pub const RING_COLOR: [u8; 4] = [55, 142, 240, 230];
pub const RING_WIDTH: f64 = 1.5;

/// An airport marker: where, and the code shown (`iata || icao`).
#[derive(Debug, Clone, PartialEq)]
pub struct AirportMark {
    pub lon: f64,
    pub lat: f64,
    pub code: String,
}

/// The searched place's marker.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceMark {
    pub lon: f64,
    pub lat: f64,
    pub name: String,
}

/// The selected aircraft's ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    /// `at(selectedPose, ex)`: `[lon, lat, metres]`, the drawn (not wheel-raised) position.
    pub position: [f64; 3],
    /// `screenPx(sizeOf(selected.icon)) · 0.6 + 4`, px.
    pub radius_px: f64,
}

/// What the scene builder hands these layers (`SceneInput`'s field of the same name). The default
/// draws nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {
    /// `route-airports`: the selected flight's origin and destination that are known, in that
    /// order (both even when they are the same airport).
    pub route_airports: Vec<AirportMark>,
    /// `focus-airport`: the airport whose board is open.
    pub focus_airport: Option<AirportMark>,
    /// The searched place, marked until dismissed.
    pub place: Option<PlaceMark>,
    /// The ring round the selected aircraft, when it is drawn.
    pub ring: Option<Ring>,
}

/// One dots layer and its text layer.
struct Group {
    dots: circle::Instances,
    text: TextBatch,
}

/// The dots and texts of one group for this frame's camera: `marks` as `(lon, lat, text)`.
fn group(
    cx: &PrepareCx<'_>,
    atlas: &mut GlyphAtlas,
    marks: &[(f64, f64, &str)],
    dot: (f64, [u8; 4]),
    text: (Face, f64, [u8; 4], [f64; 2]),
) -> (Vec<circle::Instance>, Vec<Glyph>) {
    let (radius, fill) = dot;
    let (face, size, color, offset) = text;
    let dots = marks.iter().map(|&(lon, lat, _)| circle::Instance::at(cx.view, [lon, lat], 0.0).radius(radius).fill(fill).line(MARKER_LINE, DOT_LINE_WIDTH)).collect();
    let laid: Vec<LaidText> = lay_out(atlas, face, marks.iter().map(|m| m.2));
    let glyphs = marks
        .iter()
        .zip(&laid)
        .flat_map(|(&(lon, lat, _), text)| {
            let label = Label::at(cx.view, [lon, lat, 0.0], size, color).with_pixel_offset(offset);
            label.glyphs(text).collect::<Vec<_>>()
        })
        .collect();
    (dots, glyphs)
}

/// `texts` laid out in `atlas` in `face`, again from scratch if the atlas packed itself anew
/// meanwhile (which moves the glyphs).
fn lay_out<'a>(atlas: &mut GlyphAtlas, face: Face, texts: impl Iterator<Item = &'a str> + Clone) -> Vec<LaidText> {
    let mut laid = Vec::new();
    for _ in 0..4 {
        let generation = atlas.generation();
        laid = texts.clone().map(|t| atlas.layout(face, t, &TextLayout::DEFAULT)).collect();
        if atlas.generation() == generation {
            break;
        }
    }
    laid
}

/// Rows 22–24: route airports, focused airport, searched place.
pub struct Markers {
    dots: Circle,
    dot_style: circle::Style,
    atlas: GlyphAtlas,
    texture: AtlasTexture,
    text: TextPipeline,
    /// `route-airports`, `focus-airport`, `place`.
    groups: [Group; 3],
}

impl Markers {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let dots = Circle::new(init, "marker dots", MARKER);
        let dot_style = dots.style(device, &CircleStyle { billboard: true, filled: true, stroked: true, ..CircleStyle::PIXELS });
        let mut atlas = GlyphAtlas::new(FontSettings::LABELS);
        let texture = AtlasTexture::new(init.gpu, "marker glyphs", &mut atlas);
        let text = TextPipeline::new(init, "marker texts", DrawState::LABEL);
        let groups = ["route airports", "focus airport", "place"].map(|label| Group { dots: circle::Instances::new(device, label), text: text.batch(device, label, &texture) });
        Markers { dots, dot_style, atlas, texture, text, groups }
    }

    /// How many dots and glyphs each group draws this frame (for tests): route airports, focused
    /// airport, place.
    pub fn counts(&self) -> [(u32, u32); 3] {
        self.groups.each_ref().map(|g| (g.dots.count(), g.text.len()))
    }
}

impl Layer for Markers {
    fn name(&self) -> &'static str {
        "markers"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let input = &cx.scene.markers;
        fn airport(a: &AirportMark) -> (f64, f64, &str) {
            (a.lon, a.lat, a.code.as_str())
        }
        let route: Vec<_> = input.route_airports.iter().map(airport).collect();
        let focus: Vec<_> = input.focus_airport.iter().map(airport).collect();
        let place: Vec<_> = input.place.iter().map(|p| (p.lon, p.lat, p.name.as_str())).collect();
        let airport_dot = (AIRPORT_DOT_RADIUS, MARKER_FILL);
        let code = (CODE_FACE, CODE_SIZE, CODE_COLOR, CODE_OFFSET);
        let made = [
            group(cx, &mut self.atlas, &route, airport_dot, code),
            group(cx, &mut self.atlas, &focus, airport_dot, code),
            group(cx, &mut self.atlas, &place, (PLACE_DOT_RADIUS, PLACE_FILL), (PLACE_FACE, PLACE_NAME_SIZE, PLACE_NAME_COLOR, PLACE_NAME_OFFSET)),
        ];
        self.texture.sync(cx.gpu, &mut self.atlas);
        let (device, queue) = (&cx.gpu.device, &cx.gpu.queue);
        for (g, (dots, glyphs)) in self.groups.iter_mut().zip(made) {
            g.dots.write(device, queue, &dots);
            // not scaled by the label size setting
            g.text.set_style(queue, &TextStyle::LABELS, &BackgroundStyle::DEFAULT, &FontSettings::LABELS, cx.view);
            g.text.set_glyphs(device, queue, &glyphs);
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass != Pass::Map {
            return;
        }
        for g in &self.groups {
            self.dots.draw(pass, &self.dot_style, &g.dots);
            if !g.text.is_empty() {
                self.text.draw(pass, &g.text);
            }
        }
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Offset
    }
}

/// Row 30: `selection-ring`, drawn after the aircraft.
pub struct SelectionRing {
    ring: Circle,
    style: circle::Style,
    instances: circle::Instances,
}

impl SelectionRing {
    pub fn new(init: &Init<'_>) -> Self {
        let device = &init.gpu.device;
        let ring = Circle::new(init, "selection ring", MARKER);
        let style = ring.style(device, &CircleStyle { billboard: true, filled: false, stroked: true, ..CircleStyle::PIXELS });
        SelectionRing { ring, style, instances: circle::Instances::new(device, "selection ring") }
    }

    /// Whether the ring is drawn this frame (for tests).
    pub fn drawn(&self) -> bool {
        !self.instances.is_empty()
    }
}

impl Layer for SelectionRing {
    fn name(&self) -> &'static str {
        "selection-ring"
    }

    fn prepare(&mut self, cx: &mut PrepareCx<'_>) {
        let ring: Vec<circle::Instance> = cx
            .scene
            .markers
            .ring
            .iter()
            .map(|r| {
                let [lon, lat, h] = r.position;
                circle::Instance::at(cx.view, [lon, lat], h).radius(r.radius_px).line(RING_COLOR, RING_WIDTH)
            })
            .collect();
        self.instances.write(&cx.gpu.device, &cx.gpu.queue, &ring);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, cx: &DrawCx<'_>) {
        if cx.pass == Pass::Map {
            self.ring.draw(pass, &self.style, &self.instances);
        }
    }

    fn drape(&self) -> super::Drape {
        super::Drape::Offset
    }
}

#[cfg(test)]
mod tests {
    use rustradar_core::geo::Basemap;
    use rustradar_scene::camera::{ViewState, viewport_for};

    use super::*;
    use crate::gpu::testing::gpu;
    use crate::headless::{Image, render_with};
    use crate::renderer::{MapRenderer, SceneInput};
    use crate::headless::FORMAT;

    const SIZE: [f64; 2] = [640.0, 400.0];

    fn v() -> ViewState {
        ViewState { longitude: 4.86, latitude: 52.315, zoom: 10.0, pitch: 0.0, bearing: 0.0, position: None }
    }

    fn screen(view: ViewState, [lon, lat, h]: [f64; 3]) -> [f64; 2] {
        let p = viewport_for(&view, SIZE).project([lon, lat, h]);
        [p[0], p[1]]
    }

    fn render(gpu: &crate::gpu::Gpu, view: ViewState, input: Input) -> Image {
        let mut scene = SceneInput::new(view, SIZE, 1.0, Basemap::Dark);
        scene.earth = crate::layers::earth::Input::SHOWN;
        scene.markers = input;
        let mut renderer = MapRenderer::new(gpu, FORMAT);
        render_with(gpu, &mut renderer, &scene, 1.0).expect("rendered")
    }

    fn save(image: &Image, name: &str) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/shots");
        std::fs::create_dir_all(dir).expect("shot folder");
        image.save_png(&std::path::Path::new(dir).join(name)).expect("png");
    }

    /// The pixels within `r` of `[x, y]` that `pick` keeps.
    fn count_near(image: &Image, [x, y]: [f64; 2], r: f64, pick: impl Fn([u8; 4]) -> bool) -> usize {
        let mut n = 0;
        for py in (y - r).max(0.0) as u32..((y + r) as u32).min(image.height) {
            for px in (x - r).max(0.0) as u32..((x + r) as u32).min(image.width) {
                if (f64::from(px) + 0.5 - x).hypot(f64::from(py) + 0.5 - y) <= r && pick(image.pixel(px, py)) {
                    n += 1;
                }
            }
        }
        n
    }

    /// The route's airport dots in `MARKER_FILL` with their codes above them in light text, the
    /// searched place's dot and name, and the ring of `RING_COLOR` round the aircraft.
    #[test]
    fn airports_places_and_the_ring() {
        let Some(gpu) = gpu() else { return };
        let ams = AirportMark { lon: 4.7639, lat: 52.3086, code: "AMS".into() };
        let input = Input {
            route_airports: vec![ams.clone()],
            focus_airport: Some(AirportMark { lon: 4.95, lat: 52.37, code: "XYZ".into() }),
            place: Some(PlaceMark { lon: 4.90, lat: 52.25, name: "Dam".into() }),
            ring: Some(Ring { position: [4.85, 52.33, 600.0], radius_px: 12.0 }),
        };
        let image = render(&gpu, v(), input);
        save(&image, "a8-markers.png");
        let dot = screen(v(), [ams.lon, ams.lat, 0.0]);
        let blue = |p: [u8; 4]| (0..3).all(|c| p[c].abs_diff(MARKER_FILL[c]) <= 3);
        let fill = count_near(&image, dot, 6.0, blue);
        // radius 5 with a 2 px line: the fill inside about 4 px
        assert!((35..=70).contains(&fill), "{fill} px of the dot's blue");
        let dark = count_near(&image, dot, 7.5, |p| p[0] < 40);
        assert!(dark > 20, "the dot's dark outline: {dark}");
        // the code above the dot: light pixels 16 px up, none below
        let above = count_near(&image, [dot[0], dot[1] - 16.0], 14.0, |p| p[0] > 200 && p[1] > 200);
        assert!(above > 20, "the code's letters: {above}");
        let below = count_near(&image, [dot[0], dot[1] + 16.0], 8.0, |p| p[0] > 200);
        assert_eq!(below, 0);
        // the focused airport the same way
        let focus = screen(v(), [4.95, 52.37, 0.0]);
        assert!(count_near(&image, focus, 6.0, blue) > 35);
        // the place's light dot and its name 14 px above
        let place = screen(v(), [4.90, 52.25, 0.0]);
        assert!(count_near(&image, place, 3.0, |p| p[0] > 200) >= 12, "the place dot");
        assert!(count_near(&image, [place[0], place[1] - 14.0], 12.0, |p| p[0] > 200) > 10, "its name");
        // the ring at its drawn height, a circle of radius about 12.75 px (outer edge 12 + 0.75)
        let ring = screen(v(), [4.85, 52.33, 600.0]);
        let on = |p: [u8; 4]| u16::from(p[2]) > u16::from(p[0]) + 60;
        let inside = count_near(&image, ring, 10.0, on);
        assert_eq!(inside, 0, "not filled");
        let line = count_near(&image, ring, 14.0, on);
        assert!((50..=130).contains(&line), "a 1.5 px ring of radius 12: {line}");
        // its colour where it covers a pixel fully: RING_COLOR over the land
        let expected = (0..3).map(|c| 50.0 + (f64::from(RING_COLOR[c]) - 50.0) * f64::from(RING_COLOR[3]) / 255.0).collect::<Vec<_>>();
        let mut best = [0u8; 4];
        for py in (ring[1] - 14.0) as u32..(ring[1] + 14.0) as u32 {
            for px in (ring[0] - 14.0) as u32..(ring[0] + 14.0) as u32 {
                let p = image.pixel(px, py);
                if p[2] > best[2] {
                    best = p;
                }
            }
        }
        for c in 0..3 {
            assert!((f64::from(best[c]) - expected[c]).abs() <= 3.0, "the ring's colour {best:?}, expected {expected:?}");
        }
    }

    /// The ring and the dots draw through the globe: a marker on the far side still shows.
    #[test]
    fn markers_draw_through_the_globe() {
        let Some(gpu) = gpu() else { return };
        let view = ViewState { longitude: 10.0, latitude: 0.0, zoom: 1.5, pitch: 0.0, bearing: 0.0, position: None };
        // just past the limb, behind the sphere
        let input = Input { route_airports: vec![AirportMark { lon: 102.0, lat: 0.0, code: "FAR".into() }], ..Default::default() };
        let image = render(&gpu, view, input);
        save(&image, "a8-markers-far-side.png");
        let vp = viewport_for(&view, SIZE);
        let p = vp.project([102.0, 0.0, 0.0]);
        let blue = |px: [u8; 4]| u16::from(px[2]) > u16::from(px[0]) + 100;
        assert!(count_near(&image, [p[0], p[1]], 7.0, blue) > 20, "drawn through the earth at {p:?}");
    }
}
