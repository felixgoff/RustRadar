//! Item W5's shots: the weather radar drawn by the whole app (`--screenshot`'s path: the radar
//! state asking the `--fixture` folder, the scene builder, the map's radar layer) from the
//! committed fixtures (`--fixture crates/map/tests/data`: RainViewer's frame list and tiles, the
//! recorded MRMS volume), each beside the same shot with the weather off.
//!
//! - V2 (Europe, zoom 4.5): RainViewer's frame. Wherever the radar is one colour it is one of the
//!   NWS bands at 55 % over the map below it (W2's ground layer), or a few bands stacked where a
//!   tile's own mesh covers a place twice (as today's does, W2's finding).
//! - The Ohio Valley (39 N, 84 W, zoom 5.3): the recorded MRMS scan, the bands at 70 % (W4's
//!   ground layer). RainViewer's fixture tiles do not reach there, so only the measured layer
//!   draws.
//! - With the weather off neither changes a pixel of the map.
//! - V2 with a measured volume over England and Wales that sees the north-west quarter of its
//!   bounds (a 2 × 2 coverage mask, north row first) and no echo there: RainViewer stands back in
//!   that quarter alone (`setRadarCoverage`, doc 02 §14.2 item 1), and tiles far from it are drawn
//!   as without the volume.
//!
//! The night is off in these shots: drawn over the radar (row 6 after rows 5a and 5b) it would
//! tint the bands. There is no reference picture with the radar on; the map under it is
//! `docs/port/reference/dark-v2-1440x900.png`'s. Needs a wgpu adapter (SwiftShader or lavapipe
//! through `VK_ICD_FILENAMES`); without one it prints `skipped: …` and passes. Shots go to
//! `target/shots/w5-*.png`.

mod common;

use std::sync::Arc;

use common::shots::{V2, gpu, save};
use common::{app_shot_with, options};
use rustradar_app::Options;
use rustradar_app::state::AppState;
use rustradar_app::state::aircraft::Answer;
use rustradar_app::state::radar::{Arrived, FixtureFeeds, Radar, RadarFeeds};
use rustradar_backend::rainviewer::Frame;
use rustradar_core::geo::{Area, Basemap};
use rustradar_core::model::{RadarBounds, RadarVolume};
use rustradar_map::headless::Image;
use rustradar_map::io;
use rustradar_map::layers::radar::measured::{Volume, VolumeKey};
use rustradar_scene::camera::{ViewState, viewport_for};
use rustradar_scene::radar::{BAND_COUNT, band_color};

const SIZE: [u32; 2] = [1100, 720];

/// The committed fixtures' folder, as `--fixture` takes it.
fn fixtures() -> Options {
    Options { fixture: Some(io::fixture_tiles_dir().parent().expect("the data folder").to_owned()), ..Default::default() }
}

/// The app's shot at `view` from the committed fixtures with the weather `on` or off, the night
/// off.
fn shot(gpu: &rustradar_map::Gpu, view: ViewState, on: bool) -> Image {
    shot_with(gpu, view, on, |_| {})
}

/// [`shot`] with `setup` given the app's state after the weather and the night are set.
fn shot_with(gpu: &rustradar_map::Gpu, view: ViewState, on: bool, setup: impl FnOnce(&mut AppState)) -> Image {
    let options = Options { fixture: fixtures().fixture, ..options(view, Basemap::Dark, SIZE, 1.0) };
    app_shot_with(gpu, &options, |state| {
        state.persisted.settings.weather = on;
        state.persisted.settings.daylight = false;
        setup(state);
    })
}

/// The bounds of [`Covered`]'s volume, `[west, south, east, north]`: England and Wales.
const COVERED: [f64; 4] = [-6.0, 50.0, 2.0, 54.0];

/// The `--fixture` folder's RainViewer frame, and as the measured volumes one volume over
/// [`COVERED`] that sees its north-west quarter (mask `[1, 0, 0, 0]`, north row first) and no echo
/// in it.
struct Covered(FixtureFeeds);

impl RadarFeeds for Covered {
    fn frame(&mut self) -> Answer<Result<Option<Frame>, String>> {
        self.0.frame()
    }

    fn measures(&self) -> bool {
        true
    }

    fn volumes(&mut self, _: Option<Area>, held: Vec<VolumeKey>) -> Answer<Result<Vec<Arrived>, String>> {
        let [west, south, east, north] = COVERED;
        let volume = Volume {
            frame: RadarVolume {
                source: "test".into(),
                time: 1_794_446_561,
                bounds: RadarBounds { west, south, east, north },
                levels: vec![],
                coverage_width: 2,
                coverage_height: 2,
            },
            coverage: vec![1, 0, 0, 0],
            ..Default::default()
        };
        let key = volume.key();
        Answer::ready(Ok(vec![if held.contains(&key) { Arrived::Held(key) } else { Arrived::New(Arc::new(volume)) }]))
    }
}

/// `[r, g, b, a]` drawn at `opacity` over `under`.
fn over(under: [u8; 4], [r, g, b, a]: [u8; 4], opacity: f64) -> [f64; 3] {
    let alpha = f64::from(a) / 255.0 * opacity;
    [0, 1, 2].map(|i| f64::from(under[i]) + (f64::from([r, g, b][i]) - f64::from(under[i])) * alpha)
}

/// Whether the 3 × 3 pixels round `(x, y)` are one colour (no edge there).
fn uniform(image: &Image, x: u32, y: u32) -> bool {
    let c = image.pixel(x, y);
    (x - 1..=x + 1).all(|i| (y - 1..=y + 1).all(|j| image.pixel(i, j) == c))
}

/// Whether `got` is some stack of one to six bands, weakest first, drawn at `opacity` over
/// `under` (within 2 levels).
fn stacked(under: [u8; 4], got: [u8; 4], opacity: f64, from: usize, left: usize) -> bool {
    (from..BAND_COUNT).any(|band| {
        let c = over(under, band_color(band), opacity);
        let next = [c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255];
        let off = (0..3).map(|i| (i32::from(next[i]) - i32::from(got[i])).abs()).max().unwrap();
        off <= 2 || (left > 1 && stacked(next, got, opacity, band + 1, left - 1))
    })
}

/// What the radar changed between `base` (the weather off) and `image`: pixels changed, those
/// inside a band (no edge in either), those of one band at `opacity` over the map, those of a
/// stack of bands, and the bands seen.
struct Changes {
    changed: usize,
    inside: usize,
    one_band: usize,
    stacked: usize,
    bands: [usize; BAND_COUNT],
}

fn changes(base: &Image, image: &Image, opacity: f64) -> Changes {
    let mut c = Changes { changed: 0, inside: 0, one_band: 0, stacked: 0, bands: [0; BAND_COUNT] };
    for y in 1..image.height - 1 {
        for x in 1..image.width - 1 {
            let (before, after) = (base.pixel(x, y), image.pixel(x, y));
            if before == after {
                continue;
            }
            c.changed += 1;
            if !uniform(image, x, y) || !uniform(base, x, y) {
                continue;
            }
            c.inside += 1;
            let off = |want: [f64; 3]| (0..3).map(|i| (want[i] - f64::from(after[i])).abs()).fold(0.0, f64::max);
            let (miss, band) = (0..BAND_COUNT).map(|band| (off(over(before, band_color(band), opacity)), band)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
            if miss <= 1.5 {
                c.one_band += 1;
                c.bands[band] += 1;
            } else if stacked(before, after, opacity, 0, 6) {
                c.stacked += 1;
            }
        }
    }
    c
}

/// V2 with the weather on: RainViewer's bands at 55 % over the dark map, from light rain to heavy;
/// with it off, not a pixel of radar.
#[test]
fn v2_draws_rainviewer_from_the_fixture_with_the_weather_on() {
    let Some(gpu) = gpu() else { return };
    let base = shot(&gpu, V2.view(), false);
    let image = shot(&gpu, V2.view(), true);
    save(&base, "w5-v2-weather-off.png");
    save(&image, "w5-v2-weather-on.png");
    let c = changes(&base, &image, 0.55);
    eprintln!("V2: {} px changed, {} inside a band, {} one band, {} stacked; by band {:?}", c.changed, c.inside, c.one_band, c.stacked, c.bands);
    assert!(c.changed > 50_000, "{} px of radar", c.changed);
    assert!((c.one_band + c.stacked) * 100 >= c.inside * 97, "{} + {} of {} inside are bands at 55 %", c.one_band, c.stacked, c.inside);
    assert!(c.bands[..9].iter().all(|&n| n > 0), "cyan to orange: {:?}", c.bands);
    // the weather off again is the map alone: the same as a shot that never had it on
    let again = shot(&gpu, V2.view(), false);
    assert_eq!(again.rgba, base.rgba, "the weather off draws no radar");
}

/// Over the Ohio Valley the recorded MRMS scan: its bands at 70 % over the dark map.
#[test]
fn the_ohio_valley_draws_the_recorded_mrms_scan() {
    let Some(gpu) = gpu() else { return };
    let ohio = ViewState { latitude: 39.0, longitude: -84.0, zoom: 5.3, pitch: 0.0, bearing: 0.0, position: None };
    let base = shot(&gpu, ohio, false);
    let image = shot(&gpu, ohio, true);
    save(&base, "w5-ohio-weather-off.png");
    save(&image, "w5-ohio-weather-on.png");
    let c = changes(&base, &image, 0.7);
    eprintln!("Ohio: {} px changed, {} inside a band, {} one band, {} stacked; by band {:?}", c.changed, c.inside, c.one_band, c.stacked, c.bands);
    assert!(c.changed > 20_000, "{} px of radar", c.changed);
    // (W4 found places today's meshing covers twice: those show stacked bands)
    assert!((c.one_band + c.stacked) * 100 >= c.inside * 97, "{} + {} of {} inside are bands at 70 %", c.one_band, c.stacked, c.inside);
    assert!(c.bands.iter().filter(|&&n| n > 0).count() >= 5, "several bands: {:?}", c.bands);
}

/// V2 with [`Covered`]'s volume: in the quarter it sees RainViewer stands back (not a pixel of
/// radar where the plain V2 shot has plenty), in the three it does not see RainViewer is drawn as
/// before, and far from it (east of 22.5° E, past every tile it could touch from zoom 4 up) the
/// picture is the plain shot's.
#[test]
fn rainviewer_stands_back_where_a_measured_volume_sees() {
    let Some(gpu) = gpu() else { return };
    let view = V2.view();
    let base = shot(&gpu, view, false);
    let plain = shot(&gpu, view, true);
    let dir = fixtures().fixture.expect("the fixture folder");
    let covered = shot_with(&gpu, view, true, |state| state.radar = Radar::with_feeds(Some(Box::new(Covered(FixtureFeeds::new(dir))))));
    save(&covered, "w5-v2-covered.png");
    let viewport = viewport_for(&view, SIZE.map(f64::from));
    let [west, south, east, north] = COVERED;
    let (middle_lon, middle_lat) = ((west + east) / 2.0, (south + north) / 2.0);
    // clear of the mask's cell edges by more than the blur and the contours reach (5 or more of a
    // tile's pixels from zoom 4 up)
    const MARGIN: f64 = 0.5;
    let within = |low: f64, high: f64, v: f64| v > low + MARGIN && v < high - MARGIN;
    // `[seen in the plain shot, drawn now, drawn now as in the plain shot]`, radar pixels each
    let (mut seen, mut unseen, mut far) = ([0usize; 3], [0usize; 3], [0usize; 3]);
    for y in 0..covered.height {
        for x in 0..covered.width {
            let [lon, lat] = viewport.unproject([f64::from(x) + 0.5, f64::from(y) + 0.5], 0.0);
            let under = base.pixel(x, y);
            let (was, now) = (plain.pixel(x, y), covered.pixel(x, y));
            let west_half = within(west, middle_lon, lon);
            let east_half = within(middle_lon, east, lon);
            let north_half = within(middle_lat, north, lat);
            let south_half = within(south, middle_lat, lat);
            let count = if west_half && north_half {
                &mut seen
            } else if (west_half || east_half) && (north_half || south_half) {
                &mut unseen
            } else if lon > 22.5 {
                &mut far
            } else {
                continue;
            };
            if was != under {
                count[0] += 1;
            }
            if now != under {
                count[1] += 1;
                if now == was {
                    count[2] += 1;
                }
            }
        }
    }
    eprintln!("covered: the quarter seen {seen:?}, the three unseen {unseen:?}, far east {far:?} ([radar before, radar now, as before])");
    assert!(seen[0] > 2_000, "the plain shot has RainViewer in the quarter seen: {seen:?}");
    assert_eq!(seen[1], 0, "no RainViewer where the volume sees");
    assert!(unseen[0] > 5_000, "the plain shot has RainViewer in the quarters not seen: {unseen:?}");
    assert!(unseen[2] * 100 >= unseen[0] * 99 && unseen[1] <= unseen[0] + unseen[0] / 100, "RainViewer as before where the volume does not see: {unseen:?}");
    assert!(far[0] > 5_000, "{far:?}");
    assert_eq!((far[1], far[2]), (far[0], far[0]), "far from the volume nothing changes");
}
